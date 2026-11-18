#![allow(clippy::unwrap_used)]
//! The host launcher: the real machine, with Clio's folders masked. The argv of both launchers is
//! checked without running anything; `Bwrap` also runs for real (it needs unprivileged user
//! namespaces: where they are missing the test prints `SKIPPED`). `Systemd` needs passwordless
//! `sudo`, so it runs for real only with `CLIO_TEST_SUDO=1`: this suite never uses `sudo` by
//! default.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clio_node::host::{HostJail, HostJob, Launcher};
use tempfile::TempDir;

fn with(l: Launcher) -> Option<HostJail> {
    match HostJail::with(l) {
        Ok(h) => Some(h),
        Err(why) => {
            eprintln!("SKIPPED ({l:?} is not available here, nothing was tested): {why}");
            None
        }
    }
}

fn job(dir: &Path, command: &str) -> HostJob {
    HostJob {
        command: command.into(),
        cwd: dir.into(),
        home: dir.into(),
        network: false,
        timeout: Some(Duration::from_secs(20)),
        max_output: 64 * 1024,
        masks: vec![],
    }
}

/// A folder with a decoy inside, plus a mask path that does not exist.
fn masked(d: &TempDir) -> (PathBuf, PathBuf) {
    let m = d.path().join("clio");
    std::fs::create_dir(&m).unwrap();
    std::fs::write(m.join("decoy"), "secret").unwrap();
    (m.canonicalize().unwrap(), d.path().join("missing"))
}

/// The values that follow each `flag` in `args`.
fn after<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
    args.windows(2)
        .filter(|w| w[0] == flag)
        .map(|w| w[1].as_str())
        .collect()
}

fn uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").unwrap().uid()
}

#[test]
fn bwrap_args_mask_existing_folders_cut_the_network_and_clear_the_environment() {
    let Some(h) = with(Launcher::Bwrap) else {
        return;
    };
    assert_eq!(h.launcher(), Launcher::Bwrap);
    let d = TempDir::new().unwrap();
    let (mask, missing) = masked(&d);
    let mut j = job(d.path(), "echo hi");
    j.masks = vec![mask.clone(), missing.clone()];
    let (prog, a) = h.args(&j).unwrap();
    assert!(prog.ends_with("bwrap"), "{prog:?}");
    let m = mask.to_string_lossy();
    assert_eq!(after(&a, "--tmpfs"), [m.as_ref()]);
    assert_eq!(after(&a, "--remount-ro"), [m.as_ref()]);
    assert!(!a.iter().any(|x| x.contains("missing")), "{a:?}");
    for flag in [
        "--unshare-net",
        "--unshare-pid",
        "--clearenv",
        "--die-with-parent",
        "--new-session",
    ] {
        assert!(a.contains(&flag.to_owned()), "{flag} missing from {a:?}");
    }
    assert_eq!(after(&a, "--bind"), ["/"]);
    let keys: BTreeSet<&str> = after(&a, "--setenv").into_iter().collect();
    assert_eq!(
        keys,
        BTreeSet::from(["HOME", "LANG", "LOGNAME", "PATH", "USER", "XDG_RUNTIME_DIR"])
    );
    assert!(a.ends_with(&["-lc".into(), "echo hi".into()]), "{a:?}");
    assert!(a[a.len() - 3].ends_with("/bash"));

    j.network = true;
    let (_, a) = h.args(&j).unwrap();
    assert!(!a.contains(&"--unshare-net".to_owned()));
}

