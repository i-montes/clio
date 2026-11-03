#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use clio_memory::*;
use common::*;

fn run(db: Option<&Path>, args: &[&str]) -> Output {
    // Tests run in parallel threads. While one thread holds an engine open, another can fork a
    // child that inherits a copy of the lock file until its `exec`; the lock then outlives the
    // first thread's `drop(engine)` for a moment. That is the harness, not the product: wait it
    // out. The only tests that want the lock error use the library, not this helper.
    for _ in 0..60 {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio-memory"));
        if let Some(db) = db {
            cmd.arg("--db").arg(db);
        }
        let o = cmd.env("USER", "tester").args(args).output().unwrap();
        let locked = o.status.code() == Some(1)
            && String::from_utf8_lossy(&o.stderr).contains("another process owns the write lock");
        if !locked {
            return o;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio-memory"));
    if let Some(db) = db {
        cmd.arg("--db").arg(db);
    }
    cmd.env("USER", "tester").args(args).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap()
}

/// A scope "main" with an active memory, a quarantined one and a pending Embed job.
fn populated() -> Fixture {
    let f = fixture_with(Arc::new(HashEmbedder::new("m", 256)));
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let mut ext = prop(f.scope, f.agent, "dato de fuera");
    ext.origin = Origin::External;
    f.engine.propose(ext).unwrap();
    f
}

#[test]
fn no_command_or_unknown_command_exits_2_with_usage() {
    let f = populated();
    let o = run(None, &[]);
    assert_eq!(code(&o), 2);
    assert!(err(&o).to_lowercase().contains("usage"), "{}", err(&o));
    let o = run(Some(&f.path), &["frobnicate"]);
    assert_eq!(code(&o), 2);
    assert!(err(&o).contains("frobnicate"));
    let o = run(None, &["scopes"]);
    assert_eq!(code(&o), 2);
    assert!(err(&o).contains("--db"));
    let o = run(Some(&f.path), &["get", "not-an-id"]);
    assert_eq!(code(&o), 2);
    let o = run(Some(&f.path), &["list"]);
    assert_eq!(code(&o), 2, "list needs a scope");
    let o = run(None, &["--help"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("clio-memory --db"));
}

#[test]
fn scopes_stats_list_get_history_review_jobs_print_what_the_library_holds() {
    let f = populated();
    let o = run(Some(&f.path), &["scopes"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("main") && out(&o).contains("es"));

    let o = run(Some(&f.path), &["stats"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("main") && out(&o).contains("operations"));

    let o = run(Some(&f.path), &["list", "main"]);
    assert!(out(&o).contains("vive en Medellín") && out(&o).contains("dato de fuera"));
    let o = run(Some(&f.path), &["list", "main", "--state", "quarantine"]);
    assert!(!out(&o).contains("Medellín") && out(&o).contains("dato de fuera"));
    let o = run(Some(&f.path), &["list", "main", "--state", "bogus"]);
    assert_eq!(code(&o), 2);

    for id in ["1", "m1"] {
        let o = run(Some(&f.path), &["get", id]);
        assert_eq!(code(&o), 0);
        assert!(out(&o).contains("vive en Medellín") && out(&o).contains("Active"));
    }
    let o = run(Some(&f.path), &["get", "99"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).to_lowercase().contains("not found"));

    let o = run(Some(&f.path), &["history", "1"]);
    assert!(out(&o).contains("Add"));

    let o = run(Some(&f.path), &["review", "main"]);
    assert!(out(&o).contains("dato de fuera") && !out(&o).contains("Medellín"));

    let o = run(Some(&f.path), &["jobs"]);
    assert!(out(&o).contains("Embed") && out(&o).contains("Pending"));
    let o = run(Some(&f.path), &["jobs", "--state", "failed"]);
    assert!(!out(&o).contains("Embed"));

    let o = run(Some(&f.path), &["list", "nope"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("unknown scope") && err(&o).contains("main"));
}

#[test]
fn json_flag_prints_valid_json() {
    let f = populated();
    let json = |args: &[&str]| -> serde_json::Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let o = run(Some(&f.path), &all);
        assert_eq!(code(&o), 0, "{args:?}: {}", err(&o));
        serde_json::from_str(&out(&o)).unwrap_or_else(|e| panic!("{args:?}: {e}: {}", out(&o)))
    };
    assert_eq!(json(&["scopes"])[0]["name"], "main");
    assert!(json(&["stats"])["file_bytes"].as_u64().unwrap() > 0);
    let doctor = json(&["doctor"]);
    assert_eq!(doctor["ok"], true);
    assert!(doctor["checks"].as_array().unwrap().len() >= 8);
    assert_eq!(json(&["list", "main"]).as_array().unwrap().len(), 2);
    assert_eq!(json(&["get", "1"])["content"], "vive en Medellín");
    assert_eq!(json(&["history", "1"]).as_array().unwrap().len(), 1);
    assert_eq!(json(&["jobs"])[0]["kind"], "Embed");
}

#[test]
fn doctor_exits_0_when_healthy_and_1_when_a_check_fails() {
    let f = populated();
    let o = run(Some(&f.path), &["doctor"]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    rusqlite::Connection::open(&f.path)
        .unwrap()
        .execute("UPDATE memories SET content = 'x' WHERE id = 1", [])
        .unwrap();
    let o = run(Some(&f.path), &["doctor"]);
    assert_eq!(code(&o), 1);
    assert!(out(&o).contains("projection_matches_log") && out(&o).contains("FAIL"));
}

#[test]
fn recall_and_context_work_without_a_model() {
    let f = populated();
    let o = run(Some(&f.path), &["recall", "main", "Medellín"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("vive en Medellín"));
    let o = run(Some(&f.path), &["recall", "main", "astronomía"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("nothing relevant"));
    let o = run(Some(&f.path), &["context", "main", "Medellín"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("vive en Medellín"));
}

#[test]
fn read_commands_work_while_another_process_holds_the_writer_lock() {
    let f = populated(); // `f.engine` keeps the writer lock for the whole test
    for args in [
        vec!["scopes"],
        vec!["list", "main"],
        vec!["recall", "main", "Medellín"],
    ] {
        let o = run(Some(&f.path), &args);
        assert_eq!(code(&o), 0, "{args:?}: {}", err(&o));
    }
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !n.ends_with("-wal") && !n.ends_with("-shm"))
        .collect();
    names.sort();
    names
}

#[test]
fn read_commands_never_write() {
    let f = populated();
    let (path, clock) = (f.path.clone(), f.clock.clone());
    let jobs_before = job_rows(&path);
    let dir = path.parent().unwrap().to_owned();
    drop(f.engine);
    let bytes = std::fs::read(&path).unwrap();
    let names = listing(&dir);
    for args in [
        vec!["recall", "main", "Medellín"],
        vec!["context", "main", "Medellín"],
        vec!["list", "main"],
        vec!["stats"],
        vec!["doctor"],
    ] {
        let o = run(Some(&path), &args);
        assert_eq!(code(&o), 0, "{args:?}: {}", err(&o));
    }
    // SQLite itself keeps `-wal`/`-shm` next to a WAL database while someone reads it; `listing`
    // leaves those out, and the main file and every other name must be untouched
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "the file changed");
    assert_eq!(listing(&dir), names, "files appeared or vanished");
    assert_eq!(job_rows(&path), jobs_before, "a job was queued");
    let _ = clock;
}

#[test]
fn missing_database_is_a_clear_error_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("none.db");
    let o = run(Some(&db), &["scopes"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).to_lowercase().contains("not found"), "{}", err(&o));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

// S-25: whoever can register a person can promote; the command line has no way to do it
#[test]
fn the_cli_has_no_way_to_register_actors_create_scopes_or_set_retention() {
    let f = populated();
    for word in [
        "register-actor",
        "register_actor",
        "create-scope",
        "create_scope",
        "set-retention",
        "set_retention",
        "retention",
    ] {
        let o = run(Some(&f.path), &[word, "x"]);
        assert_eq!(code(&o), 2, "{word} should be an unknown command");
    }
}

#[cfg(not(feature = "model2vec"))]
#[test]
fn the_model_flag_needs_the_model2vec_feature() {
    let f = populated();
    let o = run(
        Some(&f.path),
        &["--model", "/nowhere", "recall", "main", "x"],
    );
    assert_eq!(code(&o), 2);
    assert!(err(&o).contains("model2vec"));
}

#[test]
fn terminal_escapes_in_memories_are_not_printed_raw() {
    let f = populated();
    f.engine
        .propose(prop(
            f.scope,
            f.person,
            "hola \u{1b}[31mrojo\u{1b}[0m amigo\r",
        ))
        .unwrap();
    for args in [
        vec!["list", "main"],
        vec!["get", "3"],
        vec!["recall", "main", "rojo"],
        vec!["context", "main", "rojo"],
        vec!["history", "3"],
    ] {
        let o = run(Some(&f.path), &args);
        assert_eq!(code(&o), 0, "{args:?}: {}", err(&o));
        assert!(!o.stdout.contains(&0x1b), "{args:?} printed a raw ESC");
        assert!(!o.stdout.contains(&b'\r'), "{args:?} printed a raw CR");
    }
    let o = run(Some(&f.path), &["list", "main"]);
    assert!(out(&o).contains("rojo"), "the text itself still shows");
    // JSON escapes control characters on its own
    let o = run(Some(&f.path), &["--json", "get", "3"]);
    assert!(!o.stdout.contains(&0x1b));
}

// ---- write commands ----

/// `populated()` with the engine closed, so the CLI can take the writer lock.
/// Memory 1 is active, memory 2 is quarantined; operations 1 and 2 added them.
fn closed() -> (tempfile::TempDir, std::path::PathBuf) {
    let Fixture {
        engine, path, _dir, ..
    } = populated();
    drop(engine);
    (_dir, path)
}

fn state_of(db: &Path, id: &str) -> String {
    let o = run(Some(db), &["--json", "get", id]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    serde_json::from_str::<serde_json::Value>(&out(&o)).unwrap()["state"]
        .as_str()
        .unwrap()
        .to_string()
}

// S-25: promote, archive and revert are done by a person actor, and are in the history under that name
#[test]
fn promote_archive_revert_work_as_a_person() {
    let (_dir, db) = closed();
    let o = run(Some(&db), &["promote", "2"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(state_of(&db, "2"), "Active");
    let o = run(Some(&db), &["history", "m2"]);
    assert!(
        out(&o).contains("Promote") && out(&o).contains("cli:tester"),
        "{}",
        out(&o)
    );

    let o = run(Some(&db), &["archive", "1"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(state_of(&db, "1"), "Archived");
    // operations 1 and 2 were the adds, 3 the promote, 4 the archive
    let o = run(Some(&db), &["revert", "4"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(state_of(&db, "1"), "Active");

    assert_eq!(
        code(&run(Some(&db), &["promote", "1"])),
        1,
        "already active"
    );
    assert_eq!(code(&run(Some(&db), &["revert", "999"])), 1);
    assert_eq!(code(&run(Some(&db), &["revert", "x"])), 2);
}

#[test]
fn feedback_through_the_cli_can_flag_a_memory() {
    let (_dir, db) = closed();
    let o = run(Some(&db), &["feedback", "1", "useful"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("0.60"), "{}", out(&o));
    for _ in 0..3 {
        assert_eq!(code(&run(Some(&db), &["feedback", "1", "wrong"])), 0);
    }
    assert_eq!(state_of(&db, "1"), "Quarantine");
    assert!(out(&run(Some(&db), &["history", "1"])).contains("Flag"));
    assert_eq!(
        code(&run(Some(&db), &["feedback", "1", "wrong"])),
        1,
        "not active any more"
    );
    assert_eq!(code(&run(Some(&db), &["feedback", "1", "meh"])), 2);
}

fn job_states(db: &Path) -> Vec<(String, String)> {
    job_rows(db)
}

#[test]
fn run_jobs_applies_queued_work_and_leaves_heavy_jobs_unless_asked() {
    let f = fixture(); // no embedder, so no Embed jobs
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    f.engine
        .recall(f.scope, "Medellín", &RecallOpts::default())
        .unwrap(); // queues a Touch
    f.engine.enqueue_vacuum().unwrap();
    let (path, _dir) = (f.path.clone(), f._dir);
    drop(f.engine);

    let o = run(Some(&path), &["run-jobs"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(
        out(&o).contains("done 1") && out(&o).contains("skipped heavy 1"),
        "{}",
        out(&o)
    );
    assert!(job_states(&path).contains(&("Touch".into(), "Done".into())));
    assert!(job_states(&path).contains(&("Vacuum".into(), "Pending".into())));

    let o = run(Some(&path), &["run-jobs", "--heavy"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(job_states(&path).contains(&("Vacuum".into(), "Done".into())));
}

#[test]
fn run_jobs_will_not_fail_embed_jobs_it_cannot_run() {
    let (_dir, db) = closed(); // an embedder queued an Embed job; the CLI has no model
    let o = run(Some(&db), &["run-jobs"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("--model"), "{}", err(&o));
    assert_eq!(
        job_states(&db),
        [("Embed".to_string(), "Pending".to_string())]
    );
}

// S-23: a backup made from the command line is 0600 and never overwrites
#[test]
fn backup_creates_a_private_copy_that_opens() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, db) = closed(); // has a pending Embed job the backup must not run
    let copy = dir.path().join("copy.db");
    let o = run(Some(&db), &["backup", copy.to_str().unwrap()]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(
        std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(code(&run(Some(&copy), &["doctor"])), 0);
    assert!(out(&run(Some(&copy), &["list", "main"])).contains("vive en Medellín"));
    assert_eq!(
        job_states(&db),
        [
            ("Embed".to_string(), "Pending".to_string()),
            ("Backup".to_string(), "Done".to_string()),
        ]
    );

    // never over an existing file, and without leaving a failed job behind
    let o = run(Some(&db), &["backup", copy.to_str().unwrap()]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("overwrite"), "{}", err(&o));
    assert!(out(&run(Some(&db), &["jobs", "--state", "failed"])).is_empty());
}

// S-23: an export made from the command line is 0600
#[test]
fn export_then_import_through_the_cli_round_trips() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, db) = closed();
    let dump = dir.path().join("dump.jsonl");
    let o = run(Some(&db), &["export", dump.to_str().unwrap()]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(
        std::fs::metadata(&dump).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let restored = dir.path().join("restored.db");
    let o = run(Some(&restored), &["import", dump.to_str().unwrap()]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(code(&run(Some(&restored), &["doctor"])), 0);
    assert_eq!(
        out(&run(Some(&restored), &["list", "main"])),
        out(&run(Some(&db), &["list", "main"]))
    );
    // exporting the restored file gives the same lines (only the header carries a time)
    let again = dir.path().join("again.jsonl");
    assert_eq!(
        code(&run(Some(&restored), &["export", again.to_str().unwrap()])),
        0
    );
    let body = |p: &Path| -> Vec<String> {
        std::fs::read_to_string(p)
            .unwrap()
            .lines()
            .skip(1)
            .map(String::from)
            .collect()
    };
    assert_eq!(body(&dump), body(&again));
    // importing again is refused: the file has data now
    let o = run(Some(&restored), &["import", dump.to_str().unwrap()]);
    assert_eq!(code(&o), 1);
}

#[test]
fn export_refuses_to_overwrite() {
    let (dir, db) = closed();
    let dump = dir.path().join("dump.jsonl");
    std::fs::write(&dump, "precious").unwrap();
    let o = run(Some(&db), &["export", dump.to_str().unwrap()]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("overwrite"), "{}", err(&o));
    assert_eq!(std::fs::read_to_string(&dump).unwrap(), "precious");
}

#[test]
fn import_memories_reports_rejected_lines_and_exits_1() {
    let (dir, db) = closed();
    let file = dir.path().join("loose.jsonl");
    std::fs::write(
        &file,
        "{\"content\":\"usa Rust\"}\nthis is not json\n{\"content\":\"tiene un perro\"}\n",
    )
    .unwrap();
    let o = run(
        Some(&db),
        &["import-memories", "main", file.to_str().unwrap()],
    );
    assert_eq!(code(&o), 1, "{}", err(&o));
    assert!(out(&o).contains("line 2"), "{}", out(&o));
    assert!(
        out(&o).contains("read 3") && out(&o).contains("quarantined 2"),
        "{}",
        out(&o)
    );
    let review = out(&run(Some(&db), &["review", "main"]));
    assert!(review.contains("usa Rust") && review.contains("tiene un perro"));
    let actors = out(&run(Some(&db), &["--json", "stats"]));
    assert!(!actors.is_empty());

    // a clean file exits 0; --origin user vouches for it
    std::fs::write(&file, "{\"content\":\"es zurdo\"}\n").unwrap();
    let o = run(
        Some(&db),
        &[
            "import-memories",
            "main",
            file.to_str().unwrap(),
            "--origin",
            "user",
        ],
    );
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("added 1"), "{}", out(&o));
    assert_eq!(
        code(&run(
            Some(&db),
            &[
                "import-memories",
                "main",
                file.to_str().unwrap(),
                "--origin",
                "bogus"
            ]
        )),
        2
    );
    assert_eq!(
        code(&run(
            Some(&db),
            &["import-memories", "nope", file.to_str().unwrap()]
        )),
        1
    );
}

#[test]
fn write_commands_fail_clearly_while_another_process_holds_the_writer_lock() {
    let f = populated(); // keeps the writer lock
    for args in [
        vec!["promote", "2"],
        vec!["run-jobs"],
        vec!["feedback", "1", "useful"],
    ] {
        let o = run(Some(&f.path), &args);
        assert_eq!(code(&o), 1, "{args:?}");
        assert!(err(&o).contains("lock"), "{args:?}: {}", err(&o));
    }
    // reading is still fine, and so is exporting (it only reads)
    let dump = f.path.with_file_name("dump.jsonl");
    assert_eq!(
        code(&run(Some(&f.path), &["export", dump.to_str().unwrap()])),
        0
    );
}

#[test]
fn write_commands_do_not_create_a_missing_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("none.db");
    let o = run(Some(&db), &["promote", "1"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).to_lowercase().contains("not found"), "{}", err(&o));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn actor_flag_must_name_a_person() {
    let (_dir, db) = closed();
    let o = run(Some(&db), &["--actor", "bot", "promote", "2"]); // "bot" is an agent
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("person"), "{}", err(&o));
    assert_eq!(state_of(&db, "2"), "Quarantine");
    // an unknown name becomes a person
    let o = run(Some(&db), &["--actor", "marta", "promote", "2"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&run(Some(&db), &["history", "2"])).contains("marta"));
}

#[test]
fn the_default_actor_is_created_once_and_reused() {
    let (_dir, db) = closed();
    for _ in 0..2 {
        assert_eq!(code(&run(Some(&db), &["feedback", "1", "useful"])), 0);
    }
    let named = |like: &str| -> i64 {
        rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM actors WHERE name = ?1", [like], |r| {
                r.get(0)
            })
            .unwrap()
    };
    assert_eq!(named("cli:tester"), 1);
    // without $USER the name falls back to "unknown"
    let o = Command::new(env!("CARGO_BIN_EXE_clio-memory"))
        .env_remove("USER")
        .arg("--db")
        .arg(&db)
        .args(["feedback", "1", "useful"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(named("cli:unknown"), 1);
}

#[test]
fn a_failed_import_into_a_new_file_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.jsonl");
    std::fs::write(&bad, "this is not an export\n").unwrap();
    let db = dir.path().join("new.db");
    let o = run(Some(&db), &["import", bad.to_str().unwrap()]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("import rejected"), "{}", err(&o));
    let left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left, ["bad.jsonl"]);
    // a missing input file is the same: no database appears
    let o = run(
        Some(&db),
        &["import", dir.path().join("nope").to_str().unwrap()],
    );
    assert_eq!(code(&o), 1);
    assert!(!db.exists());
}

// ---- fixes from the final review ----

#[test]
fn backup_that_could_not_happen_is_not_reported_as_written() {
    let (dir, db) = closed();
    let copy = dir.path().join("copy.db");
    // a directory where the temporary file should go: an error no retry would fix quickly
    std::fs::create_dir(dir.path().join("copy.db.tmp")).unwrap();
    let o = run(Some(&db), &["backup", copy.to_str().unwrap()]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    assert!(!out(&o).contains("backup written"), "{}", out(&o));
    assert!(!copy.exists());
    assert!(err(&o).to_lowercase().contains("backup"), "{}", err(&o));
}

#[test]
fn run_jobs_sees_a_waiting_embed_job_under_a_thousand_newer_ones() {
    let (_dir, db) = closed(); // one Embed job, queued first
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    for _ in 0..1001 {
        conn.execute(
            "INSERT INTO jobs(kind, payload, priority, state, run_after, created_at)
             VALUES ('Touch', '{\"ids\":[1],\"at\":1}', 5, 'Pending', 0, 1)",
            [],
        )
        .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    drop(conn);
    let o = run(Some(&db), &["run-jobs"]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    assert!(err(&o).contains("--model"), "{}", err(&o));
    assert!(job_states(&db).contains(&("Embed".to_string(), "Pending".to_string())));
}

#[test]
fn run_jobs_exits_1_when_a_job_failed() {
    let f = fixture(); // no embedder: no Embed jobs
    let taken = f.path.with_file_name("taken.db");
    std::fs::write(&taken, "x").unwrap();
    f.engine.enqueue_backup(&taken).unwrap(); // the destination exists: fails for good
    let (path, _dir) = (f.path.clone(), f._dir);
    drop(f.engine);
    let o = run(Some(&path), &["run-jobs", "--heavy"]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    assert!(out(&o).contains("failed 1"), "{}", out(&o));
}

/// A new file with one scope, made through the library: the CLI cannot do it (S-25).
fn new_db_with_scope(dir: &Path) -> std::path::PathBuf {
    let db = dir.join("new.db");
    let engine = Engine::open(&db, OpenMode::ReadWrite, Arc::new(SystemClock)).unwrap();
    engine.create_scope("work", "es").unwrap();
    db
}

#[test]
fn remember_writes_through_the_normal_rules() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db_with_scope(dir.path());
    let o = run(
        Some(&db),
        &[
            "remember",
            "work",
            "vive",
            "en",
            "Medellín",
            "--topic",
            "ciudad",
        ],
    );
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("Add"), "{}", out(&o));
    let o = run(Some(&db), &["remember", "work", "vive en Medellín"]);
    assert!(out(&o).contains("Noop"), "same content twice: {}", out(&o));
    let listed = out(&run(Some(&db), &["list", "work", "--state", "active"]));
    assert!(listed.contains("Medellín"), "{listed}");
    assert!(out(&run(Some(&db), &["recall", "work", "Medellín"])).contains("Medellín"));
}

#[test]
fn remember_as_an_agent_goes_through_the_agent_rules() {
    let dir = tempfile::tempdir().unwrap();
    let db = new_db_with_scope(dir.path());
    let o = run(
        Some(&db),
        &[
            "--actor",
            "claude",
            "remember",
            "work",
            "el CLI no crea ámbitos",
            "--kind",
            "episodic",
        ],
    );
    assert_eq!(code(&o), 0, "{}", err(&o));
    let got = out(&run(Some(&db), &["get", "1"]));
    assert!(got.contains("Agent") && got.contains("Episodic"), "{got}");
}

#[test]
fn remember_needs_an_existing_scope_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("x.db");
    let o = run(Some(&db), &["remember", "work", "algo"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("database not found"), "{}", err(&o));
    assert!(!db.exists(), "remember must not create the file");
    let db = new_db_with_scope(dir.path());
    let o = run(Some(&db), &["remember", "otro", "algo"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("unknown scope"), "{}", err(&o));
    let o = run(Some(&db), &["remember", "work", "algo", "--kind", "raro"]);
    assert_eq!(code(&o), 2);
}

// S-25: purge is for a person, asks first, and leaves a tombstone that `get` and `history` show
#[test]
fn purge_needs_confirmation_a_person_and_leaves_a_tombstone() {
    let (_dir, db) = closed();
    // no terminal and no --yes: refused, nothing erased
    let o = run(Some(&db), &["purge", "1"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("--yes"), "{}", err(&o));
    assert_eq!(state_of(&db, "1"), "Active");
    // an agent cannot (the actor "bot" is an agent)
    let o = run(Some(&db), &["--actor", "bot", "purge", "1", "--yes"]);
    assert_eq!(code(&o), 1);
    assert_eq!(state_of(&db, "1"), "Active");

    let o = run(Some(&db), &["purge", "m1", "--yes", "--reason", "pedido"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(out(&o).lines().count(), 1, "{}", out(&o));
    assert!(out(&o).contains("purged m1 (op "), "{}", out(&o));

    let got = out(&run(Some(&db), &["get", "1"]));
    assert!(got.contains("Purged") && got.contains("pedido"), "{got}");
    assert!(!got.contains("Medell"), "{got}");
    let json = out(&run(Some(&db), &["--json", "get", "1"]));
    assert!(json.contains("\"reason\":\"pedido\"") && !json.contains("Medell"));
    let hist = out(&run(Some(&db), &["history", "1"]));
    assert!(hist.contains("Purge") && !hist.contains("Medell"), "{hist}");
    let hist = out(&run(Some(&db), &["--json", "history", "1"]));
    assert!(!hist.contains("Medell"), "{hist}");
    assert!(!dump_all(&db).contains("Medell"));

    let o = run(Some(&db), &["purge", "1", "--yes"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("already purged"), "{}", err(&o));
    let o = run(Some(&db), &["purge", "99", "--yes"]);
    assert!(err(&o).contains("does not exist"), "{}", err(&o));
    assert_eq!(code(&run(Some(&db), &["get", "99"])), 1);
    assert_eq!(code(&run(Some(&db), &["doctor"])), 0);
    assert_eq!(code(&run(Some(&db), &["purge"])), 2);
}
