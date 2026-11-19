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
        shows: vec![],
        read_only: false,
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

/// A mask with a shown folder inside it (Clio's registered skills): `clio/skills/live`, with a
/// script, beside the decoy.
fn masked_with_window(d: &TempDir) -> (PathBuf, PathBuf) {
    let (mask, _) = masked(d);
    let live = mask.join("skills/live/demo");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::write(live.join("run.sh"), "echo from-the-skill\n").unwrap();
    (mask.clone(), mask.join("skills/live"))
}

// S-68: the skills a command may run are shown read-only inside Clio's masked folder, in both
// launchers' arguments, and never bound writable.
#[test]
fn a_shown_folder_is_a_read_only_window_inside_its_mask_in_both_launchers() {
    let d = TempDir::new().unwrap();
    let (mask, live) = masked_with_window(&d);
    let mut j = job(d.path(), "true");
    j.masks = vec![mask.clone()];
    j.shows = vec![live.clone(), d.path().join("missing")];
    let (m, w) = (mask.to_string_lossy(), live.to_string_lossy());
    if let Some(h) = with(Launcher::Bwrap) {
        let (_, a) = h.args(&j).unwrap();
        let at = |x: &str| a.iter().position(|y| y == x).unwrap();
        assert_eq!(after(&a, "--ro-bind"), [w.as_ref()]);
        assert_eq!(after(&a, "--bind"), ["/"], "{a:?}");
        // After the empty mask is made, before it becomes read-only.
        assert!(at("--tmpfs") < at("--ro-bind") && at("--ro-bind") < at("--remount-ro"));
        assert_eq!(after(&a, "--remount-ro"), [m.as_ref()]);
        assert!(!a.iter().any(|x| x.contains("missing")), "{a:?}");
    }
    if let Some(h) = with(Launcher::Systemd) {
        let (_, a) = h.args(&j).unwrap();
        let props = after(&a, "-p");
        assert!(
            props.contains(&format!("BindReadOnlyPaths={w}").as_str()),
            "{props:?}"
        );
        assert!(
            !props.iter().any(|p| p.starts_with("BindPaths=")),
            "{props:?}"
        );
    }
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

/// A folder that is not under `/tmp` (the build's own tmp folder): a review's or a service's run
/// has a `/tmp` of its own and would not see one there. `None` (and why) when the build is under
/// `/tmp` itself.
fn outside_tmp() -> Option<TempDir> {
    let d = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    if d.path().canonicalize().unwrap().starts_with("/tmp") {
        eprintln!(
            "SKIPPED (the build folder is under /tmp, which a run does not see; nothing was tested)"
        );
        return None;
    }
    Some(d)
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

// S-68: for real, with `bwrap`: a command reads and runs what is in the shown folder and can
// write, rename or delete none of it; the rest of the mask stays empty.
#[test]
fn bwrap_for_real_shows_the_window_read_only_and_hides_the_rest_of_the_mask() {
    let Some(h) = bwrap() else { return };
    let d = TempDir::new().unwrap();
    let (mask, live) = masked_with_window(&d);
    let w = live.display();
    let mut j = job(
        d.path(),
        &format!(
            "bash {w}/demo/run.sh; test -e {m}/decoy && echo SEEN; \
             echo x > {w}/demo/run.sh 2>/dev/null && echo WROTE; \
             touch {w}/demo/new 2>/dev/null && echo MADE; \
             mv {w}/demo {w}/moved 2>/dev/null && echo MOVED; \
             rm -f {w}/demo/run.sh 2>/dev/null; test -e {w}/demo/run.sh || echo DELETED; true",
            m = mask.display()
        ),
    );
    j.masks = vec![mask.clone()];
    j.shows = vec![live.clone()];
    let o = h.run(&j).unwrap();
    assert_eq!(o.stdout, "from-the-skill\n", "{o:?}");
    assert_eq!(
        std::fs::read_to_string(live.join("demo/run.sh")).unwrap(),
        "echo from-the-skill\n"
    );
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

// S-67: a review's command sees the machine read-only, a `/tmp` of its own, and gets no new
// privileges.
#[test]
fn a_read_only_job_writes_only_its_own_tmp() {
    for l in [Launcher::Bwrap, Launcher::Systemd] {
        let Some(h) = with(l) else { continue };
        let d = TempDir::new().unwrap();
        let mut j = job(d.path(), "true");
        j.read_only = true;
        let (_, a) = h.args(&j).unwrap();
        match l {
            Launcher::Bwrap => {
                assert_eq!(after(&a, "--ro-bind"), ["/"], "{a:?}");
                assert!(after(&a, "--bind").is_empty(), "{a:?}");
                assert!(after(&a, "--tmpfs").contains(&"/tmp"), "{a:?}");
                assert!(after(&a, "--tmpfs").contains(&"/run"), "{a:?}");
                assert_eq!(after(&a, "--dev"), ["/dev"], "{a:?}");
                for flag in ["--unshare-pid", "--unshare-ipc", "--unshare-net"] {
                    assert!(a.contains(&flag.to_owned()), "{flag} missing from {a:?}");
                }
            }
            Launcher::Systemd => {
                let props = after(&a, "-p");
                for p in [
                    "ReadOnlyPaths=/",
                    "PrivateTmp=yes",
                    "NoNewPrivileges=yes",
                    "TemporaryFileSystem=/run:ro",
                ] {
                    assert!(props.contains(&p), "{p} missing from {a:?}");
                }
                assert!(!props.contains(&"ReadWritePaths=/tmp"), "{a:?}");
            }
        }
    }
    let Some(h) = bwrap() else {
        return;
    };
    // A folder outside `/tmp` (the build's own) cannot be written; its own `/tmp` can, and what
    // it writes there is not in the host's.
    let Some(outside) = outside_tmp() else { return };
    let m = marker();
    let mut j = job(
        outside.path(),
        &format!("touch x; echo $? ; touch /tmp/{m}; echo $?"),
    );
    j.read_only = true;
    let o = h.run(&j).unwrap();
    assert_eq!(
        o.stdout.split_whitespace().collect::<Vec<_>>(),
        ["1", "0"],
        "{o:?}"
    );
    assert!(!outside.path().join("x").exists());
    assert!(!Path::new("/tmp").join(&m).exists(), "its /tmp is its own");
}

/// A Unix socket at `path`, listening, accepting in a thread of its own (this test's own socket).
fn listening(path: &Path) -> std::os::unix::net::UnixListener {
    let l = std::os::unix::net::UnixListener::bind(path).unwrap();
    let c = l.try_clone().unwrap();
    std::thread::spawn(move || {
        for s in c.incoming().flatten() {
            drop(s);
        }
    });
    l
}

/// Prints `<label> yes` when a Unix socket at `path` can be connected to, `<label> no` if not.
fn connects(path: &Path, label: &str) -> String {
    format!(
        "python3 -I -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])' {} >/dev/null 2>&1 && echo {label} yes || echo {label} no; ",
        path.display()
    )
}

// S-67, for real with `bwrap`: a review's command reaches no Unix socket (one in the host's /tmp,
// one in the home: the same escape as a terminal multiplexer's), reads nothing of Clio's data
// but the stage it is shown, and still runs a local command; outside the jail the same sockets
// do connect (the check itself works).
#[test]
fn bwrap_for_real_a_review_reaches_no_socket_and_reads_only_the_stage() {
    let Some(h) = bwrap() else { return };
    if !Path::new("/usr/bin/python3").is_file() {
        eprintln!("SKIPPED (no python3 to connect with; nothing was tested)");
        return;
    }
    let Some(home) = outside_tmp() else { return };
    let home_path = home.path().canonicalize().unwrap();
    let in_tmp = TempDir::new_in("/tmp").unwrap();
    let (a, b) = (in_tmp.path().join("s.sock"), home_path.join("s.sock"));
    let (_la, _lb) = (listening(&a), listening(&b));
    let data = home_path.join("data");
    let stage = data.join("reviews/1/stage");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::write(data.join("vault.bin"), "the vault").unwrap();
    std::fs::write(stage.join("SKILL.md"), "the stage\n").unwrap();
    let probe = format!(
        "{}{}cat {}/vault.bin >/dev/null 2>&1 && echo data yes || echo data no; cat {}/SKILL.md; echo $((6 * 7))",
        connects(&a, "tmp"),
        connects(&b, "home"),
        data.display(),
        stage.display(),
    );
    let outside = std::process::Command::new("bash")
        .args(["-c", &probe])
        .output()
        .unwrap();
    let outside = String::from_utf8_lossy(&outside.stdout);
    assert!(
        outside.contains("tmp yes") && outside.contains("home yes"),
        "the check connects outside the jail: {outside}"
    );
    let mut j = job(&home_path, &probe);
    j.read_only = true;
    j.masks = vec![data.clone()];
    j.shows = vec![stage.clone()];
    let o = h.run(&j).unwrap();
    assert_eq!(
        o.stdout.lines().collect::<Vec<_>>(),
        ["tmp no", "home no", "data no", "the stage", "42"],
        "{o:?}"
    );
}

// ---- services (stage 3, S-69) -----------------------------------------------------------------

use clio_node::host::ServiceRun;

/// A service's run in `d`: its files in `files` (cwd), Clio's data `data` masked with its state
/// folder `data/services/state/svc` writable inside, `out` declared, everything else read-only.
fn service(d: &Path, command: &str) -> ServiceRun {
    let data = d.join("data");
    let state = data.join("services/state/svc");
    let files = data.join("services/versions/svc/1");
    for p in [
        &state,
        &files,
        &data.join("services/state/other"),
        &d.join("out"),
        &d.join("home/.ssh"),
    ] {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(data.join("vault.bin"), "the vault").unwrap();
    std::fs::write(data.join("services/state/other/token"), "other's state").unwrap();
    std::fs::write(d.join("home/.ssh/id_ed25519"), "a key").unwrap();
    let c = |p: PathBuf| p.canonicalize().unwrap();
    ServiceRun {
        job: HostJob {
            command: command.into(),
            cwd: c(files.clone()),
            home: c(d.join("home")),
            network: false,
            timeout: Some(Duration::from_secs(20)),
            max_output: 64 * 1024,
            masks: vec![c(data), c(d.join("home/.ssh"))],
            shows: vec![c(files)],
            read_only: false,
        },
        writable: vec![c(state), c(d.join("out"))],
        cpu_percent: 50,
        memory_mb: 256,
        env: vec![(
            "CLIO_STATE_DIR".into(),
            c(d.join("data/services/state/svc")).display().to_string(),
        )],
    }
}

// S-69: a service's limits and its only writable paths are in both launchers' arguments; a run
// with a network of its own, no time limit or a writable path through a link is refused.
#[test]
fn a_service_run_has_its_limits_and_writes_only_its_paths_in_both_launchers() {
    let d = TempDir::new().unwrap();
    let s = service(d.path(), "true");
    let (state, out) = (
        s.writable[0].display().to_string(),
        s.writable[1].display().to_string(),
    );
    let mask = s.job.masks[0].display().to_string();
    let mut any = false;
    if let Some(h) = with(Launcher::Bwrap) {
        any = true;
        let (_, a) = h.service_args(&s, false).unwrap();
        assert_eq!(after(&a, "--ro-bind")[0], "/", "{a:?}");
        // Only the state folder (inside the mask) and the declared path, never `/`.
        assert_eq!(after(&a, "--bind"), [state.as_str(), out.as_str()], "{a:?}");
        assert!(
            after(&a, "--tmpfs").contains(&"/tmp"),
            "a private /tmp: {a:?}"
        );
        // Its /tmp and /dev/shm no larger than its memory.
        let mb = (256 * 1024 * 1024).to_string();
        assert_eq!(after(&a, "--size"), [mb.as_str(), mb.as_str()], "{a:?}");
        let i = a.iter().position(|x| x == "--tmpfs").unwrap();
        assert_eq!((a[i - 2].as_str(), a[i + 1].as_str()), ("--size", "/tmp"));
        assert!(after(&a, "--tmpfs").contains(&mask.as_str()));
        for flag in [
            "--unshare-net",
            "--unshare-pid",
            "--unshare-ipc",
            "--clearenv",
            "--die-with-parent",
        ] {
            assert!(a.contains(&flag.to_owned()), "{flag} missing from {a:?}");
        }
        assert!(!a.contains(&"--dev-bind".to_owned()), "{a:?}");
        // Per process: address space and CPU seconds (20 s at 50 %), at the lowest priority.
        let i = a
            .iter()
            .position(|x| x.ends_with("/prlimit"))
            .expect("prlimit");
        assert_eq!(a[i + 1], format!("--as={}", 256 * 1024 * 1024));
        assert_eq!(a[i + 2], "--cpu=10");
        assert_eq!(a[i + 3], "--nproc=256");
        assert!(
            a[i + 5].ends_with("/nice") && a[i + 6] == "-n" && a[i + 7] == "19",
            "{a:?}"
        );
        // `bash -c`: no login files.
        assert!(a.ends_with(&["-c".into(), "true".into()]), "{a:?}");
        assert!(after(&a, "--setenv").contains(&"CLIO_STATE_DIR"));

        // In a scope: `env XDG_RUNTIME_DIR=… systemd-run --user --scope …` with the cgroup's
        // limits, then the same jail, without the per-process limits.
        let scoped = h
            .clone()
            .with_scope(Some(PathBuf::from("/usr/bin/systemd-run")));
        let (prog, b) = scoped.service_args(&s, true).unwrap();
        assert!(prog.ends_with("env"), "{prog:?}");
        let cut = b.iter().position(|x| x == "--").unwrap();
        assert_eq!(
            b[..cut],
            [
                format!("XDG_RUNTIME_DIR=/run/user/{}", uid()),
                "/usr/bin/systemd-run".into(),
                "--user".into(),
                "--scope".into(),
                "--quiet".into(),
                "--collect".into(),
                "-p".into(),
                "MemoryMax=256M".into(),
                "-p".into(),
                "MemorySwapMax=0".into(),
                "-p".into(),
                "CPUQuota=50%".into(),
                "-p".into(),
                "TasksMax=256".into(),
            ]
        );
        assert!(b[cut + 1].ends_with("/bwrap"), "{b:?}");
        let (_, plain) = h.service_args(&s, false).unwrap();
        let jail = |v: &[String]| -> Vec<String> {
            v.iter()
                .filter(|x| {
                    !x.contains("prlimit")
                        && !x.starts_with("--as=")
                        && !x.starts_with("--cpu=")
                        && !x.starts_with("--nproc=")
                })
                .cloned()
                .collect()
        };
        // The same jail inside (the unit names and the order of everything but the limits).
        let inner = jail(&b[cut + 2..]);
        let outer = jail(&plain);
        assert_eq!(inner.len(), outer.len() - 1, "{inner:?} / {outer:?}");
        assert!(!b.iter().any(|x| x.contains("prlimit")), "{b:?}");
    }
    if let Some(h) = with(Launcher::Systemd) {
        any = true;
        let (_, a) = h.service_args(&s, false).unwrap();
        let props = after(&a, "-p");
        for p in [
            "ReadOnlyPaths=/".to_owned(),
            "PrivateTmp=yes".into(),
            "PrivateDevices=yes".into(),
            "NoNewPrivileges=yes".into(),
            "PrivateNetwork=yes".into(),
            "CPUQuota=50%".into(),
            "MemoryMax=256M".into(),
            "MemorySwapMax=0".into(),
            "RuntimeMaxSec=20".into(),
            format!("BindPaths={state}"),
            format!("ReadWritePaths={out}"),
            format!("TemporaryFileSystem={mask}:ro"),
        ] {
            assert!(props.contains(&p.as_str()), "{p} missing from {a:?}");
        }
        assert!(!props.contains(&"ReadWritePaths=/tmp"), "{a:?}");
        assert!(
            a.iter().any(|x| x.starts_with("--uid=")),
            "it runs as this user: {a:?}"
        );
    }
    if !any {
        return;
    }
    let h = with(Launcher::Bwrap)
        .or_else(|| with(Launcher::Systemd))
        .unwrap();
    let mut bad = s.clone();
    bad.job.network = true;
    assert!(h.service_args(&bad, false).is_err());
    let mut bad = s.clone();
    bad.job.timeout = None;
    assert!(h.service_args(&bad, false).is_err());
    let link = d.path().join("link");
    std::os::unix::fs::symlink(d.path().join("out"), &link).unwrap();
    let mut bad = s.clone();
    bad.writable.push(link);
    assert!(h.service_args(&bad, false).is_err());
}

// S-69, for real with `bwrap`: a run writes its state folder and its declared path and nothing
// else; it cannot read Clio's data, another service's state or ~/.ssh; it has no network and no
// new privileges; its /tmp is its own; its secret comes on its input; it ends at its time limit
// and cannot go past its memory.
#[test]
fn bwrap_for_real_a_service_run_stays_in_its_bounds() {
    let Some(h) = bwrap() else { return };
    // Not under /tmp: the run's /tmp is its own, empty.
    let (Some(d), Some(elsewhere)) = (outside_tmp(), outside_tmp()) else {
        return;
    };
    let m = marker();
    let cmd = format!(
        "IFS= read -r -d '' TOKEN || exit 75; export TOKEN; exec </dev/null; \
         t() {{ if eval \"$1\" >/dev/null 2>&1; then echo \"$2 yes\"; else echo \"$2 no\"; fi; }}; \
         t 'echo s > \"$CLIO_STATE_DIR/s\"' state; \
         t 'echo o > {out}/o' out; \
         t 'echo e > {else}/e' elsewhere; \
         t 'echo h > $HOME/h' home; \
         t 'echo {m} > /tmp/{m}' tmp; \
         t 'cat {data}/vault.bin' vault; \
         t 'cat {data}/services/state/other/token' other; \
         t 'cat {ssh}/id_ed25519' ssh; \
         t 'exec 3<>/dev/tcp/1.1.1.1/443' network; \
         grep -q 'NoNewPrivs:[[:space:]]*1' /proc/self/status && echo nnp yes; \
         echo \"token ${{#TOKEN}}\"",
        out = d.path().canonicalize().unwrap().join("out").display(),
        else = elsewhere.path().display(),
        data = d.path().canonicalize().unwrap().join("data").display(),
        ssh = d.path().canonicalize().unwrap().join("home/.ssh").display(),
    );
    let s = service(d.path(), &cmd);
    let (o, scoped) = h.run_service(&s, b"abc\0".to_vec(), &|| false).unwrap();
    assert!(!scoped, "no scope unless asked for");
    assert!(o.success(), "{o:?}");
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "state yes",
            "out yes",
            "elsewhere no",
            "home no",
            "tmp yes",
            "vault no",
            "other no",
            "ssh no",
            "network no",
            "nnp yes",
            "token 3"
        ],
        "{o:?}"
    );
    assert!(d.path().join("data/services/state/svc/s").is_file());
    assert!(d.path().join("out/o").is_file());
    assert!(!elsewhere.path().join("e").exists());
    assert!(!Path::new("/tmp").join(&m).exists(), "its /tmp is its own");

    // Its time limit: everything it started ends with it.
    let m = marker();
    let mut s = service(
        d.path(),
        &format!("(sleep 300; echo {m}) & sleep 300; echo {m}"),
    );
    s.job.timeout = Some(Duration::from_secs(2));
    let t = Instant::now();
    let (o, _) = h.run_service(&s, vec![], &|| false).unwrap();
    assert!(o.timed_out && !o.success(), "{o:?}");
    assert!(t.elapsed() < Duration::from_secs(15));
    gone(&m, "a service's time limit");

    // Its memory: 64 MB of address space cannot hold 200 MB.
    let mut s = service(
        d.path(),
        "python3 -c 'x = bytearray(200 * 1024 * 1024)' && echo allocated",
    );
    s.memory_mb = 64;
    let (o, _) = h.run_service(&s, vec![], &|| false).unwrap();
    assert!(!o.success() && !o.stdout.contains("allocated"), "{o:?}");
    // Nor 200 MB in its /tmp, which is memory too.
    let mut s = service(
        d.path(),
        "head -c 209715200 /dev/zero > /tmp/f && echo written; ls -l /tmp/f",
    );
    s.memory_mb = 64;
    let (o, _) = h.run_service(&s, vec![], &|| false).unwrap();
    assert!(!o.stdout.contains("written"), "{o:?}");
}

/// A stand-in for `systemd-run` in `d`: `ok` runs what follows `--` (as a scope would), else it
/// fails before running anything (as when no scope can be made).
fn fake_systemd_run(d: &Path, ok: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = d.join(if ok {
        "systemd-run-ok"
    } else {
        "systemd-run-fails"
    });
    let body = if ok {
        "#!/bin/sh\necho \"$@\" >> \"$0.log\"\nwhile [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n"
    } else {
        "#!/bin/sh\necho 'Failed to start transient scope unit' >&2\nexit 1\n"
    };
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

// S-69: a run goes in a scope when one starts (here a stand-in: this suite never starts a real
// scope), and takes the per-process limits by itself when it does not; it says which.
#[test]
fn a_service_falls_back_to_per_process_limits_when_no_scope_starts() {
    let Some(h) = bwrap() else { return };
    let Some(d) = outside_tmp() else { return };
    let s = service(
        d.path(),
        "echo ran; grep -c . /proc/self/limits >/dev/null && echo done",
    );
    let fails = h
        .clone()
        .with_scope(Some(fake_systemd_run(d.path(), false)));
    let (o, scoped) = fails.run_service(&s, vec![], &|| false).unwrap();
    assert!(o.success() && o.stdout.contains("ran"), "{o:?}");
    assert!(!scoped, "the scope did not start: per-process limits");
    let fake = fake_systemd_run(d.path(), true);
    let works = h.clone().with_scope(Some(fake.clone()));
    let (o, scoped) = works.run_service(&s, vec![], &|| false).unwrap();
    assert!(o.success() && o.stdout.contains("ran"), "{o:?}");
    assert!(scoped);
    // The probe and the run, each with the limits as cgroup properties.
    let log = std::fs::read_to_string(format!("{}.log", fake.display())).unwrap();
    assert_eq!(log.lines().count(), 2, "{log}");
    assert!(
        log.lines()
            .all(|l| l.starts_with("--user --scope --quiet --collect -p MemoryMax=256M")),
        "{log}"
    );
}
