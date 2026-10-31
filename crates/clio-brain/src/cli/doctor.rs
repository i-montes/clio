//! `clio doctor`: Clio's own checks, then the host script of `infra/local/check-host.sh` (run in
//! its embedded mode, which leaves out the checks Clio did itself: each appears once).
//! `collect` gathers the checks as data; the plain command and the TUI both show them.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use super::home::{Config, Dirs, KeyStatus, key_status};
use super::mode;
use super::tui::app::plural;
use super::tui::sanitize;
use super::ui::{Level, Style};

#[derive(Debug, Clone)]
pub struct Check {
    pub level: Level,
    pub msg: String,
    /// What to do about a WARN or a FAIL.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub checks: Vec<Check>,
    /// The host script that ran, if it was found.
    pub host_script: Option<String>,
    /// The jail: whether `run_tests` can be offered, why not, and every self-test check.
    pub jail: Option<serde_json::Value>,
}

impl Report {
    pub fn count(&self, l: Level) -> usize {
        self.checks.iter().filter(|c| c.level == l).count()
    }

    pub fn ok(&self) -> bool {
        self.count(Level::Fail) == 0
    }

    fn say(&mut self, level: Level, msg: String, hint: Option<String>) {
        self.checks.push(Check { level, msg, hint });
    }
}

/// Every file and folder under `dir`: folders must be 0700, files and sockets without access for
/// group and others.
fn check_tree(o: &mut Report, dir: &Path, depth: usize) {
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
                    Level::Fail,
                    format!("{} tiene modo {md:o}, debe ser 700 (S-23)", p.display()),
                    Some(format!("chmod 700 {}", p.display())),
                );
            }
            if depth < 8 {
                check_tree(o, &p, depth + 1);
            }
        } else if md & 0o077 != 0 {
            o.say(
                Level::Fail,
                format!("{} tiene modo {md:o}, debe ser 600 (S-23)", p.display()),
                Some(format!("chmod 600 {}", p.display())),
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

/// The jail's self-test, one line per check. A FAIL here closes `run_tests`.
fn jail_lines(o: &mut Report) {
    use super::jail::{Status, describe};
    let st = super::jail::check_fresh();
    let (bwrap, test) = match &st {
        Status::Unavailable(why) => {
            o.say(
                Level::Warn,
                format!("jaula: run_tests desactivada, {why}"),
                Some(
                    "Hace falta Linux, bubblewrap (bwrap), prlimit (util-linux), python3 y espacios de nombres de usuario sin privilegios; ver docs/uso-local.md.".into(),
                ),
            );
            o.jail = Some(json!({"available": false, "reason": why, "checks": []}));
            return;
        }
        Status::Ready { bwrap, test } | Status::Failed { bwrap, test } => (bwrap, test),
    };
    o.say(
        Level::Pass,
        format!("jaula: bwrap {bwrap} arranca con espacios de nombres sin privilegios"),
        None,
    );
    for c in &test.checks {
        if c.passed {
            o.say(Level::Pass, format!("jaula: {}", describe(c)), None);
        } else {
            o.say(
                Level::Fail,
                format!("jaula: FALLÓ, {} ({})", describe(c), c.detail),
                Some("run_tests queda desactivada hasta que esto pase; no uses la jaula.".into()),
            );
        }
    }
    o.say(
        if st.ready() { Level::Pass } else { Level::Fail },
        if st.ready() {
            "jaula: autoprueba completa, run_tests disponible para el agente".into()
        } else {
            "jaula: la autoprueba falló, run_tests NO se ofrece al agente".into()
        },
        None,
    );
    let checks: Vec<_> = test
        .checks
        .iter()
        .map(|c| json!({"name": c.name, "passed": c.passed, "detail": c.detail}))
        .collect();
    o.jail = Some(json!({"available": st.ready(), "bwrap": bwrap, "checks": checks}));
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

fn host_lines(o: &mut Report, script: &Path) {
    // The script skips what `collect` already checked itself, so nothing appears twice.
    let out = match Command::new("bash")
        .arg(script)
        .env("CLIO_DOCTOR_EMBEDDED", "1")
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            o.say(
                Level::Warn,
                format!("no se pudo correr el chequeo del equipo: {e}"),
                None,
            );
            return;
        }
    };
    let before = o.checks.len();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let (lv, rest) = if let Some(r) = line.strip_prefix("PASS") {
            (Level::Pass, r)
        } else if let Some(r) = line.strip_prefix("WARN") {
            (Level::Warn, r)
        } else if let Some(r) = line.strip_prefix("FAIL") {
            (Level::Fail, r)
        } else {
            continue;
        };
        let hint = (lv != Level::Pass).then(|| {
            "Del chequeo del equipo (infra/local/check-host.sh); ver docs/uso-local.md.".to_owned()
        });
        o.say(lv, rest.trim().to_owned(), hint);
    }
    let host_failed = o.checks[before..].iter().any(|c| c.level == Level::Fail);
    if !out.status.success() && !host_failed {
        o.say(
            Level::Fail,
            "el chequeo del equipo reportó un fallo".into(),
            None,
        );
    }
}

