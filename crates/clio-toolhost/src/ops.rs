//! The primitive operations Pi's own tools are made of (read a file, write a file, stat, make a
//! folder, list, glob, run a command). Pi keeps its tools (truncation, diff, path resolution,
//! rendering); only these primitives are served here, inside the session's working copy.
//!
//! They are `clio-node` tools: the executor runs them only after the gateway allowed the exact
//! action, and `exec` runs ONLY in the `bwrap` jail (the one free shell, S-12).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clio_core::Effect;
use clio_node::jail::{Jail, JailOutput, Job};
use clio_node::{Error, Result, Sandbox, Tool};
use serde_json::{Value, json};

use crate::PROTECTED_PATHS;

/// The largest file `fs_read` returns, and the most `exec` output that is kept.
const MAX_FILE_BYTES: u64 = 8 << 20;
const BASH: &str = "/usr/bin/bash";
const MAX_TIMEOUT_SECS: u64 = 120;
/// shortcut: in host mode a glob stops after this many folders (a glob on `/` would run for
/// minutes), so a match past it is not found; raise it or add a time limit if real searches hit it.
const MAX_GLOB_DIRS: usize = 20_000;

fn text<'a>(a: &'a Value, k: &str) -> Result<&'a str> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::BadArgs(format!("missing string `{k}`")))
}

fn tool(
    name: &str,
    description: &str,
    effect: Effect,
    run: fn(&Sandbox, &Value) -> Result<Value>,
) -> Tool {
    Tool {
        name: name.into(),
        description: description.into(),
        keywords: vec![],
        effect: Some(effect),
        visible: true,
        run,
    }
}

/// `host`: `exec` is declared here but served by `ToolHost` itself, with the host launcher.
pub fn tools(jail: bool, host: bool) -> Vec<Tool> {
    let mut v = vec![
        tool(
            "fs_read",
            "Read a file of the working copy",
            Effect::Read,
            fs_read,
        ),
        tool(
            "fs_stat",
            "Stat or check access to a path of the working copy",
            Effect::Read,
            fs_stat,
        ),
        tool(
            "fs_list",
            "List a folder of the working copy",
            Effect::Read,
            fs_list,
        ),
        tool(
            "fs_glob",
            "Find files by glob pattern in the working copy",
            Effect::Read,
            fs_glob,
        ),
        tool(
            "fs_write",
            "Write a file of the working copy (atomic replace)",
            Effect::LocalReversible,
            |s, a| {
                s.write(text(a, "path")?, text(a, "content")?.as_bytes())?;
                Ok(json!({}))
            },
        ),
        tool(
            "fs_mkdir",
            "Make a folder in the working copy",
            Effect::LocalReversible,
            |s, a| {
                s.mkdir(text(a, "path")?)?;
                Ok(json!({}))
            },
        ),
    ];
    if jail && host {
        v.push(tool(
            "exec",
            "Run a shell command on the host (network only when asked, Clio's folders hidden)",
            Effect::LocalReversible,
            |_, _| {
                Err(Error::Jail(
                    "host commands are not run by the executor".into(),
                ))
            },
        ));
    } else if jail {
        v.push(tool(
            "exec",
            "Run a shell command inside the jail (no network, only the working copy)",
            Effect::LocalReversible,
            exec,
        ));
    }
    v
}

fn fs_read(s: &Sandbox, a: &Value) -> Result<Value> {
    let file = s.open_regular(text(a, "path")?, false)?;
    let len = file.metadata()?.len();
    if len > MAX_FILE_BYTES {
        return Err(Error::BadArgs(format!(
            "the file is {len} bytes, more than the {MAX_FILE_BYTES} allowed"
        )));
    }
    let mut buf = Vec::new();
    file.take(MAX_FILE_BYTES).read_to_end(&mut buf)?;
    Ok(json!({ "content": String::from_utf8_lossy(&buf) }))
}

