#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use clio_memory::*;
use common::*;

const SECRET: &str = "sk-a1B2c3D4e5F6g7H8i9J0k1L2";

#[test]
fn t02_same_content_twice_is_add_then_noop() {
    let f = fixture();
    let a = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let b = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    assert_eq!((a.kind, b.kind), (OpKind::Add, OpKind::Noop));
    assert_eq!(b.memory, a.memory);
    assert_eq!(f.engine.list(f.scope, None, None).unwrap().len(), 1);
    let kinds: Vec<_> = f
        .engine
        .history(a.memory)
        .unwrap()
        .iter()
        .map(|o| o.kind)
        .collect();
    assert_eq!(kinds, [OpKind::Add, OpKind::Noop]);
}

#[test]
fn unicode_equivalent_content_is_noop() {
    let f = fixture();
    f.engine
        .propose(prop(f.scope, f.person, "café  con leche"))
        .unwrap();
    let b = f
        .engine
        .propose(prop(f.scope, f.person, " cafe\u{301} con leche "))
        .unwrap();
    assert_eq!(b.kind, OpKind::Noop);
}

// S-18: a rejected secret leaves no trace
#[test]
fn t03_secret_is_rejected_and_leaves_no_trace() {
    let f = fixture();
    let err = f
        .engine
        .propose(prop(f.scope, f.person, &format!("mi llave es {SECRET}")));
    assert!(matches!(err, Err(Error::SecretDetected)));
    assert!(!dump_all(&f.path).contains(SECRET));
    assert!(f.engine.list(f.scope, None, None).unwrap().is_empty());
}

// S-18: no secret reaches any table, even from metadata fields
#[test]
fn s18_secret_in_metadata_fields_is_rejected() {
    let f = fixture();
    let cases: [fn(&mut Proposal, String); 4] = [
        |p, s| p.topic_key = Some(s),
        |p, s| p.session = Some(s),
        |p, s| p.episode = Some(s),
        |p, s| p.source = Some(s),
    ];
    for set in cases {
        let mut p = prop(f.scope, f.person, "dato inocente");
        set(&mut p, SECRET.into());
        assert!(matches!(f.engine.propose(p), Err(Error::SecretDetected)));
    }
    assert!(!dump_all(&f.path).contains(SECRET));
}

// S-25: external content stays in quarantine
#[test]
fn t04_external_origin_hidden_until_promote_s25() {
    let f = fixture();
    let mut p = prop(f.scope, f.person, "página web dice algo");
    p.origin = Origin::External;
    let out = f.engine.propose(p).unwrap();
    assert_eq!(out.state, State::Quarantine);
    assert!(
        f.engine
            .search(f.scope, "página", SearchOpts::default())
            .unwrap()
            .is_empty()
    );
    let quarantined = f
        .engine
        .list(f.scope, Some(State::Quarantine), None)
        .unwrap();
    assert_eq!(quarantined.len(), 1);
}

// S-26: agent-proposed procedures wait for review
#[test]
fn s26_agent_procedural_is_quarantined_person_procedural_is_active() {
    let f = fixture();
    let mut p = prop(f.scope, f.agent, "para desplegar: correr just deploy");
    p.kind = MemoryType::Procedural;
    assert_eq!(f.engine.propose(p).unwrap().state, State::Quarantine);
    let mut p = prop(f.scope, f.person, "para probar: correr just test");
    p.kind = MemoryType::Procedural;
    assert_eq!(f.engine.propose(p).unwrap().state, State::Active);
}

#[test]
fn agent_cannot_claim_user_origin() {
    let f = fixture();
    let out = f
        .engine
        .propose(prop(f.scope, f.agent, "le gusta el café"))
        .unwrap();
    assert_eq!(f.engine.get(out.memory).unwrap().origin, Origin::Agent);
}

// S-27: scope isolation
#[test]
fn t05_scopes_are_isolated() {
    let f = fixture();
    let other = f.engine.create_scope("other", "es").unwrap();
    for scope in [f.scope, other] {
        let mut p = prop(scope, f.person, "vive en Medellín");
        p.topic_key = Some("residencia".into());
        assert_eq!(f.engine.propose(p).unwrap().kind, OpKind::Add);
    }
    let hits = f
        .engine
        .search(f.scope, "Medellín", SearchOpts::default())
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory.scope, f.scope);
}

