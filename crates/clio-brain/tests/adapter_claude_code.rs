#![allow(clippy::unwrap_used)]
//! The adapter of the Claude connector for single calls (`adapters/models/claude_code.py`): it
//! runs the official Claude Code (`claude -p`) with the subscription token only in its
//! environment, never calls the API itself, and answers in Clio's shape. Here a fake `claude`
//! records what it was given and answers a scripted JSON.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const TOKEN: &str = "sk-ant-oat01-test-token-0123456789";

/// A fake `claude` in `dir` that writes its argv, environment, stdin and system prompt file to
/// `dir/seen.json` and prints `reply` with exit code `code`.
fn fake_claude(dir: &Path, reply: &str, code: i32) -> PathBuf {
    let seen = dir.join("seen.json");
    let bin = dir.join("claude");
    std::fs::write(
        &bin,
        format!(
            "#!/usr/bin/env python3\nimport json, os, sys\na = sys.argv[1:]\nf = a[a.index('--system-prompt-file') + 1] if '--system-prompt-file' in a else None\njson.dump({{'argv': a, 'env': dict(os.environ), 'cwd': os.getcwd(), 'stdin': sys.stdin.read(), 'system_file': f, 'system': open(f).read() if f else None, 'system_mode': os.stat(f).st_mode & 0o777 if f else None}}, open({seen:?}, 'w'))\nsys.stdout.write({reply:?})\nsys.exit({code})\n",
            seen = seen.display().to_string(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    bin
}

fn seen(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("seen.json")).unwrap()).unwrap()
}

/// Runs the adapter with `args`, an environment of only `env` (plus PATH), and `stdin`.
fn run_adapter(args: &[&str], env: &[(&str, &str)], stdin: &str) -> std::process::Output {
    let adapter =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/models/claude_code.py");
    let mut child = Command::new("python3")
        .arg("-I")
        .arg(adapter)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn envelope(messages: &str) -> String {
    format!(
        r#"{{"key":"{TOKEN}","request":{{"model":"claude-sonnet-5-5","max_tokens":123,"messages":{messages}}}}}"#
    )
}

const OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"hola","usage":{"input_tokens":17,"output_tokens":3}}"#;

#[test]
fn runs_claude_code_with_the_token_only_in_its_env() {
    let t = tempfile::tempdir().unwrap();
    let bin = fake_claude(t.path(), OK, 0);
    let person = t.path().join("persona");
    std::fs::create_dir(&person).unwrap();
    let o = run_adapter(
        &["--claude", bin.to_str().unwrap()],
        &[
            ("HOME", person.to_str().unwrap()),
            ("ANTHROPIC_API_KEY", "sk-ant-api03-other"),
            ("ANTHROPIC_BASE_URL", "http://evil.test"),
            ("CLAUDE_CONFIG_DIR", "/elsewhere"),
            ("OTHER_SECRET", "x"),
        ],
        &envelope(
            r#"[{"role":"system","content":"Eres Clio."},{"role":"user","content":"hola?"},{"role":"assistant","content":"si"},{"role":"user","content":"y?"}]"#,
        ),
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        out,
        serde_json::json!({"content":"hola","input_tokens":17,"output_tokens":3})
    );

    let s = seen(t.path());
    let argv: Vec<&str> = s["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(!argv.join(" ").contains(TOKEN), "{argv:?}");
    for (flag, value) in [
        ("--output-format", "json"),
        ("--model", "claude-sonnet-5-5"),
        ("--tools", ""),
        ("--setting-sources", ""),
    ] {
        let i = argv
            .iter()
            .position(|a| *a == flag)
            .unwrap_or_else(|| panic!("{flag}: {argv:?}"));
        assert_eq!(argv[i + 1], value, "{flag}");
    }
    for flag in ["-p", "--no-session-persistence", "--strict-mcp-config"] {
        assert!(argv.contains(&flag), "{flag}: {argv:?}");
    }
    // The conversation goes on stdin, without the system message.
    let stdin = s["stdin"].as_str().unwrap();
    assert!(
        stdin.contains("hola?") && stdin.contains("si") && stdin.contains("y?"),
        "{stdin}"
    );
    assert!(!stdin.contains("Eres Clio"), "{stdin}");
    // The system prompt goes in a file inside Claude Code's private HOME, readable by its owner only.
    assert_eq!(s["system"], "Eres Clio.");
    assert_eq!(s["system_mode"], 0o600);
    assert!(
        Path::new(s["system_file"].as_str().unwrap()).starts_with(s["cwd"].as_str().unwrap()),
        "{s}"
    );

    let env = s["env"].as_object().unwrap();
    assert_eq!(env["CLAUDE_CODE_OAUTH_TOKEN"], TOKEN);
    // LC_CTYPE is the fake's own Python setting its locale (PEP 538), not something it was given.
    let mut names: Vec<&str> = env
        .keys()
        .map(String::as_str)
        .filter(|k| *k != "LC_CTYPE")
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "DISABLE_AUTOUPDATER",
            "DISABLE_TELEMETRY",
            "HOME",
            "PATH",
        ]
    );
    for v in [
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        "DISABLE_AUTOUPDATER",
        "DISABLE_TELEMETRY",
    ] {
        assert_eq!(env[v], "1", "{v}");
    }
    // A private HOME of its own, gone when the call is over.
    let home = env["HOME"].as_str().unwrap();
    assert_ne!(Path::new(home), person);
    assert!(!Path::new(home).exists(), "{home} was left behind");
    assert_eq!(s["cwd"].as_str().unwrap(), home);
}

