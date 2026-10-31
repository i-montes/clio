//! A jail for ONE command, built on `bwrap` (bubblewrap). Nothing here runs a shell: the caller
//! hands over an explicit argv. Everything fails closed: if `bwrap`, `prlimit` or unprivileged
//! namespaces are missing, `Jail::detect` / `Jail::probe` return an error and nothing runs with
//! less jail than asked for.
//!
//! What the command sees: a new root made of the read-only `/usr` (plus the symlinks or folders
//! `/bin`, `/lib`, `/lib64` and the loader cache), a private `/proc`, `/dev` and `/tmp`, and the
//! working folder at `/work` (read and write). Nothing else of the person's machine is mounted.
//! No network, no other processes, an empty environment with a fixed `PATH`.
//!
//! What it does NOT do (see docs/threat-model.md): no seccomp filter (the kernel's whole syscall
//! surface stays reachable), no CPU-time accounting beyond `prlimit`, no disk quota on `/work`,
//! no protection against a kernel bug or a side channel.

use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Oldest `bwrap` accepted: 0.8.0 added `--disable-userns`, which we require.
pub const MIN_BWRAP: (u32, u32, u32) = (0, 8, 0);
/// `--size` for `--tmpfs` arrived in 0.9.0; older versions run with an unbounded `/tmp`.
const SIZE_BWRAP: (u32, u32, u32) = (0, 9, 0);
/// The environment of the jailed command. Nothing else exists in it.
const FIXED_PATH: &str = "/usr/bin:/bin";
const TRUSTED_DIRS: [&str; 3] = ["/usr/bin", "/bin", "/usr/local/bin"];

#[derive(Debug, Clone)]
pub struct Limits {
    /// Wall-clock time; the whole jail is killed when it passes.
    pub timeout: Duration,
    /// Bytes kept per stream (the tail). Far more output than this kills the command.
    pub max_output: usize,
    /// `RLIMIT_CPU`, seconds.
    pub cpu_secs: u64,
    /// `RLIMIT_AS`, bytes.
    pub mem_bytes: u64,
    /// `RLIMIT_NPROC` (counts threads too), inside the jail's own user namespace.
    pub procs: u64,
    /// `RLIMIT_FSIZE`: the largest file the command can write.
    pub file_bytes: u64,
    pub open_files: u64,
    /// Size of the `/tmp` tmpfs (only enforced when `bwrap` is 0.9 or newer).
    pub tmp_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            max_output: 64 * 1024,
            cpu_secs: 120,
            mem_bytes: 2 << 30,
            procs: 128,
            file_bytes: 64 << 20,
            open_files: 512,
            tmp_bytes: 256 << 20,
        }
    }
}

/// One command to run.
#[derive(Debug, Clone, Default)]
pub struct Job {
    /// The working folder, mounted read-write at `/work`. Must be an existing directory.
    pub work: PathBuf,
    /// Paths relative to `work` that must stay read-only inside the jail (if they exist).
    pub read_only: Vec<String>,
    /// `argv[0]` must be an absolute path inside the jail (for example `/usr/bin/python3`).
    pub argv: Vec<String>,
    /// Variables of the jailed command, besides the fixed ones.
    pub env: Vec<(String, String)>,
    /// Extra read-only mounts `(host, inside)`, for a toolchain that lives outside `/usr`.
    pub mounts: Vec<(PathBuf, PathBuf)>,
    /// Overrides the jail's limits for this job.
    pub limits: Option<Limits>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JailOutput {
    pub exit_code: Option<i32>,
    /// The signal that ended the command, when it did not exit by itself.
    pub signal: Option<i32>,
    pub timed_out: bool,
    /// Stopped because it wrote far more than `max_output`.
    pub flooded: bool,
    pub stdout: String,
    pub stderr: String,
    /// Some output was dropped (only the tail is kept).
    pub truncated: bool,
    pub elapsed_ms: u64,
}

impl JailOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.flooded
    }
}

#[derive(Debug, Clone)]
pub struct Jail {
    bwrap: PathBuf,
    prlimit: PathBuf,
    kill: Option<PathBuf>,
    version: (u32, u32, u32),
    pub limits: Limits,
}

