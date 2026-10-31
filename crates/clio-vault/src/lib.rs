//! The vault: keeps the secrets, hands a value only to the gateway for an allowed destination,
//! and scrubs known values out of every output. The model only ever sees `{{secreto:name}}`.
//!
//! No `Debug` or `Display` in this crate prints a value (S-18). Values are encrypted at rest
//! with XChaCha20-Poly1305; the master key comes from a passphrase (Argon2id) and is never
//! written to disk. A fresh or restarted vault is locked: nothing resolves until `unlock`.

mod crypto;
mod scrub;

pub use scrub::{Form, Redaction, Scrubber};

use clio_core::Clock;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

const FILE_CHECK: &[u8] = b"clio-vault:v1";
const BACKUP_MAGIC: &[u8] = b"CLIOBK1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    Locked,
    BadPassphrase,
    NotFound,
    /// The destination is not allowed for this secret (S-19).
    Denied(String),
    Revoked,
    Expired,
    /// A token that is unknown, expired, or used outside its scope.
    BadToken,
    InvalidName,
    /// Empty or too short to scrub safely.
    InvalidValue,
    Crypto,
    Io(String),
    /// A file or backup that is not ours, or was tampered with, or the wrong key.
    Format,
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Denied(why) => write!(f, "denied: {why}"),
            Self::Io(e) => write!(f, "io: {e}"),
            other => write!(f, "{other:?}"),
        }
    }
}
impl std::error::Error for VaultError {}

type Result<T> = std::result::Result<T, VaultError>;

/// Proof that the caller is the gateway. `resolve` and `issue_token` ask for it.
/// shortcut: in one process this is a convention, not a wall; the wall is the vault running as
/// its own user behind a 0600 Unix socket (task 233/234, not built yet).
/// ```compile_fail
/// let _ = clio_vault::GatewayCap(());
/// ```
pub struct GatewayCap(());

impl GatewayCap {
    /// Only the gateway's startup code should call this.
    pub fn for_gateway() -> Self {
        Self(())
    }
}

/// Proof that the person unlocked the vault with the passphrase. `put`, `rotate`, `revoke`,
/// `list`, `audit` and `backup` ask for it, so the model-facing code cannot call them.
/// ```compile_fail
/// let _ = clio_vault::PersonCap(());
/// ```
pub struct PersonCap(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    ApiKey,
    OAuthToken,
    Password,
    SshKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Personal,
    Organization,
    Project(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Active,
    Revoked,
}

/// Where a secret may go: one tool and one domain. A domain also covers its subdomains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dest {
    pub tool: String,
    pub domain: String,
}

impl Dest {
    pub fn new(tool: &str, domain: &str) -> Self {
        Self {
            tool: tool.to_string(),
            domain: domain.to_ascii_lowercase(),
        }
    }

    fn allows(&self, tool: &str, domain: &str) -> bool {
        let d = domain.to_ascii_lowercase();
        self.tool == tool && (d == self.domain || d.ends_with(&format!(".{}", self.domain)))
    }
}

/// Everything about a secret except its value. This is what `list` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    pub kind: Kind,
    pub destinations: Vec<Dest>,
    pub scope: Scope,
    pub rotate_every_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub rotated_at_ms: i64,
    pub state: State,
    pub last_used_ms: Option<i64>,
}

/// What the person gives to `put`. Deliberately without `Debug`.
pub struct SecretSpec {
    pub name: String,
    pub value: String,
    pub kind: Kind,
    pub destinations: Vec<Dest>,
    pub scope: Scope,
    pub rotate_every_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
}

/// A value in memory. It prints as `[redacted]` and has no `Display`.
pub struct SecretValue(String);

impl SecretValue {
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([redacted])")
    }
}

/// A short-lived bearer token for one secret, one tool and one node (S-21).
pub struct Token {
    id: String,
    pub node: String,
    pub expires_at_ms: i64,
}

impl Token {
    pub fn as_str(&self) -> &str {
        &self.id
    }
}
impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token {{ node: {:?}, id: [redacted] }}", self.node)
    }
}

