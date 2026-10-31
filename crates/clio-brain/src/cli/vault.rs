//! `clio vault ...`: a thin wrapper that starts and talks to the `clio-vault` binary through its
//! sockets in `$XDG_RUNTIME_DIR/clio/`. Values and the passphrase come from stdin (echo off on a
//! terminal) and are never printed.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::home::Dirs;
use super::{Res, io_err, mkdir_private, write_private};

fn vault_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("CLIO_VAULT_BIN") {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("clio-vault")))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("clio-vault"))
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

fn paths(dirs: &Dirs) -> Res<Paths> {
    let dir = dirs.need_runtime()?.to_path_buf();
    Ok(Paths {
        vault: dirs.data.join("vault.bin"),
        gateway: dir.join("vault-gateway.sock"),
        person: dir.join("vault-person.sock"),
        pid: dir.join("vault.pid"),
        dir,
    })
}

/// Runs `clio-vault` with `args`, feeding `stdin` to it. Its output goes straight to the terminal.
fn call(args: &[String], stdin: Option<&str>) -> Res<()> {
    let bin = vault_bin();
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
    let bin = vault_bin();
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
            "usage: clio vault start|stop|status|put NAME --dest TOOL:DOMAIN|list|revoke NAME"
                .into(),
        );
    }
    let sub = args.remove(0);
    match sub.as_str() {
        "start" => start(dirs),
        "stop" => stop(dirs),
        "status" | "list" | "revoke" | "put" => {
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
            let value = (sub == "put").then(|| read_secret("value")).transpose()?;
            call(&full, value.as_deref())
        }
        other => Err(format!("unknown vault command {other:?}")),
    }
}
