#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]
//! ADR 0009, phase 3a (attack-54, S-22, S-03, S-16): Pi runs inside a `bwrap` jail. A process the
//! jail starts cannot see the vault's sockets or the person's files, can reach the toolhost's
//! socket, and finds the working copy read-only. If the jail cannot be built, Pi does not start.

mod pi_support;

use clio_attacks::{Case, Observed, Status, Target, load_cases, run as run_cases};
use std::ffi::OsString;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clio_brain::pi::jail::{self, PiJail};
use clio_brain::pi::launch::{Launch, start};
use clio_core::SystemClock;
use clio_vault::Vault;
use pi_support::Rig;

/// Tries to connect to each unix socket given and says what happened.
const PROBE: &str = r#"
import socket, sys
for p in sys.argv[1:]:
    s = socket.socket(socket.AF_UNIX)
    try:
        s.connect(p)
        print("CONNECTED", p.rsplit("/", 1)[1])
    except OSError as e:
        print("blocked", p.rsplit("/", 1)[1], e.errno)
"#;

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// The jail for a launch in a temporary folder. Skipped (printing why) only where `bwrap` cannot
/// start at all; where it can, a jail that fails to prepare is a failure of the test.
fn jail_for(test: &str, rig: &Rig, l: &Launch) -> Option<PiJail> {
    rig.paths.ensure().unwrap();
    if let jail::Status::Unavailable(why) = jail::status() {
        assert!(
            std::env::var_os("CLIO_REQUIRE_JAIL").is_none(),
            "CLIO_REQUIRE_JAIL is set and the jail is not available: {why}"
        );
        eprintln!("SKIPPED {test} (the jail cannot run here, nothing was tested): {why}");
        return None;
    }
    match jail::prepare(l) {
        Ok(Some(j)) => Some(j),
        Ok(None) => panic!("{test}: on Linux the jail is not optional"),
        Err(e) => panic!("{test}: the jail did not prepare: {e}"),
    }
}

fn run(j: &PiJail, l: &Launch, argv: &[&str]) -> String {
    let o = j.command(l, "sess-test-1", &os(argv)).output().unwrap();
    String::from_utf8_lossy(&[o.stdout, o.stderr].concat()).into_owned()
}

