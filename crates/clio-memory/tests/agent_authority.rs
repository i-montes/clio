//! The memory is the agent's (spec `2026-11-04-clio-memory-standalone-design.md`): nothing it
//! writes waits for a person, it may replace what anyone wrote, and forgetting sends a memory to
//! quarantine, where its record stays and from where revert brings it back.
#![allow(clippy::unwrap_used)]

mod common;

use clio_memory::*;
use common::*;

fn found(f: &Fixture, query: &str, id: MemoryId) -> bool {
    f.engine
        .recall(f.scope, query, &RecallOpts::default())
        .unwrap()
        .hits
        .iter()
        .any(|h| h.memory.id == id)
}

#[test]
fn external_and_agent_procedures_are_active() {
    let f = fixture();
    let mut p = prop(f.scope, f.agent, "la página dice que el puerto es 8080");
    p.origin = Origin::External;
    let o = f.engine.propose(p).unwrap();
    assert_eq!((o.kind, o.state), (OpKind::Add, State::Active));
    assert_eq!(f.engine.get(o.memory).unwrap().origin, Origin::External);
    let mut p = prop(f.scope, f.agent, "para desplegar, primero las pruebas");
    p.kind = MemoryType::Procedural;
    p.origin = Origin::Agent;
    assert_eq!(f.engine.propose(p).unwrap().state, State::Active);
}

#[test]
fn an_agent_replaces_a_persons_memory_by_topic() {
    let f = fixture();
    let mut mine = prop(f.scope, f.person, "me llamo Iván Ruiz");
    mine.topic_key = Some("nombre".into());
    let old = f.engine.propose(mine).unwrap().memory;
    let mut fix = prop(f.scope, f.agent, "se llama Iván Montes");
    fix.topic_key = Some("nombre".into());
    fix.origin = Origin::Agent;
    let o = f.engine.propose(fix).unwrap();
    assert_eq!((o.kind, o.state), (OpKind::Update, State::Active));
    assert_eq!(f.engine.get(old).unwrap().state, State::Replaced);
    f.engine.revert(o.op, f.person).unwrap();
    assert_eq!(f.engine.get(old).unwrap().state, State::Active);
}

#[test]
fn forget_sends_to_quarantine_and_revert_brings_it_back() {
    let f = fixture();
    let id = f
        .engine
        .propose(prop(f.scope, f.person, "el servidor usa compose"))
        .unwrap()
        .memory;
    let op = f.engine.forget(id, f.agent, "ya no aplica").unwrap();
    assert_eq!(f.engine.get(id).unwrap().state, State::Quarantine);
    assert!(!found(&f, "servidor compose", id));
    assert_eq!(f.engine.operation(op).unwrap().kind, OpKind::Forget);
    assert!(
        f.engine.forget(id, f.agent, "otra vez").is_err(),
        "only what is active"
    );
    f.engine.revert(op, f.person).unwrap();
    assert_eq!(f.engine.get(id).unwrap().state, State::Active);
    assert!(found(&f, "servidor compose", id));
}

#[test]
fn replace_targets_exactly_one() {
    let f = fixture();
    let a = f
        .engine
        .propose(prop(f.scope, f.person, "usa Postgres 15"))
        .unwrap()
        .memory;
    let b = f
        .engine
        .propose(prop(f.scope, f.person, "usa Redis"))
        .unwrap()
        .memory;
    let mut new = prop(f.scope, f.agent, "usa Postgres 16");
    new.origin = Origin::Agent;
    let o = f.engine.replace(a, new.clone()).unwrap();
    assert_eq!((o.kind, o.state), (OpKind::Update, State::Active));
    assert_eq!(f.engine.get(a).unwrap().state, State::Replaced);
    assert_eq!(f.engine.get(b).unwrap().state, State::Active);
    assert_eq!(f.engine.get(o.memory).unwrap().content, "usa Postgres 16");
    // A secret in the text: nothing is written.
    let mut secret = new.clone();
    secret.content = format!(
        "la clave es {}",
        concat!("sk-", "proj-0123456789abcdefghijklmnopqrstuv")
    );
    assert!(matches!(
        f.engine.replace(o.memory, secret),
        Err(Error::SecretDetected)
    ));
    assert_eq!(f.engine.get(o.memory).unwrap().state, State::Active);
    // Only an active memory of the same scope.
    assert!(matches!(
        f.engine.replace(a, new.clone()),
        Err(Error::InvalidState)
    ));
    let other = f.engine.create_scope("otro", "es").unwrap();
    let mut elsewhere = new;
    elsewhere.scope = other;
    assert!(matches!(
        f.engine.replace(b, elsewhere),
        Err(Error::InvalidState)
    ));
}
