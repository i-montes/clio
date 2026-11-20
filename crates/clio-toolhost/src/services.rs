//! Services (stage 3, S-69): programs that run by themselves on a schedule, with no model in the
//! loop. What the tool host does with them, and nothing more:
//!
//! - `service_write`: the model's only way to propose a service or a new version of one. The draft
//!   goes the way of a skill's (`skills.rs`: the same checks of files and sizes, the judge reads
//!   every byte, the same caps and drafts folder), its main file is the manifest `service.json`
//!   composed here, and its id starts with `s`. Registering is never the tool host's: the person's
//!   `clio` reviews it and then it ALWAYS waits for the person's approval.
//! - `service_status`: the list, or one service, read from the services folder. What a run
//!   printed can carry text from the network: the answer is outside text, judged and marking.
//!
//! The manifest's shape, its checks and the schedule are here so the person's `clio` (which
//! registers and runs services) and the tool host read them the same way.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use clio_vault::count_masks;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::host::{now_ms, private_dir};
use crate::judge::{ToJudge, one_line};
use crate::skills::{self, Draft, SkillFile};
use crate::{DecisionOut, Request, Response, TextVerdict, ToolHost};

/// A service's main file: what it is, when it runs and what it may reach, as approved.
pub const MANIFEST: &str = "service.json";
pub const MIN_EVERY_MS: i64 = 5 * 60_000;
const MAX_EVERY_MS: i64 = 366 * 86_400_000;
pub const DEFAULT_LIMITS: Limits = Limits {
    cpu_percent: 50,
    memory_mb: 512,
    seconds: 300,
};
pub const MAX_LIMITS: Limits = Limits {
    cpu_percent: 100,
    memory_mb: 2048,
    seconds: 3600,
};
const MAX_COMMAND: usize = 2000;
const MAX_HOSTS: usize = 10;
const MAX_SECRETS: usize = 8;
const MAX_WRITES: usize = 10;
/// A write path's tree is walked this deep and this far, at most; past it, it is refused.
const TREE_DEPTH: usize = 4;
const TREE_ENTRIES: usize = 2000;
/// Names in a write path's tree that load code or settings by themselves, besides
/// `PROTECTED_PATHS` and the home's own (`PROTECTED_HOME`): a direnv file, Node's packages
/// (their `.bin`), a virtualenv.
const TREE_LOADS: [&str; 3] = [".envrc", "node_modules", "pyvenv.cfg"];
const MAX_REASON: usize = 300;
/// Runs kept per service, and the bytes kept of each one's output (its tail).
pub const KEPT_RUNS: usize = 50;
pub const TAIL_BYTES: usize = 4000;
/// Exit code of a run that needs the person's attention: not a failure.
pub const ATTENTION: i32 = 10;
/// Failed runs in a row that pause a service.
pub const FAILS_TO_PAUSE: u32 = 3;
/// Under the home, never a service's to write: what logs in, starts or holds keys, and Clio's own.
pub const PROTECTED_HOME: [&str; 21] = [
    ".ssh",
    ".gnupg",
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".bash_logout",
    ".profile",
    ".zshrc",
    ".zprofile",
    ".zshenv",
    ".zlogin",
    ".zlogout",
    ".pam_environment",
    ".config/fish",
    ".config/systemd",
    ".config/autostart",
    ".config/environment.d",
    ".config/clio",
    ".local/share/clio",
    ".local/bin",
    ".cargo/bin",
];
/// Outside the home, what a jail hides too (a service's run, an MCP server, npm for one): the
/// person's other data, sites and mounts. `/etc`, `/usr`, `/lib*`, `/bin` and `/sbin` stay for the
/// runtime, TLS and DNS. A read may name a path under one of them (never the root itself).
pub const HIDDEN_ROOTS: [&str; 7] = ["/home", "/root", "/var", "/srv", "/opt", "/mnt", "/media"];
/// Variables a service's secret may not take: what the launcher, the shell or the proxy set.
const RESERVED_ENV: [&str; 13] = [
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "SHELL", "PWD", "OLDPWD", "TMPDIR", "IFS", "ENV",
    "BASH_ENV", "NO_PROXY",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub cpu_percent: u32,
    pub memory_mb: u64,
    pub seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    /// A whole number of minutes, hours or days: `15m`, `1h`, `1d`.
    pub every: String,
    /// `HH:MM`, local time; only with `every: "1d"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretUse {
    /// The vault's name of it.
    pub name: String,
    /// The variable the run gets it in.
    pub env: String,
    /// One of the service's hosts: the vault releases it only when it is bound to `service:host`.
    pub host: String,
}

/// `service.json`, as approved: written by `compose`, read back only if it is exactly that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub schedule: Schedule,
    /// Run with `bash -c` from the service's files folder (read-only).
    pub command: String,
    pub verify: String,
    /// The only hosts it may reach (`host` is port 443, or `host:port`), through a proxy.
    pub network: Vec<String>,
    pub secrets: Vec<SecretUse>,
    /// Absolute paths under the home it may write, besides its state folder.
    pub writes: Vec<String>,
    /// Absolute paths under the home it may read (shown read-only): nothing else of the home is
    /// visible to it. Left out of the text when empty, so a manifest approved before it reads
    /// back the same.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
    pub limits: Limits,
}

/// One run, as the person's `clio` records it (`<services>/runs/<name>.json`, the last 50).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub start_ms: i64,
    pub ms: u64,
    pub version: u32,
    /// `ok`, `attention`, `failed`, `waiting` (the vault or a secret; not a failure), `paused`.
    pub outcome: String,
    #[serde(default)]
    pub exit: Option<i32>,
    /// Why, in a few words, when it did not just run.
    #[serde(default)]
    pub why: String,
    /// The end of what it printed, scrubbed and capped.
    #[serde(default)]
    pub tail: String,
    /// `timer` or `person`.
    #[serde(default)]
    pub by: String,
    /// The limits it ran under: `cgroup` (a transient user scope held them for the whole run)
    /// or `per-process` (no scope could be made: each process had them).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub limits: String,
}

