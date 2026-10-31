//! Spans and computed cost in their own SQLite file. A span carries ids, sizes, times and
//! decisions, never text: there is no column for content (S-46), and the text that does get in
//! (names and attribute values) is short and checked for secrets.

pub mod names;
mod platform;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: i64 = 1;
const MAX_TEXT: usize = 80;

#[derive(Debug)]
pub enum Error {
    Sql(String),
    Io(String),
    /// The file is readable by group or others (expected mode 0600).
    Permissions(PathBuf),
    SchemaVersion(i64),
    /// No price for this provider and model at the time of the span.
    NoPrice {
        provider: String,
        model: String,
    },
    /// Text that is too long or looks like a secret.
    Rejected(String),
    BadTraceparent(String),
    UnknownSpan(String),
    Overflow,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(m) | Self::Io(m) => write!(f, "{m}"),
            Self::Permissions(p) => write!(
                f,
                "{} must not be accessible by group or others (expected mode 0600)",
                p.display()
            ),
            Self::SchemaVersion(v) => write!(f, "schema version {v} is not {SCHEMA_VERSION}"),
            Self::NoPrice { provider, model } => write!(f, "no price for {provider}/{model}"),
            Self::Rejected(m) => write!(f, "rejected: {m}"),
            Self::BadTraceparent(m) => write!(f, "bad traceparent: {m}"),
            Self::UnknownSpan(id) => write!(f, "unknown or already ended span {id}"),
            Self::Overflow => write!(f, "cost overflow"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Clock {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    }
}

/// A clock that only moves when told to, so runs are reproducible.
pub struct FixedClock(Cell<i64>);

impl FixedClock {
    pub fn new(ms: i64) -> Self {
        Self(Cell::new(ms))
    }

    pub fn advance(&self, ms: i64) {
        self.0.set(self.0.get() + ms);
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.get()
    }
}

impl<C: Clock + ?Sized> Clock for std::rc::Rc<C> {
    fn now_ms(&self) -> i64 {
        (**self).now_ms()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Agent,
    Model,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Error,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
        }
    }
}

/// A value an attribute can hold. Text is short and checked; there is no way to attach content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attr {
    Int(i64),
    Bool(bool),
    Text(String),
}

#[derive(Debug, Clone, Copy)]
pub struct Usage<'a> {
    pub provider: &'a str,
    /// Exact model id with its version, as in the price table.
    pub model: &'a str,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// An open span. Hand it back to `Trace::end_span`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub trace_id: String,
    pub span_id: String,
    pub parent_id: Option<String>,
}

impl Span {
    /// The header to put on a message so the other side hangs its spans from this one.
    pub fn traceparent(&self) -> Traceparent {
        Traceparent {
            trace_id: self.trace_id.clone(),
            span_id: self.span_id.clone(),
        }
    }
}

/// W3C trace context: `00-<32 hex trace id>-<16 hex span id>-01`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traceparent {
    pub trace_id: String,
    pub span_id: String,
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && s.bytes().any(|b| b != b'0')
}

impl Traceparent {
    pub fn parse(s: &str) -> Result<Self> {
        let bad = |m: &str| Err(Error::BadTraceparent(m.to_string()));
        let parts: Vec<&str> = s.split('-').collect();
        let [version, trace_id, span_id, flags] = parts[..] else {
            return bad("expected four fields");
        };
        if version != "00" {
            return bad("unsupported version");
        }
        if !is_hex(trace_id, 32) || !is_hex(span_id, 16) {
            return bad("ids must be non-zero lowercase hex of 32 and 16 characters");
        }
        if flags.len() != 2 || !flags.bytes().all(|b| b.is_ascii_hexdigit()) {
            return bad("flags must be two hex digits");
        }
        Ok(Self {
            trace_id: trace_id.into(),
            span_id: span_id.into(),
        })
    }
}

impl fmt::Display for Traceparent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "00-{}-{}-01", self.trace_id, self.span_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub spans: i64,
    pub errors: i64,
    /// Spans that never ended: a crash or a hang.
    pub open: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micro_usd: i64,
    pub duration_ms: i64,
}

impl Summary {
    /// Every span ended and none failed. Whether the task itself succeeded is the bench's call.
    pub fn clean(&self) -> bool {
        self.errors == 0 && self.open == 0
    }
}

pub struct Trace {
    conn: Connection,
    clock: Box<dyn Clock>,
    seed: u64,
    counter: Cell<u64>,
}

