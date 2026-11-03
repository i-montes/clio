#![allow(clippy::unwrap_used)]
//! Security audit of 2026-11-02, stage 1 (finding 4, attack-53): the model adapter holds the
//! provider key and sends it in a header. The key goes to the host it was configured for and
//! nowhere else. Fails until stage 2.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const KEY: &str = "sk-egress-test-key-0123456789abcdef";

/// Reads one HTTP request, returns its `Authorization` header (if any) and answers with `reply`.
fn serve_once(l: TcpListener, reply: String, seen: mpsc::Sender<Option<String>>) {
    std::thread::spawn(move || {
        let Ok((mut s, _)) = l.accept() else { return };
        let mut r = BufReader::new(s.try_clone().unwrap());
        let (mut auth, mut len) = (None, 0usize);
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("authorization:") {
                auth = Some(line.trim().to_owned());
            }
            if let Some(v) = lower.strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; len];
        let _ = r.read_exact(&mut body);
        let _ = seen.send(auth);
        let _ = s.write_all(reply.as_bytes());
    });
}

fn respond(status: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

// S-19, S-44: a provider (or whoever answers in its place) that redirects the call to another
// host must not receive the key. Python's `urllib` follows a 302 and keeps `Authorization`.
#[test]
fn the_provider_key_is_not_sent_to_a_host_a_redirect_points_to() {
    let first = TcpListener::bind("127.0.0.1:0").unwrap();
    let second = TcpListener::bind("127.0.0.1:0").unwrap();
    let (p1, p2) = (
        first.local_addr().unwrap().port(),
        second.local_addr().unwrap().port(),
    );
    let (tx1, rx1) = mpsc::channel();
    let (tx2, rx2) = mpsc::channel();
    serve_once(
        first,
        respond(
            "302 Found",
            &format!("Location: http://127.0.0.1:{p2}/elsewhere\r\n"),
            "{}",
        ),
        tx1,
    );
    serve_once(
        second,
        respond(
            "200 OK",
            "",
            r#"{"choices":[{"message":{"content":"hi"}}],"usage":{}}"#,
        ),
        tx2,
    );
    let adapter =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/models/openai_compat.py");
    let mut child = Command::new("python3")
        .arg("-I")
        .arg(adapter)
        .env_clear()
        .env("OPENCODE_API_KEY", KEY)
        .env("LLM_BASE_URL", format!("http://127.0.0.1:{p1}/v1"))
        .env("LLM_TIMEOUT_SECONDS", "10")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#)
        .unwrap();
    let _ = child.wait_with_output().unwrap();
    // The first host was asked, with the key: that is what the configuration says.
    let at_first = rx1.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        at_first.is_some_and(|a| a.contains(KEY)),
        "the configured host gets the key"
    );
    // The second one must have got nothing, or not been called at all.
    if let Ok(at_second) = rx2.recv_timeout(Duration::from_secs(2)) {
        assert!(
            at_second.is_none(),
            "the key was forwarded to another host by a redirect: {at_second:?}"
        );
    }
    let _ = TcpStream::connect(("127.0.0.1", p2)); // frees the listener thread if it is waiting
}

// S-19 S-44: the key goes over TLS, or to this machine. `http://` to another host would send it
// readable by everyone on the way; the adapter refuses before it asks anything. (The host here
// does not resolve, so a bug would be a network error, not a leak.)
#[test]
fn a_plain_http_url_to_another_host_is_refused_before_any_request() {
    let adapter =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/models/openai_compat.py");
    for base in [
        "http://llm.invalid/v1",
        "ftp://llm.invalid/v1",
        "file:///etc/passwd",
    ] {
        let mut child = Command::new("python3")
            .arg("-I")
            .arg(&adapter)
            .env_clear()
            .env("OPENCODE_API_KEY", KEY)
            .env("LLM_BASE_URL", base)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(br#"{"model":"m","messages":[]}"#)
            .unwrap();
        let o = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            !o.status.success() && err.contains("https"),
            "{base}: {err}"
        );
        assert!(!err.contains(KEY), "the key is never printed: {err}");
    }
}

// ---------------------------------------------------------------------------------------------
// The attack case of this file, through clio-attacks (the gate of the battery)
// ---------------------------------------------------------------------------------------------

use clio_attacks::{Case, Observed, Status, Target, load_cases, run as run_cases};

/// A scenario holds when it does not panic: the plain tests above are the scenarios.
fn held(f: impl FnOnce() + std::panic::UnwindSafe) -> bool {
    std::panic::catch_unwind(f).is_ok()
}

struct AdapterTarget;

impl Target for AdapterTarget {
    fn attack(&self, case: &Case) -> Observed {
        let refused = match case.id.as_str() {
            "attack-53" => held(|| {
                the_provider_key_is_not_sent_to_a_host_a_redirect_points_to();
                a_plain_http_url_to_another_host_is_refused_before_any_request();
            }),
            other => panic!("{other} is active but this target has no scenario for it"),
        };
        Observed { refused }
    }
}

// S-19 S-44: attack 53
#[test]
fn the_adapter_case_is_active_and_refused() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testing/attacks/cases");
    let mine: Vec<Case> = load_cases(&dir)
        .unwrap()
        .into_iter()
        .filter(|c| c.id == "attack-53")
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "attack-53 is missing in testing/attacks/cases"
    );
    assert!(mine.iter().all(|c| c.status == Status::Active));
    let report = run_cases(&mine, &AdapterTarget);
    assert!(report.ok(), "{}", report.line());
    eprintln!("{}", report.line());
}