/// A service draft's id starts with `s` (a skill's is hex digits only). The id decides how the
/// person's `clio` reads the draft, and the draft must then pass that kind's own checks
/// (`check_stored`): a service's id with a skill's text, or the other way, is dropped.
pub fn is_service_draft(id: &str) -> bool {
    id.starts_with('s')
}

/// `every` in milliseconds: a whole number of `m`, `h` or `d`, at least 5 minutes.
pub fn every_ms(every: &str) -> Option<i64> {
    let (n, unit) = every.split_at(every.len().checked_sub(1)?);
    if n.is_empty() || n.len() > 6 || !n.bytes().all(|b| b.is_ascii_digit()) || n.starts_with('0') {
        return None;
    }
    let ms = n.parse::<i64>().ok()?
        * match unit {
            "m" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            _ => return None,
        };
    (MIN_EVERY_MS..=MAX_EVERY_MS).contains(&ms).then_some(ms)
}

/// `HH:MM` in minutes after midnight.
pub fn at_minutes(at: &str) -> Option<i64> {
    let (h, m) = at.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

/// Is a run due at `now`? `last`: when the last real run started; `since`: when the current
/// version was registered; `offset`: the local time's seconds from UTC. Without `at`: due once
/// `every` has passed since the last run (at once if it never ran), with half a minute of slack
/// for the timer's own drift. With `at` (daily): due when the latest local `HH:MM` came after
/// both. A gap of any length gives one run, never one per slot missed.
// shortcut: the offset is today's; across a change of summer time a daily run may come an hour
// early or late once.
pub fn due(s: &Schedule, last: Option<i64>, since: i64, now: i64, offset: i64) -> bool {
    let Some(every) = every_ms(&s.every) else {
        return false;
    };
    match s.at.as_deref().and_then(at_minutes) {
        None => last.is_none_or(|l| now - l >= every - 30_000),
        Some(at) => {
            let day = 86_400_000;
            let local = now + offset * 1000;
            let mut slot = local.div_euclid(day) * day + at * 60_000 - offset * 1000;
            if slot > now {
                slot -= day;
            }
            last.unwrap_or(since).max(since) < slot
        }
    }
}

/// When it is due next (for the list): `None` when it is due now.
pub fn next_ms(s: &Schedule, last: Option<i64>, since: i64, now: i64, offset: i64) -> Option<i64> {
    if due(s, last, since, now, offset) {
        return None;
    }
    let every = every_ms(&s.every)?;
    Some(match s.at.as_deref().and_then(at_minutes) {
        None => last.unwrap_or(now) + every,
        Some(at) => {
            let day = 86_400_000;
            let local = now + offset * 1000;
            let slot = local.div_euclid(day) * day + at * 60_000 - offset * 1000;
            if slot > now { slot } else { slot + day }
        }
    })
}

/// A host as the proxy allows it: `host:port` (443 when none is given), lowercase, a name of at
/// least two labels whose last is not a number (no address literal), and not a name kept for the
/// machine or its own network (`.localhost`, `.local`, `.internal`, `.arpa`).
pub fn host_port(h: &str) -> Option<String> {
    let (host, port) = match h.rsplit_once(':') {
        Some((host, p)) => (host, p.parse::<u16>().ok().filter(|p| *p > 0)?),
        None => (h, 443),
    };
    let labels: Vec<&str> = host.split('.').collect();
    let ok = host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && labels
            .last()
            .is_some_and(|l| l.bytes().any(|b| b.is_ascii_lowercase()))
        && !matches!(
            labels.last(),
            Some(&("localhost" | "local" | "internal" | "arpa"))
        );
    ok.then(|| format!("{host}:{port}"))
}

/// The host part of a declared host (`api.x.com:8443` → `api.x.com`).
pub fn host_of(h: &str) -> &str {
    h.rsplit_once(':').map_or(h, |(host, _)| host)
}

pub(crate) fn valid_env(e: &str) -> bool {
    !e.is_empty()
        && e.len() <= 64
        && e.as_bytes()[0].is_ascii_uppercase()
        && e.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !RESERVED_ENV.contains(&e)
        && !e.starts_with("LD_")
        && !e.starts_with("CLIO_")
        && !e.starts_with("XDG_")
        && !e.ends_with("_PROXY")
}

/// Why a declared path (read or write) is refused, if it is: it must be absolute, plain (no `.`,
/// `..` or doubled `/`), under `home` and not the home itself (a read: or under one of
/// `HIDDEN_ROOTS`, not the root itself, nor the home's folder or one above it), neither inside nor
/// above Clio's folders (`control`) or one of `home`'s `also`, and exist as named (no link in it).
fn path_refused(
    p: &str,
    home: &Path,
    control: &[PathBuf],
    also: &[&str],
    read: bool,
) -> Option<String> {
    let path = Path::new(p);
    let plain = path.is_absolute()
        && p.len() <= 200
        && !p.ends_with('/')
        && !p.contains("//")
        && !p.split('/').any(|c| c == "." || c == "..")
        && !p.chars().any(skills::hidden)
        && path
            .components()
            .skip(1)
            .all(|c| matches!(c, Component::Normal(_)));
    if !plain {
        return Some(format!("{}: an absolute, plain path", one_line(p)));
    }
    let under_root = |r: &&str| path.starts_with(r) && path != Path::new(r);
    let inside = (path.starts_with(home) && path != home)
        || (read && HIDDEN_ROOTS.iter().any(under_root) && !home.starts_with(path));
    if !inside {
        return Some(format!(
            "{}: only under the home folder{}",
            one_line(p),
            if read {
                " (or under /var, /srv, /opt, /mnt, /media, /root or /home, never one of them itself)"
            } else {
                ""
            }
        ));
    }
    let protected = control
        .iter()
        .cloned()
        .chain(also.iter().map(|r| home.join(r)));
    for x in protected {
        if path.starts_with(&x) || x.starts_with(path) {
            return Some(format!(
                "{}: it is, or holds, {} (never a service's)",
                one_line(p),
                x.display()
            ));
        }
    }
    match path.canonicalize() {
        Ok(c) if c == path => None,
        Ok(_) => Some(format!("{}: a link is never a service's path", one_line(p))),
        Err(_) => Some(format!(
            "{}: it does not exist; it must exist before the service is written",
            one_line(p)
        )),
    }
}

/// Why a declared read path is refused, if it is (`path_refused`, keys: `~/.ssh`, `~/.gnupg`):
/// under the home or one of `HIDDEN_ROOTS`.
pub fn read_refused(p: &str, home: &Path, control: &[PathBuf]) -> Option<String> {
    path_refused(p, home, control, &[".ssh", ".gnupg"], true)
}

/// Why a declared write path is refused, if it is: `path_refused` with the home's
/// `PROTECTED_HOME`; no name in it under the home that starts with a dot (settings, keys,
/// what a program loads by itself), nor one of `PROTECTED_PATHS`; and its tree (`TREE_DEPTH`
/// levels, `TREE_ENTRIES` entries at most, past that refused) holds no repository and nothing
/// that loads code by itself. Checked at draft time, at approval and before each run.
pub fn write_refused(p: &str, home: &Path, control: &[PathBuf]) -> Option<String> {
    if let Some(why) = path_refused(p, home, control, &PROTECTED_HOME, false) {
        return Some(why);
    }
    let path = Path::new(p);
    let under = path.strip_prefix(home).unwrap_or(path);
    if under
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    {
        return Some(format!(
            "{}: a name that starts with a dot under the home holds settings or code that load by themselves",
            one_line(p)
        ));
    }
    if path
        .components()
        .any(|c| crate::PROTECTED_PATHS.contains(&c.as_os_str().to_string_lossy().as_ref()))
    {
        return Some(format!(
            "{}: a folder that loads code or settings by itself",
            one_line(p)
        ));
    }
    tree_refused(path).map(|why| format!("{}: {why}; declare a narrower folder", one_line(p)))
}

/// What in the tree under `top` makes it no service's to write: a repository or something that
/// loads code by itself, or more than can be checked. Links are not followed.
fn tree_refused(top: &Path) -> Option<String> {
    let loads = |n: &str| {
        crate::PROTECTED_PATHS.contains(&n)
            || TREE_LOADS.contains(&n)
            || PROTECTED_HOME.iter().any(|h| !h.contains('/') && *h == n)
    };
    let mut todo = vec![(top.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((dir, depth)) = todo.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            return Some(format!("{} cannot be read", dir.display()));
        };
        for e in entries {
            let Ok(e) = e else {
                return Some(format!("{} cannot be read", dir.display()));
            };
            seen += 1;
            if depth == TREE_DEPTH || seen > TREE_ENTRIES {
                return Some(format!(
                    "it holds more than can be checked ({TREE_ENTRIES} entries within {TREE_DEPTH} levels)"
                ));
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if loads(&name) {
                return Some(format!(
                    "it holds {} (a repository, or something that loads code by itself)",
                    e.path().display()
                ));
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                todo.push((e.path(), depth + 1));
            }
        }
    }
    None
}

/// The grants of a manifest (a service's or an MCP server's): its hosts by name, its secrets
/// each for one of them, at most `MAX_WRITES` declared paths, and its limits within `max`.
pub(crate) fn check_grants(
    network: &[String],
    secrets: &[SecretUse],
    paths: usize,
    l: Limits,
    max: Limits,
) -> Result<(), String> {
    if network.len() > MAX_HOSTS {
        return Err(format!("network: at most {MAX_HOSTS} hosts"));
    }
    for h in network {
        let Some(hp) = host_port(h) else {
            return Err(format!(
                "network: {:?} is not a host name (lowercase, like api.example.com or api.example.com:8443)",
                one_line(h)
            ));
        };
        if (hp != *h && hp != format!("{h}:443")) || (hp.ends_with(":443") && h.contains(':')) {
            return Err(format!(
                "network: write {:?} as {:?}",
                one_line(h),
                hp.trim_end_matches(":443")
            ));
        }
    }
    let mut seen = std::collections::HashSet::new();
    if network.iter().any(|h| !seen.insert(h)) {
        return Err("network: a host is given twice".into());
    }
    if secrets.len() > MAX_SECRETS {
        return Err(format!("secrets: at most {MAX_SECRETS}"));
    }
    let (mut names, mut envs) = (
        std::collections::HashSet::new(),
        std::collections::HashSet::new(),
    );
    for s in secrets {
        if !clio_vault::valid_name(&s.name) || !names.insert(&s.name) {
            return Err(format!(
                "secrets: {:?} is not a vault name (lowercase letters, digits, '-', '_', '.'), or it is given twice",
                one_line(&s.name)
            ));
        }
        if !valid_env(&s.env) || !envs.insert(&s.env) {
            return Err(format!(
                "secrets: env {:?} must be UPPER_CASE, not one the shell or Clio sets (PATH, HOME, LD_*, *_PROXY, CLIO_*...), and given once",
                one_line(&s.env)
            ));
        }
        if !network.iter().any(|h| host_of(h) == s.host) {
            return Err(format!(
                "secrets: {:?} goes to {:?}, which is not one of the hosts in network",
                one_line(&s.name),
                one_line(&s.host)
            ));
        }
    }
    if paths > MAX_WRITES {
        return Err(format!("writes, reads: at most {MAX_WRITES} paths each"));
    }
    if !(1..=max.cpu_percent).contains(&l.cpu_percent)
        || !(16..=max.memory_mb).contains(&l.memory_mb)
        || !(10..=max.seconds).contains(&l.seconds)
    {
        return Err(format!(
            "limits: cpu_percent 1 to {}, memory_mb 16 to {}, seconds 10 to {}",
            max.cpu_percent, max.memory_mb, max.seconds
        ));
    }
    Ok(())
}

/// The manifest's own shape (everything but the writes against a home): `Ok(())` or why not.
pub fn check(m: &Manifest) -> Result<(), String> {
    if !skills::valid_name(&m.name) {
        return Err(
            "name: lowercase letters, digits and single hyphens, at most 64 (like disk-check)"
                .into(),
        );
    }
    let dl = m.description.chars().count();
    if !(skills::MIN_DESCRIPTION..=skills::MAX_DESCRIPTION).contains(&dl)
        || m.description.contains('\n')
    {
        return Err(format!(
            "description: one line of {} to {} characters that says what it does",
            skills::MIN_DESCRIPTION,
            skills::MAX_DESCRIPTION
        ));
    }
    if every_ms(&m.schedule.every).is_none() {
        return Err(
            "schedule.every: a whole number of minutes, hours or days (\"15m\", \"1h\", \"1d\"), at least 5 minutes"
                .into(),
        );
    }
    if let Some(at) = &m.schedule.at
        && (m.schedule.every != "1d" || at_minutes(at).is_none())
    {
        return Err("schedule.at: \"HH:MM\" local time, only with every \"1d\"".into());
    }
    let c = m.command.trim();
    if c.is_empty() || m.command.len() > MAX_COMMAND || m.command.contains(['\n', '\0']) {
        return Err(format!(
            "command: one line of at most {MAX_COMMAND} bytes, run from the service's folder"
        ));
    }
    if m.verify.trim().is_empty() || m.verify.len() > skills::MAX_VERIFY {
        return Err(format!(
            "verify: a check that can be run, at most {} bytes (it is required)",
            skills::MAX_VERIFY
        ));
    }
    if skills::no_check(&m.verify) {
        return Err(
            "verify: that succeeds whatever the service does, so it is no check; give commands that fail when it does not do what it says (run it on a sample, without the network, and compare)"
                .into(),
        );
    }
    check_grants(
        &m.network,
        &m.secrets,
        m.writes.len().max(m.reads.len()),
        m.limits,
        MAX_LIMITS,
    )?;
    if [&m.description, &m.command, &m.verify]
        .iter()
        .any(|t| t.chars().any(skills::hidden))
    {
        return Err(
            "it has a control, invisible or tab character the person would not see as written"
                .into(),
        );
    }
    Ok(())
}

/// The manifest's text: what is shown, approved, hashed and run.
pub fn compose(m: &Manifest) -> String {
    format!("{}\n", serde_json::to_string_pretty(m).unwrap_or_default())
}

/// A stored service (manifest text and files), checked as a draft is: its manifest must be one
/// `compose` wrote, of this name, and its files plain and within the skills' limits.
pub fn check_stored(name: &str, manifest: &str, files: &[SkillFile]) -> Result<Manifest, String> {
    let m: Manifest =
        serde_json::from_str(manifest).map_err(|e| format!("{MANIFEST} does not read: {e}"))?;
    if m.name != name || compose(&m) != manifest {
        return Err(format!(
            "{MANIFEST} is not what Clio wrote for «{}»",
            one_line(name)
        ));
    }
    check(&m)?;
    check_files(files)?;
    if manifest.len() + files.iter().map(|f| f.content.len()).sum::<usize>() > skills::MAX_TOTAL {
        return Err("too large".into());
    }
    Ok(m)
}

fn check_files(files: &[SkillFile]) -> Result<(), String> {
    if files.len() > skills::MAX_FILES {
        return Err(format!("files: at most {}", skills::MAX_FILES));
    }
    let mut seen = std::collections::HashSet::new();
    for f in files {
        skills::valid_path(&f.path)?;
        if f.path.eq_ignore_ascii_case(MANIFEST) {
            return Err(format!("{MANIFEST} is written from the other fields"));
        }
        if !seen.insert(f.path.as_str()) || f.content.len() > skills::MAX_FILE {
            return Err(format!(
                "file {:?}: given twice, or more than {} bytes",
                one_line(&f.path),
                skills::MAX_FILE
            ));
        }
        if f.content.chars().any(skills::hidden) {
            return Err(format!(
                "{}: it has a control, invisible or tab character the person would not see as written; use plain text and spaces",
                one_line(&f.path)
            ));
        }
    }
    Ok(())
}

/// `<services>/records/<name>.json` (the person's `clio` writes it), if registered.
pub fn record(root: &Path, name: &str) -> Option<Value> {
    if !skills::valid_name(name) {
        return None;
    }
    serde_json::from_str(
        &fs::read_to_string(root.join("records").join(format!("{name}.json"))).ok()?,
    )
    .ok()
}

/// The runs recorded of `name`, oldest first.
pub fn runs(root: &Path, name: &str) -> Vec<Run> {
    fs::read_to_string(root.join("runs").join(format!("{name}.json")))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The registered manifest of `rec` (its current version), as stored.
pub fn current_manifest(root: &Path, rec: &Value) -> Option<Manifest> {
    let name = rec["name"].as_str()?;
    let n = rec["current"].as_u64()?;
    let t = fs::read_to_string(
        root.join("versions")
            .join(name)
            .join(n.to_string())
            .join(MANIFEST),
    )
    .ok()?;
    serde_json::from_str(&t).ok()
}

/// What `limits` of a manifest is: the given ones over the defaults (`d`).
pub(crate) fn limits_of(v: Option<&Value>, d: Limits) -> Result<Limits, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(d);
    };
    let o = v.as_object().ok_or("limits: an object")?;
    if o.keys()
        .any(|k| !matches!(k.as_str(), "cpu_percent" | "memory_mb" | "seconds"))
    {
        return Err("limits: only cpu_percent, memory_mb and seconds".into());
    }
    let n = |k: &str, d: u64| match o.get(k) {
        None | Some(Value::Null) => Ok(d),
        Some(x) => x.as_u64().ok_or(format!("limits.{k}: a whole number")),
    };
    Ok(Limits {
        cpu_percent: u32::try_from(n("cpu_percent", u64::from(d.cpu_percent))?).unwrap_or(u32::MAX),
        memory_mb: n("memory_mb", d.memory_mb)?,
        seconds: n("seconds", d.seconds)?,
    })
}

pub(crate) fn strings(v: Option<&Value>, what: &str) -> Result<Vec<String>, String> {
    match v {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_owned)
                    .ok_or(format!("{what}: a list of strings"))
            })
            .collect(),
        Some(_) => Err(format!("{what}: a list of strings")),
    }
}

