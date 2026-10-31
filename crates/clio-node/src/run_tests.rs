//! The `run_tests` tool: not a shell, a fixed menu of test runners. The tool builds the argv
//! itself from `{"runner": ..., "target": ...}` and runs it in the jail (`crate::jail`).

use crate::jail::{Jail, Job, Limits, find_in_trusted};
use crate::{Error, Registry, Result, Sandbox, Tool};
use clio_core::Effect;
use serde_json::{Value, json};
use std::time::Duration;

/// Folders the jailed code must not change even though they are inside `/work` (S-16 mirror:
/// they load code or settings by themselves). They are mounted read-only.
pub const READ_ONLY: [&str; 5] = ["extensions", "skills", ".pi", ".claude", ".git"];

/// `cargo_test` is NOT offered: a `cargo` under /usr is usually a rustup shim whose toolchains
/// live in the person's home, and making it work would mean mounting part of that home (and the
/// crates registry) into the jail.
const RUNNERS: [&str; 2] = ["unittest", "pytest"];

/// `[A-Za-z0-9_./-]+`, not starting with `-`, no `..`.
pub fn valid_target(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 200
        && !t.starts_with('-')
        && !t.starts_with('/')
        && !t.contains("..")
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-'))
}

/// Resolves `target` inside the sandbox (symlinks that leave it are rejected), requires that it
/// exists, and returns the canonical path relative to the root, checked again.
fn resolve_target(s: &Sandbox, target: &str) -> Result<String> {
    if !valid_target(target) {
        return Err(Error::BadArgs(format!(
            "target {target:?} must match [A-Za-z0-9_./-]+, not start with '-' and not contain '..'"
        )));
    }
    let p = s.resolve(target)?;
    let canon = p
        .canonicalize()
        .map_err(|_| Error::BadArgs(format!("target {target:?} does not exist in the folder")))?;
    let rel = canon
        .strip_prefix(s.root())
        .map_err(|_| Error::Escape(target.into()))?
        .to_string_lossy()
        .into_owned();
    let rel = if rel.is_empty() { ".".to_owned() } else { rel };
    if rel != "." && !valid_target(&rel) {
        return Err(Error::BadArgs(format!(
            "target resolves to {rel:?}, which has characters outside [A-Za-z0-9_./-]"
        )));
    }
    Ok(rel)
}

fn python() -> Result<String> {
    find_in_trusted("python3")
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| Error::Jail("python3 is not installed in /usr/bin".into()))
}

fn run(s: &Sandbox, a: &Value) -> Result<Value> {
    let obj = a
        .as_object()
        .ok_or_else(|| Error::BadArgs("arguments must be an object".into()))?;
    if let Some(k) = obj.keys().find(|k| *k != "runner" && *k != "target") {
        return Err(Error::BadArgs(format!("unknown argument {k:?}")));
    }
    let runner = obj
        .get("runner")
        .and_then(Value::as_str)
        .filter(|r| RUNNERS.contains(r))
        .ok_or_else(|| Error::BadArgs("runner must be unittest or pytest".into()))?;
    let target = match obj.get("target") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) => Some(resolve_target(s, t)?),
        Some(_) => return Err(Error::BadArgs("target must be a string".into())),
    };
    let jail = Jail::detect().map_err(Error::Jail)?;
    let env = vec![("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned())];
    let limits = jail.limits.clone();
    let argv: Vec<String> = match runner {
        "unittest" => {
            let mut v = vec![python()?, "-m".into(), "unittest".into()];
            match target {
                Some(t) if s.root().join(&t).is_dir() => {
                    v.extend(["discover".into(), "-q".into(), "-s".into(), t]);
                }
                Some(t) => v.extend(["-q".into(), t]),
                None => v.push("-q".into()),
            }
            v
        }
        "pytest" => {
            // Is pytest there? Ask the jail itself, so the answer is about what tests will use.
            let probe = Job {
                work: s.root().to_path_buf(),
                argv: vec![
                    python()?,
                    "-c".into(),
                    "import importlib.util,sys;sys.exit(0 if importlib.util.find_spec('pytest') else 3)"
                        .into(),
                ],
                env: env.clone(),
                read_only: READ_ONLY.map(String::from).to_vec(),
                limits: Some(Limits {
                    timeout: Duration::from_secs(20),
                    ..limits.clone()
                }),
                ..Job::default()
            };
            if !jail.run(&probe).map_err(Error::Jail)?.success() {
                return Err(Error::Jail(
                    "pytest is not installed on this machine".into(),
                ));
            }
            let mut v = vec![python()?, "-m".into(), "pytest".into(), "-q".into()];
            v.extend(["-p".into(), "no:cacheprovider".into()]);
            v.extend(target);
            v
        }
        _ => unreachable!("runner was checked against RUNNERS"),
    };
    let out = jail
        .run(&Job {
            work: s.root().to_path_buf(),
            read_only: READ_ONLY.map(String::from).to_vec(),
            argv,
            env,
            ..Job::default()
        })
        .map_err(Error::Jail)?;
    let mut text = out.stdout.clone();
    if !out.stderr.is_empty() {
        if !text.is_empty() {
            text.push_str("\n--- stderr ---\n");
        }
        text.push_str(&out.stderr);
    }
    if out.timed_out {
        text.push_str("\n[killed: time limit reached]");
    }
    if out.flooded {
        text.push_str("\n[killed: it wrote far too much output]");
    }
    if out.truncated {
        text.push_str("\n[output truncated: only the tail is shown]");
    }
    Ok(json!({
        "runner": runner,
        "exit_code": out.exit_code,
        "signal": out.signal,
        "passed": out.success(),
        "timed_out": out.timed_out,
        "truncated": out.truncated,
        "elapsed_ms": out.elapsed_ms,
        "output": text,
    }))
}

/// The tool itself. `LocalReversible`: it can change files in the folder (the snapshot undoes it).
pub fn run_tests_tool() -> Tool {
    Tool {
        name: "run_tests".into(),
        description: "Run the project's tests in a jail with no network. Args: \"runner\" (unittest or pytest) and optional \"target\" (a file or folder inside the working folder). Returns the exit code and the end of the output".into(),
        keywords: ["test", "tests", "unittest", "pytest", "cargo", "check", "verify"]
            .map(String::from)
            .to_vec(),
        effect: Some(Effect::LocalReversible),
        visible: true,
        run,
    }
}

/// Registers `run_tests` only where the jail can exist: Linux, with `bwrap` and `prlimit`. The
/// caller must also have PROVEN the jail with `Jail::probe` and `Jail::self_test`.
pub fn register_run_tests(registry: &mut Registry) -> Result<()> {
    Jail::detect().map_err(Error::Jail)?;
    registry.register(run_tests_tool())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_follow_the_pattern() {
        for ok in ["a.py", "tests/test_x.py", "a-b_c/d.e", "."] {
            assert!(valid_target(ok), "{ok}");
        }
        for bad in [
            "", "--foo", "-x", "../x", "a/../b", "a;b", "a b", "a|b", "$(id)", "a`id`", "a\nb",
            "é.py", "a*", "~/x", "/etc",
        ] {
            assert!(!valid_target(bad), "{bad}");
        }
    }
}
