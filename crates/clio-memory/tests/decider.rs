#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use std::sync::Arc;

use clio_memory::*;
use common::*;

type Reg = (Arc<ScriptedDecider>, DeciderMode, f32);

fn active(d: &Arc<ScriptedDecider>) -> Reg {
    (d.clone(), DeciderMode::Active, 0.8)
}

fn shadow(d: &Arc<ScriptedDecider>) -> Reg {
    (d.clone(), DeciderMode::Shadow, 0.8)
}

fn rig(regs: Vec<Reg>) -> Fixture {
    let mut f = fixture();
    let mut engine = std::mem::replace(
        &mut f.engine,
        Engine::open(
            &f._dir.path().join("unused.db"),
            OpenMode::ReadWrite,
            f.clock.clone(),
        )
        .unwrap(),
    );
    for (d, mode, min) in regs {
        engine = engine.with_decider(d, mode, min);
    }
    f.engine = engine;
    f
}

fn records(f: &Fixture) -> Vec<DecisionRecord> {
    f.engine.decisions(f.scope, 0, i64::MAX).unwrap()
}

#[test]
fn without_deciders_nothing_is_logged() {
    let f = fixture();
    let out = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    assert_eq!((out.kind, out.state), (OpKind::Add, State::Active));
    assert!(records(&f).is_empty());
}

#[test]
fn active_review_quarantines_and_is_logged() {
    let d = ScriptedDecider::new("clasificador", Behavior::Say(Choice::Review, 0.9));
    let f = rig(vec![active(&d)]);
    let out = f
        .engine
        .propose(prop(f.scope, f.person, "dato dudoso"))
        .unwrap();
    assert_eq!(out.state, State::Quarantine);
    let reason = &f.engine.history(out.memory).unwrap()[0].reason;
    assert!(
        reason.contains("review requested by clasificador"),
        "{reason}"
    );
    let r = records(&f);
    assert_eq!(r.len(), 1);
    assert_eq!(
        (r[0].status, r[0].choice, r[0].acted),
        (DecisionStatus::Decided, Some(Choice::Review), true)
    );
    assert_eq!(
        (r[0].provider.as_str(), r[0].mode, r[0].op),
        ("clasificador", DeciderMode::Active, Some(out.op))
    );
    // a person resolves it like any quarantined memory
    f.engine.promote(out.memory, f.person).unwrap();
    assert_eq!(f.engine.get(out.memory).unwrap().state, State::Active);
}

#[test]
fn active_add_changes_nothing_but_is_logged() {
    let d = ScriptedDecider::new("d", Behavior::Say(Choice::Add, 0.95));
    let f = rig(vec![active(&d)]);
    let out = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    assert_eq!(out.state, State::Active);
    let r = records(&f);
    assert_eq!(
        (r.len(), r[0].status, r[0].acted),
        (1, DecisionStatus::Decided, true)
    );
}

#[test]
fn below_threshold_is_logged_not_applied() {
    let d = ScriptedDecider::new("d", Behavior::Say(Choice::Review, 0.5));
    let f = rig(vec![active(&d)]);
    let out = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    assert_eq!(out.state, State::Active);
    let r = records(&f);
    assert_eq!(
        (r[0].status, r[0].acted, r[0].confidence),
        (DecisionStatus::BelowThreshold, false, Some(0.5))
    );
}

#[test]
fn min_confidence_is_per_registration() {
    let d = ScriptedDecider::new("d", Behavior::Say(Choice::Review, 0.5));
    let f = rig(vec![(d.clone(), DeciderMode::Active, 0.4)]);
    assert_eq!(
        f.engine
            .propose(prop(f.scope, f.person, "dato"))
            .unwrap()
            .state,
        State::Quarantine
    );
}

#[test]
fn shadow_logs_and_never_acts() {
    let d = ScriptedDecider::new("nuevo", Behavior::Say(Choice::Review, 0.99));
    let f = rig(vec![shadow(&d)]);
    let out = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    assert_eq!(out.state, State::Active);
    let r = records(&f);
    assert_eq!(
        (r[0].mode, r[0].status, r[0].choice, r[0].acted),
        (
            DeciderMode::Shadow,
            DecisionStatus::Decided,
            Some(Choice::Review),
            false
        )
    );
}

