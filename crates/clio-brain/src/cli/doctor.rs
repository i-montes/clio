//! `clio doctor`: Clio's own checks, then the host script of `infra/local/check-host.sh` (run in
//! its embedded mode, which leaves out the checks Clio did itself: each appears once).
//! `collect` gathers the checks as data; the plain command and the TUI both show them.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use clio_brain::connector::{Connector, by_provider};
use clio_vault::proto::Request;
use clio_vault::{Scrubber, VaultClient};
use serde_json::json;

use super::home::{Config, Dirs, Mode, builtin_adapter_path, lacks_bridge, names_builtin_adapter};
use super::mode;
use super::tui::app::plural;
use super::tui::sanitize;
use super::ui::{Level, Style};
use super::vault::PERSON_SOCK;

#[derive(Debug, Clone)]
pub struct Check {
    pub level: Level,
    pub msg: String,
    /// What to do about a WARN or a FAIL.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub checks: Vec<Check>,
    /// The host script that ran, if it was found.
    pub host_script: Option<String>,
    /// The jail: whether `run_tests` can be offered, why not, and every self-test check.
    pub jail: Option<serde_json::Value>,
}

impl Report {
    pub fn count(&self, l: Level) -> usize {
        self.checks.iter().filter(|c| c.level == l).count()
    }

    pub fn ok(&self) -> bool {
        self.count(Level::Fail) == 0
    }

    fn say(&mut self, level: Level, msg: String, hint: Option<String>) {
        self.checks.push(Check { level, msg, hint });
    }
}

/// The folder a jailed program writes, under Clio's `data`: a service's state
/// (`services/state/<name>`) or an MCP server's data, its `HOME` (`mcp/data/<name>`), live or
/// kept with a removed one (`skills::remove_in`). What the program wrote, not Clio: its own mode
/// is checked, what is inside is the program's (Chromium makes its caches 755). Only at those
/// places from the data root: the same names deeper (a skill's files, a project copy) are Clio's.
fn jailed_data(data: &Path, p: &Path) -> bool {
    let Ok(rel) = p.strip_prefix(data) else {
        return false;
    };
    let c: Vec<&str> = rel.iter().map(|s| s.to_str().unwrap_or("")).collect();
    matches!(
        c[..],
        ["mcp", "data", _]
            | ["mcp", "removed", _, "data"]
            | ["services", "state", _]
            | ["services", "removed", _, "state"]
    )
}

/// Every file and folder under `dir`: folders must be 0700, files and sockets without access for
/// group and others. A jailed program's own folder under `data` is checked, not walked.
fn check_tree(o: &mut Report, data: &Path, dir: &Path, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let Ok(m) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        let md = m.mode() & 0o777;
        if m.is_symlink() {
            continue; // a link's own mode is meaningless; the key file is checked on its own
        }
        if m.is_dir() {
            if md & 0o077 != 0 {
                o.say(
                    Level::Fail,
                    format!("{} has mode {md:o}, it must be 700 (S-23)", p.display()),
                    Some(format!("chmod 700 {}", p.display())),
                );
            }
            if depth < 8 && !jailed_data(data, &p) {
                check_tree(o, data, &p, depth + 1);
            }
        } else if md & 0o077 != 0 {
            o.say(
                Level::Fail,
                format!("{} has mode {md:o}, it must be 600 (S-23)", p.display()),
                Some(format!("chmod 600 {}", p.display())),
            );
        }
    }
}

/// Clio processes listening on a TCP address that is not loopback or tailscale (100.x).
fn public_listeners() -> Vec<String> {
    let mut inode_addr: Vec<(String, String)> = Vec::new();
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != "0A" {
                continue;
            }
            let Some((ip, port)) = f[1].split_once(':') else {
                continue;
            };
            let local = if v6 {
                ip == "00000000000000000000000001000000"
                    || ip.starts_with("0000000000000000FFFF00007F") // ::ffff:127.x
            } else {
                ip.ends_with("7F") || ip.ends_with("64")
            };
            if !local {
                let port = u16::from_str_radix(port, 16).unwrap_or(0);
                inode_addr.push((f[9].to_owned(), format!("{ip}:{port}")));
            }
        }
    }
    let mut out = Vec::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return out;
    };
    for p in procs.filter_map(Result::ok) {
        let path = p.path();
        let Ok(comm) = std::fs::read_to_string(path.join("comm")) else {
            continue;
        };
        if !comm.trim().starts_with("clio") {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(path.join("fd")) else {
            continue;
        };
        for fd in fds.filter_map(Result::ok) {
            let Ok(t) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let t = t.to_string_lossy().into_owned();
            for (inode, addr) in &inode_addr {
                if t == format!("socket:[{inode}]") {
                    out.push(format!(
                        "{} (pid {}) on {addr}",
                        comm.trim(),
                        p.file_name().to_string_lossy()
                    ));
                }
            }
        }
    }
    out
}

/// Folders inside the data folder that hold the person's own files, not what Clio wrote: the
/// copy of a project, the originals `apply` saved, what npm installed for Pi or an MCP server
/// (each package checked against its lockfile's integrity when installed; a server's tree is
/// hashed again before each start), and the private Node the installer unpacked (`node`,
/// nodejs.org's files, checked against the SHA-256 the binary pins).
const NOT_CLIO_OUTPUT: [&str; 5] = ["sandbox", "scratch", "backup", "node_modules", "node"];
/// Trees from the data folder's root that others wrote, checked another way, and so not read for
/// key shapes (S-18) by the doctor nor by the vault: Chromium (`browsers`: the archive whose
/// SHA-256 the catalog pins, unpacked once, made private, shown read-only to the server that
/// reads it) and npm's store of the registry's files (`content-v2`: each file named by its own
/// SHA-512, which npm checks on every read). npm's index and logs beside it are still read; the
/// modes of all of them are still checked (S-23).
const NOT_READ: [&str; 2] = ["browsers", "mcp/npm-cache/_cacache/content-v2"];

fn not_read(data: &Path, p: &Path) -> bool {
    NOT_READ.iter().any(|r| p == data.join(r))
}
/// A file bigger than this is not read.
const AUDIT_MAX_BYTES: u64 = 32 << 20;