fn fs_stat(s: &Sandbox, a: &Value) -> Result<Value> {
    let path = text(a, "path")?;
    let p = s.resolve(path)?;
    let mode = a.get("mode").and_then(Value::as_str).unwrap_or("exists");
    let Ok(meta) = fs::metadata(&p) else {
        if mode == "exists" {
            return Ok(json!({"exists": false, "is_dir": false}));
        }
        return Err(Error::BadArgs("no such file or folder".into()));
    };
    match (mode, meta.is_dir()) {
        ("exists", _) => {}
        ("r", true) => drop(fs::read_dir(&p)?),
        ("rw", true) => return Err(Error::BadArgs(format!("{path} is a folder"))),
        ("r", false) => drop(s.open_regular(path, false)?),
        ("rw", false) => {
            drop(s.open_regular(path, false)?);
            drop(s.open_regular(path, true)?);
        }
        (other, _) => return Err(Error::BadArgs(format!("unknown mode {other:?}"))),
    }
    Ok(json!({"exists": true, "is_dir": meta.is_dir()}))
}

fn fs_list(s: &Sandbox, a: &Value) -> Result<Value> {
    Ok(json!({ "entries": s.list(text(a, "path")?)? }))
}

/// `*` and `?` stay inside one path segment; `**` crosses segments.
pub fn glob_match(pat: &[u8], txt: &[u8]) -> bool {
    match (pat.first(), txt.first()) {
        (None, None) => true,
        (Some(b'*'), _) if pat.starts_with(b"**") => {
            let rest = pat[2..].strip_prefix(b"/").unwrap_or(&pat[2..]);
            (0..=txt.len()).any(|i| glob_match(rest, &txt[i..]))
        }
        (Some(b'*'), _) => (0..=txt.len())
            .take_while(|&i| i == 0 || txt[i - 1] != b'/')
            .any(|i| glob_match(&pat[1..], &txt[i..])),
        (Some(b'?'), Some(c)) if *c != b'/' => glob_match(&pat[1..], &txt[1..]),
        (Some(p), Some(c)) if p == c => glob_match(&pat[1..], &txt[1..]),
        _ => false,
    }
}

/// What a glob walk carries: the folders it never enters (host mode: Clio's own) and how many
/// folders it may still visit.
struct Walk<'a> {
    skip: &'a [PathBuf],
    dirs_left: usize,
}

fn walk(base: &Path, dir: &Path, pat: &str, limit: usize, w: &mut Walk, out: &mut Vec<String>) {
    if w.dirs_left == 0 || w.skip.iter().any(|s| dir.starts_with(s)) {
        return;
    }
    w.dirs_left -= 1;
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by_key(|e| e.file_name());
    for e in items {
        if out.len() >= limit {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        let rel = e
            .path()
            .strip_prefix(base)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Links are never followed, so the walk cannot leave the folder; the same ignore list Pi
        // gives its own glob.
        if ft.is_dir() && !ft.is_symlink() {
            if name != ".git" && name != "node_modules" {
                walk(base, &e.path(), pat, limit, w, out);
            }
        } else if (pat.contains('/') && glob_match(pat.as_bytes(), rel.as_bytes()))
            || (!pat.contains('/') && glob_match(pat.as_bytes(), name.as_bytes()))
        {
            out.push(rel);
        }
    }
}

fn fs_glob(s: &Sandbox, a: &Value) -> Result<Value> {
    let dir = s.resolve(text(a, "path")?)?;
    let pat = text(a, "pattern")?;
    let limit = a
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(1000, |l| usize::try_from(l).unwrap_or(1000))
        .clamp(1, 10_000);
    // Host mode puts Clio's own folders here (`skip`): never walked into.
    let skip: Vec<PathBuf> = a
        .get("skip")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .collect();
    let mut w = Walk {
        skip: &skip,
        // Host mode is the only one whose root is `/`.
        dirs_left: if s.root().parent().is_none() {
            MAX_GLOB_DIRS
        } else {
            usize::MAX
        },
    };
    let mut out = Vec::new();
    walk(&dir, &dir, pat, limit, &mut w, &mut out);
    Ok(json!({ "paths": out }))
}

/// Protected folders that do not exist yet are created empty for the length of the command, so
/// the jail mounts them read-only and the command cannot create them (S-16); the ones this call
/// created are removed again afterwards.
struct MountPoints(Vec<std::path::PathBuf>);

impl MountPoints {
    fn make(root: &Path) -> Result<Self> {
        let mut made = Vec::new();
        for name in PROTECTED_PATHS {
            let p = root.join(name);
            if fs::symlink_metadata(&p).is_err() {
                if let Err(e) = fs::create_dir(&p) {
                    let _ = Self(made); // drop cleans what was made
                    return Err(Error::Io(e));
                }
                made.push(p);
            }
        }
        Ok(Self(made))
    }
}

impl Drop for MountPoints {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = fs::remove_dir(p); // only if still empty: it was read-only inside the jail
        }
    }
}

