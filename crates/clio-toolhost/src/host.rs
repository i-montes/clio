//! Host mode: the tools act on the person's real machine (`/`), never on Clio's own folders
//! (`control`). Every change of a file is copied first (`changes.jsonl`, undone by `undo`), and
//! every command or change leaves a line in the action log before it runs and another after.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clio_node::{Action, HostJail, HostJob, Launcher, Limits};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ask::HostRules;
use crate::{Approved, Request, ToolHost, ops};

pub struct HostCfg {
    /// Relative paths are taken from here, and commands start here.
    pub home: PathBuf,
    /// `Dirs.data/host`: the action log, the pending actions and the day's questions.
    pub state: PathBuf,
    /// Clio's own folders: the file tools refuse them and commands see them empty.
    pub control: Vec<PathBuf>,
    /// The factory list plus the person's `host.json`; `control` is added to it here.
    pub rules: HostRules,
    /// The run id, for the log.
    pub run: String,
    /// `None`: `HostJail::detect` (it tries `sudo`). Tests pick `Bwrap`, which never does.
    pub launcher: Option<Launcher>,
}

pub(crate) struct Host {
    pub(crate) cfg: HostCfg,
    /// Only when the launcher passed its self-test: otherwise there is no `exec`.
    jail: Option<HostJail>,
}

pub(crate) const CONTROL: &str = "eso es de Clio: solo la persona lo toca";
/// Tools that change something: they are logged, and set aside when the list says so.
pub(crate) const LOGGED: [&str; 3] = ["exec", "fs_write", "fs_mkdir"];
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 3600;

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn sha256(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// Where `p` really lands: canonical, or for a path that does not exist yet, its longest existing
/// ancestor made canonical plus the rest. So neither a link nor `..` hides it. `None`: a dangling
/// link or a `..` past what exists.
fn real(p: &Path) -> Option<PathBuf> {
    let (mut base, mut tail) = (p.to_path_buf(), Vec::new());
    loop {
        if let Ok(c) = base.canonicalize() {
            return Some(tail.iter().rev().fold(c, |acc, n| acc.join(n)));
        }
        if fs::symlink_metadata(&base).is_ok_and(|m| m.is_symlink()) {
            return None;
        }
        tail.push(base.file_name()?.to_owned());
        base.pop();
    }
}

impl Host {
    pub(crate) fn new(mut cfg: HostCfg, scratch: &Path, exec: bool) -> (Self, Result<(), String>) {
        // Both spellings: a folder made later is still caught by the one given.
        let mut control = cfg.control.clone();
        control.extend(cfg.control.iter().filter_map(|c| c.canonicalize().ok()));
        control.dedup();
        cfg.rules = cfg.rules.with_control(&cfg.home, &control);
        cfg.control = control;
        let jail = if exec {
            cfg.launcher
                .map_or_else(HostJail::detect, HostJail::with)
                .and_then(|j| j.self_test(scratch).map(|()| j))
        } else {
            Err("the jail is switched off".to_owned())
        };
        let status = jail.as_ref().map(|_| ()).map_err(Clone::clone);
        (
            Self {
                cfg,
                jail: jail.ok(),
            },
            status,
        )
    }

    /// The real path of a file tool's `path`, unless it is Clio's: `Err((rule, why))`.
    pub(crate) fn path(&self, raw: &str) -> Result<PathBuf, (&'static str, String)> {
        let p = real(Path::new(raw)).ok_or(("host/path", format!("cannot resolve {raw:?}")))?;
        if self.cfg.control.iter().any(|c| p.starts_with(c)) {
            return Err(("host/control", CONTROL.into()));
        }
        Ok(p)
    }

    /// One line of `actions.jsonl` (0600). An `Err` before an action means it must not run.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn log(
        &self,
        session: &str,
        tool: &str,
        what: &str,
        network: bool,
        outcome: &str,
        rule: &str,
        exit_code: Option<i64>,
    ) -> std::io::Result<()> {
        private_dir(&self.cfg.state)?;
        let mut line = json!({
            "at_ms": now_ms(),
            "run": self.cfg.run,
            "session": session,
            "tool": tool,
            "what": what,
            "network": network,
            "outcome": outcome,
            "rule": rule,
            "exit_code": exit_code,
        })
        .to_string();
        line.push('\n');
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.cfg.state.join("actions.jsonl"))?
            .write_all(line.as_bytes())
    }
}