fn audit_walk(
    data: &Path,
    dir: &Path,
    depth: usize,
    f: &Scrubber,
    out: &mut (usize, Vec<PathBuf>, usize),
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            let skip = e
                .file_name()
                .to_str()
                .is_some_and(|n| NOT_CLIO_OUTPUT.contains(&n));
            if !skip && depth < 8 && !jailed_data(data, &p) && !not_read(data, &p) {
                audit_walk(data, &p, depth + 1, f, out);
            }
        } else if t.is_file() {
            if e.metadata().map_or(0, |m| m.len()) > AUDIT_MAX_BYTES {
                out.2 += 1;
                continue;
            }
            let Ok(bytes) = std::fs::read(&p) else {
                out.2 += 1;
                continue;
            };
            out.0 += 1;
            if !f.find(&String::from_utf8_lossy(&bytes)).is_empty() {
                out.1.push(p);
            }
        }
    }
}

/// S-18: nothing shaped like a key may sit in what Clio wrote (run logs, transcripts, Pi's
/// sessions, the memory). Looks for the key shapes the output filter knows; the paths are shown,
/// never the values. The vault's own values are checked by `vault_audit`, which needs its server.
fn data_audit(o: &mut Report, data: &Path) {
    let mut found = (0, Vec::new(), 0);
    audit_walk(data, data, 0, &Scrubber::default(), &mut found);
    let (read, mut hits, unread) = found;
    hits.sort();
    if hits.is_empty() {
        o.say(
            Level::Pass,
            format!("nothing shaped like a key in what Clio wrote ({read} files checked, S-18)"),
            None,
        );
    } else {
        let list: Vec<String> = hits
            .iter()
            .take(8)
            .map(|p| sanitize::line(&p.strip_prefix(data).unwrap_or(p).display().to_string()))
            .collect();
        let more = hits.len().saturating_sub(list.len());
        o.say(
            Level::Fail,
            format!(
                "{} file(s) of Clio hold something shaped like a key (S-18): {}{}",
                hits.len(),
                list.join(", "),
                if more > 0 { format!(" and {more} more") } else { String::new() }
            ),
            Some(
                "Inside ~/.local/share/clio. If it is the effects log of a run from an earlier version, delete that file; if not, delete the run (`clio runs`).".into(),
            ),
        );
    }
    if unread > 0 {
        o.say(
            Level::Warn,
            format!("{unread} file(s) of Clio could not be checked (too big or no permission)"),
            None,
        );
    }
}

/// The jail's self-test, one line per check. A FAIL here closes `run_tests`.
fn jail_lines(o: &mut Report) {
    use super::jail::{Status, describe};
    let st = super::jail::check_fresh();
    let (bwrap, test) = match &st {
        Status::Unavailable(why) => {
            o.say(
                Level::Warn,
                format!("jail: run_tests is off, {why}"),
                Some(
                    "It needs Linux, bubblewrap (bwrap), prlimit (util-linux), python3 and unprivileged user namespaces; see docs/usage.md.".into(),
                ),
            );
            o.jail = Some(json!({"available": false, "reason": why, "checks": []}));
            return;
        }
        Status::Ready { bwrap, test } | Status::Failed { bwrap, test } => (bwrap, test),
    };
    o.say(
        Level::Pass,
        format!("jail: bwrap {bwrap} starts with unprivileged namespaces"),
        None,
    );
    for c in &test.checks {
        if c.passed {
            o.say(Level::Pass, format!("jail: {}", describe(c)), None);
        } else {
            o.say(
                Level::Fail,
                format!("jail: FAILED, {} ({})", describe(c), c.detail),
                Some("run_tests stays off until this passes; do not use the jail.".into()),
            );
        }
    }
    o.say(
        if st.ready() { Level::Pass } else { Level::Fail },
        if st.ready() {
            "jail: self-test complete, run_tests is available to the agent".into()
        } else {
            "jail: the self-test failed, run_tests is NOT offered to the agent".into()
        },
        None,
    );
    let checks: Vec<_> = test
        .checks
        .iter()
        .map(|c| json!({"name": c.name, "passed": c.passed, "detail": c.detail}))
        .collect();
    o.jail = Some(json!({"available": st.ready(), "bwrap": bwrap, "checks": checks}));
}

/// The Claude bridge, for a connector that runs through it: the chat and the single calls both
/// need it.
fn bridge_report(o: &mut Report, dirs: &Dirs, c: &Connector) {
    let name = c.name;
    if c.pi_extension.is_some() {
        if lacks_bridge(dirs, c) {
            o.say(
                Level::Warn,
                format!("connector {name}: the Claude bridge or its Claude Code is missing: clio pi install"),
                Some("The chat and the single calls go through Claude Code, which brings the bridge (pi-claude-agent-sdk).".into()),
            );
        } else {
            o.say(
                Level::Pass,
                format!("connector {name}: the bridge and Claude Code are in the installed Pi"),
                None,
            );
        }
    }
}

/// The active connector's key: the variable if set (it wins over the vault), else whether the
/// vault holds it and lets `model` send it to the connector's domain. No value moves.
fn connector_report(o: &mut Report, dirs: &Dirs, c: &Connector) {
    let name = c.name;
    if std::env::var_os(c.key_env).is_some_and(|v| !v.is_empty()) {
        o.say(
            Level::Pass,
            format!("connector {name}: the key comes from {}", c.key_env),
            None,
        );
        return;
    }
    use clio_vault::VaultError as E;
    let ready = super::vault::gateway_socket(dirs)
        .map_err(|_| "the vault is not running".to_owned())
        .and_then(|s| {
            VaultClient::new(&s)
                .with_timeout(std::time::Duration::from_secs(2))
                .check(c.secret, "model", Some(c.domain), "")
                .map_err(|e| match e {
                    E::Locked => "the vault is locked".to_owned(),
                    E::NotFound => "it is not in the vault".to_owned(),
                    E::Denied(_) => format!("the vault does not let it go to {}", c.domain),
                    E::Revoked => "it is revoked".to_owned(),
                    E::Expired => "it has expired".to_owned(),
                    _ => "the vault is not running".to_owned(),
                })
        });
    match ready {
        Ok(()) => o.say(
            Level::Pass,
            format!(
                "connector {name}: the key is in the vault and can only go to {}",
                c.domain
            ),
            None,
        ),
        Err(why) => o.say(
            Level::Warn,
            format!("connector {name}: the key cannot be used ({why}): clio connect {name}"),
            Some(format!(
                "Start the vault (`clio vault start`) and run `clio connect {name}`."
            )),
        ),
    }
}

