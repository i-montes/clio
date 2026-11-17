#![allow(clippy::unwrap_used)]
//! The Anthropic adapter of the Claude connector (`adapters/models/anthropic_oauth.py`): it asks
//! as Claude Code does (the subscription token only works that way), answers in Clio's shape, and
//! keeps the contract of the OpenAI one: the token goes only to its server or this machine, a
//! redirect is an error, an HTTP error never prints the body.

mod pi_support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use pi_support::FakeProvider;

const TOKEN: &str = "sk-ant-oat01-test-token-0123456789";

/// Runs the adapter with an empty environment plus `env`, feeding it `stdin`.
fn run_adapter(env: &[(&str, &str)], stdin: &str) -> std::process::Output {
    let adapter =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/models/anthropic_oauth.py");
    let mut child = Command::new("python3")
        .arg("-I")
        .arg(adapter)
        .env_clear()
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

#[test]
fn asks_as_claude_code_and_answers_in_clios_shape() {
    let t = tempfile::tempdir().unwrap();
    let fake = FakeProvider::start_with(
        t.path(),
        r#"[{"text":"hola","usage":[17,3]}]"#,
        Some(TOKEN),
        &["--log-body"],
    );
    let base = fake.base_url.trim_end_matches("/v1").to_owned();
    let o = run_adapter(
        &[("LLM_BASE_URL", &base), ("LLM_TIMEOUT_SECONDS", "10")],
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

    let seen = fake.requests();
    assert_eq!(seen.len(), 1);
    let r = &seen[0];
    assert_eq!(r["path"], "/v1/messages");
    let h = &r["headers"];
    assert_eq!(h["authorization_matches"], true, "Bearer <token>");
    assert_eq!(h["anthropic-version"], "2023-06-01");
    assert_eq!(h["anthropic-beta"], "claude-code-20250219,oauth-2025-04-20");
    assert_eq!(h["user-agent"], "claude-cli/2.1.280");
    assert_eq!(h["x-app"], "cli");
    let body = &r["body"];
    assert_eq!(body["max_tokens"], 123);
    assert!(
        body["system"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("You are Claude Code")
    );
    assert_eq!(body["system"][1]["text"], "Eres Clio.");
    let roles: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user"], "no system role");
    // The token is in no log line.
    assert!(!std::fs::read_to_string(&fake.log).unwrap().contains(TOKEN));
}

#[test]
fn max_tokens_defaults_and_a_bare_request_uses_the_variable() {
    let t = tempfile::tempdir().unwrap();
    let fake =
        FakeProvider::start_with(t.path(), r#"[{"text":"ok"}]"#, Some(TOKEN), &["--log-body"]);
    let base = fake.base_url.trim_end_matches("/v1").to_owned();
    let o = run_adapter(
        &[("LLM_BASE_URL", &base), ("CLAUDE_CODE_OAUTH_TOKEN", TOKEN)],
        r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let seen = fake.requests();
    assert_eq!(seen[0]["body"]["max_tokens"], 4096);
    assert_eq!(seen[0]["headers"]["authorization_matches"], true);
    // No key at all: an error that names the variable, not a request.
    let o = run_adapter(&[("LLM_BASE_URL", &base)], r#"{"model":"m","messages":[]}"#);
    assert!(
        !o.status.success()
            && String::from_utf8_lossy(&o.stderr).contains("CLAUDE_CODE_OAUTH_TOKEN")
    );
    assert_eq!(fake.requests().len(), 1);
}

#[test]
fn an_error_never_prints_the_body() {
    let t = tempfile::tempdir().unwrap();
    let script = format!(r#"[{{"status":401,"body":"invalid token {TOKEN}"}}]"#);
    let fake = FakeProvider::start(t.path(), &script, None);
    let base = fake.base_url.trim_end_matches("/v1").to_owned();
    let o = run_adapter(
        &[("LLM_BASE_URL", &base), ("LLM_TIMEOUT_SECONDS", "10")],
        &envelope(r#"[{"role":"user","content":"x"}]"#),
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success() && err.contains("HTTP 401"), "{err}");
    assert!(
        !err.contains(TOKEN) && !err.contains("invalid token"),
        "{err}"
    );
}

#[test]
fn refuses_a_redirect_and_a_remote_base() {
    // A 302 to a second listener: the second one must not be called with the token.
    let first = TcpListener::bind("127.0.0.1:0").unwrap();
    let second = TcpListener::bind("127.0.0.1:0").unwrap();
    let (p1, p2) = (
        first.local_addr().unwrap().port(),
        second.local_addr().unwrap().port(),
    );
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut s, _)) = first.accept() else {
            return;
        };
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; len];
        let _ = r.read_exact(&mut body);
        let _ = s.write_all(
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{p2}/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        );
    });
    std::thread::spawn(move || {
        let _ = tx.send(second.accept().is_ok());
    });
    let base = format!("http://127.0.0.1:{p1}");
    let o = run_adapter(
        &[("LLM_BASE_URL", &base), ("LLM_TIMEOUT_SECONDS", "10")],
        &envelope(r#"[{"role":"user","content":"x"}]"#),
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success() && err.contains("HTTP 302"), "{err}");
    assert!(!err.contains(TOKEN));
    assert!(
        rx.recv_timeout(Duration::from_secs(1)).is_err(),
        "the redirect was followed"
    );
    let _ = std::net::TcpStream::connect(("127.0.0.1", p2)); // frees the waiting thread

    // A remote base is refused before anything is sent.
    for base in [
        "https://evil.test",
        "https://api.anthropic.com.evil.test",
        "https://api.anthropic.com@evil.test",
        "http://api.anthropic.com",
    ] {
        let o = run_adapter(
            &[("LLM_BASE_URL", base)],
            &envelope(r#"[{"role":"user","content":"x"}]"#),
        );
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            !o.status.success() && err.contains("LLM_BASE_URL"),
            "{base}: {err}"
        );
        assert!(!err.contains(TOKEN));
    }
}