#[test]
fn first_active_wins_later_actives_are_skipped_shadows_still_run() {
    let a = ScriptedDecider::new("a", Behavior::Say(Choice::Add, 0.9));
    let b = ScriptedDecider::new("b", Behavior::Say(Choice::Review, 0.9));
    let s = ScriptedDecider::new("s", Behavior::Say(Choice::Review, 0.9));
    let f = rig(vec![active(&a), active(&b), shadow(&s)]);
    let out = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    assert_eq!(out.state, State::Active);
    assert_eq!((a.calls(), b.calls(), s.calls()), (1, 0, 1));
    let r = records(&f);
    assert_eq!(
        r.iter()
            .map(|x| (x.provider.as_str(), x.acted))
            .collect::<Vec<_>>(),
        [("a", true), ("s", false)]
    );
}

#[test]
fn active_nothing_falls_through_to_the_next() {
    let a = ScriptedDecider::new("a", Behavior::Nothing);
    let b = ScriptedDecider::new("b", Behavior::Say(Choice::Review, 0.9));
    let f = rig(vec![active(&a), active(&b)]);
    assert_eq!(
        f.engine
            .propose(prop(f.scope, f.person, "dato"))
            .unwrap()
            .state,
        State::Quarantine
    );
    let r = records(&f);
    assert_eq!(
        r.iter().map(|x| x.status).collect::<Vec<_>>(),
        [DecisionStatus::None, DecisionStatus::Decided]
    );
}

// S-25: the rules decide trust; a decider is never consulted about it and cannot undo it
#[test]
fn rules_veto_and_end_the_cascade() {
    let d = ScriptedDecider::new("d", Behavior::Say(Choice::Add, 0.99));
    let f = rig(vec![active(&d)]);
    let mut ext = prop(f.scope, f.person, "la página dice algo");
    ext.origin = Origin::External;
    assert_eq!(f.engine.propose(ext).unwrap().state, State::Quarantine);
    assert_eq!(d.calls(), 0, "untrusted origin is settled by the rules");

    f.engine
        .propose(prop(f.scope, f.person, "hecho uno"))
        .unwrap();
    assert_eq!(d.calls(), 1);
    let dup = f
        .engine
        .propose(prop(f.scope, f.person, "hecho uno"))
        .unwrap();
    assert_eq!(dup.kind, OpKind::Noop);
    assert_eq!(d.calls(), 1, "an exact duplicate is settled by the rules");

    let mut t1 = prop(f.scope, f.person, "reside en Medellín");
    t1.topic_key = Some("residencia".into());
    f.engine.propose(t1).unwrap();
    assert_eq!(d.calls(), 2);
    f.clock.set(2000);
    let mut t2 = prop(f.scope, f.person, "reside en Bogotá");
    t2.topic_key = Some("residencia".into());
    assert_eq!(f.engine.propose(t2).unwrap().kind, OpKind::Update);
    assert_eq!(d.calls(), 2, "a topic update is settled by the rules");
    assert_eq!(records(&f).len(), 2);
}

#[test]
fn decider_error_and_panic_do_not_block_the_write() {
    let a = ScriptedDecider::new("falla", Behavior::Fail);
    let b = ScriptedDecider::new("explota", Behavior::Panic);
    let f = rig(vec![active(&a), active(&b)]);
    let out = f
        .engine
        .propose(prop(f.scope, f.person, "dato importante"))
        .unwrap();
    assert_eq!(out.state, State::Active);
    let r = records(&f);
    assert_eq!(r.len(), 2);
    assert!(
        r.iter()
            .all(|x| x.status == DecisionStatus::Error && !x.acted)
    );
    assert!(r[1].reason.as_deref().unwrap().contains("decider exploded"));
}

