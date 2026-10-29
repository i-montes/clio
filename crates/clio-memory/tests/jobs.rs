#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use std::sync::Arc;

use clio_memory::*;
use common::*;

fn embedded() -> Fixture {
    fixture_with(Arc::new(HashEmbedder::new("m", 256)))
}

fn pending_embeds(f: &Fixture) -> usize {
    job_rows(&f.path)
        .iter()
        .filter(|(k, s)| k == "Embed" && s == "Pending")
        .count()
}

fn run(f: &Fixture) -> RunReport {
    f.engine.run_jobs(&RunOpts::default()).unwrap()
}

#[test]
fn t09_ten_updates_make_one_embed_job() {
    let f = embedded();
    for i in 0..10 {
        let mut p = prop(f.scope, f.person, &format!("vive en la ciudad {i}"));
        p.topic_key = Some("residencia".into());
        f.engine.propose(p).unwrap();
    }
    assert_eq!(
        job_rows(&f.path),
        [("Embed".to_string(), "Pending".to_string())]
    );
}

#[test]
fn t08_operation_and_job_are_written_together() {
    let f = embedded();
    let out = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    assert_eq!(f.engine.history(out.memory).unwrap().len(), 1);
    assert_eq!(pending_embeds(&f), 1);
    // a proposal that is rejected before anything is written leaves no job behind
    let before = job_rows(&f.path).len();
    assert!(f.engine.propose(prop(ScopeId(999), f.person, "x")).is_err());
    assert_eq!(job_rows(&f.path).len(), before);
}

#[test]
fn run_jobs_embeds_everything_pending() {
    let f = embedded();
    for c in [
        "tiene un coche rojo",
        "vive en Medellín",
        "prefiere respuestas breves",
    ] {
        f.engine.propose(prop(f.scope, f.person, c)).unwrap();
    }
    let report = run(&f);
    assert_eq!((report.done, report.failed), (1, 0));
    assert_eq!(
        f.engine.embed_pending(10).unwrap(),
        0,
        "nothing was left pending"
    );
    let r = f
        .engine
        .recall(f.scope, "auto", &RecallOpts::default())
        .unwrap();
    assert!(
        r.hits[0].vector.is_some(),
        "found by vector, so the vectors exist"
    );
}

#[test]
fn a_memory_added_after_the_job_ran_gets_its_own_job() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    run(&f);
    assert_eq!(pending_embeds(&f), 0);
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    assert_eq!(pending_embeds(&f), 1);
    run(&f);
    assert_eq!(f.engine.embed_pending(10).unwrap(), 0);
}

#[test]
fn no_embedder_means_no_embed_jobs() {
    let f = fixture();
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    assert!(job_rows(&f.path).is_empty());
}

#[test]
fn archive_enqueues_nothing() {
    let f = embedded();
    let m = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    run(&f);
    f.engine.archive(m.memory, f.person).unwrap();
    assert_eq!(pending_embeds(&f), 0);
}

#[test]
fn read_only_engine_cannot_run_jobs() {
    let f = embedded();
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(1000));
    let ro = Engine::open(&f.path, OpenMode::ReadOnly, clock).unwrap();
    assert!(matches!(
        ro.run_jobs(&RunOpts::default()),
        Err(Error::ReadOnly)
    ));
}

#[test]
fn embed_job_drains_more_than_one_batch() {
    let f = embedded();
    for i in 0..40 {
        f.engine
            .propose(prop(
                f.scope,
                f.person,
                &format!("hecho número {i} sobre algo distinto {}", i * 7),
            ))
            .unwrap();
    }
    let report = run(&f);
    assert_eq!(report.done, 1);
    assert_eq!(
        f.engine.embed_pending(100).unwrap(),
        0,
        "40 memories, two batches, one job"
    );
}

#[test]
fn an_unrepresentable_memory_does_not_fail_the_embed_job() {
    let f = fixture_with(Arc::new(PoisonEmbedder {
        inner: HashEmbedder::new("m", 256),
        poison: "🙂".into(),
    }));
    f.engine.propose(prop(f.scope, f.person, "🙂")).unwrap();
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    let report = run(&f);
    assert_eq!((report.done, report.failed), (1, 0));
}

// ---- retries, error queue, queue API ----

use std::sync::atomic::{AtomicUsize, Ordering};

