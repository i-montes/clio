#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use std::sync::Arc;

use clio_memory::*;
use common::*;

fn embedded() -> Fixture {
    fixture_with(Arc::new(HashEmbedder::new("m", 256)))
}

fn episodic(f: &Fixture, content: &str) -> MemoryId {
    let mut p = prop(f.scope, f.person, content);
    p.kind = MemoryType::Episodic;
    f.engine.propose(p).unwrap().memory
}

// ---- scopes, actors, stats ----

#[test]
fn scopes_and_actors_list_what_exists() {
    let f = fixture();
    let other = f.engine.create_scope("trabajo", "en").unwrap();
    f.engine.set_retention(other, Some(30), f.person).unwrap();

    let scopes = f.engine.scopes().unwrap();
    assert_eq!(scopes.len(), 2);
    assert_eq!(
        scopes[0],
        ScopeInfo {
            id: f.scope,
            name: "main".into(),
            language: "es".into(),
            retention_days: None
        }
    );
    assert_eq!(scopes[1].name, "trabajo");
    assert_eq!(scopes[1].language, "en");
    assert_eq!(scopes[1].retention_days, Some(30));

    let kinds = |f: &Fixture| -> Vec<(String, ActorKind)> {
        f.engine
            .actors()
            .unwrap()
            .into_iter()
            .map(|a| (a.name, a.kind))
            .collect()
    };
    assert_eq!(
        kinds(&f),
        [
            ("ana".to_string(), ActorKind::Person),
            ("bot".to_string(), ActorKind::Agent)
        ]
    );
    // the system actor shows up once background forgetting has run
    f.engine.set_retention(f.scope, Some(1), f.person).unwrap();
    f.engine.enqueue_decay(f.scope).unwrap();
    f.engine.run_jobs(&RunOpts::default()).unwrap();
    assert!(kinds(&f).contains(&("clio-memory:system".to_string(), ActorKind::System)));
}

#[test]
fn stats_counts_memories_by_state_and_scope() {
    let f = fixture();
    let other = f.engine.create_scope("trabajo", "es").unwrap();
    let a = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap()
        .memory;
    f.engine
        .propose(prop(f.scope, f.person, "tiene un perro"))
        .unwrap();
    let mut ext = prop(f.scope, f.agent, "dato de fuera");
    ext.origin = Origin::External;
    f.engine.propose(ext).unwrap(); // quarantined
    f.engine.archive(a, f.person).unwrap();
    let mut upd = prop(other, f.person, "usa Rust");
    upd.topic_key = Some("lenguaje".into());
    f.engine.propose(upd.clone()).unwrap();
    upd.content = "usa Go".into();
    f.engine.propose(upd).unwrap(); // the first becomes Replaced

    let s = f.engine.stats().unwrap();
    let main = &s.scopes[0];
    assert_eq!(main.name, "main");
    assert_eq!(
        (main.active, main.quarantine, main.archived, main.replaced),
        (1, 1, 1, 0)
    );
    let work = &s.scopes[1];
    assert_eq!(
        (work.active, work.quarantine, work.archived, work.replaced),
        (1, 0, 0, 1)
    );
    assert!(s.operations >= 5);
    assert_eq!(s.decisions, 0);
}

#[test]
fn stats_sees_queue_vectors_and_pending_embeddings() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    f.engine
        .propose(prop(f.scope, f.person, "tiene un perro"))
        .unwrap();
    let s = f.engine.stats().unwrap();
    assert_eq!((s.vectors, s.pending_embeddings), (0, 2));
    assert_eq!(s.queue.pending, 1, "one deduplicated Embed job");

    f.engine.run_jobs(&RunOpts::default()).unwrap();
    let s = f.engine.stats().unwrap();
    assert_eq!((s.vectors, s.pending_embeddings), (2, 0));
    assert_eq!(s.queue.pending, 0);
}

#[test]
fn stats_reports_file_and_wal_sizes() {
    let f = fixture();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let s = f.engine.stats().unwrap();
    assert!(s.file_bytes > 0);
    let wal = f.path.with_file_name("memory.db-wal");
    let real = std::fs::metadata(&wal).map_or(0, |m| m.len());
    assert_eq!(s.wal_bytes, real);
}

#[test]
fn stats_work_on_a_read_only_engine() {
    let f = embedded();
    episodic(&f, "dato suelto");
    drop(f.engine);
    let ro = Engine::open(&f.path, OpenMode::ReadOnly, f.clock.clone()).unwrap();
    assert_eq!(ro.scopes().unwrap().len(), 1);
    assert_eq!(ro.actors().unwrap().len(), 2);
    assert_eq!(ro.stats().unwrap().scopes[0].active, 1);
}

// ---- doctor ----

