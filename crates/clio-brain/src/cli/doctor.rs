//! `clio doctor`: Clio's own checks, then the host script of `infra/local/check-host.sh` (run in
//! its embedded mode, which leaves out the checks Clio did itself: each appears once).
//! `collect` gathers the checks as data; the plain command and the TUI both show them.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use clio_vault::proto::Request;
use clio_vault::{Scrubber, VaultClient};
use serde_json::json;

use super::home::{Config, Dirs, KeyStatus, key_status};
use super::mode;
use super::tui::app::plural;
use super::tui::sanitize;
use super::ui::{Level, Style};
use super::vault::PERSON_SOCK;

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

/// Folders inside the data folder that hold the person's own files, not what Clio wrote: the
/// copy of a project, the originals `apply` saved, and what npm installed for Pi.
const NOT_CLIO_OUTPUT: [&str; 4] = ["sandbox", "scratch", "backup", "node_modules"];
/// A file bigger than this is not read.
const AUDIT_MAX_BYTES: u64 = 32 << 20;

fn audit_walk(dir: &Path, depth: usize, f: &Scrubber, out: &mut (usize, Vec<PathBuf>, usize)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            let skip = e
                .file_name()
                .to_str()
                .is_some_and(|n| NOT_CLIO_OUTPUT.contains(&n));
            if !skip && depth < 8 {
                audit_walk(&p, depth + 1, f, out);
            }
        } else if t.is_file() {
            if e.metadata().map_or(0, |m| m.len()) > AUDIT_MAX_BYTES {
                out.2 += 1;
                continue;
            }
            let Ok(bytes) = std::fs::read(&p) else {
                out.2 += 1;
                continue;
            };
            out.0 += 1;
            if !f.find(&String::from_utf8_lossy(&bytes)).is_empty() {
                out.1.push(p);
            }
        }
    }
}