/// The old key file is never read any more; if one is still there, say how to move it, or, when
/// the vault already holds the key, that it can go.
fn old_key_file(o: &mut Report, dirs: &Dirs) {
    let file = dirs.config.join("opencode-api-key");
    let in_vault = clio_brain::connector::by_name("opencode")
        .is_some_and(|c| super::vault::has_model_key(dirs, c));
    if file.exists() && in_vault {
        o.say(
            Level::Warn,
            format!(
                "a key is left in {} and it is already in the vault: rm {}",
                file.display(),
                file.display()
            ),
            Some(
                "Clio no longer reads that file and the vault already has the key: delete it."
                    .into(),
            ),
        );
    } else if file.exists() {
        o.say(
            Level::Warn,
            format!(
                "a key is left in {}: clio connect opencode moves it into the vault",
                file.display()
            ),
            Some("Clio no longer reads that file. Move the key into the vault with `clio connect opencode`.".into()),
        );
    }
}

/// Jev is optional: off, it is said and nothing else is checked; on, what it needs is: Python for
/// the adapter and the secret `jev` in the vault, allowed for the API's host and no other.
fn jev_report(o: &mut Report, dirs: &Dirs, jev: &super::home::JevCfg) {
    if !jev.enabled {
        o.say(
            Level::Pass,
            "Jev: off (it is optional: everything works without it)".into(),
            None,
        );
        return;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        o.say(
            Level::Warn,
            "Jev: it is on but python3 is missing for the adapter; going on without it".into(),
            Some("Install python3 or turn Jev off (`\"jev\": {\"enabled\": false}` in the configuration).".into()),
        );
        return;
    }
    let (secret, domain) = super::jev::key_for(jev.base_url.as_deref());
    let ready = super::vault::gateway_socket(dirs).ok().map(|s| {
        VaultClient::new(&s)
            .check(secret, "jev", Some(domain), "")
            .map_err(|e| e.to_string())
    });
    match ready {
        Some(Ok(())) => o.say(
            Level::Pass,
            format!("Jev: on; the key is in the vault and can only be used toward {domain}"),
            None,
        ),
        Some(Err(why)) => o.say(
            Level::Warn,
            format!("Jev: it is on but the key cannot be used ({})", sanitize::line(&why)),
            Some(format!("With the vault running and unlocked: `clio vault put {secret} --dest jev:{domain}`. Until then Clio goes on without Jev.")),
        ),
        None => o.say(
            Level::Warn,
            "Jev: it is on but there is no runtime folder to talk to the vault".into(),
            None,
        ),
    }
}

/// A vault made before the cost of the passphrase was raised still opens, but its passphrase is
/// cheaper to guess for whoever gets the file.
/// The memory as the person sees it: whether it opens (an old file is upgraded on first open),
/// its places, what waits for review and what is past its review cycle.
fn memory_report(o: &mut Report, dirs: &Dirs) {
    use clio_brain::memory_link::{EmbedModel, MemoryLink};
    // A look only: an old file is not upgraded from here (that takes the writer and changes it).
    let db = MemoryLink::db_path(&dirs.data);
    if db.is_file()
        && let Err(clio_memory::Error::SchemaTooOld(_)) = clio_memory::Engine::open(
            &db,
            clio_memory::OpenMode::ReadOnly,
            std::sync::Arc::new(clio_memory::SystemClock),
        )
    {
        o.say(
            Level::Pass,
            "memory: stored with the earlier schema; it updates itself the next time Clio uses it"
                .into(),
            None,
        );
        return;
    }
    let link = match MemoryLink::open_read(&dirs.data, &EmbedModel::none()) {
        Ok(Some(l)) => l.for_person(),
        Ok(None) => {
            o.say(Level::Pass, "memory: it does not exist yet".into(), None);
            return;
        }
        Err(e) => {
            o.say(
                Level::Warn,
                format!("memory: it cannot be opened ({})", sanitize::line(&e)),
                Some("If another process is updating it, try again in a moment.".into()),
            );
            return;
        }
    };
    let places = link.places().unwrap_or_default();
    let active: u64 = places.iter().map(|p| p.1).sum();
    let pending = link.pending().map(|v| v.len()).unwrap_or(0);
    let due = link.review_due().map(|v| v.len()).unwrap_or(0);
    let projects = places.iter().filter(|p| p.0.starts_with("project")).count();
    o.say(
        Level::Pass,
        format!(
            "memory: {active} active memories; {projects} project{}, the personal ones and the lessons",
            if projects == 1 { "" } else { "s" }
        ),
        None,
    );
    if pending > 0 {
        o.say(
            Level::Warn,
            format!("memory: {pending} wait for your review"),
            Some(
                "`clio memory list --pending`, then `clio memory promote ID` or `archive ID`."
                    .into(),
            ),
        );
    }
    sleep_report(o, &dirs.data, &dirs.home, now_secs());
    if due > 0 {
        o.say(
            Level::Warn,
            format!("memory: {due} are past their review date (they stay active)"),
            Some(
                "`clio memory review-due`; if they are still true, `clio memory reviewed ID`."
                    .into(),
            ),
        );
    }
}

