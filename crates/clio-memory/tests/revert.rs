#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use clio_memory::*;
use common::*;

fn topical(f: &Fixture, actor: ActorId, content: &str) -> Proposal {
    let mut p = prop(f.scope, actor, content);
    p.topic_key = Some("residencia".into());
    p
}

fn state(f: &Fixture, m: MemoryId) -> State {
    f.engine.get(m).unwrap().state
}

#[test]
fn t10_revert_add_archives_and_logs() {
    let f = fixture();
    let add = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    let rev = f.engine.revert(add.op, f.person).unwrap();
    assert_eq!(state(&f, add.memory), State::Archived);
    let last = f.engine.history(add.memory).unwrap().pop().unwrap();
    assert_eq!(
        (last.id, last.kind, last.reverts),
        (rev, OpKind::Revert, Some(add.op))
    );
}

#[test]
fn revert_update_restores_old_and_archives_new() {
    let f = fixture();
    f.clock.set(100);
    let old = f
        .engine
        .propose(topical(&f, f.person, "vive en Medellín"))
        .unwrap();
    f.clock.set(200);
    let new = f
        .engine
        .propose(topical(&f, f.person, "vive en Bogotá"))
        .unwrap();
    f.engine.revert(new.op, f.person).unwrap();
    let old_row = f.engine.get(old.memory).unwrap();
    assert_eq!(old_row.state, State::Active);
    assert_eq!((old_row.valid_to, old_row.replaced_by), (None, None));
    assert_eq!(state(&f, new.memory), State::Archived);
}

#[test]
fn revert_promote_returns_to_quarantine() {
    let f = fixture();
    let mut p = prop(f.scope, f.person, "dato externo");
    p.origin = Origin::External;
    let cand = f.engine.propose(p).unwrap();
    let promote = f.engine.promote(cand.memory, f.person).unwrap();
    f.engine.revert(promote, f.person).unwrap();
    assert_eq!(state(&f, cand.memory), State::Quarantine);
}

#[test]
fn revert_of_revert_reapplies() {
    let f = fixture();
    let add = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    let rev = f.engine.revert(add.op, f.person).unwrap();
    f.engine.revert(rev, f.person).unwrap();
    assert_eq!(state(&f, add.memory), State::Active);
}

#[test]
fn revert_blocked_when_memory_changed_later() {
    let f = fixture();
    let other = f
        .engine
        .register_actor("otra", ActorKind::Person, Quota::default())
        .unwrap();
    f.clock.set(100);
    let add = f
        .engine
        .propose(topical(&f, f.person, "vive en Medellín"))
        .unwrap();
    f.clock.set(200);
    f.engine
        .propose(topical(&f, other, "vive en Bogotá"))
        .unwrap();
    let before = f.engine.history(add.memory).unwrap().len();
    assert!(matches!(
        f.engine.revert(add.op, f.person),
        Err(Error::NotRevertible(_))
    ));
    assert_eq!(f.engine.history(add.memory).unwrap().len(), before);
}

#[test]
fn t15_revert_actor_undoes_all_of_one_actor() {
    let f = fixture();
    let x = f
        .engine
        .register_actor("x", ActorKind::Person, Quota::default())
        .unwrap();
    let y = f
        .engine
        .register_actor("y", ActorKind::Person, Quota::default())
        .unwrap();
    let mine: Vec<_> = ["uno", "dos", "tres"]
        .iter()
        .map(|c| f.engine.propose(prop(f.scope, x, c)).unwrap())
        .collect();
    let theirs = f.engine.propose(prop(f.scope, y, "cuatro")).unwrap();

    let report = f.engine.revert_actor(x, f.person).unwrap();
    assert_eq!(report.reverted.len(), 3);
    assert!(report.skipped.is_empty());
    assert!(mine.iter().all(|o| state(&f, o.memory) == State::Archived));
    assert_eq!(state(&f, theirs.memory), State::Active);

    assert!(
        f.engine
            .revert_actor(x, f.person)
            .unwrap()
            .reverted
            .is_empty()
    );
}

#[test]
fn revert_actor_skips_covered_operations() {
    let f = fixture();
    let x = f
        .engine
        .register_actor("x", ActorKind::Person, Quota::default())
        .unwrap();
    f.clock.set(100);
    let add = f
        .engine
        .propose(topical(&f, x, "vive en Medellín"))
        .unwrap();
    f.clock.set(200);
    let upd = f
        .engine
        .propose(topical(&f, f.person, "vive en Bogotá"))
        .unwrap();
    let report = f.engine.revert_actor(x, f.person).unwrap();
    assert_eq!((report.reverted, report.skipped), (vec![], vec![add.op]));
    assert_eq!(state(&f, upd.memory), State::Active);
}

#[test]
fn revert_since_walks_newest_first() {
    let f = fixture();
    let ops: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|c| f.engine.propose(prop(f.scope, f.person, c)).unwrap())
        .collect();
    let report = f.engine.revert_since(ops[1].op, f.person).unwrap();
    assert_eq!(report.reverted.len(), 2);
    let log = f.engine.history(ops[2].memory).unwrap();
    let first_revert = f.engine.history(ops[1].memory).unwrap();
    assert!(log.last().unwrap().id < first_revert.last().unwrap().id);
    assert_eq!(state(&f, ops[0].memory), State::Active);
}

#[test]
fn revert_keeps_known_since() {
    let f = fixture();
    f.clock.set(100);
    let add = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    f.clock.set(500);
    f.engine.revert(add.op, f.person).unwrap();
    assert_eq!(f.engine.get(add.memory).unwrap().known_since, 100);
}

#[test]
fn revert_then_rebuild_matches() {
    let f = fixture();
    f.clock.set(100);
    let a = f
        .engine
        .propose(topical(&f, f.person, "vive en Medellín"))
        .unwrap();
    f.clock.set(200);
    let b = f
        .engine
        .propose(topical(&f, f.person, "vive en Bogotá"))
        .unwrap();
    let r = f.engine.revert(b.op, f.person).unwrap();
    f.engine.revert(r, f.person).unwrap();
    f.engine.revert(a.op, f.person).unwrap_err();
    let before = f.engine.list(f.scope, None, None).unwrap();
    f.engine.rebuild_projection().unwrap();
    assert_eq!(f.engine.list(f.scope, None, None).unwrap(), before);
}

#[test]
fn only_people_can_archive_or_revert() {
    let f = fixture();
    let m = f.engine.propose(prop(f.scope, f.person, "dato")).unwrap();
    assert!(matches!(
        f.engine.archive(m.memory, f.agent),
        Err(Error::NotAllowed)
    ));
    assert!(matches!(
        f.engine.revert(m.op, f.agent),
        Err(Error::NotAllowed)
    ));
    assert!(matches!(
        f.engine.revert_actor(f.person, f.agent),
        Err(Error::NotAllowed)
    ));
    assert!(matches!(
        f.engine.revert_since(m.op, f.agent),
        Err(Error::NotAllowed)
    ));
    assert_eq!(state(&f, m.memory), State::Active);
    assert!(matches!(
        f.engine.archive(m.memory, ActorId(999)),
        Err(Error::UnknownActor)
    ));
}