fn exec(s: &Sandbox, a: &Value) -> Result<Value> {
    let command = text(a, "command")?;
    let secs = a
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(MAX_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    let jail = Jail::detect().map_err(Error::Jail)?;
    let mut limits = jail.limits.clone();
    limits.timeout = Duration::from_secs(secs);
    let _mounts = MountPoints::make(s.root())?;
    let out = jail
        .run(&Job {
            work: s.root().to_path_buf(),
            read_only: PROTECTED_PATHS.map(String::from).to_vec(),
            argv: vec![BASH.into(), "-c".into(), command.into()],
            env: vec![],
            limits: Some(limits),
            ..Job::default()
        })
        .map_err(Error::Jail)?;
    Ok(exec_result(&out, Some(secs)))
}

/// What a command returns to the model, in either mode.
/// `secs`: its time limit, `None` when it had none.
pub fn exec_result(out: &JailOutput, secs: Option<u64>) -> Value {
    let mut output = out.stdout.clone();
    if !out.stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&out.stderr);
    }
    if out.flooded {
        output.push_str("\n[killed: it wrote far too much output]");
    }
    if out.truncated {
        output.push_str("\n[output truncated: only the tail is shown]");
    }
    if out.stopped {
        output.push_str("\n[stopped: the person stopped it or the session ended]");
    }
    json!({
        "output": output,
        "exit_code": out.exit_code,
        "signal": out.signal,
        "timed_out": out.timed_out,
        "stopped": out.stopped,
        "timeout_secs": secs,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{Walk, glob_match, walk};

    #[test]
    fn a_glob_stops_after_its_folder_budget() {
        let t = tempfile::tempdir().unwrap();
        for d in ["a", "b", "c"] {
            std::fs::create_dir(t.path().join(d)).unwrap();
            std::fs::write(t.path().join(d).join("x.txt"), "").unwrap();
        }
        let mut w = Walk {
            skip: &[],
            dirs_left: 3,
        };
        let mut out = Vec::new();
        walk(t.path(), t.path(), "**/*.txt", 100, &mut w, &mut out);
        // The root and two folders: the third is never read.
        assert_eq!(out, ["a/x.txt", "b/x.txt"]);
    }

    #[test]
    fn globs() {
        let m = |p: &str, t: &str| glob_match(p.as_bytes(), t.as_bytes());
        assert!(m("*.py", "a.py") && !m("*.py", "d/a.py"));
        assert!(m("**/*.py", "d/e/a.py") && m("**/*.py", "a.py"));
        assert!(m("src/**", "src/a/b.rs") && !m("src/*", "src/a/b.rs"));
        assert!(m("a?c", "abc") && !m("a?c", "a/c"));
        assert!(!m("*.py", "a.rs") && m("*", "x") && !m("", "x"));
    }
}