/// `secrets` of a manifest: `{name, env, host}` each, nothing else.
pub(crate) fn secrets_of(v: Option<&Value>) -> Result<Vec<SecretUse>, String> {
    Ok(match v {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| {
                let g = |k: &str| x.get(k).and_then(Value::as_str).map(str::to_owned);
                match (g("name"), g("env"), g("host")) {
                    (Some(name), Some(env), Some(host))
                        if x.as_object().is_some_and(|o| o.len() == 3) =>
                    {
                        Ok(SecretUse { name, env, host })
                    }
                    _ => Err("secrets: each one is {name, env, host}".to_owned()),
                }
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("secrets: a list of {name, env, host}".into()),
    })
}

/// The manifest the model's arguments describe (not checked yet).
pub fn manifest_of(args: &Map<String, Value>) -> Result<Manifest, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let sched = args
        .get("schedule")
        .and_then(Value::as_object)
        .ok_or("schedule: {\"every\": \"15m\"}, or {\"every\": \"1d\", \"at\": \"03:00\"}")?;
    if sched.keys().any(|k| k != "every" && k != "at") {
        return Err("schedule: only every and at".into());
    }
    let secrets = secrets_of(args.get("secrets"))?;
    Ok(Manifest {
        name: s("name"),
        description: s("description").trim().to_owned(),
        schedule: Schedule {
            every: sched
                .get("every")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            at: sched.get("at").and_then(Value::as_str).map(str::to_owned),
        },
        command: s("command"),
        verify: s("verify").trim().to_owned(),
        network: strings(args.get("network"), "network")?,
        secrets,
        writes: strings(args.get("writes"), "writes")?,
        reads: strings(args.get("reads"), "reads")?,
        limits: limits_of(args.get("limits"), DEFAULT_LIMITS)?,
    })
}