// S-22 S-03, finding 5 (attack-54): the vault tells its two sockets apart by which one a request came in on,
// and both are 0600 sockets of the same user. Pi's process must not be able to see them.
#[test]
fn a_process_in_pis_jail_cannot_reach_the_vault_sockets_but_reaches_the_toolhost() {
    let Some(node) = pi_support::need("pi jail: sockets", false) else {
        return;
    };
    if !Path::new("/usr/bin/python3").exists() {
        eprintln!("SKIPPED pi jail: sockets (no /usr/bin/python3 to probe with)");
        return;
    }
    let rig = Rig::new(node);
    let runtime: PathBuf = rig.tmp.path().join("run/clio");
    let th_dir = runtime.join("th-1-0");
    std::fs::create_dir_all(&th_dir).unwrap();
    // The vault, as `clio vault start` runs it: two sockets in the runtime folder.
    let (gw, person) = (
        runtime.join("vault-gateway.sock"),
        runtime.join("vault-person.sock"),
    );
    let (vault, _cap) =
        Vault::init(None, "correct horse battery staple", Arc::new(SystemClock)).unwrap();
    let (g, p) = (gw.clone(), person.clone());
    std::thread::spawn(move || clio_vault::server::serve(Arc::new(vault), &g, &p));
    // The toolhost's socket, in its own folder next to them.
    let th_sock = th_dir.join("toolhost.sock");
    let _toolhost = UnixListener::bind(&th_sock).unwrap();
    let t = std::time::Instant::now();
    while !(gw.exists() && person.exists()) {
        assert!(t.elapsed().as_secs() < 10, "the vault never came up");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut l = rig.launch("http://127.0.0.1:1/v1", None);
    l.extra_env = vec![("CLIO_TOOLHOST_SOCKET".into(), th_sock.display().to_string())];
    let Some(j) = jail_for("pi jail: sockets", &rig, &l) else {
        return;
    };
    let out = run(
        &j,
        &l,
        &[
            "/usr/bin/python3",
            "-I",
            "-c",
            PROBE,
            gw.to_str().unwrap(),
            person.to_str().unwrap(),
            th_sock.to_str().unwrap(),
        ],
    );
    assert!(out.contains("blocked vault-gateway.sock"), "{out}");
    assert!(out.contains("blocked vault-person.sock"), "{out}");
    assert!(out.contains("CONNECTED toolhost.sock"), "{out}");
}

// S-16 S-03: a compromised Pi must not write in the copy itself and skip the toolhost's rules
// (`.git/hooks`, `skills/`). Its own folders stay writable: the sessions are Pi's.
#[test]
fn the_working_copy_is_read_only_inside_pis_jail_and_pis_own_folders_are_not() {
    let Some(node) = pi_support::need("pi jail: read-only copy", false) else {
        return;
    };
    let rig = Rig::new(node);
    std::fs::write(rig.proj.join("a.txt"), "x").unwrap();
    let l = rig.launch("http://127.0.0.1:1/v1", None);
    let Some(j) = jail_for("pi jail: read-only copy", &rig, &l) else {
        return;
    };
    let (proj, agent) = (rig.proj.display(), rig.paths.agent_dir());
    let out = run(
        &j,
        &l,
        &[
            "/usr/bin/sh",
            "-c",
            &format!(
                "cat {proj}/a.txt; ( : > {proj}/new ) 2>/dev/null && echo copy-writable; ( : > {proj}/a.txt ) 2>/dev/null && echo copy-overwritable; ( : > {a}/note ) && echo agent-writable",
                a = agent.display()
            ),
        ],
    );
    assert!(out.contains('x') && out.contains("agent-writable"), "{out}");
    assert!(
        !out.contains("copy-writable") && !out.contains("copy-overwritable"),
        "{out}"
    );
    assert!(!rig.proj.join("new").exists());
}

// S-01 S-02: fail closed. If the jail cannot be built, Pi does not start, and the person is told why.
#[test]
fn pi_does_not_start_when_its_jail_cannot_be_built() {
    let Some(node) = pi_support::need("pi jail: fails closed", false) else {
        return;
    };
    let rig = Rig::new(node);
    let mut l = rig.launch("http://127.0.0.1:1/v1", None);
    // A folder that does not exist cannot be mounted: the jail (and so its self-test) fails.
    l.install.dir = rig.tmp.path().join("not-installed");
    let err = match start(&l) {
        Ok(_) => panic!("Pi started without a jail it could build"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("jaula"), "{err}");
}

// S-06 S-03, attack-55 (phase 3b): the jail has a network of its own. Inside, a host's abstract
// socket, a loopback port of the host and the internet are out of reach; the provider's address
// is reached only through the proxy, and any other `CONNECT` is refused.
#[test]
fn pis_jail_reaches_the_provider_through_the_proxy_and_nothing_else() {
    let Some(node) = pi_support::need("pi jail: network", false) else {
        return;
    };
    if !Path::new("/usr/bin/python3").exists() {
        eprintln!("SKIPPED pi jail: network (no /usr/bin/python3 to probe with)");
        return;
    }
    let rig = Rig::new(node);
    // The provider: a TCP server on the host's loopback that answers "pong". The jail's launch
    // names it as the provider, so it is the one allowed destination.
    let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let pport = provider.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in provider.incoming().flatten() {
            let mut s = s;
            let mut b = [0u8; 8];
            if std::io::Read::read(&mut s, &mut b).is_ok_and(|n| n > 0) {
                let _ = std::io::Write::write_all(&mut s, b"pong");
            }
        }
    });
    // Something else on the host's loopback, and an abstract socket: both must stay out of reach.
    let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let oport = other.local_addr().unwrap().port();
    let abstract_name = format!("clio-egress-probe-{}", std::process::id());
    let _abstract = {
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(&abstract_name).unwrap();
        UnixListener::bind_addr(&addr).unwrap()
    };
    let l = rig.launch(&format!("http://127.0.0.1:{pport}/v1"), None);
    let Some(j) = jail_for("pi jail: network", &rig, &l) else {
        return;
    };
    // What the self-test itself asked for and was refused is not Pi's: the chat must not show it.
    assert!(
        j.egress_denied().is_empty(),
        "the self-test's refusals leak into what is shown: {:?}",
        j.egress_denied()
    );
    let script = format!(
        r#"
import socket, os
def t(name, f):
    try: f(); print(name, "REACHED")
    except OSError as e: print(name, "blocked")
t("internet", lambda: socket.create_connection(("1.1.1.1", 443), timeout=4))
t("host-loopback", lambda: socket.create_connection(("127.0.0.1", {oport}), timeout=4))
def ab():
    s = socket.socket(socket.AF_UNIX); s.connect("\0{abstract_name}")
t("abstract-socket", ab)
def via_proxy(target):
    s = socket.create_connection(("127.0.0.1", 18080), timeout=6)
    s.sendall(("CONNECT %s HTTP/1.1\r\n\r\n" % target).encode())
    head = b""
    while b"\r\n\r\n" not in head:
        d = s.recv(1); 
        if not d: break
        head += d
    code = head.split(b" ")[1].decode() if b" " in head else "?"
    if code == "200":
        s.sendall(b"ping"); return code + " " + s.recv(8).decode()
    return code
print("provider:", via_proxy("127.0.0.1:{pport}"))
print("other-host:", via_proxy("example.com:443"))
print("other-port:", via_proxy("127.0.0.1:{oport}"))
print("proxy-env:", os.environ.get("HTTPS_PROXY"))
"#
    );
    let out = run(&j, &l, &["/usr/bin/python3", "-I", "-c", &script]);
    for want in [
        "internet blocked",
        "host-loopback blocked",
        "abstract-socket blocked",
        "provider: 200 pong",
        "other-host: 403",
        "other-port: 403",
        "proxy-env: http://127.0.0.1:18080",
    ] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}");
    }
    // What was refused is remembered, for the chat to say (the provider's own connection is not).
    let denied = j.egress_denied();
    assert!(denied.iter().any(|d| d == "example.com:443"), "{denied:?}");
    assert!(
        !denied
            .iter()
            .any(|d| d.contains(&format!("127.0.0.1:{pport}"))),
        "{denied:?}"
    );
    drop(other);
}

