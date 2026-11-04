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
            title: None,
            category: None,
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

/// A proposal with no title or category.
fn draft(text: &str, kind: ProposalKind) -> clio_toolhost::ProposalDraft {
    clio_toolhost::ProposalDraft {
        text: text.to_owned(),
        title: None,
        category: None,
        kind,
    }
}

/// What these tests need of a proposal: the id, with no judge and nothing known about the session.
trait ProposeId {
    fn propose_id(&self, text: &str, kind: ProposalKind, session: &str) -> Result<i64, String>;
}

impl ProposeId for clio_brain::memory_port::LinkPort {
    fn propose_id(&self, text: &str, kind: ProposalKind, session: &str) -> Result<i64, String> {
        MemoryPort::propose(
            self,
            &draft(text, kind),
            session,
            &clio_toolhost::Judging::default(),
        )
        .map(|d| d.id)
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
        let id = p.propose_id(text, kind, "sess-1").unwrap();
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
        .propose_id("un dato", ProposalKind::Semantic, "s\u{1b}[31m1\nfalso")
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
    p.propose_id("un dato", ProposalKind::Semantic, "s")
        .unwrap();
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
        p.propose_id(
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
        p.propose_id(&format!("dato distinto {i}"), ProposalKind::Semantic, "s")
            .unwrap();
    }
    let marker = "frase-del-modelo-4714";
    let why = p
        .propose_id(marker, ProposalKind::Semantic, "s")
        .unwrap_err();
    assert!(why.contains("cuota"), "{why}");
    assert!(!why.contains(marker), "{why}");

    let w = world();
    let p = port(&w);
    let key = concat!("sk-", "proj-0123456789abcdefghijklmnopqrstuv");
    let why = p
        .propose_id(&format!("la clave es {key}"), ProposalKind::Semantic, "s")
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
        .propose_id("un dato", ProposalKind::Semantic, "s")
        .unwrap_err();
    assert!(!why.contains(w.root.to_str().unwrap()), "{why}");
}

// ---- proposals decided by Jev (spec 2026-11-03-jev-design.md, entrega 3) ----

use clio_brain::memory_judge::MemoryJudge;
use clio_jev::{Config as JevConfig, Jev, Transport};
use clio_toolhost::{Judging, Saved};

/// A Jev that picks `choice` (`update_first` and `noop_first` mean the first such option offered)
/// with `confidence`, and says `said` to «the person said it».
struct Scripted {
    choice: String,
    confidence: f64,
    said: f64,
}

impl Transport for Scripted {
    fn call(&self, _k: &str, body: &Value, _t: Duration) -> Result<Value, String> {
        let q = body["questions"].as_object().unwrap();
        let mut answers = serde_json::Map::new();
        if let Some(op) = q.get("operacion") {
            let want = self.choice.strip_suffix("_first");
            let choice = match want {
                Some(prefix) => op["criteria"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .find(|k| k.starts_with(prefix))
                    .cloned()
                    .unwrap_or_else(|| "review".into()),
                None => self.choice.clone(),
            };
            answers.insert(
                "operacion".into(),
                json!({"type": "choice", "choice": choice, "confidence": self.confidence}),
            );
        }
        for name in [
            "la_persona_lo_dijo",
            "pide_olvidar",
            "pide_guardar",
            "pide_deshacer",
        ] {
            if q.contains_key(name) {
                answers.insert(name.into(), json!({"type": "noul", "noul": self.said}));
            }
        }
        Ok(json!({"model": "jev-t", "answers": answers, "usage": {"input_tokens": 10}}))
    }
}

fn judged_port(w: &World, choice: &str, confidence: f64, said: f64) -> LinkPort {
    let jev = Arc::new(Jev::new(
        JevConfig::default(),
        Box::new(Scripted {
            choice: choice.into(),
            confidence,
            said,
        }),
        Box::new(|| Ok("KEY".into())),
        Arc::new(clio_core::SystemClock),
    ));
    let judge = MemoryJudge::new(jev, Arc::new(|s| Ok(s.to_owned())));
    LinkPort::new(&w.data, None).with_judge(Arc::new(judge))
}

fn ask(p: &LinkPort, text: &str, kind: ProposalKind, said: &[&str]) -> clio_toolhost::Proposed {
    let judging = Judging {
        tainted: false,
        person_said: said.iter().map(|s| (*s).to_owned()).collect(),
        session: "sess-1".into(),
    };
    MemoryPort::propose(p, &draft(text, kind), "sess-1", &judging).unwrap()
}

// S-25, S-61: the judge turns a proposal into memory; the author, the origin and the signature say
// who did what, and it reads like any memory of the agent (so reading it taints a session).
// S-64
#[test]
fn a_proposal_the_judge_calls_new_is_memory_now_signed_by_the_judge() {
    let w = world();
    let p = judged_port(&w, "add", 0.95, 0.0);
    let d = ask(
        &p,
        "El proyecto usa PostgreSQL 16",
        ProposalKind::Semantic,
        &[],
    );
    assert_eq!(d.saved, Saved::Active);
    let link = open(&w);
    let row = link.get(clio_memory::MemoryId(d.id)).unwrap();
    assert_eq!((row.state, row.origin), (State::Active, MOrigin::External));
    assert_eq!(
        row.actor,
        link.actors().agente,
        "the author is still the agent"
    );
    let op = &link.engine().history(row.id).unwrap()[0];
    assert_ne!(op.actor, link.actors().agente, "the judge signed it");
    // It is found and readable by the model like any active memory.
    assert!(!p.search("PostgreSQL", 5).unwrap().items.is_empty());
    assert!(p.get(d.id).is_ok());
}

#[test]
fn a_proposal_the_judge_says_replaces_one_replaces_it_and_a_repeat_is_known() {
    let w = world();
    let p = judged_port(&w, "add", 0.95, 0.0);
    let first = ask(
        &p,
        "El proyecto usa PostgreSQL 16",
        ProposalKind::Semantic,
        &[],
    );
    let replace = judged_port(&w, "update_first", 0.95, 0.0);
    let d = ask(
        &replace,
        "El proyecto ahora usa PostgreSQL 17",
        ProposalKind::Semantic,
        &[],
    );
    assert_eq!(d.saved, Saved::Replaced);
    // (Not kept open: an open writer would lock the next call out.)
    assert_eq!(
        open(&w).get(clio_memory::MemoryId(first.id)).unwrap().state,
        State::Replaced
    );
    let known = judged_port(&w, "noop_first", 0.95, 0.0);
    let again = ask(
        &known,
        "Se usa PostgreSQL 17 en el proyecto",
        ProposalKind::Semantic,
        &[],
    );
    assert_eq!((again.saved, again.id), (Saved::Known, d.id));
    assert_eq!(open(&w).list(State::Active).unwrap().len(), 1);
}

#[test]
fn when_jev_is_not_sure_the_proposal_waits_for_the_person_as_before() {
    let w = world();
    let p = judged_port(&w, "add", 0.5, 0.0);
    let d = ask(&p, "Algo dudoso", ProposalKind::Semantic, &[]);
    assert_eq!(d.saved, Saved::Waiting);
    let row = open(&w).get(clio_memory::MemoryId(d.id)).unwrap();
    assert_eq!(row.state, State::Quarantine);
    assert!(
        p.get(d.id).is_err(),
        "not readable until the person promotes"
    );
}

// S-26, S-64
#[test]
fn a_procedure_needs_more_confidence_than_a_fact() {
    let w = world();
    let p = judged_port(&w, "add", 0.85, 0.0);
    assert_eq!(
        ask(&p, "El hecho nuevo", ProposalKind::Semantic, &[]).saved,
        Saved::Active
    );
    assert_eq!(
        ask(
            &p,
            "Para desplegar, corre deploy.sh primero",
            ProposalKind::Procedural,
            &[]
        )
        .saved,
        Saved::Waiting,
        "0.85 is not enough for something that will act as an instruction"
    );
    let sure = judged_port(&w, "add", 0.95, 0.0);
    assert_eq!(
        ask(
            &sure,
            "Para desplegar, corre deploy.sh primero",
            ProposalKind::Procedural,
            &[]
        )
        .saved,
        Saved::Active
    );
}

// S-25, S-27: what the person wrote is replaced only with the person's own words.
// S-64
#[test]
fn what_the_person_wrote_is_replaced_only_when_their_own_words_say_the_new_fact() {
    let w = world();
    let mine = open(&w).remember("Marta vive en Medellín", None).unwrap();
    // No words of the person in the session: the proposal waits, the memory stays.
    let p = judged_port(&w, "update_first", 0.95, 0.99);
    let d = ask(
        &p,
        "Marta ahora vive en Bogotá",
        ProposalKind::Semantic,
        &[],
    );
    assert_eq!(d.saved, Saved::Waiting);
    assert_eq!(open(&w).get(mine).unwrap().state, State::Active);
    // The person did say it, and Jev agrees: replaced.
    let d = ask(
        &p,
        "Marta ahora vive en Cali",
        ProposalKind::Semantic,
        &["me mudé a Cali"],
    );
    assert_eq!(d.saved, Saved::Replaced);
    assert_eq!(open(&w).get(mine).unwrap().state, State::Replaced);
    // The person's words do not say it: it waits.
    let w2 = world();
    let mine2 = open(&w2).remember("Marta vive en Medellín", None).unwrap();
    let doubtful = judged_port(&w2, "update_first", 0.95, 0.1);
    let d = ask(
        &doubtful,
        "Marta ahora vive en Cali",
        ProposalKind::Semantic,
        &["hola"],
    );
    assert_eq!(d.saved, Saved::Waiting);
    assert_eq!(open(&w2).get(mine2).unwrap().state, State::Active);
}

#[test]
fn the_engines_checks_still_come_first() {
    let w = world();
    let p = judged_port(&w, "add", 0.99, 0.0);
    let key = format!("{}{}", "sk-", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6");
    let e = MemoryPort::propose(
        &p,
        &draft(&format!("mi clave es {key}"), ProposalKind::Semantic),
        "s",
        &Judging::default(),
    )
    .unwrap_err();
    assert!(e.contains("secreto") || e.contains("clave"), "{e}");
    assert!(open(&w).list(State::Active).unwrap().is_empty());
}

// While Jev thinks the writer is not held: the person's own `clio memory remember` goes through.
#[test]
fn the_writer_is_not_held_while_jev_answers() {
    use std::sync::Mutex;
    struct Slow(Mutex<mpsc::Sender<()>>);
    impl Transport for Slow {
        fn call(&self, _k: &str, body: &Value, _t: Duration) -> Result<Value, String> {
            let _ = self.0.lock().unwrap().send(());
            std::thread::sleep(Duration::from_millis(1500));
            let q = body["questions"].as_object().unwrap();
            let name = q.keys().next().unwrap().clone();
            Ok(json!({"model": "jev-t", "usage": {"input_tokens": 1},
                "answers": {name: {"type": "choice", "choice": "add", "confidence": 0.95}}}))
        }
    }
    let w = world();
    let (tx, rx) = mpsc::channel();
    let jev = Arc::new(Jev::new(
        JevConfig::default(),
        Box::new(Slow(Mutex::new(tx))),
        Box::new(|| Ok("KEY".into())),
        Arc::new(clio_core::SystemClock),
    ));
    let judge = MemoryJudge::new(jev, Arc::new(|s| Ok(s.to_owned())));
    let p = Arc::new(LinkPort::new(&w.data, None).with_judge(Arc::new(judge)));
    // The database has to exist before the reader looks at it.
    drop(open(&w));
    let q = p.clone();
    let proposing = std::thread::spawn(move || {
        MemoryPort::propose(
            &*q,
            &draft("El proyecto usa PostgreSQL 16", ProposalKind::Semantic),
            "s",
            &Judging::default(),
        )
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("Jev was asked");
    let t = Instant::now();
    let mine = open(&w).remember("Marta vive en Medellín", None);
    assert!(
        mine.is_ok() && t.elapsed() < Duration::from_millis(1200),
        "{mine:?} {:?}",
        t.elapsed()
    );
    assert_eq!(proposing.join().unwrap().unwrap().saved, Saved::Active);
}

// ---- changes the person asks for through the model (entrega 4) ----

fn said(words: &[&str], tainted: bool) -> Judging {
    Judging {
        tainted,
        person_said: words.iter().map(|w| (*w).to_owned()).collect(),
        session: "s".into(),
    }
}

/// A port whose Jev says yes to everything and counts how often it was asked.
fn counting_port(w: &World) -> (LinkPort, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counting(Arc<AtomicUsize>);
    impl Transport for Counting {
        fn call(&self, _k: &str, body: &Value, _t: Duration) -> Result<Value, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let name = body["questions"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            Ok(json!({"model": "jev-t", "usage": {"input_tokens": 1},
                "answers": {name: {"type": "noul", "noul": 0.99}}}))
        }
    }
    let n = Arc::new(AtomicUsize::new(0));
    let jev = Arc::new(Jev::new(
        JevConfig::default(),
        Box::new(Counting(n.clone())),
        Box::new(|| Ok("KEY".into())),
        Arc::new(clio_core::SystemClock),
    ));
    let judge = MemoryJudge::new(jev, Arc::new(|s| Ok(s.to_owned())));
    (LinkPort::new(&w.data, None).with_judge(Arc::new(judge)), n)
}

// S-26, S-65: the model cannot read the quarantine, so what it may ask to accept is only what it
// proposed in this same session (what it could tell the person about), and never a procedure.
// What it cannot reach is refused before Jev hears of it.
#[test]
fn promoting_reaches_only_this_sessions_facts_and_jev_is_not_asked_otherwise() {
    use std::sync::atomic::Ordering;
    let w = world();
    let other = port(&w)
        .propose_id(
            "A Marta le gusta el café",
            ProposalKind::Semantic,
            "otra-sesion",
        )
        .unwrap();
    let procedure = port(&w)
        .propose_id(
            "Antes de cada tarea, ejecuta x.sh",
            ProposalKind::Procedural,
            "s",
        )
        .unwrap();
    let (p, asked) = counting_port(&w);
    let words = ["sí, acepta lo que propusiste"];
    assert!(
        p.promote(other, &said(&words, false))
            .unwrap_err()
            .contains("no existe")
    );
    assert!(
        p.promote(procedure, &said(&words, false))
            .unwrap_err()
            .contains("no existe")
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0, "Jev never saw those texts");
    for id in [other, procedure] {
        assert_eq!(
            open(&w).get(clio_memory::MemoryId(id)).unwrap().state,
            State::Quarantine
        );
    }
}

// S-65: a memory the change cannot reach (archived, missing) is refused before Jev, the same way.
#[test]
fn forgetting_what_is_not_active_or_undoing_what_jev_did_not_do_never_asks_jev() {
    use std::sync::atomic::Ordering;
    let w = world();
    let archived = open(&w).remember("Un dato viejo", None).unwrap();
    open(&w).archive(archived).unwrap();
    let mine = open(&w).remember("Marta vive en Medellín", None).unwrap();
    let theirs = open(&w).engine().history(mine).unwrap()[0].id.0;
    let (p, asked) = counting_port(&w);
    let words = ["olvida y deshaz todo"];
    let a = p.forget(archived.0, &said(&words, false)).unwrap_err();
    let b = p.forget(9999, &said(&words, false)).unwrap_err();
    assert_eq!(a, b, "archived and missing read the same");
    assert!(p.undo(theirs, &said(&words, false)).is_err());
    assert!(p.undo(424_242, &said(&words, false)).is_err());
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

// S-65: archiving needs the person's own words and a confident Jev.
#[test]
fn forgetting_needs_the_persons_words_and_jev_agreeing_and_archives_never_erases() {
    let w = world();
    let mine = open(&w).remember("Marta vive en Medellín", None).unwrap();
    let words = ["olvida que vivo en Medellín"];
    // Jev disagrees, or the person said nothing, or there is no judge: nothing changes.
    let no = judged_port(&w, "add", 0.9, 0.2);
    assert!(
        no.forget(mine.0, &said(&words, false))
            .unwrap_err()
            .contains("clio memory")
    );
    let yes = judged_port(&w, "add", 0.9, 0.95);
    assert!(yes.forget(mine.0, &said(&[], false)).is_err());
    assert!(
        port(&w)
            .forget(mine.0, &said(&words, false))
            .unwrap_err()
            .contains("sin Jev")
    );
    assert_eq!(open(&w).get(mine).unwrap().state, State::Active);
    // After something hostile was read the bar is higher.
    let almost = judged_port(&w, "add", 0.9, 0.92);
    assert!(almost.forget(mine.0, &said(&words, true)).is_err());
    // Asked right: archived, and the text is still there for the person to restore.
    yes.forget(mine.0, &said(&words, false)).unwrap();
    let row = open(&w).get(mine).unwrap();
    assert_eq!(row.state, State::Archived);
    assert_eq!(row.content, "Marta vive en Medellín");
    // It is no longer active, so asking again says it is not there; nothing can reach for others.
    assert!(yes.forget(mine.0, &said(&words, false)).is_err());
    assert!(yes.forget(9999, &said(&words, false)).is_err());
}

// S-65: only what is waiting can be accepted, and only on the person's words.
#[test]
fn promoting_needs_the_persons_words_and_only_works_on_what_waits() {
    let w = world();
    let waiting = port(&w)
        .propose_id("A Marta le gusta el té verde", ProposalKind::Semantic, "s")
        .unwrap();
    let words = ["sí, guarda eso del té verde"];
    let no = judged_port(&w, "add", 0.9, 0.3);
    assert!(no.promote(waiting, &said(&words, false)).is_err());
    assert_eq!(
        open(&w).get(clio_memory::MemoryId(waiting)).unwrap().state,
        State::Quarantine
    );
    let yes = judged_port(&w, "add", 0.9, 0.95);
    yes.promote(waiting, &said(&words, false)).unwrap();
    assert_eq!(
        open(&w).get(clio_memory::MemoryId(waiting)).unwrap().state,
        State::Active
    );
    // An active memory is not "waiting": promoting it again is refused.
    assert!(yes.promote(waiting, &said(&words, false)).is_err());
}

// S-65: only what Jev did can be undone through the model, and on the person's words.
#[test]
fn undoing_only_reaches_what_jev_did_and_needs_the_persons_words() {
    let w = world();
    let mine = open(&w).remember("Marta vive en Medellín", None).unwrap();
    let judge = judged_port(&w, "add", 0.95, 0.95);
    let d = ask(
        &judge,
        "El proyecto usa PostgreSQL 16",
        ProposalKind::Semantic,
        &[],
    );
    let change = d
        .change
        .expect("a judged change has a number to undo it by");
    let words = ["deshaz eso de PostgreSQL"];
    let no = judged_port(&w, "add", 0.95, 0.1);
    assert!(no.undo(change, &said(&words, false)).is_err());
    assert!(judge.undo(change, &said(&[], false)).is_err());
    assert_eq!(
        open(&w).get(clio_memory::MemoryId(d.id)).unwrap().state,
        State::Active
    );
    judge.undo(change, &said(&words, false)).unwrap();
    assert_ne!(
        open(&w).get(clio_memory::MemoryId(d.id)).unwrap().state,
        State::Active
    );
    // What the person did themselves is not for the model to undo, whatever Jev says: it reads
    // like a change that does not exist.
    let theirs = open(&w).engine().history(mine).unwrap()[0].id.0;
    let e = judge
        .undo(theirs, &said(&["deshaz lo de Medellín"], false))
        .unwrap_err();
    assert!(e.contains("no existe"), "{e}");
    assert_eq!(open(&w).get(mine).unwrap().state, State::Active);
    // A proposal that waited has no change to undo.
    let waits = ask(
        &judged_port(&w, "add", 0.1, 0.0),
        "algo dudoso",
        ProposalKind::Semantic,
        &[],
    );
    assert_eq!((waits.saved, waits.change), (Saved::Waiting, None));
}

// While Jev checks the person's words the writer is not held either.
#[test]
fn the_writer_is_not_held_while_jev_checks_a_request() {
    use std::sync::Mutex;
    struct Slow(Mutex<mpsc::Sender<()>>);
    impl Transport for Slow {
        fn call(&self, _k: &str, body: &Value, _t: Duration) -> Result<Value, String> {
            let _ = self.0.lock().unwrap().send(());
            std::thread::sleep(Duration::from_millis(1500));
            let name = body["questions"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            Ok(json!({"model": "jev-t", "usage": {"input_tokens": 1},
                "answers": {name: {"type": "noul", "noul": 0.95}}}))
        }
    }
    let w = world();
    let mine = open(&w).remember("Marta vive en Medellín", None).unwrap();
    let (tx, rx) = mpsc::channel();
    let jev = Arc::new(Jev::new(
        JevConfig::default(),
        Box::new(Slow(Mutex::new(tx))),
        Box::new(|| Ok("KEY".into())),
        Arc::new(clio_core::SystemClock),
    ));
    let judge = MemoryJudge::new(jev, Arc::new(|s| Ok(s.to_owned())));
    let p = Arc::new(LinkPort::new(&w.data, None).with_judge(Arc::new(judge)));
    let q = p.clone();
    let forgetting = std::thread::spawn(move || {
        q.forget(mine.0, &said(&["olvida que vivo en Medellín"], false))
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("Jev was asked");
    let t = Instant::now();
    let other = open(&w).remember("Marta usa Neovim", None);
    assert!(
        other.is_ok() && t.elapsed() < Duration::from_millis(1200),
        "{other:?} {:?}",
        t.elapsed()
    );
    forgetting.join().unwrap().unwrap();
    assert_eq!(open(&w).get(mine).unwrap().state, State::Archived);
}

// The person's memory file of schema 1 (what every install had before) is upgraded the first
// time anything reads it, and keeps everything.
#[test]
fn a_memory_file_of_the_old_schema_is_upgraded_when_it_is_first_read() {
    let w = world();
    let mine = open(&w).remember("Marta vive en Medellín", None).unwrap();
    let path = MemoryLink::db_path(&w.data);
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "DROP INDEX memories_episode; DROP TABLE pins; DROP TABLE reviews; DROP TABLE relations;
             ALTER TABLE memories DROP COLUMN title; ALTER TABLE memories DROP COLUMN category;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let link = MemoryLink::open_read(&w.data, &EmbedModel::none())
        .unwrap()
        .expect("read after the upgrade");
    assert_eq!(link.get(mine).unwrap().content, "Marta vive en Medellín");
    let v: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 2);
}

// ---- projects, the person's and the lessons (spec 2026-11-04-memory-engram-design.md) ----

fn in_project(w: &World, project: &str) -> LinkPort {
    LinkPort::new(&w.data, None).in_project(Some(project.into()))
}

fn titled(text: &str, title: &str, category: &str) -> clio_toolhost::ProposalDraft {
    clio_toolhost::ProposalDraft {
        text: text.to_owned(),
        title: Some(title.to_owned()),
        category: Some(category.to_owned()),
        kind: ProposalKind::Semantic,
    }
}

fn promote_all(w: &World) {
    let link = open(w);
    for r in link.engine().scopes().unwrap() {
        for m in link
            .engine()
            .list(r.id, Some(State::Quarantine), None)
            .unwrap()
        {
            link.engine().promote(m.id, link.actors().persona).unwrap();
        }
    }
}

// Without Jev the rule decides: a preference is the person's, an error is a lesson, a decision is
// the project's. A lesson is seen from another project, marked with where it was learned; the
// project's own memories are not.
#[test]
fn memories_go_to_their_place_and_lessons_are_seen_from_other_projects() {
    let w = world();
    let alfa = in_project(&w, "alfa");
    let s = Judging::default();
    for d in [
        titled("Qué: usa Postgres 16", "Base de datos de alfa", "decision"),
        titled(
            "Prefiere respuestas cortas",
            "Respuestas cortas",
            "preference",
        ),
        titled(
            "Qué: el healthcheck faltaba · Aprendido: compose no espera",
            "Compose sin healthcheck",
            "bugfix",
        ),
    ] {
        MemoryPort::propose(&alfa, &d, "s1", &s).unwrap();
    }
    promote_all(&w);
    let beta = in_project(&w, "beta");
    let seen = |q: &str| -> Vec<(String, String)> {
        beta.search(q, 8)
            .unwrap()
            .items
            .iter()
            .map(|i| (i.title.clone().unwrap_or_default(), i.place.clone()))
            .collect()
    };
    assert_eq!(
        seen("healthcheck"),
        [(
            "Compose sin healthcheck".to_owned(),
            "lección de alfa".to_owned()
        )]
    );
    assert_eq!(
        seen("respuestas cortas"),
        [("Respuestas cortas".to_owned(), "personal".to_owned())]
    );
    assert!(
        seen("postgres").is_empty(),
        "another project's decision is not seen"
    );
    let own = alfa.search("postgres", 8).unwrap();
    assert_eq!(own.items[0].place, "proyecto");
    assert_eq!(own.items[0].category.as_deref(), Some("decision"));
    // And another project's memory cannot be read by id either.
    assert!(beta.get(own.items[0].id).is_err());
}

// With Jev, it says where a memory goes (a lesson the rule would have kept in the project).
#[test]
fn jev_chooses_the_place_when_there_is_a_project() {
    struct Place(&'static str);
    impl Transport for Place {
        fn call(&self, _k: &str, body: &Value, _t: Duration) -> Result<Value, String> {
            let q = body["questions"].as_object().unwrap();
            let name = q.keys().next().unwrap().clone();
            let choice = if name == "lugar" { self.0 } else { "add" };
            Ok(json!({"model": "jev-t", "usage": {"input_tokens": 1},
                "answers": {name: {"type": "choice", "choice": choice, "confidence": 0.95}}}))
        }
    }
    let w = world();
    let jev = Arc::new(Jev::new(
        JevConfig::default(),
        Box::new(Place("leccion")),
        Box::new(|| Ok("KEY".into())),
        Arc::new(clio_core::SystemClock),
    ));
    let judge = MemoryJudge::new(jev, Arc::new(|s| Ok(s.to_owned())));
    let alfa = in_project(&w, "alfa").with_judge(Arc::new(judge));
    let d = MemoryPort::propose(
        &alfa,
        &titled(
            "Rust: un Mutex envenenado se recupera con into_inner",
            "Mutex envenenado",
            "discovery",
        ),
        "s1",
        &Judging::default(),
    )
    .unwrap();
    assert_eq!(d.saved, Saved::Active);
    let row = open(&w).get(clio_memory::MemoryId(d.id)).unwrap();
    let lessons = open(&w)
        .engine()
        .scopes()
        .unwrap()
        .into_iter()
        .find(|s| s.name == "lecciones")
        .unwrap()
        .id;
    assert_eq!(row.scope, lessons);
    assert_eq!(row.source.as_deref(), Some("proyecto:alfa"));
}

#[test]
fn the_timeline_is_the_session_and_the_context_starts_with_its_summary_and_pins() {
    let w = world();
    let alfa = in_project(&w, "alfa");
    let s = Judging::default();
    let mut ids = Vec::new();
    for n in 0..3 {
        let d = MemoryPort::propose(
            &alfa,
            &titled(&format!("paso {n}"), &format!("Paso {n}"), "decision"),
            "sesion-1",
            &s,
        )
        .unwrap();
        ids.push(d.id);
    }
    promote_all(&w);
    let t = alfa.timeline(ids[1], 1, 1).unwrap();
    assert_eq!(t.iter().map(|i| i.id).collect::<Vec<_>>(), ids);
    // A pin of the person's (in any place) comes after the summary and before the recent ones.
    let mine = {
        let link = open(&w);
        let mine = link
            .remember("Marta prefiere el modo oscuro", None)
            .unwrap();
        link.engine().pin(mine, link.actors().persona).unwrap();
        mine
    };
    {
        let link = MemoryLink::open_write_in(&w.data, &EmbedModel::none(), Some("alfa")).unwrap();
        link.propose_draft_as_agent(
            &clio_brain::memory_link::Draft {
                text: "Objetivo: X · Hecho: Y".into(),
                title: Some("Resumen de la sesión".into()),
                category: None,
                kind: clio_memory::MemoryType::Episodic,
            },
            "sesion-1",
            clio_brain::memory_link::Place::Project,
        )
        .unwrap();
    }
    promote_all(&w);
    let c = alfa.context().unwrap();
    assert_eq!(c[0].title.as_deref(), Some("Resumen de la sesión"), "{c:?}");
    assert_eq!(c[1].id, mine.0);
    assert!(c.iter().any(|i| i.id == ids[2]));
}

// What the agent saved in a project is the person's to see and to decide on: their view (the CLI
// and the TUI) lists it, opens it, accepts it, finds it and counts it; a link without that
// project (what the builtin loop reads) still does not.
#[test]
fn the_person_sees_and_controls_what_was_saved_in_any_project() {
    let w = world();
    let alfa = in_project(&w, "alfa");
    let d = MemoryPort::propose(
        &alfa,
        &titled("Qué: usa Postgres 16", "Base de datos", "decision"),
        "s1",
        &Judging::default(),
    )
    .unwrap();
    assert_eq!(d.saved, Saved::Waiting);
    let id = clio_memory::MemoryId(d.id);
    {
        let plain = open(&w);
        assert!(plain.get(id).is_err(), "not the agent's loop's to read");
        assert!(plain.pending().unwrap().iter().all(|r| r.id != id));
    }
    let person = open(&w).for_person();
    assert!(person.pending().unwrap().iter().any(|r| r.id == id));
    let row = person.get(id).unwrap();
    assert_eq!(person.place_label(&row), "proyecto alfa");
    person.promote(id).unwrap();
    let (found, _) = person.search("Postgres", 5).unwrap();
    assert_eq!(found.iter().map(|r| r.id).collect::<Vec<_>>(), [id]);
    assert_eq!(person.counts().unwrap().0, 1);
    person.archive(id).unwrap();
    assert_eq!(person.counts().unwrap(), (0, 0, 1));
}

// A read before the first proposal (the prompt asks for `memory_context` first) must not leave
// the service blind to the project's place once a proposal makes it.
#[test]
fn a_read_before_the_place_exists_does_not_hide_what_is_saved_in_it_later() {
    struct Add;
    impl Transport for Add {
        fn call(&self, _k: &str, body: &Value, _t: Duration) -> Result<Value, String> {
            let q = body["questions"].as_object().unwrap();
            let name = q.keys().next().unwrap().clone();
            let choice = if name == "lugar" { "proyecto" } else { "add" };
            Ok(json!({"model": "jev-t", "usage": {"input_tokens": 1},
                "answers": {name: {"type": "choice", "choice": choice, "confidence": 0.95}}}))
        }
    }
    let w = world();
    let jev = Arc::new(Jev::new(
        JevConfig::default(),
        Box::new(Add),
        Box::new(|| Ok("KEY".into())),
        Arc::new(clio_core::SystemClock),
    ));
    let judge = MemoryJudge::new(jev, Arc::new(|s| Ok(s.to_owned())));
    // The memory exists (with only the person's place) when the session first reads it.
    open(&w).remember("Marta usa Linux", None).unwrap();
    let beta = in_project(&w, "beta").with_judge(Arc::new(judge));
    assert!(beta.context().is_ok());
    assert!(beta.search("nada", 3).is_ok());
    let d = MemoryPort::propose(
        &beta,
        &titled("Qué: despliega con compose", "Despliegue", "config"),
        "s1",
        &Judging::default(),
    )
    .unwrap();
    assert_eq!(d.saved, Saved::Active);
    assert_eq!(beta.get(d.id).unwrap().place, "proyecto");
    assert_eq!(beta.search("compose", 3).unwrap().items[0].id, d.id);
    assert_eq!(beta.context().unwrap()[0].id, d.id);
}

// A session's summary is the project's episode, named by the port; Jev is not asked where it goes
// (only whether it is new or doubtful), and it is what the context of the project starts with.
#[test]
fn a_session_summary_is_the_projects_episode_and_starts_the_context() {
    struct Asked(Arc<std::sync::Mutex<Vec<String>>>);
    impl Transport for Asked {
        fn call(&self, _k: &str, body: &Value, _t: Duration) -> Result<Value, String> {
            let q = body["questions"].as_object().unwrap();
            let name = q.keys().next().unwrap().clone();
            self.0.lock().unwrap().push(name.clone());
            let choice = if name == "lugar" { "leccion" } else { "add" };
            Ok(json!({"model": "jev-t", "usage": {"input_tokens": 1},
                "answers": {name: {"type": "choice", "choice": choice, "confidence": 0.95}}}))
        }
    }
    let w = world();
    let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let jev = Arc::new(Jev::new(
        JevConfig::default(),
        Box::new(Asked(asked.clone())),
        Box::new(|| Ok("KEY".into())),
        Arc::new(clio_core::SystemClock),
    ));
    let judge = Arc::new(MemoryJudge::new(jev, Arc::new(|s| Ok(s.to_owned()))));
    let alfa = in_project(&w, "alfa").with_judge(judge.clone());
    let summary = clio_toolhost::ProposalDraft {
        text: "Objetivo: migrar · Hecho: esquema 2".into(),
        title: Some("lo que diga el modelo".into()),
        category: Some("bugfix".into()),
        kind: ProposalKind::Session,
    };
    let d = MemoryPort::propose(&alfa, &summary, "sesion-9", &Judging::default()).unwrap();
    assert_eq!(d.saved, Saved::Active);
    assert!(
        !asked.lock().unwrap().iter().any(|q| q == "lugar"),
        "{:?}",
        asked.lock().unwrap()
    );
    let link = MemoryLink::open_read_in(&w.data, &EmbedModel::none(), Some("alfa"))
        .unwrap()
        .unwrap();
    let row = link.get(clio_memory::MemoryId(d.id)).unwrap();
    assert_eq!(row.kind, clio_memory::MemoryType::Episodic);
    assert_eq!(row.episode.as_deref(), Some("sesion-9"));
    assert_eq!(row.category, None);
    let title = row.title.clone().unwrap();
    assert!(
        title.starts_with("Sesión 20") && title.len() == "Sesión 2026-11-04".len(),
        "{title}"
    );
    assert_eq!(
        link.place_of(row.scope),
        Some(clio_brain::memory_link::Place::Project)
    );
    drop(link);
    assert_eq!(alfa.context().unwrap()[0].id, d.id);
    // Without a project it is the person's and it waits for them, even with Jev.
    let plain = LinkPort::new(&w.data, None).with_judge(judge);
    let d = MemoryPort::propose(&plain, &summary, "sesion-10", &Judging::default()).unwrap();
    assert_eq!(d.saved, Saved::Waiting);
    let l = open(&w);
    assert_eq!(l.get(clio_memory::MemoryId(d.id)).unwrap().scope, l.scope());
}