fn populated() -> Fixture {
    let f = embedded();
    let a = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap()
        .memory;
    f.engine
        .propose(prop(f.scope, f.person, "tiene un perro"))
        .unwrap();
    let block = f
        .engine
        .create_block(f.scope, "perfil", 100, f.person)
        .unwrap();
    f.engine.block_set(block, &[a], f.person).unwrap();
    f.engine.feedback(a, Signal::Useful, f.agent).unwrap();
    f.engine.run_jobs(&RunOpts::default()).unwrap();
    f.engine
        .recall(f.scope, "Medellín", &RecallOpts::default())
        .unwrap();
    f.engine.run_jobs(&RunOpts::default()).unwrap(); // applies the Touch
    f
}

fn failing(report: &DoctorReport) -> Vec<&str> {
    report
        .checks
        .iter()
        .filter(|c| !c.ok)
        .map(|c| c.name.as_str())
        .collect()
}

fn raw(f: &Fixture, sql: &str) {
    rusqlite::Connection::open(&f.path)
        .unwrap()
        .execute_batch(sql)
        .unwrap();
}

#[test]
fn a_healthy_file_passes_every_check() {
    let f = populated();
    let r = f.engine.doctor().unwrap();
    assert!(r.all_ok(), "failing: {:?}", failing(&r));
    for name in [
        "permissions",
        "schema_version",
        "projection_matches_log",
        "no_orphan_vectors",
        "no_orphan_usage_or_feedback",
        "block_entries_exist",
        "failed_jobs",
        "pending_embeddings",
    ] {
        assert!(r.get(name).is_some(), "missing check {name}");
    }
}

#[test]
fn doctor_detects_a_corrupt_projection() {
    let f = populated();
    raw(&f, "UPDATE memories SET content = 'x' WHERE id = 1");
    let r = f.engine.doctor().unwrap();
    assert_eq!(failing(&r), ["projection_matches_log"]);
    assert!(
        r.get("projection_matches_log")
            .unwrap()
            .detail
            .contains("ids: 1")
    );
}

#[test]
fn doctor_detects_a_row_missing_from_the_projection() {
    let f = populated();
    raw(&f, "DELETE FROM memories WHERE id = 2");
    let r = f.engine.doctor().unwrap();
    // its vector is now an orphan too
    assert_eq!(failing(&r), ["projection_matches_log", "no_orphan_vectors"]);
    assert!(
        r.get("projection_matches_log")
            .unwrap()
            .detail
            .contains("ids: 2")
    );
}

#[test]
fn doctor_detects_orphans() {
    let f = populated();
    raw(
        &f,
        "INSERT INTO vectors(memory, model, dim, vec) VALUES (999, 'm', 1, x'00000000');
         INSERT INTO usage(memory, uses, last_used) VALUES (998, 1, 1);
         INSERT INTO feedback(memory, actor, signal, at) VALUES (997, 1, 'Useful', 1);
         INSERT INTO block_memories(block, position, memory) VALUES (1, 50, 996);",
    );
    let r = f.engine.doctor().unwrap();
    assert_eq!(
        failing(&r),
        [
            "no_orphan_vectors",
            "no_orphan_usage_or_feedback",
            "block_entries_exist"
        ]
    );
}

#[test]
fn doctor_flags_failed_jobs() {
    let f = populated();
    raw(
        &f,
        "INSERT INTO jobs(kind, payload, priority, state, run_after, created_at, error)
         VALUES ('Vacuum', '{}', 1, 'Failed', 0, 1, 'boom')",
    );
    let r = f.engine.doctor().unwrap();
    assert_eq!(failing(&r), ["failed_jobs"]);
}

#[test]
fn doctor_flags_loose_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = populated();
    std::fs::set_permissions(&f.path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let r = f.engine.doctor().unwrap();
    assert_eq!(failing(&r), ["permissions"]);
}

#[test]
fn pending_embeddings_is_informational() {
    let f = embedded();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let r = f.engine.doctor().unwrap();
    assert!(r.all_ok());
    assert!(r.get("pending_embeddings").unwrap().detail.contains('1'));
}