pub(crate) fn find_in_trusted(name: &str) -> Option<PathBuf> {
    TRUSTED_DIRS
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

fn push(a: &mut Vec<String>, xs: &[&str]) {
    a.extend(xs.iter().map(|s| (*s).to_owned()));
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let v = text.split_whitespace().last()?;
    let mut it = v.split('.').map(|n| n.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

/// Keeps the last `cap` bytes of a stream and counts all of them.
struct Tail {
    buf: Mutex<Vec<u8>>,
    total: AtomicUsize,
    done: AtomicBool,
}

fn drain(mut r: impl Read, tail: &Tail, cap: usize, over: &AtomicBool, flood_at: usize) {
    let mut chunk = [0u8; 8192];
    while let Ok(n) = r.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let total = tail.total.fetch_add(n, Ordering::SeqCst) + n;
        if total > flood_at {
            over.store(true, Ordering::SeqCst);
        }
        if let Ok(mut b) = tail.buf.lock() {
            b.extend_from_slice(&chunk[..n]);
            if b.len() > cap * 2 {
                let cut = b.len() - cap;
                b.drain(..cut);
            }
        }
    }
    tail.done.store(true, Ordering::SeqCst);
}

/// Kills the jail when dropped, so a failing caller leaves nothing running.
struct Reaper<'a> {
    child: Child,
    kill: Option<&'a Path>,
}

impl Reaper<'_> {
    fn kill_all(&mut self) {
        // The jail's own pid namespace dies with its first process; the group kill is a belt.
        if let Some(k) = self.kill {
            let _ = Command::new(k)
                .args(["-KILL", "--"])
                .arg(format!("-{}", self.child.id()))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
    }
}

impl Drop for Reaper<'_> {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            self.kill_all();
        }
        let _ = self.child.wait();
    }
}

impl Jail {
    /// Finds `bwrap` and `prlimit` in the system folders and checks the `bwrap` version. This
    /// does NOT prove namespaces work on this kernel: `probe` does.
    pub fn detect() -> Result<Self, String> {
        if !cfg!(target_os = "linux") {
            return Err("the jail exists only on Linux (it needs bubblewrap)".into());
        }
        let bwrap = find_in_trusted("bwrap")
            .ok_or("bwrap (bubblewrap) is not installed in /usr/bin, /bin or /usr/local/bin")?;
        let prlimit = find_in_trusted("prlimit")
            .ok_or("prlimit (util-linux) is not installed: there would be no resource limits")?;
        let out = Command::new(&bwrap)
            .arg("--version")
            .env_clear()
            .output()
            .map_err(|e| format!("could not run {}: {e}", bwrap.display()))?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let version = parse_version(&text)
            .ok_or_else(|| format!("could not read the bwrap version from {text:?}"))?;
        if version < MIN_BWRAP {
            return Err(format!(
                "bwrap {}.{}.{} is too old, 0.8.0 or newer is needed (--disable-userns)",
                version.0, version.1, version.2
            ));
        }
        if !Path::new("/usr").is_dir() {
            return Err("there is no /usr to mount read-only".into());
        }
        Ok(Self {
            bwrap,
            prlimit,
            kill: find_in_trusted("kill"),
            version,
            limits: Limits::default(),
        })
    }

    pub fn bwrap_version(&self) -> String {
        format!("{}.{}.{}", self.version.0, self.version.1, self.version.2)
    }

    /// Runs a trivial command in the jail. An error means the jail cannot start here (the kernel
    /// forbids unprivileged user namespaces, AppArmor blocks `bwrap`, ...): do not offer tools.
    pub fn probe(&self) -> Result<(), String> {
        let work = ScratchDir::new("probe")?;
        let true_bin = find_in_trusted("true").ok_or("no /usr/bin/true to probe with")?;
        let job = Job {
            work: work.path().to_path_buf(),
            argv: vec![true_bin.to_string_lossy().into_owned()],
            limits: Some(Limits {
                timeout: Duration::from_secs(20),
                ..self.limits.clone()
            }),
            ..Job::default()
        };
        let out = self.run(&job)?;
        if out.success() {
            return Ok(());
        }
        let why = out.stderr.lines().next().unwrap_or("").trim().to_owned();
        Err(format!(
            "bwrap cannot start here ({}): the kernel or the system policy probably forbids unprivileged user namespaces",
            if why.is_empty() {
                "no message".into()
            } else {
                why
            }
        ))
    }

