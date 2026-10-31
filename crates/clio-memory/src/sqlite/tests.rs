#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use tempfile::TempDir;

use super::*;
use crate::error::Error;
use crate::model::{OpKind, Origin};

fn store() -> (TempDir, PathBuf, SqliteStore) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let s = SqliteStore::open(&path, OpenMode::ReadWrite).unwrap();
    (dir, path, s)
}

fn row(id: i64, scope: ScopeId, actor: ActorId, content: &str, from: i64) -> MemoryRow {
    MemoryRow {
        id: MemoryId(id),
        scope,
        kind: MemoryType::Semantic,
        content: content.into(),
        content_hash: format!("hash-{id}"),
        topic_key: None,
        origin: Origin::User,
        actor,
        session: None,
        episode: None,
        source: None,
        state: State::Active,
        valid_from: from,
        valid_to: None,
        known_since: from,
        replaced_by: None,
        supersedes: None,
    }
}

fn op(scope: ScopeId, actor: ActorId, kind: OpKind, at: i64, changes: Vec<Change>) -> NewOperation {
    NewOperation {
        scope,
        kind,
        memory: changes.first().map(|c| c.memory),
        changes,
        actor,
        reason: "test".into(),
        at,
        reverts: None,
        trace_id: None,
    }
}

fn add(m: &MemoryRow) -> Change {
    Change {
        memory: m.id,
        before: None,
        after: m.clone(),
    }
}

fn setup() -> (TempDir, PathBuf, SqliteStore, ScopeId, ActorId) {
    let (d, p, s) = store();
    let scope = s.create_scope("main", "es").unwrap();
    let actor = s
        .register_actor("ana", ActorKind::Person, Quota::default())
        .unwrap();
    (d, p, s, scope, actor)
}

fn put(s: &SqliteStore, scope: ScopeId, actor: ActorId, m: &MemoryRow) {
    s.append(op(scope, actor, OpKind::Add, m.known_since, vec![add(m)]))
        .unwrap();
}

fn ids(hits: &[Hit]) -> Vec<i64> {
    hits.iter().map(|h| h.memory.id.0).collect()
}

#[test]
fn t01_rebuild_projection_gives_identical_state() {
    let (_d, path, s, scope, actor) = setup();

    let m1 = row(1, scope, actor, "vive en Medellín", 100);
    put(&s, scope, actor, &m1);

    let m2 = row(2, scope, actor, "vive en Bogotá", 200);
    let mut old = m1.clone();
    old.state = State::Replaced;
    old.valid_to = Some(200);
    old.replaced_by = Some(m2.id);
    let update = vec![
        Change {
            memory: m1.id,
            before: Some(m1.clone()),
            after: old,
        },
        add(&m2),
    ];
    s.append(op(scope, actor, OpKind::Update, 200, update))
        .unwrap();

    let mut archived = m2.clone();
    archived.state = State::Archived;
    let change = Change {
        memory: m2.id,
        before: Some(m2.clone()),
        after: archived,
    };
    s.append(op(scope, actor, OpKind::Archive, 300, vec![change]))
        .unwrap();

    let before = s.list(scope, None, None).unwrap();
    assert_eq!(
        before.iter().map(|r| r.state).collect::<Vec<_>>(),
        [State::Replaced, State::Archived]
    );
    let at = SearchOpts {
        as_of: Some(150),
        ..SearchOpts::default()
    };
    let hits_before = s.search(scope, "vive", &at).unwrap();
    assert_eq!(hits_before.len(), 1);
    let history_before = s.history(m1.id).unwrap();
    assert_eq!(history_before.len(), 2);

    // Wipe the projection behind the store's back: only the log is left.
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "DELETE FROM memories; DELETE FROM memories_fts; DELETE FROM operation_memories;",
    )
    .unwrap();
    assert!(s.list(scope, None, None).unwrap().is_empty());

    s.rebuild_projection().unwrap();
    assert_eq!(s.list(scope, None, None).unwrap(), before);
    assert_eq!(s.search(scope, "vive", &at).unwrap(), hits_before);
    assert_eq!(s.history(m1.id).unwrap(), history_before);
}

#[test]
fn append_is_atomic() {
    let (_d, _p, s) = store();
    let actor = s
        .register_actor("ana", ActorKind::Person, Quota::default())
        .unwrap();
    let ghost = ScopeId(999);
    let m = row(1, ghost, actor, "x", 1);
    assert!(
        s.append(op(ghost, actor, OpKind::Add, 1, vec![add(&m)]))
            .is_err()
    );
    assert!(matches!(s.operation(OpId(1)), Err(Error::NotFound)));
}

#[test]
fn schema_too_new_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.db");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "user_version", 99).unwrap();
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        SqliteStore::open(&path, OpenMode::ReadWrite),
        Err(Error::SchemaTooNew(99))
    ));
}

#[test]
fn bump_quota_limits_per_window() {
    let (_d, _p, s) = store();
    let actor = s
        .register_actor(
            "bot",
            ActorKind::Agent,
            Quota {
                max: 2,
                window_secs: 3600,
            },
        )
        .unwrap();
    s.bump_quota(actor, 1000).unwrap();
    s.bump_quota(actor, 1000).unwrap();
    assert!(matches!(
        s.bump_quota(actor, 1000),
        Err(Error::QuotaExceeded)
    ));
    s.bump_quota(actor, 1000 + 3600).unwrap();
}

