#![cfg(feature = "telemetry")]
#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use clio_memory::*;
use common::*;

fn is_trace_id(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn trace_of(f: &Fixture, memory: MemoryId, index: usize) -> Option<String> {
    f.engine.history(memory).unwrap()[index].trace_id.clone()
}

fn add(f: &Fixture, text: &str) -> MemoryId {
    f.engine
        .propose(prop(f.scope, f.person, text))
        .unwrap()
        .memory
}

#[test]
fn every_operation_gets_a_32_hex_trace_id() {
    let f = fixture();
    let a = add(&f, "vive en Medellín");
    let b = add(&f, "tiene un perro");
    let (ta, tb) = (trace_of(&f, a, 0).unwrap(), trace_of(&f, b, 0).unwrap());
    assert!(is_trace_id(&ta) && is_trace_id(&tb), "{ta} {tb}");
    assert_ne!(ta, tb, "each call is its own trace");
    // a duplicate is a NOOP operation and has one too
    let dup = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    assert!(is_trace_id(
        &f.engine
            .history(a)
            .unwrap()
            .last()
            .unwrap()
            .trace_id
            .clone()
            .unwrap()
    ));
    assert_eq!(dup.kind, OpKind::Noop);
}

#[test]
fn generated_ids_do_not_repeat() {
    let f = fixture();
    let busy = f
        .engine
        .register_actor(
            "busy",
            ActorKind::Person,
            Quota {
                max: 10_000,
                window_secs: 3600,
            },
        )
        .unwrap();
    let mut seen = std::collections::HashSet::new();
    for i in 0..300 {
        let m = f
            .engine
            .propose(prop(f.scope, busy, &format!("dato {i}")))
            .unwrap()
            .memory;
        assert!(seen.insert(trace_of(&f, m, 0).unwrap()));
    }
}

#[test]
fn a_provider_overrides_the_generated_id() {
    let mut f = fixture();
    let engine = f.engine;
    f.engine =
        engine.with_trace_id_provider(Arc::new(|| Some("4bf92f3577b34da6a3ce929d0e0e4736".into())));
    let m = add(&f, "vive en Medellín");
    assert_eq!(
        trace_of(&f, m, 0).as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
}

#[test]
fn a_provider_returning_none_falls_back_to_a_generated_id() {
    let mut f = fixture();
    let engine = f.engine;
    f.engine = engine.with_trace_id_provider(Arc::new(|| None));
    let m = add(&f, "vive en Medellín");
    assert!(is_trace_id(&trace_of(&f, m, 0).unwrap()));
}

#[test]
fn the_provider_is_only_asked_when_an_operation_is_written() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut f = fixture();
    let counter = calls.clone();
    let engine = f.engine;
    f.engine = engine.with_trace_id_provider(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        None
    }));
    let m = add(&f, "vive en Medellín");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // promoting an active memory fails inside the write lock, before anything is logged
    assert!(f.engine.promote(m, f.person).is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn all_operations_of_one_call_share_one_trace_id() {
    let f = fixture();
    let ids: Vec<MemoryId> = (0..3).map(|i| add(&f, &format!("dato {i}"))).collect();
    let before: Vec<String> = ids.iter().map(|m| trace_of(&f, *m, 0).unwrap()).collect();
    let report = f.engine.revert_actor(f.person, f.person).unwrap();
    assert_eq!(report.reverted.len(), 3);
    let after: Vec<String> = ids.iter().map(|m| trace_of(&f, *m, 1).unwrap()).collect();
    assert!(
        after.windows(2).all(|w| w[0] == w[1]),
        "one revert_actor call, one trace"
    );
    assert!(before.iter().all(|b| *b != after[0]));
}

#[test]
fn one_decay_run_is_one_trace() {
    let f = fixture();
    f.clock.set(1000);
    f.engine.set_retention(f.scope, Some(1), f.person).unwrap();
    let mut ids = Vec::new();
    for i in 0..3 {
        let mut p = prop(f.scope, f.person, &format!("episodio {i}"));
        p.kind = MemoryType::Episodic;
        ids.push(f.engine.propose(p).unwrap().memory);
    }
    f.clock.set(1000 + 3 * 86_400);
    f.engine.enqueue_decay(f.scope).unwrap();
    f.engine.run_jobs(&RunOpts::default()).unwrap();
    let archived: Vec<String> = ids.iter().map(|m| trace_of(&f, *m, 1).unwrap()).collect();
    assert!(archived.windows(2).all(|w| w[0] == w[1]));
    assert!(is_trace_id(&archived[0]));
}

#[test]
fn trace_id_survives_export_and_import() {
    let f = fixture();
    let m = add(&f, "vive en Medellín");
    f.engine.feedback(m, Signal::Wrong, f.agent).unwrap();
    f.engine.feedback(m, Signal::Wrong, f.agent).unwrap();
    f.engine.feedback(m, Signal::Wrong, f.agent).unwrap(); // the third one flags it
    let mut out = Vec::new();
    f.engine.export(&mut out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let restored = Engine::open(
        &dir.path().join("r.db"),
        OpenMode::ReadWrite,
        Arc::new(FixedClock::new(1000)),
    )
    .unwrap();
    restored.import(&mut out.as_slice()).unwrap();
    let (a, b) = (f.engine.history(m).unwrap(), restored.history(m).unwrap());
    assert_eq!(a, b);
    assert!(
        a.iter()
            .all(|o| o.trace_id.as_deref().is_some_and(is_trace_id))
    );
    assert_eq!(a.len(), 2, "the add and the flag");
}

#[test]
fn a_provider_that_panics_or_returns_junk_never_blocks_a_write() {
    for answer in [
        None, // panics
        Some("has spaces and\nnewlines"),
        Some(""),
    ] {
        let mut f = fixture();
        let engine = f.engine;
        f.engine = engine.with_trace_id_provider(Arc::new(move || match answer {
            Some(s) => Some(s.to_string()),
            None => panic!("the host's tracing blew up"),
        }));
        let m = add(&f, "vive en Medellín");
        let id = trace_of(&f, m, 0).unwrap();
        assert!(is_trace_id(&id), "{answer:?} gave {id:?}");
    }
}

#[test]
fn a_trace_does_not_leak_into_the_next_call() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut f = fixture();
    let c = counter.clone();
    let engine = f.engine;
    f.engine = engine.with_trace_id_provider(Arc::new(move || {
        Some(format!("trace-{}", c.fetch_add(1, Ordering::SeqCst)))
    }));
    let a = add(&f, "uno");
    let b = add(&f, "dos");
    f.engine.archive(a, f.person).unwrap();
    assert_eq!(trace_of(&f, a, 0).as_deref(), Some("trace-0"));
    assert_eq!(trace_of(&f, b, 0).as_deref(), Some("trace-1"));
    assert_eq!(trace_of(&f, a, 1).as_deref(), Some("trace-2"));
}

// ---- spans and events ----

mod recorder {
    use std::collections::BTreeMap;
    use std::fmt::Debug;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};
    use tracing_core::span::Current;

    #[derive(Debug, Clone, Default)]
    pub struct SpanRec {
        pub id: u64,
        pub name: String,
        pub parent: Option<u64>,
        pub fields: BTreeMap<String, String>,
    }

    #[derive(Debug, Clone)]
    pub struct EventRec {
        pub level: Level,
        pub fields: BTreeMap<String, String>,
    }

    #[derive(Default)]
    pub struct Seen {
        pub spans: Vec<SpanRec>,
        pub events: Vec<EventRec>,
        stack: Vec<u64>,
        metadata: Vec<(u64, &'static Metadata<'static>)>,
    }

    impl Seen {
        pub fn spans_named(&self, name: &str) -> Vec<&SpanRec> {
            self.spans.iter().filter(|s| s.name == name).collect()
        }

        pub fn children_of(&self, id: u64) -> Vec<&SpanRec> {
            self.spans.iter().filter(|s| s.parent == Some(id)).collect()
        }

        pub fn events_named(&self, name: &str) -> Vec<&EventRec> {
            self.events
                .iter()
                .filter(|e| e.fields.get("event").map(String::as_str) == Some(name))
                .collect()
        }

        /// Every name and value the subscriber was ever shown.
        pub fn everything(&self) -> Vec<String> {
            let mut out = Vec::new();
            for s in &self.spans {
                out.push(s.name.clone());
                for (k, v) in &s.fields {
                    out.push(k.clone());
                    out.push(v.clone());
                }
            }
            for e in &self.events {
                for (k, v) in &e.fields {
                    out.push(k.clone());
                    out.push(v.clone());
                }
            }
            out
        }
    }

    struct Fields<'a>(&'a mut BTreeMap<String, String>);

    impl Visit for Fields<'_> {
        fn record_debug(&mut self, f: &Field, v: &dyn Debug) {
            self.0.insert(f.name().to_string(), format!("{v:?}"));
        }
        fn record_str(&mut self, f: &Field, v: &str) {
            self.0.insert(f.name().to_string(), v.to_string());
        }
    }

    /// Installed once for the whole test binary. `tracing` caches, per call site, whether anyone
    /// is listening; with only per-thread subscribers a thread that runs with none can cache
    /// "nobody" while another thread records. A global subscriber that accepts everything keeps
    /// every call site on, and `with_default` still routes each test's spans to its own recorder.
    pub fn install_global() {
        struct AcceptAll(AtomicU64);
        impl Subscriber for AcceptAll {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &Attributes<'_>) -> Id {
                Id::from_u64(self.0.fetch_add(1, Ordering::Relaxed))
            }
            fn record(&self, _: &Id, _: &Record<'_>) {}
            fn record_follows_from(&self, _: &Id, _: &Id) {}
            fn event(&self, _: &Event<'_>) {}
            fn enter(&self, _: &Id) {}
            fn exit(&self, _: &Id) {}
        }
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = tracing::subscriber::set_global_default(AcceptAll(AtomicU64::new(1)));
        });
    }

    pub struct Recorder {
        next: AtomicU64,
        pub seen: Arc<Mutex<Seen>>,
    }

    impl Recorder {
        pub fn new() -> (Self, Arc<Mutex<Seen>>) {
            let seen = Arc::new(Mutex::new(Seen::default()));
            (
                Self {
                    next: AtomicU64::new(1),
                    seen: seen.clone(),
                },
                seen,
            )
        }
    }

    impl Subscriber for Recorder {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, attrs: &Attributes<'_>) -> Id {
            let id = self.next.fetch_add(1, Ordering::SeqCst);
            let mut seen = self.seen.lock().unwrap();
            let parent = if attrs.is_contextual() {
                seen.stack.last().copied()
            } else {
                attrs.parent().map(|p| p.into_u64())
            };
            let mut fields = BTreeMap::new();
            attrs.record(&mut Fields(&mut fields));
            seen.metadata.push((id, attrs.metadata()));
            seen.spans.push(SpanRec {
                id,
                name: attrs.metadata().name().to_string(),
                parent,
                fields,
            });
            Id::from_u64(id)
        }

        fn record(&self, id: &Id, values: &Record<'_>) {
            let mut seen = self.seen.lock().unwrap();
            if let Some(s) = seen.spans.iter_mut().find(|s| s.id == id.into_u64()) {
                values.record(&mut Fields(&mut s.fields));
            }
        }

        fn record_follows_from(&self, _: &Id, _: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut seen = self.seen.lock().unwrap();
            let mut fields = BTreeMap::new();
            event.record(&mut Fields(&mut fields));
            seen.events.push(EventRec {
                level: *event.metadata().level(),
                fields,
            });
        }

        /// `Span::current()` (which the engine uses to fill in late fields) asks for this.
        fn current_span(&self) -> Current {
            let seen = self.seen.lock().unwrap();
            let Some(top) = seen.stack.last() else {
                return Current::none();
            };
            match seen.metadata.iter().find(|(id, _)| id == top) {
                Some((id, meta)) => Current::new(Id::from_u64(*id), meta),
                None => Current::none(),
            }
        }

        fn enter(&self, id: &Id) {
            self.seen.lock().unwrap().stack.push(id.into_u64());
        }

        fn exit(&self, id: &Id) {
            let mut seen = self.seen.lock().unwrap();
            if let Some(i) = seen.stack.iter().rposition(|s| *s == id.into_u64()) {
                seen.stack.remove(i);
            }
        }
    }
}

