//! The host launcher: ONE shell command on the person's real machine (`/` as it is, the network
//! only when asked), with Clio's own folders shown empty and read-only. Two ways, picked by
//! `detect`: `Systemd` runs it in a transient unit through `sudo -n systemd-run`, where `sudo`
//! works; `Bwrap` binds `/` onto itself, where `sudo` does not (`bwrap` sets `no_new_privs`).
//! Everything fails closed: if the launcher cannot be built, nothing runs, never unmasked.
//!
//! The masks are not a wall against the command: with `Systemd` it has `sudo`. They keep Clio's
//! folders out of an ordinary command's sight, not out of root's reach.

use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::jail::{JailOutput, Sink, bwrap_version, find_in_trusted, run_command};

/// systemd's own default for services; without it `bash` falls back to a `PATH` ending in `.`.
const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    Systemd,
    Bwrap,
}

#[derive(Debug, Clone)]
enum Tools {
    Systemd {
        sudo: PathBuf,
        run: PathBuf,
        ctl: PathBuf,
    },
    Bwrap(PathBuf),
}

#[derive(Debug, Clone)]
pub struct HostJail {
    tools: Tools,
    bash: PathBuf,
    kill: Option<PathBuf>,
    uid: u32,
    gid: u32,
    user: String,
}

/// One command to run on the host.
#[derive(Debug, Clone)]
pub struct HostJob {
    /// Goes to `bash -lc`.
    pub command: String,
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub network: bool,
    /// `None`: no time limit; it runs until it ends or is stopped (`run_until`).
    pub timeout: Option<Duration>,
    /// Bytes kept per stream (the tail).
    pub max_output: usize,
    /// Clio's folders: shown empty and read-only. Absolute; one that does not exist is skipped.
    pub masks: Vec<PathBuf>,
    /// Folders inside a mask shown as they are, read-only (Clio's registered skills): a command
    /// can read and run what is there and change none of it. One that does not exist is skipped.
    pub shows: Vec<PathBuf>,
    /// A review's command: everything read-only but `/tmp`, and no new privileges (no `sudo`).
    pub read_only: bool,
}

/// A service's run (stage 3, S-69): `job` (its command, its files' folder as `cwd`, masks, the
/// windows it may read and its time limit) with the whole machine read-only but `writable`, a
/// private `/tmp` and `/dev`, no new privileges and no network of its own: `job.network` must be
/// false, and its only way out is a proxy socket in one of `job.shows`. Limits: with `Systemd`,
/// `CPUQuota`, `MemoryMax` (no swap) and `RuntimeMaxSec`; with `Bwrap`, `prlimit --as` and
/// `--cpu` on each process and `nice -n 19` (no cgroup: per process, not for the whole run).
#[derive(Debug, Clone)]
pub struct ServiceRun {
    pub job: HostJob,
    /// Its state folder and the paths it declared: the only ones it may write. Absolute, existing.
    pub writable: Vec<PathBuf>,
    pub cpu_percent: u32,
    pub memory_mb: u64,
    /// Variables besides the launcher's own. Never a secret: they are in the arguments.
    pub env: Vec<(String, String)>,
}

fn unit_name() -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        "clio-host-{}-{}-{nanos}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    )
}

/// The argv that stops, without waiting, every unit the process `pid` started.
fn stop_units_args(ctl: &Path, pid: u32) -> Vec<String> {
    vec![
        "-n".into(),
        ctl.to_string_lossy().into_owned(),
        "stop".into(),
        "--no-block".into(),
        format!("clio-host-{pid}-*"),
    ]
}

/// Best effort, for whoever started the process `pid` once it is gone: its `Systemd` units
/// outlive it (killing a `sudo systemd-run` client does not stop its unit). Does nothing without
/// `sudo` and `systemctl` in the system folders.
pub fn stop_units_of(pid: u32) {
    if let (Some(sudo), Some(ctl)) = (find_in_trusted("sudo"), find_in_trusted("systemctl")) {
        let _ = quiet(Command::new(sudo).args(stop_units_args(&ctl, pid))).status();
    }
}

