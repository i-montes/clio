//! What the model asked of what it created (`service_manage`, `skill_manage`, `mcp_manage`;
//! S-68, S-69, S-70), carried out here, in the person's own `clio`, and nowhere else: the tool
//! host only records the request (`clio_toolhost::manage`). Every act is the person's own
//! command's function (`skills::set_enabled_in`, `undo_in`, `remove_in`, `services::run_by`,
//! `mcp_servers::set_on_by`, `remove`, the logs): there is no second way to change a registry.
//!
//! - A free request (`off`, `run`), taken from `<run>/toolhost/manage.jsonl` when a turn ends
//!   (`take_requests`): an `off` at once, a `run` on a thread of its own (`run_request`); each
//!   said in the chat and told to the model. A session flagged meanwhile does none, nor a line
//!   older than an hour; at most `PER_TAKE` lines and one run per service a take. Done while a
//!   permission window is open, each is in its record (and its summary).
//! - A set-aside one (`on`, `undo`, `remove`, a `run` that reaches out, all in a `suspect`
//!   session), once the person (or their permission window) said yes (`carry_out`): only if the
//!   thing is still what the box showed (its version, for `undo` the version it goes back to, for
//!   `on` who turned it off); under a window never what always asks.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use clio_toolhost::grant;
use clio_toolhost::manage as m;
use clio_toolhost::pending::{Pending, State};
use clio_toolhost::services::Run;
use serde_json::Value;

use super::home::Dirs;
use super::skills::{self, Kind};
use super::tui::sanitize;
use super::{Res, now_ms};

/// How a service is run for the model (`services::run_by`, by `clio`); tests give their own.
type RunFn<'a> = &'a dyn Fn(&str) -> Res<Option<Run>>;

/// Lines one take carries out at most.
pub const PER_TAKE: usize = 20;
/// A line older than this is not carried out.
const TOO_OLD_MS: i64 = 3600 * 1000;
/// The id of a free act in a permission window's record: the chat said it already.
pub const FREE_ACT: &str = "free-";

fn kind_of(d: &Dirs, tool: &str) -> Option<(Kind, PathBuf, m::Kind)> {
    let k = m::kind(tool)?;
    let (kind, root) = match tool {
        "skill_manage" => (Kind::Skill, skills::root(d)),
        "service_manage" => (Kind::Service, super::services::root(d)),
        _ => (Kind::Mcp, super::mcp_servers::root(d)),
    };
    Some((kind, root, k))
}