use recorder::{Recorder, Seen};

/// Runs `f` with a recording subscriber on this thread and returns what it saw.
fn capture<T>(f: impl FnOnce() -> T) -> (T, Seen) {
    recorder::install_global();
    let (recorder, seen) = Recorder::new();
    let out = tracing::subscriber::with_default(recorder, f);
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    (out, seen)
}

fn embedded() -> Fixture {
    fixture_with(Arc::new(HashEmbedder::new("m", 256)))
}

fn field<'a>(s: &'a recorder::SpanRec, name: &str) -> &'a str {
    s.fields
        .get(name)
        .unwrap_or_else(|| panic!("span {} has no field {name}: {:?}", s.name, s.fields))
}

#[test]
fn propose_emits_its_spans_with_the_expected_children() {
    let mut f = embedded();
    let engine = f.engine;
    f.engine = engine.with_decider(
        ScriptedDecider::new("classifier", Behavior::Nothing),
        DeciderMode::Shadow,
        0.8,
    );
    let (out, seen) = capture(|| {
        f.engine
            .propose(prop(f.scope, f.person, "vive en Medellín"))
            .unwrap()
    });
    let propose = &seen.spans_named("propose")[0];
    assert_eq!(field(propose, "scope"), f.scope.0.to_string());
    assert_eq!(field(propose, "actor"), f.person.0.to_string());
    assert_eq!(field(propose, "kind"), "Semantic");
    assert_eq!(field(propose, "outcome"), "Add");
    assert_eq!(field(propose, "state"), "Active");
    assert!(is_trace_id(field(propose, "trace_id")));
    let children: Vec<&str> = seen
        .children_of(propose.id)
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(
        children,
        [
            "secrets",
            "normalize",
            "rules",
            "neighbors",
            "decider",
            "append"
        ]
    );
    let decider = seen.spans_named("decider")[0];
    assert_eq!(field(decider, "provider"), "classifier");
    assert_eq!(out.kind, OpKind::Add);

    // a rejected proposal says why in one word, not in prose
    let (_, seen) = capture(|| {
        let _ = f
            .engine
            .propose(prop(f.scope, f.person, "clave AKIAIOSFODNN7EXAMPLE"));
    });
    let propose = &seen.spans_named("propose")[0];
    assert_eq!(field(propose, "outcome"), "secret_detected");
}

