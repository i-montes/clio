//! `clio pi doctor|install|sessions`: the CLI side of the Pi harness (ADR 0007), plus the pieces
//! `clio init` and `clio doctor` use. The logic lives in `clio_brain::pi`.

use clio_brain::pi::PiPaths;
use clio_brain::pi::install::{self, installed_version, pinned_version};
use clio_brain::pi::sessions::list_sessions;
use clio_brain::pi::text;
use serde_json::json;

use super::home::Dirs;
use super::tui::sanitize;
use super::ui::{Level, Style};
use super::{Args, Res, stamp};

pub fn paths(dirs: &Dirs) -> PiPaths {
    PiPaths::under_data(&dirs.data)
}

fn path_var() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// The Pi checks of `clio doctor`: Node, Pi installed and equal to the pinned version, and that
/// Clio's Pi folder is not the person's own `~/.pi`.
pub fn checks(dirs: &Dirs) -> Vec<(Level, String, Option<String>)> {
    let mut out = Vec::new();
    match install::find_node(&path_var()) {
        Ok(n) => out.push((
            Level::Pass,
            format!(
                "Node {}.{}.{} for Pi ({})",
                n.version.0,
                n.version.1,
                n.version.2,
                n.path.display()
            ),
            None,
        )),
        Err(e) => out.push((
            Level::Warn,
            e.to_string(),
            Some("Pi (the agent harness) needs Node 22.19 or newer.".into()),
        )),
    }
    let p = paths(dirs);
    match pinned_version() {
        Err(e) => out.push((Level::Fail, e.to_string(), None)),
        Ok(want) => {
            let dir = p.install_dir(&want);
            match (installed_version(&dir), install::locate(&p)) {
                (_, Ok(i)) => out.push((
                    Level::Pass,
                    format!("Pi {} installed in {}", i.version, i.dir.display()),
                    None,
                )),
                (Some(v), Err(_)) if v != want => out.push((
                    Level::Fail,
                    format!("{} holds Pi {v} and Clio pins {want}", dir.display()),
                    Some(format!(
                        "Delete {} and run `clio pi install`.",
                        dir.display()
                    )),
                )),
                (_, Err(e)) => out.push((
                    Level::Warn,
                    e.to_string(),
                    Some(
                        "Run `clio init` or `clio pi install` (it needs the network once).".into(),
                    ),
                )),
            }
        }
    }
    let theirs = dirs.home.join(".pi");
    let ours = p.agent_dir();
    if ours == theirs || ours.starts_with(&theirs) || theirs.starts_with(&ours) {
        out.push((
            Level::Fail,
            format!(
                "Clio's Pi directory ({}) is the same as {}",
                ours.display(),
                theirs.display()
            ),
            Some("Clio must not touch the person's ~/.pi (their own Pi uses it).".into()),
        ));
    } else {
        out.push((
            Level::Pass,
            format!("Clio's Pi directory ({}) is not ~/.pi", ours.display()),
            None,
        ));
    }
    out
}

/// What `clio init` prints for Pi. Never fails `init`: a problem is a warning with the fix.
pub fn init_lines(dirs: &Dirs) {
    let st = Style::stdout();
    if std::env::var_os("CLIO_SKIP_PI_INSTALL").is_some() {
        println!(
            "{}  Pi: install skipped (CLIO_SKIP_PI_INSTALL)",
            st.mark(Level::Warn)
        );
        return;
    }
    let p = paths(dirs);
    if let Err(e) = p.ensure() {
        println!("{}  Pi: {e}", st.mark(Level::Warn));
        return;
    }
    let already = install::locate(&p).is_ok_and(|i| install::is_current(&i));
    match install::install(&p, &path_var()) {
        Ok(i) => println!(
            "{}  Pi {} {} {} (0700)",
            st.mark(Level::Pass),
            i.version,
            if already {
                "was already in"
            } else {
                "installed in"
            },
            i.dir.display()
        ),
        Err(e) => println!(
            "{}  Pi was not installed: {}\n      {} try again with `clio pi install`",
            st.mark(Level::Warn),
            sanitize::line(&e.to_string()),
            st.arrow()
        ),
    }
}

