#![allow(clippy::unwrap_used)]
//! The egress proxy of Pi's jail (ADR 0009, phase 3b): one destination, taken from the
//! provider's URL, and nothing else. It is the only way out of the jail, so what it refuses
//! matters as much as what it lets through (S-06, S-02).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use clio_brain::pi::net::{EgressProxy, allow_of};

/// A TCP server that answers "pong" to whatever it receives, and the proxy that allows it.
fn rig() -> (EgressProxy, u16, tempfile::TempDir) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut b = [0u8; 16];
                if s.read(&mut b).is_ok_and(|n| n > 0) {
                    let _ = s.write_all(b"pong");
                }
            });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let p = EgressProxy::start(dir.path(), &format!("127.0.0.1:{port}")).unwrap();
    (p, port, dir)
}

/// Sends `request` to the proxy and returns the first line of its answer.
fn ask(p: &EgressProxy, request: &[u8]) -> (String, UnixStream) {
    let mut s = UnixStream::connect(p.socket()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // The proxy may refuse and close before it has read everything: a failed write is not a
    // failed test, what it answered is what is checked.
    let _ = s.write_all(request);
    let mut got = Vec::new();
    let mut b = [0u8; 1];
    while !got.ends_with(b"\r\n") {
        if s.read(&mut b).unwrap_or(0) == 0 {
            break;
        }
        got.push(b[0]);
    }
    (String::from_utf8_lossy(&got).trim().to_owned(), s)
}

#[test]
fn it_tunnels_to_the_one_destination_it_was_given() {
    let (p, port, _d) = rig();
    let (line, mut s) = ask(
        &p,
        format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(),
    );
    assert!(line.starts_with("HTTP/1.1 200"), "{line}");
    // The empty line that ends the answer, then the tunnel: what goes in comes out of the server.
    let mut crlf = [0u8; 2];
    s.read_exact(&mut crlf).unwrap();
    s.write_all(b"ping").unwrap();
    let mut pong = [0u8; 4];
    s.read_exact(&mut pong).unwrap();
    assert_eq!(&pong, b"pong");
    assert!(p.denied().is_empty());
}

#[test]
fn it_refuses_everything_else_and_remembers_what_was_asked() {
    let (p, port, _d) = rig();
    for (req, want) in [
        // Another host, another port of the same host, and tricks that read like the allowed one.
        ("CONNECT example.com:443 HTTP/1.1\r\n\r\n".to_owned(), "403"),
        (
            format!(
                "CONNECT 127.0.0.1:{} HTTP/1.1\r\n\r\n",
                port.wrapping_add(1)
            ),
            "403",
        ),
        (
            format!("CONNECT 127.0.0.1:{port}@evil.example:443 HTTP/1.1\r\n\r\n"),
            "403",
        ),
        (
            format!("CONNECT user@127.0.0.1:{port} HTTP/1.1\r\n\r\n"),
            "403",
        ),
        (format!("CONNECT 127.0.0.1.:{port} HTTP/1.1\r\n\r\n"), "403"),
        // Not a tunnel at all: a plain request would make the proxy fetch for Pi.
        (
            format!("GET http://127.0.0.1:{port}/ HTTP/1.1\r\n\r\n"),
            "405",
        ),
        ("POST / HTTP/1.1\r\n\r\n".to_owned(), "405"),
        // Not HTTP.
        ("hello\r\n\r\n".to_owned(), "400"),
        (String::from("CONNECT\r\n\r\n"), "400"),
    ] {
        let (line, _s) = ask(&p, req.as_bytes());
        assert!(line.contains(want), "{req:?} -> {line}");
    }
    let denied = p.denied();
    assert!(
        denied.iter().any(|d| d.contains("example.com:443")),
        "{denied:?}"
    );
}

#[test]
fn it_does_not_read_a_head_without_end() {
    let (p, _port, _d) = rig();
    // 64 KiB of header and no blank line: it answers and closes, it does not keep reading.
    let mut junk = b"CONNECT 127.0.0.1:1 HTTP/1.1\r\nX: ".to_vec();
    junk.extend(std::iter::repeat_n(b'a', 64 * 1024));
    let (line, _s) = ask(&p, &junk);
    assert!(line.contains("431") || line.contains("400"), "{line}");
}

#[test]
fn the_allowed_destination_comes_from_the_providers_url() {
    for (url, want) in [
        (
            "https://opencode.ai/inference/go/openai/v1",
            "opencode.ai:443",
        ),
        ("https://API.Example.com:8443/v1", "api.example.com:8443"),
        ("http://127.0.0.1:4000/v1", "127.0.0.1:4000"),
        ("http://localhost/v1", "localhost:80"),
    ] {
        assert_eq!(allow_of(url).as_deref(), Ok(want), "{url}");
    }
    // A URL that reads as one host and means another is not a destination.
    for bad in [
        "https://user:pw@opencode.ai/v1",
        "https://opencode.ai@evil.example/v1",
        "ftp://opencode.ai/v1",
        "opencode.ai",
        "https:///v1",
        "https://[::1]/v1",
        "https://opencode.ai:notaport/v1",
        "https://a b/v1",
    ] {
        assert!(allow_of(bad).is_err(), "{bad}");
    }
}

#[test]
fn dropping_the_proxy_removes_its_socket_and_folder() {
    let (p, _port, _d) = rig();
    let sock = p.socket().to_path_buf();
    assert!(sock.exists());
    drop(p);
    assert!(!sock.exists() && !sock.parent().unwrap().exists());
}