/// S-18: nothing shaped like a key may sit in what Clio wrote (run logs, transcripts, Pi's
/// sessions, the memory). Looks for the key shapes the output filter knows; the paths are shown,
/// never the values. The vault's own values are checked by `vault_audit`, which needs its server.
fn data_audit(o: &mut Report, data: &Path) {
    let mut found = (0, Vec::new(), 0);
    audit_walk(data, 0, &Scrubber::default(), &mut found);
    let (read, mut hits, unread) = found;
    hits.sort();
    if hits.is_empty() {
        o.say(
            Level::Pass,
            format!(
                "nada con forma de llave en lo que Clio escribió ({read} archivos revisados, S-18)"
            ),
            None,
        );
    } else {
        let list: Vec<String> = hits
            .iter()
            .take(8)
            .map(|p| sanitize::line(&p.strip_prefix(data).unwrap_or(p).display().to_string()))
            .collect();
        let more = hits.len().saturating_sub(list.len());
        o.say(
            Level::Fail,
            format!(
                "{} archivo(s) de Clio tienen algo con forma de llave (S-18): {}{}",
                hits.len(),
                list.join(", "),
                if more > 0 { format!(" y {more} más") } else { String::new() }
            ),
            Some(
                "Dentro de ~/.local/share/clio. Si es el log de efectos de un run de una versión anterior, borra ese archivo; si no, borra el run (`clio runs`).".into(),
            ),
        );
    }
    if unread > 0 {
        o.say(
            Level::Warn,
            format!(
                "{unread} archivo(s) de Clio no se pudieron revisar (muy grandes o sin permiso)"
            ),
            None,
        );
    }
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

/// Jev is optional: off, it is said and nothing else is checked; on, what it needs is: Python for
/// the adapter and the secret `jev` in the vault, allowed for the API's host and no other.
fn jev_report(o: &mut Report, dirs: &Dirs, jev: &super::home::JevCfg) {
    if !jev.enabled {
        o.say(
            Level::Pass,
            "Jev: apagado (es opcional: todo funciona sin él)".into(),
            None,
        );
        return;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        o.say(
            Level::Warn,
            "Jev: está encendido pero falta python3 para el adaptador; se sigue sin él".into(),
            Some("Instala python3 o apaga Jev (`\"jev\": {\"enabled\": false}` en la configuración).".into()),
        );
        return;
    }
    let ready = super::vault::gateway_socket(dirs).ok().map(|s| {
        VaultClient::new(&s)
            .check("jev", "jev", Some("api.typesafe.ai"), "")
            .map_err(|e| e.to_string())
    });
    match ready {
        Some(Ok(())) => o.say(
            Level::Pass,
            "Jev: encendido; la clave está en el baúl y puede usarse solo hacia api.typesafe.ai".into(),
            None,
        ),
        Some(Err(why)) => o.say(
            Level::Warn,
            format!("Jev: está encendido pero no se puede usar la clave ({})", sanitize::line(&why)),
            Some("Con el baúl en marcha y desbloqueado: `clio vault put jev --dest jev:api.typesafe.ai`. Mientras tanto Clio sigue sin Jev.".into()),
        ),
        None => o.say(
            Level::Warn,
            "Jev: está encendido pero no hay carpeta de ejecución para hablar con el baúl".into(),
            None,
        ),
    }
}

/// A vault made before the cost of the passphrase was raised still opens, but its passphrase is
/// cheaper to guess for whoever gets the file.
fn kdf_warning(status: &serde_json::Value) -> Option<String> {
    (status["kdf_current"] == false).then(|| {
        format!(
            "baúl: la frase del baúl se estira con un coste antiguo ({})",
            sanitize::line(status["kdf"].as_str().unwrap_or("?"))
        )
    })
}

/// The vault's own values (S-18, S-58): the key patterns cannot know a value the vault holds that
/// looks like nothing in particular, so the running vault is asked whether any of its values is in
/// what Clio wrote. The vault answers file, secret name and form, never a value. A vault that is not
/// running, or is locked, does not know its values: that is said, not passed over.
fn vault_audit(o: &mut Report, dirs: &Dirs) {
    let Some(rt) = &dirs.runtime else {
        return; // the missing runtime folder already has its own line
    };
    let client = VaultClient::new(&rt.join(PERSON_SOCK));
    let status = client
        .call(&Request::Status)
        .ok()
        .filter(|r| r.ok)
        .and_then(|r| r.data);
    let Some(status) = status else {
        if dirs.data.join("vault.bin").exists() {
            o.say(
                Level::Warn,
                "baúl: hay un baúl pero no está corriendo, sus valores no se comprobaron en lo que Clio escribió (S-18)".into(),
                Some("`clio vault start` y vuelve a correr `clio doctor`.".into()),
            );
        } else {
            o.say(Level::Pass, "baúl: no hay (es opcional)".into(), None);
        }
        return;
    };
    if let Some(line) = kdf_warning(&status) {
        o.say(
            Level::Warn,
            line,
            Some("`clio vault upgrade-kdf` (pide tu frase; deja una copia del archivo anterior que puedes borrar cuando compruebes el baúl).".into()),
        );
    }
    if status["locked"] == true {
        o.say(
            Level::Warn,
            "baúl: está bloqueado, no conoce sus valores y no puede decir que no estén en lo que Clio escribió (S-18)".into(),
            Some("Desbloquéalo y vuelve a correr `clio doctor`.".into()),
        );
        return;
    }
    let scan = Request::ScanDir {
        path: dirs.data.display().to_string(),
        skip: NOT_CLIO_OUTPUT.map(String::from).to_vec(),
    };
    let hits = match client.call(&scan) {
        Ok(r) if r.ok => r.data.unwrap_or_default(),
        Ok(r) => {
            let why = r.error.unwrap_or_default();
            o.say(
                Level::Warn,
                format!("baúl: no pudo revisar lo que Clio escribió ({why})"),
                None,
            );
            return;
        }
        Err(e) => {
            o.say(
                Level::Warn,
                format!("baúl: no pudo revisar lo que Clio escribió ({e})"),
                None,
            );
            return;
        }
    };
    let mut found: Vec<String> = hits
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let file = Path::new(h["file"].as_str()?);
            let rel = file
                .strip_prefix(&dirs.data)
                .unwrap_or(file)
                .display()
                .to_string();
            Some(format!(
                "{} ({})",
                sanitize::line(&rel),
                sanitize::line(h["name"].as_str()?)
            ))
        })
        .collect();
    found.sort();
    found.dedup();
    if found.is_empty() {
        o.say(
            Level::Pass,
            "baúl: ningún valor del baúl en lo que Clio escribió (S-18)".into(),
            None,
        );
    } else {
        let more = found.len().saturating_sub(8);
        o.say(
            Level::Fail,
            format!(
                "baúl: {} archivo(s) de Clio tienen un valor del baúl (S-18): {}{}",
                found.len(),
                found.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
                if more > 0 { format!(" y {more} más") } else { String::new() }
            ),
            Some("Dentro de ~/.local/share/clio. Borra el run o el archivo y rota ese secreto (`clio vault`): alguien pudo verlo.".into()),
        );
    }
}

