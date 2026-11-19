//! MCP servers (stage 4, S-70): someone else's program, from npm at an exact version, that the
//! person approved and that runs in a service's jail behind the tool host. What the tool host
//! does with them:
//!
//! - `mcp_propose`: the model's only way to ask for a server (or a new version of one). Checked
//!   here (a package name by npm's rules, an exact version, hosts by name, each secret for one of
//!   them, read and write paths, limits), with npm's facts about it fetched by the person's npm
//!   in its jail (`McpPort::npm_view`), and set aside for the person as class `mcp`. Nothing is
//!   downloaded or run on the model's say-so; installing, probing, turning on and the tools'
//!   modes are the person's `clio`'s alone.
//! - `mcp_tools`: the servers that are on and their tools (outside text: judged, marking).
//! - `mcp_call`: one tool of one server. A `free` tool runs; an `ask` one, a tool that changed
//!   since the person confirmed it, or any call of a `suspect` session is set aside (`send`)
//!   with the exact server, tool and arguments. The result is outside text.
//!
//! The client is here too: JSON-RPC 2.0 over the server's standard input and output, one message
//! a line; `initialize`, `notifications/initialized`, `tools/list` and `tools/call`, nothing else.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant};

use clio_gateway::wrap_untrusted;
use clio_vault::{Scrubber, count_masks};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::ask::Class;
use crate::host::{now_ms, private_dir};
use crate::judge::one_line;
use crate::services::{self, Limits, SecretUse};
use crate::{DecisionOut, Request, Response, SUSPECT_REASON, ToolHost, lock, sanitize, skills};

/// A server's main file in its versions folder: what the person approved.
pub const MANIFEST: &str = "mcp.json";
/// The only host npm reaches, for the facts and the install.
pub const REGISTRY: &str = "registry.npmjs.org";
pub const DEFAULT_LIMITS: Limits = Limits {
    cpu_percent: 50,
    memory_mb: 1024,
    seconds: 60,
};
/// `seconds` is a call's time limit here.
pub const MAX_LIMITS: Limits = Limits {
    cpu_percent: 100,
    memory_mb: 2048,
    seconds: 300,
};
/// The least memory a server gets: Node does not start in less.
pub const MIN_MEMORY_MB: u64 = 512;
/// A server nobody called for this long is stopped.
pub const IDLE: Duration = Duration::from_secs(600);
/// The protocol version Clio asks for; the server answers with its own.
pub const PROTOCOL: &str = "2025-06-18";
/// A line from a server longer than this ends it.
pub const MAX_LINE: usize = 2 << 20;
/// What a call may read from a server in all, and how many messages, before it is ended.
const MAX_CALL_BYTES: usize = 16 << 20;
const MAX_MESSAGES: usize = 10_000;
/// Lines that are not JSON, and requests of the server's, a call puts up with.
const MAX_BAD: usize = 100;
/// A call's arguments, as JSON, at most.
pub const MAX_ARGS: usize = 32 * 1024;
/// What reaches the model of a result.
const MAX_RESULT: usize = 48 * 1024;
/// What is kept of a tool: its description, its schema as JSON; how many tools, pages.
const MAX_DESCRIPTION: usize = 2000;
const MAX_SCHEMA: usize = 16 * 1024;
const MAX_TOOLS: usize = 100;
const MAX_PAGES: usize = 10;
const MAX_REASON: usize = 300;
/// What is kept of the registry's text: each string, the maintainers listed.
const MAX_FACT: usize = 200;
/// What is kept of the package's own `license` (its words, not the registry's).
const MAX_LICENSE: usize = 40;
/// The fields `npm view` is asked for: only these, so its answer stays small (no `versions` or
/// `time` of the whole package).
pub const NPM_FIELDS: [&str; 10] = [
    "name",
    "version",
    "dist.integrity",
    "dist.tarball",
    "maintainers",
    "_npmUser",
    "license",
    "repository",
    "deprecated",
    "dependencies",
];
/// A server's fixed arguments and settings: how many, and how long each.
const MAX_ARGS_N: usize = 20;
const MAX_ARG: usize = 200;
const MAX_ENV_N: usize = 20;
const MAX_ENV_VALUE: usize = 500;
/// Proposals a session may make in an hour (counted before npm runs).
pub const PROPOSALS_PER_HOUR: usize = 6;
const MAX_MAINTAINERS: usize = 10;
/// Events kept in a server's log.
const KEPT_EVENTS: usize = 50;

/// `mcp.json`, as approved: written by `compose`, read back only if it is exactly that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    /// The npm package, by npm's rules (`@scope/name` or `name`).
    pub package: String,
    /// Exact: `1.2.3` or `1.2.3-beta.1`, never a range, a tag, a URL or a path.
    pub version: String,
    /// Fixed arguments, passed after the entry point (never a secret).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Settings that are not secret, as variables (never `NODE_*`, `PATH`, `HOME`, `LD_*`, nor a
    /// secret's variable).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The only hosts it may reach, through the proxy (none: no network).
    pub network: Vec<String>,
    pub secrets: Vec<SecretUse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    /// Processor and memory for the process; `seconds` for each call.
    pub limits: Limits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Runs when the model calls it.
    Free,
    /// Waits for the person each time.
    Ask,
}

/// One tool of a server, as it described itself (outside text, cleaned and capped) and as the
/// person set it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub schema: Value,
    /// The server says it only reads (`annotations.readOnlyHint`).
    #[serde(default)]
    pub read_only: bool,
    pub mode: Mode,
    /// New or changed since the person confirmed the list: it asks, whatever its mode.
    #[serde(default)]
    pub flagged: bool,
}

/// `<mcp>/state/<name>.json`: what the person's `clio` installed and probed, and what the person
/// confirmed. Written by the person's `clio`; the tool host only flags a changed tool here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub name: String,
    pub package: String,
    pub version: String,
    /// The file `node` starts, relative to the installed folder.
    pub entry: String,
    /// The installed tree's digest: checked before each start.
    pub tree: String,
    /// The tarball's integrity, as the registry gave it and the lockfile recorded it.
    pub integrity: String,
    /// How many packages npm installed in all (the package and every dependency).
    #[serde(default)]
    pub packages: usize,
    /// The person confirmed the tool list: only then can it be turned on.
    pub confirmed: bool,
    pub tools: Vec<Tool>,
}

/// One line of a server's log (`<mcp>/logs/<name>.json`): never arguments, never results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub at_ms: i64,
    pub what: String,
    pub outcome: String,
    #[serde(default)]
    pub why: String,
}