/// When the memory last slept, whether the night was complete, and what it warned about.
fn sleep_report(o: &mut Report, data: &Path, home: &Path, now: i64) {
    let setup = "`clio memory setup sleep` installs the timer that makes it sleep at night.";
    let Some(r) = clio_memory::sleep::last_report(data) else {
        // A timer is there (this one or clio-memory's): it only has not run yet.
        if clio_memory::setup::sleep_present(home) {
            o.say(
                Level::Pass,
                "sleep: it has not slept yet (the timer is already installed)".into(),
                None,
            );
            return;
        }
        o.say(
            Level::Warn,
            "sleep: the memory has not slept yet".into(),
            Some(setup.into()),
        );
        return;
    };
    let days = (now - r.ended) / 86_400;
    let msg = format!(
        "sleep: last night {} ({})",
        clio_memory::sleep::night_of(r.started),
        if r.complete { "complete" } else { "incomplete" }
    );
    if !r.alarms.is_empty() {
        o.say(
            Level::Warn,
            format!("{msg}; alarms: {}", sanitize::line(&r.alarms.join("; "))),
            Some("`clio memory sleep --report` shows the report.".into()),
        );
    } else if days > 3 {
        o.say(
            Level::Warn,
            format!("{msg}, {days} days ago"),
            Some(format!("Is the timer there? {setup}")),
        );
    } else {
        o.say(Level::Pass, msg, None);
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn kdf_warning(status: &serde_json::Value) -> Option<String> {
    (status["kdf_current"] == false).then(|| {
        format!(
            "vault: the vault passphrase is stretched with an old cost ({})",
            sanitize::line(status["kdf"].as_str().unwrap_or("?"))
        )
    })
}

/// The vault's own values (S-18, S-58): the key patterns cannot know a value the vault holds that
/// looks like nothing in particular, so the running vault is asked whether any of its values is in
/// what Clio wrote. The vault answers file, secret name and form, never a value. A vault that is not
/// running, or is locked, does not know its values: that is said, not passed over.
fn vault_audit(o: &mut Report, dirs: &Dirs) {
    let Some(rt) = &dirs.runtime else {
        return; // the missing runtime folder already has its own line
    };
    let client = VaultClient::new(&rt.join(PERSON_SOCK));
    let status = client
        .call(&Request::Status)
        .ok()
        .filter(|r| r.ok)
        .and_then(|r| r.data);
    let Some(status) = status else {
        if dirs.data.join("vault.bin").exists() {
            o.say(
                Level::Warn,
                "vault: there is a vault but it is not running, its values were not checked against what Clio wrote (S-18)".into(),
                Some("`clio vault start` and run `clio doctor` again.".into()),
            );
        } else {
            o.say(
                Level::Pass,
                "vault: there is none (it is optional)".into(),
                None,
            );
        }
        return;
    };
    if let Some(line) = kdf_warning(&status) {
        o.say(
            Level::Warn,
            line,
            Some("`clio vault upgrade-kdf` (it asks for your passphrase; it leaves a copy of the earlier file that you can delete once you have checked the vault).".into()),
        );
    }
    if status["locked"] == true {
        o.say(
            Level::Warn,
            "vault: it is locked, it does not know its values and cannot say they are not in what Clio wrote (S-18)".into(),
            Some("Unlock it and run `clio doctor` again.".into()),
        );
        return;
    }
    let scan = Request::ScanDir {
        path: dirs.data.display().to_string(),
        skip: NOT_CLIO_OUTPUT
            .iter()
            .map(|n| (*n).to_owned())
            .chain(
                NOT_READ
                    .iter()
                    .map(|r| dirs.data.join(r).display().to_string()),
            )
            .collect(),
    };
    let hits = match client.call(&scan) {
        Ok(r) if r.ok => r.data.unwrap_or_default(),
        Ok(r) => {
            let why = r.error.unwrap_or_default();
            o.say(
                Level::Warn,
                format!("vault: it could not check what Clio wrote ({why})"),
                None,
            );
            return;
        }
        Err(e) => {
            o.say(
                Level::Warn,
                format!("vault: it could not check what Clio wrote ({e})"),
                None,
            );
            return;
        }
    };
    let mut found: Vec<String> = hits
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let file = Path::new(h["file"].as_str()?);
            let rel = file
                .strip_prefix(&dirs.data)
                .unwrap_or(file)
                .display()
                .to_string();
            Some(format!(
                "{} ({})",
                sanitize::line(&rel),
                sanitize::line(h["name"].as_str()?)
            ))
        })
        .collect();
    found.sort();
    found.dedup();
    if found.is_empty() {
        o.say(
            Level::Pass,
            "vault: no value of the vault in what Clio wrote (S-18)".into(),
            None,
        );
    } else {
        let more = found.len().saturating_sub(8);
        o.say(
            Level::Fail,
            format!(
                "vault: {} file(s) of Clio hold a value of the vault (S-18): {}{}",
                found.len(),
                found.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
                if more > 0 { format!(" and {more} more") } else { String::new() }
            ),
            Some("Inside ~/.local/share/clio. Delete the run or the file and rotate that secret (`clio vault`): someone may have seen it.".into()),
        );
    }
}

/// Pi's own jail (ADR 0009): on Linux Pi starts only inside it. With a connector that runs
/// Claude Code (the bridge) Pi does not start without it at all, switched off or not.
fn pi_jail_line(o: &mut Report, dirs: &Dirs) {
    use clio_brain::pi::jail::{Status, status};
    let bridge = Config::load(dirs)
        .ok()
        .and_then(|c| by_provider(&c.model.provider))
        .filter(|c| c.pi_extension.is_some())
        .map(|c| c.name);
    let no_claude = |name: &str| format!("; the connector {name} will NOT start without it");
    match status() {
        Status::Ready(v) => o.say(
            Level::Pass,
            format!("Pi jail: bwrap {v} starts; Pi runs without seeing the vault or your files and reaches only the model provider, and the self-test runs on every start (ADR 0009)"),
            None,
        ),
        Status::Off => o.say(
            Level::Fail,
            format!(
                "Pi jail: CLIO_PI_JAIL=off, Pi runs with your own permissions and can see the vault and your files{}",
                bridge.map(no_claude).unwrap_or_default()
            ),
            Some("Remove CLIO_PI_JAIL=off from the environment.".into()),
        ),
        Status::Unsupported => match bridge {
            Some(name) => o.say(
                Level::Fail,
                format!("Pi jail: this system cannot have it (only Linux with bubblewrap){}", no_claude(name)),
                Some("Use another connector on this system, for example `clio connect opencode`.".into()),
            ),
            None => o.say(
                Level::Warn,
                "Pi jail: this system cannot have it (only Linux with bubblewrap), Pi runs with your own permissions".into(),
                None,
            ),
        },
        Status::Unavailable(why) => o.say(
            Level::Fail,
            format!("Pi jail: it cannot start here, Pi will NOT start ({why})"),
            Some("It needs bubblewrap (bwrap), prlimit (util-linux) and unprivileged user namespaces; see docs/decisions/0009-pi-runs-inside-a-jail.md.".into()),
        ),
    }
}