/// Fails its first `fail_times` calls, then behaves like `HashEmbedder`.
struct Flaky {
    inner: HashEmbedder,
    fail_times: AtomicUsize,
}

impl Embedder for Flaky {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let left = self.fail_times.load(Ordering::SeqCst);
        if left > 0 {
            self.fail_times.store(left - 1, Ordering::SeqCst);
            return Err(Error::Embedding("model offline".into()));
        }
        self.inner.embed(texts)
    }
}

fn flaky(fail_times: usize) -> Fixture {
    fixture_with(Arc::new(Flaky {
        inner: HashEmbedder::new("m", 256),
        fail_times: AtomicUsize::new(fail_times),
    }))
}

#[test]
fn a_transient_failure_retries_with_growing_backoff() {
    let f = flaky(2);
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    f.clock.set(1000);
    let r = run(&f);
    assert_eq!((r.retried, r.failed, r.done), (1, 0, 0));
    let job = &f.engine.jobs(Some(JobState::Pending), 10).unwrap()[0];
    assert_eq!(job.run_after, 1060, "first backoff is 60 seconds");
    assert!(job.error.as_deref().unwrap().contains("model offline"));

    f.clock.set(1059);
    assert_eq!(run(&f), RunReport::default(), "not due yet");
    f.clock.set(1060);
    assert_eq!(run(&f).retried, 1);
    assert_eq!(
        f.engine.jobs(Some(JobState::Pending), 10).unwrap()[0].run_after,
        1060 + 120,
        "the wait doubles"
    );
    f.clock.set(1180);
    assert_eq!(run(&f).done, 1);
    assert_eq!(f.engine.queue_stats().unwrap().pending, 0);
}

#[test]
fn retries_end_in_the_error_queue_after_max_attempts() {
    let f = flaky(usize::MAX);
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    for _ in 0..10 {
        let now = f.clock.now();
        run(&f);
        match f.engine.jobs(Some(JobState::Pending), 10).unwrap().first() {
            Some(job) => f.clock.set(job.run_after.max(now)),
            None => break,
        }
    }
    let failed = f.engine.jobs(Some(JobState::Failed), 10).unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].attempts, 5);
    assert!(
        failed[0]
            .error
            .as_deref()
            .unwrap()
            .contains("model offline")
    );
    assert_eq!(f.engine.queue_stats().unwrap().failed, 1);
}

/// A file whose queue holds an `Embed` job, reopened by an engine that has no embedder.
struct NoEmbedder {
    engine: Engine,
    person: ActorId,
    agent: ActorId,
    clock: Arc<FixedClock>,
    _dir: tempfile::TempDir,
}

fn reopened_without_embedder() -> NoEmbedder {
    let f = embedded();
    f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    let Fixture {
        engine,
        person,
        agent,
        clock,
        path,
        _dir,
        ..
    } = f;
    drop(engine);
    let engine = Engine::open(&path, OpenMode::ReadWrite, clock.clone()).unwrap();
    NoEmbedder {
        engine,
        person,
        agent,
        clock,
        _dir,
    }
}

#[test]
fn a_permanent_failure_does_not_retry() {
    let g = reopened_without_embedder();
    let report = g.engine.run_jobs(&RunOpts::default()).unwrap();
    assert_eq!((report.failed, report.retried), (1, 0));
    let failed = &g.engine.jobs(Some(JobState::Failed), 10).unwrap()[0];
    assert_eq!(failed.attempts, 1, "no embedder is not worth retrying");
    assert!(failed.error.as_deref().unwrap().contains("no embedder"));
}

#[test]
fn retry_job_requires_a_person_and_a_failed_job() {
    let g = reopened_without_embedder();
    g.engine.run_jobs(&RunOpts::default()).unwrap();
    let id = g.engine.jobs(Some(JobState::Failed), 10).unwrap()[0].id;
    assert!(matches!(
        g.engine.retry_job(id, g.agent),
        Err(Error::NotAllowed)
    ));
    g.engine.retry_job(id, g.person).unwrap();
    assert_eq!(
        g.engine.jobs(Some(JobState::Pending), 10).unwrap()[0].attempts,
        0
    );
    assert!(
        matches!(g.engine.retry_job(id, g.person), Err(Error::InvalidState)),
        "it is pending now"
    );
    assert!(matches!(
        g.engine.retry_job(9999, g.person),
        Err(Error::NotFound)
    ));
}