/// A started server's process, from the port: the tool host talks to it and kills it.
pub struct Proc {
    pub child: Child,
    /// Whatever must live as long as the process (the proxy of its hosts).
    pub keep: Option<Box<dyn Send>>,
    /// Its secrets' values: struck out of everything it says.
    pub scrub: Scrubber,
    /// Written first on its input (its secrets, NUL-terminated), then zeroed.
    pub input: Vec<u8>,
}

/// What only the person's `clio` can do, for the tool host (the binary's `cli::mcp_servers`).
pub trait McpPort: Send + Sync {
    /// `<data>/mcp`: records, versions, state, logs.
    fn root(&self) -> PathBuf;
    /// npm's facts about `package@version` (`npm view --json`), from the person's npm in its jail
    /// with only the registry reachable. Outside text. Nothing is downloaded but this.
    fn npm_view(&self, package: &str, version: &str) -> Result<Value, String>;
    /// Starts server `name` in its jail: only one that is on, whose tools the person confirmed
    /// and whose installed files are as they were (else it is turned off, with a notice).
    fn start(&self, name: &str) -> Result<Proc, String>;
}

/// A package name by npm's rules, narrowed: `name` or `@scope/name`, each part lowercase letters,
/// digits, `-`, `.` and `_`, starting with a letter or a digit, at most 214 in all. Nothing that
/// npm would read as a URL, a git source, a path, a tag, a range or an option.
pub fn valid_package(p: &str) -> bool {
    let part = |s: &str| {
        !s.is_empty()
            && !s.starts_with(['.', '_', '-'])
            && s.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_')
            })
    };
    let name = match p.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, name)) if part(scope) => name,
            _ => return false,
        },
        None => p,
    };
    p.len() <= 214
        && part(name)
        && !name.contains('/')
        && !matches!(name, "node_modules" | "favicon.ico")
}

/// An exact semantic version: `MAJOR.MINOR.PATCH`, numbers without leading zeros, and maybe a
/// `-prerelease` of dot-separated identifiers. No `v`, no build part, no range, no tag.
pub fn exact_version(v: &str) -> bool {
    let num = |s: &str| {
        !s.is_empty()
            && s.len() <= 9
            && s.bytes().all(|b| b.is_ascii_digit())
            && (s == "0" || !s.starts_with('0'))
    };
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    v.len() <= 64
        && parts.len() == 3
        && parts.iter().all(|p| num(p))
        && pre.is_none_or(|p| {
            p.split('.').all(|id| {
                !id.is_empty()
                    && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && (!id.bytes().all(|b| b.is_ascii_digit()) || num(id))
            })
        })
}

/// The manifest's own shape (the paths are checked against a home apart).
pub fn check(m: &Manifest) -> Result<(), String> {
    if !skills::valid_name(&m.name) {
        return Err(
            "name: lowercase letters, digits and single hyphens, at most 64 (like time)".into(),
        );
    }
    let dl = m.description.chars().count();
    if !(skills::MIN_DESCRIPTION..=skills::MAX_DESCRIPTION).contains(&dl)
        || m.description.chars().any(skills::hidden)
        || m.description.contains('\n')
    {
        return Err(format!(
            "description: one line of {} to {} characters that says what it is for",
            skills::MIN_DESCRIPTION,
            skills::MAX_DESCRIPTION
        ));
    }
    if !valid_package(&m.package) {
        return Err(format!(
            "package: {:?} is not an npm package name (lowercase, like mcp-server-time or @scope/server-name; no URL, git source, path or tag)",
            one_line(&m.package)
        ));
    }
    if !exact_version(&m.version) {
        return Err(format!(
            "version: {:?} is not an exact version (like 1.2.3, never a range, a tag or latest)",
            one_line(&m.version)
        ));
    }
    check_settings(m)?;
    // Node does not start in less.
    if m.limits.memory_mb < MIN_MEMORY_MB {
        return Err(format!(
            "limits: memory_mb at least {MIN_MEMORY_MB} (Node needs it)"
        ));
    }
    services::check_grants(
        &m.network,
        &m.secrets,
        m.writes.len().max(m.reads.len()),
        m.limits,
        MAX_LIMITS,
    )
}

/// `args` and `env`: few, short, one line each, no hidden character; the variables' names by the
/// secrets' rules, never Node's own (`NODE_*`: options, CA files, TLS checks), and never a
/// secret's. Text shaped like a secret is refused with the whole manifest (`mcp_propose`).
fn check_settings(m: &Manifest) -> Result<(), String> {
    let plain = |t: &str, max: usize| {
        t.chars().count() <= max && !t.chars().any(|c| c.is_control() || skills::hidden(c))
    };
    if m.args.len() > MAX_ARGS_N || m.args.iter().any(|a| a.is_empty() || !plain(a, MAX_ARG)) {
        return Err(format!(
            "args: at most {MAX_ARGS_N}, each one line of 1 to {MAX_ARG} characters (never a secret: declare it in secrets)"
        ));
    }
    if m.env.len() > MAX_ENV_N {
        return Err(format!("env: at most {MAX_ENV_N} settings"));
    }
    for (k, v) in &m.env {
        if !services::valid_env(k)
            || k.starts_with("NODE_")
            || m.secrets.iter().any(|s| s.env == *k)
        {
            return Err(format!(
                "env: {:?} must be UPPER_CASE, not one the shell, Node or Clio sets (PATH, HOME, LD_*, NODE_*, *_PROXY, CLIO_*...) and not a secret's",
                one_line(k)
            ));
        }
        if !plain(v, MAX_ENV_VALUE) {
            return Err(format!(
                "env: {:?} is one line of at most {MAX_ENV_VALUE} characters (never a secret: declare it in secrets)",
                one_line(k)
            ));
        }
    }
    Ok(())
}

/// The manifest's text: what is shown, approved, hashed and installed.
pub fn compose(m: &Manifest) -> String {
    format!("{}\n", serde_json::to_string_pretty(m).unwrap_or_default())
}

/// A stored manifest: one `compose` wrote, of this name, that checks.
pub fn check_stored(
    name: &str,
    manifest: &str,
    files: &[skills::SkillFile],
) -> Result<Manifest, String> {
    let m: Manifest =
        serde_json::from_str(manifest).map_err(|e| format!("{MANIFEST} does not read: {e}"))?;
    if m.name != name || compose(&m) != manifest || !files.is_empty() {
        return Err(format!(
            "{MANIFEST} is not what Clio wrote for «{}»",
            one_line(name)
        ));
    }
    check(&m)?;
    Ok(m)
}