// ---------------------------------------------------------------------------------------------
// The attack cases that live in this file, through clio-attacks (the gate of the battery)
// ---------------------------------------------------------------------------------------------

/// A scenario holds when it does not panic: the plain tests above are the scenarios, with their
/// own messages.
fn held(f: impl FnOnce() + std::panic::UnwindSafe) -> bool {
    std::panic::catch_unwind(f).is_ok()
}

const OURS: [&str; 2] = ["attack-54", "attack-55"];

struct PiJailTarget;

impl Target for PiJailTarget {
    fn attack(&self, case: &Case) -> Observed {
        let refused = match case.id.as_str() {
            "attack-54" => {
                held(a_process_in_pis_jail_cannot_reach_the_vault_sockets_but_reaches_the_toolhost)
            }
            "attack-55" => held(pis_jail_reaches_the_provider_through_the_proxy_and_nothing_else),
            other => panic!("{other} is active but this target has no scenario for it"),
        };
        Observed { refused }
    }
}

// S-22 S-03 S-06: attacks 54 and 55
#[test]
fn the_pi_jail_cases_are_active_and_refused() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testing/attacks/cases");
    let mine: Vec<Case> = load_cases(&dir)
        .unwrap()
        .into_iter()
        .filter(|c| OURS.contains(&c.id.as_str()))
        .collect();
    assert_eq!(
        mine.len(),
        OURS.len(),
        "a case is missing in testing/attacks/cases"
    );
    assert!(mine.iter().all(|c| c.status == Status::Active));
    let report = run_cases(&mine, &PiJailTarget);
    assert!(report.ok(), "{}", report.line());
    assert_eq!(report.passed.len(), OURS.len());
    eprintln!("{}", report.line());
}

// S-01 S-02: the self-test decides whether Pi starts, so it must not be flaky. It reads an answer
// the proxy writes, and a read can return only a part of it: preparing the jail many times in a
// row, a jail that is fine must never be refused (it was, now and then, under load).
#[test]
fn the_self_test_does_not_refuse_a_good_jail_now_and_then() {
    let Some(node) = pi_support::need("pi jail: stability", false) else {
        return;
    };
    let rig = Rig::new(node);
    let l = rig.launch("http://127.0.0.1:1/v1", None);
    rig.paths.ensure().unwrap();
    if let jail::Status::Unavailable(why) = jail::status() {
        assert!(std::env::var_os("CLIO_REQUIRE_JAIL").is_none(), "{why}");
        eprintln!("SKIPPED pi jail: stability ({why})");
        return;
    }
    // Several at a time, as the chat tests do: the proxy answers while the machine is busy.
    let results: Vec<_> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(|| {
                    (0..15)
                        .filter_map(|_| jail::prepare(&l).err().map(|e| e.to_string()))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    assert!(
        results.is_empty(),
        "{} of 60 refused: {:?}",
        results.len(),
        results.first()
    );
}