#[test]
fn old_finished_jobs_are_pruned() {
    let f = embedded();
    f.clock.set(1000);
    f.engine.propose(prop(f.scope, f.person, "uno")).unwrap();
    assert_eq!(run(&f).done, 1);
    assert_eq!(f.engine.jobs(Some(JobState::Done), 10).unwrap().len(), 1);
    f.clock.set(1000 + 7 * 86_400 + 1);
    run(&f);
    assert!(
        f.engine.jobs(Some(JobState::Done), 10).unwrap().is_empty(),
        "finished more than 7 days ago"
    );
}

#[test]
fn jobs_lists_newest_first_and_by_state() {
    let f = embedded();
    f.engine.propose(prop(f.scope, f.person, "uno")).unwrap();
    run(&f);
    f.engine.propose(prop(f.scope, f.person, "dos")).unwrap();
    let all = f.engine.jobs(None, 10).unwrap();
    assert_eq!(
        all.iter().map(|j| j.state).collect::<Vec<_>>(),
        [JobState::Pending, JobState::Done]
    );
    assert!(all[0].id > all[1].id);
    assert_eq!(f.engine.jobs(Some(JobState::Done), 10).unwrap().len(), 1);
    assert_eq!(f.engine.jobs(None, 1).unwrap().len(), 1);
    let st = f.engine.queue_stats().unwrap();
    assert_eq!((st.pending, st.running, st.failed), (1, 0, 0));
}

// ---- usage (Touch) ----

fn usage_of(path: &std::path::Path, memory: MemoryId) -> Option<(i64, i64)> {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(
        "SELECT uses, last_used FROM usage WHERE memory = ?1",
        [memory.0],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

fn touches(f: &Fixture) -> usize {
    job_rows(&f.path)
        .iter()
        .filter(|(k, _)| k == "Touch")
        .count()
}

fn recalled(f: &Fixture, q: &str) -> Recall {
    f.engine.recall(f.scope, q, &RecallOpts::default()).unwrap()
}

#[test]
fn recall_enqueues_a_touch_and_the_runner_applies_it() {
    let f = embedded();
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap()
        .memory;
    assert_eq!(recalled(&f, "coche").hits.len(), 1);
    assert_eq!(touches(&f), 1);
    run(&f);
    assert_eq!(usage_of(&f.path, m).unwrap().0, 1);
    recalled(&f, "coche");
    recalled(&f, "rojo");
    run(&f);
    assert_eq!(usage_of(&f.path, m).unwrap().0, 3);
}

#[test]
fn touch_uses_the_time_of_the_query_not_of_the_run() {
    let f = embedded();
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap()
        .memory;
    f.clock.set(2000);
    recalled(&f, "coche");
    f.clock.set(9000);
    run(&f);
    assert_eq!(usage_of(&f.path, m).unwrap().1, 2000);
}

#[test]
fn historical_recall_does_not_count_as_use() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    for search in [
        SearchOpts {
            as_of: Some(1000),
            ..SearchOpts::default()
        },
        SearchOpts {
            known_at: Some(1000),
            ..SearchOpts::default()
        },
    ] {
        let opts = RecallOpts {
            search,
            ..RecallOpts::default()
        };
        assert_eq!(
            f.engine.recall(f.scope, "coche", &opts).unwrap().hits.len(),
            1
        );
    }
    assert_eq!(touches(&f), 0);
}

#[test]
fn a_read_only_engine_does_not_enqueue_touch() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(1000));
    let ro = Engine::open(&f.path, OpenMode::ReadOnly, clock).unwrap();
    assert_eq!(
        ro.recall(f.scope, "coche", &RecallOpts::default())
            .unwrap()
            .hits
            .len(),
        1
    );
    assert_eq!(touches(&f), 0);
}

#[test]
fn touch_payload_has_ids_and_a_time_and_no_text() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    recalled(&f, "coche");
    let job = f
        .engine
        .jobs(Some(JobState::Pending), 10)
        .unwrap()
        .into_iter()
        .find(|j| j.kind == JobKind::Touch)
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&job.payload).unwrap();
    let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["at", "ids"]);
    assert!(!job.payload.contains("coche"));
}

