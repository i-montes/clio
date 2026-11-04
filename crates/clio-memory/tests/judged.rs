#![allow(clippy::unwrap_used)] // tests: panicking is the failure mode
//! The judge (Jev) decides what to do with a proposal: new, replaces, already known. Everything
//! else about the write path stays as it was: quota and secrets run first, the memory keeps its
//! author, the judge signs the operation, nothing it did is beyond REVERT, it has no review powers,
//! and what the person wrote is replaced only when the person's own words said so.

mod common;

use clio_memory::*;
use common::*;

fn external(f: &Fixture, text: &str) -> Proposal {
    let mut p = prop(f.scope, f.agent, text);
    p.origin = Origin::External;
    p
}

fn sure() -> Sureness {
    Sureness { confidence: 0.9 }
}

fn active(f: &Fixture) -> Vec<MemoryRow> {
    f.engine.list(f.scope, Some(State::Active), None).unwrap()
}

// S-25 S-61
#[test]
fn the_judge_is_made_by_the_engine_and_nobody_else_can_be_one() {
    let f = fixture();
    let a = f.engine.judge().unwrap();
    let b = f.engine.judge().unwrap();
    // The same actor every time: the judge is registered once.
    let one = f
        .engine
        .propose_judged(&a, external(&f, "uno"), Judged::Add, sure())
        .unwrap();
    let two = f
        .engine
        .propose_judged(&b, external(&f, "dos"), Judged::Add, sure())
        .unwrap();
    let who = |o: &Outcome| f.engine.history(o.memory).unwrap()[0].actor;
    assert_eq!(who(&one), who(&two));
    // Nobody registers a judge, or takes its name.
    for (name, kind) in [
        ("otro", ActorKind::Judge),
        ("jev", ActorKind::Person),
        ("jev", ActorKind::Agent),
    ] {
        let e = f
            .engine
            .register_actor(name, kind, Quota::default())
            .unwrap_err();
        assert!(matches!(e, Error::NotAllowed), "{name} {kind:?}: {e:?}");
    }
}

// S-25: a judged add from outside is active, keeps its origin and is signed by the judge.
// S-64
#[test]
fn a_judged_add_is_active_and_the_judge_signs_it() {
    let f = fixture();
    let j = f.engine.judge().unwrap();
    let o = f
        .engine
        .propose_judged(
            &j,
            external(&f, "La reunión es los martes a las 10"),
            Judged::Add,
            sure(),
        )
        .unwrap();
    assert_eq!((o.kind, o.state), (OpKind::Add, State::Active));
    let row = f.engine.get(o.memory).unwrap();
    assert_eq!(row.actor, f.agent, "the memory keeps its author");
    assert_eq!(
        row.origin,
        Origin::External,
        "and its origin, so reading it still taints"
    );
    let op = &f.engine.history(o.memory).unwrap()[0];
    assert_ne!(op.actor, f.agent, "the judge signed the operation");
    assert!(op.reason.is_empty() || !op.reason.contains("judged: update"));
    assert!(active(&f).iter().any(|m| m.id == o.memory));
}

#[test]
fn a_procedure_judged_as_new_is_active_too_because_the_person_said_everything_goes_through_jev() {
    let f = fixture();
    let j = f.engine.judge().unwrap();
    let mut p = external(&f, "Para desplegar, corre deploy.sh en staging primero");
    p.kind = MemoryType::Procedural;
    let o = f.engine.propose_judged(&j, p, Judged::Add, sure()).unwrap();
    assert_eq!(o.state, State::Active);
}

#[test]
fn an_exact_copy_of_something_active_is_a_no_op_even_when_judged_as_new() {
    let f = fixture();
    let old = f
        .engine
        .propose(prop(f.scope, f.person, "Marta vive en Medellín"))
        .unwrap();
    let j = f.engine.judge().unwrap();
    let o = f
        .engine
        .propose_judged(
            &j,
            external(&f, "Marta vive en Medellín"),
            Judged::Add,
            sure(),
        )
        .unwrap();
    assert_eq!((o.kind, o.memory), (OpKind::Noop, old.memory));
    assert_eq!(active(&f).len(), 1);
}

#[test]
fn a_judged_update_replaces_the_old_memory_and_the_person_can_revert_it() {
    let f = fixture();
    let j = f.engine.judge().unwrap();
    let old = f
        .engine
        .propose_judged(
            &j,
            external(&f, "El proyecto usa PostgreSQL 16"),
            Judged::Add,
            sure(),
        )
        .unwrap();
    let new = f
        .engine
        .propose_judged(
            &j,
            external(&f, "El proyecto ahora usa PostgreSQL 17"),
            Judged::Update {
                old: old.memory,
                person_confirmed: false,
            },
            sure(),
        )
        .unwrap();
    assert_eq!((new.kind, new.state), (OpKind::Update, State::Active));
    assert_eq!(f.engine.get(old.memory).unwrap().state, State::Replaced);
    assert_eq!(
        f.engine.get(old.memory).unwrap().replaced_by,
        Some(new.memory)
    );
    let ids: Vec<_> = active(&f).iter().map(|m| m.id).collect();
    assert_eq!(ids, [new.memory]);
    // Nothing the judge did is beyond REVERT.
    f.engine.revert(new.op, f.person).unwrap();
    assert_eq!(f.engine.get(old.memory).unwrap().state, State::Active);
}

