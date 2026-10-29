#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use clio_memory::*;
use common::*;

fn third(f: &Fixture) -> ActorId {
    f.engine
        .register_actor("tercero", ActorKind::Agent, Quota::default())
        .unwrap()
}

fn fact(f: &Fixture) -> MemoryId {
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap()
        .memory
}

fn state(f: &Fixture, m: MemoryId) -> State {
    f.engine.get(m).unwrap().state
}

fn wrong(f: &Fixture, m: MemoryId, actor: ActorId) {
    f.engine.feedback(m, Signal::Wrong, actor).unwrap();
}

#[test]
fn t19_three_wrong_signals_send_a_memory_to_review() {
    let f = fixture();
    let m = fact(&f);
    let t = third(&f);
    wrong(&f, m, f.person);
    wrong(&f, m, f.agent);
    assert_eq!(state(&f, m), State::Active, "two signals are not enough");
    wrong(&f, m, t);
    assert_eq!(state(&f, m), State::Quarantine);
    let last = f.engine.history(m).unwrap().pop().unwrap();
    assert_eq!(
        (last.kind, last.actor),
        (OpKind::Flag, t),
        "signed by the actor who gave the third signal"
    );
    assert!(last.reason.contains("equivocado"), "{}", last.reason);
    assert_eq!(
        f.engine
            .list(f.scope, Some(State::Quarantine), None)
            .unwrap()
            .len(),
        1
    );
    assert!(
        f.engine
            .search(f.scope, "Medellín", SearchOpts::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_person_resolves_a_flag_with_promote_or_archive_and_revert_restores_it() {
    let f = fixture();
    let (a, b) = (
        fact(&f),
        f.engine
            .propose(prop(f.scope, f.person, "tiene un coche rojo"))
            .unwrap()
            .memory,
    );
    let t = third(&f);
    for m in [a, b] {
        for who in [f.person, f.agent, t] {
            wrong(&f, m, who);
        }
        assert_eq!(state(&f, m), State::Quarantine);
    }
    f.engine.promote(a, f.person).unwrap();
    assert_eq!(state(&f, a), State::Active);
    f.engine.archive(b, f.person).unwrap();
    assert_eq!(state(&f, b), State::Archived);

    let c = f
        .engine
        .propose(prop(f.scope, f.person, "prefiere respuestas breves"))
        .unwrap()
        .memory;
    for who in [f.person, f.agent, t] {
        wrong(&f, c, who);
    }
    let flag = f.engine.history(c).unwrap().last().unwrap().id;
    f.engine.revert(flag, f.person).unwrap();
    assert_eq!(
        state(&f, c),
        State::Active,
        "revert of a Flag brings the memory back"
    );
}

#[test]
fn promote_spends_wrong_signals() {
    let f = fixture();
    let m = fact(&f);
    let t = third(&f);
    for who in [f.person, f.agent, t] {
        wrong(&f, m, who);
    }
    f.engine.promote(m, f.person).unwrap();
    assert_eq!(
        f.engine.feedback_summary(m).unwrap().wrong,
        0,
        "a person vindicated it"
    );
    wrong(&f, m, f.agent);
    wrong(&f, m, f.agent);
    assert_eq!(state(&f, m), State::Active, "two new signals do not flag");
    wrong(&f, m, f.agent);
    assert_eq!(state(&f, m), State::Quarantine, "three new ones do");
}

#[test]
fn a_reverted_flag_needs_three_new_signals_to_flag_again() {
    let f = fixture();
    let m = fact(&f);
    for _ in 0..3 {
        wrong(&f, m, f.agent);
    }
    let flag = f.engine.history(m).unwrap().last().unwrap().id;
    f.engine.revert(flag, f.person).unwrap();
    assert_eq!(state(&f, m), State::Active);
    wrong(&f, m, f.agent);
    assert_eq!(state(&f, m), State::Active, "the first three were spent");
    wrong(&f, m, f.agent);
    wrong(&f, m, f.agent);
    assert_eq!(state(&f, m), State::Quarantine);
}

#[test]
fn useful_raises_and_wrong_lowers_confidence() {
    let f = fixture();
    let m = fact(&f);
    assert_eq!(f.engine.confidence(m).unwrap(), 0.5);
    f.engine.feedback(m, Signal::Useful, f.agent).unwrap();
    assert!((f.engine.confidence(m).unwrap() - 0.6).abs() < 1e-6);
    wrong(&f, m, f.agent);
    let s = f.engine.feedback_summary(m).unwrap();
    assert_eq!((s.useful, s.wrong), (1, 1));
    assert!((s.confidence - 0.4).abs() < 1e-6, "0.5 + 0.1 - 0.2");
    for _ in 0..8 {
        f.engine.feedback(m, Signal::Useful, f.agent).unwrap();
    }
    assert_eq!(
        f.engine.confidence(m).unwrap(),
        1.0,
        "nine useful signals saturate at 1"
    );
}

#[test]
fn confidence_is_clamped_to_zero_and_one() {
    let f = fixture();
    let (hi, lo) = (
        fact(&f),
        f.engine
            .propose(prop(f.scope, f.person, "otro dato"))
            .unwrap()
            .memory,
    );
    let busy = f
        .engine
        .register_actor(
            "ocupado",
            ActorKind::Agent,
            Quota {
                max: 1000,
                window_secs: 3600,
            },
        )
        .unwrap();
    for _ in 0..10 {
        f.engine.feedback(hi, Signal::Useful, busy).unwrap();
    }
    assert_eq!(f.engine.confidence(hi).unwrap(), 1.0);
    for _ in 0..2 {
        wrong(&f, lo, busy);
    }
    assert!((f.engine.confidence(lo).unwrap() - 0.1).abs() < 1e-6);
}

#[test]
fn feedback_needs_an_active_memory() {
    let f = fixture();
    let m = fact(&f);
    f.engine.archive(m, f.person).unwrap();
    assert!(matches!(
        f.engine.feedback(m, Signal::Useful, f.agent),
        Err(Error::InvalidState)
    ));
    let mut ext = prop(f.scope, f.person, "dato externo");
    ext.origin = Origin::External;
    let q = f.engine.propose(ext).unwrap().memory;
    assert!(matches!(
        f.engine.feedback(q, Signal::Wrong, f.agent),
        Err(Error::InvalidState)
    ));
    assert!(matches!(
        f.engine.feedback(MemoryId(999), Signal::Wrong, f.agent),
        Err(Error::NotFound)
    ));
}

#[test]
fn system_actor_cannot_give_feedback() {
    let f = fixture();
    f.clock.set(1000);
    f.engine.set_retention(f.scope, Some(1), f.person).unwrap();
    let mut p = prop(f.scope, f.person, "viejo");
    p.kind = MemoryType::Episodic;
    let old = f.engine.propose(p).unwrap().memory;
    f.clock.set(1000 + 3 * 86_400);
    f.engine.enqueue_decay(f.scope).unwrap();
    f.engine.run_jobs(&RunOpts::default()).unwrap();
    let system = f.engine.history(old).unwrap().last().unwrap().actor;
    let m = fact(&f);
    assert!(matches!(
        f.engine.feedback(m, Signal::Useful, system),
        Err(Error::NotAllowed)
    ));
}

#[test]
fn feedback_counts_against_the_quota() {
    let f = fixture();
    let m = fact(&f);
    let limited = f
        .engine
        .register_actor(
            "limitado",
            ActorKind::Agent,
            Quota {
                max: 2,
                window_secs: 3600,
            },
        )
        .unwrap();
    f.engine.feedback(m, Signal::Useful, limited).unwrap();
    f.engine.feedback(m, Signal::Useful, limited).unwrap();
    assert!(matches!(
        f.engine.feedback(m, Signal::Useful, limited),
        Err(Error::QuotaExceeded)
    ));
}

// S-25: feedback can only send a memory to review; it never deletes, replaces or edits anything
#[test]
fn feedback_never_changes_content_or_deletes() {
    let f = fixture();
    let m = fact(&f);
    let before = f.engine.get(m).unwrap();
    let t = third(&f);
    f.engine.feedback(m, Signal::Useful, f.agent).unwrap();
    wrong(&f, m, f.person);
    wrong(&f, m, f.agent);
    let after = f.engine.get(m).unwrap();
    assert_eq!(
        (after.content, after.kind, after.topic_key),
        (before.content, before.kind, before.topic_key)
    );
    assert_eq!(
        f.engine.history(m).unwrap().len(),
        1,
        "no operation below the third signal"
    );
    wrong(&f, m, t);
    let kinds: Vec<_> = f
        .engine
        .history(m)
        .unwrap()
        .iter()
        .map(|o| o.kind)
        .collect();
    assert_eq!(kinds, [OpKind::Add, OpKind::Flag]);
    assert_eq!(
        f.engine.get(m).unwrap().content,
        "vive en Medellín",
        "the text is untouched"
    );
}

#[test]
fn signals_survive_rebuild_projection() {
    let f = fixture();
    let m = fact(&f);
    f.engine.feedback(m, Signal::Useful, f.agent).unwrap();
    wrong(&f, m, f.agent);
    f.engine.rebuild_projection().unwrap();
    let s = f.engine.feedback_summary(m).unwrap();
    assert_eq!((s.useful, s.wrong), (1, 1));
}

// ---- confidence in recall and protection from forgetting ----

#[test]
fn recall_prefers_the_trusted_memory_on_a_tie() {
    let f = fixture();
    let rojo = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap()
        .memory;
    let azul = f
        .engine
        .propose(prop(f.scope, f.person, "tiene un coche azul"))
        .unwrap()
        .memory;
    let first = |f: &Fixture| {
        f.engine
            .recall(f.scope, "tiene coche", &RecallOpts::default())
            .unwrap()
            .hits[0]
            .memory
            .id
    };
    assert_eq!(
        first(&f),
        rojo,
        "without feedback the older memory comes first"
    );
    for _ in 0..3 {
        f.engine.feedback(azul, Signal::Useful, f.agent).unwrap();
    }
    assert_eq!(first(&f), azul, "the trusted one now wins the tie");
}

#[test]
fn recall_hit_carries_its_confidence() {
    let f = fixture();
    let m = fact(&f);
    f.engine.feedback(m, Signal::Useful, f.agent).unwrap();
    f.engine.feedback(m, Signal::Useful, f.agent).unwrap();
    let r = f
        .engine
        .recall(f.scope, "Medellín", &RecallOpts::default())
        .unwrap();
    assert!((r.hits[0].confidence - 0.7).abs() < 1e-6);
}

#[test]
fn useful_feedback_protects_from_decay() {
    const DAY: i64 = 86_400;
    let f = fixture();
    f.clock.set(1000);
    f.engine.set_retention(f.scope, Some(10), f.person).unwrap();
    let episodic = |f: &Fixture, c: &str| {
        let mut p = prop(f.scope, f.person, c);
        p.kind = MemoryType::Episodic;
        f.engine.propose(p).unwrap().memory
    };
    let praised = episodic(&f, "dato elogiado");
    let plain = episodic(&f, "dato común");
    f.clock.set(1000 + 11 * DAY);
    f.engine.feedback(praised, Signal::Useful, f.agent).unwrap();
    f.engine.enqueue_decay(f.scope).unwrap();
    f.engine.run_jobs(&RunOpts::default()).unwrap();
    assert_eq!(
        state(&f, praised),
        State::Active,
        "a useful signal counts as activity"
    );
    assert_eq!(state(&f, plain), State::Archived);
}
