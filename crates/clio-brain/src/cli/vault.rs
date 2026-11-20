//! `clio vault ...`: a thin wrapper that starts and talks to the `clio-vault` binary through its
//! sockets in `$XDG_RUNTIME_DIR/clio/`. Values and the passphrase come from stdin (echo off on a
//! terminal) and are never printed.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clio_vault::proto::Request;
use clio_vault::{Dest, Kind, SecretValue, VaultClient, VaultError};

use super::home::Dirs;
use super::{Res, io_err, mkdir_private, write_private};
use clio_brain::connector::Connector;
use clio_brain::pi::launch::Secret;

/// The person's socket, inside `$XDG_RUNTIME_DIR/clio/` (`clio doctor` asks the vault through it).
pub(super) const PERSON_SOCK: &str = "vault-person.sock";

/// The binary that receives the master passphrase on its input: next to `clio`, or the absolute path
/// in `CLIO_VAULT_BIN`. Never searched in the PATH: whatever came first there would get the passphrase.
fn vault_bin() -> Res<PathBuf> {
    vault_bin_from(
        std::env::var_os("CLIO_VAULT_BIN"),
        std::env::current_exe().ok(),
    )
}

fn vault_bin_from(env: Option<std::ffi::OsString>, exe: Option<PathBuf>) -> Res<PathBuf> {
    if let Some(p) = env {
        let p = PathBuf::from(p);
        return if p.is_absolute() && p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "CLIO_VAULT_BIN must be the absolute path of a file ({})",
                p.display()
            ))
        };
    }
    exe.and_then(|e| e.parent().map(|d| d.join("clio-vault")))
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            "cannot find `clio-vault` next to `clio`: install both together (`just install`); it is not searched in the PATH, because it receives your master passphrase".into()
        })
}

/// One line from stdin; with echo off when stdin is a terminal.
pub(crate) fn read_secret(prompt: &str) -> Res<String> {
    let tty = std::io::stdin().is_terminal();
    if tty {
        eprint!("{prompt}: ");
        let _ = std::io::stderr().flush();
        let _ = Command::new("stty").arg("-echo").status();
    }
    let mut s = String::new();
    let r = std::io::stdin().lock().read_line(&mut s);
    if tty {
        let _ = Command::new("stty").arg("echo").status();
        eprintln!();
    }
    r.map_err(|e| e.to_string())?;
    Ok(s.trim_end_matches(['\n', '\r']).to_owned())
}

struct Paths {
    dir: PathBuf,
    vault: PathBuf,
    gateway: PathBuf,
    person: PathBuf,
    pid: PathBuf,
}

/// The gateway socket of the vault: where a secret is asked for and text is scrubbed.
pub fn gateway_socket(dirs: &Dirs) -> Res<PathBuf> {
    Ok(paths(dirs)?.gateway)
}

fn paths(dirs: &Dirs) -> Res<Paths> {
    let dir = dirs.need_runtime()?.to_path_buf();
    Ok(Paths {
        vault: dirs.data.join("vault.bin"),
        gateway: dir.join("vault-gateway.sock"),
        person: dir.join(PERSON_SOCK),
        pid: dir.join("vault.pid"),
        dir,
    })
}

/// The model key of connector `c`: its variable if set and not empty (tests, bench and CI), else
/// the vault, which sends it only to `model:<domain>`. Errors never carry the key.
pub fn model_key(dirs: &Dirs, c: &Connector) -> Res<Secret> {
    key_from(std::env::var(c.key_env).ok(), dirs, c)
}

fn key_from(env: Option<String>, dirs: &Dirs, c: &Connector) -> Res<Secret> {
    if let Some(v) = env.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) {
        return Ok(Secret::new(v));
    }
    let not_running = || {
        format!(
            "the vault is not running: clio vault start (and then clio connect {} if you have not stored the key yet)",
            c.name
        )
    };
    let p = paths(dirs).map_err(|_| not_running())?;
    if running(&p).is_none() {
        return Err(not_running());
    }
    VaultClient::new(&p.gateway)
        .with_timeout(Duration::from_secs(2))
        .resolve(c.secret, "model", c.domain, "")
        .map(|v| Secret::new(v.expose()))
        .map_err(|e| match e {
            VaultError::Locked => "the vault is locked: clio vault start".to_owned(),
            VaultError::NotFound
            | VaultError::Denied(_)
            | VaultError::Revoked
            | VaultError::Expired => {
                format!(
                    "the key for {0} is not in the vault: clio connect {0}",
                    c.name
                )
            }
            _ => not_running(),
        })
}