#[test]
fn invalid_confidence_is_recorded_and_ignored() {
    for bad in [f32::NAN, 1.5, -0.1] {
        let d = ScriptedDecider::new("d", Behavior::Say(Choice::Review, bad));
        let f = rig(vec![active(&d)]);
        let out = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
        assert_eq!(out.state, State::Active, "confidence {bad} must not act");
        let r = records(&f);
        assert_eq!(
            (r[0].status, r[0].acted),
            (DecisionStatus::Invalid, false),
            "confidence {bad}"
        );
    }
}

#[test]
fn a_decider_can_never_drop_the_text() {
    // Whatever a decider does, the proposal is stored (active or quarantined). There is no
    // delete operation to ask for: `OpKind` has no such variant.
    let behaviors = [
        Behavior::Say(Choice::Review, 0.9),
        Behavior::Say(Choice::Add, 0.9),
        Behavior::Say(Choice::Review, f32::NAN),
        Behavior::Nothing,
        Behavior::Fail,
        Behavior::Panic,
    ];
    for (i, b) in behaviors.into_iter().enumerate() {
        let d = ScriptedDecider::new("d", b);
        let f = rig(vec![active(&d)]);
        let text = format!("texto único número {i}");
        let out = f.engine.propose(prop(f.scope, f.person, &text)).unwrap();
        assert_eq!(f.engine.get(out.memory).unwrap().content, text);
        assert_ne!(
            out.kind,
            OpKind::Noop,
            "a decider cannot turn a proposal into a no-op"
        );
    }
}

#[test]
fn decisions_range_by_time_and_survive_rebuild_and_reopen() {
    let d = ScriptedDecider::new("d", Behavior::Say(Choice::Add, 0.9));
    let f = rig(vec![active(&d)]);
    f.clock.set(100);
    f.engine.propose(prop(f.scope, f.person, "uno")).unwrap();
    f.clock.set(200);
    f.engine.propose(prop(f.scope, f.person, "dos")).unwrap();
    assert_eq!(f.engine.decisions(f.scope, 100, 200).unwrap().len(), 1);
    assert_eq!(f.engine.decisions(f.scope, 100, 201).unwrap().len(), 2);

    f.engine.rebuild_projection().unwrap();
    assert_eq!(
        records(&f).len(),
        2,
        "the audit log is not part of the projection"
    );

    let Fixture {
        path,
        engine,
        scope,
        _dir,
        ..
    } = f;
    drop(engine);
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(300));
    let reopened = Engine::open(&path, OpenMode::ReadWrite, clock).unwrap();
    assert_eq!(reopened.decisions(scope, 0, i64::MAX).unwrap().len(), 2);
}

// ---- hardening the trust boundary ----

// S-18, S-01: what a decider writes into the audit log is bounded and filtered by code
#[test]
fn decider_text_is_clipped_and_secrets_are_redacted() {
    let key = format!("sk-{}", "a1B2c3D4e5F6g7H8i9J0k1L2");
    let long_name = "n".repeat(1000);
    let d = ScriptedDecider::with_reason(
        &long_name,
        Behavior::Say(Choice::Review, 0.9),
        &"r".repeat(5_000_000),
    );
    let f = rig(vec![active(&d)]);
    let out = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    let r = &records(&f)[0];
    assert!(
        r.provider.len() <= 256,
        "provider {} bytes",
        r.provider.len()
    );
    assert!(r.reason.as_deref().unwrap().len() <= 256);
    assert!(
        f.engine.history(out.memory).unwrap()[0].reason.len() < 400,
        "the operation reason is bounded too"
    );

    let leaky = ScriptedDecider::with_reason(
        "filtrador",
        Behavior::Say(Choice::Add, 0.9),
        &format!("el prompt decía {key}"),
    );
    let g = rig(vec![active(&leaky)]);
    g.engine
        .propose(prop(g.scope, g.person, "otro dato"))
        .unwrap();
    assert!(!records(&g)[0].reason.as_deref().unwrap().contains(&key));
    assert!(!dump_all(&g.path).contains(&key));
}

