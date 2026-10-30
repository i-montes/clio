//! A deterministic stand-in for an LLM: record real answers once, keyed by the hash of the
//! request, and replay them in tests. A request with no recording fails loudly instead of
//! reaching the network.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// Every field here is part of the hash: adding one invalidates existing tapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Exact model id with its version, never an alias.
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub content: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug)]
pub enum Error {
    /// Replay mode and the tape has no entry for this request.
    NotRecorded {
        hash: String,
    },
    /// The answer to record looks like it holds a secret; nothing was written.
    Secret {
        hash: String,
    },
    ScriptExhausted,
    Tape(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRecorded { hash } => {
                write!(f, "no recording for request {hash}; record it first")
            }
            Self::Secret { hash } => write!(
                f,
                "request {hash} holds something that looks like a secret; not recorded"
            ),
            Self::ScriptExhausted => write!(f, "the script has no more answers"),
            Self::Tape(m) => write!(f, "tape error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub trait Llm {
    fn complete(&self, request: &Request) -> Result<Response, Error>;
}

/// SHA-256 of the request as JSON with sorted keys (serde_json's default map is ordered), so
/// the order of fields in the source never changes the hash.
pub fn request_hash(request: &Request) -> String {
    let value = serde_json::to_value(request).unwrap_or_default();
    let canonical = serde_json::to_string(&value).unwrap_or_default();
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    hash: String,
    request: Request,
    response: Response,
}

/// The recordings, one JSON line each. Meant to be read in a diff.
#[derive(Debug, Default)]
pub struct Tape {
    path: Option<PathBuf>,
    entries: BTreeMap<String, Entry>,
}

impl Tape {
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads `path` if it exists; new entries are appended to it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let mut entries = BTreeMap::new();
        if path.exists() {
            let text = fs::read_to_string(&path).map_err(|e| Error::Tape(e.to_string()))?;
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                let e: Entry =
                    serde_json::from_str(line).map_err(|e| Error::Tape(e.to_string()))?;
                entries.insert(e.hash.clone(), e);
            }
        }
        Ok(Self {
            path: Some(path),
            entries,
        })
    }

    pub fn get(&self, request: &Request) -> Option<&Response> {
        self.entries
            .get(&request_hash(request))
            .map(|e| &e.response)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn insert(&mut self, request: &Request, response: &Response) -> Result<(), Error> {
        let hash = request_hash(request);
        if looks_like_secret(&request_text(request)) || looks_like_secret(&response.content) {
            return Err(Error::Secret { hash });
        }
        let entry = Entry {
            hash: hash.clone(),
            request: request.clone(),
            response: response.clone(),
        };
        if let Some(path) = &self.path {
            let line = serde_json::to_string(&entry).map_err(|e| Error::Tape(e.to_string()))?;
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| Error::Tape(e.to_string()))?;
            writeln!(file, "{line}").map_err(|e| Error::Tape(e.to_string()))?;
        }
        self.entries.insert(hash, entry);
        Ok(())
    }
}

fn request_text(request: &Request) -> String {
    request
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// shortcut: a few well-known key shapes only; replace with the vault's single output filter
/// (S-20) once it exists.
fn looks_like_secret(text: &str) -> bool {
    const PREFIXES: [(&str, usize); 5] = [
        ("sk-", 20),
        ("AKIA", 16),
        ("ghp_", 30),
        ("xox", 20),
        ("Bearer ", 20),
    ];
    if text.contains("-----BEGIN") {
        return true;
    }
    PREFIXES.iter().any(|(prefix, min)| {
        text.match_indices(prefix).any(|(i, _)| {
            let rest = &text[i + prefix.len()..];
            rest.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .count()
                >= *min
        })
    })
}

/// Replay mode: answers from the tape, never touches the network.
#[derive(Debug)]
pub struct Replay {
    tape: Tape,
}

impl Replay {
    pub fn new(tape: Tape) -> Self {
        Self { tape }
    }
}

impl Llm for Replay {
    fn complete(&self, request: &Request) -> Result<Response, Error> {
        self.tape
            .get(request)
            .cloned()
            .ok_or_else(|| Error::NotRecorded {
                hash: request_hash(request),
            })
    }
}

/// Record mode: answers from the tape when it can, otherwise asks `inner` and records it.
pub struct Record<L> {
    inner: L,
    tape: Mutex<Tape>,
}

impl<L: Llm> Record<L> {
    pub fn new(inner: L, tape: Tape) -> Self {
        Self {
            inner,
            tape: Mutex::new(tape),
        }
    }
}

impl<L: Llm> Llm for Record<L> {
    fn complete(&self, request: &Request) -> Result<Response, Error> {
        let mut tape = self
            .tape
            .lock()
            .map_err(|_| Error::Tape("tape lock poisoned".into()))?;
        if let Some(r) = tape.get(request) {
            return Ok(r.clone());
        }
        let response = self.inner.complete(request)?;
        tape.insert(request, &response)?;
        Ok(response)
    }
}

/// Fixed answers in order, for unit tests that need no tape.
pub struct Scripted {
    answers: Mutex<VecDeque<Response>>,
}

impl Scripted {
    pub fn new(answers: impl IntoIterator<Item = Response>) -> Self {
        Self {
            answers: Mutex::new(answers.into_iter().collect()),
        }
    }
}

impl Llm for Scripted {
    fn complete(&self, _: &Request) -> Result<Response, Error> {
        self.answers
            .lock()
            .map_err(|_| Error::ScriptExhausted)?
            .pop_front()
            .ok_or(Error::ScriptExhausted)
    }
}
