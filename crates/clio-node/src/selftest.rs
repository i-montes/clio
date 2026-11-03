//! The jail's self-test: a program INSIDE the jail tries what the jail forbids, and the host
//! checks from outside that none of it worked. Used by `clio doctor` and, before the agent is
//! offered `run_tests`, by `clio run`. One FAIL closes the tool.

use crate::jail::{Jail, Job, ScratchDir};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const SCRIPT: &str = include_str!("selftest.py");

/// The checks the program must report, in the order they are shown.
pub const CHECKS: [&str; 14] = [
    "sees_only_work",
    "home_decoy_unreadable",
    "home_folder_invisible",
    "runtime_dir_invisible",
    "network_tcp",
    "network_abstract_socket",
    "write_usr",
    "write_etc",
    "write_root",
    "write_real_home",
    "host_processes_hidden",
    "kill_host_parent",
    "environment_clean",
    "no_nested_user_namespace",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct SelfTest {
    pub checks: Vec<SelfCheck>,
}

impl SelfTest {
    pub fn ok(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.passed)
    }
}

/// Removes a file when dropped (the decoy in the real home).
struct Decoy(PathBuf);
impl Drop for Decoy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A host process the jail must not be able to see or kill.
struct Victim(Child);
impl Drop for Victim {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// An abstract unix socket (Linux only; the jail is Linux only, this is never reached elsewhere).
#[cfg(target_os = "linux")]
fn bind_abstract(name: &str) -> Result<UnixListener, String> {
    use std::os::linux::net::SocketAddrExt;
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
        .map_err(|e| e.to_string())?;
    UnixListener::bind_addr(&addr).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn bind_abstract(_: &str) -> Result<UnixListener, String> {
    Err("abstract unix sockets exist only on Linux".into())
}

fn fail(name: &str, why: impl Into<String>) -> SelfCheck {
    SelfCheck {
        name: name.into(),
        passed: false,
        detail: why.into(),
    }
}

impl Jail {
    /// Runs the self-test. `Err` only when the jail could not even be started.
    pub fn self_test(&self) -> Result<SelfTest, String> {
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let work = ScratchDir::new("selftest")?;
        std::fs::write(work.path().join("marker"), "marker").map_err(|e| e.to_string())?;

        // A decoy where the person's files are: the real home (or the temp folder without one).
        let real_home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|h| h.is_absolute() && h.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let decoy = Decoy(real_home.join(format!(".clio-jail-decoy-{nonce}")));
        std::fs::write(&decoy.0, "not a secret: a decoy planted by clio doctor")
            .map_err(|e| format!("{}: {e}", decoy.0.display()))?;
        let escape = real_home.join(format!("clio-escape-{nonce}"));
        let _escape_guard = Decoy(escape.clone());

        // A decoy where Clio's own sockets live.
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir());
        let runtime_decoy = runtime
            .as_ref()
            .map(|r| r.join(format!(".clio-jail-decoy-{nonce}")));
        let _runtime_guard = runtime_decoy.clone().map(Decoy);
        if let Some(p) = &runtime_decoy {
            let _ = std::fs::write(p, "decoy");
        }

        // The host's listeners: a TCP port and an abstract unix socket.
        let tcp = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        tcp.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = tcp.local_addr().map_err(|e| e.to_string())?.port();
        let abstract_name = format!("clio-jail-selftest-{nonce}");
        let abs = bind_abstract(&abstract_name)?;
        abs.set_nonblocking(true).map_err(|e| e.to_string())?;

        let victim = Victim(
            Command::new(crate::jail::find_in_trusted("sleep").ok_or("no sleep binary")?)
                .arg("120")
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?,
        );
        // Every variable of the caller (the real keys, the ssh agent socket, ...) must be absent.
        let bait: Vec<String> = std::env::vars_os()
            .filter_map(|(k, _)| k.into_string().ok())
            .filter(|k| !matches!(k.as_str(), "PATH" | "HOME" | "LANG" | "TMPDIR" | "PWD"))
            .collect();
        let params = serde_json::json!({
            "home_decoy": decoy.0,
            "real_home": real_home,
            "runtime_decoy": runtime_decoy,
            "port": port,
            "abstract": abstract_name,
            "host_pid": victim.0.id(),
            "nonce": nonce,
            "bait": bait,
        });
        let python = crate::jail::find_in_trusted("python3")
            .ok_or("python3 is not installed in /usr/bin: the self-test needs it")?;
        let job = Job {
            work: work.path().to_path_buf(),
            argv: vec![
                python.to_string_lossy().into_owned(),
                "-I".into(),
                "-c".into(),
                SCRIPT.into(),
                params.to_string(),
            ],
            env: vec![("PYTHONDONTWRITEBYTECODE".into(), "1".into())],
            limits: Some(crate::jail::Limits {
                timeout: Duration::from_secs(30),
                ..self.limits.clone()
            }),
            ..Job::default()
        };
        let out = self.run(&job)?;

        let mut checks = Vec::new();
        let mut done = false;
        for line in out.stdout.lines() {
            if line == "DONE" {
                done = true;
            } else if let Some(r) = line.strip_prefix("RESULT ") {
                let mut it = r.splitn(3, ' ');
                if let (Some(n), Some(v)) = (it.next(), it.next()) {
                    checks.push(SelfCheck {
                        name: n.to_owned(),
                        passed: v == "PASS",
                        detail: it.next().unwrap_or("").to_owned(),
                    });
                }
            }
        }
        // The host's own view: nothing connected, nothing was written, the victim lives.
        let mut host = |name: &str, ok: bool, why: &str| {
            if !ok && let Some(c) = checks.iter_mut().find(|c| c.name == name) {
                c.passed = false;
                c.detail = why.into();
            }
        };
        host(
            "network_tcp",
            tcp.accept().is_err(),
            "the host's port got a connection",
        );
        host(
            "network_abstract_socket",
            abs.accept().is_err(),
            "the host's abstract socket got a connection",
        );
        host(
            "write_real_home",
            !escape.exists(),
            "a file appeared in the real home",
        );
        let mut victim = victim;
        host(
            "kill_host_parent",
            matches!(victim.0.try_wait(), Ok(None)),
            "the host process died",
        );
        let mut all: Vec<SelfCheck> = Vec::new();
        for name in CHECKS.iter() {
            match checks.iter().find(|c| c.name == *name) {
                Some(c) => all.push(c.clone()),
                None if *name == "runtime_dir_invisible" && runtime.is_none() => {}
                None => all.push(fail(
                    name,
                    if done {
                        "the program did not report this check".to_owned()
                    } else {
                        format!(
                            "the program did not finish (exit {:?}, stderr: {})",
                            out.exit_code,
                            out.stderr.lines().last().unwrap_or("")
                        )
                    },
                )),
            }
        }
        Ok(SelfTest { checks: all })
    }
}