/// The manifest the model's arguments describe (not checked yet).
pub fn manifest_of(args: &Map<String, Value>) -> Result<Manifest, String> {
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    Ok(Manifest {
        name: s("name"),
        description: s("description"),
        package: s("package"),
        version: s("version"),
        args: services::strings(args.get("args"), "args")?,
        env: match args.get("env") {
            None | Some(Value::Null) => BTreeMap::new(),
            Some(Value::Object(o)) => o
                .iter()
                .map(|(k, v)| {
                    v.as_str()
                        .map(|v| (k.clone(), v.to_owned()))
                        .ok_or_else(|| "env: an object of text values".to_owned())
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("env: an object of text values".into()),
        },
        network: services::strings(args.get("network"), "network")?,
        secrets: services::secrets_of(args.get("secrets"))?,
        reads: services::strings(args.get("reads"), "reads")?,
        writes: services::strings(args.get("writes"), "writes")?,
        limits: services::limits_of(args.get("limits"), DEFAULT_LIMITS)?,
    })
}

/// What is shown of npm's facts (`npm view <package>@<version> <NPM_FIELDS> --json`: an object
/// whose `dist` fields come as `"dist.integrity"`, maintainers as `"name <email>"`): outside text,
/// each string cleaned by `clean` and cut, people by name only, the maintainers capped, the
/// package's own `license` cut to 40. `Err` when it is not the asked version or has no integrity.
pub fn npm_facts(
    v: &Value,
    package: &str,
    version: &str,
    clean: &dyn Fn(&str) -> String,
) -> Result<Value, String> {
    // `npm view` of several versions answers a list: the asked one only.
    let v = match v {
        Value::Array(a) => a
            .iter()
            .find(|x| x["version"] == version)
            .unwrap_or(&Value::Null),
        v => v,
    };
    if v["name"] != package || v["version"] != version {
        return Err(format!(
            "the registry has no {package}@{version} (check the name and that the version exists)"
        ));
    }
    let cut = |s: &str, max: usize| -> String { clean(s).chars().take(max).collect() };
    let text = |x: &Value, key: &str| -> String {
        match x {
            Value::String(s) => cut(s, MAX_FACT),
            Value::Object(o) => o
                .get(key)
                .and_then(Value::as_str)
                .map(|s| cut(s, MAX_FACT))
                .unwrap_or_default(),
            _ => String::new(),
        }
    };
    // A person: `name <email> (url)` or `{name, email}`; the name only.
    let person = |x: &Value| -> String {
        let t = text(x, "name");
        t.split(['<', '(']).next().unwrap_or("").trim().to_owned()
    };
    let integrity = v
        .get("dist.integrity")
        .unwrap_or(&v["dist"]["integrity"])
        .as_str()
        .unwrap_or("");
    if !integrity.starts_with("sha512-")
        || !integrity[7..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    {
        return Err(format!(
            "the registry gave no sha512 integrity for {package}@{version}: it is not installed"
        ));
    }
    let maintainers: Vec<String> = match &v["maintainers"] {
        Value::Array(a) => a.iter().map(person).filter(|s| !s.is_empty()).collect(),
        x => Some(person(x))
            .filter(|s| !s.is_empty())
            .into_iter()
            .collect(),
    };
    Ok(json!({
        "publisher": person(&v["_npmUser"]),
        "maintainers": maintainers.iter().take(MAX_MAINTAINERS).collect::<Vec<_>>(),
        "more_maintainers": maintainers.len().saturating_sub(MAX_MAINTAINERS),
        "integrity": integrity,
        "dependencies": v["dependencies"].as_object().map_or(0, Map::len),
        "license": cut(&text(&v["license"], "type"), MAX_LICENSE),
        "repository": text(&v["repository"], "url"),
        "deprecated": text(&v["deprecated"], ""),
    }))
}

// ---- the folder ------------------------------------------------------------------------------

/// The person's `clio` and the tool host change a server's files one at a time (`<mcp>/.lock`,
/// the lock `skills::lock_in` takes for this folder too).
pub fn lock_root(root: &Path) -> Result<fs::File, String> {
    private_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let p = root.join(".lock");
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&p)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    rustix::fs::flock(&f, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(f)
}

/// One npm at a time on the machine (`<mcp>/npm.lock`): `wait` for it (an install, the person's),
/// or `None` at once when another npm runs (a proposal: the model's, within its tool's time).
pub fn lock_npm(root: &Path, wait: bool) -> Result<Option<fs::File>, String> {
    private_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let p = root.join("npm.lock");
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&p)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    let how = if wait {
        rustix::fs::FlockOperation::LockExclusive
    } else {
        rustix::fs::FlockOperation::NonBlockingLockExclusive
    };
    match rustix::fs::flock(&f, how) {
        Ok(()) => Ok(Some(f)),
        Err(rustix::io::Errno::WOULDBLOCK) if !wait => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn write_json(p: &Path, v: &impl Serialize) -> Result<(), String> {
    let e = |x: std::io::Error| format!("{}: {x}", p.display());
    if let Some(d) = p.parent() {
        private_dir(d).map_err(e)?;
    }
    let tmp = p.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(v).map_err(|x| x.to_string())?;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(&body))
        .and_then(|()| fs::rename(&tmp, p))
        .map_err(e)
}

fn state_path(root: &Path, name: &str) -> PathBuf {
    root.join("state").join(format!("{name}.json"))
}

pub fn load_state(root: &Path, name: &str) -> Option<State> {
    if !skills::valid_name(name) {
        return None;
    }
    serde_json::from_str(&fs::read_to_string(state_path(root, name)).ok()?).ok()
}

/// Written whole; the caller holds `lock_root`.
pub fn save_state(root: &Path, s: &State) -> Result<(), String> {
    write_json(&state_path(root, &s.name), s)
}

/// `<mcp>/records/<name>.json` (the person's `clio` writes it), if registered.
pub fn record(root: &Path, name: &str) -> Option<Value> {
    if !skills::valid_name(name) {
        return None;
    }
    serde_json::from_str(
        &fs::read_to_string(root.join("records").join(format!("{name}.json"))).ok()?,
    )
    .ok()
}

/// The approved manifest of `name`'s current version.
pub fn current_manifest(root: &Path, name: &str) -> Option<Manifest> {
    let rec = record(root, name)?;
    let n = rec["current"].as_u64()?;
    let t = fs::read_to_string(
        root.join("versions")
            .join(name)
            .join(n.to_string())
            .join(MANIFEST),
    )
    .ok()?;
    check_stored(name, &t, &[]).ok()
}

/// Whether `name` may start: on, and its tools confirmed.
pub fn is_on(root: &Path, name: &str) -> bool {
    record(root, name).is_some_and(|r| r["enabled"] == Value::Bool(true))
        && load_state(root, name).is_some_and(|s| s.confirmed)
}

/// Every registered server's name, sorted.
pub fn names(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root.join("records"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_str()?.strip_suffix(".json")?.to_owned();
            skills::valid_name(&n).then_some(n)
        })
        .collect();
    v.sort();
    v
}

pub fn events(root: &Path, name: &str) -> Vec<Event> {
    fs::read_to_string(root.join("logs").join(format!("{name}.json")))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// One line in `name`'s log (the last 50 are kept). Best effort.
pub fn log_event(root: &Path, name: &str, what: &str, outcome: &str, why: &str) {
    if !skills::valid_name(name) {
        return;
    }
    let Ok(_l) = lock_root(root) else { return };
    let mut all = events(root, name);
    all.push(Event {
        at_ms: now_ms(),
        what: what.into(),
        outcome: outcome.into(),
        why: one_line(&sanitize(why)),
    });
    let keep = all.len().saturating_sub(KEPT_EVENTS);
    all.drain(..keep);
    if let Err(e) = write_json(&root.join("logs").join(format!("{name}.json")), &all) {
        eprintln!("clio-toolhost: a server's log lost a line: {e}");
    }
}

/// The tools a start found, against the stored ones: one whose description or schema changed, or
/// a new one, is `ask` and flagged; the others keep their mode and flag. Gone ones are dropped.
pub fn observe(stored: &[Tool], seen: Vec<Tool>) -> Vec<Tool> {
    seen.into_iter()
        .map(|mut t| {
            match stored.iter().find(|s| s.name == t.name) {
                Some(s) if s.description == t.description && s.schema == t.schema => {
                    t.mode = s.mode;
                    t.flagged = s.flagged;
                }
                _ => {
                    t.mode = Mode::Ask;
                    t.flagged = true;
                }
            }
            t
        })
        .collect()
}

/// The digest of a tool list as the person is shown it to confirm (names, descriptions, schemas,
/// modes, flags): confirming is refused if the stored list is no longer this.
pub fn tools_digest(tools: &[Tool]) -> String {
    skills::digest(&serde_json::to_string(tools).unwrap_or_default(), &[])
}

/// The modes a first probe gives: `free` where the server says the tool only reads, else `ask`.
pub fn first_modes(seen: Vec<Tool>) -> Vec<Tool> {
    seen.into_iter()
        .map(|mut t| {
            t.mode = if t.read_only { Mode::Free } else { Mode::Ask };
            t.flagged = false;
            t
        })
        .collect()
}

/// `server · tool {arguments}`: what a call set aside shows, and must still say when approved.
pub fn call_text(server: &str, tool: &str, arguments: &Value) -> String {
    format!(
        "MCP server «{server}», tool «{tool}», arguments {}",
        serde_json::to_string(arguments).unwrap_or_default()
    )
}

/// A small check of `arguments` against a tool's input schema: an object, no larger than
/// `MAX_ARGS`, every required field there, each top-level field of the type the schema gives,
/// and no other field when the schema says so.
pub fn check_arguments(schema: &Value, arguments: &Value) -> Result<(), String> {
    let Some(a) = arguments.as_object() else {
        return Err("arguments: a JSON object".into());
    };
    if serde_json::to_string(arguments).map_or(usize::MAX, |s| s.len()) > MAX_ARGS {
        return Err(format!("arguments: at most {MAX_ARGS} bytes as JSON"));
    }
    for r in schema["required"].as_array().into_iter().flatten() {
        if let Some(r) = r.as_str()
            && !a.contains_key(r)
        {
            return Err(format!("arguments: {:?} is required", one_line(r)));
        }
    }
    let props = schema["properties"].as_object();
    for (k, v) in a {
        let Some(p) = props.and_then(|p| p.get(k)) else {
            if schema["additionalProperties"] == Value::Bool(false) {
                return Err(format!(
                    "arguments: {:?} is not one of the tool's",
                    one_line(k)
                ));
            }
            continue;
        };
        let types: Vec<&str> = match &p["type"] {
            Value::String(t) => vec![t.as_str()],
            Value::Array(t) => t.iter().filter_map(Value::as_str).collect(),
            _ => continue,
        };
        let fits = |t: &str| match t {
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "object" => v.is_object(),
            "array" => v.is_array(),
            "null" => v.is_null(),
            _ => true,
        };
        if !types.iter().any(|t| fits(t)) {
            return Err(format!(
                "arguments: {:?} must be {}",
                one_line(k),
                types.join(" or ")
            ));
        }
    }
    Ok(())
}

// ---- the client ------------------------------------------------------------------------------

enum Msg {
    Line(Vec<u8>),
    TooLong,
    End,
}

/// A running server and the client's side of its pipes. Dropped, it is killed.
pub struct Server {
    child: Child,
    _keep: Option<Box<dyn Send>>,
    pub scrub: Scrubber,
    tx: Sender<Vec<u8>>,
    rx: Receiver<Msg>,
    stderr: Arc<Mutex<Vec<u8>>>,
    next: u64,
    pub last_use: Instant,
    /// What `tools/list` gave at its start (cleaned, capped; modes not set).
    pub tools: Vec<Tool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.kill();
    }
}

fn read_lines(out: ChildStdout, tx: SyncSender<Msg>) {
    let mut r = BufReader::new(out);
    let mut line = Vec::new();
    loop {
        let buf = match r.fill_buf() {
            Ok([]) | Err(_) => {
                let _ = tx.send(Msg::End);
                return;
            }
            Ok(b) => b,
        };
        let (take, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        line.extend_from_slice(&buf[..take]);
        r.consume(take);
        if line.len() > MAX_LINE {
            let _ = tx.send(Msg::TooLong);
            return;
        }
        if done && tx.send(Msg::Line(std::mem::take(&mut line))).is_err() {
            return;
        }
    }
}

fn keep_tail(mut err: ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut buf = [0u8; 4096];
    while let Ok(n) = err.read(&mut buf) {
        if n == 0 {
            return;
        }
        let mut t = lock(&tail);
        t.extend_from_slice(&buf[..n]);
        let cut = t.len().saturating_sub(2000);
        t.drain(..cut);
    }
}

impl Server {
    /// Takes the process, says `initialize` and `notifications/initialized`, and lists its tools,
    /// all within `limit`. Any failure kills it.
    pub fn start(mut p: Proc, limit: Duration) -> Result<Self, String> {
        let (Some(mut stdin), Some(stdout), Some(stderr)) = (
            p.child.stdin.take(),
            p.child.stdout.take(),
            p.child.stderr.take(),
        ) else {
            let _ = p.child.kill();
            let _ = p.child.wait();
            return Err("the server's pipes are missing".into());
        };
        // Its writes come from a thread of their own: a server that does not read cannot block
        // the tool host. Its secrets first, then the messages.
        let (tx, wrx) = channel::<Vec<u8>>();
        let mut input = std::mem::take(&mut p.input);
        std::thread::spawn(move || {
            let first = stdin.write_all(&input).and_then(|()| stdin.flush());
            input.fill(0);
            if first.is_err() {
                return;
            }
            for line in wrx {
                if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
                    return;
                }
            }
        });
        // A few lines parked at most (each up to `MAX_LINE`): an idle server that keeps writing
        // waits on its pipe, not in the tool host's memory.
        let (ltx, rx) = sync_channel(4);
        std::thread::spawn(move || read_lines(stdout, ltx));
        let tail = Arc::new(Mutex::new(Vec::new()));
        let t2 = tail.clone();
        std::thread::spawn(move || keep_tail(stderr, t2));
        let mut s = Self {
            child: p.child,
            _keep: p.keep,
            scrub: p.scrub,
            tx,
            rx,
            stderr: tail,
            next: 0,
            last_use: Instant::now(),
            tools: vec![],
        };
        let until = Instant::now() + limit;
        if let Err(e) = s.handshake(until) {
            // Its last words, a moment after it ended, say why.
            std::thread::sleep(Duration::from_millis(100));
            let tail = s.stderr_tail();
            return Err(if tail.is_empty() {
                e
            } else {
                format!("{e} (its last words: {tail})")
            });
        }
        Ok(s)
    }

    /// `initialize`, `notifications/initialized` and the tool list, by `until`.
    fn handshake(&mut self, until: Instant) -> Result<(), String> {
        let init = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": "clio", "version": env!("CARGO_PKG_VERSION")},
            }),
            until,
        )?;
        if !init.is_object() {
            return Err("its answer to initialize is not an object".into());
        }
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |c| json!({"cursor": c}));
            let r = self.request("tools/list", params, until)?;
            for t in r["tools"].as_array().into_iter().flatten() {
                if self.tools.len() < MAX_TOOLS
                    && let Some(t) = tool_of(t)
                    && !self.tools.iter().any(|o| o.name == t.name)
                {
                    self.tools.push(t);
                }
            }
            cursor = r["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(())
    }

    /// The server's last words on its error stream, its secrets struck out, one line.
    pub fn stderr_tail(&self) -> String {
        let t = String::from_utf8_lossy(&lock(&self.stderr)).into_owned();
        let flat: String = sanitize(&self.scrub.scrub(&t))
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let n = flat.chars().count();
        flat.chars()
            .skip(n.saturating_sub(400))
            .collect::<String>()
            .trim()
            .to_owned()
    }

    pub fn kill(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // Its own pid namespace dies with its first process; the group kill is a belt.
            if let Ok(pid) = i32::try_from(self.child.id())
                && let Some(pid) = rustix::process::Pid::from_raw(pid)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }

    fn send(&self, v: &Value) -> Result<(), String> {
        let mut line = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        line.push(b'\n');
        self.tx
            .send(line)
            .map_err(|_| "the server stopped reading".to_owned())
    }

    /// One request and its answer, by `until`. What comes meanwhile: notifications and answers
    /// with another id are dropped, the server's own requests (sampling, roots, elicitation) are
    /// refused with an error, lines that are not JSON are skipped; too many of any, too much in
    /// all, a line over `MAX_LINE`, its end or the time limit fail the call.
    fn request(&mut self, method: &str, params: Value, until: Instant) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let (mut bytes, mut messages, mut bad) = (0usize, 0usize, 0usize);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let line = match self.rx.recv_timeout(left) {
                Ok(Msg::Line(l)) => l,
                Ok(Msg::TooLong) => return Err(format!("it wrote a line over {MAX_LINE} bytes")),
                Ok(Msg::End) | Err(RecvTimeoutError::Disconnected) => {
                    let code = self.child.try_wait().ok().flatten();
                    return Err(match code {
                        Some(c) => format!("it ended ({c})"),
                        None => "it closed its output".into(),
                    });
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err("it did not answer within its time limit".into());
                }
            };
            bytes += line.len();
            messages += 1;
            if bytes > MAX_CALL_BYTES || messages > MAX_MESSAGES {
                return Err("it sent too much without answering".into());
            }
            let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(&line) else {
                bad += 1;
                if bad > MAX_BAD {
                    return Err("it sent too many lines that are not JSON-RPC".into());
                }
                continue;
            };
            match (m.get("id"), m.get("method")) {
                // A request of the server's: Clio answers none.
                (Some(rid), Some(_)) => {
                    bad += 1;
                    if bad > MAX_BAD {
                        return Err("it sent too many requests of its own".into());
                    }
                    self.send(&json!({"jsonrpc": "2.0", "id": rid, "error": {
                        "code": -32601, "message": "Clio's client answers no requests"}}))?;
                }
                (Some(rid), None) if rid.as_u64() == Some(id) => {
                    if let Some(e) = m.get("error") {
                        let said = e["message"].as_str().unwrap_or("no message");
                        return Err(format!(
                            "it answered with an error: {}",
                            one_line(&sanitize(&self.scrub.scrub(said)))
                        ));
                    }
                    return Ok(m.get("result").cloned().unwrap_or(Value::Null));
                }
                _ => {}
            }
        }
    }

    /// `tools/call`: the result's content as text (outside text, its secrets struck out; not
    /// cleaned or cut yet), and whether the tool said it failed.
    pub fn call(
        &mut self,
        tool: &str,
        arguments: &Value,
        limit: Duration,
    ) -> Result<(String, bool), String> {
        self.last_use = Instant::now();
        let r = self.request(
            "tools/call",
            json!({"name": tool, "arguments": arguments}),
            Instant::now() + limit,
        )?;
        self.last_use = Instant::now();
        let mut parts = vec![];
        for c in r["content"].as_array().into_iter().flatten() {
            parts.push(match c["type"].as_str().unwrap_or("") {
                "text" => c["text"].as_str().unwrap_or("").to_owned(),
                "resource" => {
                    let res = &c["resource"];
                    match res["text"].as_str() {
                        Some(t) => format!("[resource {}]\n{t}", res["uri"].as_str().unwrap_or("")),
                        None => {
                            format!("[resource {}, not text]", res["uri"].as_str().unwrap_or(""))
                        }
                    }
                }
                "resource_link" => format!("[link {}]", c["uri"].as_str().unwrap_or("")),
                other => format!("[{} content, not shown]", one_line(other)),
            });
        }
        if parts.is_empty() && !r["structuredContent"].is_null() {
            parts.push(r["structuredContent"].to_string());
        }
        let text = parts.join("\n");
        Ok((self.scrub.scrub(&text), r["isError"] == Value::Bool(true)))
    }
}