impl ToolHost {
    /// Services live beside the skills folder (`<data>/services`), in a host-mode chat only.
    fn services_root(&self) -> Option<PathBuf> {
        self.host.as_ref()?;
        Some(self.skills.as_deref()?.parent()?.join("services"))
    }

    fn service_answer(&self, req: &Request, ok: bool, rule: &str, content: String) -> Response {
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

    /// `service_write`: checks the draft, has the judge read all of it and stores it for review.
    pub(crate) fn service_write(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let fail = |why: &str| {
            self.service_answer(
                req,
                false,
                "services/draft",
                format!(
                    "service_write failed: {}. Nothing was stored.",
                    one_line(&self.clean(why))
                ),
            )
        };
        let (Some(root), Some(h)) = (self.services_root(), self.host.as_ref()) else {
            return fail("services exist only in a chat on the person's machine (host mode)");
        };
        let reason = args
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if reason.is_empty() || reason.chars().count() > MAX_REASON {
            return fail(&format!(
                "reason: why this service, one line of at most {MAX_REASON} characters"
            ));
        }
        let files = match skills::files_of(args) {
            Ok(f) => f,
            Err(why) => return fail(&why),
        };
        let m = match manifest_of(args) {
            Ok(m) => m,
            Err(why) => return fail(&why),
        };
        if let Err(why) = check(&m).and_then(|()| check_files(&files)) {
            return fail(&why);
        }
        if let Some(why) = m
            .writes
            .iter()
            .find_map(|w| write_refused(w, &h.cfg.home, &h.cfg.control))
        {
            return fail(&format!("writes: {why}"));
        }
        if let Some(why) = m
            .reads
            .iter()
            .find_map(|r| read_refused(r, &h.cfg.home, &h.cfg.control))
        {
            return fail(&format!("reads: {why}"));
        }
        let manifest = compose(&m);
        if manifest.len() + files.iter().map(|f| f.content.len()).sum::<usize>() > skills::MAX_TOTAL
        {
            return fail(&format!(
                "the service would be over {} bytes",
                skills::MAX_TOTAL
            ));
        }
        // Nothing shaped like a secret: a service gets its secrets from the vault, by name.
        for t in std::iter::once(manifest.as_str()).chain(files.iter().map(|f| f.content.as_str()))
        {
            match self.try_clean(t) {
                None => return fail("the vault could not filter the text"),
                Some(c) if c != t || count_masks(&c) > count_masks(t) => {
                    return fail(
                        "it holds something shaped like a secret (a key, a token, a password): a service never carries one; declare it in secrets and the person stores it",
                    );
                }
                Some(_) => {}
            }
        }
        let (name, now) = (m.name.clone(), now_ms());
        if let Some(until) = skills::dropped_until(&self.run_dir, &name, now) {
            return fail(&format!(
                "a draft of «{name}» was dropped after three failed reviews; that name is refused for {} more minutes. Tell the person what failed instead of writing it again",
                (until - now) / 60_000 + 1
            ));
        }
        // Skills' and services' drafts share the hour's cap: each one costs a review.
        let recent = skills::drafts(&self.run_dir)
            .into_iter()
            .chain(skills::drafts_in(&skills::taken_dir(&self.run_dir)))
            .filter(|d| d.session == req.session && d.created_ms > now - 3_600_000)
            .count();
        if recent >= skills::DRAFTS_PER_HOUR {
            return fail(&format!(
                "this session already wrote {} drafts in the last hour; each one costs a review",
                skills::DRAFTS_PER_HOUR
            ));
        }
        let text = skills::whole_text(&manifest, &files);
        let source = format!("service_write «{name}»");
        match self.verdict_of(&text, &source) {
            TextVerdict::Clean => {}
            TextVerdict::Injection => {
                let w = ToJudge {
                    tool: "service_write",
                    source,
                };
                let mut out = [Value::String(text.clone())];
                self.withhold(req, &mut out, &[0], &w, &text);
                return fail(
                    "Clio's judge flagged its text as an attempt to give instructions to an AI agent; the person can read it",
                );
            }
            TextVerdict::NoJudge => {
                return fail(
                    "a service must be read by Clio's judge first, and the judge is off. Tell the person that services need the judge on (jev.enabled in config.json)",
                );
            }
            TextVerdict::NoAnswer => return fail("Clio's judge did not answer; try again later"),
        }
        let sha256 = skills::digest(&manifest, &files);
        let id: String = std::iter::once("s".to_owned())
            .chain(
                Sha256::digest(format!("{}|{sha256}|{now}", req.session).as_bytes())[..4]
                    .iter()
                    .map(|b| format!("{b:02x}")),
            )
            .collect();
        let update = record(&root, &name).is_some();
        let draft = Draft {
            id: id.clone(),
            session: req.session.clone(),
            name: name.clone(),
            description: m.description.clone(),
            reason: reason.to_owned(),
            verify: m.verify.clone(),
            skill_md: manifest,
            files,
            created_ms: now,
            update,
            sha256,
            marked: self.is_tainted(&req.session),
            suspect: self.is_suspect(&req.session),
        };
        let dir = skills::drafts_dir(&self.run_dir);
        let stored = private_dir(&dir).and_then(|()| {
            let body = serde_json::to_vec_pretty(&draft).map_err(std::io::Error::other)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(dir.join(format!("{id}.json")))
                .and_then(|mut f| f.write_all(&body))
        });
        if let Err(e) = stored {
            return fail(&format!("the draft could not be kept: {e}"));
        }
        if let Err(e) = h.log(
            &req.session,
            "service_write",
            &self.clean(&name),
            false,
            "draft",
            "services/draft",
            None,
        ) {
            eprintln!("clio-toolhost: the action log lost a line: {e}");
        }
        let what = if update {
            "a new version of the service"
        } else {
            "the new service"
        };
        // What happens after the review, as it will: bypass registers a service by itself (a
        // window never does); else the person decides.
        let then = if self.bypass_acts(&h.cfg.state, &req.session) {
            "if it passes, Clio registers it by herself, since the person's bypass is on, and you are told what came of it (registered, the review's findings, or a failure) in a turn she starts by herself"
        } else {
            "then it waits for the person's approval (always, for a service). The result comes by itself with a later message"
        };
        self.service_answer(
            req,
            true,
            "services/draft",
            format!(
                "draft {id} of {what} «{name}» stored. It is not registered: a reviewer checks it now, in the background, and {then}. Go on with the rest; do not write it again until then."
            ),
        )
    }

    /// `service_status`: the services, or one, with their last runs. What a run printed is outside
    /// text: the answer is judged when the guard filters it, and it marks the session.
    pub(crate) fn service_status(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let Some(root) = self.services_root() else {
            return self.service_answer(
                req,
                false,
                "services/status",
                "service_status failed: services exist only in a chat on the person's machine (host mode)".into(),
            );
        };
        let offset = local_offset(now_ms());
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let text = if name.is_empty() {
            status_list(&root, now_ms(), offset)
        } else {
            match record(&root, name) {
                None => format!("there is no service «{}»", one_line(name)),
                Some(r) => status_one(&root, &r, now_ms(), offset),
            }
        };
        let raw = text.clone();
        let text = self.clean(&text);
        // A run's output may hold text from outside: as `job_output`'s, judged and marking.
        self.mark(&req.session, "service");
        self.judge_later(req, "service_status");
        Response {
            id: req.id.clone(),
            ok: true,
            content: text.clone(),
            data: json!({ "masked": count_masks(&text).saturating_sub(count_masks(&raw)) }),
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "services/status".into(),
                reason: "reading the services' records".into(),
            },
            tainted: self.is_tainted(&req.session),
            external: true,
        }
    }
}