#[test]
fn doctor_never_writes() {
    let f = populated();
    drop(f.engine);
    let ro = Engine::open(&f.path, OpenMode::ReadOnly, f.clock.clone()).unwrap();
    let dump = dump_all(&f.path);
    let before = std::fs::metadata(&f.path).unwrap();
    ro.doctor().unwrap();
    let after = std::fs::metadata(&f.path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_eq!(dump, dump_all(&f.path));
}

// ---- export ----

fn export_lines(engine: &Engine) -> (Vec<serde_json::Value>, ExportSummary) {
    let mut out = Vec::new();
    let summary = engine.export(&mut out).unwrap();
    let lines = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (lines, summary)
}

fn kind(line: &serde_json::Value) -> &str {
    line["type"].as_str().unwrap()
}

#[test]
fn export_writes_a_versioned_header_then_typed_lines_in_order() {
    let f = populated();
    let (lines, summary) = export_lines(&f.engine);
    let header = &lines[0];
    assert_eq!(kind(header), "header");
    assert_eq!(header["format"], "clio-memory-export");
    assert_eq!(header["format_version"], EXPORT_FORMAT_VERSION);
    assert_eq!(EXPORT_FORMAT_VERSION, "1.0.0");
    assert_eq!(header["schema_version"], 1);
    assert_eq!(header["exported_at"], 1000);

    let order = [
        "header",
        "scope",
        "actor",
        "operation",
        "decision",
        "block",
        "block_memory",
        "usage",
        "feedback",
    ];
    let rank = |l: &serde_json::Value| order.iter().position(|t| *t == kind(l)).unwrap();
    assert!(lines.windows(2).all(|w| rank(&w[0]) <= rank(&w[1])));

    let count = |t: &str| lines.iter().filter(|l| kind(l) == t).count();
    assert_eq!(count("scope"), summary.scopes);
    assert_eq!(count("actor"), summary.actors);
    assert_eq!(count("operation"), summary.operations);
    assert_eq!(count("block"), summary.blocks);
    assert_eq!(count("usage"), summary.usage);
    assert_eq!(count("feedback"), summary.feedback);
    assert_eq!(
        (
            summary.scopes,
            summary.actors,
            summary.blocks,
            summary.feedback
        ),
        (1, 2, 1, 1)
    );
}

#[test]
fn export_contains_everything_needed_to_rebuild() {
    let f = populated();
    let (lines, _) = export_lines(&f.engine);
    let exported: Vec<Operation> = lines
        .iter()
        .filter(|l| kind(l) == "operation")
        .map(|l| serde_json::from_value(l.clone()).unwrap())
        .collect();
    let mut expected = Vec::new();
    for id in [1, 2] {
        expected.extend(f.engine.history(MemoryId(id)).unwrap());
    }
    expected.sort_by_key(|o| o.id);
    assert_eq!(exported, expected);
    let scope = lines.iter().find(|l| kind(l) == "scope").unwrap();
    assert_eq!(
        (scope["name"].as_str(), scope["language"].as_str()),
        (Some("main"), Some("es"))
    );
}

#[test]
fn export_omits_vectors_jobs_and_quota_counters() {
    let f = populated();
    f.engine.enqueue_vacuum().unwrap();
    let mut out = Vec::new();
    f.engine.export(&mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    for banned in [
        "\"vec\"",
        "\"dim\"",
        "\"payload\"",
        "\"window_start\"",
        "Vacuum",
    ] {
        assert!(!text.contains(banned), "export leaked {banned}");
    }
}

// S-18: nothing that looks like a secret ever reaches an export
#[test]
fn export_has_no_secret_and_is_snapshot_consistent() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = populated();
    let secret = "clave AKIAIOSFODNN7EXAMPLE";
    assert!(f.engine.propose(prop(f.scope, f.person, secret)).is_err());
    let done = AtomicBool::new(false);
    let mut exports = 0;
    std::thread::scope(|s| {
        let writer = s.spawn(|| {
            for i in 0..60 {
                let m = f
                    .engine
                    .propose(prop(f.scope, f.person, &format!("dato concurrente {i}")))
                    .unwrap()
                    .memory;
                // a line in a later table that points at the memory just written
                f.engine.feedback(m, Signal::Useful, f.agent).unwrap();
            }
            done.store(true, Ordering::SeqCst);
        });
        while !done.load(Ordering::SeqCst) {
            let mut out = Vec::new();
            f.engine.export(&mut out).unwrap();
            exports += 1;
            let text = String::from_utf8(out).unwrap();
            assert!(!text.contains("AKIA"));
            let lines: Vec<serde_json::Value> = text
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            let known: std::collections::HashSet<i64> = lines
                .iter()
                .filter(|l| kind(l) == "operation")
                .flat_map(|l| l["changes"].as_array().unwrap().iter())
                .map(|c| c["memory"].as_i64().unwrap())
                .collect();
            for l in lines
                .iter()
                .filter(|l| matches!(kind(l), "block_memory" | "usage" | "feedback"))
            {
                let id = l["memory"].as_i64().unwrap();
                assert!(
                    known.contains(&id),
                    "line {l} points at an unexported memory"
                );
            }
        }
        writer.join().unwrap();
    });
    assert!(exports > 0);
}

#[test]
fn export_works_on_a_read_only_engine() {
    let f = populated();
    let (rw_lines, _) = export_lines(&f.engine);
    drop(f.engine);
    let ro = Engine::open(&f.path, OpenMode::ReadOnly, f.clock.clone()).unwrap();
    let (ro_lines, _) = export_lines(&ro);
    assert_eq!(rw_lines, ro_lines);
}

// ---- import (restore) ----

fn export_text(engine: &Engine) -> String {
    let mut out = Vec::new();
    engine.export(&mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// An empty file with an embedder, ready to restore into.
fn target() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Engine,
    Arc<FixedClock>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restored.db");
    let clock = Arc::new(FixedClock::new(1000));
    let engine = Engine::open(&path, OpenMode::ReadWrite, clock.clone())
        .unwrap()
        .with_embedder(Arc::new(HashEmbedder::new("m", 256)));
    (dir, path, engine, clock)
}

fn bad_import(e: Result<ExportSummary>) -> String {
    match e {
        Err(Error::BadImport(m)) => m,
        other => panic!("expected BadImport, got {other:?}"),
    }
}

/// `populated()` plus a decision in the log, so every table of the export has something.
fn source() -> Fixture {
    let mut f = populated();
    let rig = ScriptedDecider::new("clasificador", Behavior::Say(Choice::Add, 0.9));
    f.engine = f.engine.with_decider(rig, DeciderMode::Shadow, 0.8);
    f.engine
        .propose(prop(f.scope, f.person, "trabaja en un periódico"))
        .unwrap();
    f
}

#[test]
fn export_then_import_round_trips() {
    let f = source();
    let text = export_text(&f.engine);
    let (_dir, tpath, restored, _) = target();
    let summary = restored.import(&mut text.as_bytes()).unwrap();
    assert_eq!(summary, f.engine.export(&mut Vec::new()).unwrap());
    assert_eq!(summary.decisions, 1);

    // everything the export carries comes back the same, byte for byte
    assert_eq!(export_text(&restored), text);
    let (a, b) = (&f.engine, &restored);
    assert_eq!(
        a.list(f.scope, None, None).unwrap(),
        b.list(f.scope, None, None).unwrap()
    );
    for id in [1, 2, 3] {
        assert_eq!(
            a.history(MemoryId(id)).unwrap(),
            b.history(MemoryId(id)).unwrap()
        );
    }
    assert_eq!(a.blocks(f.scope).unwrap(), b.blocks(f.scope).unwrap());
    assert_eq!(
        a.decisions(f.scope, 0, i64::MAX).unwrap(),
        b.decisions(f.scope, 0, i64::MAX).unwrap()
    );
    assert_eq!(
        a.feedback_summary(MemoryId(1)).unwrap(),
        b.feedback_summary(MemoryId(1)).unwrap()
    );
    // the projection was refolded: search works, and the file passes its own health check
    let found = b
        .search(f.scope, "Medellín", SearchOpts::default())
        .unwrap();
    assert_eq!(found.len(), 1);
    let report = b.doctor().unwrap();
    assert!(report.all_ok(), "failing: {:?}", failing(&report));
    // and the restored file keeps working: new ids continue after the imported ones
    let next = b
        .propose(prop(f.scope, ActorId(1), "dato nuevo tras restaurar"))
        .unwrap();
    assert_eq!(next.memory, MemoryId(4));
    drop(tpath);
}

#[test]
fn import_refuses_a_file_with_data() {
    let f = source();
    let text = export_text(&f.engine);
    let (_dir, tpath, restored, _) = target();
    restored.create_scope("otro", "es").unwrap();
    let before = dump_all(&tpath);
    assert!(matches!(
        restored.import(&mut text.as_bytes()),
        Err(Error::InvalidState)
    ));
    assert_eq!(before, dump_all(&tpath));
    // restoring twice is the same case
    let (_d2, _p2, again, _) = target();
    again.import(&mut text.as_bytes()).unwrap();
    assert!(matches!(
        again.import(&mut text.as_bytes()),
        Err(Error::InvalidState)
    ));
}

#[test]
fn import_accepts_a_target_that_only_has_the_system_actor() {
    let f = source();
    let text = export_text(&f.engine);
    let (dir, tpath, engine, clock) = target();
    drop(engine);
    // a file that already ran background jobs has the internal actor and nothing else
    rusqlite::Connection::open(&tpath)
        .unwrap()
        .execute(
            "INSERT INTO actors(id, name, kind, quota_max, quota_window)
             VALUES (7, 'clio-memory:system', 'System', 100, 3600)",
            [],
        )
        .unwrap();
    let engine = Engine::open(&tpath, OpenMode::ReadWrite, clock).unwrap();
    engine.import(&mut text.as_bytes()).unwrap();
    let names: Vec<String> = engine
        .actors()
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(
        names,
        ["ana", "bot"],
        "the exported actors replace the internal one"
    );
    drop(dir);

    // any other actor in the target is data: refuse
    let (_d2, p2, e2, _) = target();
    e2.register_actor("alguien", ActorKind::Person, Quota::default())
        .unwrap();
    assert!(matches!(
        e2.import(&mut text.as_bytes()),
        Err(Error::InvalidState)
    ));
    drop(p2);
}

#[test]
fn import_rejects_a_different_major_a_newer_minor_and_a_different_schema_version() {
    let f = source();
    let text = export_text(&f.engine);
    for (from, to) in [
        (
            "\"format_version\":\"1.0.0\"",
            "\"format_version\":\"2.0.0\"",
        ),
        (
            "\"format_version\":\"1.0.0\"",
            "\"format_version\":\"1.1.0\"",
        ),
        (
            "\"format_version\":\"1.0.0\"",
            "\"format_version\":\"banana\"",
        ),
        ("\"schema_version\":1", "\"schema_version\":2"),
        ("clio-memory-export", "something-else"),
    ] {
        assert!(text.contains(from), "{from}");
        let changed = text.replacen(from, to, 1);
        let (_dir, tpath, restored, _) = target();
        let before = dump_all(&tpath);
        bad_import(restored.import(&mut changed.as_bytes()));
        assert_eq!(before, dump_all(&tpath), "{to} left traces");
    }
    // a newer patch of the same minor is fine
    let patched = text.replacen(
        "\"format_version\":\"1.0.0\"",
        "\"format_version\":\"1.0.7\"",
        1,
    );
    let (_dir, _p, restored, _) = target();
    restored.import(&mut patched.as_bytes()).unwrap();
}

#[test]
fn import_rejects_an_unknown_line_type_and_a_bad_line_with_its_line_number() {
    let f = source();
    let text = export_text(&f.engine);
    let count = text.lines().count();
    let (_dir, tpath, restored, _) = target();
    let before = dump_all(&tpath);

    let weird = format!("{text}{{\"type\":\"weird\"}}\n");
    let msg = bad_import(restored.import(&mut weird.as_bytes()));
    assert!(msg.contains(&format!("line {}", count + 1)), "{msg}");

    let garbage = text.replacen("\"type\":\"usage\"", "not json", 1);
    let msg = bad_import(restored.import(&mut garbage.as_bytes()));
    assert!(msg.contains("line "), "{msg}");

    let headless: String = text.lines().skip(1).map(|l| format!("{l}\n")).collect();
    bad_import(restored.import(&mut headless.as_bytes()));
    let two_headers = format!("{text}{}\n", text.lines().next().unwrap());
    bad_import(restored.import(&mut two_headers.as_bytes()));
    bad_import(restored.import(&mut "".as_bytes()));
    assert_eq!(before, dump_all(&tpath));
}

// S-18: a restore never brings a secret back into the memory, and a rejected file changes nothing
#[test]
fn import_rejects_a_secret_and_leaves_the_target_untouched() {
    let f = source();
    let text = export_text(&f.engine);
    let tainted = text.replacen("tiene un perro", "clave AKIAIOSFODNN7EXAMPLE", 1);
    assert_ne!(tainted, text);
    let (_dir, tpath, restored, _) = target();
    let before = dump_all(&tpath);
    bad_import(restored.import(&mut tainted.as_bytes()));
    assert_eq!(before, dump_all(&tpath));
    assert!(restored.scopes().unwrap().is_empty());
}

#[test]
fn import_is_atomic() {
    let f = source();
    let text = export_text(&f.engine);
    let broken = format!("{text}{{\"type\":\"usage\"}}\n"); // the last line is missing its fields
    let (_dir, tpath, restored, _) = target();
    let before = dump_all(&tpath);
    bad_import(restored.import(&mut broken.as_bytes()));
    assert_eq!(before, dump_all(&tpath));
    // the same engine can still restore a good file afterwards
    restored.import(&mut text.as_bytes()).unwrap();
}

#[test]
fn import_rejects_an_oversized_line() {
    let f = source();
    let text = export_text(&f.engine);
    let huge = format!("{text}{}\n", "x".repeat(9 << 20));
    let (_dir, _p, restored, _) = target();
    let msg = bad_import(restored.import(&mut huge.as_bytes()));
    assert!(msg.contains("too long"), "{msg}");
}

#[test]
fn import_enqueues_embedding_with_an_embedder() {
    let f = source();
    let text = export_text(&f.engine);
    let (_dir, tpath, restored, _) = target();
    restored.import(&mut text.as_bytes()).unwrap();
    assert_eq!(
        job_rows(&tpath),
        [("Embed".to_string(), "Pending".to_string())]
    );
    assert_eq!(restored.stats().unwrap().pending_embeddings, 3);

    // without an embedder there is nothing to queue
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.db");
    let plain = Engine::open(&path, OpenMode::ReadWrite, Arc::new(FixedClock::new(1000))).unwrap();
    plain.import(&mut text.as_bytes()).unwrap();
    assert!(job_rows(&path).is_empty());
}

#[test]
fn import_needs_a_read_write_engine() {
    let f = source();
    let text = export_text(&f.engine);
    let (_dir, tpath, restored, clock) = target();
    drop(restored);
    let ro = Engine::open(&tpath, OpenMode::ReadOnly, clock).unwrap();
    assert!(matches!(
        ro.import(&mut text.as_bytes()),
        Err(Error::ReadOnly)
    ));
}

// ---- import_memories ----

fn importer(f: &Fixture, quota: Quota) -> ActorId {
    f.engine
        .register_actor("cli-import:ana", ActorKind::Importer, quota)
        .unwrap()
}

fn load(f: &Fixture, actor: ActorId, text: &str, opts: &ImportOpts) -> ImportSummary {
    f.engine
        .import_memories(&mut text.as_bytes(), f.scope, actor, opts)
        .unwrap()
}

fn as_user() -> ImportOpts {
    ImportOpts {
        origin: Origin::User,
    }
}

// S-25: loose memories from outside wait in quarantine; only a person's promote activates them
#[test]
fn import_memories_goes_through_propose_and_quarantines_by_default() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    let text = "{\"content\":\"vive en Medellín\"}\n{\"content\":\"tiene un perro\"}\n";
    let s = load(&f, imp, text, &ImportOpts::default());
    assert_eq!(ImportOpts::default().origin, Origin::External);
    assert_eq!((s.read, s.added, s.noop, s.quarantined), (2, 0, 0, 2));
    assert!(s.rejected.is_empty());
    assert_eq!(
        f.engine
            .list(f.scope, Some(State::Quarantine), None)
            .unwrap()
            .len(),
        2
    );
    assert!(
        f.engine
            .search(f.scope, "Medellín", SearchOpts::default())
            .unwrap()
            .is_empty()
    );
    // a person promotes what they accept; the history shows the importer as the author
    let m = f
        .engine
        .list(f.scope, Some(State::Quarantine), None)
        .unwrap()[0]
        .id;
    f.engine.promote(m, f.person).unwrap();
    assert_eq!(f.engine.history(m).unwrap()[0].actor, imp);
}

#[test]
fn import_memories_honors_every_field() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    let line = "{\"content\":\"usa Rust\",\"kind\":\"Procedural\",\"topic_key\":\"lenguaje\",\
                \"valid_from\":500,\"valid_to\":9000,\"source\":\"notas.md\"}\n";
    let s = load(&f, imp, line, &as_user());
    assert_eq!((s.added, s.rejected.len()), (1, 0));
    let row = &f.engine.list(f.scope, None, None).unwrap()[0];
    assert_eq!(row.kind, MemoryType::Procedural);
    assert_eq!(row.topic_key.as_deref(), Some("lenguaje"));
    assert_eq!((row.valid_from, row.valid_to), (500, Some(9000)));
    assert_eq!(row.source.as_deref(), Some("notas.md"));
}

