//! Pi inside `bwrap` (ADR 0009, phases 3a and 3b). Pi and its npm dependencies run as the person, so
//! they could connect to the vault's sockets, read `~/.ssh`, or write the working copy and skip
//! the toolhost's rules. Inside the jail Pi sees only what it needs, at the same absolute paths
//! it has outside (no argument or variable changes):
//!
//! - the system, read-only, and the certificates TLS needs;
//! - Node and Pi's install folder (which holds the guard), read-only;
//! - Pi's own `agent/`, `sessions/` and `home/`, read-write;
//! - the prompt file, read-only, and the folder of the toolhost's socket (made for it alone),
//!   the only way out to an effect;
//! - the working copy, **read-only**: every change goes through the toolhost. In host mode
//!   (`CLIO_GUARD_HOST=1`) the working folder is the person's home, and Pi sees it EMPTY and
//!   read-only: nothing of the person is mounted, the toolhost does it all.
//! - with the claude connector, the copy's `.claude/`, `.pi/` and `.mcp.json` covered read-only
//!   (a copy where one is a link does not start), and Clio's
//!   managed settings for Claude Code at `/etc/claude-code/managed-settings.json`, read-only.
//!
//! - the `clio` binary and the folder of the egress proxy's socket (see `net`): the only way out
//!   to the network, which is a private one. Pi reaches the provider's address through them and
//!   nothing else: no host loopback, no abstract sockets, no internet.
//!
//! It fails closed: on Linux Pi starts only if `bwrap` works, the proxy starts, and a self-test
//! from inside passes. `CLIO_PI_JAIL=off` is the only way around it, and `clio doctor` shows it
//! as a FAIL. With the claude connector there is no way around it: no jail, no Claude Code.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clio_node::jail::Jail;

use super::PiError;
use super::launch::{Launch, hardened_env};
use super::net::{EgressProxy, FORWARD_ADDR, PROXY_URL, allow_of};

const SELFTEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Runs inside the jail: the working copy is there and cannot be written, and none of the decoys
/// (files placed outside what the jail mounts) can be seen. The write is tried in a subshell: a
/// failed redirection on a special builtin like `:` ends a POSIX shell, and here failing is the
/// right answer.
const PROBE: &str = r#"cwd=$1; shift
[ -d "$cwd" ] || { echo "the working copy is not mounted"; exit 11; }
( : > "$cwd/.clio-jail-probe" ) 2>/dev/null && { rm -f "$cwd/.clio-jail-probe"; echo "the working copy is writable"; exit 12; }
for p in "$@"; do [ -e "$p" ] && { echo "visible from inside: $p"; exit 13; }; done
exit 0"#;

/// What `doctor` says about Pi's jail.
pub enum Status {
    /// `bwrap` (this version) starts here; the self-test runs at every start.
    Ready(String),
    /// `CLIO_PI_JAIL=off`.
    Off,
    /// Not Linux: there is no jail to build.
    Unsupported,
    /// Linux, but the jail cannot start: Pi will not start either.
    Unavailable(String),
}

pub fn status() -> Status {
    if switched_off() {
        return Status::Off;
    }
    if !cfg!(target_os = "linux") {
        return Status::Unsupported;
    }
    match Jail::detect().and_then(|j| j.probe().map(|()| j.bwrap_version())) {
        Ok(v) => Status::Ready(v),
        Err(why) => Status::Unavailable(why),
    }
}

fn switched_off() -> bool {
    std::env::var("CLIO_PI_JAIL").is_ok_and(|v| v == "off")
}

/// A jail that passed its self-test for one launch. It owns the session's egress proxy: keep it
/// alive as long as Pi runs.
pub struct PiJail {
    bwrap: PathBuf,
    proxy: EgressProxy,
}