/// A tool as `tools/list` gave it, kept only if its name is a plain one and its schema is small:
/// description cleaned and cut.
fn tool_of(t: &Value) -> Option<Tool> {
    let name = t["name"].as_str()?;
    let plain = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    let schema = t
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    if !plain || !schema.is_object() || schema.to_string().len() > MAX_SCHEMA {
        return None;
    }
    Some(Tool {
        name: name.to_owned(),
        description: sanitize(t["description"].as_str().unwrap_or(""))
            .chars()
            .take(MAX_DESCRIPTION)
            .collect(),
        schema,
        // Its own claim, and only when nothing it says contradicts it: a destructive or
        // open-world hint makes it a tool that asks.
        read_only: t["annotations"]["readOnlyHint"] == Value::Bool(true)
            && t["annotations"]["destructiveHint"] != Value::Bool(true)
            && t["annotations"]["openWorldHint"] != Value::Bool(true),
        mode: Mode::Ask,
        flagged: false,
    })
}

/// A started server, and what it was started as: when its record's version, its installed
/// version or its tree's digest is no longer this, or it is off, it is stopped.
pub(crate) struct Entry {
    server: Server,
    current: u64,
    version: String,
    tree: String,
}

/// One server's place in the pool: a call holds it (never the whole pool) while it starts or
/// calls the server, so a slow server keeps no other waiting.
type Slot = Arc<Mutex<Option<Entry>>>;