// S-18: a line with a secret is rejected (and never echoed back)
#[test]
fn a_bad_line_is_rejected_with_its_number_and_the_rest_import() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    let long = format!("{{\"content\":\"{}\"}}", "a".repeat(20_000));
    let text = [
        "{\"content\":\"vive en Medellín\"}".to_string(), // 1 ok
        "not json".to_string(),                           // 2
        String::new(),                                    // 3 blank: skipped, still numbered
        "{\"kind\":\"semantic\"}".to_string(),            // 4 no content
        "{\"content\":\"x\",\"kind\":\"banana\"}".to_string(), // 5 unknown kind
        long,                                             // 6 too long
        "{\"content\":\"clave AKIAIOSFODNN7EXAMPLE\"}".to_string(), // 7 secret
        "{\"content\":\"tiene un perro\"}".to_string(),   // 8 ok
    ]
    .join("\n");
    let s = load(&f, imp, &text, &as_user());
    assert_eq!((s.read, s.added), (7, 2), "blank lines are not read");
    let numbers: Vec<usize> = s.rejected.iter().map(|r| r.0).collect();
    assert_eq!(numbers, [2, 4, 5, 6, 7]);
    assert_eq!(s.rejected_total, 5);
    assert!(s.rejected.iter().all(|r| !r.1.is_empty()));
    assert!(
        s.rejected.iter().all(|r| !r.1.contains("AKIA")),
        "never echo a secret"
    );
}

