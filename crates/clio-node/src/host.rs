//! The host launcher: ONE shell command on the person's real machine (`/` as it is, the network
//! only when asked), with Clio's own folders shown empty and read-only. Two ways, picked by
//! `detect`: `Systemd` runs it in a transient unit through `sudo -n systemd-run`, where `sudo`
//! works; `Bwrap` binds `/` onto itself, where `sudo` does not (`bwrap` sets `no_new_privs`).
//! Everything fails closed: if the launcher cannot be built, nothing runs, never unmasked.
//!
//! The masks are not a wall against the command: with `Systemd` it has `sudo`. They keep Clio's
//! folders out of an ordinary command's sight, not out of root's reach.

use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
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
    /// `systemd-run`, for a service's run in a transient user scope (`with_scope`); `None`: never.
    scope: Option<PathBuf>,
}

/// The most processes a service's run may have at once.
pub const SERVICE_TASKS: u32 = 256;

/// One command to run on the host.
#[derive(Debug, Clone)]
pub struct HostJob {
    /// Goes to `bash -lc` (a service's to `bash -c`: no login files).
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
    /// A review's command: everything read-only, a `/tmp` and `/dev` of its own, `/run` hidden,
    /// no new privileges (no `sudo`) and, with `Bwrap`, no Unix socket of its own (a seccomp
    /// filter refuses `socket(AF_UNIX, ...)`): a socket anywhere it can see cannot be connected to.
    pub read_only: bool,
}