// S-27: per-actor quota
#[test]
fn s27_quota_exceeded_is_rejected_and_secret_rejections_count() {
    let f = fixture();
    let bot = f
        .engine
        .register_actor(
            "limited",
            ActorKind::Agent,
            Quota {
                max: 2,
                window_secs: 3600,
            },
        )
        .unwrap();
    assert!(matches!(
        f.engine.propose(prop(f.scope, bot, SECRET)),
        Err(Error::SecretDetected)
    ));
    f.engine.propose(prop(f.scope, bot, "uno")).unwrap();
    assert!(matches!(
        f.engine.propose(prop(f.scope, bot, "dos")),
        Err(Error::QuotaExceeded)
    ));
}

#[test]
fn empty_content_rejected() {
    let f = fixture();
    for c in ["", "   \n\t"] {
        assert!(matches!(
            f.engine.propose(prop(f.scope, f.person, c)),
            Err(Error::EmptyContent)
        ));
    }
}

#[test]
fn invalid_validity_rejected() {
    let f = fixture();
    for (from, to) in [(Some(100), 100), (Some(100), 50), (None, 1000)] {
        let mut p = prop(f.scope, f.person, "algo");
        p.valid_from = from;
        p.valid_to = Some(to);
        assert!(
            matches!(f.engine.propose(p), Err(Error::InvalidValidity)),
            "{from:?}..{to}"
        );
    }
}

// S-18: names are text that reaches a table too
#[test]
fn s18_secret_in_scope_or_actor_name_is_rejected() {
    let f = fixture();
    assert!(matches!(
        f.engine.create_scope(SECRET, "es"),
        Err(Error::SecretDetected)
    ));
    assert!(matches!(
        f.engine
            .register_actor(SECRET, ActorKind::Agent, Quota::default()),
        Err(Error::SecretDetected)
    ));
    assert!(!dump_all(&f.path).contains(SECRET));
}