#[test]
fn systemd_args_run_as_this_user_with_masks_no_network_a_deadline_and_a_fixed_environment() {
    let Some(h) = with(Launcher::Systemd) else {
        return;
    };
    assert_eq!(h.launcher(), Launcher::Systemd);
    let d = TempDir::new().unwrap();
    let (mask, missing) = masked(&d);
    let mut j = job(d.path(), "echo hi");
    j.masks = vec![mask.clone(), missing];
    j.timeout = Some(Duration::from_secs(90));
    let (prog, a) = h.args(&j).unwrap();
    assert!(prog.ends_with("sudo"), "{prog:?}");
    assert_eq!(a[0], "-n");
    assert!(a[1].ends_with("systemd-run"), "{a:?}");
    for flag in ["--quiet", "--wait", "--pipe", "--collect"] {
        assert!(a.contains(&flag.to_owned()), "{flag} missing from {a:?}");
    }
    let u = uid().to_string();
    assert!(a.contains(&format!("--uid={u}")), "{a:?}");
    assert!(a.iter().any(|x| x.starts_with("--gid=")), "{a:?}");
    let cwd = d.path().canonicalize().unwrap();
    assert!(a.contains(&format!("--working-directory={}", cwd.display())));
    let unit = a
        .iter()
        .find(|x| x.starts_with("--unit=clio-host-"))
        .unwrap();
    let props = after(&a, "-p");
    let ro = format!("TemporaryFileSystem={}:ro", mask.display());
    assert_eq!(
        props
            .iter()
            .filter(|p| p.starts_with("TemporaryFileSystem="))
            .collect::<Vec<_>>(),
        [&ro.as_str()]
    );
    assert!(props.contains(&"PrivateNetwork=yes"));
    assert!(props.contains(&"KillMode=control-group"));
    // A backstop a little past our own deadline, so ours is the one that reports the timeout.
    let max: u64 = props
        .iter()
        .find_map(|p| p.strip_prefix("RuntimeMaxSec="))
        .unwrap()
        .parse()
        .unwrap();
    assert!((90..=120).contains(&max), "{max}");
    let env: BTreeSet<&str> = after(&a, "-E")
        .into_iter()
        .map(|e| e.split('=').next().unwrap())
        .collect();
    assert_eq!(
        env,
        BTreeSet::from(["HOME", "LANG", "LOGNAME", "PATH", "USER", "XDG_RUNTIME_DIR"])
    );
    assert!(after(&a, "-E").contains(&format!("XDG_RUNTIME_DIR=/run/user/{u}").as_str()));
    assert!(a.ends_with(&["-lc".into(), "echo hi".into()]), "{a:?}");

    j.network = true;
    let (_, b) = h.args(&j).unwrap();
    assert!(!after(&b, "-p").contains(&"PrivateNetwork=yes"));
    // No time limit asked: no backstop either.
    j.timeout = None;
    let (_, c) = h.args(&j).unwrap();
    assert!(
        !after(&c, "-p")
            .iter()
            .any(|p| p.starts_with("RuntimeMaxSec")),
        "{c:?}"
    );
    let other = b.iter().find(|x| x.starts_with("--unit=")).unwrap();
    assert_ne!(unit, other, "every run gets its own unit");
}

#[test]
fn a_mask_or_folder_that_cannot_be_honoured_is_refused() {
    let d = TempDir::new().unwrap();
    for l in [Launcher::Bwrap, Launcher::Systemd] {
        let Some(h) = with(l) else { continue };
        let mut j = job(d.path(), "true");
        j.masks = vec!["relative/clio".into()];
        assert!(h.args(&j).is_err(), "{l:?}: a relative mask");
        let mut j = job(&d.path().join("nope"), "true");
        j.masks = vec![];
        assert!(h.args(&j).is_err(), "{l:?}: a missing working folder");
    }
    // `TemporaryFileSystem=` splits on spaces and colons: such a mask would not mask.
    let Some(h) = with(Launcher::Systemd) else {
        return;
    };
    let odd = d.path().join("a b");
    std::fs::create_dir(&odd).unwrap();
    let mut j = job(d.path(), "true");
    j.masks = vec![odd];
    assert!(h.args(&j).is_err());
}

/// `Bwrap` if it really runs here (it needs unprivileged user namespaces).
fn bwrap() -> Option<HostJail> {
    let h = with(Launcher::Bwrap)?;
    let d = TempDir::new().unwrap();
    match h.run(&job(d.path(), "true")) {
        Ok(o) if o.success() => Some(h),
        r => {
            eprintln!("SKIPPED (bwrap cannot start here, nothing was tested): {r:?}");
            None
        }
    }
}