#[test]
fn rejected_lines_are_capped_but_counted() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    let text = "nope\n".repeat(150);
    let s = load(&f, imp, &text, &as_user());
    assert_eq!(
        (s.read, s.rejected.len(), s.rejected_total),
        (150, 100, 150)
    );
    assert_eq!(s.rejected[0].0, 1);
}

#[test]
fn an_oversized_line_costs_only_that_line() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    let text = format!(
        "{{\"content\":\"vive en Medellín\"}}\n{}\n{{\"content\":\"tiene un perro\"}}\n",
        "x".repeat(9 << 20)
    );
    let s = load(&f, imp, &text, &as_user());
    assert_eq!((s.added, s.rejected.len()), (2, 1));
    assert_eq!(s.rejected[0].0, 2);
}

// S-27: the actor's quota applies to every imported line
#[test]
fn quota_is_respected_and_counted_as_rejected() {
    let f = fixture();
    let imp = importer(
        &f,
        Quota {
            max: 3,
            window_secs: 3600,
        },
    );
    let text: String = (0..5)
        .map(|i| format!("{{\"content\":\"dato {i}\"}}\n"))
        .collect();
    let s = load(&f, imp, &text, &as_user());
    assert_eq!((s.read, s.added), (5, 3));
    assert_eq!(s.rejected.iter().map(|r| r.0).collect::<Vec<_>>(), [4, 5]);
    assert!(s.rejected[0].1.contains("quota"), "{}", s.rejected[0].1);
}