#[test]
fn concurrent_identical_proposals_make_one_add_and_noops() {
    let f = fixture();
    let engine = std::sync::Arc::new(f.engine);
    let kinds: Vec<OpKind> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let e = engine.clone();
                let (scope, actor) = (f.scope, f.person);
                s.spawn(move || {
                    e.propose(prop(scope, actor, "vive en Medellín"))
                        .unwrap()
                        .kind
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(kinds.iter().filter(|k| **k == OpKind::Add).count(), 1);
    assert_eq!(kinds.iter().filter(|k| **k == OpKind::Noop).count(), 7);
    assert_eq!(engine.list(f.scope, None, None).unwrap().len(), 1);
}

// ---- size limits ----

fn limited(max_content: usize) -> (Engine, ScopeId, ActorId, tempfile::TempDir) {
    let Fixture {
        engine,
        scope,
        person,
        _dir,
        ..
    } = fixture();
    let limits = Limits {
        max_content_bytes: max_content,
        ..Limits::default()
    };
    (engine.with_limits(limits), scope, person, _dir)
}

#[test]
fn default_limits_are_16_kib_and_256() {
    let l = Limits::default();
    assert_eq!((l.max_content_bytes, l.max_field_bytes), (16 * 1024, 256));
}

#[test]
fn content_at_the_limit_is_accepted_and_one_byte_over_is_not() {
    let (e, scope, person, _d) = limited(10);
    assert!(e.propose(prop(scope, person, &"a".repeat(10))).is_ok());
    assert!(matches!(
        e.propose(prop(scope, person, &"a".repeat(11))),
        Err(Error::TooLong)
    ));
}

#[test]
fn limit_counts_bytes_not_characters() {
    let (e, scope, person, _d) = limited(10);
    assert!(e.propose(prop(scope, person, &"é".repeat(5))).is_ok());
    assert!(matches!(
        e.propose(prop(scope, person, &"é".repeat(6))),
        Err(Error::TooLong)
    ));
}

#[test]
fn oversized_proposal_leaves_nothing_and_uses_quota() {
    let f = fixture();
    let bot = f
        .engine
        .register_actor(
            "limited",
            ActorKind::Agent,
            Quota {
                max: 2,
                window_secs: 3600,
            },
        )
        .unwrap();
    let path = f.path.clone();
    let engine = f.engine.with_limits(Limits {
        max_content_bytes: 20,
        ..Limits::default()
    });
    let huge = format!("MARKER-XYZ {}", "a".repeat(100));
    assert!(matches!(
        engine.propose(prop(f.scope, bot, &huge)),
        Err(Error::TooLong)
    ));
    assert!(!dump_all(&path).contains("MARKER-XYZ"));
    engine.propose(prop(f.scope, bot, "uno")).unwrap();
    assert!(matches!(
        engine.propose(prop(f.scope, bot, "dos")),
        Err(Error::QuotaExceeded)
    ));
    assert_eq!(engine.list(f.scope, None, None).unwrap().len(), 1);
}

#[test]
fn metadata_fields_have_their_own_limit() {
    let f = fixture();
    let set: [fn(&mut Proposal, String); 4] = [
        |p, s| p.topic_key = Some(s),
        |p, s| p.session = Some(s),
        |p, s| p.episode = Some(s),
        |p, s| p.source = Some(s),
    ];
    for (i, set) in set.into_iter().enumerate() {
        let mut ok = prop(f.scope, f.person, &format!("dato {i}"));
        set(&mut ok, "k".repeat(256));
        assert!(f.engine.propose(ok).is_ok(), "256 bytes must be accepted");
        let mut bad = prop(f.scope, f.person, &format!("otro {i}"));
        set(&mut bad, "k".repeat(257));
        assert!(
            matches!(f.engine.propose(bad), Err(Error::TooLong)),
            "257 bytes must be rejected"
        );
    }
}

#[test]
fn names_are_limited_too() {
    let f = fixture();
    let long = "n".repeat(257);
    assert!(matches!(
        f.engine.create_scope(&long, "es"),
        Err(Error::TooLong)
    ));
    assert!(matches!(
        f.engine
            .register_actor(&long, ActorKind::Agent, Quota::default()),
        Err(Error::TooLong)
    ));
    assert!(matches!(
        f.engine.create_block(f.scope, &long, 10, f.person),
        Err(Error::TooLong)
    ));
}

#[test]
fn limits_can_be_raised() {
    let f = fixture();
    let text = "paso ".repeat(4000); // 20 000 bytes, a long procedure
    assert!(matches!(
        f.engine.propose(prop(f.scope, f.person, &text)),
        Err(Error::TooLong)
    ));
    let engine = f.engine.with_limits(Limits {
        max_content_bytes: 64 * 1024,
        ..Limits::default()
    });
    assert!(engine.propose(prop(f.scope, f.person, &text)).is_ok());
}

#[test]
fn limit_applies_to_the_normalized_text() {
    let (e, scope, person, _d) = limited(30);
    // 10 x U+FB2C: 30 bytes as typed, 60 bytes once normalized to NFC
    let sneaky = "\u{FB2C}".repeat(10);
    assert_eq!(sneaky.len(), 30);
    assert!(matches!(
        e.propose(prop(scope, person, &sneaky)),
        Err(Error::TooLong)
    ));
    assert!(e.list(scope, None, None).unwrap().is_empty());
    // plain text at the limit is still fine
    assert!(e.propose(prop(scope, person, &"a".repeat(30))).is_ok());
}

#[test]
fn scope_language_is_checked() {
    let f = fixture();
    let key = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    assert!(matches!(
        f.engine.create_scope("otro", &"l".repeat(257)),
        Err(Error::TooLong)
    ));
    assert!(matches!(
        f.engine.create_scope("otro", &key),
        Err(Error::SecretDetected)
    ));
    assert!(!dump_all(&f.path).contains(&key));
    assert!(f.engine.create_scope("otro", "es").is_ok());
}

#[test]
fn whitespace_heavy_input_over_the_limit_is_rejected_early() {
    // documented behavior: the raw input is bounded too, even if it would shrink when normalized
    let (e, scope, person, _d) = limited(10);
    let padded = format!("hola{}mundo", " ".repeat(40));
    assert!(matches!(
        e.propose(prop(scope, person, &padded)),
        Err(Error::TooLong)
    ));
    assert!(e.propose(prop(scope, person, "hola mundo")).is_ok());
}