#[test]
fn search_finds_stemmed_match() {
    let (_d, _p, s, scope, actor) = setup();
    put(
        &s,
        scope,
        actor,
        &row(1, scope, actor, "corriendo en el parque", 10),
    );
    let hits = s.search(scope, "corren", &SearchOpts::default()).unwrap();
    assert_eq!(ids(&hits), [1]);
}

#[test]
fn search_ignores_other_scopes() {
    let (_d, _p, s, a, actor) = setup();
    let b = s.create_scope("other", "es").unwrap();
    put(&s, a, actor, &row(1, a, actor, "vive en Medellín", 10));
    put(&s, b, actor, &row(2, b, actor, "vive en Medellín", 10));
    let hits = s.search(a, "Medellín", &SearchOpts::default()).unwrap();
    assert_eq!(ids(&hits), [1]);
}

#[test]
fn quarantine_hidden_unless_requested() {
    let (_d, _p, s, scope, actor) = setup();
    let mut m = row(1, scope, actor, "dato externo", 10);
    m.state = State::Quarantine;
    put(&s, scope, actor, &m);
    assert!(
        s.search(scope, "dato", &SearchOpts::default())
            .unwrap()
            .is_empty()
    );
    let opts = SearchOpts {
        include_quarantine: true,
        ..SearchOpts::default()
    };
    assert_eq!(ids(&s.search(scope, "dato", &opts).unwrap()), [1]);
}

#[test]
fn t06_as_of_returns_fact_valid_then() {
    let (_d, _p, s, scope, actor) = setup();
    let mut old = row(1, scope, actor, "vive en Medellín", 100);
    old.state = State::Replaced;
    old.valid_to = Some(200);
    old.replaced_by = Some(MemoryId(2));
    put(&s, scope, actor, &old);
    put(
        &s,
        scope,
        actor,
        &row(2, scope, actor, "vive en Bogotá", 200),
    );
    let at = |t| SearchOpts {
        as_of: Some(t),
        ..SearchOpts::default()
    };
    assert_eq!(ids(&s.search(scope, "vive", &at(150)).unwrap()), [1]);
    assert_eq!(ids(&s.search(scope, "vive", &at(250)).unwrap()), [2]);
}

#[test]
fn t17_known_at_hides_later_learned_facts() {
    let (_d, _p, s, scope, actor) = setup();
    let mut m = row(1, scope, actor, "se mudó a Cali", 10);
    m.known_since = 300;
    put(&s, scope, actor, &m);
    let q = |known| SearchOpts {
        as_of: Some(150),
        known_at: Some(known),
        ..SearchOpts::default()
    };
    assert!(s.search(scope, "Cali", &q(100)).unwrap().is_empty());
    assert_eq!(ids(&s.search(scope, "Cali", &q(400)).unwrap()), [1]);
}

#[test]
fn hostile_queries_do_not_error() {
    let (_d, _p, s, scope, actor) = setup();
    put(&s, scope, actor, &row(1, scope, actor, "algo normal", 10));
    for q in [
        r#"a" OR NEAR( * -x"#,
        "\"",
        "AND",
        "col:val",
        "***",
        "",
        "( ) ^ {",
    ] {
        assert!(
            s.search(scope, q, &SearchOpts::default()).is_ok(),
            "query {q:?}"
        );
    }
}

#[test]
fn log_lookups_use_indexes() {
    let (_d, path, _s) = store();
    let raw = rusqlite::Connection::open(&path).unwrap();
    for sql in [history_sql(), by_actor_sql()] {
        let mut stmt = raw.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let plan: Vec<String> = stmt
            .query_map([1], |r| r.get(3))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(
            !plan.iter().any(|step| step.starts_with("SCAN")),
            "full scan in: {sql}\n{plan:?}"
        );
    }
}

#[test]
fn history_finds_every_operation_that_touched_a_memory() {
    let (_d, _p, s, scope, actor) = setup();
    let m1 = row(1, scope, actor, "vive en Medellín", 100);
    put(&s, scope, actor, &m1);
    let m2 = row(2, scope, actor, "vive en Bogotá", 200);
    let mut old = m1.clone();
    old.state = State::Replaced;
    let update = vec![
        Change {
            memory: m1.id,
            before: Some(m1.clone()),
            after: old,
        },
        add(&m2),
    ];
    let upd = s
        .append(op(scope, actor, OpKind::Update, 200, update))
        .unwrap();
    let noop = NewOperation {
        changes: vec![],
        ..op(scope, actor, OpKind::Noop, 300, vec![add(&m1)])
    };
    let noop = s.append(noop).unwrap();
    let ops =
        |m: &MemoryRow| -> Vec<OpId> { s.history(m.id).unwrap().iter().map(|o| o.id).collect() };
    assert_eq!(ops(&m1), [OpId(1), upd, noop]);
    assert_eq!(ops(&m2), [upd]);
}