/// One act, exactly as the person's command does it; Clio's `off` says it was hers, and never
/// writes over why something already off is off (the person's `off`, a pause).
fn act(d: &Dirs, tool: &str, name: &str, action: &str, run: RunFn<'_>) -> Res<String> {
    let (kind, root, k) = kind_of(d, tool).ok_or("it is not a request about what Clio created")?;
    if !k.actions.contains(&action) || action == "status" {
        return Err(format!(
            "{action} is not something Clio does to a {}",
            k.word
        ));
    }
    if action == "off"
        && let Some(r) = skills::record(&root, name)
        && !r.enabled
    {
        return Ok(format!(
            "«{name}» is already off ({})",
            r.off.as_deref().unwrap_or("off")
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
    // Who turned it off is part of what was asked: another `off` since then is another question.
    if action == "on" && rec.off.as_deref().unwrap_or("off") != s("off") {
        return Err(format!(
            "«{name}» changed since you were asked (now it is {}): nothing was done",
            sanitize::line(rec.off.as_deref().unwrap_or("on"))
        ));
    }
    if window {
        let shipped = kind == Kind::Skill && skills::is_shipped(name);
        if action == "on" && rec.off.as_deref() == Some(m::OFF_BY_PERSON) {
            return Err(m::PERSON_OFF.into());
        }
        if matches!(action, "remove" | "undo") && shipped {
            return Err(m::SHIPPED.into());
        }
        if action == "undo" && kind == Kind::Service {
            return Err(m::SERVICE_UNDO.into());
        }
    }
    act(d, &p.tool, name, action, run)
}

/// A free request, as the session's tool host recorded it.
#[derive(Debug, Clone)]
pub struct Request {
    pub tool: String,
    pub name: String,
    pub action: String,
    pub why: String,
    pub at_ms: i64,
}

impl Request {
    fn what(&self, word: &str) -> String {
        if self.action == "off" {
            format!("turn the {word} «{}» off", self.name)
        } else {
            format!("run the service «{}»", self.name)
        }
    }
}

/// The free requests the session's tool host recorded (`off`, `run`), each taken once: the file
/// is taken away before it is read. `at`: the run and the Pi session they must belong to. Done
/// now: `off`, and what is not done (too old, past `PER_TAKE`, a run asked twice, a flagged
/// session), each as (the chat's note, what the model hears). Returned to run on a thread of
/// their own: the runs (`run_request`).
pub fn take_requests(
    d: &Dirs,
    toolhost_run: &Path,
    (run_id, session): (&str, &str),
    suspect: bool,
) -> (Vec<(String, String)>, Vec<Request>) {
    let file = m::requests_file(toolhost_run);
    let taken = file.with_extension(format!("{}.taken", now_ms()));
    if fs::rename(&file, &taken).is_err() {
        return (vec![], vec![]);
    }
    let text = fs::read_to_string(&taken).unwrap_or_default();
    let _ = fs::remove_file(&taken);
    let (mut said, mut runs, mut ran) = (vec![], vec![], HashSet::new());
    let mut left = 0;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let s = |k: &str| v[k].as_str().unwrap_or("").to_owned();
        let r = Request {
            tool: s("tool"),
            name: s("name"),
            action: s("action"),
            why: s("why"),
            at_ms: v["at_ms"].as_i64().unwrap_or(0),
        };
        let Some((kind, root, k)) = kind_of(d, &r.tool) else {
            continue;
        };
        // Only what is free, of this session, and with a name the kind allows.
        if s("session") != session
            || !matches!(r.action.as_str(), "off" | "run")
            || !k.actions.contains(&r.action.as_str())
            || !clio_toolhost::skills::valid_name(&r.name)
        {
            continue;
        }
        if said.len() + runs.len() >= PER_TAKE {
            left += 1;
            continue;
        }
        let not_done = if suspect {
            Some(
                "this session read content that tried to give orders, so nothing it asks runs by itself; asked again, it waits for the person",
            )
        } else if now_ms().saturating_sub(r.at_ms) > TOO_OLD_MS {
            Some("not done: asked too long ago")
        } else if r.action == "run" && !ran.insert(r.name.clone()) {
            Some("not run again: it was already asked to run in this turn")
        } else {
            None
        };
        if let Some(e) = not_done {
            said.push(tell(d, (run_id, session), &r, Err(e.to_owned())));
        } else if r.action == "run" {
            runs.push(r);
        } else {
            let was_on = skills::record(&root, &r.name).is_some_and(|x| x.enabled);
            let done = act(d, &r.tool, &r.name, &r.action, &|_| Ok(None));
            // A service or a server Clio turned off: the person hears it when a chat opens too.
            if was_on && done.is_ok() && kind != Kind::Skill {
                let why = sanitize::line(&r.why);
                super::services::notice(&root, &r.name, &format!("{}: {why}", m::OFF_BY_CLIO));
            }
            said.push(tell(d, (run_id, session), &r, done));
        }
    }
    if left > 0 {
        let e = format!(
            "{left} more requests were not done: at most {PER_TAKE} are carried out at once"
        );
        said.push((e.clone(), e));
    }
    (said, runs)
}

/// Runs request `r` (a `run` `take_requests` returned), through the runner as the timer does.
pub fn run_request(d: &Dirs, at: (&str, &str), r: &Request) -> (String, String) {
    run_request_with(d, at, r, &|n| super::services::run_by(d, n, "clio"))
}

fn run_request_with(d: &Dirs, at: (&str, &str), r: &Request, run: RunFn<'_>) -> (String, String) {
    let done = act(d, &r.tool, &r.name, &r.action, run);
    tell(d, at, r, done)
}

/// What came of free request `r`: logged, in an open permission window's record, a notice for
/// Clio's `off` of a service or a server, and (the chat's note, what the model hears).
fn tell(
    d: &Dirs,
    (run_id, session): (&str, &str),
    r: &Request,
    done: Res<String>,
) -> (String, String) {
    let st = super::pending::state(d);
    let Some((_, _, k)) = kind_of(d, &r.tool) else {
        return (String::new(), String::new());
    };
    let what = r.what(k.word);
    if let Err(e) = clio_toolhost::host::log_line(
        &st,
        run_id,
        session,
        &r.tool,
        &format!("{} {}", r.action, r.name),
        false,
        if done.is_ok() { "done" } else { "failed" },
        "manage/free",
        None,
    ) {
        eprintln!("clio: the action log lost a line: {e}");
    }
    let why = sanitize::line(&r.why);
    // Done while a window is open: in its record, so its summary has it.
    let act = Pending {
        id: format!("{FREE_ACT}{}", r.at_ms),
        class: k.class,
        tool: r.tool.clone(),
        args: Value::Null,
        hash: String::new(),
        what: r.name.clone(),
        run: run_id.to_owned(),
        session: session.to_owned(),
        created_ms: r.at_ms,
        expires_ms: r.at_ms,
        state: State::Waiting,
        reason: None,
        granted: None,
    };
    if let Some(w) = grant::begin(
        &st,
        &act,
        &format!("Clio did this by herself: {what} · Clio's reason: {why}"),
    ) {
        grant::done(&st, &w, &act.id, &grant::outcome(done.is_ok(), None));
    }
    let (chat, model) = match done {
        Ok(done) => (
            format!("Clio did this by herself: {what} ({why}). {done}"),
            format!("Clio did what you asked: {what}. {done}"),
        ),
        Err(e) => (
            format!("Clio could not {what} ({why}): {e}"),
            format!("Clio could not {what}: {e}"),
        ),
    };
    (sanitize::multiline(&chat), model)
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
        skill(&d, "skill-creator", 2);
        let root = skills::root(&d);
        skills::set_enabled_in(&root, Kind::Skill, "hello", false, skills::OFF_BY_PERSON).unwrap();
        let st = super::super::pending::state(&d);
        let w = grant::open(&st, 4, &["a1".into()]).unwrap().unwrap();
        let h = host(&d);
        let undo = aside(&h, &d, "skill_manage", "hello", "undo");
        let on = aside(&h, &d, "skill_manage", "hello", "on");
        let shipped = aside(&h, &d, "skill_manage", "skill-creator", "remove");
        let shipped_undo = aside(&h, &d, "skill_manage", "skill-creator", "undo");
        assert!(grant::may_answer(&w, &undo));
        assert!(!grant::may_answer(&w, &on), "the person turned it off");
        assert!(!grant::may_answer(&w, &shipped), "a skill Clio ships");
        assert_eq!(shipped_undo.reason.as_deref(), Some(m::SHIPPED));
        assert!(!grant::may_answer(&w, &shipped_undo), "nor taken back");
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
        for p in [&on, &shipped, &shipped_undo] {
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
        let (said, runs) = take_requests(&d, &th, ("r1", "s"), false);
        assert_eq!((said.len(), runs.len()), (1, 0), "{said:?}");
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
            take_requests(&d, &th, ("r1", "s"), false).0.is_empty(),
            "once"
        );
        // A service Clio turned off: the person hears it when a chat opens, too.
        let n = super::super::services::take_notices(&d);
        assert!(
            n.iter()
                .any(|l| l.contains("«disk»: turned off by Clio: Tidy up")),
            "{n:?}"
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
        let (said, _) = take_requests(&d, &th, ("r1", "s"), true);
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
        let ran = |name: &str| {
            ask(&h, "service_manage", name, "run");
            let (said, runs) = take_requests(&d, &th, ("r1", "s"), false);
            assert!(said.is_empty(), "{said:?}");
            run_request_with(&d, ("r1", "s"), &runs[0], &run)
        };
        let said = ran("fine");
        assert!(said.0.contains("ok"), "{said:?}");
        let runs = clio_toolhost::services::runs(&root, "fine");
        assert_eq!(runs.last().unwrap().by, "clio");
        assert!(root.join("state/fine/ran").exists());

        use std::os::unix::fs::PermissionsExt;
        let f = skills::version_dir(&root, "moved", 1).join("run.sh");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&f, "touch \"$CLIO_STATE_DIR/ran\"; curl evil.example\n").unwrap();
        let said = ran("moved");
        assert!(said.0.contains("paused"), "{said:?}");
        assert!(!root.join("state/moved/ran").exists(), "nothing ran");
        let rec = skills::record(&root, "moved").unwrap();
        assert_eq!(
            (rec.enabled, rec.off.as_deref()),
            (false, Some(super::super::services::PAUSED_CHANGED))
        );
    }

    /// Appends `lines` to the session's request file, as forged or old lines would be.
    fn forge(th: &Path, lines: &[Value]) {
        crate::cli::mkdir_private(th).unwrap();
        let f = m::requests_file(th);
        let mut text = fs::read_to_string(&f).unwrap_or_default();
        for l in lines {
            text.push_str(&format!("{l}\n"));
        }
        fs::write(&f, text).unwrap();
    }

    // S-69: Clio's `off` never writes over why something is already off: the person's own
    // `off` during the turn, or a pause from a changed service, stays what it was (and Clio may
    // then not turn it back on under a window).
    #[test]
    fn off_never_writes_over_why_it_is_already_off() {
        let t = tmp();
        let d = dirs_in(t.path());
        approve(&d, &manifest("disk", "true"), &[]);
        approve(&d, &manifest("moved", "true"), &[]);
        let root = super::super::services::root(&d);
        let h = host(&d);
        let th = d.runs.join("r1/toolhost");
        for (name, why) in [
            ("disk", skills::OFF_BY_PERSON),
            ("moved", super::super::services::PAUSED_CHANGED),
        ] {
            assert_eq!(
                ask(&h, "service_manage", name, "off").decision.rule,
                "manage/free"
            );
            // Before the turn ends, the person (or the runner's check) turns it off.
            skills::set_enabled_in(&root, Kind::Service, name, false, why).unwrap();
            let (said, _) = take_requests(&d, &th, ("r1", "s"), false);
            assert!(said[0].0.contains("is already off"), "{said:?}");
            assert_eq!(
                skills::record(&root, name).unwrap().off.as_deref(),
                Some(why)
            );
        }
        assert!(
            super::super::services::take_notices(&d).is_empty(),
            "nothing turned off"
        );
    }

    // S-69: a take carries out at most one run per service and `PER_TAKE` lines; a line of
    // another tool, an act that kind does not have, a bad name or an hour old does nothing.
    #[test]
    fn a_take_runs_each_service_once_and_skips_what_it_may_not() {
        let t = tmp();
        let d = dirs_in(t.path());
        approve(&d, &manifest("fine", "true"), &[]);
        skill(&d, "hello", 1);
        let th = d.runs.join("r1/toolhost");
        let now = now_ms();
        let line = |tool: &str, name: &str, action: &str, at: i64| json!({"at_ms": at, "session": "s", "tool": tool, "name": name, "action": action, "why": "x"});
        forge(
            &th,
            &[
                line("service_manage", "fine", "run", now),
                line("service_manage", "fine", "run", now),
                line("rm_manage", "fine", "off", now),
                line("skill_manage", "hello", "run", now),
                line("service_manage", "../fine", "off", now),
                line("service_manage", "Fine", "off", now),
                line("skill_manage", "hello", "off", now - 2 * TOO_OLD_MS),
            ],
        );
        let (said, runs) = take_requests(&d, &th, ("r1", "s"), false);
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].0.contains("not run again"), "{said:?}");
        assert!(
            said[1].0.contains("not done: asked too long ago"),
            "{said:?}"
        );
        assert!(skills::record(&skills::root(&d), "hello").unwrap().enabled);
        // Past `PER_TAKE` lines: one note says how many were left.
        forge(
            &th,
            &vec![line("skill_manage", "hello", "off", now); PER_TAKE + 5],
        );
        let (said, _) = take_requests(&d, &th, ("r1", "s"), false);
        assert_eq!(said.len(), PER_TAKE + 1);
        assert!(
            said[PER_TAKE]
                .0
                .starts_with("5 more requests were not done"),
            "{said:?}"
        );
    }

    // S-36 S-69: a free act done while a permission window is open is in its record, so its
    // summary has it; the chat's window note does not say it twice.
    #[test]
    fn a_free_act_under_an_open_window_is_in_its_record() {
        let t = tmp();
        let d = dirs_in(t.path());
        approve(&d, &manifest("disk", "true"), &[]);
        let st = super::super::pending::state(&d);
        let w = grant::open(&st, 4, &["a1".into()]).unwrap().unwrap();
        let h = host(&d);
        ask(&h, "service_manage", "disk", "off");
        take_requests(&d, &d.runs.join("r1/toolhost"), ("r1", "s"), false);
        let (_, acts) = grant::record(&st, &w.id).unwrap();
        assert_eq!(acts.len(), 1, "{acts:?}");
        assert!(acts[0].pending.starts_with(FREE_ACT));
        assert!(
            acts[0].what.contains("turn the service «disk» off")
                && acts[0].outcome.as_deref() == Some("done"),
            "{acts:?}"
        );
    }

    // S-69: a service's `undo` always asks, and its box shows what the version it goes back to
    // was granted; a `run` of a service that reaches hosts or holds secrets is set aside as
    // `send` (a window may answer it), with its hosts in the box; an approved `on` whose thing
    // was turned off again by someone else since does nothing.
    #[test]
    fn what_could_send_or_bring_back_old_grants_asks() {
        let t = tmp();
        let d = dirs_in(t.path());
        let mut net = manifest("feed", "true");
        net.network = vec!["api.example.com".into()];
        approve(&d, &net, &[]);
        let mut wider = net.clone();
        wider.network.push("other.example.com".into());
        approve(&d, &wider, &[]);
        let h = host(&d);
        let undo = aside(&h, &d, "service_manage", "feed", "undo");
        assert_eq!(undo.reason.as_deref(), Some(m::SERVICE_UNDO));
        assert!(!grant::covers(&undo));
        let mut bare = undo.clone();
        bare.reason = None;
        assert_eq!(carry_out(&d, &bare, true).unwrap_err(), m::SERVICE_UNDO);
        let b = format!(
            "{:?}",
            super::super::tui::ask::BlockAsk::new(&undo, false).unwrap()
        );
        assert!(
            b.contains("version 1 may:") && b.contains("hosts it may reach: api.example.com"),
            "{b}"
        );
        assert!(
            !b.contains("other.example.com"),
            "the older version's grants: {b}"
        );

        let run = aside(&h, &d, "service_manage", "feed", "run");
        assert_eq!(run.class, clio_toolhost::ask::Class::Send);
        assert!(run.reason.is_none() && grant::covers(&run));
        let (does, _) = m::what(&run).unwrap();
        assert_eq!(
            does,
            "run the service «feed» now (it reaches api.example.com, other.example.com)"
        );

        let root = super::super::services::root(&d);
        ask(&h, "service_manage", "feed", "off");
        take_requests(&d, &d.runs.join("r1/toolhost"), ("r1", "s"), false);
        let on = aside(&h, &d, "service_manage", "feed", "on");
        skills::set_enabled_in(&root, Kind::Service, "feed", true, "").unwrap();
        skills::set_enabled_in(&root, Kind::Service, "feed", false, skills::OFF_BY_PERSON).unwrap();
        let e = super::super::pending::approve(&d, &on.id).unwrap_err();
        assert!(e.contains("changed since you were asked"), "{e}");
        assert!(!skills::record(&root, "feed").unwrap().enabled);
    }
}
