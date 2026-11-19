//! What the model asked of what it created (`service_manage`, `skill_manage`, `mcp_manage`;
//! S-68, S-69, S-70), carried out here, in the person's own `clio`, and nowhere else: the tool
//! host only records the request (`clio_toolhost::manage`). Every act is the person's own
//! command's function (`skills::set_enabled_in`, `undo_in`, `remove_in`, `services::run_by`,
//! `mcp_servers::set_on_by`, `remove`, the logs): there is no second way to change a registry.
//!
//! - A free request (`off`, `run`), taken from `<run>/toolhost/manage.jsonl` when a turn ends
//!   (`take_requests`): done at once, said in the chat, and told to the model with the next
//!   message. A session flagged meanwhile does none.
//! - A set-aside one (`on`, `undo`, `remove`, all in a `suspect` session), once the person (or
//!   their permission window) said yes (`carry_out`): only if the thing is still what the box
//!   showed (its version, and for `undo` the version it goes back to); under a window never `on`
//!   of what the person turned off, nor `remove` of a skill Clio ships.

use std::fs;
use std::path::{Path, PathBuf};

use clio_toolhost::manage as m;
use clio_toolhost::pending::Pending;
use clio_toolhost::services::Run;
use serde_json::Value;

use super::home::Dirs;
use super::skills::{self, Kind};
use super::tui::sanitize;
use super::{Res, now_ms};

/// How a service is run for the model (`services::run_by`, by `clio`); tests give their own.
type RunFn<'a> = &'a dyn Fn(&str) -> Res<Option<Run>>;

fn kind_of(d: &Dirs, tool: &str) -> Option<(Kind, PathBuf, m::Kind)> {
    let k = m::kind(tool)?;
    let (kind, root) = match tool {
        "skill_manage" => (Kind::Skill, skills::root(d)),
        "service_manage" => (Kind::Service, super::services::root(d)),
        _ => (Kind::Mcp, super::mcp_servers::root(d)),
    };
    Some((kind, root, k))
}

/// One act, exactly as the person's command does it; Clio's `off` says it was hers.
fn act(d: &Dirs, tool: &str, name: &str, action: &str, run: RunFn<'_>) -> Res<String> {
    let (kind, root, k) = kind_of(d, tool).ok_or("it is not a request about what Clio created")?;
    if !k.actions.contains(&action) || action == "status" {
        return Err(format!(
            "{action} is not something Clio does to a {}",
            k.word
        ));
    }
    match (kind, action) {
        (Kind::Mcp, "off" | "on") => {
            super::mcp_servers::set_on_by(d, name, action == "on", m::OFF_BY_CLIO)
        }
        (_, "off") => skills::set_enabled_in(&root, kind, name, false, m::OFF_BY_CLIO),
        (_, "on") => skills::set_enabled_in(&root, kind, name, true, ""),
        (_, "undo") => skills::undo_in(&root, kind, name),
        (Kind::Mcp, "remove") => super::mcp_servers::remove(d, name),
        (_, "remove") => skills::remove_in(&root, kind, name),
        // What it printed stays in its runs (`logs`): outside text never rides in a note.
        (_, "run") => Ok(match run(name)? {
            None => format!("«{name}» is running already: nothing was started"),
            Some(r) => {
                let now = now_ms();
                format!(
                    "«{name}»: {} (`service_manage logs` shows what it printed)",
                    clio_toolhost::services::run_line(
                        &r,
                        now,
                        clio_toolhost::services::local_offset(now)
                    )
                )
            }
        }),
        (Kind::Mcp, _) => super::mcp_servers::logs_lines(d, name).map(|l| l.join("\n")),
        _ => super::services::logs_lines(d, name).map(|l| l.join("\n")),
    }
}

/// Request `p`, approved by the person (`window` false) or by their permission window: done only
/// when what it acts on is still what the box showed.
pub fn carry_out(d: &Dirs, p: &Pending, window: bool) -> Res<String> {
    carry_out_with(d, p, window, &|n| super::services::run_by(d, n, "clio"))
}