fn marker() -> String {
    format!(
        "clio-host-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

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

/// Shared by both real tests: the self test, what a command sees, and a timeout that kills
/// everything the command started.
fn real(h: &HostJail) {
    let d = TempDir::new().unwrap();
    h.self_test(d.path()).unwrap();

    let (mask, _) = masked(&d);
    let mut j = job(
        d.path(),
        "pwd; ls -A clio | wc -l; echo \"${CARGO_PKG_NAME:-none} $HOME\"",
    );
    j.masks = vec![mask.clone()];
    let o = h.run(&j).unwrap();
    assert!(o.success(), "{o:?}");
    let cwd = d.path().canonicalize().unwrap();
    assert_eq!(
        o.stdout,
        format!("{}\n0\nnone {}\n", cwd.display(), d.path().display())
    );
    assert!(mask.join("decoy").is_file(), "the decoy is only hidden");

    let m = marker();
    let mut j = job(
        d.path(),
        &format!("(sleep 300; echo {m}) & sleep 300; echo {m}"),
    );
    j.timeout = Some(Duration::from_secs(2));
    let t = Instant::now();
    let o = h.run(&j).unwrap();
    assert!(o.timed_out && !o.success(), "{o:?}");
    assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    gone(&m, "the timeout");

    // No time limit: it runs until it is stopped, and stopping it kills all it started.
    let m = marker();
    let mut j = job(
        d.path(),
        &format!("(sleep 300; echo {m}) & sleep 300; echo {m}"),
    );
    j.timeout = None;
    let t = Instant::now();
    let o = h
        .run_until(&j, &|| t.elapsed() > Duration::from_millis(500))
        .unwrap();
    assert!(o.stopped && !o.timed_out && !o.success(), "{o:?}");
    assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    gone(&m, "a stop");
}

fn gone(m: &str, what: &str) {
    let until = Instant::now() + Duration::from_secs(5);
    while host_procs_with(m) > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(host_procs_with(m), 0, "a child outlived {what}");
}

#[test]
fn bwrap_for_real_hides_the_mask_cuts_the_network_on_request_and_kills_on_timeout() {
    let Some(h) = bwrap() else { return };
    real(&h);
}

#[test]
fn systemd_for_real_only_with_clio_test_sudo() {
    if std::env::var("CLIO_TEST_SUDO").as_deref() != Ok("1") {
        eprintln!("SKIPPED (set CLIO_TEST_SUDO=1 to run the systemd launcher with sudo)");
        return;
    }
    let h = HostJail::detect().unwrap();
    assert_eq!(h.launcher(), Launcher::Systemd);
    real(&h);
}

// A background job: its output reaches the sink as it comes, while it still runs, and a lot of
// it does not stop it (the sink keeps its own capped record).
#[test]
fn a_streamed_command_hands_over_its_output_as_it_comes_and_is_not_cut_for_volume() {
    let Some(h) = bwrap() else { return };
    let d = tempfile::tempdir().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let s = seen.clone();
    let sink: clio_node::Sink =
        std::sync::Arc::new(move |b: &[u8]| s.lock().unwrap().extend_from_slice(b));
    let mut j = job(
        d.path(),
        "echo first; head -c 3000000 /dev/zero | tr '\\0' x; echo; sleep 1; echo last",
    );
    j.max_output = 1024;
    let watcher = {
        let seen = seen.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(10) {
                if seen.lock().unwrap().starts_with(b"first") {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        })
    };
    let out = h.run_streaming(&j, &|| false, sink).unwrap();
    assert!(
        watcher.join().unwrap(),
        "the first line came before the end"
    );
    assert!(!out.flooded && out.exit_code == Some(0), "{out:?}");
    let all = seen.lock().unwrap();
    assert!(
        all.len() > 3_000_000 && all.ends_with(b"last\n"),
        "{}",
        all.len()
    );
}
