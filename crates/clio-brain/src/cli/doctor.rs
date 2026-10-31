//! `clio doctor`: Clio's own checks, then the host script of `infra/local/check-host.sh`.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::home::{Config, Dirs, KeyStatus, key_status};
use super::mode;

struct Out {
    fail: bool,
}

impl Out {
    fn say(&mut self, level: &str, msg: String) {
        if level == "FAIL" {
            self.fail = true;
        }
        println!("{level:<5} {msg}");
    }
}

/// Every file and folder under `dir`: folders must be 0700, files and sockets without access for
/// group and others.
fn check_tree(o: &mut Out, dir: &Path, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let Ok(m) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        let md = m.mode() & 0o777;
        if m.is_symlink() {
            continue; // a link's own mode is meaningless; the key file is checked on its own
        }
        if m.is_dir() {
            if md & 0o077 != 0 {
                o.say(
                    "FAIL",
                    format!("{} is mode {md:o}, it must be 700 (S-23)", p.display()),
                );
            }
            if depth < 8 {
                check_tree(o, &p, depth + 1);
            }
        } else if md & 0o077 != 0 {
            o.say(
                "FAIL",
                format!("{} is mode {md:o}, it must be 600 (S-23)", p.display()),
            );
        }
    }
}

/// Clio processes listening on a TCP address that is not loopback or tailscale (100.x).
fn public_listeners() -> Vec<String> {
    let mut inode_addr: Vec<(String, String)> = Vec::new();
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != "0A" {
                continue;
            }
            let Some((ip, port)) = f[1].split_once(':') else {
                continue;
            };
            let local = if v6 {
                ip == "00000000000000000000000001000000"
                    || ip.starts_with("0000000000000000FFFF00007F") // ::ffff:127.x
            } else {
                ip.ends_with("7F") || ip.ends_with("64")
            };
            if !local {
                let port = u16::from_str_radix(port, 16).unwrap_or(0);
                inode_addr.push((f[9].to_owned(), format!("{ip}:{port}")));
            }
        }
    }
    let mut out = Vec::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return out;
    };
    for p in procs.filter_map(Result::ok) {
        let path = p.path();
        let Ok(comm) = std::fs::read_to_string(path.join("comm")) else {
            continue;
        };
        if !comm.trim().starts_with("clio") {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(path.join("fd")) else {
            continue;
        };
        for fd in fds.filter_map(Result::ok) {
            let Ok(t) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let t = t.to_string_lossy().into_owned();
            for (inode, addr) in &inode_addr {
                if t == format!("socket:[{inode}]") {
                    out.push(format!(
                        "{} (pid {}) on {addr}",
                        comm.trim(),
                        p.file_name().to_string_lossy()
                    ));
                }
            }
        }
    }
    out
}

fn find_host_check() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLIO_HOST_CHECK") {
        return Some(PathBuf::from(p));
    }
    let rel = Path::new("infra/local/check-host.sh");
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        starts.extend(exe.parent().map(Path::to_path_buf));
    }
    starts.extend(std::env::current_dir().ok());
    starts
        .iter()
        .flat_map(|s| s.ancestors().map(|d| d.join(rel)).collect::<Vec<_>>())
        .find(|c| c.is_file())
}

/// Returns true when every check passed or only warned.
pub fn doctor(dirs: &Dirs) -> bool {
    let mut o = Out { fail: false };
    let mut folders = vec![dirs.config.clone(), dirs.data.clone()];
    folders.extend(dirs.runtime.clone());
    for d in &folders {
        match mode(d) {
            None => o.say(
                "WARN",
                format!("{} does not exist (run `clio init`)", d.display()),
            ),
            Some(m) if m & 0o077 != 0 => {
                o.say(
                    "FAIL",
                    format!("{} is mode {m:o}, it must be 700 (S-23)", d.display()),
                );
            }
            Some(_) => o.say("PASS", format!("{} is private (700)", d.display())),
        }
        check_tree(&mut o, d, 0);
    }
    if dirs.runtime.is_none() {
        o.say(
            "WARN",
            "XDG_RUNTIME_DIR is not set, so the vault sockets cannot be checked".into(),
        );
    }
    match Config::load(dirs) {
        Ok(c) => {
            o.say(
                "PASS",
                format!(
                    "configuration loads ({}/{})",
                    c.model.provider, c.model.model
                ),
            );
            match c.model.command.get(1) {
                Some(script) if !Path::new(script).is_file() => {
                    o.say("FAIL", format!("the model adapter {script} does not exist"));
                }
                _ => {}
            }
        }
        Err(e) => o.say("WARN", e),
    }
    match key_status(dirs) {
        KeyStatus::Env => o.say("PASS", "model key present in OPENCODE_API_KEY".into()),
        KeyStatus::File { open_mode: None } => o.say("PASS", "model key file is 600".into()),
        KeyStatus::File { open_mode: Some(m) } => o.say(
            "FAIL",
            format!(
                "{} is mode {m:o}, others can read the key (chmod 600)",
                dirs.key_file().display()
            ),
        ),
        KeyStatus::Missing => o.say(
            "WARN",
            "no model key (OPENCODE_API_KEY or the key file)".into(),
        ),
    }
    if let Some(rt) = &dirs.runtime
        && let Ok(rd) = std::fs::read_dir(rt)
    {
        {
            for e in rd.filter_map(Result::ok) {
                if e.file_type().is_ok_and(|t| t.is_socket()) {
                    let m = mode(&e.path()).unwrap_or(0o777);
                    if m & 0o077 != 0 {
                        o.say(
                            "FAIL",
                            format!(
                                "socket {} is mode {m:o}, it must be 600",
                                e.path().display()
                            ),
                        );
                    } else {
                        o.say("PASS", format!("socket {} is 600", e.path().display()));
                    }
                }
            }
        }
    }
    let l = public_listeners();
    if l.is_empty() {
        o.say(
            "PASS",
            "no clio process listens outside loopback (S-38)".into(),
        );
    } else {
        o.say(
            "FAIL",
            format!("clio listens outside loopback: {}", l.join(", ")),
        );
    }
    match find_host_check() {
        Some(script) => {
            println!("--- {} ---", script.display());
            match Command::new("bash").arg(&script).output() {
                Ok(out) => {
                    print!("{}", String::from_utf8_lossy(&out.stdout));
                    eprint!("{}", String::from_utf8_lossy(&out.stderr));
                    if !out.status.success() {
                        o.say("FAIL", "the host check reported a FAIL".into());
                    }
                }
                Err(e) => o.say("WARN", format!("could not run the host check: {e}")),
            }
        }
        None => o.say(
            "WARN",
            "infra/local/check-host.sh not found (set CLIO_HOST_CHECK)".into(),
        ),
    }
    println!(
        "{}",
        if o.fail {
            "doctor: FAIL"
        } else {
            "doctor: ok (WARN lines are your call)"
        }
    );
    !o.fail
}