    /// The exact argv of `bwrap`. Public so tests (and a reviewer) can read what the jail is.
    pub fn bwrap_args(&self, job: &Job) -> Result<Vec<String>, String> {
        let work = job
            .work
            .canonicalize()
            .map_err(|e| format!("working folder {}: {e}", job.work.display()))?;
        if !work.is_dir() {
            return Err(format!("{} is not a folder", work.display()));
        }
        let lim = job.limits.as_ref().unwrap_or(&self.limits);
        let mut a: Vec<String> = Vec::new();
        push(
            &mut a,
            &[
                "--unshare-all",
                "--unshare-user", // `--unshare-all` only tries this one: we require it
                "--unshare-cgroup",
                "--disable-userns",
                "--cap-drop",
                "ALL",
                "--die-with-parent",
                "--new-session",
                "--clearenv",
                "--setenv",
                "PATH",
                FIXED_PATH,
                "--setenv",
                "HOME",
                "/tmp/home",
                "--setenv",
                "LANG",
                "C.UTF-8",
                "--setenv",
                "TMPDIR",
                "/tmp",
            ],
        );
        for (k, v) in &job.env {
            a.extend(["--setenv".into(), k.clone(), v.clone()]);
        }
        push(&mut a, &["--ro-bind", "/usr", "/usr"]);
        for top in ["/bin", "/sbin", "/lib", "/lib32", "/lib64"] {
            match std::fs::symlink_metadata(top) {
                Ok(m) if m.is_symlink() => {
                    let t = std::fs::read_link(top).map_err(|e| format!("{top}: {e}"))?;
                    a.extend([
                        "--symlink".into(),
                        t.to_string_lossy().into_owned(),
                        top.into(),
                    ]);
                }
                Ok(m) if m.is_dir() => {
                    a.extend(["--ro-bind".into(), top.into(), top.into()]);
                }
                _ => {}
            }
        }
        if Path::new("/etc/ld.so.cache").is_file() {
            push(
                &mut a,
                &["--ro-bind", "/etc/ld.so.cache", "/etc/ld.so.cache"],
            );
        }
        push(&mut a, &["--proc", "/proc", "--dev", "/dev"]);
        if self.version >= SIZE_BWRAP {
            a.extend(["--size".into(), lim.tmp_bytes.to_string()]);
        }
        a.extend([
            "--tmpfs".into(),
            "/tmp".into(),
            "--dir".into(),
            "/tmp/home".into(),
        ]);
        for (host, inside) in &job.mounts {
            a.extend([
                "--ro-bind".into(),
                host.to_string_lossy().into_owned(),
                inside.to_string_lossy().into_owned(),
            ]);
        }
        let w = work.to_string_lossy().into_owned();
        a.extend(["--bind".into(), w.clone(), "/work".into()]);
        for rel in &job.read_only {
            let host = work.join(rel);
            if host.exists() && !rel.contains("..") {
                a.extend([
                    "--ro-bind".into(),
                    host.to_string_lossy().into_owned(),
                    format!("/work/{rel}"),
                ]);
            }
        }
        // The root is a tmpfs; nothing may be created in it outside the mounts above.
        a.extend([
            "--remount-ro".into(),
            "/".into(),
            "--chdir".into(),
            "/work".into(),
        ]);
        a.push("--".into());
        a.extend([
            self.prlimit.to_string_lossy().into_owned(),
            format!("--cpu={}", lim.cpu_secs),
            format!("--as={}", lim.mem_bytes),
            format!("--nproc={}", lim.procs),
            format!("--fsize={}", lim.file_bytes),
            format!("--nofile={}", lim.open_files),
            "--core=0".into(),
            "--".into(),
        ]);
        let first = job.argv.first().ok_or("empty command")?;
        if !first.starts_with('/') {
            return Err(format!("{first:?} is not an absolute path inside the jail"));
        }
        a.extend(job.argv.iter().cloned());
        Ok(a)
    }

