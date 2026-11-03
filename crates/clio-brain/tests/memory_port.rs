#![allow(clippy::unwrap_used)]
//! `LinkPort`: the tool host's memory over a real database (read only). Memories of every state,
//! origin and scope; nothing here may leak which of "missing", "quarantined", "archived" or
//! "another scope's" a memory is, or write a byte.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use clio_brain::memory_link::{EmbedModel, MemoryLink};
use clio_brain::memory_port::LinkPort;
use clio_memory::{MemoryType, Origin as MOrigin, Proposal, ScopeId, State};
use clio_toolhost::{MemoryPort, Origin, ProposalKind};
use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");
const WORD: &str = "xilofono";

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    World {
        data: root.join("data"),
        root,
        _tmp: tmp,
    }
}

fn open(w: &World) -> MemoryLink {
    MemoryLink::open_write(&w.data, &EmbedModel::none()).unwrap()
}

/// Proposes straight through the engine: the only way to get an agent's active memory, a
/// quarantined one and one of another scope into the file.
fn propose(link: &MemoryLink, scope: ScopeId, origin: MOrigin, text: &str) -> i64 {
    let a = link.actors();
    let actor = if origin == MOrigin::User {
        a.persona
    } else {
        a.sesion
    };
    link.engine()
        .propose(Proposal {
            scope,
            actor,
            kind: MemoryType::Semantic,
            content: text.into(),
            topic_key: None,
            origin,
            session: None,
            episode: None,
            source: None,
            valid_from: None,
            valid_to: None,
        })
        .unwrap()
        .memory
        .0
}

struct Ids {
    person: i64,
    agent: i64,
    quarantine: i64,
    archived: i64,
    foreign: i64,
}

/// One memory of each kind, all of them mentioning `WORD`.
fn seed(w: &World) -> Ids {
    let link = open(w);
    let person = link
        .remember(&format!("la persona prefiere el {WORD} de madera"), None)
        .unwrap()
        .0;
    let archived = link
        .remember(&format!("el {WORD} viejo ya no se usa"), None)
        .unwrap();
    link.archive(archived).unwrap();
    let agent = propose(
        &link,
        link.scope(),
        MOrigin::Agent,
        &format!("el agente terminó de afinar el {WORD}"),
    );
    let quarantine = propose(
        &link,
        link.scope(),
        MOrigin::External,
        &format!("un correo dice que el {WORD} es de la persona"),
    );
    let other = link.engine().create_scope("otro", "es").unwrap();
    let foreign = propose(
        &link,
        other,
        MOrigin::User,
        &format!("el {WORD} de otro ámbito"),
    );
    Ids {
        person,
        agent,
        quarantine,
        archived: archived.0,
        foreign,
    }
}

fn port(w: &World) -> LinkPort {
    LinkPort::new(&w.data, None)
}

/// Runs `f` off this thread: a call that hangs fails the test instead of the whole suite.
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs))
        .expect("the call did not finish in time")
}

// S-60
#[test]
fn search_finds_only_active_memories_and_labels_origins() {
    let w = world();
    let ids = seed(&w);
    let found = port(&w).search(WORD, 8).unwrap();
    let mut got: Vec<(i64, Origin)> = found.items.iter().map(|i| (i.id, i.origin)).collect();
    got.sort_by_key(|(id, _)| *id);
    let mut want = vec![(ids.person, Origin::Person), (ids.agent, Origin::Agent)];
    want.sort_by_key(|(id, _)| *id);
    assert_eq!(got, want, "{found:?}");
    for hidden in [ids.quarantine, ids.archived, ids.foreign] {
        assert!(found.items.iter().all(|i| i.id != hidden), "{hidden}");
    }
    let person = found.items.iter().find(|i| i.id == ids.person).unwrap();
    assert_eq!(person.kind, "semántico");
    assert!(person.text.contains(WORD));
    // A nonsense query is an answer, not an error; and the abstention is the memory's.
    let nothing = port(&w).search("zzqqxx", 5).unwrap();
    assert!(nothing.items.is_empty() && nothing.abstained, "{nothing:?}");
    // No vectors in this test: the model is told it is a text-only search.
    assert!(
        found
            .note
            .as_deref()
            .is_some_and(|n| n.contains("solo texto"))
    );
}