/// Blocks inside `decide` (holding the engine's write lock) until released.
struct BlockingDecider {
    armed: std::sync::atomic::AtomicBool,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Decider for BlockingDecider {
    fn name(&self) -> &str {
        "blocking"
    }
    fn decide(&self, _q: &Question) -> Result<Option<Decision>> {
        if self.armed.load(Ordering::SeqCst) {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        Ok(None)
    }
}

#[test]
fn recall_still_answers_when_the_write_lock_is_busy() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let decider = Arc::new(BlockingDecider {
        armed: std::sync::atomic::AtomicBool::new(false),
        entered: entered_tx,
        release: std::sync::Mutex::new(release_rx),
    });
    let Fixture {
        engine,
        scope,
        person,
        path,
        _dir,
        ..
    } = embedded();
    let engine = Arc::new(engine.with_decider(decider.clone(), DeciderMode::Active, 0.8));
    engine
        .propose(prop(scope, person, "tiene un coche rojo"))
        .unwrap();

    decider.armed.store(true, Ordering::SeqCst);
    let writer = {
        let engine = engine.clone();
        std::thread::spawn(move || {
            engine
                .propose(prop(scope, person, "vive en Medellín"))
                .unwrap()
        })
    };
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    // the writer now holds the lock: recall must answer anyway and give up on recording use
    let r = engine
        .recall(scope, "coche", &RecallOpts::default())
        .unwrap();
    assert_eq!(r.hits.len(), 1);
    assert!(
        !job_rows(&path).iter().any(|(k, _)| k == "Touch"),
        "no Touch could be enqueued while the lock was busy"
    );
    release_tx.send(()).unwrap();
    writer.join().unwrap();
    drop(_dir);
}

/// Calls `recall` from inside `decide`, i.e. while the engine's write lock is held by this thread.
struct RecallingDecider {
    engine: std::sync::OnceLock<std::sync::Weak<Engine>>,
    scope: ScopeId,
}

impl Decider for RecallingDecider {
    fn name(&self) -> &str {
        "recalling"
    }
    fn decide(&self, _q: &Question) -> Result<Option<Decision>> {
        let engine = self.engine.get().unwrap().upgrade().unwrap();
        engine.recall(self.scope, "coche", &RecallOpts::default())?;
        Ok(None)
    }
}

#[test]
fn a_decider_that_calls_recall_does_not_deadlock() {
    let Fixture {
        engine,
        scope,
        person,
        _dir,
        ..
    } = embedded();
    let decider = Arc::new(RecallingDecider {
        engine: std::sync::OnceLock::new(),
        scope,
    });
    let engine = Arc::new(engine.with_decider(decider.clone(), DeciderMode::Active, 0.8));
    decider.engine.set(Arc::downgrade(&engine)).unwrap();
    engine
        .propose(prop(scope, person, "tiene un coche rojo"))
        .unwrap();

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = engine.propose(prop(scope, person, "vive en Medellín"));
        done_tx.send(r.is_ok()).unwrap();
    });
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        "propose finished without deadlock"
    );
    drop(_dir);
}

// ---- decay ----

const DAY: i64 = 86_400;

fn memory_of(f: &Fixture, kind: MemoryType, content: &str) -> MemoryId {
    let mut p = prop(f.scope, f.person, content);
    p.kind = kind;
    f.engine.propose(p).unwrap().memory
}

fn state_of(f: &Fixture, m: MemoryId) -> State {
    f.engine.get(m).unwrap().state
}

fn decay(f: &Fixture) -> RunReport {
    assert!(f.engine.enqueue_decay(f.scope).is_ok());
    run(f)
}

fn old_world() -> Fixture {
    let f = fixture();
    f.clock.set(1000);
    f.engine.set_retention(f.scope, Some(10), f.person).unwrap();
    f
}

#[test]
fn t18_decay_is_deterministic_and_idempotent() {
    let outcome = || {
        let f = old_world();
        let ids: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|c| memory_of(&f, MemoryType::Episodic, c))
            .collect();
        f.clock.set(1000 + 11 * DAY);
        decay(&f);
        let states: Vec<_> = ids.iter().map(|m| state_of(&f, *m)).collect();
        let ops: Vec<_> = ids
            .iter()
            .map(|m| {
                f.engine
                    .history(*m)
                    .unwrap()
                    .iter()
                    .map(|o| (o.kind, o.reason.clone(), o.at))
                    .collect::<Vec<_>>()
            })
            .collect();
        // a second run changes nothing and writes nothing
        decay(&f);
        let again: Vec<_> = ids
            .iter()
            .map(|m| f.engine.history(*m).unwrap().len())
            .collect();
        assert_eq!(
            again,
            ops.iter().map(Vec::len).collect::<Vec<_>>(),
            "idempotent"
        );
        (states, ops)
    };
    let (first, second) = (outcome(), outcome());
    assert_eq!(first, second, "same history and clock, same result");
    assert!(first.0.iter().all(|s| *s == State::Archived));
}