#[test]
fn search_filters_by_kind() {
    let (_d, _p, s, scope, actor) = setup();
    put(
        &s,
        scope,
        actor,
        &row(1, scope, actor, "dato semántico", 10),
    );
    let mut procedural = row(2, scope, actor, "dato procedimental", 10);
    procedural.kind = MemoryType::Procedural;
    put(&s, scope, actor, &procedural);
    let only = SearchOpts {
        kind: Some(MemoryType::Procedural),
        ..SearchOpts::default()
    };
    assert_eq!(ids(&s.search(scope, "dato", &only).unwrap()), [2]);
    let mut both = ids(&s.search(scope, "dato", &SearchOpts::default()).unwrap());
    both.sort();
    assert_eq!(both, [1, 2]);
}

use crate::embed::unit_vector;
use crate::vector::VectorIndex;

fn unit(axis: usize, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0; dim];
    v[axis] = 1.0;
    unit_vector(v, dim).unwrap()
}

fn put_with_state(
    s: &SqliteStore,
    scope: ScopeId,
    actor: ActorId,
    mut m: MemoryRow,
    state: State,
) -> MemoryRow {
    m.state = state;
    put(s, scope, actor, &m);
    m
}

fn near(s: &SqliteStore, scope: ScopeId, model: &str, q: &[f32], opts: &SearchOpts) -> Vec<i64> {
    s.nearest(scope, model, q, opts, 10)
        .unwrap()
        .iter()
        .map(|(id, _)| id.0)
        .collect()
}

#[test]
fn nearest_respects_scope_state_and_time() {
    let (_d, _p, s, a, actor) = setup();
    let b = s.create_scope("other", "es").unwrap();
    let mut later = row(4, a, actor, "d", 10);
    later.known_since = 300;
    let mut replaced = row(3, a, actor, "c", 100);
    replaced.valid_to = Some(200);
    let rows = [
        put_with_state(&s, a, actor, row(1, a, actor, "a", 10), State::Active),
        put_with_state(&s, a, actor, row(2, a, actor, "b", 10), State::Archived),
        put_with_state(&s, a, actor, replaced, State::Replaced),
        put_with_state(&s, a, actor, later, State::Active),
        put_with_state(&s, b, actor, row(5, b, actor, "e", 10), State::Active),
    ];
    let e0 = unit(0, 8);
    let all: Vec<_> = rows.iter().map(|r| (r.id, e0.clone())).collect();
    s.put_vectors("m", 8, &all).unwrap();

    let default = SearchOpts::default();
    assert_eq!(near(&s, a, "m", &e0, &default), [1, 4]);
    let as_of = SearchOpts {
        as_of: Some(150),
        ..default
    };
    assert_eq!(near(&s, a, "m", &e0, &as_of), [1, 3, 4]);
    let known = SearchOpts {
        as_of: Some(150),
        known_at: Some(100),
        ..default
    };
    assert_eq!(near(&s, a, "m", &e0, &known), [1, 3]);
    assert_eq!(near(&s, b, "m", &e0, &default), [5]);
}