/// A service's run (stage 3, S-69): `job` (its command, its files' folder as `cwd`, masks, the
/// windows it may read and its time limit) with the whole machine read-only but `writable`, a
/// private `/tmp` (and `/dev/shm`) the size of its memory and a `/dev` of its own, no new
/// privileges and no network of its own: `job.network` must be false, and its only way out is a
/// proxy socket in one of `job.shows`. Limits: with `Systemd`, `CPUQuota`, `MemoryMax` (no swap)
/// and `RuntimeMaxSec`; with `Bwrap`, inside a transient user scope when one starts
/// (`with_scope`: `MemoryMax`, `MemorySwapMax=0`, `CPUQuota`, `TasksMax`, cgroup limits for the
/// whole run), else `prlimit --as`, `--cpu` and `--nproc` on each process (per process, not for
/// the whole run); always `nice -n 19`.
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
            scope: None,
        })
    }

    /// Services' runs (with `Bwrap`) go in a transient user scope through `systemd_run`, when one
    /// starts; `with_user_scopes` finds it in the system folders. Nothing else uses it.
    #[must_use]
    pub fn with_scope(mut self, systemd_run: Option<PathBuf>) -> Self {
        self.scope = systemd_run;
        self
    }

    /// The `systemd-run` services' runs go through, if any (`with_scope`).
    pub fn scope(&self) -> Option<&Path> {
        self.scope.as_deref()
    }

    /// `with_scope` with the system's `systemd-run`, if it is installed.
    #[must_use]
    pub fn with_user_scopes(self) -> Self {
        self.with_scope(find_in_trusted("systemd-run"))
    }

    /// The program and arguments that put what follows them in a transient user scope with the
    /// service's limits: `env XDG_RUNTIME_DIR=… systemd-run --user --scope … --`. `None` without
    /// a `systemd-run` or `env`. The person's own user manager: no `sudo`, no unit file.
    pub fn scope_prefix(&self, run: &ServiceRun) -> Option<(PathBuf, Vec<String>)> {
        let systemd_run = self.scope.as_ref()?;
        let env = find_in_trusted("env")?;
        let mut a = vec![
            // The user manager's bus; nothing else of the caller's (the environment is cleared).
            format!("XDG_RUNTIME_DIR=/run/user/{}", self.uid),
            systemd_run.to_string_lossy().into_owned(),
            "--user".into(),
            "--scope".into(),
            "--quiet".into(),
            "--collect".into(),
        ];
        for p in [
            format!("MemoryMax={}M", run.memory_mb),
            "MemorySwapMax=0".into(),
            format!("CPUQuota={}%", run.cpu_percent),
            format!("TasksMax={SERVICE_TASKS}"),
        ] {
            a.extend(["-p".into(), p]);
        }
        a.push("--".into());
        Some((env, a))
    }

    /// Whether a scope with `run`'s limits starts now: `true` runs in it. Any failure (no user
    /// manager, no bus, a refused property) is `false`, and the run takes the per-process limits.
    fn scope_starts(&self, run: &ServiceRun) -> bool {
        let (Tools::Bwrap(_), Some((env, mut a)), Some(t)) =
            (&self.tools, self.scope_prefix(run), find_in_trusted("true"))
        else {
            return false;
        };
        a.push(t.to_string_lossy().into_owned());
        quiet(Command::new(env).args(a))
            .status()
            .is_ok_and(|s| s.success())
    }

    pub fn launcher(&self) -> Launcher {
        match self.tools {
            Tools::Systemd { .. } => Launcher::Systemd,
            Tools::Bwrap(_) => Launcher::Bwrap,
        }
    }

    /// The program and argv that would run `job`. Public so tests (and a reviewer) can read them.
    pub fn args(&self, job: &HostJob) -> Result<(PathBuf, Vec<String>), String> {
        self.build(job, &unit_name(), None, false)
    }

    /// The program and argv that would run a service's `run`; `scoped`: in a transient scope.
    pub fn service_args(
        &self,
        run: &ServiceRun,
        scoped: bool,
    ) -> Result<(PathBuf, Vec<String>), String> {
        self.build(&run.job, &unit_name(), Some(run), scoped)
    }

    fn build(
        &self,
        job: &HostJob,
        unit: &str,
        svc: Option<&ServiceRun>,
        scoped: bool,
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
        let mut asked = job.masks.clone();
        // A review's command: `/run` (the person's D-Bus, sockets of what starts programs) too.
        if job.read_only {
            asked.push(PathBuf::from("/run"));
        }
        let mut masks = canon(&asked, "mask")?;
        // A mask inside another is hidden by it already (Clio's data inside a masked home).
        masks.sort();
        masks.dedup();
        let all = masks.clone();
        masks.retain(|m| !all.iter().any(|o| o != m && Path::new(m).starts_with(o)));
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
            if svc.is_some() { "-c" } else { "-lc" }.into(),
            job.command.clone(),
        ];
        // Bwrap has no cgroup of its own: in a scope the cgroup holds the limits; without one each
        // process gets them. Always the lowest priority.
        if let (Some(s), Tools::Bwrap(_)) = (svc, &self.tools) {
            let need = |n: &str| {
                find_in_trusted(n).ok_or_else(|| {
                    format!("{n} is not installed: a service's limits cannot be enforced")
                })
            };
            let mut pre = vec![];
            if !scoped {
                let secs = job.timeout.map_or(0, |t| t.as_secs());
                let cpu = (secs * u64::from(s.cpu_percent)).div_ceil(100).max(1);
                pre.extend([
                    need("prlimit")?.to_string_lossy().into_owned(),
                    format!("--as={}", s.memory_mb * 1024 * 1024),
                    format!("--cpu={cpu}"),
                    format!("--nproc={SERVICE_TASKS}"),
                    "--".into(),
                ]);
            }
            pre.extend([
                need("nice")?.to_string_lossy().into_owned(),
                "-n".into(),
                "19".into(),
            ]);
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
                // A service or a review: a /tmp and a /dev of its own (the host's /tmp holds
                // other programs' sockets, its /dev/shm is writable). A service's are no larger
                // than its memory.
                let size = svc.map(|s| (s.memory_mb * 1024 * 1024).to_string());
                if svc.is_some() || job.read_only {
                    if let Some(n) = &size {
                        a.extend(["--size".into(), n.clone()]);
                    }
                    a.extend(["--tmpfs", "/tmp", "--dev", "/dev"].map(String::from));
                    if let Some(n) = &size {
                        a.extend(["--size".into(), n.clone()]);
                        a.extend(["--tmpfs", "/dev/shm"].map(String::from));
                    }
                    a.push("--unshare-ipc".into());
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
                        "PrivateTmp=yes",
                        "PrivateDevices=yes",
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
        // In the scope, the jail exactly as it is.
        if let (true, Some(s), Tools::Bwrap(_)) = (scoped, svc, &self.tools) {
            let (env, mut pre) = self
                .scope_prefix(s)
                .ok_or("a scope needs systemd-run and env")?;
            pre.push(program.to_string_lossy().into_owned());
            pre.append(&mut a);
            return Ok((env, pre));
        }
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
    /// reads into its variables: never an argument, never a file). With it, whether its limits
    /// were a cgroup's (a scope started) or each process's.
    pub fn run_service(
        &self,
        run: &ServiceRun,
        input: Vec<u8>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(JailOutput, bool), String> {
        let scoped = self.scope_starts(run);
        self.run_inner_with(&run.job, cancel, None, Some(run), Some(input), scoped)
            .map(|o| (o, scoped))
    }

    /// Starts a service's `run` and does not wait for it (an MCP server, stage 4): the same jail
    /// and limits as `run_service`, its standard input, output and error piped, in a process
    /// group of its own, with an empty environment. The caller writes the secrets first on its
    /// input (as `run_service` does), talks to it, and kills it (`kill_group`); `job.timeout` only
    /// sizes the per-process processor limit. With it, whether its limits were a cgroup's.
    pub fn spawn_service(&self, run: &ServiceRun) -> Result<(std::process::Child, bool), String> {
        let scoped = self.scope_starts(run);
        let (program, args) = self.build(&run.job, &unit_name(), Some(run), scoped)?;
        Command::new(&program)
            .args(args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map(|c| (c, scoped))
            .map_err(|e| format!("could not start {}: {e}", program.display()))
    }

    fn run_inner(
        &self,
        job: &HostJob,
        cancel: &dyn Fn() -> bool,
        sink: Option<&Sink>,
    ) -> Result<JailOutput, String> {
        self.run_inner_with(job, cancel, sink, None, None, false)
    }

    fn run_inner_with(
        &self,
        job: &HostJob,
        cancel: &dyn Fn() -> bool,
        sink: Option<&Sink>,
        svc: Option<&ServiceRun>,
        input: Option<Vec<u8>>,
        scoped: bool,
    ) -> Result<JailOutput, String> {
        let unit = unit_name();
        let (program, mut args) = self.build(job, &unit, svc, scoped)?;
        // A review's command under bwrap: its seccomp filter, on a descriptor bwrap inherits. Kept
        // open until it has started; another thread's child may inherit it too (a few bytes of
        // filter, nothing else).
        let _filter = match (&self.tools, job.read_only) {
            (Tools::Bwrap(_), true) => {
                let f = seccomp_fd()?;
                args.splice(
                    0..0,
                    [
                        "--seccomp".into(),
                        std::os::fd::AsRawFd::as_raw_fd(&f).to_string(),
                    ],
                );
                Some(f)
            }
            _ => None,
        };
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

/// A seccomp program (classic BPF, as `bwrap --seccomp` reads it) that refuses
/// `socket(AF_UNIX, ...)` with `EACCES` and allows every other call; a call of another
/// architecture (or x32) is refused or kills. `None` on an architecture it does not know.
pub fn no_unix_sockets() -> Option<Vec<u8>> {
    let (arch, socket): (u32, u32) = if cfg!(target_arch = "x86_64") {
        (0xC000_003E, 41)
    } else if cfg!(target_arch = "aarch64") {
        (0xC000_00B7, 198)
    } else {
        return None;
    };
    // The low half of the first argument in `struct seccomp_data` (nr, arch, ip, args[6]).
    let arg0 = if cfg!(target_endian = "little") {
        16
    } else {
        20
    };
    let (ld, jeq, jge, ret) = (0x20u16, 0x15u16, 0x35u16, 0x06u16);
    let program: [(u16, u8, u8, u32); 10] = [
        (ld, 0, 0, 4),                 // the architecture
        (jeq, 1, 0, arch),             // ours: on
        (ret, 0, 0, 0x8000_0000),      // another: kill the process
        (ld, 0, 0, 0),                 // the call's number
        (jge, 4, 0, 0x4000_0000),      // x32: refused
        (jeq, 0, 2, socket),           // not socket(): allowed
        (ld, 0, 0, arg0),              // its domain
        (jeq, 1, 0, 1),                // AF_UNIX: refused
        (ret, 0, 0, 0x7fff_0000),      // allow
        (ret, 0, 0, 0x0005_0000 | 13), // EACCES
    ];
    Some(
        program
            .iter()
            .flat_map(|(c, t, f, k)| {
                let mut b = c.to_ne_bytes().to_vec();
                b.extend([*t, *f]);
                b.extend(k.to_ne_bytes());
                b
            })
            .collect(),
    )
}

/// `no_unix_sockets` in a memory file at its start, open WITHOUT close-on-exec (bwrap reads it).
#[cfg(target_os = "linux")]
fn seccomp_fd() -> Result<std::fs::File, String> {
    use std::io::{Seek, Write};
    let program = no_unix_sockets()
        .ok_or("a review's commands need a seccomp filter this architecture does not have")?;
    let fd = rustix::fs::memfd_create("clio-seccomp", rustix::fs::MemfdFlags::empty())
        .map_err(|e| format!("seccomp filter: {e}"))?;
    let mut f = std::fs::File::from(fd);
    f.write_all(&program)
        .and_then(|()| f.rewind())
        .map_err(|e| format!("seccomp filter: {e}"))?;
    Ok(f)
}

/// A review's filter only exists on Linux (`memfd_create`, `bwrap --seccomp`): elsewhere there is
/// no bwrap, and a review is refused, never run without its filter.
#[cfg(not(target_os = "linux"))]
fn seccomp_fd() -> Result<std::fs::File, String> {
    Err("a review's seccomp filter only exists on Linux".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_seccomp_program_is_ten_instructions_ending_in_allow_and_eacces() {
        let p = super::no_unix_sockets().unwrap();
        assert_eq!(p.len(), 10 * 8);
        let k = |i: usize| u32::from_ne_bytes(p[i * 8 + 4..i * 8 + 8].try_into().unwrap());
        assert_eq!((k(8), k(9)), (0x7fff_0000, 0x0005_000d));
    }

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