#[test]
fn an_agent_cannot_be_the_importer() {
    let f = fixture();
    let err = f
        .engine
        .import_memories(
            &mut "{\"content\":\"x\"}\n".as_bytes(),
            f.scope,
            f.agent,
            &ImportOpts::default(),
        )
        .unwrap_err();
    assert!(matches!(err, Error::NotAllowed));
    assert!(f.engine.list(f.scope, None, None).unwrap().is_empty());
}

#[test]
fn origin_user_is_only_honored_for_a_person_or_importer() {
    for who in ["importer", "person"] {
        let f = fixture();
        let actor = if who == "importer" {
            importer(&f, Quota::default())
        } else {
            f.person
        };
        let text = "{\"content\":\"vive en Medellín\"}\n";
        let s = load(&f, actor, text, &as_user());
        assert_eq!((s.added, s.quarantined), (1, 0), "{who} with origin user");
        let f = fixture();
        let actor = if who == "importer" {
            importer(&f, Quota::default())
        } else {
            f.person
        };
        let s = load(&f, actor, text, &ImportOpts::default());
        assert_eq!((s.added, s.quarantined), (0, 1), "{who} by default");
    }
}

#[test]
fn duplicates_are_noops_and_counted() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    let text = "{\"content\":\"vive en Medellín\"}\n{\"content\":\"vive en Medellín\"}\n";
    let s = load(&f, imp, text, &as_user());
    assert_eq!((s.read, s.added, s.noop), (2, 1, 1));
    let again = load(&f, imp, text, &as_user());
    assert_eq!((again.added, again.noop), (0, 2));
}

