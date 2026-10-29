#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use clio_memory::*;
use common::*;

fn topical(f: &Fixture, content: &str) -> Proposal {
    let mut p = prop(f.scope, f.person, content);
    p.topic_key = Some("residencia".into());
    p
}

fn external(f: &Fixture, content: &str) -> Proposal {
    let mut p = topical(f, content);
    p.origin = Origin::External;
    p
}

#[test]
fn update_replaces_and_closes_validity() {
    let f = fixture();
    f.clock.set(100);
    let old = f.engine.propose(topical(&f, "vive en Medellín")).unwrap();
    f.clock.set(200);
    let new = f.engine.propose(topical(&f, "vive en Bogotá")).unwrap();
    assert_eq!(new.kind, OpKind::Update);

    let old_row = f.engine.get(old.memory).unwrap();
    assert_eq!(old_row.state, State::Replaced);
    assert_eq!(
        (old_row.valid_to, old_row.replaced_by),
        (Some(200), Some(new.memory))
    );
    assert_eq!(f.engine.get(new.memory).unwrap().state, State::Active);

    let now = f
        .engine
        .search(f.scope, "Medellín", SearchOpts::default())
        .unwrap();
    assert!(now.is_empty());
    let then = SearchOpts {
        as_of: Some(150),
        ..SearchOpts::default()
    };
    assert_eq!(f.engine.search(f.scope, "Medellín", then).unwrap().len(), 1);
}

#[test]
fn update_with_earlier_valid_from_is_rejected() {
    let f = fixture();
    f.clock.set(500);
    let first = f.engine.propose(topical(&f, "vive en Medellín")).unwrap();
    let mut p = topical(&f, "vive en Bogotá");
    p.valid_from = Some(400);
    assert!(matches!(f.engine.propose(p), Err(Error::InvalidValidity)));
    assert_eq!(f.engine.list(f.scope, None, None).unwrap().len(), 1);
    assert_eq!(f.engine.history(first.memory).unwrap().len(), 1);
}

// S-25: untrusted updates cannot replace trusted memories directly
#[test]
fn s25_external_update_waits_in_quarantine_until_promoted() {
    let f = fixture();
    f.clock.set(100);
    let old = f.engine.propose(topical(&f, "vive en Medellín")).unwrap();
    f.clock.set(200);
    let cand = f.engine.propose(external(&f, "vive en Bogotá")).unwrap();
    assert_eq!(cand.state, State::Quarantine);
    assert_eq!(
        f.engine.get(cand.memory).unwrap().supersedes,
        Some(old.memory)
    );
    assert_eq!(f.engine.get(old.memory).unwrap().state, State::Active);

    f.engine.promote(cand.memory, f.person).unwrap();
    assert_eq!(f.engine.get(old.memory).unwrap().state, State::Replaced);
    assert_eq!(f.engine.get(cand.memory).unwrap().state, State::Active);
}

#[test]
fn promote_requires_person() {
    let f = fixture();
    let cand = f.engine.propose(external(&f, "dato externo")).unwrap();
    assert!(matches!(
        f.engine.promote(cand.memory, f.agent),
        Err(Error::NotAllowed)
    ));
    assert_eq!(f.engine.get(cand.memory).unwrap().state, State::Quarantine);
}

#[test]
fn promote_with_stale_target_only_activates() {
    let f = fixture();
    f.clock.set(100);
    let old = f.engine.propose(topical(&f, "vive en Medellín")).unwrap();
    f.clock.set(200);
    let cand = f.engine.propose(external(&f, "vive en Bogotá")).unwrap();
    f.engine.archive(old.memory, f.person).unwrap();
    f.engine.promote(cand.memory, f.person).unwrap();
    assert_eq!(f.engine.get(old.memory).unwrap().state, State::Archived);
    assert_eq!(f.engine.get(cand.memory).unwrap().state, State::Active);
}

#[test]
fn promote_non_quarantine_is_invalid_state() {
    let f = fixture();
    let m = f.engine.propose(topical(&f, "algo")).unwrap();
    assert!(matches!(
        f.engine.promote(m.memory, f.person),
        Err(Error::InvalidState)
    ));
}

#[test]
fn archive_hides_from_search() {
    let f = fixture();
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "dato viejo"))
        .unwrap();
    f.engine.archive(m.memory, f.person).unwrap();
    assert_eq!(f.engine.get(m.memory).unwrap().state, State::Archived);
    assert!(
        f.engine
            .search(f.scope, "viejo", SearchOpts::default())
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        f.engine.archive(m.memory, f.person),
        Err(Error::InvalidState)
    ));
}

#[test]
fn promote_older_candidate_never_leaves_two_active_for_one_topic() {
    let f = fixture();
    f.clock.set(100);
    f.engine.propose(topical(&f, "vive en Medellín")).unwrap();
    f.clock.set(200);
    let cand = f.engine.propose(external(&f, "vive en Bogotá")).unwrap();
    f.clock.set(300);
    let cur = f.engine.propose(topical(&f, "vive en Cali")).unwrap();
    f.engine.promote(cand.memory, f.person).unwrap();
    let active = f.engine.list(f.scope, Some(State::Active), None).unwrap();
    assert_eq!(
        active.iter().map(|r| r.id).collect::<Vec<_>>(),
        [cur.memory]
    );
    let c = f.engine.get(cand.memory).unwrap();
    assert_eq!(
        (c.state, c.valid_to, c.replaced_by),
        (State::Replaced, Some(300), Some(cur.memory))
    );
}