/// The policy. `Ok(None)`: Pi runs without a jail (switched off, or a system that cannot have
/// one). `Err`: it must not start.
pub fn prepare(l: &Launch) -> Result<Option<PiJail>, PiError> {
    if unjailed(l, switched_off(), cfg!(target_os = "linux"))? {
        return Ok(None);
    }
    let bridge = l.provider.extension.is_some();
    // In host mode nothing of the working folder is mounted, so there is nothing of it to mask.
    if bridge && !host_mode(l) {
        refuse_odd_claude_paths(&l.cwd)?;
    }
    let refuse = |why: String| {
        // With the bridge there is no way around it: `CLIO_PI_JAIL=off` is refused too.
        let off = if bridge {
            ""
        } else {
            ", or set CLIO_PI_JAIL=off at your own risk"
        };
        PiError::Spawn(format!(
            "Pi does not start: the Pi jail could not be set up ({why}). Without the jail Pi would have your own permissions; install bubblewrap (bwrap) and enable user namespaces{off} (ADR 0009)"
        ))
    };
    let jail = Jail::detect().map_err(&refuse)?;
    // The one address Pi may reach: the provider's, from its URL.
    let allow = allow_of(&l.provider.base_url).map_err(&refuse)?;
    let proxy = EgressProxy::start(&egress_parent(l), &allow).map_err(&refuse)?;
    let j = PiJail {
        bwrap: jail.bwrap().to_path_buf(),
        proxy,
    };
    j.selftest(l).map_err(refuse)?;
    j.proxy.clear_denied();
    Ok(Some(j))
}

/// Whether Pi runs without a jail: only when it is switched off or the system cannot have one,
/// and never with the Claude bridge, whose Claude Code would then run as the person on the
/// working copy's settings (ADR 0009).
fn unjailed(l: &Launch, off: bool, linux: bool) -> Result<bool, PiError> {
    if !off && linux {
        return Ok(false);
    }
    if l.provider.extension.is_none() {
        return Ok(true);
    }
    Err(PiError::Spawn(if off {
        "the claude connector needs the Pi jail: remove CLIO_PI_JAIL=off (ADR 0009)".into()
    } else {
        "the claude connector needs the Pi jail, which only exists on Linux with bubblewrap: use another connector on this system, for example `clio connect opencode` (ADR 0009)".into()
    }))
}

/// A host-mode session (the guard reads the same variable).
fn host_mode(l: &Launch) -> bool {
    l.extra_env
        .iter()
        .any(|(k, v)| k == super::session::HOST_ENV && v == "1")
}

/// What the bridge's Claude Code (and the bridge) load from the working copy.
const CLAUDE_PATHS: [&str; 3] = [".claude", ".pi", ".mcp.json"];

/// `claude_masks` would mask a link's target as the host sees it, Claude Code follows it inside
/// the jail: a copy with one of these as a link is not launched at all. Nor one whose `.mcp.json`
/// is not a plain file (a folder or a FIFO): `/dev/null` is bound only over a plain file.
fn refuse_odd_claude_paths(cwd: &Path) -> Result<(), PiError> {
    for name in CLAUDE_PATHS {
        let p = cwd.join(name);
        let Ok(m) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        let what = if m.is_symlink() {
            "as a symbolic link"
        } else if name == ".mcp.json" && !m.is_file() {
            "and is not a regular file"
        } else {
            continue;
        };
        return Err(PiError::Spawn(format!(
            "the working copy has {} {what}: Clio does not start Claude Code like that",
            p.display()
        )));
    }
    Ok(())
}

/// Where the proxy's folder goes: next to the toolhost's (both short-lived, both 0700, both in
/// the runtime folder), or the system's temporary folder when there is no toolhost.
fn egress_parent(l: &Launch) -> PathBuf {
    l.extra_env
        .iter()
        .find(|(k, _)| k == "CLIO_TOOLHOST_SOCKET")
        .and_then(|(_, v)| Path::new(v).parent()?.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).to_owned()).collect()
}