/// The servers a session started, each once, until it closes, they sit idle, or what they were
/// started as changes; and when each session proposed a server (the brake on `mcp_propose`).
#[derive(Default)]
pub(crate) struct Pool {
    servers: Arc<Mutex<HashMap<String, Slot>>>,
    proposals: Mutex<HashMap<String, Vec<Instant>>>,
}

/// What `name` would start as now: its record's version, the installed version and digest, if it
/// is on.
fn started_as(root: &Path, name: &str) -> Option<(u64, String, String)> {
    if !is_on(root, name) {
        return None;
    }
    let current = record(root, name)?["current"].as_u64()?;
    let st = load_state(root, name)?;
    Some((current, st.version, st.tree))
}

fn still_fresh(root: &Path, name: &str, e: &Entry) -> bool {
    e.server.last_use.elapsed() < IDLE
        && started_as(root, name)
            .is_some_and(|(c, v, t)| c == e.current && v == e.version && t == e.tree)
}

/// Stops the servers that sat idle or are no longer what they were started as (off, not
/// confirmed, removed, updated): those a call holds are left to it, which checks them itself.
fn prune(servers: &Mutex<HashMap<String, Slot>>, root: &Path) {
    lock(servers).retain(|name, slot| match slot.try_lock() {
        Ok(mut g) => {
            if g.as_ref().is_some_and(|e| !still_fresh(root, name, e)) {
                *g = None;
                log_event(
                    root,
                    name,
                    "stop",
                    "ok",
                    "idle, or no longer what was started",
                );
            }
            g.is_some()
        }
        Err(_) => true,
    });
}