#[test]
fn an_invalid_min_confidence_fails_closed() {
    // NaN must not turn "act above the threshold" into "act on anything"
    let d = ScriptedDecider::new("d", Behavior::Say(Choice::Review, 0.1));
    let f = rig(vec![(d.clone(), DeciderMode::Active, f32::NAN)]);
    assert_eq!(
        f.engine
            .propose(prop(f.scope, f.person, "dato"))
            .unwrap()
            .state,
        State::Active
    );
    assert_eq!(records(&f)[0].status, DecisionStatus::BelowThreshold);
}

// ---- neighbors ----

fn rig_embedded(regs: Vec<Reg>, embedder: Arc<dyn Embedder>) -> Fixture {
    let mut f = fixture_with(embedder);
    let mut engine = std::mem::replace(
        &mut f.engine,
        Engine::open(
            &f._dir.path().join("unused.db"),
            OpenMode::ReadWrite,
            f.clock.clone(),
        )
        .unwrap(),
    );
    for (d, mode, min) in regs {
        engine = engine.with_decider(d, mode, min);
    }
    f.engine = engine;
    f
}

fn hash() -> Arc<dyn Embedder> {
    Arc::new(HashEmbedder::new("m", 256))
}

fn last_question(d: &ScriptedDecider) -> Question {
    d.seen.lock().unwrap().last().cloned().unwrap()
}

const FACTS: [&str; 7] = [
    "tiene un coche rojo",
    "vive en Medellín desde hace años",
    "prefiere respuestas breves",
    "trabaja en una empresa de logística",
    "le gusta el café sin azúcar",
    "estudió ingeniería en Antioquia",
    "el perro se llama Toby",
];

#[test]
fn neighbors_arrive_best_first_within_the_scope() {
    let d = ScriptedDecider::new("d", Behavior::Nothing);
    let f = rig_embedded(vec![active(&d)], hash());
    let other = f.engine.create_scope("otro", "es").unwrap();
    for c in FACTS {
        f.engine.propose(prop(f.scope, f.person, c)).unwrap();
    }
    f.engine
        .propose(prop(other, f.person, "tiene un coche rojo"))
        .unwrap();
    f.engine.embed_pending(100).unwrap();

    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo grande"))
        .unwrap();
    let q = last_question(&d);
    assert_eq!(q.candidate.content, "tiene un coche rojo grande");
    assert_eq!(q.neighbors.len(), 5, "at most five neighbors");
    assert!(
        q.neighbors.iter().all(|n| n.memory.scope == f.scope),
        "never another scope"
    );
    assert_eq!(q.neighbors[0].memory.content, "tiene un coche rojo");
    assert!(
        q.neighbors[0].similarity > 0.8 && q.neighbors[0].similarity < 0.9,
        "{}",
        q.neighbors[0].similarity
    );
    assert!(
        q.neighbors
            .windows(2)
            .all(|w| w[0].similarity >= w[1].similarity),
        "best first"
    );
}

#[test]
fn no_embedder_means_no_neighbors() {
    let d = ScriptedDecider::new("d", Behavior::Nothing);
    let f = rig(vec![active(&d)]);
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo grande"))
        .unwrap();
    assert!(last_question(&d).neighbors.is_empty());
}

#[test]
fn memories_without_vectors_are_not_neighbors() {
    let d = ScriptedDecider::new("d", Behavior::Nothing);
    let f = rig_embedded(vec![active(&d)], hash());
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    // embed_pending has not run: the first memory has no vector yet
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo grande"))
        .unwrap();
    assert!(last_question(&d).neighbors.is_empty());
}

struct FailingEmbedder;

impl Embedder for FailingEmbedder {
    fn model_id(&self) -> &str {
        "m"
    }
    fn dim(&self) -> usize {
        256
    }
    fn embed(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Err(Error::Embedding("model offline".into()))
    }
}

#[test]
fn a_failing_or_unrepresentable_candidate_gives_no_neighbors_and_the_write_continues() {
    for embedder in [
        Arc::new(FailingEmbedder) as Arc<dyn Embedder>,
        Arc::new(PoisonEmbedder {
            inner: HashEmbedder::new("m", 256),
            poison: "grande".into(),
        }),
    ] {
        let d = ScriptedDecider::new("d", Behavior::Nothing);
        let f = rig_embedded(vec![active(&d)], embedder);
        let out = f
            .engine
            .propose(prop(f.scope, f.person, "tiene un coche rojo grande"))
            .unwrap();
        assert_eq!(out.state, State::Active);
        assert!(last_question(&d).neighbors.is_empty());
        assert_eq!(
            records(&f).len(),
            1,
            "the decider was still consulted and logged"
        );
    }
}