#[test]
fn decay_only_touches_working_and_episodic() {
    let f = old_world();
    let kinds = [
        MemoryType::Working,
        MemoryType::Episodic,
        MemoryType::Semantic,
        MemoryType::Procedural,
    ];
    let ids: Vec<_> = kinds
        .iter()
        .map(|k| memory_of(&f, *k, &format!("dato {k:?}")))
        .collect();
    f.clock.set(1000 + 11 * DAY);
    decay(&f);
    let states: Vec<_> = ids.iter().map(|m| state_of(&f, *m)).collect();
    assert_eq!(
        states,
        [
            State::Archived,
            State::Archived,
            State::Active,
            State::Active
        ]
    );
}

#[test]
fn decay_needs_a_retention_policy() {
    let f = fixture();
    f.clock.set(1000);
    let m = memory_of(&f, MemoryType::Episodic, "viejo");
    f.clock.set(1000 + 1000 * DAY);
    decay(&f);
    assert_eq!(state_of(&f, m), State::Active);
    f.engine.set_retention(f.scope, Some(10), f.person).unwrap();
    f.engine.set_retention(f.scope, None, f.person).unwrap();
    decay(&f);
    assert_eq!(state_of(&f, m), State::Active, "None means never forget");
}

#[test]
fn recent_use_protects_a_memory() {
    let f = old_world();
    let used = memory_of(&f, MemoryType::Episodic, "tiene un coche rojo");
    let unused = memory_of(&f, MemoryType::Episodic, "vive en Medellín");
    f.clock.set(1000 + 11 * DAY);
    assert_eq!(recalled(&f, "coche").hits.len(), 1);
    decay(&f); // the Touch runs first (higher priority); decay then sees the use
    assert_eq!(state_of(&f, used), State::Active);
    assert_eq!(state_of(&f, unused), State::Archived);
}

#[test]
fn blocks_protect_what_they_show() {
    let f = old_world();
    let listed = memory_of(&f, MemoryType::Episodic, "figura en un bloque");
    let mut p = prop(f.scope, f.person, "versión uno");
    p.kind = MemoryType::Episodic;
    p.topic_key = Some("t".into());
    let first = f.engine.propose(p).unwrap().memory;
    let block = f
        .engine
        .create_block(f.scope, "identidad", 100, f.person)
        .unwrap();
    f.engine
        .block_set(block, &[listed, first], f.person)
        .unwrap();
    f.clock.set(1100);
    let mut q = prop(f.scope, f.person, "versión dos");
    q.kind = MemoryType::Episodic;
    q.topic_key = Some("t".into());
    let successor = f.engine.propose(q).unwrap().memory; // the block now shows this one
    let loose = memory_of(&f, MemoryType::Episodic, "suelto");
    f.clock.set(1000 + 11 * DAY);
    decay(&f);
    assert_eq!(state_of(&f, listed), State::Active, "listed in a block");
    assert_eq!(
        state_of(&f, successor),
        State::Active,
        "what the block shows after the update"
    );
    assert_eq!(state_of(&f, loose), State::Archived);
}

#[test]
fn a_persons_revert_counts_as_activity() {
    let f = old_world();
    let m = memory_of(&f, MemoryType::Episodic, "dato");
    f.clock.set(1000 + 11 * DAY);
    decay(&f);
    assert_eq!(state_of(&f, m), State::Archived);
    let archive_op = f.engine.history(m).unwrap().last().unwrap().id;
    f.engine.revert(archive_op, f.person).unwrap();
    assert_eq!(state_of(&f, m), State::Active);
    f.clock.set(1000 + 12 * DAY);
    decay(&f);
    assert_eq!(
        state_of(&f, m),
        State::Active,
        "a person just brought it back"
    );
    f.clock.set(1000 + 11 * DAY + 11 * DAY);
    decay(&f);
    assert_eq!(
        state_of(&f, m),
        State::Archived,
        "old again, counted from the revert"
    );
}