#[test]
fn recall_context_and_jobs_emit_spans() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let (_, seen) = capture(|| {
        f.engine.run_jobs(&RunOpts::default()).unwrap(); // the Embed job
        f.engine
            .recall(f.scope, "Medellín", &RecallOpts::default())
            .unwrap();
        f.engine
            .context(
                f.scope,
                &ContextOpts {
                    query: Some("Medellín".into()),
                    ..ContextOpts::default()
                },
            )
            .unwrap();
        f.engine.run_jobs(&RunOpts::default()).unwrap(); // the Touch job
    });
    let recall = &seen.spans_named("recall")[0];
    assert_eq!(field(recall, "scope"), f.scope.0.to_string());
    assert_eq!(field(recall, "hits"), "1");
    assert_eq!(field(recall, "abstained"), "false");
    assert_eq!(field(recall, "outcome"), "ok");
    assert!(field(recall, "text_hits").parse::<usize>().unwrap() >= 1);
    assert!(field(recall, "vector_hits").parse::<usize>().is_ok());
    assert!(field(recall, "limit").parse::<usize>().unwrap() > 0);
    // the context span contains a recall of its own
    let context = &seen.spans_named("context")[0];
    assert_eq!(field(context, "with_query"), "true");
    assert_eq!(field(context, "outcome"), "ok");
    assert!(
        seen.children_of(context.id)
            .iter()
            .any(|s| s.name == "recall")
    );
    let jobs = seen.spans_named("job");
    let kinds: Vec<&str> = jobs.iter().map(|j| field(j, "kind")).collect();
    assert_eq!(kinds, ["Embed", "Touch"]);
    for j in jobs {
        assert_eq!(field(j, "outcome"), "done");
        assert_eq!(field(j, "attempts"), "1");
        assert!(field(j, "id").parse::<i64>().is_ok());
    }
    // an abstaining recall says so
    let (_, seen) = capture(|| {
        f.engine
            .recall(f.scope, "astronomía", &RecallOpts::default())
            .unwrap()
    });
    let recall = &seen.spans_named("recall")[0];
    assert_eq!(
        (field(recall, "hits"), field(recall, "abstained")),
        ("0", "true")
    );
}

