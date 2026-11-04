#![allow(clippy::unwrap_used)]
//! The Jev adapter holds the key for the length of a call and sends it in a header (S-18, S-44).
//! It goes to the host it was configured for and nowhere else, whatever the answer says.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use clio_core::SystemClock;
use clio_jev::{Clean, Config, Jev, JevError, Process, Questions};
use serde_json::{Value, json};
use std::sync::Arc;

const KEY: &str = "test-key-0123456789";

/// Answers one request with `reply` and reports its `Authorization` header and body.
fn serve_once(l: TcpListener, reply: String, seen: mpsc::Sender<(Option<String>, String)>) {
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
        let _ = seen.send((auth, String::from_utf8_lossy(&body).into_owned()));
        let _ = s.write_all(reply.as_bytes());
    });
}

fn respond(status: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn jev(base: &str, timeout: Duration) -> Jev {
    let process = Process {
        python: "python3".into(),
        script: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../adapters/jev/jev_systemone.py"
        )
        .into(),
        base_url: Some(base.to_owned()),
    };
    Jev::new(
        Config {
            timeout,
            ..Config::default()
        },
        Box::new(process),
        Box::new(|| Ok(KEY.to_owned())),
        Arc::new(SystemClock),
    )
}

fn questions() -> Questions {
    Questions::new().noul("x", "¿Sí?").unwrap()
}

fn ask(j: &Jev) -> Result<clio_jev::Answers, JevError> {
    j.ask(
        &Clean::new(json!({"mensaje_de_la_persona": "hola"}), &|s| s.to_owned()),
        &questions(),
    )
}

fn good() -> String {
    let body = json!({"model":"jev-1.14.0","answers":{"x":{"type":"noul","noul":0.25}},"usage":{"input_tokens":12,"output_tokens":1}});
    respond("200 OK", "", &body.to_string())
}

// S-62
#[test]
fn a_call_reaches_the_api_with_the_key_in_one_header_and_the_questions_in_the_body() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let (tx, rx) = mpsc::channel();
    serve_once(l, good(), tx);
    let a = ask(&jev(&base, Duration::from_secs(5))).unwrap();
    assert_eq!(a.noul("x"), Some(0.25));
    let (auth, body) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        auth.as_deref(),
        Some(format!("Authorization: Bearer {KEY}").as_str())
    );
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"]["mensaje_de_la_persona"], "hola");
    assert_eq!(body["questions"]["x"]["type"], "noul");
    assert!(
        !body.to_string().contains(KEY),
        "the key is not in the body"
    );
}

// S-44: whoever answers in the API's place and redirects the call must not receive the key.
// S-62
#[test]
fn the_key_is_not_sent_to_a_host_a_redirect_points_to() {
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
            &format!("Location: http://127.0.0.1:{p2}/v1/systemone\r\n"),
            "{}",
        ),
        tx1,
    );
    serve_once(second, good(), tx2);
    let r = ask(&jev(
        &format!("http://127.0.0.1:{p1}"),
        Duration::from_secs(5),
    ));
    assert!(matches!(r, Err(JevError::Call(_))), "{r:?}");
    assert!(
        rx1.recv_timeout(Duration::from_secs(2)).is_ok(),
        "the first host was asked"
    );
    assert!(
        rx2.recv_timeout(Duration::from_millis(500)).is_err(),
        "the second host was never asked"
    );
}

// S-62
#[test]
fn an_http_error_is_a_failed_call_and_the_key_is_not_in_its_text() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let (tx, _rx) = mpsc::channel();
    serve_once(
        l,
        respond(
            "401 Unauthorized",
            "",
            &format!("{{\"detail\":\"bad key {KEY}\"}}"),
        ),
        tx,
    );
    let e = ask(&jev(&base, Duration::from_secs(5))).unwrap_err();
    let text = e.to_string();
    assert!(
        matches!(e, JevError::Call(_)) && text.contains("401"),
        "{text}"
    );
    assert!(!text.contains(KEY), "{text}");
}

// S-62
#[test]
fn a_server_that_never_answers_ends_by_the_time_limit() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    std::thread::spawn(move || {
        let _held = l.accept();
        std::thread::sleep(Duration::from_secs(30));
    });
    let t = Instant::now();
    let r = ask(&jev(&base, Duration::from_millis(500)));
    assert!(matches!(r, Err(JevError::Call(_))), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
}

// S-62
#[test]
fn the_adapter_refuses_to_send_the_key_to_any_other_host() {
    for base in [
        "http://example.com",
        "https://example.com",
        "http://127.0.0.1.evil.test",
        "https://api.typesafe.ai.evil.test",
    ] {
        let e = ask(&jev(base, Duration::from_secs(2))).unwrap_err();
        assert!(
            e.to_string().contains("JEV_BASE_URL must be"),
            "{base}: {e}"
        );
    }
}

// The transport cleans its own error text, whoever calls it.
// S-62
#[test]
fn the_transport_alone_does_not_leak_the_key_in_an_error() {
    use clio_jev::Transport;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let (tx, _rx) = mpsc::channel();
    serve_once(
        l,
        respond(
            "401 Unauthorized",
            "",
            &format!("{{\"detail\":\"bad key {KEY}\"}}"),
        ),
        tx,
    );
    let p = Process {
        python: "python3".into(),
        script: concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../adapters/jev/jev_systemone.py"
        )
        .into(),
        base_url: Some(base),
    };
    let e = p
        .call(
            KEY,
            &json!({"model": "m", "state": "s", "questions": {}}),
            Duration::from_secs(5),
        )
        .unwrap_err();
    assert!(e.contains("401") && !e.contains(KEY), "{e}");
}

// A server that repeats the request's header in its error body must not leak even a part of the key:
// the adapter reports the status and its reason, nothing the server said.
// S-62
#[test]
fn a_part_of_the_key_echoed_by_the_server_does_not_reach_the_error() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let (tx, _rx) = mpsc::channel();
    let echo = format!("{}Authorization: Bearer {}", "x".repeat(50), &KEY[..12]);
    serve_once(l, respond("401 Unauthorized", "", &echo), tx);
    let e = ask(&jev(&base, Duration::from_secs(5)))
        .unwrap_err()
        .to_string();
    assert!(e.contains("401"), "{e}");
    assert!(!e.contains(&KEY[..8]) && !e.contains("Bearer"), "{e}");
}