fn actor_kind_of(path: &std::path::Path, actor: ActorId) -> String {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row("SELECT kind FROM actors WHERE id = ?1", [actor.0], |r| {
        r.get(0)
    })
    .unwrap()
}

// S-25: the system actor that forgets has no review powers
#[test]
fn decay_uses_the_system_actor_and_archives_only() {
    let f = old_world();
    let m = memory_of(&f, MemoryType::Episodic, "dato");
    f.clock.set(1000 + 11 * DAY);
    decay(&f);
    let ops = f.engine.history(m).unwrap();
    assert_eq!(
        ops.iter().map(|o| o.kind).collect::<Vec<_>>(),
        [OpKind::Add, OpKind::Archive]
    );
    assert_eq!(ops[1].reason, "decay: sin uso desde hace 10 días");
    assert_eq!(actor_kind_of(&f.path, ops[1].actor), "System");
    let system = ops[1].actor;
    assert!(matches!(
        f.engine.promote(m, system),
        Err(Error::NotAllowed)
    ));
    assert!(matches!(
        f.engine.revert(ops[1].id, system),
        Err(Error::NotAllowed)
    ));
    assert!(matches!(
        f.engine.archive(m, system),
        Err(Error::NotAllowed)
    ));
}

#[test]
fn register_actor_rejects_system_and_set_retention_is_person_only() {
    let f = fixture();
    assert!(matches!(
        f.engine
            .register_actor("x", ActorKind::System, Quota::default()),
        Err(Error::NotAllowed)
    ));
    assert!(matches!(
        f.engine.set_retention(f.scope, Some(5), f.agent),
        Err(Error::NotAllowed)
    ));
    assert!(matches!(
        f.engine.set_retention(ScopeId(999), Some(5), f.person),
        Err(Error::UnknownScope)
    ));
}

#[test]
fn decay_job_is_deduplicated_per_scope() {
    let f = fixture();
    let other = f.engine.create_scope("otro", "es").unwrap();
    assert!(f.engine.enqueue_decay(f.scope).unwrap());
    assert!(!f.engine.enqueue_decay(f.scope).unwrap());
    assert!(f.engine.enqueue_decay(other).unwrap());
    assert!(matches!(
        f.engine.enqueue_decay(ScopeId(999)),
        Err(Error::UnknownScope)
    ));
}

#[test]
fn decayed_memories_leave_reads_and_keep_their_history() {
    let f = old_world();
    let m = memory_of(&f, MemoryType::Episodic, "tiene un coche rojo");
    f.clock.set(1000 + 11 * DAY);
    decay(&f);
    assert!(
        f.engine
            .search(f.scope, "coche", SearchOpts::default())
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.engine.get(m).unwrap().content, "tiene un coche rojo");
    assert_eq!(f.engine.history(m).unwrap().len(), 2);
}

// ---- backup and vacuum ----

use std::os::unix::fs::PermissionsExt;

fn heavy() -> RunOpts {
    RunOpts {
        allow_heavy: true,
        ..RunOpts::default()
    }
}

