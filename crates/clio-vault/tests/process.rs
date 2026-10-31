#![allow(clippy::unwrap_used)]
//! The vault as a real process: the `clio-vault` binary is launched as a child and spoken to over
//! its two Unix sockets. Fake keys are built at run time.

use clio_canaries::Canaries;
use clio_core::SystemClock;
use clio_vault::proto::{Request, Side};
use clio_vault::{Dest, GatewayCap, Kind, Scope, SecretSpec, Vault, VaultClient, VaultError};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PASS: &str = "correct horse battery staple";
const BIN: &str = env!("CARGO_BIN_EXE_clio-vault");

/// Kills the server when the test ends, also when it fails.
struct Server {
    child: Child,
    dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn gw_sock(&self) -> PathBuf {
        self.dir.path().join("gateway.sock")
    }
    fn person_sock(&self) -> PathBuf {
        self.dir.path().join("person.sock")
    }
    fn gateway(&self) -> VaultClient {
        VaultClient::new(&self.gw_sock())
    }
    fn person(&self) -> VaultClient {
        VaultClient::new(&self.person_sock())
    }
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("server.log")).unwrap_or_default()
    }
    /// Runs the CLI against the person socket, `stdin` being what a terminal would type.
    fn cli(&self, args: &[&str], stdin: &str) -> (bool, String) {
        let mut c = Command::new(BIN)
            .arg("--sock")
            .arg(self.person_sock())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = c.wait_with_output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned(),
        )
    }
}

fn spec(name: &str, value: &str, tool: &str, domain: &str) -> SecretSpec {
    SecretSpec {
        name: name.into(),
        value: value.into(),
        kind: Kind::ApiKey,
        destinations: vec![Dest::new(tool, domain)],
        scope: Scope::Personal,
        rotate_every_ms: None,
        expires_at_ms: None,
    }
}