/// Runs `clio-vault` with `args`, feeding `stdin` to it. Its output goes straight to the terminal.
fn call(args: &[String], stdin: Option<&str>) -> Res<()> {
    let bin = vault_bin()?;
    let mut c = Command::new(&bin);
    c.args(args).stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::inherit()
    });
    let mut child = c.spawn().map_err(|e| format!("{}: {e}", bin.display()))?;
    if let (Some(text), Some(mut si)) = (stdin, child.stdin.take()) {
        let _ = writeln!(si, "{text}");
    }
    let st = child.wait().map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("clio-vault exited with {st}"))
    }
}

fn running(p: &Paths) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(&p.pid).ok()?.trim().parse().ok()?;
    (comm(pid)? == "clio-vault").then_some(pid)
}

/// The command name of process `pid`, `None` when there is no such process.
fn comm(pid: u32) -> Option<String> {
    if cfg!(target_os = "linux") {
        let c = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        return Some(c.trim().to_owned());
    }
    // No /proc on macOS: ps gives the command's path.
    let o = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let c = String::from_utf8_lossy(&o.stdout);
    let name = c.trim().rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// Where the vault is: `clio setup` decides from this whether to ask for a new passphrase or the
/// existing one, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No vault file yet.
    Missing,
    /// The file, and no server.
    Stopped,
    Locked,
    Open,
}

pub fn state(dirs: &Dirs) -> State {
    let Ok(p) = paths(dirs) else {
        return if dirs.data.join("vault.bin").exists() {
            State::Stopped
        } else {
            State::Missing
        };
    };
    if running(&p).is_some() {
        return match locked(&p) {
            Some(false) => State::Open,
            _ => State::Locked,
        };
    }
    if p.vault.exists() {
        State::Stopped
    } else {
        State::Missing
    }
}

fn start(dirs: &Dirs) -> Res<()> {
    let p = paths(dirs)?;
    if let Some(pid) = running(&p)
        && locked(&p) != Some(true)
    {
        return Err(format!(
            "the vault is already running (pid {pid}) and unlocked"
        ));
    }
    if !p.vault.exists() {
        println!("creating a new vault at {}", p.vault.display());
    }
    let pass = SecretValue::new(read_secret("vault passphrase")?);
    let pid = start_with(dirs, &pass)?;
    println!("vault running (pid {pid}), unlocked");
    Ok(())
}