impl Pool {
    /// Every half minute while the pool lives: `prune`.
    pub(crate) fn reap_idle(&self, root: PathBuf) {
        let weak = Arc::downgrade(&self.servers);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let Some(p) = weak.upgrade() else { return };
                prune(&p, &root);
            }
        });
    }

    /// A proposal of `session` now, if it is under `PROPOSALS_PER_HOUR` in the last hour.
    fn may_propose(&self, session: &str) -> bool {
        let mut all = lock(&self.proposals);
        let times = all.entry(session.to_owned()).or_default();
        times.retain(|t| t.elapsed() < Duration::from_secs(3600));
        if times.len() >= PROPOSALS_PER_HOUR {
            return false;
        }
        times.push(Instant::now());
        true
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    format!("{}\n[cut: {} more bytes]", &s[..i], s.len() - i)
}

impl ToolHost {
    /// Why there are no MCP servers here: a copy-mode chat, or a host whose side could not be set
    /// up (no bwrap, no npm beside Node).
    fn mcp_missing(&self) -> String {
        match &self.mcp_off {
            Some(why) => format!("MCP servers are not available here: {why}"),
            None => "MCP servers exist only in a chat on the person's machine (host mode)".into(),
        }
    }

    fn mcp_answer(&self, req: &Request, ok: bool, rule: &str, content: String) -> Response {
        Response {
            id: req.id.clone(),
            ok,
            content: content.clone(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: if ok { "allow" } else { "deny" }.into(),
                rule: rule.into(),
                reason: content,
            },
            tainted: self.is_tainted(&req.session),
            external: false,
        }
    }