fn find_host_check() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLIO_HOST_CHECK") {
        return Some(PathBuf::from(p));
    }
    // Only next to the executable, never in the working directory: the script runs outside the jail.
    super::find_above_exe(Path::new("infra/local/check-host.sh"))
}

fn host_lines(o: &mut Report, script: &Path) {
    // The script skips what `collect` already checked itself, so nothing appears twice.
    let out = match Command::new("bash")
        .arg(script)
        .env("CLIO_DOCTOR_EMBEDDED", "1")
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            o.say(
                Level::Warn,
                format!("the machine check could not be run: {e}"),
                None,
            );
            return;
        }
    };
    let before = o.checks.len();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let (lv, rest) = if let Some(r) = line.strip_prefix("PASS") {
            (Level::Pass, r)
        } else if let Some(r) = line.strip_prefix("WARN") {
            (Level::Warn, r)
        } else if let Some(r) = line.strip_prefix("FAIL") {
            (Level::Fail, r)
        } else {
            continue;
        };
        let hint = (lv != Level::Pass).then(|| {
            "From the machine check (infra/local/check-host.sh); see docs/usage.md.".to_owned()
        });
        o.say(lv, rest.trim().to_owned(), hint);
    }
    let host_failed = o.checks[before..].iter().any(|c| c.level == Level::Fail);
    if !out.status.success() && !host_failed {
        o.say(
            Level::Fail,
            "the machine check reported a failure".into(),
            None,
        );
    }
}

/// Host mode's console: which launcher runs the commands, and whether it passes its self-test
/// (in a scratch folder). Only when host mode is configured: the self-test runs commands, with
/// `sudo` where there is passwordless `sudo`.
fn host_mode_lines(o: &mut Report, dirs: &Dirs) {
    if !Config::load(dirs).is_ok_and(|c| c.mode == Mode::Host) {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("clio-doctor-host-{}", std::process::id()));
    let found = clio_node::HostJail::detect().map(|j| (j.launcher(), j.self_test(&scratch)));
    let _ = std::fs::remove_dir_all(&scratch);
    host_report(o, found);
}

fn host_report(o: &mut Report, found: Result<(clio_node::Launcher, Result<(), String>), String>) {
    use clio_node::Launcher;
    let fix = Some("It needs bubblewrap (bwrap) or passwordless sudo with systemd-run; without that Clio works without a console in host mode.".to_owned());
    match &found {
        // Off Linux there is no launcher to install: a warning, as for the jail.
        Err(why) => o.say(
            if cfg!(target_os = "linux") { Level::Fail } else { Level::Warn },
            format!("host mode: there is nothing to run the console with ({why}): there will be no console"),
            fix,
        ),
        Ok((l, Err(why))) => o.say(
            Level::Fail,
            format!("host mode: the console ({l:?}) does not pass its self-test ({why}): there will be no console"),
            fix,
        ),
        Ok((l, Ok(()))) => o.say(
            Level::Pass,
            format!("host mode: the console runs with {l:?} and passes its self-test (Clio's own hidden, network only when asked for)"),
            None,
        ),
    }
    if matches!(found, Ok((Launcher::Bwrap, _))) {
        o.say(
            Level::Warn,
            "Clio's console will have no sudo".into(),
            Some("With passwordless sudo (and systemd-run) the console would have it.".into()),
        );
    }
}

/// Runs every check. The host script's own lines are included as checks.
pub fn collect(dirs: &Dirs) -> Report {
    collect_with(dirs, true)
}

/// [`collect`]; without `host_self_test` host mode's console is not tried (that self-test runs
/// commands, with `sudo` where there is passwordless `sudo`): what the installer shows.
pub fn collect_with(dirs: &Dirs, host_self_test: bool) -> Report {
    collect_live(dirs, host_self_test, &mut |_| {})
}

/// What [`collect_live`] tells as it goes: the part it starts, and each check once it is made.
pub enum Live<'a> {
    Doing(&'static str),
    Done(&'a Check),
}

/// Hands the checks made since the last call to `f`, then what comes next.
struct Tell<'a> {
    shown: usize,
    f: &'a mut dyn FnMut(Live<'_>),
}

impl Tell<'_> {
    fn next(&mut self, o: &Report, doing: Option<&'static str>) {
        for c in &o.checks[self.shown..] {
            (self.f)(Live::Done(c));
        }
        self.shown = o.checks.len();
        if let Some(d) = doing {
            (self.f)(Live::Doing(d));
        }
    }
}