#[test]
fn nearest_ranks_by_cosine_and_ignores_other_models_and_dims() {
    let (_d, _p, s, scope, actor) = setup();
    for id in 1..=3 {
        put(&s, scope, actor, &row(id, scope, actor, "x", 10));
    }
    s.put_vectors(
        "a",
        8,
        &[(MemoryId(1), unit(1, 8)), (MemoryId(2), unit(0, 8))],
    )
    .unwrap();
    s.put_vectors("a", 4, &[(MemoryId(3), unit(0, 4))]).unwrap();
    let q = unit(0, 8);
    let hits = s
        .nearest(scope, "a", &q, &SearchOpts::default(), 10)
        .unwrap();
    assert_eq!(hits.iter().map(|h| h.0.0).collect::<Vec<_>>(), [2, 1]);
    assert!((hits[0].1 - 1.0).abs() < 1e-6 && hits[1].1.abs() < 1e-6);
    assert!(near(&s, scope, "b", &q, &SearchOpts::default()).is_empty());
    assert_eq!(
        s.nearest(scope, "a", &q, &SearchOpts::default(), 1)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn pending_embeddings_skips_archived_and_embedded() {
    let (_d, _p, s, scope, actor) = setup();
    put_with_state(
        &s,
        scope,
        actor,
        row(1, scope, actor, "uno", 10),
        State::Active,
    );
    put_with_state(
        &s,
        scope,
        actor,
        row(2, scope, actor, "dos", 10),
        State::Archived,
    );
    put_with_state(
        &s,
        scope,
        actor,
        row(3, scope, actor, "tres", 10),
        State::Quarantine,
    );
    s.put_vectors("a", 8, &[(MemoryId(1), unit(0, 8))]).unwrap();
    let ids = |model, limit| -> Vec<(i64, String)> {
        s.pending_embeddings(model, limit)
            .unwrap()
            .into_iter()
            .map(|(id, text)| (id.0, text))
            .collect()
    };
    assert_eq!(ids("a", 10), [(3, "tres".to_string())]);
    assert_eq!(ids("b", 10).iter().map(|p| p.0).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(ids("b", 1).len(), 1);
}

#[test]
fn put_vectors_replaces_and_is_atomic() {
    let (_d, _p, s, scope, actor) = setup();
    put(&s, scope, actor, &row(1, scope, actor, "x", 10));
    s.put_vectors("a", 8, &[(MemoryId(1), unit(0, 8))]).unwrap();
    s.put_vectors("a", 8, &[(MemoryId(1), unit(1, 8))]).unwrap();
    let hit = s
        .nearest(scope, "a", &unit(1, 8), &SearchOpts::default(), 1)
        .unwrap();
    assert!((hit[0].1 - 1.0).abs() < 1e-6);

    let bad = [(MemoryId(1), unit(2, 8)), (MemoryId(999), unit(2, 8))];
    assert!(s.put_vectors("a", 8, &bad).is_err());
    let still = s
        .nearest(scope, "a", &unit(1, 8), &SearchOpts::default(), 1)
        .unwrap();
    assert!(
        (still[0].1 - 1.0).abs() < 1e-6,
        "the failed batch must not change anything"
    );
}

#[test]
fn nearest_scans_vectors_in_order() {
    let (_d, path, _s) = store();
    let raw = rusqlite::Connection::open(&path).unwrap();
    let mut stmt = raw
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            nearest_sql(&SearchOpts::default())
        ))
        .unwrap();
    let plan: Vec<String> = stmt
        .query_map(
            rusqlite::named_params! {":model": "m", ":dim": 8, ":scope": 1, ":as_of": None::<i64>, ":known_at": None::<i64>, ":kind": None::<String>},
            |r| r.get(3),
        )
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert!(
        plan[0].starts_with("SCAN v"),
        "must be driven from vectors: {plan:?}"
    );
}

/// A unit vector at angle `deg` from axis 0 in the plane of axes 0 and 1: cosine to e0 is cos(deg).
fn at_angle(deg: f32, dim: usize) -> Vec<f32> {
    let r = deg.to_radians();
    let mut v = vec![0.0; dim];
    v[0] = r.cos();
    v[1] = r.sin();
    unit_vector(v, dim).unwrap()
}

#[test]
fn nearest_returns_everything_sorted_when_k_exceeds_rows() {
    let (_d, _p, s, scope, actor) = setup();
    let angles = [50.0, 10.0, 80.0, 30.0, 0.0];
    let mut vectors = Vec::new();
    for (i, a) in angles.iter().enumerate() {
        let id = i as i64 + 1;
        put(&s, scope, actor, &row(id, scope, actor, "x", 10));
        vectors.push((MemoryId(id), at_angle(*a, 8)));
    }
    s.put_vectors("m", 8, &vectors).unwrap();
    let got = s
        .nearest(scope, "m", &unit(0, 8), &SearchOpts::default(), 1_000_000)
        .unwrap();
    assert_eq!(
        got.iter().map(|g| g.0.0).collect::<Vec<_>>(),
        [5, 2, 4, 1, 3]
    );
}

#[test]
fn nearest_ties_break_by_lower_id() {
    let (_d, _p, s, scope, actor) = setup();
    for id in [3, 1, 2] {
        put(&s, scope, actor, &row(id, scope, actor, "x", 10));
        s.put_vectors("m", 8, &[(MemoryId(id), unit(0, 8))])
            .unwrap();
    }
    assert_eq!(
        near(&s, scope, "m", &unit(0, 8), &SearchOpts::default()),
        [1, 2, 3]
    );
}

#[test]
fn nearest_with_k_zero_is_empty() {
    let (_d, _p, s, scope, actor) = setup();
    put(&s, scope, actor, &row(1, scope, actor, "x", 10));
    s.put_vectors("m", 8, &[(MemoryId(1), unit(0, 8))]).unwrap();
    assert!(
        s.nearest(scope, "m", &unit(0, 8), &SearchOpts::default(), 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unwritable_directory_error_names_a_real_directory_for_relative_paths() {
    let failure = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY),
        None,
    );
    match read_only_failure(std::path::Path::new("memory.db"), failure) {
        Error::UnwritableDirectory(dir) => assert_eq!(dir, std::path::PathBuf::from(".")),
        other => panic!("expected UnwritableDirectory, got {other:?}"),
    }
}

// ---- decision log ----

use crate::model::{Choice, DeciderMode, DecisionStatus};
use crate::store::NewDecision;

fn decision(scope: ScopeId, provider: &str, status: DecisionStatus, at: i64) -> NewDecision {
    NewDecision {
        scope,
        provider: provider.into(),
        mode: DeciderMode::Active,
        status,
        choice: Some(Choice::Review),
        confidence: Some(0.9),
        reason: Some("because".into()),
        acted: true,
        at,
    }
}

#[test]
fn decisions_are_written_with_their_operation() {
    let (_d, _p, s, scope, actor) = setup();
    let m = row(1, scope, actor, "x", 10);
    let id = s
        .append_with_decisions(
            op(scope, actor, OpKind::Add, 10, vec![add(&m)]),
            vec![
                decision(scope, "a", DecisionStatus::Decided, 10),
                NewDecision {
                    mode: DeciderMode::Shadow,
                    acted: false,
                    choice: None,
                    confidence: None,
                    reason: None,
                    ..decision(scope, "b", DecisionStatus::None, 10)
                },
            ],
        )
        .unwrap();
    let got = s.decisions(scope, 0, i64::MAX).unwrap();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|d| d.op == Some(id)));
    assert_eq!(
        (
            got[0].provider.as_str(),
            got[0].status,
            got[0].choice,
            got[0].acted
        ),
        ("a", DecisionStatus::Decided, Some(Choice::Review), true)
    );
    assert_eq!(
        (
            got[1].provider.as_str(),
            got[1].mode,
            got[1].status,
            got[1].choice,
            got[1].confidence
        ),
        ("b", DeciderMode::Shadow, DecisionStatus::None, None, None)
    );
    assert_eq!(got[0].confidence, Some(0.9));
    assert_eq!(got[0].reason.as_deref(), Some("because"));
}