/// The local time's seconds from UTC at `now` (0 when it cannot be read).
pub fn local_offset(now: i64) -> i64 {
    crate::pending::local(now, "%z")
        .and_then(|z| {
            let n = |r: std::ops::Range<usize>| z.get(r)?.parse::<i64>().ok();
            let s = n(1..3)? * 3600 + n(3..5)? * 60;
            Some(if z.starts_with('-') { -s } else { s })
        })
        .unwrap_or(0)
}

/// `HH:MM` local of `ms`, with the day when it is not today.
pub(crate) fn when(ms: i64, now: i64, offset: i64) -> String {
    let day = |t: i64| (t / 1000 + offset).div_euclid(86_400);
    let t = (ms / 1000 + offset).rem_euclid(86_400);
    let hm = format!("{:02}:{:02}", t / 3600, t / 60 % 60);
    match day(ms) - day(now) {
        0 => hm,
        1 => format!("tomorrow {hm}"),
        -1 => format!("yesterday {hm}"),
        d => format!("{hm} ({d:+} days)"),
    }
}

/// The state of a service in a few words: on, off (why), or waiting.
pub fn state_of(rec: &Value, last: Option<&Run>) -> String {
    if rec["enabled"] != Value::Bool(true) {
        return format!("off ({})", rec["off"].as_str().unwrap_or("off"));
    }
    match last {
        Some(r) if r.outcome == "waiting" => format!("on, {}", r.why),
        _ => "on".into(),
    }
}