/// [`collect_with`], telling `f` each part as it starts and each check as soon as it is made.
pub fn collect_live(dirs: &Dirs, host_self_test: bool, f: &mut dyn FnMut(Live<'_>)) -> Report {
    let mut o = Report::default();
    let mut t = Tell { shown: 0, f };
    t.next(&o, Some("the private folders and the modes inside them"));
    let mut folders = vec![dirs.config.clone(), dirs.data.clone()];
    folders.extend(dirs.runtime.clone());
    for d in &folders {
        match mode(d) {
            None => o.say(
                Level::Warn,
                format!("{} does not exist", d.display()),
                Some("Run `clio init`.".into()),
            ),
            Some(m) if m & 0o077 != 0 => o.say(
                Level::Fail,
                format!("{} has mode {m:o}, it must be 700 (S-23)", d.display()),
                Some(format!("chmod 700 {}", d.display())),
            ),
            Some(_) => o.say(
                Level::Pass,
                format!("{} is private (700)", d.display()),
                None,
            ),
        }
        check_tree(&mut o, &dirs.data, d, 0);
    }
    if dirs.runtime.is_none() {
        o.say(
            Level::Warn,
            "there is no private runtime folder, the vault sockets cannot be checked".into(),
            Some("Log in through a normal session (XDG_RUNTIME_DIR), or remove the temporary clio-<uid> folder that is not yours.".into()),
        );
    }
    t.next(&o, Some("what Clio wrote, for anything shaped like a key"));
    data_audit(&mut o, &dirs.data);
    t.next(&o, Some("the timers"));
    if let Some(w) = super::services::timer_warning(dirs) {
        o.say(Level::Warn, w, Some("clio services enable".into()));
    }
    if let Some(w) = super::round::timer_warning(dirs) {
        o.say(Level::Warn, w, Some("clio services enable".into()));
    }
    t.next(&o, Some("the memory"));
    memory_report(&mut o, dirs);
    t.next(&o, Some("the vault's values in what Clio wrote"));
    vault_audit(&mut o, dirs);
    t.next(&o, Some("the configuration and the model's key"));
    match Config::load(dirs) {
        Ok(c) => {
            o.say(
                Level::Pass,
                format!(
                    "the configuration loads ({}/{})",
                    c.model.provider, c.model.model
                ),
                None,
            );
            jev_report(&mut o, dirs, &c.jev);
            // `Config::load` guarantees `model.provider` is a connector.
            let conn = by_provider(&c.model.provider).expect("a connector");
            connector_report(&mut o, dirs, conn);
            bridge_report(&mut o, dirs, conn);
            if names_builtin_adapter(&c.model.command) {
                // A path older versions of `init` wrote is not run: said, so it can be removed.
                let old = c.model.command.get(1).map_or_else(String::new, |p| {
                    format!(
                        "; model.command names {p}, which is no longer used and can be left as []"
                    )
                });
                o.say(
                    Level::Pass,
                    format!(
                        "the model adapter is the one this binary brings ({}){old}",
                        builtin_adapter_path(dirs, conn).display()
                    ),
                    None,
                );
            } else if let Some(script) = c.model.command.get(1)
                && !Path::new(script).is_file()
            {
                o.say(
                    Level::Fail,
                    format!("the model adapter {script} does not exist"),
                    Some("Fix model.command in ~/.config/clio/config.json, or leave it as [] to use the one Clio brings.".into()),
                );
            }
        }
        Err(e) => o.say(Level::Warn, e, Some("Run `clio init`.".into())),
    }
    old_key_file(&mut o, dirs);
    t.next(&o, Some("the sockets and what listens"));
    if let Some(rt) = &dirs.runtime
        && let Ok(rd) = std::fs::read_dir(rt)
    {
        for e in rd.filter_map(Result::ok) {
            if e.file_type().is_ok_and(|t| t.is_socket()) {
                let m = mode(&e.path()).unwrap_or(0o777);
                if m & 0o077 != 0 {
                    o.say(
                        Level::Fail,
                        format!(
                            "the socket {} has mode {m:o}, it must be 600",
                            e.path().display()
                        ),
                        Some(format!("chmod 600 {}", e.path().display())),
                    );
                } else {
                    o.say(
                        Level::Pass,
                        format!("the socket {} is 600", e.path().display()),
                        None,
                    );
                }
            }
        }
    }
    let l = public_listeners();
    if l.is_empty() {
        o.say(
            Level::Pass,
            "no clio process listens outside loopback (S-38)".into(),
            None,
        );
    } else {
        o.say(
            Level::Fail,
            format!("clio listens outside loopback: {}", l.join(", ")),
            Some("Stop that process: Clio must not listen on a public address.".into()),
        );
    }
    t.next(&o, Some("Node and Pi"));
    for (level, msg, hint) in super::pi::checks(dirs) {
        o.say(level, msg, hint);
    }
    t.next(&o, Some("the jail's self-test"));
    jail_lines(&mut o);
    t.next(&o, Some("Pi's jail"));
    pi_jail_line(&mut o, dirs);
    if host_self_test {
        t.next(&o, Some("host mode's console"));
        host_mode_lines(&mut o, dirs);
    }
    // Outside the repository the script is not there, and that is no problem: nothing is said.
    if let Some(script) = find_host_check() {
        t.next(&o, Some("the machine"));
        o.host_script = Some(script.display().to_string());
        host_lines(&mut o, &script);
    }
    t.next(&o, None);
    o
}

fn level_code(l: Level) -> &'static str {
    match l {
        Level::Pass => "pass",
        Level::Warn => "warn",
        Level::Fail => "fail",
    }
}

/// A check as the plain command prints it: its line, and the hint under a WARN or a FAIL.
fn check_lines(st: Style, c: &Check) -> String {
    let mut s = format!("{}  {}\n", st.mark(c.level), sanitize::line(&c.msg));
    if let (Some(h), true) = (&c.hint, c.level != Level::Pass) {
        s += &format!(
            "{}{} {}\n",
            " ".repeat(st.mark_width() + 2),
            st.dim(st.arrow()),
            st.dim(&sanitize::line(h))
        );
    }
    s
}

