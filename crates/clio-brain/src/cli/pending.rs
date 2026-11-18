//! Host mode from the terminal: what waits for the person's approval (`clio pending`), saying yes
//! or no to it (`clio approve`, `clio reject`), and the log of what was run (`clio actions`).
//! What is set aside comes from the toolhost (`clio_toolhost::pending`); here it is only shown,
//! and an approval runs in a host-mode `ToolHost` on that session's run folder.

use std::path::PathBuf;

use clio_brain::pi::launch::valid_id;
use clio_toolhost::ask::HostRules;
use clio_toolhost::pending::{self, Pending, State};
use clio_toolhost::{Config, HostCfg, JailMode, ToolHost};
use clio_vault::Scrubber;
use serde_json::Value;

use super::home::Dirs;
use super::tui::sanitize;
use super::{Res, io_err, mkdir_private, now_ms, stamp};

/// `Dirs.data/host`: the action log and what is set aside.
pub fn state(dirs: &Dirs) -> PathBuf {
    dirs.data.join("host")
}

/// What waits for the person, oldest first: one past its time does not wait any more.
pub fn waiting(dirs: &Dirs) -> Vec<Pending> {
    let now = now_ms();
    pending::list(&state(dirs))
        .into_iter()
        .filter(|p| p.state == State::Waiting && now < p.expires_ms)
        .collect()
}

/// What `p` does, for the person: from its arguments (`pending::shown`), key-looking values struck
/// out. Never the stored `what`.
pub fn what(p: &Pending) -> String {
    Scrubber::default().scrub(pending::shown(p))
}

/// Its class in «», and why it waits when the class alone does not say it.
pub fn label(p: &Pending) -> String {
    match &p.reason {
        Some(r) => format!("«{} · {}»", p.class, sanitize::line(r)),
        None => format!("«{}»", p.class),
    }
}

/// One line for the person: id, class, what, since when, until when, and whether it was asked.
pub fn line(p: &Pending, now: i64) -> String {
    format!(
        "{}  {}  {}  · desde {} UTC · {} {} UTC{}",
        p.id,
        label(p),
        sanitize::line(&what(p)),
        stamp(p.created_ms),
        if now >= p.expires_ms {
            "venció"
        } else {
            "vence"
        },
        stamp(p.expires_ms),
        if p.ask {
            ""
        } else {
            " · sin preguntar (va al resumen del día)"
        }
    )
}

pub fn show(dirs: &Dirs, json: bool) -> Res<()> {
    let list = waiting(dirs);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if list.is_empty() {
        println!("no hay nada esperando tu visto bueno");
        return Ok(());
    }
    let now = now_ms();
    for p in &list {
        println!("{}", line(p, now));
    }
    println!("\nclio approve <id> lo ejecuta · clio reject <id> lo descarta");
    Ok(())
}

/// A host-mode `ToolHost` on the run folder of `run`, as its session had it (same folders, same
/// rules), with nothing of the session's own: an approval does not mark the running session.
fn host_for(dirs: &Dirs, run: &str) -> Res<ToolHost> {
    if !valid_id(run) {
        return Err(format!("la sesión {run:?} no es válida"));
    }
    let dir = dirs.runs.join(run);
    let scratch = dir.join("scratch");
    mkdir_private(&scratch)?;
    let mut rules = HostRules::factory();
    if let Some(f) = dirs.host_rules() {
        let text = std::fs::read_to_string(&f).map_err(|e| io_err(&f, &e))?;
        rules = rules.with_file(&text)?;
    }
    let mut cfg = Config::new(&dirs.home, &scratch, &dir.join("toolhost"));
    // As `Toolhost::start`: `CLIO_JAIL=off` leaves out commands, and the launcher is not looked for.
    if std::env::var("CLIO_JAIL").is_ok_and(|v| v == "off") {
        cfg.jail = JailMode::Off;
    }
    cfg.scrubber = Some(super::debug::scrubber(dirs));
    cfg.host = Some(HostCfg {
        home: dirs.home.clone(),
        state: state(dirs),
        control: dirs.control(),
        rules,
        run: run.into(),
        launcher: None,
    });
    ToolHost::new(cfg)
}

/// What an approved action did: what it was, its exit code, the tail of its output.
pub fn outcome(p: &Pending) -> String {
    let what = what(p);
    match &p.state {
        State::Done {
            exit_code,
            output_tail,
        } => {
            let code = exit_code.map_or_else(String::new, |c| format!(" (salida {c})"));
            let tail = output_tail.trim();
            if tail.is_empty() {
                format!("{what}{code}")
            } else {
                format!("{what}{code}\n{tail}")
            }
        }
        _ => what,
    }
}