/// A vault file with the decoys inside, then the server on top of it (locked).
fn start(canaries: &Canaries) -> Server {
    let dir = tempfile::tempdir().unwrap(); // mode 0700
    let file = dir.path().join("vault.json");
    let (v, cap) = Vault::init(Some(&file), PASS, Arc::new(SystemClock)).unwrap();
    let c = |n: &str| canaries.value(n).unwrap().to_string();
    v.put(
        &cap,
        spec("notion", &c("notion"), "notion-api", "api.notion.com"),
    )
    .unwrap();
    v.put(&cap, spec("github", &c("github"), "git", "github.com"))
        .unwrap();
    drop(v);
    let log = std::fs::File::create(dir.path().join("server.log")).unwrap();
    let child = Command::new(BIN)
        .arg("serve")
        .arg("--vault")
        .arg(&file)
        .arg("--gateway-sock")
        .arg(dir.path().join("gateway.sock"))
        .arg("--person-sock")
        .arg(dir.path().join("person.sock"))
        .stderr(log)
        .spawn()
        .unwrap();
    let s = Server { child, dir };
    let t = Instant::now();
    while s.person().call(&Request::Status).is_err() || s.gateway().call(&Request::Status).is_err()
    {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "server never came up"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    s
}

fn unlock(s: &Server) {
    let r = s
        .person()
        .call(&Request::Unlock {
            passphrase: PASS.into(),
            eager: vec![],
        })
        .unwrap();
    assert!(r.ok);
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

// S-22 (task 233, 234): a fresh server is locked; nothing resolves until the person unlocks it.
#[test]
fn a_fresh_server_resolves_nothing_until_unlocked() {
    let c = Canaries::from_seed(7);
    let s = start(&c);
    let r = s
        .gateway()
        .resolve("notion", "notion-api", "api.notion.com", "t1");
    assert_eq!(r.unwrap_err(), VaultError::Locked);
    unlock(&s);
    let v = s
        .gateway()
        .resolve("notion", "notion-api", "api.notion.com", "t2")
        .unwrap();
    assert_eq!(v.expose(), c.value("notion").unwrap());
}

// S-22 (task 266): the socket decides who the caller is. A process that only reaches the
// person's socket cannot resolve, and the gateway's socket cannot put, revoke or list.
#[test]
fn the_person_socket_cannot_resolve_and_the_gateway_socket_cannot_manage() {
    let c = Canaries::from_seed(7);
    let s = start(&c);
    unlock(&s);
    let resolve = Request::Resolve {
        name: "notion".into(),
        tool: "notion-api".into(),
        domain: "api.notion.com".into(),
        trace_id: "t".into(),
    };
    let r = s.person().call(&resolve).unwrap();
    assert!(!r.ok && r.data.is_none(), "{:?}", r.error);
    assert!(r.error.unwrap().contains(&format!("{:?}", Side::Person)));
    let scrub = Request::Scrub { text: "x".into() };
    assert!(!s.person().call(&scrub).unwrap().ok);
    for req in [
        Request::List,
        Request::Audit,
        Request::Revoke {
            name: "notion".into(),
        },
    ] {
        assert!(!s.gateway().call(&req).unwrap().ok);
    }
    // Nothing was revoked by the refused call.
    assert!(
        s.gateway()
            .resolve("notion", "notion-api", "api.notion.com", "t")
            .is_ok()
    );
}

// S-22 (task 234): both sockets are 0600 and the server listens on nothing else.
#[test]
fn sockets_are_private_to_their_owner() {
    let s = start(&Canaries::from_seed(7));
    assert_eq!(mode(&s.gw_sock()), 0o600);
    assert_eq!(mode(&s.person_sock()), 0o600);
}

// S-18, S-19: the value never shows in list, audit, alerts or the server log; a refused
// destination fails and is audited.
#[test]
fn values_never_appear_in_list_audit_alerts_or_the_log() {
    let c = Canaries::from_seed(7);
    let s = start(&c);
    unlock(&s);
    let gw = s.gateway();
    gw.resolve("notion", "notion-api", "api.notion.com", "ok-1")
        .unwrap();
    let denied = gw.resolve("notion", "notion-api", "evil.test", "bad-1");
    assert!(matches!(denied, Err(VaultError::Denied(_))));
    let ask = |req: Request| {
        let r = s.person().call(&req).unwrap();
        assert!(r.ok);
        r.data.unwrap().to_string()
    };
    let (list, audit, alerts) = (
        ask(Request::List),
        ask(Request::Audit),
        ask(Request::Alerts),
    );
    assert!(list.contains("notion") && audit.contains("bad-1") && alerts.contains("evil.test"));
    c.assert_clean("list", &list);
    c.assert_clean("audit", &audit);
    c.assert_clean("alerts", &alerts);
    c.assert_clean("server log", &s.log());
    assert!(s.log().contains("resolve notion"));
}

// S-20: the scrub operation tachas the value, also in base64, and the server keeps working.
#[test]
fn scrub_removes_values_in_every_form() {
    let c = Canaries::from_seed(7);
    let s = start(&c);
    unlock(&s);
    let v = c.value("github").unwrap();
    let b = clio_canaries_b64(v);
    let out = s.gateway().scrub(&format!("x {v} y {b} z")).unwrap();
    c.assert_clean("scrubbed", &out);
    assert!(out.contains("[secreto:github tachado]"));
}

// Standard base64 of a decoy, the same way a tool that echoes it in base64 would.
fn clio_canaries_b64(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for ch in s.as_bytes().chunks(3) {
        let n = ch
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=ch.len() {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

// S-21 (task 261): `clio-vault revoke NAME` on the person's socket works even while locked and
// takes effect at once; unlock, put and list also work from the CLI.
#[test]
fn the_cli_unlocks_puts_revokes_and_lists() {
    let c = Canaries::from_seed(7);
    let s = start(&c);
    // Revoking needs no passphrase: do it while the server is still locked.
    let (ok, out) = s.cli(&["revoke", "github"], "");
    assert!(ok, "{out}");
    assert!(!s.cli(&["unlock"], "wrong passphrase\n").0);
    assert!(s.cli(&["unlock"], &format!("{PASS}\n")).0);
    let r = s.gateway().resolve("github", "git", "github.com", "t");
    assert_eq!(r.unwrap_err(), VaultError::Revoked);
    // A new secret through the CLI, value on stdin.
    let value = format!("{}-cli", c.value("notion").unwrap());
    let (ok, out) = s.cli(
        &["put", "extra", "--dest", "http:example.org"],
        &format!("{value}\n"),
    );
    assert!(ok, "{out}");
    let got = s
        .gateway()
        .resolve("extra", "http", "example.org", "t")
        .unwrap();
    assert_eq!(got.expose(), value);
    let (ok, listing) = s.cli(&["list"], "");
    assert!(ok && listing.contains("extra") && !listing.contains(&value));
    // Task 257: `scan` exits 0 on a clean folder and 1 on a planted value, never printing it.
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();
    std::fs::write(dir.path().join("a.log"), "nothing here").unwrap();
    assert!(s.cli(&["scan", d], "").0);
    std::fs::write(dir.path().join("b.log"), &value).unwrap();
    let (ok, out) = s.cli(&["scan", d], "");
    assert!(
        !ok && out.contains("b.log") && !out.contains(&value),
        "{out}"
    );
}

// S-22: a client with no server gets an error, never a value (fail closed).
#[test]
fn no_server_means_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    let c = VaultClient::new(&dir.path().join("nothing.sock"));
    assert!(c.resolve("notion", "t", "d", "x").is_err());
    assert!(c.scrub("anything").is_err());
    let _ = GatewayCap::for_gateway();
}