/// A folder of Clio's data, 0700 whatever the umask, and closed again if an older version left it
/// open (S-23).
pub(crate) fn private_dir(d: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(d)?;
    if fs::metadata(d)?.permissions().mode() & 0o7777 != 0o700 {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// One line of `changes.jsonl`. `sha256` is what was written; `null` for a folder.
#[derive(Serialize, Deserialize)]
struct Change {
    seq: u64,
    path: PathBuf,
    backup: Option<PathBuf>,
    mode: Option<u32>,
    sha256: Option<String>,
    at_ms: i64,
}

/// Copy before change: the folders a write or mkdir will create, and the file it will replace.
fn record_change(run_dir: &Path, path: &Path, content: Option<&[u8]>) -> std::io::Result<()> {
    let log = run_dir.join("changes.jsonl");
    let first = fs::read_to_string(&log).map_or(0, |t| t.lines().count() as u64);
    let dir = if content.is_some() {
        path.parent()
    } else {
        Some(path)
    };
    let mut new: Vec<PathBuf> = dir
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|d| fs::symlink_metadata(d).is_err())
        .map(Path::to_path_buf)
        .collect();
    new.reverse();
    let mut changes: Vec<Change> = new
        .into_iter()
        .map(|path| Change {
            seq: 0,
            path,
            backup: None,
            mode: None,
            sha256: None,
            at_ms: now_ms(),
        })
        .collect();
    if let Some(content) = content {
        let seq = first + changes.len() as u64;
        let (mut backup, mut mode) = (None, None);
        if let Ok(m) = fs::metadata(path) {
            let b = run_dir.join("backups").join(seq.to_string());
            private_dir(&run_dir.join("backups"))?;
            fs::copy(path, &b)?;
            // `copy` keeps the original's mode; the backup is the person's, the mode is in `mode`.
            fs::set_permissions(&b, fs::Permissions::from_mode(0o600))?;
            (backup, mode) = (Some(b), Some(m.permissions().mode() & 0o7777));
        }
        changes.push(Change {
            seq,
            path: path.to_path_buf(),
            backup,
            mode,
            sha256: Some(sha256(content)),
            at_ms: now_ms(),
        });
    }
    let mut lines = String::new();
    for (c, seq) in changes.iter_mut().zip(first..) {
        c.seq = seq;
        lines.push_str(&serde_json::to_string(c).map_err(std::io::Error::other)?);
        lines.push('\n');
    }
    if lines.is_empty() {
        return Ok(());
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)?
        .write_all(lines.as_bytes())
}

/// Undoes a host session's file changes, last first: a replaced file gets its content and mode
/// back, a new file and a new empty folder go. A file that changed after Clio wrote it (its
/// `sha256` is no longer the written one) is left as it is. Returns, for the person, what was done
/// and what was not; empty when there is nothing (left) to undo. The journal is then set aside
/// as `changes.undone.jsonl`, so a second undo does not go over it again.
pub fn undo(run_dir: &Path) -> Result<Vec<String>, String> {
    let journal = run_dir.join("changes.jsonl");
    let text = match fs::read_to_string(&journal) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("changes.jsonl: {e}")),
    };
    let mut said = Vec::new();
    for line in text.lines().rev() {
        let c: Change = serde_json::from_str(line).map_err(|e| format!("changes.jsonl: {e}"))?;
        let p = c.path.display();
        let Some(written) = c.sha256 else {
            if c.path.is_dir() {
                said.push(match fs::remove_dir(&c.path) {
                    Ok(()) => format!("borrada la carpeta {p}"),
                    Err(_) => format!("no se tocó la carpeta {p}: no está vacía"),
                });
            }
            continue;
        };
        let Ok(now) = fs::read(&c.path) else {
            continue; // already gone: nothing to give back over
        };
        if sha256(&now) != written {
            said.push(format!("no se tocó {p}: cambió después"));
            continue;
        }
        let done = match (&c.backup, c.mode) {
            (Some(b), Some(mode)) => restore(b, &c.path, mode).map(|warn| match warn {
                None => format!("restaurado {p}"),
                Some(e) => {
                    format!("restaurado {p}, pero no se pudo devolverle el modo {mode:o}: {e}")
                }
            }),
            _ => fs::remove_file(&c.path).map(|()| format!("borrado {p}")),
        };
        said.push(done.unwrap_or_else(|e| format!("no se pudo deshacer {p}: {e}")));
    }
    fs::rename(&journal, run_dir.join("changes.undone.jsonl"))
        .map_err(|e| format!("changes.jsonl: {e}"))?;
    Ok(said)
}

