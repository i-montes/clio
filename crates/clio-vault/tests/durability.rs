#![allow(clippy::unwrap_used)]
//! The vault's file and sockets under stress (S-22, S-23, S-47): a secret that could not be written
//! is not kept as if it had been, a client does not wait forever for a vault that does not answer,
//! and a server does not take connections without end.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clio_core::SystemClock;
use clio_vault::proto::Request;
use clio_vault::{Dest, GatewayCap, Kind, Scope, SecretSpec, Vault, VaultClient, VaultError};

const PASS: &str = "correct horse battery staple";

fn spec(name: &str, value: &str) -> SecretSpec {
    SecretSpec {
        name: name.into(),
        value: value.into(),
        kind: Kind::ApiKey,
        destinations: vec![Dest::new("notion-api", "api.notion.com")],
        scope: Scope::Personal,
        rotate_every_ms: None,
        expires_at_ms: None,
    }
}

/// Makes the vault's folder read-only for the length of the test, and gives it back after, also
/// when the test fails (the temporary folder must be removable).
struct ReadOnly<'a>(&'a Path);

impl<'a> ReadOnly<'a> {
    /// `None` where the folder stays writable anyway (running as root): nothing to test there.
    fn new(dir: &'a Path) -> Option<Self> {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::File::create(dir.join(".probe")).is_ok() {
            let _ = std::fs::remove_file(dir.join(".probe"));
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            eprintln!("SKIPPED: this user can write a read-only folder (root?)");
            return None;
        }
        Some(Self(dir))
    }
}

impl Drop for ReadOnly<'_> {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o700));
    }
}

fn value_of(v: &Vault, name: &str) -> Result<String, VaultError> {
    v.resolve(
        &GatewayCap::for_gateway(),
        name,
        "notion-api",
        "api.notion.com",
        "t",
    )
    .map(|s| s.expose().to_owned())
}

// S-22: a `put` that could not reach the disk fails, and the vault does not go on as if the secret
// were stored: the next restart would lose it, and the person was told it was saved.
#[test]
fn a_secret_that_could_not_be_written_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("vault.json");
    let (v, cap) = Vault::init(Some(&file), PASS, Arc::new(SystemClock)).unwrap();
    v.put(&cap, spec("notion", "the-first-value-123")).unwrap();
    let Some(_ro) = ReadOnly::new(dir.path()) else {
        return;
    };
    // A new name: it must not appear. An existing one: it must keep its old value.
    assert!(
        v.put(&cap, spec("github", "a-brand-new-value-123"))
            .is_err()
    );
    assert!(
        v.list(&cap).iter().all(|m| m.name != "github"),
        "a secret that was never saved is listed"
    );
    assert!(v.put(&cap, spec("notion", "the-second-value-456")).is_err());
    assert_eq!(value_of(&v, "notion").unwrap(), "the-first-value-123");
}

// S-47: every wait has a deadline. A vault that accepts and never answers (hung, stopped) must not
// hold the host that asked it to scrub an output, with its one-call-at-a-time lock, forever.
#[test]
fn the_client_gives_up_on_a_vault_that_does_not_answer() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("mute.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        let held: Vec<_> = listener.incoming().flatten().collect(); // accepts, says nothing
        std::thread::sleep(Duration::from_secs(30));
        drop(held);
    });
    let client = VaultClient::new(&sock).with_timeout(Duration::from_millis(300));
    let t = Instant::now();
    assert!(client.call(&Request::Status).is_err());
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

// S-47: a process of the person can open connections without end; each one holds a thread and up
// to 16 MiB of a request. The server takes a bounded number and drops the rest at once.
#[test]
fn the_server_drops_connections_beyond_its_cap() {
    let dir = tempfile::tempdir().unwrap();
    let (gw, person) = (dir.path().join("gw.sock"), dir.path().join("person.sock"));
    let (vault, _cap) = Vault::init(None, PASS, Arc::new(SystemClock)).unwrap();
    let (g, p) = (gw.clone(), person.clone());
    std::thread::spawn(move || clio_vault::server::serve(Arc::new(vault), &g, &p));
    let t = Instant::now();
    while !(gw.exists() && person.exists()) {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the server never came up"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // 100 silent connections: the first ones are served (and wait for a request), the rest are cut.
    let conns: Vec<UnixStream> = (0..100)
        .map(|_| UnixStream::connect(&person).unwrap())
        .collect();
    std::thread::sleep(Duration::from_millis(500)); // the server takes or cuts each one
    let closed = conns
        .iter()
        .filter(|s| {
            // No waiting: a cut connection reads end of file now, a served one has nothing yet.
            s.set_nonblocking(true).unwrap();
            let mut b = [0u8; 1];
            matches!((&**s).read(&mut b), Ok(0))
        })
        .count();
    assert!(
        (30..=45).contains(&closed),
        "{closed} of 100 were cut: the cap is 64, so about 36 should be"
    );
}