impl PiJail {
    /// `argv` run inside the jail with Pi's environment. The environment is set on `bwrap`
    /// itself, never with `--setenv`: the arguments of a process are readable by every user of
    /// the machine, and the provider's key is in that environment.
    pub fn command(&self, l: &Launch, session_id: &str, argv: &[OsString]) -> Command {
        let mut c = Command::new(&self.bwrap);
        // `current_dir` is for whoever looks at this process from outside (`bwrap` keeps it);
        // inside, `--chdir` puts Pi in the same folder.
        // Inside, the forwarder comes first: it listens on the jail's loopback, sends each
        // connection to the proxy, and starts `argv` as its child.
        let mut inner: Vec<OsString> = vec![
            l.helper.clone().into_os_string(),
            "pi-net".into(),
            "--listen".into(),
            FORWARD_ADDR.into(),
            "--upstream".into(),
            self.proxy.socket().as_os_str().to_owned(),
            "--".into(),
        ];
        inner.extend(argv.iter().cloned());
        c.args(self.args(l))
            .arg("--")
            .args(&inner)
            .env_clear()
            .envs(hardened_env(l, session_id))
            .envs(proxy_env())
            .current_dir(&l.cwd);
        c
    }

    /// What Pi (or anything else in the jail) asked the proxy for and was refused.
    pub fn egress_denied(&self) -> Vec<String> {
        self.proxy.denied()
    }

    /// The exact arguments of `bwrap`, public so a test (and a reviewer) can read what Pi sees.
    pub fn args(&self, l: &Launch) -> Vec<String> {
        let mut a = strings(&[
            // `--unshare-all` includes the network: a namespace of its own, with a loopback
            // that nobody listens on. The way out is the forwarder and the proxy (see `net`).
            "--unshare-all",
            // No `--die-with-parent`: it is tied to the THREAD that started `bwrap`, and a
            // session is opened from a worker thread that ends right after. Pi (and so the jail)
            // ends by itself when its input closes, also if `clio` is killed.
            "--new-session",
            "--cap-drop",
            "ALL",
            "--ro-bind",
            "/usr",
            "/usr",
        ]);
        for top in ["/bin", "/sbin", "/lib", "/lib32", "/lib64"] {
            match std::fs::symlink_metadata(top) {
                Ok(m) if m.is_symlink() => {
                    if let Ok(t) = std::fs::read_link(top) {
                        a.extend(["--symlink".into(), t.display().to_string(), top.into()]);
                    }
                }
                Ok(m) if m.is_dir() => a.extend(["--ro-bind".into(), top.into(), top.into()]),
                _ => {}
            }
        }
        // The loader's cache and the certificates TLS needs, and nothing else of /etc: no DNS
        // files, the proxy resolves the provider's name on the host.
        for f in ["/etc/ld.so.cache", "/etc/ssl", "/etc/ca-certificates"] {
            if Path::new(f).exists() {
                a.extend(["--ro-bind".into(), f.into(), f.into()]);
            }
        }
        a.extend(strings(&[
            "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
        ]));
        let host = host_mode(l);
        let cwd = l.cwd.display().to_string();
        if host {
            // Before anything that may sit under the home (Node, Pi, Clio's data): those are
            // mounted on top; the empty folder is made read-only at the end.
            a.extend(["--tmpfs".into(), cwd.clone()]);
        }
        // From here on, mounts that may sit under /tmp (the tmpfs above comes first).
        let mut seen = BTreeSet::new();
        let mut bind = |a: &mut Vec<String>, flag: &str, p: &Path| {
            let s = p.display().to_string();
            if seen.insert((flag.to_owned(), s.clone())) {
                a.extend([flag.to_owned(), s.clone(), s]);
            }
        };
        let node_root = l.node.path.parent().and_then(Path::parent);
        if let Some(root) = node_root.filter(|r| r.parent().is_some() && !r.starts_with("/usr")) {
            bind(&mut a, "--ro-bind", root);
        }
        bind(&mut a, "--ro-bind", &l.install.dir);
        // The guard lives inside the install folder, already mounted. One that does not (a
        // symlinked checkout) is mounted too: an extension is a folder with an `index.ts` and
        // its siblings, and that folder is its own; a loose file is mounted alone, because the
        // folder around it holds other things.
        if let Some(g) = l
            .guard
            .as_deref()
            .filter(|g| !g.starts_with(&l.install.dir))
        {
            let own = g.file_name().is_some_and(|n| n == "index.ts");
            bind(&mut a, "--ro-bind", g.parent().filter(|_| own).unwrap_or(g));
        }
        for (k, v) in &l.extra_env {
            let p = Path::new(v);
            match k.as_str() {
                // The prompt: that one file. The socket: its own folder (made for it alone).
                "CLIO_GUARD_PROMPT_FILE" => bind(&mut a, "--ro-bind", p),
                "CLIO_TOOLHOST_SOCKET" => {
                    if let Some(d) = p.parent() {
                        bind(&mut a, "--bind", d);
                    }
                }
                _ => {}
            }
        }
        bind(&mut a, "--ro-bind", &l.helper);
        bind(&mut a, "--bind", self.proxy.dir());
        for rw in [
            l.paths.agent_dir(),
            l.paths.sessions_dir(),
            l.paths.home_dir(),
        ] {
            bind(&mut a, "--bind", &rw);
        }
        // Clio's enabled skills, read-only: Pi lists them, and nothing in here writes them (S-68).
        if let Some(s) = &l.skills {
            bind(&mut a, "--ro-bind", s);
        }
        if host {
            a.extend(["--remount-ro".into(), cwd.clone()]);
        } else {
            bind(&mut a, "--ro-bind", &l.cwd);
        }
        if l.provider.extension.is_some() {
            claude_masks(&mut a, l, host);
        }
        a.extend(["--chdir".into(), cwd]);
        a
    }

