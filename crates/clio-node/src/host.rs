//! The host launcher: ONE shell command on the person's real machine (`/` as it is, the network
//! only when asked), with Clio's own folders shown empty and read-only. Two ways, picked by
//! `detect`: `Systemd` runs it in a transient unit through `sudo -n systemd-run`, where `sudo`
//! works; `Bwrap` binds `/` onto itself, where `sudo` does not (`bwrap` sets `no_new_privs`).
//! Everything fails closed: if the launcher cannot be built, nothing runs, never unmasked.
//!
//! The masks are not a wall against the command: with `Systemd` it has `sudo`. They keep Clio's
//! folders out of an ordinary command's sight, not out of root's reach.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::jail::{JailOutput, bwrap_version, find_in_trusted, run_command};

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
    pub timeout: Duration,
    /// Bytes kept per stream (the tail).
    pub max_output: usize,
    /// Clio's folders: shown empty and read-only. Absolute; one that does not exist is skipped.
    pub masks: Vec<PathBuf>,
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
        self.build(job, &unit_name())
    }

    fn build(&self, job: &HostJob, unit: &str) -> Result<(PathBuf, Vec<String>), String> {
        let cwd = job
            .cwd
            .canonicalize()
            .map_err(|e| format!("working folder {}: {e}", job.cwd.display()))?;
        if !cwd.is_dir() {
            return Err(format!("{} is not a folder", cwd.display()));
        }
        let cwd = cwd.to_string_lossy().into_owned();
        let mut masks = Vec::new();
        for m in &job.masks {
            if !m.is_absolute() {
                return Err(format!("mask {} is not an absolute path", m.display()));
            }
            match m.canonicalize() {
                Ok(c) => masks.push(c.to_string_lossy().into_owned()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("mask {}: {e}", m.display())),
            }
        }
        let env = [
            ("HOME", job.home.to_string_lossy().into_owned()),
            ("USER", self.user.clone()),
            ("LOGNAME", self.user.clone()),
            ("LANG", "C.UTF-8".into()),
            ("PATH", PATH.into()),
            ("XDG_RUNTIME_DIR", format!("/run/user/{}", self.uid)),
        ];
        let shell = [
            self.bash.to_string_lossy().into_owned(),
            "-lc".into(),
            job.command.clone(),
        ];
        let mut a: Vec<String> = Vec::new();
        let program = match &self.tools {
            Tools::Bwrap(bwrap) => {
                // Its own pid namespace: when its first process dies, everything it started dies.
                a.extend(
                    [
                        "--bind",
                        "/",
                        "/",
                        "--dev-bind",
                        "/dev",
                        "/dev",
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
                    a.extend([
                        "--tmpfs".into(),
                        m.clone(),
                        "--remount-ro".into(),
                        m.clone(),
                    ]);
                }
                for (k, v) in env {
                    a.extend(["--setenv".into(), k.into(), v]);
                }
                a.extend(["--chdir".into(), cwd, "--".into()]);
                bwrap.clone()
            }
            Tools::Systemd { sudo, run, .. } => {
                // `TemporaryFileSystem=` splits on spaces and colons and unquotes: such a path
                // would not be masked as written.
                if let Some(m) = masks.iter().find(|m| {
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
                    // A backstop past our own deadline (which kills first and reports it), for
                    // when this process dies and nobody stops the unit.
                    "-p".into(),
                    format!("RuntimeMaxSec={}", job.timeout.as_secs() + 10),
                    "-p".into(),
                    "KillMode=control-group".into(),
                ]);
                if !job.network {
                    a.extend(["-p".into(), "PrivateNetwork=yes".into()]);
                }
                for m in &masks {
                    a.extend(["-p".into(), format!("TemporaryFileSystem={m}:ro")]);
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
        let unit = unit_name();
        let (program, args) = self.build(job, &unit)?;
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
        )
    }

    /// Proves, inside `scratch`, that a masked folder is empty and read-only, that the network
    /// is cut unless asked for, and (with `Systemd`) that `sudo` works. An error means no `exec`.
    pub fn self_test(&self, scratch: &Path) -> Result<(), String> {
        let mask = scratch.join("clio-mask");
        std::fs::create_dir_all(&mask).map_err(|e| format!("{}: {e}", mask.display()))?;
        std::fs::write(mask.join("decoy"), "decoy").map_err(|e| format!("decoy: {e}"))?;
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
                timeout: Duration::from_secs(20),
                max_output: 4096,
                masks: vec![mask.clone()],
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