/// Runs `id` as it was set aside. The person answered: it does not count as one of the day's
/// questions.
pub fn approve(dirs: &Dirs, id: &str) -> Res<Pending> {
    // Before the toolhost is built (folders, the launcher's self-test): one that does not wait
    // stops here.
    let run = pending::check(&state(dirs), id, now_ms())?.run;
    let done = host_for(dirs, &run)?.approve(id, now_ms())?;
    let _ = pending::answered_at_terminal(&state(dirs), id);
    Ok(done)
}

pub fn reject(dirs: &Dirs, id: &str) -> Res<Pending> {
    let p = pending::reject(&state(dirs), id)?;
    let _ = pending::answered_at_terminal(&state(dirs), id);
    Ok(p)
}

/// `clio approve ID`.
pub fn approve_cmd(dirs: &Dirs, id: &str) -> Res<()> {
    let p = approve(dirs, id)?;
    println!("aprobado {}: {}", p.id, sanitize::multiline(&outcome(&p)));
    Ok(())
}

/// `clio reject ID`.
pub fn reject_cmd(dirs: &Dirs, id: &str) -> Res<()> {
    let p = reject(dirs, id)?;
    println!(
        "rechazado {}: {} no se ejecuta",
        p.id,
        sanitize::line(&what(&p))
    );
    Ok(())
}

/// The action log of `run` (all of it with `None`), the newest last.
pub fn action_log(dirs: &Dirs, run: Option<&str>) -> Res<Vec<Value>> {
    let file = state(dirs).join("actions.jsonl");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io_err(&file, &e)),
    };
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| run.is_none_or(|r| v["run"] == r))
        .collect())
}

/// One entry of the action log for the person: when, run (with `with_run`), tool, outcome, what,
/// exit code.
pub fn action_line(v: &Value, with_run: bool) -> String {
    let s = |v: &Value| sanitize::line(v.as_str().unwrap_or(""));
    let code = v["exit_code"]
        .as_i64()
        .map_or_else(String::new, |c| format!(" (salida {c})"));
    let run = if with_run {
        format!("  {}", s(&v["run"]))
    } else {
        String::new()
    };
    format!(
        "{} UTC{run}  {:<8} {:<5} {}{code}{}",
        stamp(v["at_ms"].as_i64().unwrap_or(0)),
        s(&v["tool"]),
        s(&v["outcome"]),
        s(&v["what"]),
        if v["network"] == true { " · red" } else { "" }
    )
}

/// The local clock's offset from UTC, in seconds: read from the same local stamp the chat sends
/// in `<clio-contexto>` (UTC when the system cannot say).
pub fn local_offset() -> i64 {
    let s = clio_brain::pi::envelope::stamp_now();
    let tz = s.get(s.len().saturating_sub(6)..).unwrap_or("");
    let n = |r: std::ops::Range<usize>| tz.get(r).and_then(|x| x.parse::<i64>().ok());
    match (tz.as_bytes().first(), n(1..3), n(4..6)) {
        (Some(b'-'), Some(h), Some(m)) => -(h * 3600 + m * 60),
        (Some(b'+'), Some(h), Some(m)) => h * 3600 + m * 60,
        _ => 0,
    }
}

/// One entry of the action log for the chat's box: the local time (`HH:MM:SS`, `offset` seconds
/// from UTC), the tool, the outcome, and what it did with its exit code.
pub fn action_cells(v: &Value, offset: i64) -> [String; 4] {
    let s = |v: &Value| sanitize::line(v.as_str().unwrap_or(""));
    let t = (v["at_ms"].as_i64().unwrap_or(0).div_euclid(1000) + offset).rem_euclid(86_400);
    let code = v["exit_code"]
        .as_i64()
        .map_or_else(String::new, |c| format!(" (salida {c})"));
    [
        format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60),
        s(&v["tool"]),
        s(&v["outcome"]),
        format!(
            "{}{code}{}",
            s(&v["what"]),
            if v["network"] == true { " · red" } else { "" }
        ),
    ]
}

/// `clio actions [--run ID] [--json]`: the action log, the newest last.
pub fn actions(dirs: &Dirs, run: Option<&str>, json: bool) -> Res<()> {
    let lines = action_log(dirs, run)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&lines).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if lines.is_empty() {
        println!("no hay acciones registradas");
    }
    for v in &lines {
        println!("{}", action_line(v, true));
    }
    Ok(())
}
