#![allow(clippy::unwrap_used)] // tests: panicking is the failure mode
//! The quarantine is where a person reviews what a model or outside text wants remembered. If one
//! actor can fill it without end, what is legitimate sits buried under it (second audit, finding
//! 7): each actor has a cap of pending candidates, and the same untrusted text is one candidate.

mod common;

use clio_memory::*;
use common::*;

fn external(f: &Fixture, actor: ActorId, text: &str) -> Proposal {
    let mut p = prop(f.scope, actor, text);
    p.origin = Origin::External;
    p
}

fn capped(f: Fixture, cap: usize) -> Fixture {
    let engine = f.engine.with_limits(Limits {
        max_quarantine_per_actor: cap,
        ..Limits::default()
    });
    Fixture { engine, ..f }
}

fn pending(f: &Fixture) -> usize {
    f.engine
        .list(f.scope, Some(State::Quarantine), None)
        .unwrap()
        .len()
}

#[test]
fn the_default_cap_is_fifty_per_actor() {
    assert_eq!(Limits::default().max_quarantine_per_actor, 50);
}

// S-25 S-27
#[test]
fn an_actor_cannot_fill_the_quarantine_past_its_cap() {
    let f = capped(fixture(), 3);
    for i in 0..3 {
        let o = f
            .engine
            .propose(external(&f, f.agent, &format!("dato de fuera número {i}")))
            .unwrap();
        assert_eq!(o.state, State::Quarantine);
    }
    let err = f
        .engine
        .propose(external(&f, f.agent, "uno más que no cabe"))
        .unwrap_err();
    assert!(matches!(err, Error::QuarantineFull), "{err:?}");
    assert_eq!(pending(&f), 3, "nothing was written by the refused one");
}

// S-25 S-27
#[test]
fn the_cap_is_per_actor_and_does_not_stop_trusted_writes() {
    let f = capped(fixture(), 2);
    let other = f
        .engine
        .register_actor("otro", ActorKind::Agent, Quota::default())
        .unwrap();
    for i in 0..2 {
        f.engine
            .propose(external(&f, f.agent, &format!("relleno {i}")))
            .unwrap();
    }
    assert!(
        f.engine
            .propose(external(&f, f.agent, "relleno 2"))
            .is_err()
    );
    // Another actor has its own room.
    assert_eq!(
        f.engine
            .propose(external(&f, other, "de otro actor"))
            .unwrap()
            .state,
        State::Quarantine
    );
    // What the person writes goes straight to active and is never counted against anyone.
    assert_eq!(
        f.engine
            .propose(prop(f.scope, f.person, "vive en Medellín"))
            .unwrap()
            .state,
        State::Active
    );
}

// S-25 S-27
#[test]
fn reviewing_makes_room() {
    let f = capped(fixture(), 2);
    let first = f.engine.propose(external(&f, f.agent, "primero")).unwrap();
    f.engine.propose(external(&f, f.agent, "segundo")).unwrap();
    assert!(f.engine.propose(external(&f, f.agent, "tercero")).is_err());
    f.engine.archive(first.memory, f.person).unwrap();
    assert_eq!(
        f.engine
            .propose(external(&f, f.agent, "tercero"))
            .unwrap()
            .state,
        State::Quarantine
    );
}

// S-25 S-27
#[test]
fn the_same_untrusted_text_twice_is_one_candidate_and_costs_one_place() {
    let f = capped(fixture(), 2);
    let a = f
        .engine
        .propose(external(&f, f.agent, "El mismo texto de fuera"))
        .unwrap();
    let b = f
        .engine
        .propose(external(&f, f.agent, "El  mismo texto de fuera "))
        .unwrap();
    assert_eq!(a.memory, b.memory, "the second is the first, not a new row");
    assert_eq!(pending(&f), 1);
    // The repetition did not spend the cap: there is still room for another one.
    f.engine
        .propose(external(&f, f.agent, "otro distinto"))
        .unwrap();
    assert_eq!(pending(&f), 2);
}

// S-25 S-27
#[test]
fn a_persons_text_equal_to_a_pending_candidate_still_becomes_active() {
    let f = fixture();
    f.engine
        .propose(external(&f, f.agent, "vive en Bogotá"))
        .unwrap();
    let mine = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Bogotá"))
        .unwrap();
    // `Outcome.state` says Active even for a no-op, so look at what was really written.
    assert_eq!(mine.kind, OpKind::Add, "not swallowed by the candidate");
    assert_eq!(f.engine.get(mine.memory).unwrap().state, State::Active);
    assert_eq!(pending(&f), 1, "the candidate is still waiting for review");
}

// S-25 S-27
#[test]
fn a_procedure_proposed_by_an_agent_counts_against_the_cap_too() {
    let f = capped(fixture(), 1);
    let mut p = prop(f.scope, f.agent, "cuando falle, reintenta tres veces");
    p.kind = MemoryType::Procedural;
    p.origin = Origin::Agent; // not external, but a procedure waits for a person (S-26)
    assert_eq!(f.engine.propose(p).unwrap().state, State::Quarantine);
    let mut q = prop(f.scope, f.agent, "otro procedimiento distinto");
    q.kind = MemoryType::Procedural;
    q.origin = Origin::Agent;
    assert!(matches!(
        f.engine.propose(q).unwrap_err(),
        Error::QuarantineFull
    ));
}