#[test]
fn promote_without_supersedes_still_respects_current_topic_owner() {
    let f = fixture();
    f.clock.set(100);
    let cand = f.engine.propose(external(&f, "vive en Bogotá")).unwrap();
    f.clock.set(200);
    let cur = f.engine.propose(topical(&f, "vive en Cali")).unwrap();
    f.engine.promote(cand.memory, f.person).unwrap();
    let active = f.engine.list(f.scope, Some(State::Active), None).unwrap();
    assert_eq!(
        active.iter().map(|r| r.id).collect::<Vec<_>>(),
        [cur.memory]
    );
}

#[test]
fn update_keeps_an_explicitly_closed_validity() {
    let f = fixture();
    f.clock.set(100);
    let mut p = topical(&f, "trabaja en Acme");
    p.valid_to = Some(200);
    let old = f.engine.propose(p).unwrap();
    f.clock.set(500);
    f.engine.propose(topical(&f, "trabaja en Globex")).unwrap();
    assert_eq!(f.engine.get(old.memory).unwrap().valid_to, Some(200));
    let at = SearchOpts {
        as_of: Some(300),
        ..SearchOpts::default()
    };
    assert!(f.engine.search(f.scope, "Acme", at).unwrap().is_empty());
}

#[test]
fn backdated_replacement_is_invisible_before_it_was_known() {
    let f = fixture();
    f.clock.set(100);
    let old = f.engine.propose(topical(&f, "vive en Bogotá")).unwrap();
    f.clock.set(200);
    let mut p = topical(&f, "vive en Medellín");
    p.valid_from = Some(150);
    let new = f.engine.propose(p).unwrap();
    let q = |known| SearchOpts {
        as_of: Some(170),
        known_at: Some(known),
        ..SearchOpts::default()
    };
    let ids = |known| -> Vec<MemoryId> {
        f.engine
            .search(f.scope, "vive", q(known))
            .unwrap()
            .iter()
            .map(|h| h.memory.id)
            .collect()
    };
    assert_eq!(ids(180), [old.memory]);
    assert_eq!(ids(250), [new.memory]);
}

#[test]
fn search_query_is_nfc_normalized() {
    let f = fixture();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let hits = f
        .engine
        .search(f.scope, "Medelli\u{301}n", SearchOpts::default())
        .unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn two_updates_in_the_same_second_both_apply() {
    let f = fixture();
    f.clock.set(100);
    for c in ["uno", "dos", "tres"] {
        f.engine.propose(topical(&f, c)).unwrap();
    }
    let rows = f.engine.list(f.scope, None, None).unwrap();
    // no invented dates: all start now; the replaced ones were valid for zero seconds
    assert_eq!(
        rows.iter().map(|r| r.valid_from).collect::<Vec<_>>(),
        [100, 100, 100]
    );
    assert_eq!(
        rows.iter().map(|r| r.valid_to).collect::<Vec<_>>(),
        [Some(100), Some(100), None]
    );
    assert!(rows.iter().all(|r| r.known_since == 100));
    assert_eq!(rows.iter().filter(|r| r.state == State::Active).count(), 1);
    assert_eq!(rows[2].state, State::Active);
}

#[test]
fn same_second_update_leaves_the_latest_visible_at_that_second() {
    let f = fixture();
    f.clock.set(100);
    for c in ["vive en Medellín", "vive en Bogotá"] {
        f.engine.propose(topical(&f, c)).unwrap();
    }
    let at = |t| SearchOpts {
        as_of: Some(t),
        ..SearchOpts::default()
    };
    let now = f.engine.search(f.scope, "vive", at(100)).unwrap();
    assert_eq!(
        now.iter()
            .map(|h| h.memory.content.as_str())
            .collect::<Vec<_>>(),
        ["vive en Bogotá"]
    );
    assert!(f.engine.search(f.scope, "vive", at(99)).unwrap().is_empty());
}

#[test]
fn explicit_valid_from_not_after_the_old_one_is_still_rejected() {
    let f = fixture();
    f.clock.set(100);
    let first = f.engine.propose(topical(&f, "uno")).unwrap();
    let mut p = topical(&f, "dos");
    p.valid_from = Some(100);
    assert!(matches!(f.engine.propose(p), Err(Error::InvalidValidity)));
    assert_eq!(f.engine.list(f.scope, None, None).unwrap().len(), 1);
    assert_eq!(f.engine.history(first.memory).unwrap().len(), 1);
}

#[test]
fn defaulted_update_before_a_future_dated_owner_is_rejected() {
    let f = fixture();
    f.clock.set(100);
    let mut p = topical(&f, "estará en X");
    p.valid_from = Some(1_000_000); // a fact known in advance
    let owner = f.engine.propose(p).unwrap();
    let r = f.engine.propose(topical(&f, "está en Y"));
    assert!(
        matches!(r, Err(Error::InvalidValidity)),
        "the caller must say when Y starts"
    );
    assert_eq!(f.engine.list(f.scope, None, None).unwrap().len(), 1);
    assert_eq!(f.engine.get(owner.memory).unwrap().valid_to, None);
}

#[test]
fn promote_with_equal_start_is_rejected() {
    let f = fixture();
    f.clock.set(100);
    let owner = f.engine.propose(topical(&f, "uno")).unwrap();
    let cand = f.engine.propose(external(&f, "dos")).unwrap();
    assert!(matches!(
        f.engine.promote(cand.memory, f.person),
        Err(Error::InvalidValidity)
    ));
    assert_eq!(f.engine.get(cand.memory).unwrap().state, State::Quarantine);
    assert_eq!(f.engine.get(owner.memory).unwrap().state, State::Active);
}