    /// Runs the job and waits for it. `Err` means the jail itself could not be built or started
    /// as a process; a command that fails or is killed is an `Ok` with its outcome.
    pub fn run(&self, job: &Job) -> Result<JailOutput, String> {
        let lim = job.limits.as_ref().unwrap_or(&self.limits).clone();
        let args = self.bwrap_args(job)?;
        let mut cmd = Command::new(&self.bwrap);
        cmd.args(&args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // The launcher gets an EMPTY environment too: `bwrap`'s own environment stays readable
        // inside the jail as /proc/1/environ, so nothing of the caller's may be in it.
        let started = Instant::now();
        let mut reaper = Reaper {
            child: cmd
                .spawn()
                .map_err(|e| format!("could not start {}: {e}", self.bwrap.display()))?,
            kill: self.kill.as_deref(),
        };
        let flood_at = lim.max_output.saturating_mul(8).max(1 << 20);
        let over = Arc::new(AtomicBool::new(false));
        let mk = || {
            Arc::new(Tail {
                buf: Mutex::new(Vec::new()),
                total: AtomicUsize::new(0),
                done: AtomicBool::new(false),
            })
        };
        let (so, se) = (mk(), mk());
        for (pipe, tail) in [
            (
                reaper
                    .child
                    .stdout
                    .take()
                    .map(|p| Box::new(p) as Box<dyn Read + Send>),
                so.clone(),
            ),
            (
                reaper
                    .child
                    .stderr
                    .take()
                    .map(|p| Box::new(p) as Box<dyn Read + Send>),
                se.clone(),
            ),
        ] {
            let (over, cap) = (over.clone(), lim.max_output);
            match pipe {
                Some(p) => {
                    std::thread::spawn(move || drain(p, &tail, cap, &over, flood_at));
                }
                None => tail.done.store(true, Ordering::SeqCst),
            }
        }
        let (mut timed_out, mut flooded) = (false, false);
        let status = loop {
            if let Some(s) = reaper.child.try_wait().map_err(|e| e.to_string())? {
                break s;
            }
            if started.elapsed() >= lim.timeout {
                timed_out = true;
            } else if over.load(Ordering::SeqCst) {
                flooded = true;
            }
            if timed_out || flooded {
                reaper.kill_all();
                break reaper.child.wait().map_err(|e| e.to_string())?;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // The pipes close with the jail; give the readers a moment, never wait on them forever.
        let until = Instant::now() + Duration::from_secs(2);
        while !(so.done.load(Ordering::SeqCst) && se.done.load(Ordering::SeqCst))
            && Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        let text = |t: &Tail| {
            let b = t.buf.lock().map(|b| b.clone()).unwrap_or_default();
            let keep = &b[b.len().saturating_sub(lim.max_output)..];
            String::from_utf8_lossy(keep).into_owned()
        };
        let truncated = so.total.load(Ordering::SeqCst) > lim.max_output
            || se.total.load(Ordering::SeqCst) > lim.max_output;
        Ok(JailOutput {
            exit_code: status.code(),
            signal: status.signal(),
            timed_out,
            flooded,
            stdout: text(&so),
            stderr: text(&se),
            truncated,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A private temporary folder (mode 0700) that is removed when dropped.
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new(label: &str) -> Result<Self, String> {
        use std::os::unix::fs::DirBuilderExt;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let p = std::env::temp_dir().join(format!(
            "clio-jail-{label}-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&p)
            .map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(Self(p))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version("bubblewrap 0.13.1\n"), Some((0, 13, 1)));
        assert_eq!(parse_version("bubblewrap 0.8"), Some((0, 8, 0)));
        assert_eq!(parse_version("nonsense"), None);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_relative_binary_is_refused_before_anything_starts() {
        let Ok(j) = Jail::detect() else { return };
        let d = ScratchDir::new("t").unwrap();
        let job = Job {
            work: d.path().into(),
            argv: vec!["python3".into()],
            ..Job::default()
        };
        assert!(j.bwrap_args(&job).is_err());
    }
}