#[test]
fn failed_append_leaves_no_decisions() {
    let (_d, _p, s, scope, actor) = setup();
    let ghost = ScopeId(999);
    let m = row(1, ghost, actor, "x", 10);
    let r = s.append_with_decisions(
        op(ghost, actor, OpKind::Add, 10, vec![add(&m)]),
        vec![decision(scope, "a", DecisionStatus::Decided, 10)],
    );
    assert!(r.is_err());
    assert!(s.decisions(scope, 0, i64::MAX).unwrap().is_empty());
}

#[test]
fn decisions_range_is_half_open() {
    let (_d, _p, s, scope, actor) = setup();
    for (i, at) in [100, 200].into_iter().enumerate() {
        let m = row(i as i64 + 1, scope, actor, "x", at);
        s.append_with_decisions(
            op(scope, actor, OpKind::Add, at, vec![add(&m)]),
            vec![decision(scope, "a", DecisionStatus::Decided, at)],
        )
        .unwrap();
    }
    assert_eq!(s.decisions(scope, 100, 200).unwrap().len(), 1);
    assert_eq!(s.decisions(scope, 100, 201).unwrap().len(), 2);
    assert!(s.decisions(scope, 101, 200).unwrap().is_empty());
}

#[test]
fn rebuild_projection_keeps_decisions() {
    let (_d, _p, s, scope, actor) = setup();
    let m = row(1, scope, actor, "x", 10);
    s.append_with_decisions(
        op(scope, actor, OpKind::Add, 10, vec![add(&m)]),
        vec![decision(scope, "a", DecisionStatus::Decided, 10)],
    )
    .unwrap();
    s.rebuild_projection().unwrap();
    assert_eq!(s.decisions(scope, 0, i64::MAX).unwrap().len(), 1);
}

// ---- job queue ----

use crate::model::{JobKind, JobState};
use crate::store::NewJob;

fn job(kind: JobKind, key: Option<&str>, run_after: i64, at: i64) -> NewJob {
    NewJob {
        kind,
        payload: "{}".into(),
        dedupe_key: key.map(Into::into),
        run_after,
        at,
    }
}

fn job_ids(js: &[crate::model::JobRecord]) -> Vec<i64> {
    js.iter().map(|j| j.id).collect()
}

#[test]
fn dedupe_ignores_a_second_pending_job_but_not_after_it_runs() {
    let (_d, _p, s) = store();
    assert!(s.enqueue(job(JobKind::Embed, Some("embed"), 0, 1)).unwrap());
    assert!(!s.enqueue(job(JobKind::Embed, Some("embed"), 0, 2)).unwrap());
    assert!(s.claim_job(10, None, true, 300).unwrap().is_some());
    assert!(
        s.enqueue(job(JobKind::Embed, Some("embed"), 0, 3)).unwrap(),
        "a running job does not block a new one"
    );
    assert!(s.enqueue(job(JobKind::Touch, None, 0, 4)).unwrap());
    assert!(
        s.enqueue(job(JobKind::Touch, None, 0, 5)).unwrap(),
        "no key, no dedupe"
    );
}

#[test]
fn claim_orders_by_priority_then_age_and_respects_run_after() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Touch, None, 0, 1)).unwrap();
    s.enqueue(job(JobKind::Embed, Some("embed"), 0, 2)).unwrap();
    s.enqueue(job(JobKind::Decay, Some("d"), 0, 3)).unwrap();
    s.enqueue(job(JobKind::Embed, None, 999, 4)).unwrap();
    let kinds: Vec<_> = (0..3)
        .map(|_| s.claim_job(10, None, true, 300).unwrap().unwrap().kind)
        .collect();
    assert_eq!(kinds, [JobKind::Embed, JobKind::Touch, JobKind::Decay]);
    assert!(
        s.claim_job(10, None, true, 300).unwrap().is_none(),
        "run_after is in the future"
    );
    assert!(s.claim_job(999, None, true, 300).unwrap().is_some());
}

#[test]
fn claim_counts_the_attempt_and_sets_the_lease() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Embed, None, 0, 1)).unwrap();
    let j = s.claim_job(100, None, false, 300).unwrap().unwrap();
    assert_eq!(
        (j.state, j.attempts, j.lease_until),
        (JobState::Running, 1, Some(400))
    );
    let stored = s.jobs(Some(JobState::Running), 10).unwrap();
    assert_eq!((stored[0].attempts, stored[0].lease_until), (1, Some(400)));
}

#[test]
fn heavy_jobs_wait_for_allow_heavy_and_only_filters_by_kind() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Backup, Some("b"), 0, 1)).unwrap();
    s.enqueue(job(JobKind::Vacuum, Some("v"), 0, 1)).unwrap();
    assert!(s.claim_job(10, None, false, 300).unwrap().is_none());
    assert_eq!(s.count_ready_heavy(10).unwrap(), 2);
    assert!(s.claim_job(10, None, true, 300).unwrap().is_some());

    s.enqueue(job(JobKind::Embed, Some("embed"), 0, 1)).unwrap();
    s.enqueue(job(JobKind::Touch, None, 0, 1)).unwrap();
    assert_eq!(
        s.claim_job(10, Some(JobKind::Touch), true, 300)
            .unwrap()
            .unwrap()
            .kind,
        JobKind::Touch
    );
}