/// The last real run (not a wait).
pub fn last_start(runs: &[Run]) -> Option<i64> {
    runs.iter()
        .rev()
        .find(|r| matches!(r.outcome.as_str(), "ok" | "attention" | "failed"))
        .map(|r| r.start_ms)
}

/// When the current version was registered (`since` of `due`).
pub fn since_of(rec: &Value) -> i64 {
    rec["versions"]
        .as_array()
        .and_then(|v| v.iter().find(|v| v["n"] == rec["current"]))
        .and_then(|v| v["at_ms"].as_i64())
        .unwrap_or(0)
}

fn schedule_words(s: &Schedule) -> String {
    match &s.at {
        Some(at) => format!("every day at {at}"),
        None => format!("every {}", s.every),
    }
}

pub fn run_line(r: &Run, now: i64, offset: i64) -> String {
    let code = r.exit.map_or_else(String::new, |c| format!(" exit {c}"));
    // A wait's reason already says "waiting for …": it stands in for the outcome.
    let said = if r.why.is_empty() {
        r.outcome.clone()
    } else if r.why.starts_with(&r.outcome) {
        r.why.clone()
    } else {
        format!("{} ({})", r.outcome, r.why)
    };
    format!(
        "{} {said}{code}, {:.1} s",
        when(r.start_ms, now, offset),
        r.ms as f64 / 1000.0
    )
}