// S-60
#[test]
fn get_gives_one_error_for_missing_quarantined_archived_and_foreign() {
    let w = world();
    let ids = seed(&w);
    let p = port(&w);
    let errs: Vec<String> = [9999, ids.quarantine, ids.archived, ids.foreign]
        .into_iter()
        .map(|id| p.get(id).unwrap_err())
        .collect();
    assert!(errs.iter().all(|e| e == &errs[0]), "{errs:?}");
    assert!(errs[0].len() < 100, "{errs:?}");
    // The active ones do come back, with a date and the Spanish word for the kind.
    let item = p.get(ids.person).unwrap();
    assert_eq!((item.id, item.origin), (ids.person, Origin::Person));
    assert_eq!(item.kind, "semántico");
    let b = item.date.as_bytes();
    assert!(
        item.date.len() == 10 && b[4] == b'-' && b[7] == b'-' && item.date.starts_with("20"),
        "{}",
        item.date
    );
    assert_eq!(p.get(ids.agent).unwrap().origin, Origin::Agent);
}

// S-08
#[test]
fn a_search_that_looks_like_syntax_is_just_text() {
    let w = world();
    seed(&w);
    let p = Arc::new(port(&w));
    let long = "a".repeat(50_000);
    for q in ["\"", "\" OR *", "NEAR(", "😀 ✨", "( ) AND NOT", &long] {
        let (p, q) = (Arc::clone(&p), q.to_owned());
        let r = within(20, move || p.search(&q, 5));
        if let Err(e) = r {
            assert!(e.len() < 200, "{e}");
        }
    }
}

// S-60
#[test]
fn a_missing_empty_or_corrupt_file_is_a_short_error_and_recovers() {
    let w = world();
    let p = port(&w);
    let path = MemoryLink::db_path(&w.data);
    let shown = w.root.display().to_string();
    let check = |why: &str| {
        for e in [p.search(WORD, 5).unwrap_err(), p.get(1).unwrap_err()] {
            assert!(e.len() < 200, "{why}: {e}");
            assert!(!e.contains(&shown), "{why}: the path leaked: {e}");
        }
    };
    // No file at all.
    assert!(p.search(WORD, 5).unwrap_err().contains("todavía"));
    check("missing");
    // Empty, then garbage.
    std::fs::create_dir_all(&w.data).unwrap();
    for (why, bytes) in [
        ("empty", &b""[..]),
        ("garbage", &b"esto no es una base"[..]),
    ] {
        std::fs::write(&path, bytes).unwrap();
        check(why);
        std::fs::remove_file(&path).unwrap();
    }
    // The failure was not kept: once there is a database, the next call works.
    let ids = seed(&w);
    assert_eq!(p.get(ids.person).unwrap().id, ids.person);
    assert!(!p.search(WORD, 5).unwrap().items.is_empty());
}

// S-60
#[test]
fn the_person_can_write_while_the_agent_reads() {
    let w = world();
    seed(&w);
    let p = Arc::new(port(&w));
    let start = Arc::new(Barrier::new(2));
    let (data, b) = (w.data.clone(), Arc::clone(&start));
    let writer = std::thread::spawn(move || {
        b.wait();
        for i in 0..25 {
            let link = MemoryLink::open_write(&data, &EmbedModel::none()).unwrap();
            link.remember(&format!("recuerdo nuevo número {i} de la persona"), None)
                .unwrap();
        }
    });
    start.wait();
    for _ in 0..50 {
        let t = Instant::now();
        p.search(WORD, 5).unwrap();
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }
    writer.join().unwrap();
    // What the person wrote after the port opened is seen by the next search.
    let link = open(&w);
    link.remember("el ultimo dato es el ornitorrinco", None)
        .unwrap();
    drop(link);
    assert!(!p.search("ornitorrinco", 5).unwrap().items.is_empty());
}

/// Name, size, mtime and bytes of what the person's memory is made of. SQLite's `-shm`/`-wal`
/// are the reader's own bookkeeping (any reader makes them, `clio memory list` too): they are
/// checked apart, below.
fn fingerprint(dir: &Path) -> Vec<(String, std::time::SystemTime, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            !n.ends_with("-shm") && !n.ends_with("-wal")
        })
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.metadata().unwrap().modified().unwrap(),
                std::fs::read(e.path()).unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

