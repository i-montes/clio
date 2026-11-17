//! `clio vault ...`: a thin wrapper that starts and talks to the `clio-vault` binary through its
//! sockets in `$XDG_RUNTIME_DIR/clio/`. Values and the passphrase come from stdin (echo off on a
//! terminal) and are never printed.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clio_vault::{VaultClient, VaultError};

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
                "CLIO_VAULT_BIN debe ser la ruta absoluta de un archivo ({})",
                p.display()
            ))
        };
    }
    exe.and_then(|e| e.parent().map(|d| d.join("clio-vault")))
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            "no encuentro `clio-vault` junto a `clio`: instala los dos juntos (`just install`); no lo busco en el PATH, porque recibe tu frase maestra".into()
        })
}

/// One line from stdin; with echo off when stdin is a terminal.
fn read_secret(prompt: &str) -> Res<String> {
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
    let not_running = || "el baúl no está corriendo: clio vault start".to_owned();
    let p = paths(dirs).map_err(|_| not_running())?;
    if running(&p).is_none() {
        return Err(not_running());
    }
    VaultClient::new(&p.gateway)
        .with_timeout(Duration::from_secs(2))
        .resolve(c.secret, "model", c.domain, "")
        .map(|v| Secret::new(v.expose()))
        .map_err(|e| match e {
            VaultError::Locked => "el baúl está cerrado: clio vault start".to_owned(),
            VaultError::NotFound
            | VaultError::Denied(_)
            | VaultError::Revoked
            | VaultError::Expired => {
                format!("falta la llave de {0} en el baúl: clio connect {0}", c.name)
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
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    (comm.trim() == "clio-vault").then_some(pid)
}

fn start(dirs: &Dirs) -> Res<()> {
    let p = paths(dirs)?;
    mkdir_private(&p.dir)?;
    if let Some(pid) = running(&p) {
        return Err(format!("the vault is already running (pid {pid})"));
    }
    let pass = read_secret("vault passphrase")?;
    let bin = vault_bin()?;
    let s = |x: &std::path::Path| x.display().to_string();
    if !p.vault.exists() {
        println!("creating a new vault at {}", p.vault.display());
        call(&["init".into(), "--vault".into(), s(&p.vault)], Some(&pass))?;
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
    // setsid: the server must outlive this terminal.
    let child = Command::new("setsid")
        .arg(&bin)
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
        .map_err(|e| format!("setsid {}: {e}", bin.display()))?;
    write_private(&p.pid, child.id().to_string().as_bytes())?;
    let t = Instant::now();
    while !p.person.exists() {
        if t.elapsed() > Duration::from_secs(10) {
            return Err(format!("the vault did not come up; see {}", log.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    call(
        &["--sock".into(), s(&p.person), "unlock".into()],
        Some(&pass),
    )?;
    println!("vault running (pid {}), unlocked", child.id());
    Ok(())
}

fn stop(dirs: &Dirs) -> Res<()> {
    let p = paths(dirs)?;
    let Some(pid) = running(&p) else {
        println!("the vault is not running");
        return Ok(());
    };
    let _ = Command::new("kill").arg(pid.to_string()).status();
    let t = Instant::now();
    while std::path::Path::new(&format!("/proc/{pid}")).exists()
        && t.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    for f in [&p.gateway, &p.person, &p.pid] {
        let _ = std::fs::remove_file(f);
    }
    println!("vault stopped");
    Ok(())
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
            let p = paths(dirs)?;
            if running(&p).is_none() {
                return Err("the vault is not running (clio vault start)".into());
            }
            let mut full = vec![
                "--sock".to_owned(),
                p.person.display().to_string(),
                sub.clone(),
            ];
            full.extend(args);
            // For `put` the value is read here (echo off) so the child never touches the terminal.
            let value = match sub.as_str() {
                "put" => Some(read_secret("value")?),
                "upgrade-kdf" => Some(read_secret("vault passphrase")?),
                _ => None,
            };
            call(&full, value.as_deref())
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
        assert!(err.contains("PATH") && err.contains("junto a"), "{err}");
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