/// `clio pi sessions rm`: the named sessions (by id or file name), or every one with `--all`.
/// Says what goes with each one; a copy with changes not applied needs a yes (or `--yes`).
fn remove_sessions(dirs: &Dirs, names: &[String], all: bool, yes: bool) -> Res<i32> {
    let dir = paths(dirs).sessions_dir();
    let list = list_sessions(&dir);
    let chosen: Vec<_> = if all {
        if !names.is_empty() {
            return Err("either name sessions or use --all, not both".into());
        }
        list.sessions.iter().collect()
    } else {
        if names.is_empty() {
            return Err("nothing to delete is named: clio pi sessions rm <id>… (or --all)".into());
        }
        let mut v = Vec::new();
        for n in names {
            let hit = list.sessions.iter().find(|s| {
                s.id == *n
                    || s.path
                        .file_name()
                        .is_some_and(|f| f.to_string_lossy() == *n)
            });
            v.push(hit.ok_or_else(|| format!("there is no session {}", sanitize::line(n)))?);
        }
        v
    };
    if chosen.is_empty() {
        println!("there are no Pi sessions");
        return Ok(0);
    }
    let plans: Vec<_> = chosen
        .iter()
        .map(|s| super::sessions::plan(dirs, &dir, s))
        .collect::<Res<_>>()?;
    for p in &plans {
        println!(
            "{}  →  {}",
            text::line(&p.title, 50),
            super::sessions::describe(p)
        );
    }
    let risky = plans.iter().filter(|p| p.unapplied != Some(0)).count();
    let question = if risky > 0 {
        format!(
            "Delete {} session(s)? {risky} have changes not applied that will be lost",
            plans.len()
        )
    } else {
        format!("Delete {} session(s)?", plans.len())
    };
    if !yes && !super::apply::confirm(&question) {
        println!("nothing was deleted");
        return Ok(0);
    }
    for p in &plans {
        super::sessions::delete(p)?;
    }
    println!(
        "deleted {} session(s): they stay 30 days in the trash, `clio sessions restore` lists them and brings one back",
        plans.len()
    );
    Ok(0)
}

pub fn run(dirs: &Dirs, argv: Vec<String>) -> Res<i32> {
    let mut it = argv.into_iter();
    let sub = it.next().unwrap_or_default();
    let mut args = Args::new(it.collect());
    match sub.as_str() {
        "doctor" => {
            let json = args.flag("--json");
            args.rest()?;
            let cs = checks(dirs);
            let ok = cs.iter().all(|c| c.0 != Level::Fail);
            if json {
                let v: Vec<_> = cs
                    .iter()
                    .map(|(l, m, h)| json!({"level": format!("{l:?}").to_lowercase(), "message": m, "hint": h}))
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"ok": ok, "checks": v}))
                        .unwrap_or_default()
                );
            } else {
                let st = Style::stdout();
                for (l, m, h) in &cs {
                    println!("{}  {}", st.mark(*l), sanitize::line(m));
                    if let (Some(h), true) = (h, *l != Level::Pass) {
                        println!(
                            "{}{} {}",
                            " ".repeat(st.mark_width() + 2),
                            st.dim(st.arrow()),
                            st.dim(&sanitize::line(h))
                        );
                    }
                }
            }
            Ok(i32::from(!ok))
        }
        "install" => {
            args.rest()?;
            let p = paths(dirs);
            let i = install::install(&p, &path_var()).map_err(|e| e.to_string())?;
            println!("Pi {} installed in {}", i.version, i.dir.display());
            Ok(0)
        }
        "sessions" => {
            let json = args.flag("--json");
            let (all, yes) = (args.flag("--all"), args.flag("--yes"));
            let rest = args.rest()?;
            if rest.first().map(String::as_str) == Some("rm") {
                return remove_sessions(dirs, &rest[1..], all, yes);
            }
            if !rest.is_empty() || all {
                return Err(
                    "usage: clio pi sessions [--json] | rm <id>… | rm --all [--yes]".into(),
                );
            }
            let l = list_sessions(&paths(dirs).sessions_dir());
            if json {
                let v: Vec<_> = l
                    .sessions
                    .iter()
                    .map(|s| {
                        json!({"id": s.id, "path": s.path, "cwd": s.cwd, "created_ms": s.created_ms,
                               "last_activity_ms": s.last_activity_ms, "name": s.name, "title": s.title,
                               "messages": s.messages, "approx": s.approx})
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"sessions": v, "warnings": l.warnings}))
                        .unwrap_or_default()
                );
            } else if l.sessions.is_empty() {
                println!("there are no Pi sessions yet");
            } else {
                for s in &l.sessions {
                    println!(
                        "{}  {:>5}{}  {}  {}",
                        stamp(s.last_activity_ms),
                        s.messages,
                        if s.approx { "~" } else { " " },
                        text::line(&s.title, 60),
                        s.cwd
                    );
                }
            }
            for w in &l.warnings {
                eprintln!("warning: {w}");
            }
            Ok(0)
        }
        _ => Err(
            "usage: clio pi doctor [--json] | clio pi install | clio pi sessions [--json]".into(),
        ),
    }
}