/// A backup key, different from the master key. Without it a backup does not open.
pub struct BackupKey([u8; crypto::KEY_LEN]);

impl BackupKey {
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes: Vec<u8> = (0..s.len() / 2)
            .map(|i| {
                s.get(2 * i..2 * i + 2)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            })
            .collect::<Option<_>>()
            .ok_or(VaultError::Format)?;
        Ok(Self(bytes.try_into().map_err(|_| VaultError::Format)?))
    }
}
impl fmt::Debug for BackupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BackupKey([redacted])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Allowed,
    Denied(String),
}

/// One resolution attempt (never the value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub at_ms: i64,
    pub name: String,
    pub tool: String,
    pub domain: String,
    pub trace_id: String,
    pub via_token: bool,
    pub outcome: Outcome,
}

/// Raised when a resolution is refused; the gateway should surface it to the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub at_ms: i64,
    pub name: String,
    pub tool: String,
    pub domain: String,
    pub trace_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Telemetry {
    /// Allowed resolutions by `name -> domain`.
    pub resolutions: BTreeMap<String, u64>,
    pub denials: u64,
    pub redactions: u64,
    /// Secrets that expire or are due for rotation within the asked window.
    pub expiring: usize,
    /// Time spent locked so far, including the current stretch.
    pub locked_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    meta: Meta,
    blob: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct FileData {
    salt: Vec<u8>,
    check: Vec<u8>,
    secrets: Vec<Stored>,
}

struct TokenRec {
    secret: String,
    tool: String,
    expires_at_ms: i64,
}

struct Inner {
    key: Option<[u8; crypto::KEY_LEN]>,
    salt: Vec<u8>,
    check: Vec<u8>,
    secrets: BTreeMap<String, Stored>,
    tokens: HashMap<String, TokenRec>,
    audit: Vec<AuditEntry>,
    alerts: Vec<Alert>,
    resolutions: BTreeMap<String, u64>,
    denials: u64,
    locked_since: Option<i64>,
    locked_total_ms: i64,
}

pub struct Vault {
    clock: Arc<dyn Clock>,
    path: Option<PathBuf>,
    scrubber: Scrubber,
    inner: Mutex<Inner>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let i = self.inner();
        write!(
            f,
            "Vault {{ locked: {}, secrets: {} }}",
            i.key.is_none(),
            i.secrets.len()
        )
    }
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

/// The marker the model writes: `{{secreto:name}}`.
pub fn marker(name: &str) -> String {
    format!("{{{{secreto:{name}}}}}")
}

impl Vault {
    /// A new vault protected by `passphrase`. Starts unlocked, with the person's capability.
    /// `path` is where the encrypted file lives; `None` keeps everything in memory.
    pub fn init(
        path: Option<&Path>,
        passphrase: &str,
        clock: Arc<dyn Clock>,
    ) -> Result<(Self, PersonCap)> {
        if path.is_some_and(Path::exists) {
            return Err(VaultError::Io("vault file already exists".into()));
        }
        let salt = crypto::random::<16>()?.to_vec();
        let key = crypto::derive(passphrase, &salt)?;
        let v = Self::build(path, clock, salt, crypto::seal(&key, b"check", FILE_CHECK)?);
        {
            let mut i = v.inner();
            i.key = Some(key);
            i.locked_since = None;
        }
        v.save(&v.inner())?;
        Ok((v, PersonCap(())))
    }

    /// Opens an existing vault file. It starts locked.
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| VaultError::Io(e.to_string()))?;
        let data: FileData = serde_json::from_slice(&bytes).map_err(|_| VaultError::Format)?;
        let v = Self::build(Some(path), clock, data.salt, data.check);
        v.inner().secrets = data
            .secrets
            .into_iter()
            .map(|s| (s.meta.name.clone(), s))
            .collect();
        Ok(v)
    }

    fn build(path: Option<&Path>, clock: Arc<dyn Clock>, salt: Vec<u8>, check: Vec<u8>) -> Self {
        let now = clock.now_ms();
        Self {
            scrubber: Scrubber::new(clock.clone()),
            clock,
            path: path.map(Path::to_path_buf),
            inner: Mutex::new(Inner {
                key: None,
                salt,
                check,
                secrets: BTreeMap::new(),
                tokens: HashMap::new(),
                audit: Vec::new(),
                alerts: Vec::new(),
                resolutions: BTreeMap::new(),
                denials: 0,
                locked_since: Some(now),
                locked_total_ms: 0,
            }),
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The filter every output goes through (S-20). It keeps working while locked, but only
    /// knows the values after the first unlock.
    pub fn scrubber(&self) -> Scrubber {
        self.scrubber.clone()
    }

    /// Unlocks with the passphrase (terminal in this phase). Teaches the scrubber every value.
    pub fn unlock(&self, passphrase: &str) -> Result<PersonCap> {
        let mut i = self.inner();
        let key = crypto::derive(passphrase, &i.salt)?;
        if crypto::open(&key, b"check", &i.check).as_deref() != Some(FILE_CHECK) {
            return Err(VaultError::BadPassphrase);
        }
        for s in i.secrets.values() {
            if let Some(v) = crypto::open(&key, s.meta.name.as_bytes(), &s.blob) {
                self.scrubber
                    .add(&s.meta.name, &String::from_utf8_lossy(&v));
            }
        }
        i.key = Some(key);
        if let Some(since) = i.locked_since.take() {
            i.locked_total_ms += self.clock.now_ms() - since;
        }
        Ok(PersonCap(()))
    }

    pub fn lock(&self) {
        let mut i = self.inner();
        i.key = None;
        i.tokens.clear();
        i.locked_since.get_or_insert(self.clock.now_ms());
    }

    pub fn is_locked(&self) -> bool {
        self.inner().key.is_none()
    }

    /// Writes the encrypted file: temp file with mode 0600, then rename.
    fn save(&self, i: &Inner) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let data = FileData {
            salt: i.salt.clone(),
            check: i.check.clone(),
            secrets: i
                .secrets
                .values()
                .map(|s| Stored {
                    meta: s.meta.clone(),
                    blob: s.blob.clone(),
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&data).map_err(|e| VaultError::Io(e.to_string()))?;
        let tmp = path.with_extension("tmp");
        write_private(&tmp, &bytes).map_err(|e| VaultError::Io(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| VaultError::Io(e.to_string()))
    }

    /// Stores or replaces a secret (only the person).
    pub fn put(&self, _cap: &PersonCap, spec: SecretSpec) -> Result<()> {
        if !valid_name(&spec.name) {
            return Err(VaultError::InvalidName);
        }
        if spec.value.len() < scrub::MIN_VALUE_LEN {
            return Err(VaultError::InvalidValue);
        }
        let mut i = self.inner();
        let key = i.key.ok_or(VaultError::Locked)?;
        let blob = crypto::seal(&key, spec.name.as_bytes(), spec.value.as_bytes())?;
        self.scrubber.add(&spec.name, &spec.value);
        let meta = Meta {
            name: spec.name.clone(),
            kind: spec.kind,
            destinations: spec.destinations,
            scope: spec.scope,
            rotate_every_ms: spec.rotate_every_ms,
            expires_at_ms: spec.expires_at_ms,
            rotated_at_ms: self.clock.now_ms(),
            state: State::Active,
            last_used_ms: None,
        };
        i.secrets.insert(spec.name, Stored { meta, blob });
        self.save(&i)
    }

    /// Replaces the value of an existing secret and restarts its rotation clock. The old value
    /// stays in the scrubber.
    pub fn rotate(&self, cap: &PersonCap, name: &str, new_value: &str) -> Result<()> {
        let mut meta = self.get_meta(name)?;
        meta.state = State::Active;
        self.put(
            cap,
            SecretSpec {
                name: name.to_string(),
                value: new_value.to_string(),
                kind: meta.kind,
                destinations: meta.destinations,
                scope: meta.scope,
                rotate_every_ms: meta.rotate_every_ms,
                expires_at_ms: meta.expires_at_ms,
            },
        )
    }

    /// Takes effect at once: the next `resolve` or token use fails.
    pub fn revoke(&self, _cap: &PersonCap, name: &str) -> Result<()> {
        let mut i = self.inner();
        i.secrets
            .get_mut(name)
            .ok_or(VaultError::NotFound)?
            .meta
            .state = State::Revoked;
        self.save(&i)
    }

    fn get_meta(&self, name: &str) -> Result<Meta> {
        let i = self.inner();
        Ok(i.secrets
            .get(name)
            .ok_or(VaultError::NotFound)?
            .meta
            .clone())
    }

    /// Names and metadata. Never values.
    pub fn list(&self, _cap: &PersonCap) -> Vec<Meta> {
        self.inner()
            .secrets
            .values()
            .map(|s| s.meta.clone())
            .collect()
    }

    pub fn audit(&self, _cap: &PersonCap) -> Vec<AuditEntry> {
        self.inner().audit.clone()
    }

    pub fn alerts(&self) -> Vec<Alert> {
        self.inner().alerts.clone()
    }

    /// Gives the gateway the value for `tool` going to `domain`, in memory only (S-19).
    pub fn resolve(
        &self,
        _cap: &GatewayCap,
        name: &str,
        tool: &str,
        domain: &str,
        trace_id: &str,
    ) -> Result<SecretValue> {
        self.resolve_inner(name, tool, domain, trace_id, false)
    }

    /// A token for `name` that only works with `tool` until `now + ttl_ms` (S-21).
    pub fn issue_token(
        &self,
        _cap: &GatewayCap,
        name: &str,
        tool: &str,
        node: &str,
        ttl_ms: i64,
    ) -> Result<Token> {
        let mut i = self.inner();
        if i.key.is_none() {
            return Err(VaultError::Locked);
        }
        let meta = &i.secrets.get(name).ok_or(VaultError::NotFound)?.meta;
        if meta.state == State::Revoked {
            return Err(VaultError::Revoked);
        }
        if !meta.destinations.iter().any(|d| d.tool == tool) {
            return Err(VaultError::Denied(format!(
                "{tool} is not a destination tool"
            )));
        }
        let id: String = crypto::random::<16>()?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let expires_at_ms = self.clock.now_ms() + ttl_ms;
        i.tokens.insert(
            format!("ctk_{id}"),
            TokenRec {
                secret: name.to_string(),
                tool: tool.to_string(),
                expires_at_ms,
            },
        );
        Ok(Token {
            id: format!("ctk_{id}"),
            node: node.to_string(),
            expires_at_ms,
        })
    }

    /// Resolves through a token; the token fixes the secret and the tool, the caller still names
    /// the destination domain, which is checked as in `resolve`.
    pub fn resolve_with_token(
        &self,
        _cap: &GatewayCap,
        token: &str,
        tool: &str,
        domain: &str,
        trace_id: &str,
    ) -> Result<SecretValue> {
        let name = {
            let mut i = self.inner();
            let now = self.clock.now_ms();
            let Some(rec) = i.tokens.get(token) else {
                return Err(VaultError::BadToken);
            };
            if now >= rec.expires_at_ms {
                i.tokens.remove(token);
                return Err(VaultError::BadToken);
            }
            if rec.tool != tool {
                return Err(VaultError::BadToken);
            }
            rec.secret.clone()
        };
        self.resolve_inner(&name, tool, domain, trace_id, true)
    }

    fn resolve_inner(
        &self,
        name: &str,
        tool: &str,
        domain: &str,
        trace_id: &str,
        via_token: bool,
    ) -> Result<SecretValue> {
        let mut i = self.inner();
        let now = self.clock.now_ms();
        let verdict = match (&i.key, i.secrets.get(name)) {
            (None, _) => Err(VaultError::Locked),
            (_, None) => Err(VaultError::NotFound),
            (_, Some(s)) if s.meta.state == State::Revoked => Err(VaultError::Revoked),
            (_, Some(s)) if s.meta.expires_at_ms.is_some_and(|e| now >= e) => {
                Err(VaultError::Expired)
            }
            (_, Some(s)) if !s.meta.destinations.iter().any(|d| d.allows(tool, domain)) => Err(
                VaultError::Denied(format!("{name} is not allowed for {tool} -> {domain}")),
            ),
            (Some(key), Some(s)) => crypto::open(key, name.as_bytes(), &s.blob)
                .map(|v| String::from_utf8_lossy(&v).into_owned())
                .ok_or(VaultError::Crypto),
        };
        let entry = |outcome| AuditEntry {
            at_ms: now,
            name: name.to_string(),
            tool: tool.to_string(),
            domain: domain.to_string(),
            trace_id: trace_id.to_string(),
            via_token,
            outcome,
        };
        match verdict {
            Ok(value) => {
                i.audit.push(entry(Outcome::Allowed));
                *i.resolutions
                    .entry(format!("{name} -> {domain}"))
                    .or_default() += 1;
                if let Some(s) = i.secrets.get_mut(name) {
                    s.meta.last_used_ms = Some(now);
                }
                Ok(SecretValue(value))
            }
            Err(e) => {
                i.audit.push(entry(Outcome::Denied(e.to_string())));
                i.denials += 1;
                if matches!(e, VaultError::Denied(_)) {
                    i.alerts.push(Alert {
                        at_ms: now,
                        name: name.to_string(),
                        tool: tool.to_string(),
                        domain: domain.to_string(),
                        trace_id: trace_id.to_string(),
                        reason: e.to_string(),
                    });
                }
                Err(e)
            }
        }
    }

    /// An encrypted copy of every secret, sealed with a fresh random key that is NOT the master
    /// key. Keep the key apart from the backup.
    pub fn backup(&self, _cap: &PersonCap) -> Result<(Vec<u8>, BackupKey)> {
        let i = self.inner();
        let key = i.key.ok_or(VaultError::Locked)?;
        let mut items = Vec::new();
        for s in i.secrets.values() {
            let v =
                crypto::open(&key, s.meta.name.as_bytes(), &s.blob).ok_or(VaultError::Crypto)?;
            items.push((s.meta.clone(), String::from_utf8_lossy(&v).into_owned()));
        }
        let plain = serde_json::to_vec(&items).map_err(|e| VaultError::Io(e.to_string()))?;
        let bk = BackupKey(crypto::random()?);
        let blob = crypto::seal(&bk.0, BACKUP_MAGIC, &plain)?;
        Ok(([BACKUP_MAGIC, &blob].concat(), bk))
    }

    /// Loads a backup into this (unlocked) vault. Wrong key or tampering gives `Format`.
    pub fn restore(&self, cap: &PersonCap, backup: &[u8], key: &BackupKey) -> Result<usize> {
        let blob = backup
            .strip_prefix(BACKUP_MAGIC)
            .ok_or(VaultError::Format)?;
        let plain = crypto::open(&key.0, BACKUP_MAGIC, blob).ok_or(VaultError::Format)?;
        let items: Vec<(Meta, String)> =
            serde_json::from_slice(&plain).map_err(|_| VaultError::Format)?;
        let n = items.len();
        for (m, value) in items {
            self.put(
                cap,
                SecretSpec {
                    name: m.name,
                    value,
                    kind: m.kind,
                    destinations: m.destinations,
                    scope: m.scope,
                    rotate_every_ms: m.rotate_every_ms,
                    expires_at_ms: m.expires_at_ms,
                },
            )?;
        }
        Ok(n)
    }

    /// Counters. `soon_ms` is the window for "about to expire or be rotated".
    pub fn telemetry(&self, soon_ms: i64) -> Telemetry {
        let i = self.inner();
        let now = self.clock.now_ms();
        let soon = |m: &Meta| {
            let exp = m.expires_at_ms;
            let rot = m.rotate_every_ms.map(|r| m.rotated_at_ms + r);
            m.state == State::Active && [exp, rot].into_iter().flatten().any(|t| t - now <= soon_ms)
        };
        Telemetry {
            resolutions: i.resolutions.clone(),
            denials: i.denials,
            redactions: self.scrubber.redaction_count(),
            expiring: i.secrets.values().filter(|s| soon(&s.meta)).count(),
            locked_ms: i.locked_total_ms + i.locked_since.map_or(0, |s| now - s),
        }
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}