#[test]
fn a_bare_request_takes_the_token_from_the_variable() {
    let t = tempfile::tempdir().unwrap();
    let bin = fake_claude(t.path(), OK, 0);
    let o = run_adapter(
        &["--claude", bin.to_str().unwrap()],
        &[("CLAUDE_CODE_OAUTH_TOKEN", TOKEN)],
        r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(seen(t.path())["env"]["CLAUDE_CODE_OAUTH_TOKEN"], TOKEN);
    // No token at all: an error that names the variable, and Claude Code is not run.
    std::fs::remove_file(t.path().join("seen.json")).unwrap();
    let o = run_adapter(
        &["--claude", bin.to_str().unwrap()],
        &[],
        r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#,
    );
    assert!(
        !o.status.success()
            && String::from_utf8_lossy(&o.stderr).contains("CLAUDE_CODE_OAUTH_TOKEN")
    );
    assert!(!t.path().join("seen.json").exists());
}

#[test]
fn an_error_answer_is_a_clear_error() {
    let t = tempfile::tempdir().unwrap();
    let reply = format!(
        r#"{{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key {TOKEN} · Please run /login","usage":{{"input_tokens":0,"output_tokens":0}}}}"#
    );
    let bin = fake_claude(t.path(), &reply, 1);
    let o = run_adapter(
        &["--claude", bin.to_str().unwrap()],
        &[],
        &envelope(r#"[{"role":"user","content":"x"}]"#),
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    assert!(err.contains("Claude Code respondió con error"), "{err}");
    assert!(err.contains("Invalid API key"), "{err}");
    assert!(!err.contains(TOKEN) && !err.contains("Traceback"), "{err}");
    assert!(o.stdout.is_empty());
}

#[test]
fn broken_json_is_a_clear_error() {
    for (reply, code) in [("no soy json", 0), ("", 1), (r#"["no","objeto"]"#, 0)] {
        let t = tempfile::tempdir().unwrap();
        let bin = fake_claude(t.path(), reply, code);
        let o = run_adapter(
            &["--claude", bin.to_str().unwrap()],
            &[],
            &envelope(r#"[{"role":"user","content":"x"}]"#),
        );
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{reply}: {err}");
        assert!(err.contains("Claude Code"), "{reply}: {err}");
        assert!(
            !err.contains(TOKEN) && !err.contains("Traceback"),
            "{reply}: {err}"
        );
    }
}

#[test]
fn without_the_binary_says_to_install() {
    let t = tempfile::tempdir().unwrap();
    let not_exec = t.path().join("claude");
    std::fs::write(&not_exec, "#!/bin/sh\n").unwrap();
    let missing = t.path().join("nada/claude");
    for args in [
        vec![],
        vec!["--claude", missing.to_str().unwrap()],
        vec!["--claude", not_exec.to_str().unwrap()],
    ] {
        let o = run_adapter(&args, &[], &envelope(r#"[{"role":"user","content":"x"}]"#));
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{args:?}");
        assert!(
            err.contains("falta Claude Code (pi-claude-agent-sdk): clio pi install"),
            "{args:?}: {err}"
        );
    }
}

#[test]
fn a_system_prompt_too_big_for_one_argument_still_goes() {
    let t = tempfile::tempdir().unwrap();
    let bin = fake_claude(t.path(), OK, 0);
    let big = "x".repeat(200 * 1024);
    let o = run_adapter(
        &["--claude", bin.to_str().unwrap()],
        &[],
        &envelope(&format!(
            r#"[{{"role":"system","content":"{big}"}},{{"role":"user","content":"x"}}]"#
        )),
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(seen(t.path())["system"].as_str().unwrap().len(), big.len());
}

#[test]
fn a_proxy_is_passed_on_and_nothing_else_of_anthropic_or_claude() {
    let t = tempfile::tempdir().unwrap();
    let bin = fake_claude(t.path(), OK, 0);
    let o = run_adapter(
        &["--claude", bin.to_str().unwrap()],
        &[
            ("HTTPS_PROXY", "http://proxy.test:3128"),
            ("no_proxy", "localhost"),
            ("ANTHROPIC_API_KEY", "sk-ant-api03-other"),
            ("CLAUDE_CONFIG_DIR", "/elsewhere"),
        ],
        &envelope(r#"[{"role":"user","content":"x"}]"#),
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let env = seen(t.path())["env"].clone();
    assert_eq!(env["HTTPS_PROXY"], "http://proxy.test:3128");
    assert_eq!(env["no_proxy"], "localhost");
    assert!(env.get("ANTHROPIC_API_KEY").is_none() && env.get("CLAUDE_CONFIG_DIR").is_none());
}

#[test]
fn non_utf8_output_is_a_clear_error() {
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().join("claude");
    std::fs::write(&bin, "#!/bin/sh\nprintf '\\377\\376 no'\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let o = run_adapter(
        &["--claude", bin.to_str().unwrap()],
        &[],
        &envelope(r#"[{"role":"user","content":"x"}]"#),
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success() && err.contains("JSON válido"), "{err}");
    assert!(!err.contains("Traceback"), "{err}");
}

/// Whether process `pid` is still there (and not a zombie).
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
        s.rsplit(')')
            .next()
            .is_some_and(|r| !r.trim_start().starts_with('Z'))
    })
}

// When Clio stops the adapter (SIGTERM, what `CommandModel` sends at its deadline), Claude Code is
// stopped too and its private HOME, which held the token's session, is removed.
#[test]
fn a_stopped_adapter_stops_claude_code_and_removes_its_home() {
    let t = tempfile::tempdir().unwrap();
    let (pid_file, home_file) = (t.path().join("pid"), t.path().join("home"));
    let bin = t.path().join("claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho $$ > '{}.tmp'; echo \"$HOME\" > '{}'; mv '{}.tmp' '{}'\nexec sleep 1000\n",
            pid_file.display(),
            home_file.display(),
            pid_file.display(),
            pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let adapter =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/models/claude_code.py");
    let mut child = Command::new("python3")
        .arg("-I")
        .arg(adapter)
        .args(["--claude", bin.to_str().unwrap()])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(envelope(r#"[{"role":"user","content":"x"}]"#).as_bytes())
        .unwrap();
    let t0 = std::time::Instant::now();
    while !pid_file.exists() {
        assert!(t0.elapsed().as_secs() < 20, "Claude Code never started");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let home = std::fs::read_to_string(&home_file)
        .unwrap()
        .trim()
        .to_owned();
    assert!(alive(pid) && Path::new(&home).is_dir());
    std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(!o.status.success());
    assert!(!String::from_utf8_lossy(&o.stderr).contains("Traceback"));
    let t0 = std::time::Instant::now();
    while alive(pid) {
        assert!(
            t0.elapsed().as_secs() < 5,
            "Claude Code ({pid}) was left running"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!Path::new(&home).exists(), "{home} was left behind");
}