/// Creates the vault if there is none, starts its server if it is not running and unlocks it,
/// all with `pass`, which goes from this process to the vault's file or its socket and nowhere
/// else (no argument, no other process's input). Returns the server's pid.
pub fn start_with(dirs: &Dirs, pass: &SecretValue) -> Res<u32> {
    let p = paths(dirs)?;
    mkdir_private(&p.dir)?;
    let wrong = || "could not unlock the vault (wrong passphrase?): clio vault start".to_owned();
    if let Some(pid) = running(&p) {
        // Running but locked (a mistyped passphrase, a `lock`): unlock it here, so every "the vault
        // is locked: clio vault start" is an answer.
        unlock(&p.person, pass).map_err(|_| wrong())?;
        return Ok(pid);
    }
    let bin = vault_bin()?;
    let s = |x: &std::path::Path| x.display().to_string();
    if !p.vault.exists() {
        if let Some(d) = p.vault.parent() {
            mkdir_private(d)?;
        }
        clio_vault::Vault::init(
            Some(&p.vault),
            pass.expose(),
            std::sync::Arc::new(clio_core::SystemClock),
        )
        .map_err(|e| format!("could not create the vault: {e}"))?;
    }
    for sock in [&p.gateway, &p.person] {
        let _ = std::fs::remove_file(sock); // stale socket of a dead server
    }
    let log = p.dir.join("vault.log");
    write_private(&log, b"")?;
    let logf = std::fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .map_err(|e| io_err(&log, &e))?;
    // The server must outlive this terminal: setsid(1) on Linux; macOS has no setsid(1), so there
    // the server gets its own process group, out of reach of the terminal's Ctrl-C and hangup.
    let mut cmd = if cfg!(target_os = "linux") {
        let mut c = Command::new("setsid");
        c.arg(&bin);
        c
    } else {
        let mut c = Command::new(&bin);
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        c
    };
    let mut child = cmd
        .args([
            "serve",
            "--vault",
            &s(&p.vault),
            "--gateway-sock",
            &s(&p.gateway),
            "--person-sock",
            &s(&p.person),
        ])
        .stdin(Stdio::null())
        .stdout(logf.try_clone().map_err(|e| e.to_string())?)
        .stderr(logf)
        .spawn()
        .map_err(|e| format!("starting {}: {e}", bin.display()))?;
    write_private(&p.pid, child.id().to_string().as_bytes())?;
    // A server that did not come up or did not unlock is stopped: no locked orphan stays behind.
    let mut abort = |why: String| {
        let _ = child.kill();
        let _ = child.wait();
        for f in [&p.gateway, &p.person, &p.pid] {
            let _ = std::fs::remove_file(f);
        }
        why
    };
    let t = Instant::now();
    // Up means it accepts: the socket file exists a moment before the server listens on it.
    while std::os::unix::net::UnixStream::connect(&p.person).is_err() {
        if t.elapsed() > Duration::from_secs(10) {
            return Err(abort(format!(
                "the vault did not come up; see {}",
                log.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unlock(&p.person, pass).map_err(|_| abort(wrong()))?;
    Ok(child.id())
}

/// Unlocks the vault on `sock` with `pass`; the request is wiped once sent.
fn unlock(sock: &std::path::Path, pass: &SecretValue) -> Res<()> {
    let mut req = Request::Unlock {
        passphrase: pass.expose().to_owned(),
        eager: Vec::new(),
    };
    let r = VaultClient::new(sock)
        .with_timeout(Duration::from_secs(30))
        .call(&req);
    if let Request::Unlock { passphrase, .. } = &mut req {
        clio_vault::wipe(passphrase);
    }
    match r {
        Ok(resp) if resp.ok => Ok(()),
        Ok(resp) => Err(resp.error.unwrap_or_else(|| "refused".into())),
        Err(e) => Err(e.to_string()),
    }
}

fn stop(dirs: &Dirs) -> Res<()> {
    if stop_server(dirs)? {
        println!("vault stopped");
    } else {
        println!("the vault is not running");
    }
    Ok(())
}

/// Stops the vault's server when it runs; returns whether it did.
pub fn stop_server(dirs: &Dirs) -> Res<bool> {
    let p = paths(dirs)?;
    let Some(pid) = running(&p) else {
        return Ok(false);
    };
    let _ = Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let t = Instant::now();
    while comm(pid).is_some() && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    for f in [&p.gateway, &p.person, &p.pid] {
        let _ = std::fs::remove_file(f);
    }
    Ok(true)
}

fn running_paths(dirs: &Dirs) -> Res<Paths> {
    let p = paths(dirs)?;
    if running(&p).is_none() {
        return Err("the vault is not running (clio vault start)".into());
    }
    Ok(p)
}

/// Runs `clio-vault` against the person's socket. The vault must be running; it may be locked.
fn person_call(dirs: &Dirs, sub: &str, args: Vec<String>, stdin: Option<&str>) -> Res<()> {
    let p = running_paths(dirs)?;
    let mut full = vec![
        "--sock".to_owned(),
        p.person.display().to_string(),
        sub.into(),
    ];
    full.extend(args);
    call(&full, stdin)
}

/// Stores `value` as the secret `name` (replacing one of that name), allowed only for `dest`
/// (`TOOL:DOMAIN`): from this process straight to the vault's person socket, never in an argument
/// or another process's input.
pub fn put(dirs: &Dirs, name: &str, dest: &str, value: &SecretValue) -> Res<()> {
    let p = running_paths(dirs)?;
    let d = Dest::parse(dest).ok_or_else(|| format!("{dest} is not TOOL:DOMAIN"))?;
    VaultClient::new(&p.person)
        .with_timeout(Duration::from_secs(10))
        .put(name, value, Kind::ApiKey, vec![d])
        .map_err(|e| match e {
            VaultError::Locked => "the vault is locked: clio vault start".into(),
            VaultError::InvalidValue => {
                "that is too short to be a secret: nothing was stored".into()
            }
            other => format!("the vault did not store it: {other}"),
        })
}

/// The vault's person socket (whether or not the vault runs): `request_secret` asks it, at request
/// time, only whether it is open and which names it has.
pub fn person_socket(dirs: &Dirs) -> Res<PathBuf> {
    Ok(paths(dirs)?.person)
}

/// Stores a secret the model asked for (`request_secret`) the way `clio vault put` does, a `Put`
/// on the person's socket, but from this process: the value goes from `value` to the socket and
/// nowhere else (no other process, no argument, no file). It never replaces a secret of that name.
pub fn store_requested(dirs: &Dirs, name: &str, dest: &str, value: &SecretValue) -> Res<()> {
    let d = Dest::parse(dest).ok_or_else(|| format!("{dest} is not TOOL:DOMAIN"))?;
    let c = VaultClient::new(&paths(dirs)?.person).with_timeout(Duration::from_secs(10));
    let not_running = |_| "the vault is not running: start it (clio vault start) and answer again";
    let list = c.call(&Request::List).map_err(not_running)?;
    if !list.ok {
        return Err("the vault is locked: unlock it (clio vault start) and answer again".into());
    }
    if list
        .data
        .and_then(|d| d.as_array().cloned())
        .is_some_and(|a| a.iter().any(|m| m["name"] == name))
    {
        return Err(format!(
            "a secret named {name} already exists: nothing was stored"
        ));
    }
    c.put(name, value, Kind::ApiKey, vec![d])
        .map_err(|e| match e {
            VaultError::Locked => {
                "the vault is locked: unlock it (clio vault start) and answer again".into()
            }
            VaultError::InvalidValue => {
                "that is too short to be a secret: nothing was stored".into()
            }
            other => format!("the vault did not store it: {other}"),
        })
}

/// Ok only if the vault is running AND unlocked; the error says which of the two it is.
pub fn unlocked(dirs: &Dirs) -> Res<()> {
    let not_running = || "the vault is not running: clio vault start".to_owned();
    let p = paths(dirs).map_err(|_| not_running())?;
    if running(&p).is_none() {
        return Err(not_running());
    }
    match locked(&p) {
        None => Err(not_running()),
        Some(false) => Ok(()),
        Some(true) => Err("the vault is locked: clio vault start".into()),
    }
}

/// Ok only if the vault answers on its socket and is unlocked (`unlocked` also wants its pid
/// file): what a round needs.
pub fn open_now(dirs: &Dirs) -> Res<()> {
    let not_running = || "the vault is not running: clio vault start".to_owned();
    match locked(&paths(dirs).map_err(|_| not_running())?) {
        Some(false) => Ok(()),
        Some(true) => Err("the vault is locked: clio vault start".into()),
        None => Err(not_running()),
    }
}

/// Whether the running vault is locked; `None` when it does not answer.
fn locked(p: &Paths) -> Option<bool> {
    let status = VaultClient::new(&p.person)
        .with_timeout(Duration::from_secs(2))
        .call(&Request::Status)
        .ok()?;
    Some(status.data.and_then(|d| d["locked"].as_bool()) != Some(false))
}

/// Whether the vault already lets `model` send connector `c`'s secret to its domain. The
/// connector's variable does not count: this is about the vault.
pub fn has_model_key(dirs: &Dirs, c: &Connector) -> bool {
    paths(dirs).is_ok_and(|p| {
        VaultClient::new(&p.gateway)
            .with_timeout(Duration::from_secs(2))
            .check(c.secret, "model", Some(c.domain), "")
            .is_ok()
    })
}

pub fn run(dirs: &Dirs, mut args: Vec<String>) -> Res<()> {
    if args.is_empty() {
        return Err(
            "usage: clio vault start|stop|status|put NAME --dest TOOL:DOMAIN|list|revoke NAME|upgrade-kdf"
                .into(),
        );
    }
    let sub = args.remove(0);
    match sub.as_str() {
        "start" => start(dirs),
        "stop" => stop(dirs),
        "status" | "list" | "revoke" | "put" | "upgrade-kdf" => {
            running_paths(dirs)?; // before asking for a value that would go nowhere
            // For `put` the value is read here (echo off) so the child never touches the terminal.
            let value = match sub.as_str() {
                "put" => Some(read_secret("value")?),
                "upgrade-kdf" => Some(read_secret("vault passphrase")?),
                _ => None,
            };
            person_call(dirs, &sub, args, value.as_deref())
        }
        other => Err(format!("unknown vault command {other:?}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clio_brain::connector::by_name;
    use std::ffi::OsString;
    use std::path::Path;

    fn dirs_in(root: &Path) -> Dirs {
        let data = root.join("data");
        Dirs {
            config: root.join("config"),
            runs: data.join("runs"),
            data,
            runtime: Some(root.join("run/clio")),
            home: root.to_path_buf(),
        }
    }

    #[test]
    fn the_variable_wins_over_the_vault() {
        let d = tempfile::tempdir().unwrap();
        let c = by_name("claude").unwrap();
        // No vault is running here: the answer can only come from the variable.
        let k = key_from(Some("  var-value \n".into()), &dirs_in(d.path()), c).unwrap();
        assert_eq!(k.expose(), "var-value");
        // An empty variable is as good as none.
        assert!(key_from(Some("  ".into()), &dirs_in(d.path()), c).is_err());
    }

    #[test]
    fn no_vault_says_how_to_start_it() {
        let d = tempfile::tempdir().unwrap();
        for dirs in [
            dirs_in(d.path()),
            Dirs {
                runtime: None,
                ..dirs_in(d.path())
            },
        ] {
            let e = key_from(None, &dirs, by_name("opencode").unwrap()).unwrap_err();
            assert!(e.contains("clio vault start"), "{e}");
            assert!(e.contains("clio connect opencode"), "{e}");
            assert!(!e.contains("sk-"), "{e}");
        }
    }

    fn exe_dir_with(vault: bool) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("clio");
        std::fs::write(&exe, "").unwrap();
        if vault {
            std::fs::write(d.path().join("clio-vault"), "").unwrap();
        }
        (d, exe)
    }

    // The vault binary receives the master passphrase on its input: it is found next to `clio`,
    // or by an absolute path the person gave, and never by searching the PATH.
    #[test]
    fn the_vault_binary_is_next_to_clio_and_never_searched_in_the_path() {
        let (d, exe) = exe_dir_with(true);
        assert_eq!(
            vault_bin_from(None, Some(exe)).unwrap(),
            d.path().join("clio-vault")
        );
        let (_d, exe) = exe_dir_with(false);
        let err = vault_bin_from(None, Some(exe)).unwrap_err();
        assert!(err.contains("PATH") && err.contains("next to"), "{err}");
        assert!(vault_bin_from(None, None).is_err());
    }

    #[test]
    fn clio_vault_bin_must_be_an_absolute_path_to_a_file() {
        let (d, exe) = exe_dir_with(true);
        let real = d.path().join("clio-vault");
        let given = |p: &Path| Some(OsString::from(p));
        assert_eq!(
            vault_bin_from(given(&real), Some(exe.clone())).unwrap(),
            real
        );
        for bad in [
            Path::new("clio-vault"),
            Path::new("./x/clio-vault"),
            &d.path().join("nothing"),
        ] {
            let err = vault_bin_from(given(bad), Some(exe.clone())).unwrap_err();
            assert!(err.contains("CLIO_VAULT_BIN"), "{bad:?}: {err}");
        }
    }
}