pub fn status_list(root: &Path, now: i64, offset: i64) -> String {
    let mut names: Vec<String> = fs::read_dir(root.join("records"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    let mut out = vec![];
    for n in names {
        let Some(r) = record(root, &n) else { continue };
        let runs = runs(root, &n);
        let sched = current_manifest(root, &r).map_or_else(String::new, |m| {
            let next = next_ms(&m.schedule, last_start(&runs), since_of(&r), now, offset)
                .map_or_else(
                    || "due now".into(),
                    |t| format!("next {}", when(t, now, offset)),
                );
            format!(" · {} · {next}", schedule_words(&m.schedule))
        });
        let last = runs.last().map_or_else(
            || " · never ran".into(),
            |l| format!(" · last: {}", run_line(l, now, offset)),
        );
        out.push(format!("{n} [{}]{sched}{last}", state_of(&r, runs.last())));
    }
    if out.is_empty() {
        "no services yet".into()
    } else {
        out.join("\n")
    }
}

pub fn status_one(root: &Path, r: &Value, now: i64, offset: i64) -> String {
    let name = r["name"].as_str().unwrap_or("");
    let runs = runs(root, name);
    let mut out = vec![format!(
        "{name} [{}] version {}: {}",
        state_of(r, runs.last()),
        r["current"],
        r["description"].as_str().unwrap_or("")
    )];
    if let Some(m) = current_manifest(root, r) {
        let next = next_ms(&m.schedule, last_start(&runs), since_of(r), now, offset)
            .map_or_else(|| "due now".into(), |t| when(t, now, offset));
        out.push(format!(
            "schedule: {} · next run: {next}",
            schedule_words(&m.schedule)
        ));
        out.push(format!("command: {}", m.command));
        if !m.network.is_empty() {
            out.push(format!("hosts: {}", m.network.join(", ")));
        }
    }
    out.push("last runs:".into());
    for x in runs.iter().rev().take(5) {
        out.push(format!("- {}", run_line(x, now, offset)));
    }
    if let Some(x) = runs.iter().rev().find(|x| !x.tail.is_empty()) {
        out.push(format!(
            "output of the run at {}:",
            when(x.start_ms, now, offset)
        ));
        out.push(x.tail.clone());
    }
    out.join("\n")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn m() -> Manifest {
        Manifest {
            name: "stamp".into(),
            description: "Writes the time to its state folder.".into(),
            schedule: Schedule {
                every: "5m".into(),
                at: None,
            },
            command: "bash run.sh".into(),
            verify: "bash run.sh && test -s \"$CLIO_STATE_DIR/stamp\"".into(),
            network: vec![],
            secrets: vec![],
            writes: vec![],
            reads: vec![],
            limits: DEFAULT_LIMITS,
        }
    }

    #[test]
    fn every_is_whole_minutes_hours_or_days_from_five_minutes() {
        assert_eq!(every_ms("5m"), Some(300_000));
        assert_eq!(every_ms("1h"), Some(3_600_000));
        assert_eq!(every_ms("2d"), Some(172_800_000));
        for bad in [
            "4m",
            "0h",
            "05m",
            "1.5h",
            "m",
            "",
            "1w",
            "-5m",
            "*/5 * * * *",
            "10",
        ] {
            assert_eq!(every_ms(bad), None, "{bad}");
        }
        assert_eq!(at_minutes("03:30"), Some(210));
        for bad in ["3:30", "24:00", "12:60", "1230", "ab:cd"] {
            assert_eq!(at_minutes(bad), None, "{bad}");
        }
    }

    // S-69: due or not, the `at` time, one catch-up run after a gap.
    #[test]
    fn the_schedule_is_due_once_per_interval_and_once_after_a_gap() {
        let five = Schedule {
            every: "5m".into(),
            at: None,
        };
        let t0 = 1_800_000_000_000;
        assert!(due(&five, None, t0, t0, 0), "never ran: due at once");
        assert!(!due(&five, Some(t0), t0, t0 + 240_000, 0));
        // The timer's drift: a run that started a little late is not pushed a whole minute.
        assert!(due(&five, Some(t0), t0, t0 + 299_500, 0));
        // A day down: one run, and then the next one an interval later.
        let later = t0 + 86_400_000;
        assert!(due(&five, Some(t0), t0, later, 0));
        assert!(!due(&five, Some(later), t0, later + 60_000, 0));
        // Daily at 03:00, in a zone two hours ahead of UTC (03:00 local = 01:00 UTC).
        let daily = Schedule {
            every: "1d".into(),
            at: Some("03:00".into()),
        };
        let day = 86_400_000;
        let midnight = (t0 / day) * day; // 00:00 UTC
        let off = 7200;
        let at = midnight + 3_600_000; // 01:00 UTC
        // Registered at 00:30 UTC: not due before 01:00 UTC, due after.
        let since = midnight + 1_800_000;
        assert!(!due(&daily, None, since, at - 60_000, off));
        assert!(due(&daily, None, since, at + 60_000, off));
        assert!(!due(&daily, Some(at + 60_000), since, at + 3_600_000, off));
        // Three days missed: one run.
        let gone = at + 3 * day + 60_000;
        assert!(due(&daily, Some(at + 60_000), since, gone, off));
        assert!(!due(&daily, Some(gone), since, gone + 60_000, off));
        assert_eq!(
            next_ms(&daily, Some(gone), since, gone + 60_000, off),
            Some(at + 4 * day)
        );
    }

    #[test]
    fn the_manifest_is_checked_and_reads_back_only_as_written() {
        check(&m()).unwrap();
        let text = compose(&m());
        assert_eq!(check_stored("stamp", &text, &[]).unwrap(), m());
        assert!(check_stored("other", &text, &[]).is_err());
        // A field added by hand, or the same text spaced otherwise, is not what Clio wrote.
        let added = text.replacen('{', "{\n  \"x\": 1,", 1);
        assert!(check_stored("stamp", &added, &[]).is_err());
        assert!(check_stored("stamp", &text.replace("  ", "   "), &[]).is_err());
        let bad = |f: &dyn Fn(&mut Manifest)| {
            let mut x = m();
            f(&mut x);
            check(&x).is_err()
        };
        assert!(bad(&|x| x.schedule.every = "1m".into()));
        assert!(
            bad(&|x| x.schedule.at = Some("03:00".into())),
            "at only with 1d"
        );
        assert!(bad(&|x| x.verify = "true".into()));
        assert!(bad(&|x| x.command = "a\nb".into()));
        assert!(bad(&|x| x.network = vec!["127.0.0.1".into()]));
        assert!(bad(&|x| x.network = vec!["localhost".into()]));
        assert!(bad(&|x| x.network = vec!["API.example.com".into()]));
        assert!(bad(&|x| x.network = vec!["api.example.com:443".into()]));
        assert!(bad(&|x| x.limits.memory_mb = 4096));
        assert!(bad(&|x| x.limits.seconds = 7200));
        let mut x = m();
        x.network = vec!["api.example.com".into(), "hooks.example.org:8443".into()];
        x.secrets = vec![SecretUse {
            name: "gh-token".into(),
            env: "GH_TOKEN".into(),
            host: "api.example.com".into(),
        }];
        check(&x).unwrap();
        for env in [
            "PATH",
            "LD_PRELOAD",
            "HTTPS_PROXY",
            "CLIO_STATE_DIR",
            "lower",
            "1X",
        ] {
            let mut y = x.clone();
            y.secrets[0].env = env.into();
            assert!(check(&y).is_err(), "{env}");
        }
        let mut y = x.clone();
        y.secrets[0].host = "evil.example.net".into();
        assert!(check(&y).is_err(), "a secret only to a declared host");
        assert_eq!(
            host_port("hooks.example.org:8443").unwrap(),
            "hooks.example.org:8443"
        );
        assert_eq!(host_port("api.example.com").unwrap(), "api.example.com:443");
        // Names kept for the machine and its own network are no service's host.
        for local in [
            "db.localhost",
            "printer.local",
            "metadata.google.internal",
            "1.0.0.127.in-addr.arpa",
            "x.localhost:5432",
        ] {
            assert_eq!(host_port(local), None, "{local}");
        }
        assert!(host_port("local.example.com").is_some());
        // An old manifest (no `reads`) reads back as written; one with `reads` keeps them.
        assert!(!compose(&m()).contains("reads"));
        let mut r = m();
        r.reads = vec!["/home/u/notes".into()];
        assert_eq!(check_stored("stamp", &compose(&r), &[]).unwrap(), r);
    }

    #[test]
    fn a_write_path_stays_in_the_home_and_away_from_what_is_protected() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let h = |r: &str| home.join(r).display().to_string();
        let control = [home.join(".local/share/clio")];
        for d in [
            "backups",
            "sites/a/cache",
            "project/.git/hooks",
            "deep/1/2/3/4/5",
            ".config/git",
            ".local/share/clio/services",
            ".ssh",
            "node/node_modules/.bin",
            "venv/bin",
            "many",
        ] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        fs::write(home.join("venv/pyvenv.cfg"), "home = /usr").unwrap();
        fs::write(home.join("sites/a/.envrc"), "export X=1").unwrap();
        for i in 0..=TREE_ENTRIES {
            fs::write(home.join(format!("many/{i}")), "").unwrap();
        }
        std::os::unix::fs::symlink(home.join("backups"), home.join("link")).unwrap();
        assert_eq!(write_refused(&h("backups"), &home, &control), None);
        assert_eq!(write_refused(&h("sites/a/cache"), &home, &control), None);
        assert_eq!(write_refused(&h("deep/1/2/3"), &home, &control), None);
        for bad in [
            h(""),
            format!("{}/", h("backups")),
            "/etc/cron.d".into(),
            h("backups/../.ssh"),
            h("./backups"),
            "backups".into(),
            h(".ssh"),
            h(".bashrc"),
            h(".config/systemd/user"),
            h(".config/git"),
            h(".config"),
            h(".local"),
            h(".local/share/clio/services"),
            h(".local/bin"),
            h("project/.git/hooks"),
            // A tree that holds a repository, code that loads by itself, or too much to check.
            h("project"),
            h("sites/a"),
            h("node"),
            h("venv"),
            h("deep"),
            h("many"),
            // Not there yet, or a link.
            h("missing"),
            h("link"),
        ] {
            let why = write_refused(&bad, &home, &control);
            assert!(why.is_some(), "{bad}");
        }
        let why = write_refused(&h("project"), &home, &control).unwrap();
        assert!(why.contains(".git") && why.contains("narrower"), "{why}");
        // A read path: anything existing under the home but the home itself, Clio's and keys.
        assert_eq!(read_refused(&h("project"), &home, &control), None);
        assert_eq!(read_refused(&h(".config/git"), &home, &control), None);
        for bad in [
            h(""),
            h(".ssh"),
            h(".local"),
            h(".local/share/clio/services"),
            h("link"),
            h("missing"),
            "/etc".into(),
            "/var".into(),
            "/home".into(),
            "/var/../etc".into(),
        ] {
            assert!(read_refused(&bad, &home, &control).is_some(), "{bad}");
        }
        // Under a hidden root outside the home: a read may name it; a write never. Linux only:
        // services run only under bwrap, and macOS's `/var` is a link to `/private/var`.
        let var = cfg!(target_os = "linux")
            .then(|| tempfile::TempDir::new_in("/var/tmp").ok())
            .flatten();
        if let Some(v) = &var {
            let p = v.path().display().to_string();
            assert_eq!(read_refused(&p, &home, &control), None);
            assert!(write_refused(&p, &home, &control).is_some());
        }
        assert!(
            read_refused(
                &home.parent().unwrap().display().to_string(),
                &home,
                &control
            )
            .is_some()
        );
    }
}