impl Trace {
    /// Opens `path`, creating it with mode 0600 if it is missing. An existing file that group
    /// or others can read is refused.
    pub fn open(path: impl AsRef<Path>, clock: impl Clock + 'static) -> Result<Self> {
        let path = path.as_ref();
        platform::prepare_private_file(path)?;
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        let version: i64 = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if version == 0 {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                [SCHEMA_VERSION.to_string()],
            )?;
        } else if version != SCHEMA_VERSION {
            return Err(Error::SchemaVersion(version));
        }
        // shortcut: the ids come from the clock, the process id and a counter; replace with a
        // random source if two machines ever write to the same file.
        let seed = (clock.now_ms() as u64) ^ (u64::from(std::process::id()) << 32);
        Ok(Self {
            conn,
            clock: Box::new(clock),
            seed,
            counter: Cell::new(0),
        })
    }

    /// Price in micro-USD per million tokens, valid from `valid_from_ms` on. A later row for the
    /// same model never changes the cost of spans that started before it.
    pub fn set_price(
        &self,
        provider: &str,
        model: &str,
        valid_from_ms: i64,
        input_per_mtok: u64,
        output_per_mtok: u64,
    ) -> Result<()> {
        check_text("provider", provider)?;
        check_text("model", model)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO prices (provider, model, valid_from_ms, input_micro_per_mtok, output_micro_per_mtok) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![provider, model, valid_from_ms, input_per_mtok as i64, output_per_mtok as i64],
        )?;
        Ok(())
    }

    pub fn new_trace_id(&self) -> String {
        self.next_id(32)
    }

    /// A span inside an existing trace, for a `Traceparent` that came in from another side.
    pub fn start_span(
        &self,
        trace_id: &str,
        parent_id: Option<&str>,
        kind: Kind,
        name: &str,
    ) -> Result<Span> {
        check_text("name", name)?;
        let span_id = self.next_id(16);
        self.conn.execute(
            "INSERT INTO spans (span_id, trace_id, parent_id, name, kind, start_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![span_id, trace_id, parent_id, name, names::operation(kind), self.clock.now_ms()],
        )?;
        Ok(Span {
            trace_id: trace_id.into(),
            span_id,
            parent_id: parent_id.map(String::from),
        })
    }

    /// A child of `parent`.
    pub fn child(&self, parent: &Span, kind: Kind, name: &str) -> Result<Span> {
        self.start_span(&parent.trace_id, Some(&parent.span_id), kind, name)
    }

    /// Closes the span. With `usage` it computes the cost from the price in force when the span
    /// started; a model with no price is an error, never a cost of zero.
    pub fn end_span(
        &self,
        span: &Span,
        status: Status,
        usage: Option<Usage<'_>>,
        attrs: &BTreeMap<String, Attr>,
    ) -> Result<()> {
        let mut json = serde_json::Map::new();
        for (key, value) in attrs {
            check_text("attribute name", key)?;
            json.insert(
                key.clone(),
                match value {
                    Attr::Int(n) => (*n).into(),
                    Attr::Bool(b) => (*b).into(),
                    Attr::Text(t) => {
                        check_text("attribute value", t)?;
                        t.clone().into()
                    }
                },
            );
        }
        let start_ms: i64 = self
            .conn
            .query_row(
                "SELECT start_ms FROM spans WHERE span_id = ?1 AND end_ms IS NULL",
                [&span.span_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| Error::UnknownSpan(span.span_id.clone()))?;
        let (provider, model, input, output, cost) = match usage {
            None => (None, None, None, None, None),
            Some(u) => {
                check_text("provider", u.provider)?;
                check_text("model", u.model)?;
                let cost = self.cost(u, start_ms)?;
                (
                    Some(u.provider),
                    Some(u.model),
                    Some(i64::from(u.input_tokens)),
                    Some(i64::from(u.output_tokens)),
                    Some(cost),
                )
            }
        };
        self.conn.execute(
            "UPDATE spans SET end_ms = ?2, status = ?3, provider = ?4, model = ?5, input_tokens = ?6, output_tokens = ?7, cost_micro_usd = ?8, attrs = ?9 WHERE span_id = ?1",
            params![span.span_id, self.clock.now_ms(), status.as_str(), provider, model, input, output, cost, serde_json::Value::Object(json).to_string()],
        )?;
        Ok(())
    }

    pub fn summary(&self, trace_id: &str) -> Result<Summary> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(status = 'error'), 0), COALESCE(SUM(end_ms IS NULL), 0),
                    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cost_micro_usd), 0),
                    COALESCE(MAX(end_ms), MIN(start_ms)) - COALESCE(MIN(start_ms), 0)
             FROM spans WHERE trace_id = ?1",
            [trace_id],
            |r| {
                Ok(Summary { spans: r.get(0)?, errors: r.get(1)?, open: r.get(2)?, input_tokens: r.get(3)?, output_tokens: r.get(4)?, cost_micro_usd: r.get(5)?, duration_ms: r.get(6)? })
            },
        )?)
    }

    /// Distinct prompt versions that the model calls of a trace carried, sorted (D15).
    pub fn prompt_versions(&self, trace_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT json_extract(attrs, '$.\"clio.prompt.version\"') AS v FROM spans
             WHERE trace_id = ?1 AND kind = ?2 AND v IS NOT NULL ORDER BY v",
        )?;
        let rows = stmt.query_map(params![trace_id, names::OPERATION_MODEL], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Model calls of a trace that carry no prompt version. The gate wants zero.
    pub fn model_spans_without_prompt_version(&self, trace_id: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM spans WHERE trace_id = ?1 AND kind = ?2
             AND json_extract(attrs, '$.\"clio.prompt.version\"') IS NULL",
            params![trace_id, names::OPERATION_MODEL],
            |r| r.get(0),
        )?)
    }

    /// Records a change of state of a contract as an instant child span of `parent`.
    pub fn contract_state(
        &self,
        parent: &Span,
        contract: &str,
        from: &str,
        to: &str,
    ) -> Result<()> {
        let span = self.child(parent, Kind::Agent, names::NAME_CONTRACT_STATE)?;
        let attrs = BTreeMap::from([
            (
                names::ATTR_CONTRACT.to_string(),
                Attr::Text(contract.into()),
            ),
            (names::ATTR_FROM.to_string(), Attr::Text(from.into())),
            (names::ATTR_TO.to_string(), Attr::Text(to.into())),
        ]);
        self.end_span(&span, Status::Ok, None, &attrs)
    }

    /// The `(from, to)` changes of one contract, oldest first.
    pub fn contract_history(&self, contract: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT json_extract(attrs, '$.from'), json_extract(attrs, '$.to') FROM spans
             WHERE name = ?1 AND json_extract(attrs, '$.contract') = ?2 ORDER BY start_ms, rowid",
        )?;
        let rows = stmt.query_map(params![names::NAME_CONTRACT_STATE, contract], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn cost(&self, u: Usage<'_>, at_ms: i64) -> Result<i64> {
        let (input_price, output_price): (i64, i64) = self
            .conn
            .query_row(
                "SELECT input_micro_per_mtok, output_micro_per_mtok FROM prices
                 WHERE provider = ?1 AND model = ?2 AND valid_from_ms <= ?3 ORDER BY valid_from_ms DESC LIMIT 1",
                params![u.provider, u.model, at_ms],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::NoPrice { provider: u.provider.into(), model: u.model.into() })?;
        // Integer micro-USD, rounded half up, so sums are exact and runs reproducible.
        let total = u128::from(u.input_tokens) * input_price as u128
            + u128::from(u.output_tokens) * output_price as u128;
        i64::try_from((total + 500_000) / 1_000_000).map_err(|_| Error::Overflow)
    }

    fn next_id(&self, hex_len: usize) -> String {
        let n = self.counter.get();
        self.counter.set(n + 1);
        let digest = Sha256::digest(format!("clio-trace:{}:{n}", self.seed));
        let id: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        id[..hex_len].to_string()
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS prices (
    provider TEXT NOT NULL, model TEXT NOT NULL, valid_from_ms INTEGER NOT NULL,
    input_micro_per_mtok INTEGER NOT NULL, output_micro_per_mtok INTEGER NOT NULL,
    PRIMARY KEY (provider, model, valid_from_ms));
CREATE TABLE IF NOT EXISTS spans (
    span_id TEXT PRIMARY KEY, trace_id TEXT NOT NULL, parent_id TEXT,
    name TEXT NOT NULL, kind TEXT NOT NULL, status TEXT,
    start_ms INTEGER NOT NULL, end_ms INTEGER,
    provider TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER, cost_micro_usd INTEGER,
    attrs TEXT);
CREATE INDEX IF NOT EXISTS spans_by_trace ON spans (trace_id);
";

/// Names and attribute values are labels, not content: short, and never a secret.
/// shortcut: the same few key shapes as clio-fake-llm; replace with the vault's single output
/// filter (S-20) once it exists.
fn check_text(what: &str, text: &str) -> Result<()> {
    if text.chars().count() > MAX_TEXT {
        return Err(Error::Rejected(format!(
            "{what} is longer than {MAX_TEXT} characters; spans carry no content"
        )));
    }
    const PREFIXES: [(&str, usize); 5] = [
        ("sk-", 20),
        ("AKIA", 16),
        ("ghp_", 30),
        ("xox", 20),
        ("Bearer ", 20),
    ];
    let secret = text.contains("-----BEGIN")
        || PREFIXES.iter().any(|(prefix, min)| {
            text.match_indices(prefix).any(|(i, _)| {
                text[i + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .count()
                    >= *min
            })
        });
    if secret {
        return Err(Error::Rejected(format!("{what} looks like a secret")));
    }
    Ok(())
}