#[test]
fn fail_with_retry_goes_back_to_pending_at_the_given_time() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Embed, None, 0, 1)).unwrap();
    let j = s.claim_job(100, None, true, 300).unwrap().unwrap();
    s.fail_job(j.id, 100, "boom", Some(160)).unwrap();
    let back = &s.jobs(Some(JobState::Pending), 10).unwrap()[0];
    assert_eq!(
        (back.run_after, back.error.as_deref(), back.lease_until),
        (160, Some("boom"), None)
    );
    assert!(s.claim_job(159, None, true, 300).unwrap().is_none());
    assert_eq!(
        s.claim_job(160, None, true, 300).unwrap().unwrap().attempts,
        2
    );
}

#[test]
fn a_retry_that_collides_with_a_pending_twin_is_dropped_as_done() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Embed, Some("embed"), 0, 1)).unwrap();
    let first = s.claim_job(100, None, true, 300).unwrap().unwrap();
    assert!(s.enqueue(job(JobKind::Embed, Some("embed"), 0, 2)).unwrap());
    s.fail_job(first.id, 100, "boom", Some(160)).unwrap();
    let done = s.jobs(Some(JobState::Done), 10).unwrap();
    assert_eq!(done.len(), 1);
    assert!(done[0].error.as_deref().unwrap().contains("superseded"));
    assert_eq!(s.jobs(Some(JobState::Pending), 10).unwrap().len(), 1);
}

#[test]
fn fail_without_retry_goes_to_failed_with_the_error() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Embed, None, 0, 1)).unwrap();
    let j = s.claim_job(100, None, true, 300).unwrap().unwrap();
    s.fail_job(j.id, 105, "no way", None).unwrap();
    let f = &s.jobs(Some(JobState::Failed), 10).unwrap()[0];
    assert_eq!(
        (f.error.as_deref(), f.finished_at, f.lease_until),
        (Some("no way"), Some(105), None)
    );
}

#[test]
fn expired_leases_are_recovered_or_failed_at_max_attempts() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Embed, Some("a"), 0, 1)).unwrap();
    s.claim_job(100, None, true, 300).unwrap().unwrap(); // Running until 400
    assert_eq!(
        s.recover_leases(399).unwrap(),
        0,
        "the lease has not expired yet"
    );
    assert_eq!(s.recover_leases(500).unwrap(), 1);
    assert_eq!(s.jobs(Some(JobState::Pending), 10).unwrap().len(), 1);

    // a job that already used all its attempts and dies once more ends in the error queue
    let (_d2, _p2, t) = store();
    t.enqueue(job(JobKind::Embed, None, 0, 1)).unwrap();
    for n in 1..=4 {
        let j = t.claim_job(100 * n, None, true, 300).unwrap().unwrap();
        t.fail_job(j.id, 100 * n, "x", Some(100 * n)).unwrap();
    }
    let last = t.claim_job(1000, None, true, 300).unwrap().unwrap();
    assert_eq!(last.attempts, 5);
    assert_eq!(t.recover_leases(2000).unwrap(), 1);
    let failed = &t.jobs(Some(JobState::Failed), 10).unwrap()[0];
    assert_eq!(failed.error.as_deref(), Some("lease expired"));
}

#[test]
fn append_all_writes_jobs_with_the_operation_and_a_failed_append_leaves_none() {
    let (_d, _p, s, scope, actor) = setup();
    let m = row(1, scope, actor, "x", 10);
    s.append_all(
        op(scope, actor, OpKind::Add, 10, vec![add(&m)]),
        vec![],
        vec![job(JobKind::Embed, Some("embed"), 0, 10)],
    )
    .unwrap();
    assert_eq!(s.jobs(None, 10).unwrap().len(), 1);

    let ghost = ScopeId(999);
    let bad = row(2, ghost, actor, "y", 11);
    let r = s.append_all(
        op(ghost, actor, OpKind::Add, 11, vec![add(&bad)]),
        vec![],
        vec![job(JobKind::Touch, None, 0, 11)],
    );
    assert!(r.is_err());
    assert_eq!(
        s.jobs(None, 10).unwrap().len(),
        1,
        "the failed append left no job behind"
    );
}