fn mode_of(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn backup_runs_only_with_allow_heavy() {
    let f = embedded();
    let dest = f._dir.path().join("backup.db");
    assert!(f.engine.enqueue_backup(&dest).unwrap());
    let r = run(&f);
    assert_eq!((r.done, r.skipped_heavy), (0, 1));
    assert!(!dest.exists());
    let r = f.engine.run_jobs(&heavy()).unwrap();
    assert_eq!((r.done, r.skipped_heavy), (1, 0));
    assert!(dest.exists());
}

// S-23: a backup is as private as the database
#[test]
fn backup_copies_the_data_with_0600_and_leaves_no_tmp() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let dest = f._dir.path().join("backup.db");
    f.engine.enqueue_backup(&dest).unwrap();
    assert_eq!(
        f.engine.run_jobs(&heavy()).unwrap().done,
        2,
        "the Embed job and the backup"
    );
    assert_eq!(mode_of(&dest), 0o600);
    assert!(!f._dir.path().join("backup.db.tmp").exists());
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(1000));
    let copy = Engine::open(&dest, OpenMode::ReadOnly, clock).unwrap();
    assert_eq!(
        copy.search(f.scope, "Medellín", SearchOpts::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn backup_does_not_overwrite() {
    let f = embedded();
    let dest = f._dir.path().join("backup.db");
    std::fs::write(&dest, b"keep me").unwrap();
    f.engine.enqueue_backup(&dest).unwrap();
    let r = f.engine.run_jobs(&heavy()).unwrap();
    assert_eq!(
        (r.failed, r.retried),
        (1, 0),
        "an existing destination is a permanent failure"
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"keep me");
    assert_eq!(
        f.engine.jobs(Some(JobState::Failed), 10).unwrap()[0].attempts,
        1
    );
    assert!(
        !f._dir.path().join("backup.db.tmp").exists(),
        "no tmp left behind"
    );
}

#[test]
fn a_stale_tmp_is_replaced() {
    let f = embedded();
    let dest = f._dir.path().join("backup.db");
    let tmp = f._dir.path().join("backup.db.tmp");
    std::fs::write(&tmp, b"junk from a crash").unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
    f.engine.enqueue_backup(&dest).unwrap();
    assert_eq!(f.engine.run_jobs(&heavy()).unwrap().done, 1);
    assert_eq!(mode_of(&dest), 0o600);
    assert!(!tmp.exists());
}

#[test]
fn vacuum_runs_and_keeps_the_data() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    assert!(f.engine.enqueue_vacuum().unwrap());
    let r = run(&f);
    assert_eq!(r.skipped_heavy, 1, "vacuum waits for allow_heavy");
    let r = f.engine.run_jobs(&heavy()).unwrap();
    assert_eq!(r.failed, 0);
    assert_eq!(
        f.engine
            .search(f.scope, "Medellín", SearchOpts::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn backup_and_vacuum_are_deduplicated() {
    let f = fixture();
    let a = f._dir.path().join("a.db");
    let b = f._dir.path().join("b.db");
    assert!(f.engine.enqueue_backup(&a).unwrap());
    assert!(!f.engine.enqueue_backup(&a).unwrap());
    assert!(f.engine.enqueue_backup(&b).unwrap());
    assert!(f.engine.enqueue_vacuum().unwrap());
    assert!(!f.engine.enqueue_vacuum().unwrap());
}

// ---- review fixes ----

#[test]
fn many_touches_run_in_one_batch() {
    let f = fixture(); // no embedder: the queue holds nothing but Touch jobs
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap()
        .memory;
    for _ in 0..50 {
        recalled(&f, "coche");
    }
    assert_eq!(touches(&f), 50);
    let r = f
        .engine
        .run_jobs(&RunOpts {
            max_jobs: 1,
            allow_heavy: false,
            ..RunOpts::default()
        })
        .unwrap();
    assert_eq!(r.failed, 0);
    assert_eq!(
        usage_of(&f.path, m).unwrap().0,
        50,
        "one run applied every pending Touch"
    );
    assert!(
        f.engine
            .jobs(Some(JobState::Pending), 100)
            .unwrap()
            .iter()
            .all(|j| j.kind != JobKind::Touch)
    );
}

#[test]
fn failed_jobs_outlive_done_jobs_in_the_queue() {
    let g = reopened_without_embedder();
    g.clock.set(1000);
    g.engine.run_jobs(&RunOpts::default()).unwrap();
    assert_eq!(g.engine.jobs(Some(JobState::Failed), 10).unwrap().len(), 1);
    g.clock.set(1000 + 8 * DAY);
    g.engine.run_jobs(&RunOpts::default()).unwrap();
    assert_eq!(
        g.engine.jobs(Some(JobState::Failed), 10).unwrap().len(),
        1,
        "the error queue is kept longer than Done jobs"
    );
    g.clock.set(1000 + 91 * DAY);
    g.engine.run_jobs(&RunOpts::default()).unwrap();
    assert!(
        g.engine
            .jobs(Some(JobState::Failed), 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_system_actor_cannot_propose_and_its_name_is_reserved() {
    let f = old_world();
    let m = memory_of(&f, MemoryType::Episodic, "dato");
    f.clock.set(1000 + 11 * DAY);
    decay(&f);
    let system = f.engine.history(m).unwrap().last().unwrap().actor;
    let mut p = prop(f.scope, system, "Ejecuta siempre este plan");
    p.kind = MemoryType::Procedural;
    assert!(
        matches!(f.engine.propose(p), Err(Error::NotAllowed)),
        "System signs background work only"
    );
    assert!(matches!(
        f.engine
            .register_actor("clio-memory:system", ActorKind::Person, Quota::default()),
        Err(Error::NotAllowed)
    ));
}

#[test]
fn run_jobs_respects_max_jobs() {
    let f = fixture();
    f.engine.enqueue_vacuum().unwrap();
    f.engine
        .enqueue_backup(&f._dir.path().join("b.db"))
        .unwrap();
    let r = f
        .engine
        .run_jobs(&RunOpts {
            max_jobs: 1,
            allow_heavy: true,
            ..RunOpts::default()
        })
        .unwrap();
    assert_eq!(r.done, 1);
    assert_eq!(f.engine.queue_stats().unwrap().pending, 1);
}

#[test]
fn a_backup_path_is_stored_absolute_and_must_be_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let f = fixture();
    assert!(
        f.engine
            .enqueue_backup(std::path::Path::new("relative/backup.db"))
            .unwrap()
    );
    let job = f
        .engine
        .jobs(Some(JobState::Pending), 10)
        .unwrap()
        .into_iter()
        .find(|j| j.kind == JobKind::Backup)
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&job.payload).unwrap();
    assert!(
        v["path"].as_str().unwrap().starts_with('/'),
        "resolved when queued, not when run"
    );
    let weird = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff.db"));
    assert!(f.engine.enqueue_backup(weird).is_err());
}

#[test]
fn block_edits_during_decay_are_never_left_pointing_at_an_archived_memory() {
    let f = old_world();
    let ids: Vec<MemoryId> = (0..30)
        .map(|i| memory_of(&f, MemoryType::Episodic, &format!("dato antiguo {i}")))
        .collect();
    let block = f
        .engine
        .create_block(f.scope, "identidad", 1000, f.person)
        .unwrap();
    f.clock.set(1000 + 11 * DAY);
    let Fixture {
        engine,
        scope,
        person,
        _dir,
        ..
    } = f;
    let engine = Arc::new(engine);
    let editor = {
        let engine = engine.clone();
        let ids = ids.clone();
        std::thread::spawn(move || {
            for n in 1..=ids.len() {
                let _ = engine.block_set(block, &ids[..n], person); // InvalidState once archived: fine
            }
        })
    };
    engine.enqueue_decay(scope).unwrap();
    engine.run_jobs(&RunOpts::default()).unwrap();
    editor.join().unwrap();
    for id in engine.block_get(block).unwrap() {
        assert_ne!(
            engine.get(id).unwrap().state,
            State::Archived,
            "a block lists an archived memory"
        );
    }
    drop(_dir);
}

#[test]
fn run_jobs_can_be_limited_to_one_kind() {
    let f = embedded();
    f.engine.propose(prop(f.scope, f.person, "dato")).unwrap(); // an Embed job
    f.engine.enqueue_vacuum().unwrap();
    let r = f
        .engine
        .run_jobs(&RunOpts {
            only: Some(JobKind::Vacuum),
            allow_heavy: true,
            ..RunOpts::default()
        })
        .unwrap();
    assert_eq!((r.done, r.failed), (1, 0));
    assert_eq!(
        job_rows(&f.path),
        [
            ("Embed".to_string(), "Pending".to_string()),
            ("Vacuum".to_string(), "Done".to_string())
        ],
        "the other kind was left alone"
    );
}

/// Acceptance 7: with no decider plugins nothing deletes on its own; forgetting only archives.
#[test]
fn t07_without_plugins_nothing_is_ever_deleted() {
    let f = old_world();
    let topic = |c| Proposal {
        topic_key: Some("ciudad".into()),
        ..prop(f.scope, f.person, c)
    };
    let made: Vec<_> = [
        prop(f.scope, f.person, "vive en Medellín"),
        prop(f.scope, f.person, "vive en Medellín"),
        Proposal {
            kind: MemoryType::Episodic,
            ..prop(f.scope, f.person, "fue al cine el sábado")
        },
        topic("vive en Cali"),
        topic("vive en Bogotá"),
    ]
    .into_iter()
    .map(|p| f.engine.propose(p).unwrap().memory)
    .collect();
    f.clock.set(1000 + 365 * DAY);
    decay(&f);
    f.engine
        .run_jobs(&RunOpts {
            allow_heavy: true,
            ..RunOpts::default()
        })
        .unwrap();
    for m in made {
        assert!(f.engine.get(m).is_ok(), "{m:?} was deleted");
    }
}