#[test]
fn import_memories_enqueues_embedding_with_an_embedder() {
    let f = embedded();
    let imp = importer(&f, Quota::default());
    let text = "{\"content\":\"vive en Medellín\"}\n{\"content\":\"tiene un perro\"}\n";
    load(&f, imp, text, &as_user());
    assert_eq!(
        job_rows(&f.path),
        [("Embed".to_string(), "Pending".to_string())]
    );
}

#[test]
fn import_memories_checks_the_target_before_reading() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    let one = "{\"content\":\"x\"}\n";
    let run = |scope, actor| {
        f.engine
            .import_memories(&mut one.as_bytes(), scope, actor, &ImportOpts::default())
    };
    assert!(matches!(run(ScopeId(99), imp), Err(Error::UnknownScope)));
    assert!(matches!(
        run(f.scope, ActorId(99)),
        Err(Error::UnknownActor)
    ));
}

#[test]
fn import_memories_needs_a_read_write_engine() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    drop(f.engine);
    let ro = Engine::open(&f.path, OpenMode::ReadOnly, f.clock.clone()).unwrap();
    let r = ro.import_memories(
        &mut "{\"content\":\"x\"}\n".as_bytes(),
        f.scope,
        imp,
        &ImportOpts::default(),
    );
    assert!(matches!(r, Err(Error::ReadOnly)));
}

// S-23: an export file is created 0600 and never replaces an existing file (it is not encrypted: see the spec's open decision)
#[test]
fn export_to_file_creates_a_private_file_and_never_overwrites() {
    use std::os::unix::fs::PermissionsExt;
    let f = populated();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.jsonl");
    let summary = f.engine.export_to_file(&path).unwrap();
    assert_eq!(summary.scopes, 1);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        export_text(&f.engine)
    );
    let before = std::fs::read(&path).unwrap();
    let err = f.engine.export_to_file(&path).unwrap_err();
    assert!(matches!(&err, Error::Io(e) if e.kind() == std::io::ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

// ---- telemetry is optional, and absent means absent ----

#[test]
fn no_telemetry_table_exists_and_the_schema_is_the_expected_set() {
    let f = populated();
    let conn = rusqlite::Connection::open(&f.path).unwrap();
    let mut names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    names.retain(|n| !n.starts_with("memories_fts_") && !n.starts_with("sqlite_"));
    names.sort();
    assert_eq!(
        names,
        [
            "actors",
            "block_memories",
            "blocks",
            "decisions",
            "feedback",
            "jobs",
            "memories",
            "memories_fts",
            "operation_memories",
            "operations",
            "quota_counter",
            "scopes",
            "usage",
            "vectors",
        ]
    );
}

#[cfg(not(feature = "telemetry"))]
#[test]
fn without_the_feature_trace_id_is_none() {
    let f = populated();
    let (lines, _) = export_lines(&f.engine);
    for l in lines.iter().filter(|l| kind(l) == "operation") {
        assert!(l["trace_id"].is_null(), "{l}");
    }
    assert!(
        f.engine
            .history(MemoryId(1))
            .unwrap()
            .iter()
            .all(|o| o.trace_id.is_none())
    );
}

#[test]
fn an_export_without_trace_ids_still_imports() {
    // exports written before the field existed have no "trace_id" key at all
    let f = populated();
    let text: String = export_text(&f.engine)
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            if let Some(o) = v.as_object_mut() {
                o.remove("trace_id");
            }
            format!("{v}\n")
        })
        .collect();
    assert!(!text.contains("trace_id"));
    let (_dir, _p, restored, _) = target();
    restored.import(&mut text.as_bytes()).unwrap();
    assert!(restored.doctor().unwrap().all_ok());
}