#[test]
fn queue_stats_retry_job_prune_and_rebuild() {
    let (_d, _p, s, scope, actor) = setup();
    s.enqueue(job(JobKind::Touch, None, 0, 100)).unwrap();
    s.enqueue(job(JobKind::Touch, None, 0, 150)).unwrap();
    let st = s.queue_stats(400).unwrap();
    assert_eq!(
        (
            st.pending,
            st.running,
            st.failed,
            st.oldest_pending_age_secs
        ),
        (2, 0, 0, Some(300))
    );

    let j = s.claim_job(10, None, true, 300).unwrap().unwrap();
    s.fail_job(j.id, 20, "e", None).unwrap();
    assert!(
        matches!(s.retry_job(j.id + 1, 30), Err(Error::InvalidState)),
        "only a Failed job can be retried"
    );
    s.retry_job(j.id, 30).unwrap();
    let again = s.jobs(Some(JobState::Pending), 10).unwrap();
    assert!(
        again
            .iter()
            .any(|x| x.id == j.id && x.attempts == 0 && x.error.is_none())
    );

    let a = s.claim_job(40, None, true, 300).unwrap().unwrap();
    s.finish_job(a.id, 100).unwrap();
    let b = s.claim_job(40, None, true, 300).unwrap().unwrap();
    s.finish_job(b.id, 900).unwrap();
    assert_eq!(s.prune_jobs(500, 500).unwrap(), 1);
    assert_eq!(s.jobs(Some(JobState::Done), 10).unwrap().len(), 1);

    put(&s, scope, actor, &row(1, scope, actor, "x", 10));
    let before = job_ids(&s.jobs(None, 10).unwrap());
    s.rebuild_projection().unwrap();
    assert_eq!(
        job_ids(&s.jobs(None, 10).unwrap()),
        before,
        "rebuild must not touch the queue"
    );
}

#[test]
fn finish_and_fail_only_touch_running_jobs() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Embed, None, 0, 1)).unwrap();
    let id = s.jobs(None, 1).unwrap()[0].id;
    s.finish_job(id, 10).unwrap();
    assert_eq!(
        s.jobs(None, 1).unwrap()[0].state,
        JobState::Pending,
        "a pending job cannot be finished"
    );
    s.claim_job(20, None, true, 300).unwrap().unwrap();
    s.finish_job(id, 30).unwrap();
    assert_eq!(s.jobs(None, 1).unwrap()[0].state, JobState::Done);
    s.fail_job(
        id,
        40,
        "late failure of a worker that lost its lease",
        Some(100),
    )
    .unwrap();
    s.fail_job(id, 41, "again", None).unwrap();
    assert_eq!(
        s.jobs(None, 1).unwrap()[0].state,
        JobState::Done,
        "a finished job is not revived"
    );
}

#[test]
fn retry_job_with_a_pending_twin_closes_it_as_done() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Embed, Some("embed"), 0, 1)).unwrap();
    let j = s.claim_job(10, None, true, 300).unwrap().unwrap();
    s.fail_job(j.id, 20, "no embedder", None).unwrap();
    assert!(
        s.enqueue(job(JobKind::Embed, Some("embed"), 0, 30))
            .unwrap()
    );
    s.retry_job(j.id, 40).unwrap();
    let done = s.jobs(Some(JobState::Done), 10).unwrap();
    assert_eq!(done.len(), 1);
    assert!(done[0].error.as_deref().unwrap().contains("superseded"));
    assert_eq!(s.jobs(Some(JobState::Pending), 10).unwrap().len(), 1);
}

#[test]
fn claim_jobs_takes_many_of_one_kind_and_finish_jobs_closes_them() {
    let (_d, _p, s) = store();
    for at in 1..=5 {
        s.enqueue(job(JobKind::Touch, None, 0, at)).unwrap();
    }
    s.enqueue(job(JobKind::Embed, Some("embed"), 0, 1)).unwrap();
    let batch = s.claim_jobs(10, JobKind::Touch, 300, 3).unwrap();
    assert_eq!(batch.len(), 3);
    assert!(
        batch
            .iter()
            .all(|j| j.kind == JobKind::Touch && j.state == JobState::Running && j.attempts == 1)
    );
    s.finish_jobs(&batch.iter().map(|j| j.id).collect::<Vec<_>>(), 20)
        .unwrap();
    assert_eq!(s.jobs(Some(JobState::Done), 10).unwrap().len(), 3);
    assert_eq!(
        s.jobs(Some(JobState::Pending), 10).unwrap().len(),
        3,
        "two Touch and the Embed are still pending"
    );
}

#[test]
fn record_usages_applies_every_pair_with_the_latest_time() {
    let (_d, path, s, scope, actor) = setup();
    put(&s, scope, actor, &row(1, scope, actor, "x", 10));
    put(&s, scope, actor, &row(2, scope, actor, "y", 10));
    s.record_usages(&[(MemoryId(1), 100), (MemoryId(1), 50), (MemoryId(2), 70)])
        .unwrap();
    let counts = s.usage_counts(&[MemoryId(1), MemoryId(2)]).unwrap();
    assert_eq!((counts[&MemoryId(1)], counts[&MemoryId(2)]), (2, 1));
    let raw = rusqlite::Connection::open(&path).unwrap();
    let last: i64 = raw
        .query_row("SELECT last_used FROM usage WHERE memory = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(last, 100);
}

#[test]
fn failed_jobs_are_pruned_on_their_own_schedule() {
    let (_d, _p, s) = store();
    s.enqueue(job(JobKind::Embed, None, 0, 1)).unwrap();
    s.enqueue(job(JobKind::Touch, None, 0, 1)).unwrap();
    let a = s.claim_job(10, None, true, 300).unwrap().unwrap();
    s.fail_job(a.id, 100, "bad", None).unwrap();
    let b = s.claim_job(10, None, true, 300).unwrap().unwrap();
    s.finish_job(b.id, 100).unwrap();
    assert_eq!(
        s.prune_jobs(500, 50).unwrap(),
        1,
        "only the Done job is old enough for its cutoff"
    );
    assert_eq!(s.jobs(Some(JobState::Failed), 10).unwrap().len(), 1);
    assert_eq!(s.prune_jobs(500, 500).unwrap(), 1);
}

// ---- the copy of the vectors in memory ----

/// Repeatable pseudo-random vector in [-1, 1).
fn noise(seed: u64, dim: usize) -> Vec<f32> {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..dim)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % 2000) as f32 / 1000.0 - 1.0
        })
        .collect()
}