#[test]
fn review_actions_emit_their_spans() {
    let f = fixture();
    let mut ext = prop(f.scope, f.agent, "dato de fuera");
    ext.origin = Origin::External;
    let quarantined = f.engine.propose(ext).unwrap();
    let active = add(&f, "vive en Medellín");
    let (_, seen) = capture(|| {
        f.engine.promote(quarantined.memory, f.person).unwrap();
        f.engine.feedback(active, Signal::Useful, f.agent).unwrap();
        let archive = f.engine.archive(active, f.person).unwrap();
        f.engine.revert(archive, f.person).unwrap();
        assert!(f.engine.promote(active, f.person).is_err()); // already active
    });
    let one = |name: &str| {
        seen.spans_named(name)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(field(&one("promote")[0], "outcome"), "ok");
    assert_eq!(field(&one("promote")[1], "outcome"), "invalid_state");
    assert_eq!(field(&one("feedback")[0], "signal"), "Useful");
    assert_eq!(field(&one("archive")[0], "outcome"), "ok");
    assert_eq!(field(&one("revert")[0], "outcome"), "ok");
    assert!(is_trace_id(field(&one("promote")[0], "trace_id")));
    assert!(
        !one("promote")[1].fields.contains_key("trace_id"),
        "no operation, no trace id"
    );
}

#[test]
fn events_are_emitted_for_the_listed_situations() {
    let f = embedded();
    let m = add(&f, "vive en Medellín");
    let op = f.engine.history(m).unwrap()[0].id;
    let dir = tempfile::tempdir().unwrap();

    let (_, seen) = capture(|| {
        // a second process is turned away by the writer lock
        assert!(matches!(
            Engine::open(&f.path, OpenMode::ReadWrite, Arc::new(FixedClock::new(1))),
            Err(Error::WriterLocked)
        ));
        // the model that computed the vectors
        assert_eq!(f.engine.embed_pending(10).unwrap(), 1);
        f.engine.revert(op, f.person).unwrap();
        // a backup that works and one that cannot (the file exists)
        let ok = dir.path().join("backup.db");
        f.engine.enqueue_backup(&ok).unwrap();
        let taken = dir.path().join("taken.db");
        std::fs::write(&taken, "x").unwrap();
        f.engine.enqueue_backup(&taken).unwrap();
        let heavy = RunOpts {
            allow_heavy: true,
            ..RunOpts::default()
        };
        f.engine.run_jobs(&heavy).unwrap();
    });
    let locked = seen.events_named("writer_lock_rejected");
    assert_eq!(locked.len(), 1);
    assert_eq!(locked[0].level, tracing::Level::WARN);
    let embedded = &seen.events_named("embedded")[0].fields;
    assert_eq!(
        (embedded["model"].as_str(), embedded["stored"].as_str()),
        ("m", "1")
    );
    assert_eq!(seen.events_named("reverted").len(), 1);
    let backups: Vec<&str> = seen
        .events_named("backup")
        .iter()
        .map(|e| e.fields["outcome"].as_str())
        .collect();
    assert_eq!(backups, ["done", "failed"]);
    let failed = &seen.events_named("job_failed")[0].fields;
    assert_eq!(failed["kind"], "Backup");
    assert_eq!(failed["permanent"], "true");
    assert_eq!(
        seen.events_named("job_failed")[0].level,
        tracing::Level::WARN
    );
}

// S-18 and S-46: no span or event carries memory text, topic keys, names or paths
#[test]
fn t22_no_span_or_event_field_contains_memory_text() {
    const MARK: &str = "zebra";
    let f = embedded();
    let dir = tempfile::tempdir().unwrap();
    let hidden = dir.path().join("zebra-dir");
    std::fs::create_dir(&hidden).unwrap();
    let ((), seen) = capture(|| {
        let scope = f.engine.create_scope("zebra-scope", "es").unwrap();
        let person = f
            .engine
            .register_actor("zebra-person", ActorKind::Person, Quota::default())
            .unwrap();
        let agent = f
            .engine
            .register_actor("zebra-agent", ActorKind::Agent, Quota::default())
            .unwrap();
        let mut keep = prop(scope, person, "zebra vive en Medellín");
        keep.topic_key = Some("zebra-topic".into());
        keep.source = Some("zebra-notes.md".into());
        let first = f.engine.propose(keep.clone()).unwrap().memory;
        keep.content = "zebra vive en Bogotá".into();
        let current = f.engine.propose(keep).unwrap().memory; // an update by topic
        let mut episodic = prop(scope, person, "zebra episodio viejo");
        episodic.kind = MemoryType::Episodic;
        let old = f.engine.propose(episodic).unwrap().memory;
        let block = f
            .engine
            .create_block(scope, "zebra-block", 100, person)
            .unwrap();
        f.engine.block_set(block, &[current], person).unwrap();
        f.engine
            .recall(scope, "zebra Medellín", &RecallOpts::default())
            .unwrap();
        f.engine
            .context(
                scope,
                &ContextOpts {
                    query: Some("zebra".into()),
                    ..ContextOpts::default()
                },
            )
            .unwrap();
        for _ in 0..3 {
            f.engine.feedback(old, Signal::Wrong, agent).unwrap();
        }
        f.engine.promote(old, person).unwrap();
        f.engine.archive(old, person).unwrap();
        let archive = f.engine.history(old).unwrap().last().unwrap().id;
        f.engine.revert(archive, person).unwrap();
        f.engine.set_retention(scope, Some(1), person).unwrap();
        f.engine.enqueue_decay(scope).unwrap();
        f.engine
            .enqueue_backup(&hidden.join("zebra-backup.db"))
            .unwrap();
        f.clock.set(f.clock.now() + 5 * 86_400);
        f.engine
            .run_jobs(&RunOpts {
                allow_heavy: true,
                ..RunOpts::default()
            })
            .unwrap();
        // a failing path too: its error text must not leak either
        let _ = f
            .engine
            .propose(prop(scope, person, "zebra clave AKIAIOSFODNN7EXAMPLE"));
        let _ = f.engine.promote(first, person);
    });
    assert!(
        seen.spans.len() > 20 && !seen.events.is_empty(),
        "the test must have seen the work"
    );
    for text in seen.everything() {
        assert!(
            !text.to_lowercase().contains(MARK),
            "a span or event carried {text:?}"
        );
    }
}

#[test]
fn t24_the_operation_trace_id_matches_its_span() {
    let f = fixture();
    let (m, seen) = capture(|| add(&f, "vive en Medellín"));
    let stored = trace_of(&f, m, 0).unwrap();
    assert_eq!(field(seen.spans_named("propose")[0], "trace_id"), stored);
    let (_, seen) = capture(|| f.engine.archive(m, f.person).unwrap());
    assert_eq!(
        field(seen.spans_named("archive")[0], "trace_id"),
        trace_of(&f, m, 1).unwrap()
    );
}

#[test]
fn tracing_changes_nothing_observable() {
    fn script(f: &Fixture) -> Vec<(OpKind, String, i64)> {
        let a = add(f, "vive en Medellín");
        let b = add(f, "tiene un perro");
        f.engine.feedback(a, Signal::Useful, f.agent).unwrap();
        f.engine.archive(b, f.person).unwrap();
        f.engine
            .recall(f.scope, "Medellín", &RecallOpts::default())
            .unwrap();
        f.engine.run_jobs(&RunOpts::default()).unwrap();
        [a, b]
            .into_iter()
            .flat_map(|m| f.engine.history(m).unwrap())
            .map(|o| (o.kind, o.reason, o.id.0))
            .collect()
    }
    let quiet = script(&fixture());
    let (loud, seen) = capture(|| script(&fixture()));
    assert_eq!(quiet, loud);
    assert!(!seen.spans.is_empty());
}

#[test]
fn job_and_revert_actor_spans_carry_their_trace_id_and_never_touch_the_hosts_span() {
    let f = fixture();
    f.engine.set_retention(f.scope, Some(1), f.person).unwrap();
    let mut p = prop(f.scope, f.person, "episodio viejo");
    p.kind = MemoryType::Episodic;
    let old = f.engine.propose(p).unwrap().memory;
    let other = add(&f, "vive en Medellín");
    f.clock.set(f.clock.now() + 3 * 86_400);
    f.engine.enqueue_decay(f.scope).unwrap();
    let (_, seen) = capture(|| {
        f.engine.run_jobs(&RunOpts::default()).unwrap();
        // the embedding program's own span declares a field with the same name
        let host = tracing::info_span!("host", trace_id = "host-trace");
        let _in = host.enter();
        f.engine.revert_actor(f.person, f.person).unwrap();
    });
    let job = seen.spans_named("job")[0];
    assert_eq!(field(job, "trace_id"), trace_of(&f, old, 1).unwrap());
    let revert = seen.spans_named("revert_actor")[0];
    assert_eq!(field(revert, "trace_id"), trace_of(&f, other, 1).unwrap());
    let host = seen.spans_named("host")[0];
    assert_eq!(field(host, "trace_id"), "host-trace");
}
