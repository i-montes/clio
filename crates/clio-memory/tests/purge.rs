//! PURGE (spec #11, S-24, S-25): the content of a memory is erased everywhere, past operations
//! included, and a tombstone says it existed. Every test uses a real file.
#![allow(clippy::unwrap_used)] // tests: panicking is the failure mode

mod common;

use std::io::BufReader;
use std::sync::Arc;

use clio_memory::*;
use common::*;

/// Unique words, so a byte search cannot hit anything else in the file.
const TEXT: &str = "la tortuguita zafiro vive en el estanque violeta";
const WORD: &str = "tortug"; // what the stemmed text index holds of "tortuguita"

fn topical(f: &Fixture, content: &str) -> Proposal {
    let mut p = prop(f.scope, f.person, content);
    p.topic_key = Some("mascota".into());
    p
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    let n = needle.as_bytes();
    haystack.windows(n.len()).any(|w| w == n)
}

/// Bytes of the database file and of its write-ahead log, as they are on disk.
fn raw_files(f: &Fixture) -> Vec<u8> {
    let mut all = std::fs::read(&f.path).unwrap();
    let mut wal = f.path.as_os_str().to_owned();
    wal.push("-wal");
    if let Ok(bytes) = std::fs::read(wal) {
        all.extend(bytes);
    }
    all
}