/// Pi's own jail (ADR 0009): on Linux Pi starts only inside it.
fn pi_jail_line(o: &mut Report) {
    use clio_brain::pi::jail::{Status, status};
    match status() {
        Status::Ready(v) => o.say(
            Level::Pass,
            format!("jaula de Pi: bwrap {v} arranca; Pi corre sin ver el baúl ni tus archivos y solo alcanza al proveedor del modelo, y la autoprueba corre en cada arranque (ADR 0009)"),
            None,
        ),
        Status::Off => o.say(
            Level::Fail,
            "jaula de Pi: CLIO_PI_JAIL=off, Pi corre con tus mismos permisos y puede ver el baúl y tus archivos".into(),
            Some("Quita CLIO_PI_JAIL=off del entorno.".into()),
        ),
        Status::Unsupported => o.say(
            Level::Warn,
            "jaula de Pi: este sistema no puede tenerla (solo Linux con bubblewrap), Pi corre con tus mismos permisos".into(),
            None,
        ),
        Status::Unavailable(why) => o.say(
            Level::Fail,
            format!("jaula de Pi: no puede arrancar aquí, Pi NO arrancará ({why})"),
            Some("Hace falta bubblewrap (bwrap), prlimit (util-linux) y espacios de nombres de usuario sin privilegios; ver docs/decisions/0009-pi-runs-inside-a-jail.md.".into()),
        ),
    }
}

fn find_host_check() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLIO_HOST_CHECK") {
        return Some(PathBuf::from(p));
    }
    // Only next to the executable, never in the working directory: the script runs outside the jail.
    super::find_above_exe(Path::new("infra/local/check-host.sh"))
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
    data_audit(&mut o, &dirs.data);
    vault_audit(&mut o, dirs);
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
            jev_report(&mut o, dirs, &c.jev);
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
    for (level, msg, hint) in super::pi::checks(dirs) {
        o.say(level, msg, hint);
    }
    jail_lines(&mut o);
    pi_jail_line(&mut o);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jev_is_off_by_default_and_an_old_config_without_it_still_loads() {
        assert!(!super::super::home::JevCfg::default().enabled);
        let mut v = serde_json::to_value(Config::default_with(vec![])).unwrap();
        v.as_object_mut().unwrap().remove("jev");
        let c: Config = serde_json::from_value(v).unwrap();
        assert!(!c.jev.enabled && c.jev.memory && c.jev.tool_hints);
    }

    #[test]
    fn an_old_vault_cost_is_warned_about_and_a_current_one_is_not() {
        let old = serde_json::json!({"kdf_current": false, "kdf": "argon2id m=19 MiB t=2 p=1"});
        assert!(kdf_warning(&old).unwrap().contains("m=19 MiB"));
        assert!(kdf_warning(&serde_json::json!({"kdf_current": true})).is_none());
        assert!(kdf_warning(&serde_json::json!({"locked": false})).is_none());
    }
}