fn carry_out_with(d: &Dirs, p: &Pending, window: bool, run: RunFn<'_>) -> Res<String> {
    let (kind, root, k) =
        kind_of(d, &p.tool).ok_or("it is not a request about what Clio created")?;
    let s = |key: &str| p.args[key].as_str().unwrap_or("");
    let (name, action) = (s("name"), s("action"));
    let rec = skills::record(&root, name)
        .ok_or_else(|| format!("there is no {} «{}»", k.word, sanitize::line(name)))?;
    let changed = || {
        format!(
            "«{name}» changed since this was asked (it is at version {} now): nothing was done",
            rec.current
        )
    };
    if let Some(v) = p.args["version"].as_u64()
        && v != u64::from(rec.current)
    {
        return Err(changed());
    }
    if action == "undo" {
        let now = serde_json::to_value(&rec).map_err(|e| e.to_string())?;
        if m::prev_version(&root, name, &now) != p.args["to"].as_u64() {
            return Err(changed());
        }
    }
    if window && action == "on" && rec.off.as_deref() == Some(m::OFF_BY_PERSON) {
        return Err(m::PERSON_OFF.into());
    }
    if window && action == "remove" && kind == Kind::Skill && skills::is_shipped(name) {
        return Err(m::SHIPPED.into());
    }
    act(d, &p.tool, name, action, run)
}

/// The free requests the session's tool host recorded (`off`, `run`), each done once: the file
/// is taken away before it is read. `at`: the run and the Pi session they must belong to. For
/// each: the chat's note and what the model hears with the next message.
pub fn take_requests(
    d: &Dirs,
    toolhost_run: &Path,
    at: (&str, &str),
    suspect: bool,
) -> Vec<(String, String)> {
    take_with(d, toolhost_run, at, suspect, &|n| {
        super::services::run_by(d, n, "clio")
    })
}