fn exported(f: &Fixture) -> String {
    let mut out = Vec::new();
    f.engine.export(&mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn import_text(text: &str) -> (Result<ExportSummary>, tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(
        &dir.path().join("restored.db"),
        OpenMode::ReadWrite,
        Arc::new(FixedClock::new(5000)),
    )
    .unwrap();
    let r = engine.import(&mut BufReader::new(text.as_bytes()));
    (r, dir, engine)
}

/// A memory with a rich history: added, replaced by a newer one, archived and reverted.
fn with_history(f: &Fixture) -> (MemoryId, MemoryId) {
    f.clock.set(100);
    let old = f.engine.propose(topical(f, TEXT)).unwrap();
    f.clock.set(200);
    let new = f
        .engine
        .propose(topical(f, "la mascota nueva es un gato"))
        .unwrap();
    f.clock.set(300);
    f.engine.archive(new.memory, f.person).unwrap();
    f.engine
        .revert(
            f.engine.history(new.memory).unwrap().pop().unwrap().id,
            f.person,
        )
        .unwrap();
    (old.memory, new.memory)
}

// spec-clio-memory #11
#[test]
fn t11_purge_removes_content_and_leaves_a_tombstone() {
    let f = fixture();
    let (old, new) = with_history(&f);
    f.clock.set(900);
    let done = f.engine.purge(old, f.person, "pedido de Ana").unwrap();
    assert!(done.wal_truncated);

    // gone from the table and from the index
    assert!(matches!(f.engine.get(old), Err(Error::NotFound)));
    for q in ["tortuguita", "zafiro", "estanque"] {
        let hits = f
            .engine
            .search(
                f.scope,
                q,
                SearchOpts {
                    as_of: Some(150),
                    ..SearchOpts::default()
                },
            )
            .unwrap();
        assert!(hits.is_empty(), "{q}");
    }
    // gone from every operation of the log
    let ops = f.engine.history(old).unwrap();
    assert!(ops.len() >= 3);
    let json = serde_json::to_string(&ops).unwrap();
    assert!(
        !json.contains("tortug") && !json.contains("zafiro"),
        "{json}"
    );
    for op in f.engine.history(new).unwrap() {
        let json = serde_json::to_string(&op).unwrap();
        assert!(!json.contains("zafiro"), "op {} still carries it", op.id.0);
    }
    // and a tombstone, with no content
    let t = f.engine.tombstone(old).unwrap().unwrap();
    assert_eq!(
        (t.memory, t.op, t.actor, t.at, t.reason.as_str()),
        (old, done.op, f.person, 900, "pedido de Ana")
    );
    assert!(f.engine.tombstone(new).unwrap().is_none());
    let last = ops.last().unwrap();
    assert_eq!((last.kind, last.id), (OpKind::Purge, done.op));
    assert!(last.changes.is_empty());
    // the newer memory is untouched and still points back through the chain
    assert_eq!(f.engine.get(new).unwrap().state, State::Active);
    assert!(f.engine.doctor().unwrap().all_ok());
}

// S-24: a PURGE also erases the vector, in the file and in the copy kept in memory
#[test]
fn s24_purge_also_removes_the_vector() {
    let f = fixture_with(Arc::new(HashEmbedder::new("m", 64)));
    let keep = f
        .engine
        .propose(prop(f.scope, f.person, "el gato duerme en la sala"))
        .unwrap();
    let gone = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    f.engine.embed_pending(10).unwrap();
    let ask = RecallOpts::default();
    // warms the in-memory copy of the vectors
    let before = f
        .engine
        .recall(f.scope, "tortuguita zafiro estanque", &ask)
        .unwrap();
    assert!(before.hits.iter().any(|h| h.memory.id == gone.memory));
    let vectors = |m: MemoryId| -> i64 {
        rusqlite::Connection::open(&f.path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM vectors WHERE memory = ?1",
                [m.0],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(vectors(gone.memory), 1);

    f.engine.purge(gone.memory, f.person, "borrar").unwrap();

    assert_eq!(vectors(gone.memory), 0, "the file still has the vector");
    assert_eq!(vectors(keep.memory), 1, "another memory lost its vector");
    let after = f
        .engine
        .recall(f.scope, "tortuguita zafiro estanque", &ask)
        .unwrap();
    assert!(after.hits.iter().all(|h| h.memory.id != gone.memory));
    assert!(
        f.engine.doctor().unwrap().all_ok(),
        "no orphan vector either"
    );
}

// S-25: only a person purges; an agent, a session or an importer gets NotAllowed and nothing changes
#[test]
fn s25_only_a_person_can_purge() {
    let f = fixture();
    let m = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    let session = f
        .engine
        .register_actor("s", ActorKind::Session, Quota::default())
        .unwrap();
    let importer = f
        .engine
        .register_actor("i", ActorKind::Importer, Quota::default())
        .unwrap();
    for who in [f.agent, session, importer] {
        assert!(matches!(
            f.engine.purge(m.memory, who, "x"),
            Err(Error::NotAllowed)
        ));
    }
    assert_eq!(f.engine.get(m.memory).unwrap().content, TEXT);
    assert!(f.engine.tombstone(m.memory).unwrap().is_none());
    assert!(contains(&raw_files(&f), TEXT));
}

#[test]
fn purging_what_does_not_exist_or_twice_is_not_found() {
    let f = fixture();
    let m = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    assert!(matches!(
        f.engine.purge(MemoryId(999), f.person, "x"),
        Err(Error::NotFound)
    ));
    f.engine.purge(m.memory, f.person, "x").unwrap();
    assert!(matches!(
        f.engine.purge(m.memory, f.person, "x"),
        Err(Error::NotFound)
    ));
}

// S-24: not one byte of the text in any table, in the file or in the write-ahead log
#[test]
fn s24_no_byte_of_the_text_is_left_in_any_table_or_file() {
    let f = fixture();
    let (old, _) = with_history(&f);
    f.engine
        .propose(prop(f.scope, f.person, "otra cosa que no se toca"))
        .unwrap();
    // the control: before the PURGE the search does find it, so a miss afterwards means something
    let before = raw_files(&f);
    assert!(contains(&before, TEXT) && contains(&before, WORD));
    assert!(dump_all(&f.path).contains("zafiro"));

    f.engine.purge(old, f.person, "pedido").unwrap();

    let tables = dump_all(&f.path);
    for needle in [TEXT, WORD, "zafiro", "estanque", "violeta"] {
        assert!(!tables.contains(needle), "a table still has {needle:?}");
    }
    let after = raw_files(&f);
    for needle in [TEXT, WORD, "zafiro", "estanque", "violeta"] {
        assert!(
            !contains(&after, needle),
            "the file or its log still has {needle:?}"
        );
    }
    // after a checkpoint of the log, too
    let conn = rusqlite::Connection::open(&f.path).unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(conn);
    assert!(!contains(&raw_files(&f), "zafiro"));
    // what was not purged is still there
    assert!(contains(&after, "otra cosa que no se toca"));
}

// S-24: the decision log that quoted the memory loses the quote
#[test]
fn purge_blanks_the_reason_of_decisions_about_the_memory() {
    let f = fixture();
    let decider = ScriptedDecider::with_reason(
        "d",
        Behavior::Say(Choice::Add, 0.9),
        "se parece a la tortuguita zafiro",
    );
    let f = Fixture {
        engine: f.engine.with_decider(decider, DeciderMode::Active, 0.5),
        ..f
    };
    let m = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    let all = || f.engine.decisions(f.scope, 0, i64::MAX).unwrap();
    assert!(
        all()
            .iter()
            .any(|d| d.reason.as_deref().is_some_and(|r| r.contains("zafiro")))
    );
    f.engine.purge(m.memory, f.person, "x").unwrap();
    assert!(!all().is_empty(), "the audit log keeps the decision itself");
    assert!(all().iter().all(|d| d.reason.is_none()));
    assert!(!dump_all(&f.path).contains("zafiro"));
}

#[test]
fn rebuilding_the_projection_after_a_purge_gives_the_same_state() {
    let f = fixture();
    let (old, new) = with_history(&f);
    let other = f
        .engine
        .propose(prop(f.scope, f.person, "otra cosa"))
        .unwrap();
    f.engine.purge(old, f.person, "x").unwrap();
    let snapshot = |f: &Fixture| {
        let mut rows = f.engine.list(f.scope, None, None).unwrap();
        rows.sort_by_key(|r| r.id.0);
        rows
    };
    let before = snapshot(&f);
    assert!(before.iter().all(|r| r.id != old));
    assert!(before.iter().any(|r| r.id == new) && before.iter().any(|r| r.id == other.memory));

    f.engine.rebuild_projection().unwrap();

    assert_eq!(snapshot(&f), before);
    assert!(matches!(f.engine.get(old), Err(Error::NotFound)));
    assert!(
        f.engine
            .search(f.scope, "zafiro", SearchOpts::default())
            .unwrap()
            .is_empty()
    );
    assert!(f.engine.tombstone(old).unwrap().is_some());
    assert!(f.engine.doctor().unwrap().all_ok());
    assert!(!dump_all(&f.path).contains("zafiro"));
}

#[test]
fn reverting_the_past_cannot_bring_the_purged_content_back() {
    let f = fixture();
    f.clock.set(100);
    let old = f.engine.propose(topical(&f, TEXT)).unwrap();
    f.clock.set(200);
    let new = f
        .engine
        .propose(topical(&f, "la mascota nueva es un gato"))
        .unwrap();
    let purge = f.engine.purge(old.memory, f.person, "x").unwrap();

    // the op that created it, and the op that replaced it (which touched both memories)
    for op in [old.op, new.op] {
        assert!(matches!(
            f.engine.revert(op, f.person),
            Err(Error::NotRevertible(o)) if o == op
        ));
    }
    // and the purge itself cannot be undone
    assert!(matches!(
        f.engine.revert(purge.op, f.person),
        Err(Error::NotRevertible(_))
    ));
    // a bulk revert skips them instead of resurrecting anything
    let report = f.engine.revert_since(old.op, f.person).unwrap();
    assert!(report.skipped.contains(&old.op) && report.skipped.contains(&new.op));
    assert!(matches!(f.engine.get(old.memory), Err(Error::NotFound)));
    assert!(!dump_all(&f.path).contains("zafiro"));
}

#[test]
fn two_memories_with_the_same_text_survive_each_others_purge() {
    let f = fixture_with(Arc::new(HashEmbedder::new("m", 64)));
    let other = f.engine.create_scope("otro", "es").unwrap();
    let a = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    let b = f.engine.propose(prop(other, f.person, TEXT)).unwrap();
    assert_eq!(
        f.engine.get(a.memory).unwrap().content_hash,
        f.engine.get(b.memory).unwrap().content_hash
    );
    f.engine.embed_pending(10).unwrap();

    f.engine.purge(a.memory, f.person, "x").unwrap();

    let b_row = f.engine.get(b.memory).unwrap();
    assert_eq!((b_row.content.as_str(), b_row.state), (TEXT, State::Active));
    let hits = f
        .engine
        .search(other, "tortuguita", SearchOpts::default())
        .unwrap();
    assert_eq!(hits.len(), 1);
    let recall = f
        .engine
        .recall(other, "tortuguita zafiro estanque", &RecallOpts::default())
        .unwrap();
    assert_eq!(recall.hits.len(), 1, "the other memory keeps its vector");
    assert!(f.engine.doctor().unwrap().all_ok());
    // purging the second one finishes the job
    f.engine.purge(b.memory, f.person, "x").unwrap();
    assert!(!contains(&raw_files(&f), "zafiro"));
}

#[test]
fn the_chain_around_a_purged_memory_stays_coherent() {
    let f = fixture();
    f.clock.set(100);
    let a = f.engine.propose(topical(&f, "mascota uno")).unwrap();
    f.clock.set(200);
    let b = f.engine.propose(topical(&f, TEXT)).unwrap();
    f.clock.set(300);
    let c = f.engine.propose(topical(&f, "mascota tres")).unwrap();
    // purge the middle link: a -> b -> c
    f.engine.purge(b.memory, f.person, "x").unwrap();
    let a_row = f.engine.get(a.memory).unwrap();
    assert_eq!(
        (a_row.state, a_row.replaced_by, a_row.content.as_str()),
        (State::Replaced, Some(b.memory), "mascota uno")
    );
    assert_eq!(f.engine.get(c.memory).unwrap().state, State::Active);
    // what was true before the purged one still answers; the rest of the chain is intact
    let then = f
        .engine
        .search(
            f.scope,
            "mascota",
            SearchOpts {
                as_of: Some(150),
                ..SearchOpts::default()
            },
        )
        .unwrap();
    assert_eq!(then.len(), 1);
    assert_eq!(then[0].memory.id, a.memory);
    f.engine.rebuild_projection().unwrap();
    assert_eq!(f.engine.get(a.memory).unwrap().replaced_by, Some(b.memory));
    assert!(f.engine.doctor().unwrap().all_ok());
}

#[test]
fn a_purged_memory_is_gone_from_blocks_usage_and_feedback() {
    let f = fixture();
    let keep = f
        .engine
        .propose(prop(f.scope, f.person, "dato fijo"))
        .unwrap();
    let m = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    let block = f
        .engine
        .create_block(f.scope, "identidad", 100, f.person)
        .unwrap();
    f.engine
        .block_set(block, &[keep.memory, m.memory], f.person)
        .unwrap();
    f.engine
        .feedback(m.memory, Signal::Useful, f.person)
        .unwrap();
    f.engine.purge(m.memory, f.person, "x").unwrap();
    assert_eq!(f.engine.block_get(block).unwrap(), vec![keep.memory]);
    assert!(f.engine.doctor().unwrap().all_ok());
    assert!(matches!(
        f.engine.feedback(m.memory, Signal::Useful, f.person),
        Err(Error::NotFound)
    ));
}

// S-24: what leaves the file after a PURGE has no content, and a restore keeps it that way
#[test]
fn export_and_import_respect_the_tombstone() {
    let f = fixture();
    let (old, new) = with_history(&f);
    let before_purge = exported(&f);
    assert!(before_purge.contains("zafiro"));
    f.engine.purge(old, f.person, "pedido").unwrap();

    let text = exported(&f);
    assert!(
        !text.contains("zafiro") && !text.contains("tortug"),
        "the export leaks"
    );
    assert!(text.contains("\"Purge\""));

    let (summary, dir, restored) = import_text(&text);
    summary.unwrap();
    assert!(matches!(restored.get(old), Err(Error::NotFound)));
    assert_eq!(restored.tombstone(old).unwrap().unwrap().reason, "pedido");
    assert_eq!(
        restored.get(new).unwrap().state,
        f.engine.get(new).unwrap().state
    );
    assert!(restored.doctor().unwrap().all_ok());
    assert!(!dump_all(&dir.path().join("restored.db")).contains("zafiro"));
    // a purge that was already done stays done after a rebuild of the restored file
    restored.rebuild_projection().unwrap();
    assert!(matches!(restored.get(old), Err(Error::NotFound)));
}

fn lines_of(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn joined(lines: &[serde_json::Value]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

// S-24: a file that says "purged" but still carries the content, or that brings the memory back, is refused
#[test]
fn import_refuses_a_file_that_contradicts_a_tombstone() {
    let f = fixture();
    let m = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    let old_export = exported(&f); // before: carries the text
    f.engine.purge(m.memory, f.person, "pedido").unwrap();
    let new_export = exported(&f);

    // an old export plus the purge operation: the old lines still hold the text
    let purge_line = lines_of(&new_export)
        .into_iter()
        .find(|l| l["kind"] == "Purge")
        .unwrap();
    let mut doctored = lines_of(&old_export);
    let at = doctored
        .iter()
        .position(|l| l["type"] == "operation")
        .unwrap();
    doctored.insert(at + 1, purge_line.clone());
    let (r, _dir, _e) = import_text(&joined(&doctored));
    let msg = r.unwrap_err().to_string();
    assert!(msg.contains("was purged") && msg.contains("line"), "{msg}");

    // a file with an operation after the purge that touches the memory
    let mut revived = lines_of(&new_export);
    let mut again = lines_of(&old_export)
        .into_iter()
        .find(|l| l["type"] == "operation")
        .unwrap();
    again["id"] = serde_json::json!(999);
    revived.push(again);
    let (r, _dir, engine) = import_text(&joined(&revived));
    let msg = r.unwrap_err().to_string();
    assert!(msg.contains("was purged") && msg.contains("line"), "{msg}");
    assert!(
        engine.scopes().unwrap().is_empty(),
        "a refused file changes nothing"
    );

    // usage of a purged memory
    let mut with_usage = lines_of(&new_export);
    with_usage.push(
        serde_json::json!({"type": "usage", "memory": m.memory.0, "uses": 1, "last_used": 1}),
    );
    let (r, _dir, _e) = import_text(&joined(&with_usage));
    assert!(r.unwrap_err().to_string().contains("was purged"));
}

#[test]
fn a_backup_made_before_the_purge_still_has_the_text() {
    // Honest limit, written down so nobody assumes otherwise: PURGE cannot reach copies that
    // already left the file.
    let f = fixture();
    let m = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    let copy = f._dir.path().join("backup.db");
    f.engine.enqueue_backup(&copy).unwrap();
    f.engine
        .run_jobs(&RunOpts {
            allow_heavy: true,
            only: Some(JobKind::Backup),
            ..RunOpts::default()
        })
        .unwrap();
    f.engine.purge(m.memory, f.person, "x").unwrap();
    assert!(dump_all(&copy).contains("zafiro"));
    assert!(!dump_all(&f.path).contains("zafiro"));
}

// S-24: a file written before PURGE existed (no secure_delete) has old copies of the text in
// free space; the PURGE rewrites the file, so they go too
#[test]
fn s24_purge_cleans_stale_copies_left_by_older_writes() {
    let f = fixture();
    let m = f.engine.propose(prop(f.scope, f.person, TEXT)).unwrap();
    let (path, person) = (f.path.clone(), f.person);
    drop(f.engine);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "secure_delete", false).unwrap();
    // the old text is freed inside the page, not erased
    for text in [format!("{TEXT}, versión dos"), "otro texto".to_string()] {
        conn.execute(
            "UPDATE memories SET content = ?2 WHERE id = ?1",
            rusqlite::params![m.memory.0, text],
        )
        .unwrap();
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(conn);
    assert!(
        contains(&std::fs::read(&path).unwrap(), "zafiro"),
        "the setup should leave a stale copy"
    );

    let engine = Engine::open(&path, OpenMode::ReadWrite, Arc::new(FixedClock::new(2000))).unwrap();
    engine.purge(m.memory, person, "x").unwrap();
    drop(engine);
    assert!(!contains(&std::fs::read(&path).unwrap(), "zafiro"));
}