/// The plain command. Returns true when nothing failed (WARN is the person's call).
pub fn doctor(dirs: &Dirs, json: bool) -> bool {
    let st = Style::stdout();
    if json {
        let r = collect(dirs);
        let checks: Vec<_> = r
            .checks
            .iter()
            .map(|c| json!({"level": level_code(c.level), "message": c.msg, "hint": c.hint}))
            .collect();
        let v = json!({
            "ok": r.ok(),
            "pass": r.count(Level::Pass),
            "warn": r.count(Level::Warn),
            "fail": r.count(Level::Fail),
            "host_script": r.host_script,
            "jail": r.jail,
            "checks": checks,
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return r.ok();
    }
    // Each line as its check is made; on a terminal, what is being checked meanwhile, on a line
    // the result replaces.
    let mut shown = 0;
    let r = collect_live(dirs, true, &mut |l| {
        use std::io::Write;
        if shown > 0 {
            print!("\r{}\r", " ".repeat(std::mem::take(&mut shown)));
        }
        match l {
            Live::Doing(what) if st.tty => {
                let line = format!("{} checking {what}", st.ellipsis());
                shown = line.chars().count();
                print!("{}", st.dim(&line));
            }
            Live::Doing(_) => {}
            Live::Done(c) => print!("{}", check_lines(st, c)),
        }
        let _ = std::io::stdout().flush();
    });
    println!();
    println!(
        "{}",
        if r.ok() {
            st.green(&format!(
                "doctor: good ({} ok, {}; the warnings are your call)",
                r.count(Level::Pass),
                plural(r.count(Level::Warn), "warning", "warnings")
            ))
        } else {
            st.red(&format!(
                "doctor: {} ({} ok, {})",
                plural(r.count(Level::Fail), "failure", "failures"),
                r.count(Level::Pass),
                plural(r.count(Level::Warn), "warning", "warnings")
            ))
        }
    );
    r.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_section_says_the_launcher_and_warns_when_there_is_no_sudo() {
        use clio_node::Launcher;
        let said = |found| {
            let mut o = Report::default();
            host_report(&mut o, found);
            o.checks
                .iter()
                .map(|c| (c.level, c.msg.clone()))
                .collect::<Vec<_>>()
        };
        let sudo = said(Ok((Launcher::Systemd, Ok(()))));
        assert_eq!(sudo.len(), 1);
        assert!(
            sudo[0].0 == Level::Pass && sudo[0].1.contains("Systemd"),
            "{sudo:?}"
        );
        let bwrap = said(Ok((Launcher::Bwrap, Ok(()))));
        assert_eq!(bwrap[0].0, Level::Pass);
        assert_eq!(
            bwrap[1],
            (Level::Warn, "Clio's console will have no sudo".into())
        );
        let broken = said(Ok((Launcher::Bwrap, Err("x".into()))));
        assert_eq!(broken[0].0, Level::Fail);
        let none = if cfg!(target_os = "linux") {
            Level::Fail
        } else {
            Level::Warn
        };
        assert_eq!(said(Err("nothing".into()))[0].0, none);
    }

    // A person who makes `~/.config/clio/commands/` by hand gets 755 and 644: the check is not
    // weakened for it, and each failure says the command that fixes it.
    #[test]
    fn a_commands_folder_made_with_default_permissions_fails_and_says_how_to_fix_it() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("commands");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("review.md"), "# Review\nReview $ARGUMENTS").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(
            dir.join("review.md"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let mut o = Report::default();
        check_tree(&mut o, t.path(), t.path(), 0);
        let said: Vec<_> = o
            .checks
            .iter()
            .map(|c| (c.level, c.msg.clone(), c.hint.clone().unwrap_or_default()))
            .collect();
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said.iter().all(|(l, ..)| *l == Level::Fail), "{said:?}");
        assert!(said[0].1.contains("it must be 700 (S-23)"), "{said:?}");
        assert_eq!(said[0].2, format!("chmod 700 {}", dir.display()));
        assert_eq!(
            said[1].2,
            format!("chmod 600 {}", dir.join("review.md").display())
        );
    }

    // S-69: a service's state folder is the service's: its own mode is checked (0700), what it
    // wrote inside is neither walked for S-23 nor scanned for key shapes.
    #[test]
    fn a_services_state_folder_is_checked_but_not_walked() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let state = data.join("services/state");
        let kept = data.join("services/removed/old-1/state");
        for d in [state.join("a"), state.join("b"), kept.clone()] {
            std::fs::create_dir_all(&d).unwrap();
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::write(d.join("token"), "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
            std::fs::set_permissions(d.join("token"), std::fs::Permissions::from_mode(0o644))
                .unwrap();
        }
        for d in [
            &data,
            &data.join("services"),
            &state,
            &data.join("services/removed"),
            &data.join("services/removed/old-1"),
        ] {
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::set_permissions(state.join("b"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut o = Report::default();
        check_tree(&mut o, &data, &data, 0);
        let said: Vec<_> = o.checks.iter().map(|c| c.msg.clone()).collect();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("state/b has mode 755"), "{said:?}");
        let mut found = (0, Vec::new(), 0);
        audit_walk(&data, &data, 0, &Scrubber::default(), &mut found);
        assert!(found.1.is_empty(), "{:?}", found.1);
    }

    // S-70: an MCP server's data folder is its `HOME` in the jail, where Chromium makes its caches
    // 755: the folder's own mode is checked, what the server wrote inside is not (live or removed),
    // for S-23 nor for key shapes. Only at those places from the data root: a 755 folder anywhere
    // else (the rest of a removed copy, the same names inside a skill or a project) still fails.
    #[test]
    fn an_mcp_servers_data_folder_is_checked_but_not_walked() {
        use std::os::unix::fs::PermissionsExt;
        fn private(d: &Path) {
            std::fs::set_permissions(d, PermissionsExt::from_mode(0o700)).unwrap();
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    private(&p);
                }
            }
        }
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let skipped = ["mcp/data/playwright", "mcp/removed/playwright-1/data"];
        let walked = [
            "mcp/servers/stray",
            "mcp/removed/playwright-1/servers",
            "skills/versions/s/1/mcp/data/x",
            "skills/live/s/mcp/removed/a/data",
            "services/versions/v/2/mcp/data/x",
            "sandbox/proj/mcp/data/x",
        ];
        for d in skipped.iter().chain(&walked) {
            std::fs::create_dir_all(data.join(d).join(".cache/fontconfig")).unwrap();
        }
        std::fs::create_dir_all(data.join("mcp/data/notion")).unwrap();
        private(&data);
        let open =
            |d: &Path| std::fs::set_permissions(d, PermissionsExt::from_mode(0o755)).unwrap();
        for d in skipped {
            open(&data.join(d).join(".cache"));
            open(&data.join(d).join(".cache/fontconfig"));
        }
        let mut want: Vec<PathBuf> = walked.iter().map(|d| data.join(d).join(".cache")).collect();
        want.extend(["mcp/data/notion", "mcp/removed/playwright-1"].map(|d| data.join(d)));
        for d in &want {
            open(d);
        }
        let mut o = Report::default();
        check_tree(&mut o, &data, &data, 0);
        let mut said: Vec<_> = o.checks.iter().map(|c| c.msg.clone()).collect();
        let mut want: Vec<_> = want
            .iter()
            .map(|d| format!("{} has mode 755, it must be 700 (S-23)", d.display()))
            .collect();
        said.sort();
        want.sort();
        assert_eq!(said, want);
        // Key shapes: what the server wrote is its own; Clio's files beside it are scanned.
        let key = "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        std::fs::write(data.join("mcp/data/playwright/token"), key).unwrap();
        std::fs::write(data.join("mcp/servers/stray/token"), key).unwrap();
        let mut found = (0, Vec::new(), 0);
        audit_walk(&data, &data, 0, &Scrubber::default(), &mut found);
        assert_eq!(found.1, vec![data.join("mcp/servers/stray/token")]);
    }

    // The private Node is nodejs.org's files, not what Clio wrote: a key-looking string inside
    // its binary is not a leak (S-18); the same string in Clio's own files is.
    #[test]
    fn the_private_node_is_not_scanned_for_leaks() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        std::fs::create_dir_all(data.join("node/bin")).unwrap();
        std::fs::write(
            data.join("node/bin/node"),
            "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )
        .unwrap();
        let mut found = (0, Vec::new(), 0);
        audit_walk(&data, &data, 0, &Scrubber::default(), &mut found);
        assert!(found.1.is_empty(), "{:?}", found.1);
        std::fs::write(
            data.join("notes.txt"),
            "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )
        .unwrap();
        audit_walk(&data, &data, 0, &Scrubber::default(), &mut found);
        assert_eq!(found.1.len(), 1, "{:?}", found.1);
    }

    // Chromium and npm's store of the registry's files are not read for key shapes, only there
    // from the data root; npm's index beside the store and the same names elsewhere still are,
    // and the modes inside the skipped trees are still checked (S-23).
    #[test]
    fn chromium_and_npms_content_store_are_not_read_but_their_modes_are_checked() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let key = "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let skipped = [
            "browsers/chromium-1255/resources.pak",
            "mcp/npm-cache/_cacache/content-v2/sha512/aa/bb",
        ];
        let read = [
            "mcp/npm-cache/_cacache/index-v5/aa/bb",
            "skills/live/s/browsers/notes.md",
            "runs/r/mcp/npm-cache/_cacache/content-v2/x",
        ];
        for f in skipped.iter().chain(&read) {
            let p = data.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, key).unwrap();
        }
        let mut found = (0, Vec::new(), 0);
        audit_walk(&data, &data, 0, &Scrubber::default(), &mut found);
        found.1.sort();
        let mut want: Vec<PathBuf> = read.iter().map(|f| data.join(f)).collect();
        want.sort();
        assert_eq!(found.1, want);
        let open = data.join("browsers/chromium-1255/resources.pak");
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut o = Report::default();
        check_tree(&mut o, &data, &data, 0);
        assert!(
            o.checks
                .iter()
                .any(|c| c.msg == format!("{} has mode 644, it must be 600 (S-23)", open.display())),
            "{:?}",
            o.checks.iter().map(|c| &c.msg).collect::<Vec<_>>()
        );
    }

    // A trash with deleted conversations in it is private all through (S-23), even when the
    // session file was 0644 when it was deleted.
    #[test]
    fn a_populated_trash_passes_the_private_mode_check() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
        let file = t.path().join("2026_r1.jsonl");
        std::fs::write(&file, "{}").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let p = super::super::sessions::Plan {
            file,
            trash: data.join("trash/sessions"),
            id: "r1".into(),
            lock: None,
            title: "t".into(),
            run: None,
            unapplied: Some(0),
            claude: vec![],
        };
        super::super::sessions::delete(&p).unwrap();
        let mut o = Report::default();
        check_tree(&mut o, &data, &data, 0);
        assert!(
            o.checks.is_empty(),
            "{:?}",
            o.checks.iter().map(|c| &c.msg).collect::<Vec<_>>()
        );
    }

    #[test]
    fn jev_is_off_by_default_and_an_old_config_without_it_still_loads() {
        assert!(!super::super::home::JevCfg::default().enabled);
        let mut v = serde_json::to_value(Config::default_with(vec![])).unwrap();
        v.as_object_mut().unwrap().remove("jev");
        let c: Config = serde_json::from_value(v).unwrap();
        assert!(!c.jev.enabled && c.jev.memory && c.jev.tool_hints);
    }

    #[test]
    fn doctor_says_when_the_memory_last_slept() {
        let t = tempfile::tempdir().unwrap();
        let said = |now| {
            let mut o = Report::default();
            sleep_report(&mut o, t.path(), t.path(), now);
            let c = o.checks.pop().unwrap();
            (c.level, c.msg)
        };
        let (level, msg) = said(1_800_000_000);
        assert!(
            matches!(level, Level::Warn) && msg.contains("has not slept yet"),
            "{msg}"
        );
        // With the timer installed it only has not run yet: no warning, nothing to install.
        let unit = if cfg!(target_os = "macos") {
            t.path()
                .join("Library/LaunchAgents/com.clio-memory.sleep.plist")
        } else {
            t.path()
                .join(".config/systemd/user/clio-memory-sleep.service")
        };
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(&unit, "").unwrap();
        let (level, msg) = said(1_800_000_000);
        assert!(
            matches!(level, Level::Pass) && msg.contains("the timer is already installed"),
            "{msg}"
        );
        let mut r = clio_memory::sleep::Report {
            started: 1_800_000_000,
            ended: 1_800_000_600,
            complete: true,
            ..Default::default()
        };
        let write = |r: &clio_memory::sleep::Report| {
            let dir = t.path().join("sleep");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{}.json", clio_memory::sleep::night_of(r.started))),
                serde_json::to_string(r).unwrap(),
            )
            .unwrap();
        };
        write(&r);
        let (level, msg) = said(1_800_003_600);
        assert!(
            matches!(level, Level::Pass) && msg.contains("complete"),
            "{msg}"
        );
        let (level, msg) = said(1_800_000_000 + 5 * 86_400);
        assert!(
            matches!(level, Level::Warn) && msg.contains("4 days ago"),
            "{msg}"
        );
        r.alarms.push("the backup failed".into());
        write(&r);
        let (level, msg) = said(1_800_003_600);
        assert!(
            matches!(level, Level::Warn) && msg.contains("the backup failed"),
            "{msg}"
        );
    }

    #[test]
    fn an_old_vault_cost_is_warned_about_and_a_current_one_is_not() {
        let old = serde_json::json!({"kdf_current": false, "kdf": "argon2id m=19 MiB t=2 p=1"});
        assert!(kdf_warning(&old).unwrap().contains("m=19 MiB"));
        assert!(kdf_warning(&serde_json::json!({"kdf_current": true})).is_none());
        assert!(kdf_warning(&serde_json::json!({"locked": false})).is_none());
    }
}