    /// Outside text for the model: cleaned, cut, judged when the guard filters it, and marking.
    fn mcp_outside(
        &self,
        req: &Request,
        ok: bool,
        rule: &str,
        raw: &str,
        source: &str,
    ) -> Response {
        let text = cut(&self.clean(raw), MAX_RESULT);
        self.mark(&req.session, "mcp");
        self.judge_later(req, source);
        Response {
            id: req.id.clone(),
            ok,
            content: text.clone(),
            data: json!({ "masked": count_masks(&text).saturating_sub(count_masks(raw)) }),
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: rule.into(),
                reason: "an MCP server's text".into(),
            },
            tainted: self.is_tainted(&req.session),
            external: true,
        }
    }

    /// `mcp_propose`: checks the proposal, asks npm (the person's, jailed) about the package, and
    /// sets it aside for the person (`mcp`). Nothing is downloaded, installed or run.
    pub(crate) fn mcp_propose(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let fail = |why: &str| {
            self.mcp_answer(
                req,
                false,
                "mcp/propose",
                format!(
                    "mcp_propose failed: {}. Nothing was proposed.",
                    one_line(&self.clean(why))
                ),
            )
        };
        let (Some(port), Some(h)) = (self.mcp.as_ref(), self.host.as_ref()) else {
            return fail(&self.mcp_missing());
        };
        if self.review {
            return fail(crate::REVIEW_CANNOT_ASK);
        }
        // A session that read text the judge flagged proposes nothing: npm would run for it.
        if self.is_suspect(&req.session) {
            return fail(&format!(
                "{SUSPECT_REASON}: no server is proposed from this session"
            ));
        }
        let reason = args
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if reason.is_empty() || reason.chars().count() > MAX_REASON || reason.contains('\n') {
            return fail(&format!(
                "reason: why this server, one line of at most {MAX_REASON} characters"
            ));
        }
        if let Some(k) = args.keys().find(|k| {
            !matches!(
                k.as_str(),
                "name"
                    | "description"
                    | "package"
                    | "version"
                    | "args"
                    | "env"
                    | "network"
                    | "secrets"
                    | "reads"
                    | "writes"
                    | "limits"
                    | "reason"
            )
        }) {
            return fail(&format!(
                "{:?} is not a field of mcp_propose; only local npm servers at an exact version are supported (no command, URL, remote server or OAuth)",
                one_line(k)
            ));
        }
        let m = match manifest_of(args).and_then(|m| check(&m).map(|()| m)) {
            Ok(m) => m,
            Err(why) => return fail(&why),
        };
        if let Some(why) = m
            .writes
            .iter()
            .find_map(|w| services::write_refused(w, &h.cfg.home, &h.cfg.control))
        {
            return fail(&format!("writes: {why}"));
        }
        if let Some(why) = m
            .reads
            .iter()
            .find_map(|r| services::read_refused(r, &h.cfg.home, &h.cfg.control))
        {
            return fail(&format!("reads: {why}"));
        }
        let manifest = compose(&m);
        match self.try_clean(&manifest) {
            None => return fail("the vault could not filter the text"),
            Some(c) if c != manifest || count_masks(&c) > count_masks(&manifest) => {
                return fail(
                    "it holds something shaped like a secret, or a hidden character: a server's secrets come from the vault by name (declare them in secrets)",
                );
            }
            Some(_) => {}
        }
        let root = port.root();
        let now = current_manifest(&root, &m.name);
        let installed = load_state(&root, &m.name).map(|s| s.package);
        if let Some(other) = now
            .as_ref()
            .map(|n| n.package.clone())
            .or(installed)
            .filter(|p| *p != m.package)
        {
            return fail(&format!(
                "that name is the server {other}; choose another name or remove it first"
            ));
        }
        // The same package and version with other hosts, secrets, paths or settings is a new
        // proposal; exactly the same one is not.
        if now.as_ref() == Some(&m) {
            return fail(&format!(
                "«{}» is installed at {}@{} already, exactly so: the person turns it on with /mcp",
                m.name, m.package, m.version
            ));
        }
        // The brake, counted before npm runs.
        if !self.mcp_pool.may_propose(&req.session) {
            return fail(&format!(
                "at most {PROPOSALS_PER_HOUR} proposals an hour in a session: wait, or ask the person"
            ));
        }
        let facts = match port
            .npm_view(&m.package, &m.version)
            .and_then(|v| npm_facts(&v, &m.package, &m.version, &|s| self.clean(s)))
        {
            Ok(f) => f,
            Err(why) => return fail(&why),
        };
        let update = now.is_some();
        let pending_args = json!({
            "name": m.name,
            "description": m.description,
            "skill_md": manifest,
            "files": [],
            "sha256": skills::digest(&manifest, &[]),
            "update": update,
            "npm": facts,
        });
        let p = match crate::pending::set_aside_mcp(
            &h.cfg.state,
            &h.cfg.run,
            &req.session,
            pending_args,
            &self.clean(reason),
            h.cfg.rules.questions_per_day(),
        ) {
            Ok(p) => p,
            Err(e) => return fail(&format!("it could not be set aside ({e})")),
        };
        self.mcp_answer(
            req,
            true,
            "mcp/propose",
            format!(
                "set aside {}: the MCP server «{}» ({}@{}) waits for the person's approval. Nothing was downloaded. If they approve, Clio installs it and lists its tools, and it stays off until the person confirms them; then mcp_tools shows it. Do not propose it again meanwhile; go on with the rest and say so at the end.",
                p.id, m.name, m.package, m.version
            ),
        )
    }

    /// `mcp_tools`: the servers that are on, their tools, schemas and modes (outside text).
    pub(crate) fn mcp_tools(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let Some(port) = self.mcp.as_ref().filter(|_| self.host.is_some()) else {
            return self.mcp_answer(
                req,
                false,
                "mcp/tools",
                format!("mcp_tools failed: {}", self.mcp_missing()),
            );
        };
        let root = port.root();
        prune(&self.mcp_pool.servers, &root);
        let only = args
            .get("server")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let mut out = vec![];
        for n in names(&root) {
            if (!only.is_empty() && n != only) || !is_on(&root, &n) {
                continue;
            }
            let Some(s) = load_state(&root, &n) else {
                continue;
            };
            out.push(format!("server «{n}» ({}@{}):", s.package, s.version));
            for t in &s.tools {
                let mode = match (t.mode, t.flagged) {
                    (_, true) => "ask: it changed since the person confirmed it",
                    (Mode::Free, false) => "free",
                    (Mode::Ask, false) => "ask: each call waits for the person",
                };
                out.push(format!("- {} [{mode}]: {}", t.name, t.description));
                out.push(format!("  arguments: {}", t.schema));
            }
        }
        let text = if out.is_empty() {
            if only.is_empty() {
                "no MCP server is on".to_owned()
            } else {
                format!("no MCP server «{}» is on", one_line(only))
            }
        } else {
            out.join("\n")
        };
        self.mcp_outside(req, true, "mcp/tools", &text, "mcp_tools")
    }

    /// `mcp_call`: runs a `free` tool of a server that is on; sets aside the rest (`send`).
    pub(crate) fn mcp_call(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let fail = |why: &str| {
            self.mcp_answer(
                req,
                false,
                "mcp/call",
                format!("mcp_call failed: {}", one_line(&self.clean(why))),
            )
        };
        let (Some(port), Some(h)) = (self.mcp.as_ref(), self.host.as_ref()) else {
            return fail(&self.mcp_missing());
        };
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim();
        let (server, tool) = (s("server"), s("tool"));
        let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let root = port.root();
        // A copy turned off, removed or updated meanwhile stops before anything else.
        prune(&self.mcp_pool.servers, &root);
        if !is_on(&root, server) {
            return fail(&format!(
                "no MCP server «{}» is on (mcp_tools lists them)",
                one_line(server)
            ));
        }
        let Some(state) = load_state(&root, server) else {
            return fail("its state does not read");
        };
        let Some(t) = state.tools.iter().find(|t| t.name == tool) else {
            return fail(&format!(
                "«{server}» has no tool «{}» (mcp_tools lists them)",
                one_line(tool)
            ));
        };
        if let Err(why) = check_arguments(&t.schema, &arguments) {
            return fail(&why);
        }
        // Nothing shaped like a secret leaves: the server holds its own, from the vault.
        let text = serde_json::to_string(&arguments).unwrap_or_default();
        match self.try_clean(&text) {
            Some(c) if c == text => {}
            _ => {
                return fail(
                    "the arguments hold something shaped like a secret, or a hidden character: nothing was sent",
                );
            }
        }
        let why = if self.is_suspect(&req.session) {
            Some(SUSPECT_REASON)
        } else if t.flagged {
            Some("the tool changed since the person confirmed it")
        } else if t.mode == Mode::Ask {
            Some("this tool of the server may change something")
        } else {
            None
        };
        if let Some(why) = why {
            let mut r = req.clone();
            r.args = json!({
                "server": server,
                "tool": tool,
                "arguments": arguments,
                "version": state.version,
                "call": call_text(server, tool, &arguments),
            });
            return self.aside(h, &r, Class::Send, Some(why));
        }
        let (ok, raw) = self.mcp_run(server, tool, &arguments);
        if let Err(e) = h.log(
            &req.session,
            "mcp_call",
            &format!("{server} {tool}"),
            false,
            if ok { "run" } else { "failed" },
            "mcp/call",
            None,
        ) {
            eprintln!("clio-toolhost: the action log lost a line: {e}");
        }
        self.mcp_outside(req, ok, "mcp/call", &raw, &format!("{server} {tool}"))
    }

    /// Runs `tool` of `server` in this host's process of it, started on first use (and its tools
    /// compared with the stored ones: a changed one is flagged, and then the call waits); one that
    /// is no longer what it was started as is stopped first. The wait for the server, its start
    /// and the call share the call's time limit; no other server waits for it. The text: what it
    /// answered, or why it failed (a crashed or hung server is killed).
    pub(crate) fn mcp_run(&self, server: &str, tool: &str, arguments: &Value) -> (bool, String) {
        let Some(port) = self.mcp.as_ref() else {
            return (false, "no MCP servers here".into());
        };
        let root = port.root();
        let limit = Duration::from_secs(
            current_manifest(&root, server).map_or(DEFAULT_LIMITS.seconds, |m| m.limits.seconds),
        );
        let until = Instant::now() + limit;
        let slot = lock(&self.mcp_pool.servers)
            .entry(server.to_owned())
            .or_default()
            .clone();
        // Another call of this server holds it: wait for it, within this call's time.
        let mut held = loop {
            match slot.try_lock() {
                Ok(g) => break g,
                Err(TryLockError::Poisoned(p)) => break p.into_inner(),
                Err(TryLockError::WouldBlock) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(TryLockError::WouldBlock) => {
                    return (
                        false,
                        format!(
                            "the server «{server}» was busy with another call for its whole time limit"
                        ),
                    );
                }
            }
        };
        if held
            .as_ref()
            .is_some_and(|e| !still_fresh(&root, server, e))
        {
            *held = None;
            log_event(&root, server, "stop", "ok", "no longer what was started");
        }
        if held.is_none() {
            let Some((current, version, tree)) = started_as(&root, server) else {
                return (false, format!("the server «{server}» is not on"));
            };
            let left = until.saturating_duration_since(Instant::now());
            let started = port.start(server).and_then(|p| Server::start(p, left));
            let s = match started {
                Ok(s) => s,
                Err(e) => {
                    log_event(&root, server, "start", "failed", &e);
                    return (false, format!("the server «{server}» did not start: {e}"));
                }
            };
            // Its tools as it gives them now, against what the person confirmed.
            let changed = {
                let _l = lock_root(&root);
                match load_state(&root, server) {
                    Some(mut st) => {
                        let now = observe(&st.tools, s.tools.clone());
                        let changed = now != st.tools;
                        if changed {
                            st.tools = now;
                            if let Err(e) = save_state(&root, &st) {
                                eprintln!("clio-toolhost: {e}");
                            }
                        }
                        changed
                    }
                    None => true,
                }
            };
            log_event(
                &root,
                server,
                "start",
                "ok",
                if changed { "its tools changed" } else { "" },
            );
            *held = Some(Entry {
                server: s,
                current,
                version,
                tree,
            });
            if changed
                && load_state(&root, server)
                    .and_then(|st| st.tools.into_iter().find(|t| t.name == tool))
                    .is_none_or(|t| t.flagged)
            {
                return (
                    false,
                    format!(
                        "the server «{server}» now describes «{tool}» differently from what the person confirmed, or no longer has it: it was not called; call it again and it waits for the person"
                    ),
                );
            }
        }
        let Some(e) = held.as_mut() else {
            return (false, "the server is gone".into());
        };
        let left = until.saturating_duration_since(Instant::now());
        match e.server.call(tool, arguments, left) {
            Ok((text, failed)) => {
                log_event(
                    &root,
                    server,
                    &format!("call {tool}"),
                    if failed { "tool error" } else { "ok" },
                    "",
                );
                if failed {
                    (true, format!("the tool reported an error:\n{text}"))
                } else {
                    (true, text)
                }
            }
            Err(err) => {
                let tail = e.server.stderr_tail();
                *held = None;
                log_event(&root, server, &format!("call {tool}"), "failed", &err);
                let tail = if tail.is_empty() {
                    String::new()
                } else {
                    format!(" (its last words: {tail})")
                };
                (
                    false,
                    format!("the server «{server}» failed and was stopped: {err}{tail}"),
                )
            }
        }
    }

    /// The person approved call `args` (set aside by `mcp_call`): it runs exactly as it was shown,
    /// if the server is still on at that version and the tool is still there.
    pub(crate) fn mcp_approved(&self, args: &Value) -> Result<(bool, String), String> {
        let port = self.mcp.as_ref().ok_or_else(|| self.mcp_missing())?;
        let s = |k: &str| args[k].as_str().unwrap_or("");
        let (server, tool, arguments) = (s("server"), s("tool"), &args["arguments"]);
        if call_text(server, tool, arguments) != s("call") {
            return Err("what was shown is not what would run: it does not run".into());
        }
        let root = port.root();
        let state = load_state(&root, server).filter(|_| is_on(&root, server));
        match state {
            Some(st) if st.version == s("version") && st.tools.iter().any(|t| t.name == tool) => {
                Ok(self.mcp_run(server, tool, arguments))
            }
            _ => Err(format!(
                "«{server}» is off, at another version or has no tool «{tool}» now: it does not run"
            )),
        }
    }

    /// The servers this host started: stopped (the session closes).
    pub(crate) fn mcp_stop_all(&self) {
        // A call that holds its server keeps it until it returns; then it is dropped (killed).
        lock(&self.mcp_pool.servers).clear();
    }
}

/// What the model reads of a result before the guard wraps it: for tests of the whole path.
#[doc(hidden)]
pub fn wrapped(text: &str) -> String {
    wrap_untrusted(clio_gateway::Source::ToolResult, text)
}