// S-60
#[test]
fn the_port_never_writes() {
    let w = world();
    let ids = seed(&w);
    let before = fingerprint(&w.data);
    let p = port(&w);
    for _ in 0..3 {
        p.search(WORD, 5).unwrap();
        p.get(ids.person).unwrap();
        let _ = p.get(ids.quarantine);
    }
    assert!(
        fingerprint(&w.data) == before,
        "the port changed the person's memory"
    );
    // No page was ever written to the log of changes.
    let wal = w.data.join("memory.db-wal");
    assert!(!wal.exists() || std::fs::metadata(&wal).unwrap().len() == 0);
}

// ---- the real service

struct Host {
    child: Child,
    conn: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Host {
    /// `clio toolhost` with the memory flags given; `None` for the loose binary's behaviour.
    fn start(w: &World, memory: &[&str]) -> Self {
        let sock = w.root.join(format!("sock{}", memory.len()));
        let (work, scratch, run) = (
            w.root.join("work"),
            w.root.join("scratch"),
            w.root.join("run"),
        );
        for d in [&sock, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut child = Command::new(CLIO)
            .arg("toolhost")
            .args(["--socket-dir"])
            .arg(&sock)
            .arg("--work")
            .arg(&work)
            .arg("--scratch")
            .arg(&scratch)
            .arg("--run-dir")
            .arg(&run)
            .args(["--jail", "off"])
            .args(memory)
            .env_clear()
            .env("HOME", &w.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let line = within(30, move || {
            let mut l = String::new();
            BufReader::new(out).read_line(&mut l).unwrap();
            l
        });
        let Some(path) = line.trim().strip_prefix("READY ") else {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the service did not say READY: {line:?}");
        };
        let conn = UnixStream::connect(path).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let reader = BufReader::new(conn.try_clone().unwrap());
        Self {
            child,
            conn,
            reader,
        }
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let req = json!({"id": "i", "tool": tool, "args": args, "session": "s", "call_id": "c"});
        writeln!(self.conn, "{req}").unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // Closing stdin is how the service is told to stop; the kill is the net.
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// S-60
#[test]
fn the_real_service_serves_the_memory_only_when_it_was_given_one() {
    let w = world();
    let ids = seed(&w);
    let data = w.data.display().to_string();
    {
        let mut h = Host::start(&w, &["--memory-data", &data]);
        let r = h.call("memory_search", json!({ "query": WORD }));
        assert_eq!(r["ok"], true, "{r}");
        let text = r["content"].as_str().unwrap();
        assert!(
            text.contains(&format!("[m{}] semántico · persona", ids.person)),
            "{text}"
        );
        assert!(text.contains("solo texto"), "{text}");
        assert!(!text.contains(&format!("[m{}]", ids.quarantine)), "{text}");
        let g = h.call("memory_get", json!({ "id": ids.agent }));
        assert_eq!(g["ok"], true, "{g}");
        let e = h.call("memory_get", json!({ "id": ids.quarantine }));
        assert_eq!(e["ok"], false, "{e}");
        assert!(
            e["content"]
                .as_str()
                .unwrap()
                .contains("no existe o no se puede leer"),
            "{e}"
        );
    }
    // A model folder that is not a model: the search still works, by text.
    let bad = w.root.join("not-a-model");
    std::fs::create_dir_all(&bad).unwrap();
    {
        let mut h = Host::start(
            &w,
            &[
                "--memory-data",
                &data,
                "--memory-model",
                &bad.display().to_string(),
            ],
        );
        let r = h.call("memory_search", json!({ "query": WORD }));
        assert_eq!(r["ok"], true, "{r}");
        assert!(r["content"].as_str().unwrap().contains("solo texto"), "{r}");
    }
    // Without `--memory-data` the operation is refused, as with the loose binary.
    let mut h = Host::start(&w, &[]);
    let r = h.call("memory_search", json!({ "query": WORD }));
    assert_eq!(r["ok"], false, "{r}");
    assert_eq!(r["decision"]["outcome"], "deny", "{r}");
}

// ---- proposals (stage 2) ----

// S-61
#[test]
fn a_proposal_lands_in_quarantine_as_the_agents_and_from_outside() {
    let w = world();
    seed(&w);
    let p = port(&w);
    let mut ids = Vec::new();
    for (text, kind, want) in [
        (
            "a la persona le gusta el té verde",
            ProposalKind::Semantic,
            MemoryType::Semantic,
        ),
        (
            "para desplegar corre primero las pruebas",
            ProposalKind::Procedural,
            MemoryType::Procedural,
        ),
    ] {
        let id = p.propose(text, kind, "sess-1").unwrap();
        let link = open(&w);
        let row = link.get(clio_memory::MemoryId(id)).unwrap();
        assert_eq!(row.state, State::Quarantine, "{text}");
        assert_eq!(row.origin, MOrigin::External, "{text}");
        assert_eq!(row.kind, want, "{text}");
        assert_eq!(row.actor, link.actors().agente, "{text}");
        assert_eq!(row.scope, link.scope(), "{text}");
        assert_eq!(row.source.as_deref(), Some("pi:sess-1"), "{text}");
        ids.push(id);
    }
    // What was proposed is not memory: it is not found and cannot be read until the person promotes.
    assert!(p.search("té verde", 5).unwrap().items.is_empty());
    assert!(p.get(ids[0]).is_err());
    let link = open(&w);
    assert!(link.pending().unwrap().iter().any(|r| r.id.0 == ids[0]));
    // Only the person promotes; then it is readable.
    link.promote(clio_memory::MemoryId(ids[0])).unwrap();
    assert!(!p.search("té verde", 5).unwrap().items.is_empty());
}

// S-61
#[test]
fn a_proposal_cleans_what_is_recorded_about_the_session() {
    let w = world();
    let p = port(&w);
    let id = p
        .propose("un dato", ProposalKind::Semantic, "s\u{1b}[31m1\nfalso")
        .unwrap();
    let row = open(&w).get(clio_memory::MemoryId(id)).unwrap();
    for field in [row.session.as_deref(), row.source.as_deref()] {
        let f = field.unwrap();
        assert!(!f.contains(['\u{1b}', '\n']), "{f:?}");
    }
}

// S-61
#[test]
fn a_proposal_does_not_keep_the_writer_lock() {
    let w = world();
    let p = port(&w);
    p.propose("un dato", ProposalKind::Semantic, "s").unwrap();
    // `open_write` waits two seconds for a held lock and then fails: it must not wait at all.
    let data = w.data.clone();
    let t = Instant::now();
    within(10, move || {
        MemoryLink::open_write(&data, &EmbedModel::none()).unwrap();
    });
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

// S-61
#[test]
fn the_person_and_the_agent_can_both_write() {
    let w = world();
    seed(&w);
    let p = Arc::new(port(&w));
    let start = Arc::new(Barrier::new(2));
    let (data, b) = (w.data.clone(), Arc::clone(&start));
    let person = std::thread::spawn(move || {
        b.wait();
        for i in 0..15 {
            let link = MemoryLink::open_write(&data, &EmbedModel::none()).unwrap();
            link.remember(&format!("dato número {i} de la persona"), None)
                .unwrap();
        }
    });
    start.wait();
    for i in 0..15 {
        p.propose(
            &format!("propuesta número {i} del agente"),
            ProposalKind::Semantic,
            "s",
        )
        .unwrap();
    }
    person.join().unwrap();
    let link = open(&w);
    assert_eq!(
        link.pending().unwrap().len(),
        15 + 1,
        "the seeded quarantine one too"
    );
}

// S-27, S-61
#[test]
fn the_engines_quota_and_secret_check_still_apply_with_fixed_messages() {
    let w = world();
    let p = port(&w);
    for i in 0..20 {
        p.propose(&format!("dato distinto {i}"), ProposalKind::Semantic, "s")
            .unwrap();
    }
    let marker = "frase-del-modelo-4714";
    let why = p.propose(marker, ProposalKind::Semantic, "s").unwrap_err();
    assert!(why.contains("cuota"), "{why}");
    assert!(!why.contains(marker), "{why}");

    let w = world();
    let p = port(&w);
    let key = concat!("sk-", "proj-0123456789abcdefghijklmnopqrstuv");
    let why = p
        .propose(&format!("la clave es {key}"), ProposalKind::Semantic, "s")
        .unwrap_err();
    assert!(!why.contains(key), "{why}");
    assert!(open(&w).pending().unwrap().is_empty());
}

// S-61
#[test]
fn a_proposal_that_cannot_open_the_memory_is_a_short_error() {
    let w = world();
    std::fs::create_dir_all(&w.data).unwrap();
    std::fs::write(w.data.join("memory.db"), "no es una base de datos").unwrap();
    let p = port(&w);
    let why = p
        .propose("un dato", ProposalKind::Semantic, "s")
        .unwrap_err();
    assert!(!why.contains(w.root.to_str().unwrap()), "{why}");
}