// ---- fixes from the final review ----

// S-18: a restore scans every text a memory snapshot or a name can carry, not only `content`
#[test]
fn import_rejects_a_secret_in_any_text_field_or_name() {
    let f = source();
    let text = export_text(&f.engine);
    let key = "clave AKIAIOSFODNN7EXAMPLE";
    for (from, to) in [
        ("\"topic_key\":null", format!("\"topic_key\":\"{key}\"")),
        ("\"source\":null", format!("\"source\":\"{key}\"")),
        ("\"session\":null", format!("\"session\":\"{key}\"")),
        ("\"episode\":null", format!("\"episode\":\"{key}\"")),
        ("\"name\":\"main\"", format!("\"name\":\"{key}\"")),
        ("\"name\":\"ana\"", format!("\"name\":\"{key}\"")),
        ("\"name\":\"perfil\"", format!("\"name\":\"{key}\"")),
    ] {
        assert!(text.contains(from), "{from}");
        let tainted = text.replacen(from, &to, 1);
        let (_dir, tpath, restored, _) = target();
        let before = dump_all(&tpath);
        bad_import(restored.import(&mut tainted.as_bytes()));
        assert_eq!(before, dump_all(&tpath), "{from} left traces");
    }
}

#[test]
fn import_rejects_an_unreasonable_trace_id_and_a_dangling_reference_with_a_message() {
    let f = source();
    let text = export_text(&f.engine);
    let (_dir, _p, restored, _) = target();
    let junk = text.replacen("\"trace_id\":null", "\"trace_id\":\"has spaces\"", 1);
    if junk != text {
        bad_import(restored.import(&mut junk.as_bytes()));
    }
    // a memory snapshot that names an actor the file does not have
    let dangling = text.replacen("\"actor\":1,\"session\"", "\"actor\":99,\"session\"", 1);
    assert_ne!(dangling, text);
    let msg = bad_import(restored.import(&mut dangling.as_bytes()));
    assert!(!msg.is_empty());
}

#[test]
fn feedback_that_flags_is_all_or_nothing() {
    let f = fixture();
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap()
        .memory;
    f.engine.feedback(m, Signal::Wrong, f.agent).unwrap();
    f.engine.feedback(m, Signal::Wrong, f.agent).unwrap();
    // spending the signals will fail, after the flag operation would have been written
    raw(
        &f,
        "CREATE TRIGGER no_spend BEFORE UPDATE ON feedback BEGIN SELECT RAISE(ABORT, 'boom'); END",
    );
    assert!(f.engine.feedback(m, Signal::Wrong, f.agent).is_err());
    raw(&f, "DROP TRIGGER no_spend");
    let row = f.engine.get(m).unwrap();
    assert_eq!(
        row.state,
        State::Active,
        "no flag without the signals being spent"
    );
    assert_eq!(f.engine.history(m).unwrap().len(), 1);
    assert_eq!(
        f.engine.feedback_summary(m).unwrap().wrong,
        2,
        "the third signal was not kept either"
    );
}

#[test]
fn promote_that_cannot_spend_the_wrong_signals_does_not_promote() {
    let f = fixture();
    let mut ext = prop(f.scope, f.agent, "dato de fuera");
    ext.origin = Origin::External;
    let m = f.engine.propose(ext).unwrap().memory;
    raw(
        &f,
        &format!(
            "INSERT INTO feedback(memory, actor, signal, at, spent) VALUES ({0}, 1, 'Wrong', 1, 0);
         CREATE TRIGGER no_spend BEFORE UPDATE ON feedback BEGIN SELECT RAISE(ABORT, 'boom'); END",
            m.0
        ),
    );
    assert!(f.engine.promote(m, f.person).is_err());
    assert_eq!(f.engine.get(m).unwrap().state, State::Quarantine);
    assert_eq!(f.engine.history(m).unwrap().len(), 1);
}

#[test]
fn import_memories_reports_where_it_stopped_when_storage_fails() {
    let f = fixture();
    let imp = importer(&f, Quota::default());
    // the third operation cannot be written
    raw(
        &f,
        "CREATE TRIGGER stop BEFORE INSERT ON operations WHEN (SELECT COUNT(*) FROM operations) >= 2
         BEGIN SELECT RAISE(ABORT, 'disk on fire'); END",
    );
    let text = "{\"content\":\"uno\"}\n{\"content\":\"dos\"}\n{\"content\":\"tres\"}\n{\"content\":\"cuatro\"}\n";
    let s = load(&f, imp, text, &as_user());
    assert_eq!((s.read, s.added), (3, 2), "what got in is reported");
    let (line, why) = s.aborted.expect("the run says it stopped");
    assert_eq!(line, 3);
    assert!(!why.is_empty());
}
