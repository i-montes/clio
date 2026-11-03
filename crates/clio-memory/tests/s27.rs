//! S-27: scope isolation, per-actor quota and the secret detector, checked on every run of CI.
//! One file on purpose: when a release changes any of the three, these are the tests that must
//! still pass. They go through every read path and every text field, not one example each.
#![allow(clippy::unwrap_used)] // tests: panicking is the failure mode

mod common;

use std::sync::Arc;

use clio_memory::*;
use common::*;

/// Built at run time so no secret scanner sees a key in the source.
fn fake_keys() -> Vec<String> {
    vec![
        format!("sk-{}", "a1B2c3D4e5F6g7H8i9J0k1L2"),
        format!("ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
        format!("AKIA{}", "ABCDEFGHIJKLMNOP"),
        format!("xoxb-{}-{}", "1234567890", "abcdefghij"),
        format!("-----BEGIN RSA PRIVATE KEY{}", "-----"),
        format!(
            "eyJ{}.eyJ{}.{}",
            "hbGciOiJIUzI1NiJ9", "zdWIiOiIxMjM0NTY3ODkwIn0", "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV"
        ),
        "qZ8rT2vLx9KdWm4NpB7cYh3JfS6gAe1UoXi5VtRn0MbQ".to_string(), // long random token
    ]
}

fn operation_count(f: &Fixture) -> i64 {
    rusqlite::Connection::open(&f.path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |r| r.get(0))
        .unwrap()
}

// S-27: a question in one scope never returns a memory of another, by any read path
#[test]
fn s27_scopes_are_isolated_in_every_read_path() {
    let f = fixture_with(Arc::new(HashEmbedder::new("m", 64)));
    let other = f.engine.create_scope("other", "es").unwrap();
    let mine = f
        .engine
        .propose(prop(
            f.scope,
            f.person,
            "la llave del museo está en el cajón azul",
        ))
        .unwrap();
    let theirs = f
        .engine
        .propose(prop(
            other,
            f.person,
            "la llave del archivo está en el cajón rojo",
        ))
        .unwrap();
    f.engine.embed_pending(10).unwrap();

    let ids_of = |scope: ScopeId, q: &str| -> Vec<MemoryId> {
        f.engine
            .search(scope, q, SearchOpts::default())
            .unwrap()
            .iter()
            .map(|h| h.memory.id)
            .collect()
    };
    assert_eq!(ids_of(f.scope, "llave cajón"), [mine.memory]);
    assert_eq!(ids_of(other, "llave cajón"), [theirs.memory]);

    for (scope, own, foreign) in [
        (f.scope, mine.memory, theirs.memory),
        (other, theirs.memory, mine.memory),
    ] {
        let recall = f
            .engine
            .recall(scope, "llave cajón", &RecallOpts::default())
            .unwrap();
        let got: Vec<_> = recall.hits.iter().map(|h| h.memory.id).collect();
        assert!(
            got.contains(&own) && !got.contains(&foreign),
            "recall: {got:?}"
        );
        let listed: Vec<_> = f
            .engine
            .list(scope, None, None)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(listed, [own]);
        let ctx = f
            .engine
            .context(
                scope,
                &ContextOpts {
                    query: Some("llave cajón".into()),
                    ..ContextOpts::default()
                },
            )
            .unwrap();
        let (seen, hidden) = if scope == f.scope {
            ("azul", "rojo")
        } else {
            ("rojo", "azul")
        };
        assert!(
            ctx.contains(seen) && !ctx.contains(hidden),
            "context: {ctx}"
        );
    }
    // the same text in two scopes is two memories, not a duplicate
    let twin = f
        .engine
        .propose(prop(
            other,
            f.person,
            "la llave del museo está en el cajón azul",
        ))
        .unwrap();
    assert_eq!(twin.kind, OpKind::Add);
    // a block only takes memories of its own scope
    let block = f
        .engine
        .create_block(f.scope, "fijo", 100, f.person)
        .unwrap();
    assert!(
        f.engine
            .block_set(block, &[theirs.memory], f.person)
            .is_err()
    );
    assert!(f.engine.block_get(block).unwrap().is_empty());
}

// S-27: the quota belongs to each actor, counts everything it sends, and resets with its window
#[test]
fn s27_quota_is_per_actor_and_counts_every_attempt() {
    let f = fixture();
    let quota = Quota {
        max: 3,
        window_secs: 100,
    };
    let a = f
        .engine
        .register_actor("a", ActorKind::Agent, quota)
        .unwrap();
    let b = f
        .engine
        .register_actor("b", ActorKind::Agent, quota)
        .unwrap();
    f.clock.set(1000);
    // accepted, rejected for a secret, and a repeat all count
    f.engine.propose(prop(f.scope, a, "uno")).unwrap();
    assert!(matches!(
        f.engine.propose(prop(f.scope, a, &fake_keys()[0])),
        Err(Error::SecretDetected)
    ));
    assert_eq!(
        f.engine.propose(prop(f.scope, a, "uno")).unwrap().kind,
        OpKind::Noop
    );
    assert!(matches!(
        f.engine.propose(prop(f.scope, a, "dos")),
        Err(Error::QuotaExceeded)
    ));
    // the rejection wrote nothing; another actor and a person are not affected
    assert!(f.engine.list(f.scope, None, None).unwrap().len() == 1);
    f.engine.propose(prop(f.scope, b, "de b")).unwrap();
    f.engine.propose(prop(f.scope, f.person, "de ana")).unwrap();
    // a new window starts from zero
    f.clock.set(1000 + 100);
    f.engine.propose(prop(f.scope, a, "dos")).unwrap();
    // revert by actor still works for one that hit its quota
    let report = f.engine.revert_actor(a, f.person).unwrap();
    assert_eq!(report.reverted.len(), 2);
}

// S-27: the detector runs before anything is written, on every text a proposal carries
#[test]
fn s27_secrets_are_rejected_in_every_field_before_any_write() {
    let f = fixture();
    let before = operation_count(&f);
    let setters: [fn(&mut Proposal, String); 6] = [
        |p, s| p.content = format!("mi llave es {s}"),
        |p, s| p.content = s,
        |p, s| p.topic_key = Some(s),
        |p, s| p.session = Some(s),
        |p, s| p.episode = Some(s),
        |p, s| p.source = Some(s),
    ];
    for key in fake_keys() {
        for set in setters {
            let mut p = prop(f.scope, f.person, "dato inocente");
            set(&mut p, key.clone());
            assert!(
                matches!(f.engine.propose(p), Err(Error::SecretDetected)),
                "not rejected: {key}"
            );
        }
        assert!(matches!(
            f.engine.create_scope(&key, "es"),
            Err(Error::SecretDetected)
        ));
    }
    assert_eq!(
        operation_count(&f),
        before,
        "a rejected proposal wrote an operation"
    );
    assert!(f.engine.list(f.scope, None, None).unwrap().is_empty());
    let tables = dump_all(&f.path);
    for key in fake_keys() {
        assert!(!tables.contains(&key), "a table holds {key}");
    }
    // and ordinary text, ids and paths are not mistaken for secrets
    for ok in [
        "vive en Medellín desde 2026-03",
        "123e4567-e89b-12d3-a456-426614174000",
        "https://example.com/a/b?c=d",
    ] {
        f.engine.propose(prop(f.scope, f.person, ok)).unwrap();
    }
}