/// Runs every check. The host script's own lines are included as checks.
pub fn collect(dirs: &Dirs) -> Report {
    let mut o = Report::default();
    let mut folders = vec![dirs.config.clone(), dirs.data.clone()];
    folders.extend(dirs.runtime.clone());
    for d in &folders {
        match mode(d) {
            None => o.say(
                Level::Warn,
                format!("{} no existe", d.display()),
                Some("Ejecuta `clio init`.".into()),
            ),
            Some(m) if m & 0o077 != 0 => o.say(
                Level::Fail,
                format!("{} tiene modo {m:o}, debe ser 700 (S-23)", d.display()),
                Some(format!("chmod 700 {}", d.display())),
            ),
            Some(_) => o.say(
                Level::Pass,
                format!("{} es privada (700)", d.display()),
                None,
            ),
        }
        check_tree(&mut o, d, 0);
    }
    if dirs.runtime.is_none() {
        o.say(
            Level::Warn,
            "XDG_RUNTIME_DIR no está definida, no se pueden revisar los sockets de la bóveda"
                .into(),
            Some("Entra por una sesión normal (login) para que exista.".into()),
        );
    }
    match Config::load(dirs) {
        Ok(c) => {
            o.say(
                Level::Pass,
                format!(
                    "la configuración carga ({}/{})",
                    c.model.provider, c.model.model
                ),
                None,
            );
            if let Some(script) = c.model.command.get(1)
                && !Path::new(script).is_file()
            {
                o.say(
                    Level::Fail,
                    format!("el adaptador del modelo {script} no existe"),
                    Some("Corrige model.command en ~/.config/clio/config.json.".into()),
                );
            }
        }
        Err(e) => o.say(Level::Warn, e, Some("Ejecuta `clio init`.".into())),
    }
    match key_status(dirs) {
        KeyStatus::Env => o.say(
            Level::Pass,
            "clave del modelo en OPENCODE_API_KEY".into(),
            None,
        ),
        KeyStatus::File { open_mode: None } => {
            o.say(Level::Pass, "el archivo de la clave es 600".into(), None);
        }
        KeyStatus::File { open_mode: Some(m) } => o.say(
            Level::Fail,
            format!(
                "{} tiene modo {m:o}, otros pueden leer la clave",
                dirs.key_file().display()
            ),
            Some(format!("chmod 600 {}", dirs.key_file().display())),
        ),
        KeyStatus::Missing => o.say(
            Level::Warn,
            "no hay clave del modelo".into(),
            Some(format!(
                "Ponla en OPENCODE_API_KEY o en {} (chmod 600).",
                dirs.key_file().display()
            )),
        ),
    }
    if let Some(rt) = &dirs.runtime
        && let Ok(rd) = std::fs::read_dir(rt)
    {
        for e in rd.filter_map(Result::ok) {
            if e.file_type().is_ok_and(|t| t.is_socket()) {
                let m = mode(&e.path()).unwrap_or(0o777);
                if m & 0o077 != 0 {
                    o.say(
                        Level::Fail,
                        format!(
                            "el socket {} tiene modo {m:o}, debe ser 600",
                            e.path().display()
                        ),
                        Some(format!("chmod 600 {}", e.path().display())),
                    );
                } else {
                    o.say(
                        Level::Pass,
                        format!("el socket {} es 600", e.path().display()),
                        None,
                    );
                }
            }
        }
    }
    let l = public_listeners();
    if l.is_empty() {
        o.say(
            Level::Pass,
            "ningún proceso clio escucha fuera de loopback (S-38)".into(),
            None,
        );
    } else {
        o.say(
            Level::Fail,
            format!("clio escucha fuera de loopback: {}", l.join(", ")),
            Some("Detén ese proceso: Clio no debe escuchar en una dirección pública.".into()),
        );
    }
    jail_lines(&mut o);
    match find_host_check() {
        Some(script) => {
            o.host_script = Some(script.display().to_string());
            host_lines(&mut o, &script);
        }
        None => o.say(
            Level::Warn,
            "no se encontró infra/local/check-host.sh".into(),
            Some("Define CLIO_HOST_CHECK con la ruta del script.".into()),
        ),
    }
    o
}

fn level_code(l: Level) -> &'static str {
    match l {
        Level::Pass => "pass",
        Level::Warn => "warn",
        Level::Fail => "fail",
    }
}

/// The plain command. Returns true when nothing failed (WARN is the person's call).
pub fn doctor(dirs: &Dirs, json: bool) -> bool {
    let st = Style::stdout();
    let r = collect(dirs);
    if json {
        let checks: Vec<_> = r
            .checks
            .iter()
            .map(|c| json!({"level": level_code(c.level), "message": c.msg, "hint": c.hint}))
            .collect();
        let v = json!({
            "ok": r.ok(),
            "pass": r.count(Level::Pass),
            "warn": r.count(Level::Warn),
            "fail": r.count(Level::Fail),
            "host_script": r.host_script,
            "jail": r.jail,
            "checks": checks,
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return r.ok();
    }
    for c in &r.checks {
        println!("{}  {}", st.mark(c.level), sanitize::line(&c.msg));
        if let (Some(h), true) = (&c.hint, c.level != Level::Pass) {
            println!(
                "{}{} {}",
                " ".repeat(st.mark_width() + 2),
                st.dim(st.arrow()),
                st.dim(&sanitize::line(h))
            );
        }
    }
    println!();
    println!(
        "{}",
        if r.ok() {
            st.green(&format!(
                "doctor: bien ({} ok, {}; los avisos son decisión tuya)",
                r.count(Level::Pass),
                plural(r.count(Level::Warn), "aviso", "avisos")
            ))
        } else {
            st.red(&format!(
                "doctor: hay {} ({} ok, {})",
                plural(r.count(Level::Fail), "fallo", "fallos"),
                r.count(Level::Pass),
                plural(r.count(Level::Warn), "aviso", "avisos")
            ))
        }
    );
    r.ok()
}