    /// Files placed where the jail must not show them: every folder above Pi's own (Clio's data
    /// folder; for a review, its run folder too), the folder that holds the vault's sockets (the
    /// one above the toolhost's) and the person's home. One that cannot be written is skipped,
    /// and so is one inside the copy Pi is meant to see.
    fn decoys(&self, l: &Launch) -> Vec<PathBuf> {
        let name = format!(".pi-jail-decoy-{}", std::process::id());
        let mut dirs: Vec<PathBuf> = Vec::new();
        let seen = |a: &Path| !host_mode(l) && a.starts_with(&l.cwd);
        dirs.extend(
            l.paths
                .root
                .ancestors()
                .skip(1)
                .filter(|a| a.parent().is_some() && !seen(a))
                .map(Path::to_path_buf),
        );
        for (k, v) in &l.extra_env {
            if k == "CLIO_TOOLHOST_SOCKET" {
                dirs.extend(
                    Path::new(v)
                        .parent()
                        .and_then(Path::parent)
                        .map(Path::to_path_buf),
                );
            }
        }
        dirs.extend(std::env::var_os("HOME").map(PathBuf::from));
        let mut out: Vec<PathBuf> = dirs.into_iter().map(|d| d.join(&name)).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Starts the jail the way Pi will be started and checks it from inside, twice: the files
    /// (the copy cannot be written, the decoys cannot be seen) and the network (nothing but the
    /// proxy gets out). Any failure here (a mount that cannot be made, `bwrap` refused, a decoy
    /// visible, the copy writable, a way out that works) means Pi does not start.
    fn selftest(&self, l: &Launch) -> Result<(), String> {
        struct Decoys(Vec<PathBuf>);
        impl Drop for Decoys {
            fn drop(&mut self) {
                for p in &self.0 {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        let placed = Decoys(
            self.decoys(l)
                .into_iter()
                .filter(|p| std::fs::write(p, b"decoy").is_ok())
                .collect(),
        );
        let mut files: Vec<OsString> = vec![
            "/bin/sh".into(),
            "-c".into(),
            PROBE.into(),
            "probe".into(),
            l.cwd.clone().into_os_string(),
        ];
        files.extend(placed.0.iter().map(|p| p.clone().into_os_string()));
        self.run_probe(l, &files)?;
        // Things on the host that Pi must not reach: a loopback port and an abstract socket.
        let host_port = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = host_port.local_addr().map_err(|e| e.to_string())?.port();
        let abstract_socket = abstract_listener();
        let mut net: Vec<OsString> = vec![
            l.helper.clone().into_os_string(),
            "pi-net".into(),
            "probe".into(),
            "--tcp-port".into(),
            port.to_string().into(),
            "--denied".into(),
            "selftest.invalid:443".into(),
        ];
        if let Some((name, _)) = &abstract_socket {
            net.extend(["--abstract".into(), name.into()]);
        }
        self.run_probe(l, &net)
    }

    fn run_probe(&self, l: &Launch, argv: &[OsString]) -> Result<(), String> {
        let mut child = self
            .command(l, "selftest", argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start bwrap: {e}"))?;
        let t = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break s;
            }
            if t.elapsed() > SELFTEST_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the self-test did not finish in time".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if status.success() {
            return Ok(());
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let said = String::from_utf8_lossy(&[out.stdout, out.stderr].concat())
            .trim()
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join(" | ");
        Err(format!(
            "the self-test failed (exit {}): {said}",
            status
                .code()
                .map_or("by signal".to_owned(), |c| c.to_string())
        ))
    }
}

/// The bridge runs Claude Code in the working copy, and Claude Code (and the bridge) load code
/// and settings from there: `.claude/` (hooks, `env`, `apiKeyHelper`), `.pi/claude-bridge.json`
/// (another Claude Code binary, MCP turned back on) and `.mcp.json`. The copy is hostile content
/// (ADR 0009): they are covered with an empty, read-only folder or `/dev/null`. A link is never
/// followed (`prepare` refuses a copy that has one, and a `.mcp.json` that is not a plain file). One that does not exist cannot be covered
/// (`bwrap` would have to create it in the read-only copy); the copy is read-only in here and
/// the toolhost does not write `.claude` or `.pi` (S-16). Claude Code also gets Clio's managed
/// settings, read-only: no hook runs, whatever any other source says.
fn claude_masks(a: &mut Vec<String>, l: &Launch, host: bool) {
    // Host mode: the folder is empty in here, there is nothing of it to cover.
    let kind = |name: &str| {
        std::fs::symlink_metadata(l.cwd.join(name))
            .ok()
            .filter(|_| !host)
            .map(|m| m.file_type())
    };
    for d in [".claude", ".pi"] {
        if kind(d).is_some_and(|t| t.is_dir()) {
            let p = l.cwd.join(d).display().to_string();
            a.extend(["--tmpfs".into(), p.clone(), "--remount-ro".into(), p]);
        }
    }
    if kind(".mcp.json").is_some_and(|t| t.is_file()) {
        a.extend([
            "--ro-bind".into(),
            "/dev/null".into(),
            l.cwd.join(".mcp.json").display().to_string(),
        ]);
    }
    a.extend([
        "--ro-bind".into(),
        l.paths.claude_policy().display().to_string(),
        CLAUDE_POLICY_IN_JAIL.into(),
    ]);
}

/// Where Claude Code 2.1 reads managed settings on Linux.
pub const CLAUDE_POLICY_IN_JAIL: &str = "/etc/claude-code/managed-settings.json";

/// An abstract unix socket on the host (Linux only): the jail must not be able to connect to it.
#[cfg(target_os = "linux")]
fn abstract_listener() -> Option<(String, std::os::unix::net::UnixListener)> {
    use std::os::linux::net::SocketAddrExt;
    let name = format!(
        "clio-jail-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos())
    );
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(&name).ok()?;
    let l = std::os::unix::net::UnixListener::bind_addr(&addr).ok()?;
    Some((name, l))
}

#[cfg(not(target_os = "linux"))]
fn abstract_listener() -> Option<(String, std::os::unix::net::UnixListener)> {
    None
}

/// Pi's proxy variables: everything it sends over the network goes to the forwarder, none of it is
/// exempt (`NO_PROXY` empty).
fn proxy_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("HTTPS_PROXY", PROXY_URL),
        ("HTTP_PROXY", PROXY_URL),
        ("https_proxy", PROXY_URL),
        ("http_proxy", PROXY_URL),
        ("NO_PROXY", ""),
        ("no_proxy", ""),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::pi::PiPaths;
    use crate::pi::install::{NodeInfo, PiInstall};
    use crate::pi::launch::SessionTarget;

    fn launch_for(connector: &str, cwd: &Path, data: &Path) -> Launch {
        Launch {
            install: PiInstall {
                version: "1.4.0".into(),
                dir: "/p/1.4.0".into(),
                bin: "/p/1.4.0/node_modules/.bin/pi".into(),
            },
            node: NodeInfo {
                path: "/n/bin/node".into(),
                version: (22, 23, 2),
            },
            paths: PiPaths::under_data(data),
            provider: crate::connector::by_name(connector)
                .unwrap()
                .pi_launch_provider("m"),
            api_key: None,
            cwd: cwd.into(),
            session: SessionTarget::New { id: None },
            name: None,
            guard: None,
            guard_tools: vec![],
            extra_env: vec![],
            helper: "/p/clio".into(),
            skills: None,
        }
    }

    fn args_for(connector: &str, cwd: &Path, data: &Path) -> Vec<String> {
        let l = launch_for(connector, cwd, data);
        let j = PiJail {
            bwrap: "/usr/bin/bwrap".into(),
            proxy: EgressProxy::start(data, "api.anthropic.com:443").unwrap(),
        };
        j.args(&l)
    }

    // S-68: Pi sees Clio's enabled skills read-only, and nothing else of the skills folder.
    #[test]
    fn the_skills_are_mounted_read_only_and_only_the_live_ones() {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        let live = t.path().join("skills/live");
        std::fs::create_dir_all(&live).unwrap();
        let mut l = launch_for("opencode", &cwd, t.path());
        let j = PiJail {
            bwrap: "/usr/bin/bwrap".into(),
            proxy: EgressProxy::start(t.path(), "api.anthropic.com:443").unwrap(),
        };
        let s = live.display().to_string();
        assert!(!j.args(&l).contains(&s));
        l.skills = Some(live.clone());
        let a = j.args(&l);
        assert!(a.windows(3).any(|w| w == ["--ro-bind", &s, &s]), "{a:?}");
        assert_eq!(a.iter().filter(|x| x.starts_with(&s)).count(), 2, "{a:?}");
        let root = t.path().join("skills").display().to_string();
        assert!(!a.contains(&root));
    }

    // Finding 1 of the claude-bridge review: a working copy's `.claude/settings.json` hooks ran in
    // the jail and read the token. With the bridge they are covered, and the policy is mounted.
    #[test]
    fn with_the_bridge_the_copys_claude_settings_are_masked_and_the_policy_is_mounted() {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path().join("proj");
        std::fs::create_dir_all(cwd.join(".claude")).unwrap();
        std::fs::create_dir_all(cwd.join(".pi")).unwrap();
        std::fs::write(cwd.join(".mcp.json"), "{}").unwrap();
        let s = |p: &Path| p.display().to_string();
        let has = |a: &[String], want: &[String]| a.windows(want.len()).any(|w| w == want);
        let a = args_for("claude", &cwd, t.path());
        let at = |want: &[String]| a.windows(want.len()).position(|w| w == want).unwrap();
        let copy = at(&["--ro-bind".into(), s(&cwd), s(&cwd)]);
        // Covered, and the cover is read-only: a writable tmpfs would take what a process in the
        // jail puts there and show it to the next Claude Code of the session.
        let ro = |d: &str| {
            let p = s(&cwd.join(d));
            vec!["--tmpfs".into(), p.clone(), "--remount-ro".into(), p]
        };
        for want in [
            ro(".claude"),
            ro(".pi"),
            vec![
                "--ro-bind".into(),
                "/dev/null".into(),
                s(&cwd.join(".mcp.json")),
            ],
        ] {
            assert!(at(&want) > copy, "{want:?} after the copy: {a:?}");
        }
        let policy = PiPaths::under_data(t.path()).claude_policy();
        assert!(has(
            &a,
            &[
                "--ro-bind".into(),
                s(&policy),
                "/etc/claude-code/managed-settings.json".into()
            ]
        ));
        // A link is never followed: its host target is not what Claude Code sees in the jail.
        std::fs::remove_dir(cwd.join(".pi")).unwrap();
        std::os::unix::fs::symlink("/usr", cwd.join(".pi")).unwrap();
        let a = args_for("claude", &cwd, t.path());
        assert!(!has(&a, &["--tmpfs".into(), "/usr".into()]));
        assert!(!has(&a, &["--tmpfs".into(), s(&cwd.join(".pi"))]));
        // Another connector: nothing of this.
        let o = args_for("opencode", &cwd, t.path());
        assert!(
            !o.iter()
                .any(|x| x.contains(".claude") || x.contains(".mcp.json"))
        );
        assert!(!o.iter().any(|x| x.contains("claude-code")));
        assert_eq!(
            o.iter().filter(|x| *x == "--tmpfs").count(),
            1,
            "only /tmp: {o:?}"
        );
    }

    // ...and the launch refuses such a copy before any jail is built, whichever of the three it is.
    // Linux only: elsewhere the claude connector is refused earlier, as there is no jail at all.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_with_a_claude_path_as_a_link_or_an_odd_mcp_json_does_not_launch_the_bridge() {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(t.path().join("elsewhere")).unwrap();
        for name in CLAUDE_PATHS {
            let link = cwd.join(name);
            std::os::unix::fs::symlink(t.path().join("elsewhere"), &link).unwrap();
            let Err(e) = prepare(&launch_for("claude", &cwd, t.path())) else {
                panic!("{name} as a link: the bridge launched")
            };
            assert_eq!(
                e.to_string(),
                format!(
                    "the working copy has {} as a symbolic link: Clio does not start Claude Code like that",
                    link.display()
                )
            );
            std::fs::remove_file(&link).unwrap();
        }
        // A dangling link is a link too.
        std::os::unix::fs::symlink("/nowhere", cwd.join(".claude")).unwrap();
        assert!(refuse_odd_claude_paths(&cwd).is_err());
        std::fs::remove_file(cwd.join(".claude")).unwrap();
        // A `.mcp.json` that is a folder or a FIFO would not be covered: refused too.
        let mcp = cwd.join(".mcp.json");
        let odd = format!(
            "the working copy has {} and is not a regular file: Clio does not start Claude Code like that",
            mcp.display()
        );
        std::fs::create_dir(&mcp).unwrap();
        assert_eq!(refuse_odd_claude_paths(&cwd).unwrap_err().to_string(), odd);
        std::fs::remove_dir(&mcp).unwrap();
        let fifo = Command::new("mkfifo").arg(&mcp).status().unwrap();
        assert!(fifo.success());
        assert_eq!(refuse_odd_claude_paths(&cwd).unwrap_err().to_string(), odd);
        std::fs::remove_file(&mcp).unwrap();
        // Plain ones are fine.
        std::fs::write(&mcp, "{}").unwrap();
        std::fs::create_dir(cwd.join(".claude")).unwrap();
        assert!(refuse_odd_claude_paths(&cwd).is_ok());
        // Other connectors do not run Claude Code: no such check.
        assert!(refuse_odd_claude_paths(&t.path().join("elsewhere")).is_ok());
    }

    // No jail, no Claude: switched off or impossible, the claude connector refuses; the others
    // keep running without it, as before.
    #[test]
    fn without_a_jail_the_claude_connector_does_not_start_and_the_others_do() {
        let t = tempfile::tempdir().unwrap();
        let claude = launch_for("claude", t.path(), t.path());
        let msg = |r: Result<bool, PiError>| r.err().map(|e| e.to_string()).unwrap_or_default();
        assert_eq!(
            msg(unjailed(&claude, true, true)),
            "the claude connector needs the Pi jail: remove CLIO_PI_JAIL=off (ADR 0009)"
        );
        assert!(msg(unjailed(&claude, false, false)).contains("only exists on Linux"));
        assert!(!unjailed(&claude, false, true).unwrap());
        for other in ["opencode", "anthropic-api"] {
            let l = launch_for(other, t.path(), t.path());
            assert!(unjailed(&l, true, true).unwrap(), "{other}");
            assert!(unjailed(&l, false, false).unwrap(), "{other}");
            assert!(!unjailed(&l, false, true).unwrap(), "{other}");
        }
    }

    #[test]
    fn the_probe_script_has_no_unbalanced_quotes() {
        // `sh -n` parses without running: a typo in the probe would otherwise look like a jail failure.
        let o = Command::new("/bin/sh")
            .args(["-n", "-c", PROBE])
            .output()
            .unwrap_or_else(|e| panic!("sh: {e}"));
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    // A review's Pi lives under the run folder: the self-test plants its decoys in the run
    // folder and Clio's data folder too, so it still proves Pi sees neither.
    #[test]
    fn the_decoys_cover_every_folder_above_pis_own() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let mut l = launch_for("opencode", &data.join("runs/r1/sandbox"), &data);
        let review = data.join("runs/r1/reviews/1");
        l.paths = PiPaths {
            root: review.clone(),
        };
        let j = PiJail {
            bwrap: "/usr/bin/bwrap".into(),
            proxy: EgressProxy::start(t.path(), "api.anthropic.com:443").unwrap(),
        };
        let name = format!(".pi-jail-decoy-{}", std::process::id());
        let d = j.decoys(&l);
        for dir in [
            data.join("runs/r1/reviews"),
            data.join("runs/r1"),
            data.join("runs"),
            data.clone(),
        ] {
            assert!(d.contains(&dir.join(&name)), "{}: {d:?}", dir.display());
        }
        assert!(!d.contains(&review.join(&name)), "{d:?}");
        assert!(
            !d.iter().any(|p| p.parent() == Some(Path::new("/"))),
            "{d:?}"
        );
    }

    // Host mode: Pi's working folder is the person's home, and in the jail it is an empty,
    // read-only folder: nothing of the person is mounted (the toolhost does every file call).
    #[test]
    fn in_host_mode_the_jail_mounts_nothing_of_the_home() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".mcp.json"), "{}").unwrap();
        // Clio's data under the home, as on a real machine.
        let data = home.join(".local/share/clio");
        let s = |p: &Path| p.display().to_string();
        for connector in ["opencode", "claude"] {
            let mut l = launch_for(connector, &home, &data);
            l.extra_env = vec![(crate::pi::session::HOST_ENV.into(), "1".into())];
            let j = PiJail {
                bwrap: "/usr/bin/bwrap".into(),
                proxy: EgressProxy::start(t.path(), "api.anthropic.com:443").unwrap(),
            };
            let a = j.args(&l);
            let at = |want: &[String]| a.windows(want.len()).position(|w| w == want);
            assert!(
                !a.windows(2)
                    .any(|w| w[0].ends_with("bind") && w[1] == s(&home)),
                "{connector}: the home is mounted: {a:?}"
            );
            let empty = at(&["--tmpfs".into(), s(&home)]).expect("an empty home");
            let ro = at(&["--remount-ro".into(), s(&home)]).expect("read-only");
            // Everything else is mounted between the two: on top of the empty folder, before it
            // turns read-only.
            let agent = at(&[
                "--bind".into(),
                s(&l.paths.agent_dir()),
                s(&l.paths.agent_dir()),
            ])
            .unwrap();
            assert!(empty < agent && agent < ro, "{connector}: {a:?}");
            assert_eq!(at(&["--chdir".into(), s(&home)]), Some(a.len() - 2));
            // Nothing of the person's `.claude` or `.mcp.json` to cover: it is not there.
            assert!(
                !a.iter()
                    .any(|x| x.contains(".claude") || x.contains(".mcp.json")),
                "{connector}: {a:?}"
            );
            assert_eq!(
                a.iter().any(|x| x == CLAUDE_POLICY_IN_JAIL),
                connector == "claude"
            );
        }
    }
}
