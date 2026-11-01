//! The primitive operations Pi's own tools are made of (read a file, write a file, stat, make a
//! folder, list, glob, run a command). Pi keeps its tools (truncation, diff, path resolution,
//! rendering); only these primitives are served here, inside the session's working copy.
//!
//! They are `clio-node` tools: the executor runs them only after the gateway allowed the exact
//! action, and `exec` runs ONLY in the `bwrap` jail (the one free shell, S-12).

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use clio_core::Effect;
use clio_node::jail::{Jail, Job};
use clio_node::{Error, Result, Sandbox, Tool};
use serde_json::{Value, json};

use crate::PROTECTED_PATHS;

/// The largest file `fs_read` returns, and the most `exec` output that is kept.
const MAX_FILE_BYTES: u64 = 8 << 20;
const BASH: &str = "/usr/bin/bash";
const MAX_TIMEOUT_SECS: u64 = 120;

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

pub fn tools(jail: bool) -> Vec<Tool> {
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
    if jail {
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
    let p = s.resolve(text(a, "path")?)?;
    let meta = fs::metadata(&p)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(Error::BadArgs(format!(
            "the file is {} bytes, more than the {MAX_FILE_BYTES} allowed",
            meta.len()
        )));
    }
    let mut buf = Vec::new();
    fs::File::open(&p)?
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut buf)?;
    Ok(json!({ "content": String::from_utf8_lossy(&buf) }))
}

fn fs_stat(s: &Sandbox, a: &Value) -> Result<Value> {
    let p = s.resolve(text(a, "path")?)?;
    let mode = a.get("mode").and_then(Value::as_str).unwrap_or("exists");
    let Ok(meta) = fs::metadata(&p) else {
        if mode == "exists" {
            return Ok(json!({"exists": false, "is_dir": false}));
        }
        return Err(Error::BadArgs("no such file or folder".into()));
    };
    match mode {
        "exists" => {}
        "r" => {
            if meta.is_dir() {
                fs::read_dir(&p)?;
            } else {
                fs::File::open(&p)?;
            }
        }
        "rw" => {
            fs::File::open(&p)?;
            fs::OpenOptions::new().write(true).open(&p)?;
        }
        other => return Err(Error::BadArgs(format!("unknown mode {other:?}"))),
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

fn walk(base: &Path, dir: &Path, pat: &str, limit: usize, out: &mut Vec<String>) {
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
                walk(base, &e.path(), pat, limit, out);
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
    let mut out = Vec::new();
    walk(&dir, &dir, pat, limit, &mut out);
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
    Ok(json!({
        "output": output,
        "exit_code": out.exit_code,
        "signal": out.signal,
        "timed_out": out.timed_out,
        "timeout_secs": secs,
    }))
}

#[cfg(test)]
mod tests {
    use super::glob_match;

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
