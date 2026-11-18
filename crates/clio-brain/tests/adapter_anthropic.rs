#![allow(clippy::unwrap_used)]
//! The adapter of the Anthropic API connector (`adapters/models/anthropic_api.py`): it asks with a
//! plain API key (`x-api-key`) and nothing that passes for another program, answers in Clio's
//! shape, and keeps the contract of the OpenAI one: the key goes only to its server or this
//! machine, a redirect is an error, an HTTP error never prints the body.

mod pi_support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use pi_support::FakeProvider;

const TOKEN: &str = "sk-ant-api03-test-key-0123456789";

/// Runs the adapter with an empty environment plus `env`, feeding it `stdin`.
fn run_adapter(env: &[(&str, &str)], stdin: &str) -> std::process::Output {
    let adapter =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/models/anthropic_api.py");
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
fn asks_with_the_api_key_and_answers_in_clios_shape() {
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
    assert_eq!(h["x_api_key_matches"], true, "x-api-key: <key>");
    assert_eq!(h["authorization_present"], false);
    assert_eq!(h["anthropic-version"], "2023-06-01");
    for gone in ["anthropic-beta", "x-app"] {
        assert!(h.get(gone).is_none(), "{gone}: {h}");
    }
    assert!(
        !h["user-agent"]
            .as_str()
            .unwrap_or("")
            .contains("claude-cli"),
        "{h}"
    );
    let body = &r["body"];
    assert_eq!(body["max_tokens"], 123);
    // Clio's system, and no identity block before it.
    assert_eq!(
        body["system"],
        serde_json::json!([{"type":"text","text":"Eres Clio."}])
    );
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
        &[("LLM_BASE_URL", &base), ("ANTHROPIC_API_KEY", TOKEN)],
        r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let seen = fake.requests();
    assert_eq!(seen[0]["body"]["max_tokens"], 4096);
    assert_eq!(seen[0]["headers"]["x_api_key_matches"], true);
    // No key at all: an error that names the variable, not a request.
    let o = run_adapter(&[("LLM_BASE_URL", &base)], r#"{"model":"m","messages":[]}"#);
    assert!(
        !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("ANTHROPIC_API_KEY")
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

/// Runs one call against a fake that answers `script`, returns the output and what it saw.
fn call(script: &str, stdin: &str) -> (std::process::Output, Vec<serde_json::Value>) {
    let t = tempfile::tempdir().unwrap();
    let fake = FakeProvider::start_with(t.path(), script, Some(TOKEN), &["--log-body"]);
    let base = fake.base_url.trim_end_matches("/v1").to_owned();
    let o = run_adapter(
        &[("LLM_BASE_URL", &base), ("LLM_TIMEOUT_SECONDS", "10")],
        stdin,
    );
    (o, fake.requests())
}

#[test]
fn messages_go_as_plain_text_and_only_what_the_api_takes() {
    let (o, seen) = call(
        r#"[{"text":"ok"}]"#,
        &envelope(
            r#"[{"role":"system","content":"uno"},
                {"role":"system","content":[{"type":"text","text":"dos"}]},
                {"role":"user","content":[{"type":"text","text":"a"},{"type":"image_url","image_url":{"url":"x"}},{"type":"text","text":"b"}],"name":"ana"},
                {"role":"assistant","content":"","tool_calls":[{"id":"1"}]},
                {"role":"tool","content":"resultado"},
                {"role":"assistant","content":"r","name":"clio"}]"#,
        ),
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let body = &seen[0]["body"];
    assert_eq!(body["system"][0]["text"], "uno\n\ndos");
    assert_eq!(
        body["messages"],
        serde_json::json!([
            {"role":"user","content":"ab"},
            {"role":"assistant","content":"r"}
        ]),
        "extra fields, empty and other-role messages are left out"
    );
}

#[test]
fn a_request_with_nothing_to_send_fails_before_calling() {
    let (o, seen) = call(
        r#"[{"text":"ok"}]"#,
        &envelope(r#"[{"role":"system","content":"solo"},{"role":"user","content":""}]"#),
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("no user or assistant message"));
    assert!(seen.is_empty());
}

#[test]
fn a_null_usage_counts_zero_and_a_non_object_answer_is_an_error() {
    // Raw listeners: the fake server always sends a well-formed usage.
    for (reply, ok) in [
        (
            r#"{"content":[{"type":"text","text":"hi"}],"usage":null}"#,
            true,
        ),
        (r#"{"content":null,"usage":{"input_tokens":null}}"#, true),
        (r#"["no","object"]"#, false),
    ] {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let sent = reply.to_owned();
        std::thread::spawn(move || {
            let Ok((mut s, _)) = l.accept() else { return };
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
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sent}",
                    sent.len()
                )
                .as_bytes(),
            );
        });
        let base = format!("http://127.0.0.1:{port}");
        let o = run_adapter(
            &[("LLM_BASE_URL", &base), ("LLM_TIMEOUT_SECONDS", "10")],
            &envelope(r#"[{"role":"user","content":"x"}]"#),
        );
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.success(), ok, "{reply}: {err}");
        assert!(!err.contains("Traceback"), "{err}");
        if ok {
            let out: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
            assert_eq!(out["input_tokens"], 0);
        }
    }
}

#[test]
fn a_key_with_a_trailing_newline_is_stripped() {
    let t = tempfile::tempdir().unwrap();
    let fake = FakeProvider::start(t.path(), r#"[{"text":"ok"}]"#, Some(TOKEN));
    let base = fake.base_url.trim_end_matches("/v1").to_owned();
    let stdin =
        envelope(r#"[{"role":"user","content":"x"}]"#).replace(TOKEN, &format!("{TOKEN}\\n"));
    let o = run_adapter(&[("LLM_BASE_URL", &base)], &stdin);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(fake.requests()[0]["headers"]["x_api_key_matches"], true);
}

#[test]
fn a_key_with_a_control_or_non_ascii_character_fails_without_printing_it() {
    for bad in [
        "sk-ant-SECRET\\nmore",
        "sk-ant-SECRET\\u00e9",
        "sk-ant SECRET",
        "sk-ant-SECRET\\u0007",
    ] {
        let stdin = envelope(r#"[{"role":"user","content":"x"}]"#).replace(TOKEN, bad);
        let o = run_adapter(&[("LLM_BASE_URL", "http://127.0.0.1:9")], &stdin);
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            !o.status.success() && err.contains("HTTP header"),
            "{bad}: {err}"
        );
        assert!(!err.contains("SECRET"), "{bad}: {err}");
    }
}

#[test]
fn the_fake_server_streams_anthropic_events() {
    let t = tempfile::tempdir().unwrap();
    let fake = FakeProvider::start(t.path(), r#"[{"text":"hola","usage":[17,3]}]"#, None);
    let port = fake
        .base_url
        .rsplit(':')
        .next()
        .unwrap()
        .trim_end_matches("/v1");
    let mut s = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    let body = r#"{"model":"m","stream":true,"messages":[]}"#;
    write!(
        s,
        "POST /v1/messages HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut got = String::new();
    s.read_to_string(&mut got).unwrap();
    let names: Vec<&str> = got
        .lines()
        .filter_map(|l| l.strip_prefix("event: "))
        .collect();
    assert_eq!(
        names,
        [
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop"
        ]
    );
    assert!(got.contains(r#""text": "hola""#) && got.contains(r#""input_tokens": 17"#));
}
