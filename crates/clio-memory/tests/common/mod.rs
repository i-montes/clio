#![allow(dead_code, clippy::unwrap_used)] // test helpers: panicking is the failure mode

use std::path::PathBuf;

use clio_memory::*;
use tempfile::TempDir;

/// Every cell of every table (FTS shadow tables included) as one string.
pub fn dump_all(path: &std::path::Path) -> String {
    use rusqlite::types::ValueRef;
    let conn = rusqlite::Connection::open(path).unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, rusqlite::Error>>()
        .unwrap();
    let mut out = String::new();
    for t in tables {
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{t}\"")).unwrap();
        let n = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            for i in 0..n {
                match r.get_ref(i).unwrap() {
                    ValueRef::Integer(v) => out.push_str(&v.to_string()),
                    ValueRef::Real(v) => out.push_str(&v.to_string()),
                    ValueRef::Text(b) | ValueRef::Blob(b) => {
                        out.push_str(&String::from_utf8_lossy(b))
                    }
                    ValueRef::Null => {}
                }
                out.push('|');
            }
        }
    }
    out
}

/// Deterministic stand-in for a real embedding model: a bag of hashed words with a few
/// synonyms and stop words, so tests can reason about similarity. Not normalized.
pub struct HashEmbedder {
    pub model: String,
    pub dim: usize,
}

impl HashEmbedder {
    pub fn new(model: &str, dim: usize) -> Self {
        Self {
            model: model.into(),
            dim,
        }
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

impl Embedder for HashEmbedder {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        const STOP: [&str; 11] = [
            "en", "el", "la", "los", "las", "de", "un", "una", "y", "a", "se",
        ];
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.0f32; self.dim];
                let mut any = false;
                for w in t.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
                    if w.is_empty() || STOP.contains(&w) {
                        continue;
                    }
                    let w = if matches!(w, "auto" | "coche" | "carro") {
                        "vehiculo"
                    } else {
                        w
                    };
                    v[(fnv(w) % self.dim as u64) as usize] += 1.0;
                    any = true;
                }
                if !any {
                    v[0] = 1.0;
                }
                v
            })
            .collect())
    }
}

/// Counts how many times `embed` is called.
pub struct CountingEmbedder {
    pub inner: HashEmbedder,
    pub calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Embedder for CountingEmbedder {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.embed(texts)
    }
}

/// Behaves like `HashEmbedder`, except that text containing `poison` has no representation
/// (an all-zero vector), like an emoji-only text for a real model.
pub struct PoisonEmbedder {
    pub inner: HashEmbedder,
    pub poison: String,
}

impl Embedder for PoisonEmbedder {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut out = self.inner.embed(texts)?;
        for (v, t) in out.iter_mut().zip(texts) {
            if t.to_lowercase().contains(&self.poison) {
                v.iter_mut().for_each(|x| *x = 0.0);
            }
        }
        Ok(out)
    }
}

pub enum Fault {
    Zeros,
    WrongDim,
    TooFew,
}

/// An embedder whose output is invalid in one specific way.
pub struct BadEmbedder {
    pub model: String,
    pub fault: Fault,
}

impl Embedder for BadEmbedder {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn dim(&self) -> usize {
        8
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(match self.fault {
            Fault::Zeros => texts.iter().map(|_| vec![0.0; 8]).collect(),
            Fault::WrongDim => texts.iter().map(|_| vec![1.0; 4]).collect(),
            Fault::TooFew => Vec::new(),
        })
    }
}

pub struct Fixture {
    pub _dir: TempDir,
    pub path: PathBuf,
    pub clock: std::sync::Arc<FixedClock>,
    pub engine: Engine,
    pub scope: ScopeId,
    pub person: ActorId,
    pub agent: ActorId,
}

pub fn fixture() -> Fixture {
    build_fixture(None)
}

pub fn fixture_with(embedder: std::sync::Arc<dyn Embedder>) -> Fixture {
    build_fixture(Some(embedder))
}

fn build_fixture(embedder: Option<std::sync::Arc<dyn Embedder>>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let clock = std::sync::Arc::new(FixedClock::new(1000));
    let mut engine = Engine::open(&path, OpenMode::ReadWrite, clock.clone()).unwrap();
    if let Some(e) = embedder {
        engine = engine.with_embedder(e);
    }
    let scope = engine.create_scope("main", "es").unwrap();
    let person = engine
        .register_actor("ana", ActorKind::Person, Quota::default())
        .unwrap();
    let agent = engine
        .register_actor("bot", ActorKind::Agent, Quota::default())
        .unwrap();
    Fixture {
        _dir: dir,
        path,
        clock,
        engine,
        scope,
        person,
        agent,
    }
}

pub fn prop(scope: ScopeId, actor: ActorId, content: &str) -> Proposal {
    Proposal {
        scope,
        actor,
        kind: MemoryType::Semantic,
        content: content.into(),
        topic_key: None,
        origin: Origin::User,
        session: None,
        episode: None,
        source: None,
        valid_from: None,
        valid_to: None,
    }
}

pub enum Behavior {
    Say(Choice, f32),
    Nothing,
    Fail,
    Panic,
}

/// A decider that answers whatever it was told to, and remembers what it was asked.
pub struct ScriptedDecider {
    name: String,
    behavior: Behavior,
    reason: String,
    pub calls: std::sync::atomic::AtomicUsize,
    pub seen: std::sync::Mutex<Vec<Question>>,
}

impl ScriptedDecider {
    pub fn new(name: &str, behavior: Behavior) -> std::sync::Arc<Self> {
        Self::with_reason(name, behavior, "scripted")
    }

    pub fn with_reason(name: &str, behavior: Behavior, reason: &str) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            name: name.into(),
            behavior,
            reason: reason.into(),
            calls: std::sync::atomic::AtomicUsize::new(0),
            seen: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Decider for ScriptedDecider {
    fn name(&self) -> &str {
        &self.name
    }

    fn decide(&self, q: &Question) -> Result<Option<Decision>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.seen.lock().unwrap().push(q.clone());
        match &self.behavior {
            Behavior::Say(choice, confidence) => Ok(Some(Decision {
                choice: *choice,
                confidence: *confidence,
                reason: Some(self.reason.clone()),
            })),
            Behavior::Nothing => Ok(None),
            Behavior::Fail => Err(Error::Embedding("boom".into())),
            Behavior::Panic => panic!("decider exploded"),
        }
    }
}

/// (kind, state) of every job in the file, read straight from SQLite.
pub fn job_rows(path: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT kind, state FROM jobs ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}