fn cached_epoch(s: &SqliteStore) -> (usize, i64) {
    s.vectors.with(|r| (r.caches.len(), r.version)).unwrap()
}

#[test]
fn the_copy_in_memory_ranks_exactly_like_the_scan() {
    let (_d, _p, s, a, actor) = setup();
    let b = s.create_scope("other", "es").unwrap();
    let states = [
        State::Active,
        State::Archived,
        State::Quarantine,
        State::Replaced,
    ];
    let mut all = Vec::new();
    for id in 1..=200i64 {
        let scope = if id % 5 == 0 { b } else { a };
        let mut m = row(id, scope, actor, "x", 10 + id);
        m.known_since = 10 + id;
        if id % 4 == 3 {
            m.valid_to = Some(150);
        }
        put_with_state(&s, scope, actor, m, states[(id % 4) as usize]);
        all.push((MemoryId(id), noise(id as u64, 16)));
    }
    s.put_vectors("m", 16, &all).unwrap();
    let q = noise(999, 16);
    let default = SearchOpts::default();
    let variants = [
        SearchOpts {
            include_quarantine: true,
            ..default
        },
        SearchOpts {
            as_of: Some(100),
            ..default
        },
        SearchOpts {
            as_of: Some(100),
            known_at: Some(60),
            ..default
        },
        default,
    ];
    for opts in &variants {
        for k in [1, 5, 50, 1000] {
            let cached = s.nearest(a, "m", &q, opts, k).unwrap();
            let scan = s.nearest_scan(a, "m", &q, opts, k).unwrap();
            assert_eq!(cached, scan, "k={k} opts={opts:?}");
        }
    }
}

#[test]
fn a_new_or_replaced_vector_is_seen_by_the_next_search() {
    let (_d, _p, s, scope, actor) = setup();
    for id in 1..=3 {
        put(&s, scope, actor, &row(id, scope, actor, "x", 10));
    }
    s.put_vectors("m", 8, &[(MemoryId(1), unit(0, 8))]).unwrap();
    let q = unit(0, 8);
    let opts = SearchOpts::default();
    assert_eq!(near(&s, scope, "m", &q, &opts), [1]);
    s.put_vectors("m", 8, &[(MemoryId(2), unit(0, 8))]).unwrap();
    assert_eq!(near(&s, scope, "m", &q, &opts), [1, 2], "a new vector");
    s.put_vectors("m", 8, &[(MemoryId(1), unit(1, 8))]).unwrap();
    assert_eq!(near(&s, scope, "m", &q, &opts), [2, 1], "a replaced vector");
}

#[test]
fn a_store_that_only_reads_sees_what_another_store_writes() {
    let (_d, path, s, scope, actor) = setup();
    for id in 1..=2 {
        put(&s, scope, actor, &row(id, scope, actor, "x", 10));
    }
    s.put_vectors("m", 8, &[(MemoryId(1), unit(0, 8))]).unwrap();
    let ro = SqliteStore::open(&path, OpenMode::ReadOnly).unwrap();
    let q = unit(0, 8);
    let opts = SearchOpts::default();
    assert_eq!(near(&ro, scope, "m", &q, &opts), [1]);
    s.put_vectors("m", 8, &[(MemoryId(2), unit(0, 8))]).unwrap();
    assert_eq!(near(&ro, scope, "m", &q, &opts), [1, 2]);
}

#[test]
fn a_write_that_leaves_the_vectors_alone_keeps_the_copy() {
    let (_d, _p, s, scope, actor) = setup();
    put(&s, scope, actor, &row(1, scope, actor, "x", 10));
    s.put_vectors("m", 8, &[(MemoryId(1), unit(0, 8))]).unwrap();
    let q = unit(0, 8);
    let _ = near(&s, scope, "m", &q, &SearchOpts::default());
    let before = cached_epoch(&s);
    assert_eq!(before.0, 1);
    // A recall writes usage on every call; that must not throw the copy away.
    s.create_scope("another", "es").unwrap();
    let _ = near(&s, scope, "m", &q, &SearchOpts::default());
    assert_eq!(cached_epoch(&s), before);
}

#[test]
fn too_many_vectors_fall_back_to_the_scan_and_keep_nothing_in_memory() {
    let (_d, _p, s, scope, actor) = setup();
    for id in 1..=3 {
        put(&s, scope, actor, &row(id, scope, actor, "x", 10));
    }
    let all: Vec<_> = (1..=3)
        .map(|id| (MemoryId(id), noise(id as u64, 8)))
        .collect();
    s.put_vectors("m", 8, &all).unwrap();
    s.vectors.with(|r| r.max_bytes = 8).unwrap();
    let q = noise(7, 8);
    let opts = SearchOpts::default();
    assert_eq!(
        s.nearest(scope, "m", &q, &opts, 10).unwrap(),
        s.nearest_scan(scope, "m", &q, &opts, 10).unwrap()
    );
    assert_eq!(cached_epoch(&s).0, 0);
}