// S-25 S-27 S-64: the memory is the agent's: a judged update replaces what the person wrote, and
// the person's revert brings it back.
#[test]
fn a_judged_update_replaces_what_the_person_wrote_and_revert_restores_it() {
    let f = fixture();
    let mine = f
        .engine
        .propose(prop(f.scope, f.person, "Marta vive en Medellín"))
        .unwrap()
        .memory;
    let j = f.engine.judge().unwrap();
    let update = |confirmed| Judged::Update {
        old: mine,
        person_confirmed: confirmed,
    };
    let o = f
        .engine
        .propose_judged(
            &j,
            external(&f, "Marta ahora vive en Bogotá"),
            update(false),
            sure(),
        )
        .unwrap();
    assert_eq!(o.state, State::Active);
    assert_eq!(f.engine.get(mine).unwrap().state, State::Replaced);
    assert_eq!(active(&f).len(), 1);
    f.engine.revert(o.op, f.person).unwrap();
    assert_eq!(f.engine.get(mine).unwrap().state, State::Active);
}

#[test]
fn a_judged_update_needs_an_active_memory_of_the_same_scope_and_kind() {
    let f = fixture();
    let j = f.engine.judge().unwrap();
    let a = f
        .engine
        .propose_judged(&j, external(&f, "dato viejo"), Judged::Add, sure())
        .unwrap()
        .memory;
    f.engine.archive(a, f.person).unwrap();
    let go = |old: MemoryId, kind| {
        let mut p = external(&f, &format!("dato nuevo {}", old.0));
        p.kind = kind;
        f.engine.propose_judged(
            &j,
            p,
            Judged::Update {
                old,
                person_confirmed: false,
            },
            sure(),
        )
    };
    assert!(
        matches!(
            go(a, MemoryType::Semantic).unwrap_err(),
            Error::InvalidState
        ),
        "archived"
    );
    let b = f
        .engine
        .propose_judged(&j, external(&f, "otro dato"), Judged::Add, sure())
        .unwrap()
        .memory;
    assert!(
        matches!(
            go(b, MemoryType::Procedural).unwrap_err(),
            Error::InvalidState
        ),
        "other kind"
    );
    assert!(
        go(MemoryId(9999), MemoryType::Semantic).is_err(),
        "no such memory"
    );
}

#[test]
fn a_judged_no_op_writes_nothing() {
    let f = fixture();
    let j = f.engine.judge().unwrap();
    let known = f
        .engine
        .propose_judged(&j, external(&f, "Marta usa Neovim"), Judged::Add, sure())
        .unwrap()
        .memory;
    let before = f.engine.history(known).unwrap().len();
    let o = f
        .engine
        .propose_judged(
            &j,
            external(&f, "A Marta le gusta Neovim"),
            Judged::Noop { existing: known },
            sure(),
        )
        .unwrap();
    assert_eq!((o.kind, o.memory), (OpKind::Noop, known));
    assert_eq!(active(&f).len(), 1, "no new memory");
    assert!(
        f.engine.history(known).unwrap().len() <= before + 1,
        "at most the log of the no-op"
    );
}

// S-27: the checks before the judge still run, and the proposer's quota counts.
#[test]
fn secrets_and_the_proposers_quota_are_checked_before_the_judge_is_listened_to() {
    let f = fixture();
    let j = f.engine.judge().unwrap();
    let key = format!("{}{}", "sk-", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6");
    let e = f
        .engine
        .propose_judged(
            &j,
            external(&f, &format!("mi clave es {key}")),
            Judged::Add,
            sure(),
        )
        .unwrap_err();
    assert!(matches!(e, Error::SecretDetected), "{e:?}");
    let tight = f
        .engine
        .register_actor(
            "poco",
            ActorKind::Agent,
            Quota {
                max: 2,
                window_secs: 3600,
            },
        )
        .unwrap();
    let mut last = None;
    for i in 0..3 {
        let mut p = prop(f.scope, tight, &format!("hecho número {i}"));
        p.origin = Origin::External;
        last = Some(f.engine.propose_judged(&j, p, Judged::Add, sure()));
    }
    assert!(matches!(last.unwrap().unwrap_err(), Error::QuotaExceeded));
}

// S-25: the judge proposes nothing and reviews nothing.
#[test]
fn the_judge_has_no_review_powers_and_cannot_propose_for_itself() {
    let f = fixture();
    let j = f.engine.judge().unwrap();
    let o = f
        .engine
        .propose_judged(&j, external(&f, "dato"), Judged::Add, sure())
        .unwrap();
    let judge_actor = f.engine.history(o.memory).unwrap()[0].actor;
    let waiting = f
        .engine
        .propose(external(&f, "otro dato de fuera"))
        .unwrap()
        .memory;
    assert!(matches!(
        f.engine.promote(waiting, judge_actor).unwrap_err(),
        Error::NotAllowed
    ));
    assert!(matches!(
        f.engine.archive(o.memory, judge_actor).unwrap_err(),
        Error::NotAllowed
    ));
    let mut p = prop(f.scope, judge_actor, "lo propone el juez");
    p.origin = Origin::External;
    let e = f
        .engine
        .propose_judged(&j, p, Judged::Add, sure())
        .unwrap_err();
    assert!(matches!(e, Error::NotAllowed), "{e:?}");
}

#[test]
fn the_decision_log_says_the_judge_decided_and_how_sure_it_was() {
    let f = fixture();
    let j = f.engine.judge().unwrap();
    f.engine
        .propose_judged(
            &j,
            external(&f, "dato"),
            Judged::Add,
            Sureness { confidence: 0.87 },
        )
        .unwrap();
    let log = f.engine.decisions(f.scope, 0, i64::MAX).unwrap();
    let d = log
        .iter()
        .find(|d| d.provider == "jev")
        .expect("the judge is in the log");
    assert!(d.acted);
    assert!((d.confidence.unwrap() - 0.87).abs() < 1e-6);
    assert!(d.reason.as_deref().unwrap().contains("judged: add"));
}