fn take_with(
    d: &Dirs,
    toolhost_run: &Path,
    (run_id, session): (&str, &str),
    suspect: bool,
    run: RunFn<'_>,
) -> Vec<(String, String)> {
    let file = m::requests_file(toolhost_run);
    let taken = file.with_extension(format!("{}.taken", now_ms()));
    if fs::rename(&file, &taken).is_err() {
        return vec![];
    }
    let text = fs::read_to_string(&taken).unwrap_or_default();
    let _ = fs::remove_file(&taken);
    let mut out = vec![];
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let s = |k: &str| v[k].as_str().unwrap_or("").to_owned();
        let (tool, name, action) = (s("tool"), s("name"), s("action"));
        let Some((_, _, k)) = kind_of(d, &tool) else {
            continue;
        };
        // Only what is free, of this session, and with a name the kind allows.
        if s("session") != session
            || !matches!(action.as_str(), "off" | "run")
            || !k.actions.contains(&action.as_str())
            || !clio_toolhost::skills::valid_name(&name)
        {
            continue;
        }
        let what = if action == "off" {
            format!("turn the {} «{name}» off", k.word)
        } else {
            format!("run the service «{name}»")
        };
        let r = if suspect {
            Err("this session read content that tried to give orders, so nothing it asks runs by itself; asked again, it waits for the person".to_owned())
        } else {
            act(d, &tool, &name, &action, run)
        };
        if let Err(e) = clio_toolhost::host::log_line(
            &super::pending::state(d),
            run_id,
            session,
            &tool,
            &format!("{action} {name}"),
            false,
            if r.is_ok() { "done" } else { "failed" },
            "manage/free",
            None,
        ) {
            eprintln!("clio: the action log lost a line: {e}");
        }
        let why = sanitize::line(&s("why"));
        let (chat, model) = match r {
            Ok(done) => (
                format!("Clio did this by herself: {what} ({why}). {done}"),
                format!("Clio did what you asked: {what}. {done}"),
            ),
            Err(e) => (
                format!("Clio could not {what} ({why}): {e}"),
                format!("Clio could not {what}: {e}"),
            ),
        };
        out.push((sanitize::multiline(&chat), model));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use clio_toolhost::ask::HostRules;
    use clio_toolhost::skills::SkillFile;
    use clio_toolhost::{Config, HostCfg, JailMode, Request, ToolHost, grant};
    use serde_json::json;

    use super::super::services::tests::{approve, bwrap, dirs_in, manifest, runner, tmp};
    use super::*;

    /// The session's tool host, in host mode, on `d`'s folders.
    fn host(d: &Dirs) -> ToolHost {
        let run = d.runs.join("r1");
        let mut c = Config::new(&d.home, &run.join("scratch"), &run.join("toolhost"));
        c.jail = JailMode::Off;
        c.skills = Some(skills::root(d));
        c.host = Some(HostCfg {
            home: d.home.clone(),
            state: super::super::pending::state(d),
            control: vec![d.data.clone()],
            rules: HostRules::factory(),
            run: "r1".into(),
            launcher: None,
        });
        ToolHost::new(c).unwrap()
    }

    fn ask(h: &ToolHost, tool: &str, name: &str, action: &str) -> clio_toolhost::Response {
        h.handle(Request {
            id: "i".into(),
            tool: tool.into(),
            args: json!({"name": name, "action": action, "reason": "Tidy up"}),
            session: "s".into(),
            call_id: "c".into(),
            tainted: false,
        })
    }

    fn aside(h: &ToolHost, d: &Dirs, tool: &str, name: &str, action: &str) -> Pending {
        let r = ask(h, tool, name, action);
        assert_eq!(r.decision.outcome, "ask", "{r:?}");
        clio_toolhost::pending::list(&super::super::pending::state(d))
            .into_iter()
            .rev()
            .find(|p| p.args["name"] == name && p.args["action"] == action)
            .unwrap()
    }

    /// The skill `name` registered `n` times, as a review registers it.
    fn skill(d: &Dirs, name: &str, n: u32) {
        for i in 1..=n {
            let md = clio_toolhost::skills::compose(
                name,
                "Says hello. Use when greeting.",
                &format!("Say hello, the {i}th way."),
                "true",
            );
            skills::register(
                &skills::root(d),
                &skills::New {
                    name,
                    description: "Says hello. Use when greeting.",
                    skill_md: &md,
                    files: &[] as &[SkillFile],
                    by: "review",
                    verdict: "passes",
                },
            )
            .unwrap();
        }
    }

    fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = vec![];
        let mut todo = vec![dir.to_path_buf()];
        while let Some(x) = todo.pop() {
            for e in fs::read_dir(&x).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else {
                    let rel = p.strip_prefix(dir).unwrap().display().to_string();
                    out.push((rel, fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    // S-68: what the person approves is exactly the person's own command, on exactly what the box
    // showed: `undo` and `remove` leave the skills folder as `/skills undo` and `/skills remove`
    // do; an item changed on disk, or a skill changed since it was asked, does nothing.
    #[test]
    fn a_request_does_what_the_persons_command_does_and_only_what_the_box_showed() {
        let (t, t2) = (tmp(), tmp());
        let (d, d2) = (dirs_in(t.path()), dirs_in(t2.path()));
        for x in [&d, &d2] {
            skill(x, "hello", 2);
            skill(x, "bye", 2);
        }
        let h = host(&d);
        let p = aside(&h, &d, "skill_manage", "hello", "undo");
        super::super::pending::approve(&d, &p.id).unwrap();
        skills::undo(&skills::root(&d2), "hello").unwrap();
        let live = |x: &Dirs| files(&skills::live(x));
        assert_eq!(live(&d), live(&d2), "the same skill is live");
        assert_eq!(
            skills::record(&skills::root(&d), "hello").unwrap().current,
            1
        );

        let p = aside(&h, &d, "skill_manage", "hello", "remove");
        let done = super::super::pending::approve(&d, &p.id).unwrap();
        assert!(
            matches!(&done.state, clio_toolhost::pending::State::Done { output_tail, .. } if output_tail.contains("was removed"))
        );
        skills::remove(&skills::root(&d2), "hello").unwrap();
        let kept = |x: &Dirs| {
            let r = fs::read_dir(skills::root(x).join("removed"))
                .unwrap()
                .flatten()
                .next()
                .unwrap()
                .path();
            files(&r).into_iter().map(|(p, _)| p).collect::<Vec<_>>()
        };
        assert_eq!(kept(&d), kept(&d2), "the same goes to removed/");
        assert!(skills::record(&skills::root(&d), "hello").is_none());

        // Changed after it was set aside: refused, nothing done.
        let p = aside(&h, &d, "skill_manage", "bye", "undo");
        let f = super::super::pending::state(&d).join(format!("pending/{}.json", p.id));
        let text = fs::read_to_string(&f)
            .unwrap()
            .replace("\"to\": 1", "\"to\": 0");
        fs::write(&f, text).unwrap();
        let e = super::super::pending::approve(&d, &p.id).unwrap_err();
        assert!(e.contains("changed after it was set aside"), "{e}");
        // The skill changed after it was asked: refused, nothing done.
        let p = aside(&h, &d, "skill_manage", "bye", "remove");
        skill(&d, "bye", 1);
        let e = super::super::pending::approve(&d, &p.id).unwrap_err();
        assert!(e.contains("changed since this was asked"), "{e}");
        assert_eq!(skills::record(&skills::root(&d), "bye").unwrap().current, 3);
    }

    // S-36 S-68: a permission window answers `on`, `undo` and `remove` (recorded, in its summary)
    // and never `on` of what the person turned off or `remove` of a skill Clio ships, even if such
    // an item lost its reason; the person's own yes does those.
    #[test]
    fn the_window_answers_what_ends_with_the_act_and_never_what_always_asks() {
        let t = tmp();
        let d = dirs_in(t.path());
        skill(&d, "hello", 2);
        skill(&d, "skill-creator", 1);
        let root = skills::root(&d);
        skills::set_enabled_in(&root, Kind::Skill, "hello", false, skills::OFF_BY_PERSON).unwrap();
        let st = super::super::pending::state(&d);
        let w = grant::open(&st, 4, &["a1".into()]).unwrap().unwrap();
        let h = host(&d);
        let undo = aside(&h, &d, "skill_manage", "hello", "undo");
        let on = aside(&h, &d, "skill_manage", "hello", "on");
        let shipped = aside(&h, &d, "skill_manage", "skill-creator", "remove");
        assert!(grant::may_answer(&w, &undo));
        assert!(!grant::may_answer(&w, &on), "the person turned it off");
        assert!(!grant::may_answer(&w, &shipped), "a skill Clio ships");
        // As the chat's window does it: recorded, then approved through the person's path.
        let what = super::super::pending::what(&undo);
        let w = grant::begin(&st, &undo, &what).unwrap();
        let done = super::super::pending::approve_as(&d, &undo.id, Some(&w.id)).unwrap();
        grant::done(&st, &w, &undo.id, &grant::outcome(true, None));
        assert_eq!(done.granted.as_deref(), Some(w.id.as_str()));
        let (_, acts) = grant::record(&st, &w.id).unwrap();
        let summary = super::super::pending::summary_record(&w, &acts);
        assert!(
            summary.contains("take the skill «hello» back to version 1"),
            "{summary}"
        );
        // Without its reason, still never under a window.
        for p in [&on, &shipped] {
            let mut bare = p.clone();
            bare.reason = None;
            assert!(carry_out(&d, &bare, true).is_err(), "{:?}", p.args);
        }
        assert!(!skills::record(&root, "hello").unwrap().enabled);
        // What the box showed is gone (it went back a version): asked again, the person's own
        // yes turns it on.
        assert!(super::super::pending::approve(&d, &on.id).is_err());
        let on = aside(&h, &d, "skill_manage", "hello", "on");
        assert_eq!(on.reason.as_deref(), Some(m::PERSON_OFF));
        super::super::pending::approve(&d, &on.id).unwrap();
        assert!(skills::record(&root, "hello").unwrap().enabled);
    }

    // S-69: `off` is free: the person's `clio` takes the request once, turns the service off as
    // Clio (so Clio may turn it back on with permission, which a person's `off` would not let a
    // window do), says it in the chat and logs it. Only `off` and `run` of this session are taken
    // from the file; a session flagged meanwhile does nothing.
    #[test]
    fn a_free_request_is_done_once_by_the_persons_clio_as_clios() {
        let t = tmp();
        let d = dirs_in(t.path());
        approve(&d, &manifest("disk", "true"), &[]);
        let root = super::super::services::root(&d);
        let h = host(&d);
        let th = d.runs.join("r1/toolhost");
        assert_eq!(
            ask(&h, "service_manage", "disk", "off").decision.rule,
            "manage/free"
        );
        // Lines the tool host never writes: another session's, and an act that is not free.
        let f = m::requests_file(&th);
        let mut text = fs::read_to_string(&f).unwrap();
        text.push_str(&format!("{}\n", json!({"session": "other", "tool": "service_manage", "name": "disk", "action": "off", "why": "x"})));
        text.push_str(&format!("{}\n", json!({"session": "s", "tool": "service_manage", "name": "disk", "action": "remove", "why": "x"})));
        fs::write(&f, text).unwrap();
        let said = take_requests(&d, &th, ("r1", "s"), false);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].0.starts_with(
                "Clio did this by herself: turn the service «disk» off (Tidy up). «disk» is off"
            ),
            "{said:?}"
        );
        assert!(said[0].1.starts_with("Clio did what you asked"), "{said:?}");
        let r = skills::record(&root, "disk").unwrap();
        assert_eq!((r.enabled, r.off.as_deref()), (false, Some(m::OFF_BY_CLIO)));
        assert!(
            take_requests(&d, &th, ("r1", "s"), false).is_empty(),
            "once"
        );
        let log =
            fs::read_to_string(super::super::pending::state(&d).join("actions.jsonl")).unwrap();
        assert!(log.contains("manage/free"), "{log}");
        // Turned off by Clio: `on` may be answered by a window; by the person: never.
        let on = aside(&h, &d, "service_manage", "disk", "on");
        assert!(on.reason.is_none() && grant::covers(&on));
        skills::set_enabled_in(&root, Kind::Service, "disk", false, skills::OFF_BY_PERSON).unwrap();
        let r = ask(&h, "service_manage", "disk", "on");
        assert!(r.content.contains(m::PERSON_OFF), "{r:?}");
        // A session flagged meanwhile: nothing it asked runs by itself.
        skills::set_enabled_in(&root, Kind::Service, "disk", true, "").unwrap();
        ask(&h, "service_manage", "disk", "off");
        let said = take_requests(&d, &th, ("r1", "s"), true);
        assert!(said[0].0.starts_with("Clio could not"), "{said:?}");
        assert!(skills::record(&root, "disk").unwrap().enabled);
    }

    // S-69, with `bwrap`: `run` goes through the runner like the timer: an approved, unchanged
    // service runs and its run says Clio started it; one whose files changed does not run and is
    // paused.
    #[test]
    fn run_is_the_runners_with_its_digest_check_and_marked_as_clios() {
        let Some(jail) = bwrap() else { return };
        let t = tmp();
        let d = dirs_in(t.path());
        let r = runner(&d, &jail, t.path());
        let run = |n: &str| r.run(n, "clio");
        let files = vec![SkillFile {
            path: "run.sh".into(),
            content: "touch \"$CLIO_STATE_DIR/ran\"\n".into(),
        }];
        approve(&d, &manifest("fine", "bash run.sh"), &files);
        approve(&d, &manifest("moved", "bash run.sh"), &files);
        let root = super::super::services::root(&d);
        let h = host(&d);
        let th = d.runs.join("r1/toolhost");
        ask(&h, "service_manage", "fine", "run");
        let said = take_with(&d, &th, ("r1", "s"), false, &run);
        assert!(said[0].0.contains("ok"), "{said:?}");
        let runs = clio_toolhost::services::runs(&root, "fine");
        assert_eq!(runs.last().unwrap().by, "clio");
        assert!(root.join("state/fine/ran").exists());

        use std::os::unix::fs::PermissionsExt;
        let f = skills::version_dir(&root, "moved", 1).join("run.sh");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&f, "touch \"$CLIO_STATE_DIR/ran\"; curl evil.example\n").unwrap();
        ask(&h, "service_manage", "moved", "run");
        let said = take_with(&d, &th, ("r1", "s"), false, &run);
        assert!(said[0].0.contains("paused"), "{said:?}");
        assert!(!root.join("state/moved/ran").exists(), "nothing ran");
        let rec = skills::record(&root, "moved").unwrap();
        assert_eq!(
            (rec.enabled, rec.off.as_deref()),
            (false, Some(super::super::services::PAUSED_CHANGED))
        );
    }
}