/// Gives `path` the content of `backup` in place (no `chmod` before it: `fs::copy` truncates,
/// then sets the mode, and on a file of another owner that `chmod` fails once it is empty), then
/// its saved `mode` if it differs. A mode that cannot be set comes back as the warning; the
/// content stays restored.
fn restore(backup: &Path, path: &Path, mode: u32) -> std::io::Result<Option<std::io::Error>> {
    fs::write(path, fs::read(backup)?)?;
    if fs::metadata(path)?.permissions().mode() & 0o7777 == mode {
        return Ok(None);
    }
    Ok(fs::set_permissions(path, fs::Permissions::from_mode(mode)).err())
}

impl ToolHost {
    /// An action already allowed (by the gateway, or by the person): logged before and after,
    /// copied before it changes a file, run, and the session marked by what it read. `path` is
    /// absolute and `real` its checked real location. Returns `(ok, content, data, external)`, or
    /// `Err` with the answer when it did not run because its line before could not be logged.
    pub(crate) fn host_run(
        &self,
        h: &Host,
        req: &Request,
        action_id: String,
        rule: &str,
        real: Option<&Path>,
    ) -> Result<(bool, String, Value, bool), String> {
        let tool = req.tool.as_str();
        let args = req.args.as_object().cloned().unwrap_or_default();
        let command = args.get("command").and_then(Value::as_str).unwrap_or("");
        let network = args.get("network").and_then(Value::as_bool) == Some(true);
        let logged = LOGGED.contains(&tool);
        let what = self.clean(&match real {
            Some(r) => r.to_string_lossy().into_owned(),
            None => command.to_owned(),
        });
        if logged && let Err(e) = h.log(&req.session, tool, &what, network, "run", rule, None) {
            return Err(format!(
                "[toolhost] {tool} was not run: the action log cannot be written ({e})"
            ));
        }
        let ran = catch_unwind(AssertUnwindSafe(|| {
            self.host_effect(h, tool, &args, real, action_id)
        }))
        .unwrap_or_else(|_| Err("the tool panicked".into()));
        let (ok, content, data, external) = match ran {
            Ok(result) => self.present(req, &result),
            Err(e) => (
                false,
                format!("{tool} failed: {}", self.clean(&e)),
                Value::Null,
                false,
            ),
        };
        if logged {
            let code = data.get("exit_code").and_then(Value::as_i64);
            if let Err(e) = h.log(&req.session, tool, &what, network, "done", rule, code) {
                eprintln!("clio-toolhost: the action log lost a line: {e}");
            }
        }
        let foreign = match tool {
            "fs_read" | "fs_list" | "fs_glob" => real.is_some_and(|r| h.cfg.rules.is_foreign(r)),
            "exec" => h.cfg.rules.reads_foreign(command),
            _ => false,
        };
        if tool == "exec" && (network || h.cfg.rules.reaches_out(command)) {
            self.mark(&req.session, "web");
        }
        if foreign {
            self.mark(&req.session, "foreign");
        }
        Ok((ok, content, data, external))
    }

    fn host_effect(
        &self,
        h: &Host,
        tool: &str,
        args: &Map<String, Value>,
        real: Option<&Path>,
        action_id: String,
    ) -> Result<Value, String> {
        if tool == "exec" {
            let jail = h.jail.as_ref().ok_or("there is no host launcher")?;
            let secs = args
                .get("timeout")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS);
            let out = jail.run(&HostJob {
                command: args
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                cwd: h.cfg.home.clone(),
                home: h.cfg.home.clone(),
                network: args.get("network").and_then(Value::as_bool) == Some(true),
                timeout: Duration::from_secs(secs),
                max_output: Limits::default().max_output,
                masks: h.cfg.control.clone(),
            })?;
            return Ok(ops::exec_result(&out, secs));
        }
        let real = real.ok_or("a file tool needs a path")?;
        if matches!(tool, "fs_write" | "fs_mkdir") {
            let content = args.get("content").and_then(Value::as_str);
            record_change(&self.run_dir, real, content.map(str::as_bytes)).map_err(|e| {
                format!("the copy before the change failed, nothing was changed: {e}")
            })?;
        }
        // The sandbox's root is `/`: it takes the path relative to it.
        let mut args = args.clone();
        let rel = real.strip_prefix("/").unwrap_or(real);
        args.insert(
            "path".into(),
            Value::from(rel.to_string_lossy().into_owned()),
        );
        if tool == "fs_glob" {
            let skip: Vec<Value> = h
                .cfg
                .control
                .iter()
                .map(|c| Value::from(c.to_string_lossy().into_owned()))
                .collect();
            args.insert("skip".into(), Value::Array(skip));
        }
        let action = Action {
            id: action_id,
            tool: tool.into(),
            args: Value::Object(args),
        };
        self.executor
            .execute(&action, &Approved(action.hash()))
            .map(|o| o.result)
            .map_err(|e| e.to_string())
    }
}