/// The pids of the `clio-host-<pid>-<n>-<nanos>` units in `listing` (`systemctl list-units
/// --plain` lines) for which `alive` says no, each once. Only a name of exactly that shape counts.
fn dead_unit_pids(listing: &str, alive: impl Fn(u32) -> bool) -> Vec<u32> {
    let mut pids: Vec<u32> = listing
        .lines()
        .filter_map(|l| {
            let name = l.split_whitespace().next()?;
            let rest = name.strip_suffix(".service")?.strip_prefix("clio-host-")?;
            let parts: Vec<&str> = rest.split('-').collect();
            let digits = |p: &&str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
            if parts.len() != 3 || !parts.iter().all(digits) {
                return None;
            }
            parts[0].parse::<u32>().ok().filter(|p| *p > 0)
        })
        .filter(|p| !alive(*p))
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// When a host-mode session starts: units a `clio` that died without stopping them (SIGKILL, a
/// crash) left running, with no time limit of their own, are stopped (`stop_units_of`). In a
/// thread of its own, best effort and silent: it never holds up or breaks the start.
// shortcut: a pid the system gave to another process since is taken as alive, so that `clio`'s
// units stay until the next start after it ends; checking it is a `clio` (its start time against
// the unit's) would catch those too.
pub fn stop_leftover_units() {
    let Some(ctl) = find_in_trusted("systemctl") else {
        return;
    };
    std::thread::spawn(move || {
        let listed = quiet(Command::new(ctl).args([
            "list-units",
            "--all",
            "--plain",
            "--no-legend",
            "--no-pager",
            "--type=service",
            "clio-host-*",
        ]))
        .stdout(Stdio::piped())
        .output();
        let Ok(o) = listed else { return };
        let alive = |pid: u32| Path::new(&format!("/proc/{pid}")).exists();
        for pid in dead_unit_pids(&String::from_utf8_lossy(&o.stdout), alive) {
            stop_units_of(pid);
        }
    });
}

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

impl HostJail {
    /// `Systemd` when `sudo -n true` works and `systemd-run` exists, else `Bwrap`.
    pub fn detect() -> Result<Self, String> {
        let sudo = find_in_trusted("sudo").is_some_and(|s| {
            quiet(Command::new(s).args(["-n", "true"]))
                .status()
                .is_ok_and(|st| st.success())
        });
        if sudo && let Ok(h) = Self::with(Launcher::Systemd) {
            return Ok(h);
        }
        Self::with(Launcher::Bwrap).map_err(|e| {
            format!("no host launcher: no passwordless sudo with systemd-run, and {e}")
        })
    }

    /// The given launcher, if its programs are in the system folders. Runs nothing with `sudo`.
    pub fn with(launcher: Launcher) -> Result<Self, String> {
        if !cfg!(target_os = "linux") {
            return Err("the host launcher exists only on Linux".into());
        }
        let need = |n: &str| {
            find_in_trusted(n)
                .ok_or_else(|| format!("{n} is not installed in /usr/bin, /bin or /usr/local/bin"))
        };
        let tools = match launcher {
            Launcher::Systemd => Tools::Systemd {
                sudo: need("sudo")?,
                run: need("systemd-run")?,
                ctl: need("systemctl")?,
            },
            Launcher::Bwrap => {
                let bwrap = need("bwrap")?;
                bwrap_version(&bwrap)?;
                Tools::Bwrap(bwrap)
            }
        };
        let me = std::fs::metadata("/proc/self").map_err(|e| format!("/proc/self: {e}"))?;
        let (uid, gid) = (me.uid(), me.gid());
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let user = passwd
            .lines()
            .map(|l| l.split(':').collect::<Vec<_>>())
            .find(|f| f.get(2) == Some(&uid.to_string().as_str()))
            .map(|f| f[0].to_owned())
            .ok_or_else(|| format!("uid {uid} is not in /etc/passwd"))?;
        Ok(Self {
            tools,
            bash: need("bash")?,
            kill: find_in_trusted("kill"),
            uid,
            gid,
            user,
        })
    }

    pub fn launcher(&self) -> Launcher {
        match self.tools {
            Tools::Systemd { .. } => Launcher::Systemd,
            Tools::Bwrap(_) => Launcher::Bwrap,
        }
    }

    /// The program and argv that would run `job`. Public so tests (and a reviewer) can read them.
    pub fn args(&self, job: &HostJob) -> Result<(PathBuf, Vec<String>), String> {
        self.build(job, &unit_name(), None)
    }

    /// The program and argv that would run a service's `run`.
    pub fn service_args(&self, run: &ServiceRun) -> Result<(PathBuf, Vec<String>), String> {
        self.build(&run.job, &unit_name(), Some(run))
    }

    fn build(
        &self,
        job: &HostJob,
        unit: &str,
        svc: Option<&ServiceRun>,
    ) -> Result<(PathBuf, Vec<String>), String> {
        if let Some(s) = svc {
            if job.network || job.timeout.is_none() || job.read_only {
                return Err("a service runs with no network of its own and a time limit".into());
            }
            if s.cpu_percent == 0 || s.memory_mb == 0 {
                return Err("a service needs a processor and a memory limit".into());
            }
        }
        let cwd = job
            .cwd
            .canonicalize()
            .map_err(|e| format!("working folder {}: {e}", job.cwd.display()))?;
        if !cwd.is_dir() {
            return Err(format!("{} is not a folder", cwd.display()));
        }
        let cwd = cwd.to_string_lossy().into_owned();
        let canon = |list: &[PathBuf], what: &str| -> Result<Vec<String>, String> {
            let mut out = Vec::new();
            for m in list {
                if !m.is_absolute() {
                    return Err(format!("{what} {} is not an absolute path", m.display()));
                }
                match m.canonicalize() {
                    Ok(c) => out.push(c.to_string_lossy().into_owned()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("{what} {}: {e}", m.display())),
                }
            }
            Ok(out)
        };
        let masks = canon(&job.masks, "mask")?;
        let shows = canon(&job.shows, "shown folder")?;
        // A service's writable paths: each must exist and be named as it is (no link in it).
        let mut writable = Vec::new();
        for w in svc.map_or(&[][..], |s| &s.writable[..]) {
            let c = w
                .canonicalize()
                .map_err(|e| format!("writable path {}: {e}", w.display()))?;
            if !w.is_absolute() || c != *w {
                return Err(format!(
                    "writable path {} is not absolute or goes through a link",
                    w.display()
                ));
            }
            writable.push(c.to_string_lossy().into_owned());
        }
        let inside = |w: &str| masks.iter().any(|m| Path::new(w).starts_with(m));
        let mut env: Vec<(String, String)> = [
            ("HOME", job.home.to_string_lossy().into_owned()),
            ("USER", self.user.clone()),
            ("LOGNAME", self.user.clone()),
            ("LANG", "C.UTF-8".into()),
            ("PATH", PATH.into()),
            ("XDG_RUNTIME_DIR", format!("/run/user/{}", self.uid)),
        ]
        .map(|(k, v)| (k.to_owned(), v))
        .to_vec();
        env.extend(svc.iter().flat_map(|s| s.env.iter().cloned()));
        let mut shell = vec![
            self.bash.to_string_lossy().into_owned(),
            "-lc".into(),
            job.command.clone(),
        ];
        // Bwrap has no cgroup of its own: each process gets the limits, and the lowest priority.
        if let (Some(s), Tools::Bwrap(_)) = (svc, &self.tools) {
            let need = |n: &str| {
                find_in_trusted(n).ok_or_else(|| {
                    format!("{n} is not installed: a service's limits cannot be enforced")
                })
            };
            let secs = job.timeout.map_or(0, |t| t.as_secs());
            let cpu = (secs * u64::from(s.cpu_percent)).div_ceil(100).max(1);
            let mut pre = vec![
                need("prlimit")?.to_string_lossy().into_owned(),
                format!("--as={}", s.memory_mb * 1024 * 1024),
                format!("--cpu={cpu}"),
                "--".into(),
                need("nice")?.to_string_lossy().into_owned(),
                "-n".into(),
                "19".into(),
            ];
            pre.append(&mut shell);
            shell = pre;
        }
        let mut a: Vec<String> = Vec::new();
        let program = match &self.tools {
            Tools::Bwrap(bwrap) => {
                // Its own pid namespace: when its first process dies, everything it started dies.
                let root = if job.read_only || svc.is_some() {
                    "--ro-bind"
                } else {
                    "--bind"
                };
                a.extend([root, "/", "/"].map(String::from));
                if job.read_only {
                    a.extend(["--bind", "/tmp", "/tmp"].map(String::from));
                }
                // A service: a /tmp and a /dev of its own (the host's /dev/shm is writable).
                if svc.is_some() {
                    a.extend(
                        ["--tmpfs", "/tmp", "--dev", "/dev", "--unshare-ipc"].map(String::from),
                    );
                } else {
                    a.extend(["--dev-bind", "/dev", "/dev"].map(String::from));
                }
                a.extend(
                    [
                        "--unshare-pid",
                        "--proc",
                        "/proc",
                        "--die-with-parent",
                        "--new-session",
                        "--clearenv",
                    ]
                    .map(String::from),
                );
                if !job.network {
                    a.push("--unshare-net".into());
                }
                for m in &masks {
                    a.extend(["--tmpfs".into(), m.clone()]);
                    // A window into the mask, made while the mask can still take its mount point.
                    for w in shows.iter().filter(|w| Path::new(w).starts_with(m)) {
                        a.extend(["--ro-bind".into(), w.clone(), w.clone()]);
                    }
                    for w in writable.iter().filter(|w| Path::new(w).starts_with(m)) {
                        a.extend(["--bind".into(), w.clone(), w.clone()]);
                    }
                    a.extend(["--remount-ro".into(), m.clone()]);
                }
                for w in writable.iter().filter(|w| !inside(w)) {
                    a.extend(["--bind".into(), w.clone(), w.clone()]);
                }
                for (k, v) in env {
                    a.extend(["--setenv".into(), k, v]);
                }
                a.extend(["--chdir".into(), cwd, "--".into()]);
                bwrap.clone()
            }
            Tools::Systemd { sudo, run, .. } => {
                // `TemporaryFileSystem=` splits on spaces and colons and unquotes: such a path
                // would not be masked as written.
                if let Some(m) = masks.iter().chain(&shows).chain(&writable).find(|m| {
                    m.chars().any(|c| {
                        c.is_whitespace() || c.is_control() || matches!(c, ':' | '"' | '\'' | '\\')
                    })
                }) {
                    return Err(format!("mask {m} cannot be passed to systemd safely"));
                }
                a.extend([
                    "-n".into(),
                    run.to_string_lossy().into_owned(),
                    "--quiet".into(),
                    "--wait".into(),
                    "--pipe".into(),
                    "--collect".into(),
                    format!("--unit={unit}"),
                    format!("--uid={}", self.uid),
                    format!("--gid={}", self.gid),
                    format!("--working-directory={cwd}"),
                    "-p".into(),
                    "KillMode=control-group".into(),
                ]);
                // A backstop past our own deadline (which kills first and reports it), for when
                // this process dies and nobody stops the unit. No limit asked: none here either;
                // whoever started it stops its units (`stop_units_of`).
                if let Some(t) = job.timeout {
                    // A service's limit is the unit's own: it does not outlive it.
                    let slack = if svc.is_some() { 0 } else { 10 };
                    a.extend([
                        "-p".into(),
                        format!("RuntimeMaxSec={}", t.as_secs() + slack),
                    ]);
                }
                if let Some(s) = svc {
                    for p in [
                        "ReadOnlyPaths=/".to_owned(),
                        "PrivateTmp=yes".into(),
                        "PrivateDevices=yes".into(),
                        "NoNewPrivileges=yes".into(),
                        format!("CPUQuota={}%", s.cpu_percent),
                        format!("MemoryMax={}M", s.memory_mb),
                        "MemorySwapMax=0".into(),
                    ] {
                        a.extend(["-p".into(), p]);
                    }
                    for w in &writable {
                        let key = if inside(w) {
                            "BindPaths"
                        } else {
                            "ReadWritePaths"
                        };
                        a.extend(["-p".into(), format!("{key}={w}")]);
                    }
                }
                if !job.network {
                    a.extend(["-p".into(), "PrivateNetwork=yes".into()]);
                }
                if job.read_only {
                    for p in [
                        "ReadOnlyPaths=/",
                        "ReadWritePaths=/tmp",
                        "NoNewPrivileges=yes",
                    ] {
                        a.extend(["-p".into(), p.into()]);
                    }
                }
                for m in &masks {
                    a.extend(["-p".into(), format!("TemporaryFileSystem={m}:ro")]);
                }
                // systemd mounts these after the temporary file systems: a window into a mask.
                for w in shows
                    .iter()
                    .filter(|w| masks.iter().any(|m| Path::new(w).starts_with(m)))
                {
                    a.extend(["-p".into(), format!("BindReadOnlyPaths={w}")]);
                }
                for (k, v) in env {
                    a.extend(["-E".into(), format!("{k}={v}")]);
                }
                a.push("--".into());
                sudo.clone()
            }
        };
        a.extend(shell);
        Ok((program, a))
    }

    /// Runs the job and waits for it. `Err` means the launcher could not be built or started;
    /// a command that fails or is killed is an `Ok` with its outcome.
    pub fn run(&self, job: &HostJob) -> Result<JailOutput, String> {
        self.run_until(job, &|| false)
    }

    /// `run`, but stopped as soon as `cancel` says so (asked every 10 ms): the process group is
    /// killed and, with `Systemd`, the unit stopped. The output then says `stopped`.
    pub fn run_until(
        &self,
        job: &HostJob,
        cancel: &dyn Fn() -> bool,
    ) -> Result<JailOutput, String> {
        self.run_inner(job, cancel, None)
    }

    /// `run_until`, and every piece of output also goes to `sink` as it comes (a background
    /// job's file). No flood stop: the sink keeps its own capped record.
    pub fn run_streaming(
        &self,
        job: &HostJob,
        cancel: &dyn Fn() -> bool,
        sink: Sink,
    ) -> Result<JailOutput, String> {
        self.run_inner(job, cancel, Some(&sink))
    }

    /// Runs a service's `run` and waits for it; `input` is its standard input (the secrets it
    /// reads into its variables: never an argument, never a file).
    pub fn run_service(
        &self,
        run: &ServiceRun,
        input: Vec<u8>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<JailOutput, String> {
        self.run_inner_with(&run.job, cancel, None, Some(run), Some(input))
    }

    fn run_inner(
        &self,
        job: &HostJob,
        cancel: &dyn Fn() -> bool,
        sink: Option<&Sink>,
    ) -> Result<JailOutput, String> {
        self.run_inner_with(job, cancel, sink, None, None)
    }

    fn run_inner_with(
        &self,
        job: &HostJob,
        cancel: &dyn Fn() -> bool,
        sink: Option<&Sink>,
        svc: Option<&ServiceRun>,
        input: Option<Vec<u8>>,
    ) -> Result<JailOutput, String> {
        let unit = unit_name();
        let (program, args) = self.build(job, &unit, svc)?;
        // Killing the `sudo` client does not stop the unit: systemd has to.
        let stop = match &self.tools {
            Tools::Systemd { sudo, ctl, .. } => vec![
                sudo.to_string_lossy().into_owned(),
                "-n".into(),
                ctl.to_string_lossy().into_owned(),
                "stop".into(),
                format!("{unit}.service"),
            ],
            Tools::Bwrap(_) => vec![],
        };
        let mut cmd = Command::new(program);
        cmd.args(args);
        run_command(
            cmd,
            job.timeout,
            job.max_output,
            self.kill.as_deref(),
            (!stop.is_empty()).then_some(&stop[..]),
            cancel,
            sink,
            input,
        )
    }

    /// Proves, inside `scratch`, that a masked folder is empty and read-only, that the network
    /// is cut unless asked for, and (with `Systemd`) that `sudo` works. An error means no `exec`.
    pub fn self_test(&self, scratch: &Path) -> Result<(), String> {
        let mask = scratch.join("clio-mask");
        // Private like everything under Clio's data, whatever the umask (S-23).
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&mask)
            .map_err(|e| format!("{}: {e}", mask.display()))?;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(mask.join("decoy"))
            .and_then(|mut f| std::io::Write::write_all(&mut f, b"decoy"))
            .map_err(|e| format!("decoy: {e}"))?;
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("listen: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("listen: {e}"))?
            .port();
        let sudo = if self.launcher() == Launcher::Systemd {
            "sudo -n true || exit 13; "
        } else {
            ""
        };
        let command = format!(
            "test -e clio-mask/decoy && exit 10; touch clio-mask/new 2>/dev/null && exit 11; {sudo}\
             (echo > /dev/tcp/127.0.0.1/{port}) 2>/dev/null && exit 20; exit 21"
        );
        for network in [false, true] {
            let out = self.run(&HostJob {
                command: command.clone(),
                cwd: scratch.to_path_buf(),
                home: scratch.to_path_buf(),
                network,
                timeout: Some(Duration::from_secs(20)),
                max_output: 4096,
                masks: vec![mask.clone()],
                shows: vec![],
                read_only: false,
            })?;
            let why = match (out.exit_code, network) {
                (Some(21), false) | (Some(20), true) => continue,
                (Some(10), _) => "a masked folder is visible",
                (Some(11), _) => "a masked folder is writable",
                (Some(13), _) => "sudo does not work inside",
                (Some(20), false) => "the network is not cut",
                (Some(21), true) => "the network is not reachable when asked for",
                _ => "the launcher did not run the probe",
            };
            let detail = out.stderr.lines().next().unwrap_or("").trim().to_owned();
            return Err(format!("host launcher self test: {why} ({detail})"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_units_of_a_process_are_stopped_by_its_pid() {
        let a = super::stop_units_args(std::path::Path::new("/usr/bin/systemctl"), 42);
        assert_eq!(
            a,
            [
                "-n",
                "/usr/bin/systemctl",
                "stop",
                "--no-block",
                "clio-host-42-*"
            ]
        );
        // The pattern covers the names `run` gives (`clio-host-<pid>-<n>-<nanos>`).
        assert!(super::unit_name().starts_with(&format!("clio-host-{}-", std::process::id())));
    }

    #[test]
    fn leftover_units_are_found_only_for_dead_pids_and_only_by_their_exact_name() {
        let own = format!("{}.service", super::unit_name());
        let listing = format!(
            "clio-host-42-0-123.service loaded active running /bin/bash -lc sleep\n\
             clio-host-42-1-456.service loaded active running x\n\
             clio-host-7-0-1.service loaded active running x\n\
             {own} loaded active running x\n\
             clio-host-9-0-1.service.bak loaded active running x\n\
             clio-host-9-x-1.service loaded active running x\n\
             clio-host-9-0.service loaded active running x\n\
             clio-host--0-1.service loaded active running x\n\
             clio-host-0-0-1.service loaded active running x\n\
             evil;clio-host-9-0-1.service loaded active running x\n\
             clio-host-9-0-1-2.service loaded active running x\n\
             ssh.service loaded active running x\n"
        );
        let me = std::process::id();
        // 42 is dead, 7 alive (a pid given to another process since counts as alive).
        let alive = |p: u32| p == 7 || p == me;
        assert_eq!(super::dead_unit_pids(&listing, alive), [42]);
        assert!(super::dead_unit_pids("", alive).is_empty());
    }
}
