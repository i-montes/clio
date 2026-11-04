#![allow(clippy::unwrap_used)] // tests: panicking is the failure mode
//! The quarantine is where what the agent forgot goes: no longer recalled, its record kept, back
//! with revert (spec `2026-11-04-clio-memory-standalone-design.md`). Nothing the agent writes
//! lands there, so the cap of pending candidates never blocks a write.

mod common;

use clio_memory::*;
use common::*;

fn pending(f: &Fixture) -> usize {
    f.engine
        .list(f.scope, Some(State::Quarantine), None)
        .unwrap()
        .len()
}

#[test]
fn writes_never_meet_the_cap_of_the_quarantine() {
    let f = fixture();
    let f = Fixture {
        engine: f.engine.with_limits(Limits {
            max_quarantine_per_actor: 1,
            ..Limits::default()
        }),
        ..f
    };
    for i in 0..3 {
        let mut p = prop(f.scope, f.agent, &format!("dato de fuera número {i}"));
        p.origin = Origin::External;
        assert_eq!(f.engine.propose(p).unwrap().state, State::Active);
    }
    assert_eq!(pending(&f), 0);
}

#[test]
fn a_text_equal_to_a_forgotten_memory_is_written_again() {
    let f = fixture();
    let old = quarantined(&f, "vive en Bogotá");
    let again = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Bogotá"))
        .unwrap();
    assert_eq!(again.kind, OpKind::Add);
    assert_ne!(again.memory, old);
    assert_eq!(f.engine.get(again.memory).unwrap().state, State::Active);
    assert_eq!(pending(&f), 1, "the forgotten one stays forgotten");
}