#[test]
fn neighbors_are_not_computed_without_deciders() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let embedder = CountingEmbedder {
        inner: HashEmbedder::new("m", 256),
        calls: calls.clone(),
    };
    let f = fixture_with(Arc::new(embedder));
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "propose must not embed on its own"
    );
}

// ---- near-duplicate decider ----

fn with_near_duplicate(threshold: f32) -> Fixture {
    let mut f = fixture_with(hash());
    let engine = std::mem::replace(
        &mut f.engine,
        Engine::open(
            &f._dir.path().join("unused.db"),
            OpenMode::ReadWrite,
            f.clock.clone(),
        )
        .unwrap(),
    );
    f.engine = engine.with_decider(
        Arc::new(NearDuplicateDecider { threshold }),
        DeciderMode::Active,
        0.8,
    );
    f
}

#[test]
fn near_duplicate_sends_a_close_match_to_review() {
    let f = with_near_duplicate(0.8);
    let first = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    f.engine.embed_pending(10).unwrap();
    let out = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo grande"))
        .unwrap();
    assert_eq!(out.state, State::Quarantine);
    let history = f.engine.history(out.memory).unwrap();
    assert!(
        history[0].reason.contains("near-duplicate"),
        "{}",
        history[0].reason
    );
    let r = records(&f);
    assert!(
        r.last()
            .unwrap()
            .reason
            .as_deref()
            .unwrap()
            .contains(&format!("m{}", first.memory.0))
    );

    // with the default (strict) threshold the same candidate is simply added
    let g = with_near_duplicate(0.97);
    g.engine
        .propose(prop(g.scope, g.person, "tiene un coche rojo"))
        .unwrap();
    g.engine.embed_pending(10).unwrap();
    let out = g
        .engine
        .propose(prop(g.scope, g.person, "tiene un coche rojo grande"))
        .unwrap();
    assert_eq!(out.state, State::Active);
}

#[test]
fn a_person_resolves_the_near_duplicate_review() {
    let f = with_near_duplicate(0.8);
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    f.engine.embed_pending(10).unwrap();
    let dup = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo grande"))
        .unwrap();
    f.engine.archive(dup.memory, f.person).unwrap();
    assert_eq!(f.engine.get(dup.memory).unwrap().state, State::Archived);

    f.clock.set(2000);
    let other = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo pequeño"))
        .unwrap();
    assert_eq!(other.state, State::Quarantine);
    f.engine.promote(other.memory, f.person).unwrap();
    assert_eq!(f.engine.get(other.memory).unwrap().state, State::Active);
}

/// Prints how close a paraphrase and contradictions are with a real model, to pick a threshold.
/// `CLIO_MODEL2VEC_DIR=/path/to/model cargo test -p clio-memory --release --features model2vec
/// --test decider -- --ignored --nocapture real_model`
#[cfg(feature = "model2vec")]
#[test]
#[ignore]
fn real_model_paraphrase_vs_contradiction_probe() {
    let Ok(dir) = std::env::var("CLIO_MODEL2VEC_DIR") else {
        eprintln!("CLIO_MODEL2VEC_DIR is not set; skipping");
        return;
    };
    let model = Model2VecEmbedder::from_dir(std::path::Path::new(&dir)).unwrap();
    let base = "Vive en Medellín";
    let others = [
        "Reside en Medellín",
        "Vive en Bogotá",
        "No vive en Medellín",
        "Le gusta el café",
    ];
    let vectors = model
        .embed(&[base, others[0], others[1], others[2], others[3]])
        .unwrap();
    let cos = |a: &[f32], b: &[f32]| {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (na * nb)
    };
    for (text, v) in others.iter().zip(&vectors[1..]) {
        println!("cosine({base:?}, {text:?}) = {:.3}", cos(&vectors[0], v));
    }
}
