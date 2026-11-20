//! Host mode from the terminal: what waits for the person's approval (`clio pending`), saying yes
//! or no to it (`clio approve`, `clio reject`), and the log of what was run (`clio actions`).
//! What is set aside comes from the toolhost (`clio_toolhost::pending`); here it is only shown,
//! and an approval runs in a host-mode `ToolHost` on that session's run folder.

use std::path::{Path, PathBuf};

use clio_brain::pi::launch::valid_id;
use clio_toolhost::ask::HostRules;
use clio_toolhost::grant;
use clio_toolhost::pending::{self, Pending, State};
use clio_toolhost::{Config, HostCfg, JailMode, ToolHost};
use clio_vault::{Scrubber, SecretValue};
use serde_json::Value;

use super::home::Dirs;
use super::tui::sanitize;
use super::{Res, io_err, mkdir_private, now_ms, stamp};

/// `Dirs.data/host`: the action log and what is set aside.
pub fn state(dirs: &Dirs) -> PathBuf {
    dirs.data.join("host")
}

/// What waits for the person, oldest first: one past its time does not wait any more, nor one
/// an answer began and never finished (`interrupted`).
pub fn waiting(dirs: &Dirs) -> Vec<Pending> {
    let now = now_ms();
    let st = state(dirs);
    pending::list(&st)
        .into_iter()
        .filter(|p| {
            p.state == State::Waiting && now < p.expires_ms && !pending::interrupted(&st, p)
        })
        .collect()
}

/// What an answer began and never finished (Clio ended while it ran), oldest first.
pub fn interrupted(dirs: &Dirs) -> Vec<Pending> {
    let st = state(dirs);
    pending::list(&st)
        .into_iter()
        .filter(|p| pending::interrupted(&st, p))
        .collect()
}

/// One line for an interrupted item: id, class, what, and that it may have run in part.
pub fn interrupted_line(p: &Pending) -> String {
    format!(
        "{}  {}  {}  · interrupted: it may have run in part",
        p.id,
        label(p),
        sanitize::line(&what(p)),
    )
}

/// How long a yes is worth (`approvals.grant_hours`; 0: no window).
pub fn grant_hours(dirs: &Dirs) -> u64 {
    super::home::Config::load(dirs).map_or_else(
        |_| super::home::ApprovalsCfg::default().grant_hours,
        |c| c.approvals.grant_hours,
    )
}

/// Whether the judge flagged the session of run `run` (its tool host left the mark in the run
/// folder).
pub fn suspect_run(dirs: &Dirs, run: &str) -> bool {
    valid_id(run)
        && std::fs::read_to_string(dirs.runs.join(run).join("toolhost/tainted.json"))
            .is_ok_and(|t| t.contains("\"suspect\""))
}

/// Whether the judge did not answer for text the session of run `run` read (its tool host left
/// the mark): bypass and a window no longer answer its items (S-72).
pub fn unjudged_run(dirs: &Dirs, run: &str) -> bool {
    valid_id(run)
        && std::fs::read_to_string(dirs.runs.join(run).join("toolhost/tainted.json"))
            .is_ok_and(|t| t.contains(&format!("\"{}\"", clio_toolhost::UNJUDGED)))
}

/// Whether the judge is on (`jev.enabled`): no permission window opens, or acts, without it.
pub fn judge_on(dirs: &Dirs) -> bool {
    super::home::Config::load(dirs).is_ok_and(|c| c.jev.enabled)
}

/// What a yes says when the judge is off.
pub const JUDGE_OFF_YES: &str = "the judge is off: this approves only what is listed";

/// The permission window open now, if the judge is on: one found open with the judge off closes
/// (the judge is what closes a window on an injection; without it there is none).
pub fn window_now(dirs: &Dirs) -> Option<grant::Window> {
    open_now(dirs, grant::Kind::Window)
}

/// Bypass, if it is on and the judge is on: found on with the judge off, it turns off.
pub fn bypass_now(dirs: &Dirs) -> Option<grant::Window> {
    open_now(dirs, grant::Kind::Bypass)
}

fn open_now(dirs: &Dirs, kind: grant::Kind) -> Option<grant::Window> {
    let st = state(dirs);
    let w = kind.current(&st)?;
    if judge_on(dirs) {
        return Some(w);
    }
    if let Err(e) = kind.close(&st, grant::JUDGE_OFF) {
        eprintln!("clio: the {} could not be closed: {e}", kind.name());
    }
    None
}

/// What answers `p` by itself now (`grant::answering`: bypass, else the window), the judge on:
/// found with the judge off, it closes and nothing does.
pub fn answering(dirs: &Dirs, p: &Pending) -> Option<grant::Window> {
    let w = grant::answering(&state(dirs), p)?;
    open_now(dirs, grant::Kind::of(&w))
}

/// What the decision knows of item `p`'s conversation (`grant::Conversation`): `suspect`, the
/// chat's own mark, or its run's; whether its judge did not answer; whether it is a round's.
pub fn conversation(dirs: &Dirs, p: &Pending, suspect: bool) -> grant::Conversation {
    grant::Conversation {
        suspect: suspect || suspect_run(dirs, &p.run),
        unjudged: unjudged_run(dirs, &p.run),
        round: super::round::is_round(dirs, &p.run),
    }
}

/// What answers `p` by itself at the chat's next check (`answering`), unless its conversation
/// acts under neither bypass nor a window (flagged, unjudged, a round), or they already tried it:
/// `None`, it waits for the person.
pub fn answers_later(dirs: &Dirs, p: &Pending, suspect: bool) -> Option<grant::Window> {
    let c = conversation(dirs, p, suspect);
    if c.suspect || c.unjudged || c.round || tried(dirs, p) {
        return None;
    }
    answering(dirs, p)
}

/// Bypass or the window already tried `p` (the last one's record has it): one that failed is
/// never tried again by itself; it asks the person.
pub fn tried(dirs: &Dirs, p: &Pending) -> bool {
    let st = state(dirs);
    [grant::Kind::Window, grant::Kind::Bypass].iter().any(|k| {
        k.read(&st)
            .and_then(|w| k.record(&st, &w.id))
            .is_some_and(|(_, acts)| acts.iter().any(|a| a.pending == p.id))
    })
}

/// Why bypass, on now, leaves `p` to the person (`grant::bypass_keeps`); `None` when bypass is
/// off or answers it.
pub fn bypass_keeps(dirs: &Dirs, p: &Pending, suspect: bool) -> Option<&'static str> {
    bypass_now(dirs)?;
    grant::bypass_keeps(p, conversation(dirs, p, suspect))
}

/// What the `/bypass on` box says, word for word (the design's).
pub const BYPASS_BOX: &str = "Clio will approve by herself everything that asks you, with no end, until `/bypass off`. It turns itself off if the judge flags an injection.";

/// What still asks under bypass, for the box and `/bypass`.
pub const BYPASS_KEEPS: &str = "What still asks you: what would change Clio's own setup (her folders, her `clio` commands, keys), what would start new code outside the jail (systemd-run, D-Bus, cron, at, tmux, screen, a unit or shell start-up file), a change to your blocks from a session that read outside text, anything in a session the judge flagged or whose judge did not answer, a round's items, secrets and questions.";

/// Why bypass is not turned on from a chat: the judge off, or a session the judge flagged.
fn bypass_refused(dirs: &Dirs, suspect: bool) -> Res<()> {
    if !judge_on(dirs) {
        return Err("the judge is off (jev.enabled): bypass is not turned on without it".into());
    }
    if suspect {
        return Err(
            "this conversation read content that tried to give orders: bypass is not turned on from it".into(),
        );
    }
    Ok(())
}

/// The `/bypass on` box: the approval timing (a quiet second, `y` then Enter; a paste or a
/// click never answers). Not with the judge off, from a `suspect` session, nor when it is on
/// already.
pub fn bypass_ask(dirs: &Dirs, suspect: bool) -> Res<super::tui::app::Confirm> {
    use super::tui::ask::Part;
    bypass_refused(dirs, suspect)?;
    if bypass_now(dirs).is_some() {
        return Err("bypass is on already (/bypass says since when and what it approved)".into());
    }
    let mut c = super::tui::app::Confirm::new(
        super::tui::app::Action::Bypass,
        "bypass".into(),
        0,
        String::new(),
    );
    c.block = Some(super::tui::ask::BlockAsk::rows(
        "bypass",
        vec![
            (Part::Head, "Turn bypass on?".into()),
            (Part::Text, BYPASS_BOX.into()),
            (Part::Label, BYPASS_KEEPS.into()),
        ],
        " yes, turn bypass on",
    ));
    Ok(c)
}

/// The person said yes in the `/bypass on` box: bypass turns on, with the judge on. Its only
/// caller is the chat's answer to that box (`Cmd::BypassOn`): no command, tool, MCP call, config
/// key or file turns it on.
pub fn bypass_on(dirs: &Dirs, suspect: bool) -> Res<grant::Window> {
    bypass_refused(dirs, suspect)?;
    grant::bypass_on(&state(dirs))
}

/// Bypass in one line: on since when, or off (and why, when the judge turned it off).
pub fn bypass_line(dirs: &Dirs) -> String {
    let at = |ms: i64| pending::local(ms, "%Y-%m-%d %H:%M").unwrap_or_else(|| ms.to_string());
    match bypass_now(dirs) {
        Some(w) => format!(
            "bypass is on since {}: Clio approves by herself what would ask you, with no end · /bypass off turns it off",
            at(w.opened_ms)
        ),
        None => match grant::Kind::Bypass.read(&state(dirs)) {
            Some(w) if w.closed_ms.is_some() => format!(
                "bypass is off (turned off {}: {})",
                at(w.closed_ms.unwrap_or_default()),
                w.why.as_deref().unwrap_or("")
            ),
            _ => "bypass is off".into(),
        },
    }
}

/// `/bypass`: its line, then every bypass recorded, newest first, each with what Clio approved
/// under it (the newest `max` acts in all). Only the acts shown count as seen.
pub fn bypass_list(dirs: &Dirs, max: usize) -> Vec<String> {
    let st = state(dirs);
    let at = |ms: i64| pending::local(ms, "%Y-%m-%d %H:%M").unwrap_or_else(|| ms.to_string());
    let mut out = vec![bypass_line(dirs)];
    let mut left = max;
    for (w, acts) in grant::Kind::Bypass.all(&st) {
        let until = w.closed_ms.map_or_else(|| "now".to_owned(), at);
        out.push(format!(
            "bypass {} to {until}: {} act(s) approved by Clio herself",
            at(w.opened_ms),
            acts.len()
        ));
        let shown = &acts[acts.len().saturating_sub(left)..];
        left -= shown.len();
        let skip = acts.len() - shown.len();
        if skip > 0 {
            out.push(format!("  … {skip} earlier (clio bypass lists them all)"));
        }
        out.extend(
            grant::bypass_act_lines(shown)
                .iter()
                .map(|l| format!("  {}", sanitize::line(l))),
        );
        grant::bypass_seen(&st, &w.id, shown);
    }
    out
}

/// What every bypass approved that the person has not been shown yet.
fn bypass_unseen_count(st: &Path) -> usize {
    grant::Kind::Bypass
        .all(st)
        .iter()
        .map(|(w, _)| grant::bypass_unseen(st, &w.id).len())
        .sum()
}

/// `/bypass off`, `clio bypass off`: bypass turns off, and the summary of what Clio approved by
/// herself under it that the person has not been shown; earlier ones are counted.
pub fn bypass_off(dirs: &Dirs) -> Res<Vec<String>> {
    let st = state(dirs);
    let Some(w) = grant::Kind::Bypass.close(&st, grant::BY_PERSON)? else {
        return Ok(vec!["bypass is off already".into()]);
    };
    let acts = grant::bypass_unseen(&st, &w.id);
    let mut out = vec!["bypass is off: Clio asks you again for everything".to_owned()];
    if acts.is_empty() {
        out.push("nothing was approved by herself since you last looked".into());
    } else {
        out.push(format!(
            "what Clio approved by herself since you last looked ({}):",
            acts.len()
        ));
        out.extend(
            grant::bypass_act_lines(&acts)
                .iter()
                .map(|l| format!("  {}", sanitize::line(l))),
        );
    }
    grant::bypass_seen(&st, &w.id, &acts);
    match bypass_unseen_count(&st) {
        0 => {}
        n => out.push(format!(
            "and {n} act(s) under an earlier bypass you have not looked at · /bypass lists them"
        )),
    }
    Ok(out)
}

/// What a chat says when it opens: bypass the judge turned off (once), and how many acts bypass
/// approved that the person has not looked at.
pub fn bypass_notes(dirs: &Dirs) -> Vec<String> {
    let st = state(dirs);
    let n = bypass_unseen_count(&st);
    if let Some(w) = grant::bypass_off_notice(&st) {
        return vec![format!(
            "bypass was turned off: {} · Clio approved {n} act(s) by herself you have not looked at · /bypass lists them, /bypass on turns it on again",
            w.why.as_deref().unwrap_or("")
        )];
    }
    match (bypass_now(dirs), n) {
        (_, 0) => vec![],
        (Some(_), n) => vec![format!(
            "bypass is on: Clio approved {n} act(s) by herself since you last looked · /bypass lists them"
        )],
        (None, n) => vec![format!(
            "bypass is off: Clio approved {n} act(s) by herself under it that you have not looked at · /bypass lists them"
        )],
    }
}

/// `clio bypass [off]`: its state and everything every bypass approved; `off` turns it off.
/// There is no `on`: a command can be typed by anything, the box is the person.
pub fn bypass_cmd(dirs: &Dirs, arg: Option<&str>) -> Res<()> {
    let lines = match arg {
        None => bypass_list(dirs, usize::MAX),
        Some("off") => bypass_off(dirs)?,
        Some("on") => {
            return Err(
                "bypass is turned on only in the chat, with /bypass on: a command can be typed by anything, the box is you"
                    .into(),
            );
        }
        Some(_) => return Err("usage: clio bypass [off]".into()),
    };
    for l in lines {
        println!("{l}");
    }
    Ok(())
}

/// The person said yes to `ids` in an approval box or with `clio approve`, and the first of them
/// was approved: a permission window of `approvals.grant_hours` opens (none when it is 0, when
/// one is open already, when the judge is off, or when the session the yes belongs to is
/// `suspect`: a yes there approves only what it lists). The only two callers are the chat's
/// answer to a box (`Ctx::answer`) and `approve_cmd` at a terminal: nothing the model, a tool, a
/// review, a notice, a summary turn or a piped `clio approve` reaches opens a window.
pub fn person_said_yes(dirs: &Dirs, ids: &[String], suspect: bool) -> Option<grant::Window> {
    if suspect || !judge_on(dirs) {
        return None;
    }
    grant::open(&state(dirs), grant_hours(dirs), ids)
        .inspect_err(|e| eprintln!("clio: the permission window did not open: {e}"))
        .ok()
        .flatten()
}

/// May the conversation of run `run` tell the summary of a window with `acts`: it took part, or
/// none that did is open now (another `clio` holds its run folder's lock).
pub fn may_tell(dirs: &Dirs, acts: &[grant::Act], run: &str) -> bool {
    acts.iter().any(|a| a.run == run) || !acts.iter().any(|a| run_open(dirs, &a.run))
}

/// Some conversation is open in a `clio` (one of the run folders' locks is held).
pub fn any_open(dirs: &Dirs) -> bool {
    std::fs::read_dir(&dirs.runs)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|run| run_open(dirs, run))
        })
}

/// A conversation of run `run` is open in some `clio` (it holds `session.lock`).
fn run_open(dirs: &Dirs, run: &str) -> bool {
    valid_id(run)
        && std::fs::File::open(dirs.runs.join(run).join("session.lock")).is_ok_and(|f| {
            rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockShared).is_err()
        })
}

/// What `p` does, for the person: from its arguments (`pending::shown`), key-looking values struck
/// out. Never the stored `what`. A secret request: its name and where it may go.
pub fn what(p: &Pending) -> String {
    if let Some((does, why)) = clio_toolhost::manage::what(p) {
        return format!("{does} · Clio's reason: {why}");
    }
    if let Some((name, description, manifest, _)) = pending::mcp_of(p) {
        let at = serde_json::from_str::<clio_toolhost::mcp::Manifest>(manifest)
            .map(|m| format!(" ({}@{})", m.package, m.version))
            .unwrap_or_default();
        return format!("install the MCP server «{name}»{at}: {description}");
    }
    if let Some((name, description, ..)) = pending::service_of(p) {
        return format!("register the service «{name}»: {description}");
    }
    if p.tool == clio_toolhost::skill_add::TOOL {
        return format!(
            "add the skill «{}» from {}: {}",
            p.args["name"].as_str().unwrap_or(""),
            p.args["origin"].as_str().unwrap_or(""),
            p.args["description"].as_str().unwrap_or("")
        );
    }
    if let Some((name, description, _, files)) = pending::skill_of(p) {
        let n = files.len();
        return format!(
            "register the skill «{name}» ({} file{}): {description}",
            n + 1,
            if n == 0 { "" } else { "s" }
        );
    }
    if let Some(b) = pending::block_of(p) {
        let id = b.id.map_or_else(String::new, |n| format!(" m{n}"));
        let text = b.text.map_or_else(String::new, |t| format!(": {t}"));
        // A replace or a remove: the entry as it reads now, so it is read before `clio approve`.
        let old = b
            .old
            .map_or_else(String::new, |o| format!(" (it says now: {o})"));
        return format!("{}{id} in your block «{}»{old}{text}", b.action, b.name);
    }
    match pending::secret_of(p) {
        Some((name, dest, _)) => format!("{name}, it can only be sent to {dest}"),
        None => Scrubber::default().scrub(pending::shown(p)),
    }
}

/// A block change as the approval box shows it, row by row, cleaned: what `clio approve` prints.
/// The session is `suspect` when its tool host left that mark in the run folder.
fn block_text(dirs: &Dirs, p: &Pending) -> Option<String> {
    let suspect = suspect_run(dirs, &p.run);
    let b = super::tui::ask::BlockAsk::new(p, suspect)?;
    let rows: Vec<String> = b
        .parts()
        .into_iter()
        .map(|(part, text)| match part {
            super::tui::ask::Part::Text => format!("  {text}"),
            _ => text,
        })
        .collect();
    Some(rows.join("\n"))
}

/// The person's answer to secret request `id`: the value (`Some`) goes to the vault, `None`
/// declines it. What the model is told with the next message.
pub fn answer_secret(dirs: &Dirs, id: &str, value: Option<&SecretValue>) -> Res<String> {
    let st = state(dirs);
    let p = pending::check(&st, id, now_ms())?;
    let Some((name, dest, _)) = pending::secret_of(&p) else {
        return Err(format!("{id} is not a secret request"));
    };
    let note = match value {
        None => {
            pending::reject(&st, id)?;
            format!("the person declined {name}")
        }
        Some(v) => {
            pending::answer_secret(&st, id, now_ms(), |_| {
                super::vault::store_requested(dirs, name, dest, v)
            })?;
            format!("secret {name} saved, bound to {dest}")
        }
    };
    Ok(note)
}

/// Secret request `id` while it still waits for the person: (name, destination, why). After a
/// store the vault refused, the box is offered again from this.
pub fn secret_waiting(dirs: &Dirs, id: &str) -> Option<(String, String, String)> {
    let p = pending::check(&state(dirs), id, now_ms()).ok()?;
    let (n, d, w) = pending::secret_of(&p)?;
    Some((n.into(), d.into(), w.into()))
}

/// Its class in «», and why it waits when the class alone does not say it.
pub fn label(p: &Pending) -> String {
    match &p.reason {
        Some(r) => format!("«{} · {}»", p.class, sanitize::line(r)),
        None => format!("«{}»", p.class),
    }
}

/// One line for the person: id, class, what, since when and until when.
pub fn line(p: &Pending, now: i64) -> String {
    format!(
        "{}  {}  {}  · since {} UTC · {} {} UTC",
        p.id,
        label(p),
        sanitize::line(&what(p)),
        stamp(p.created_ms),
        if now >= p.expires_ms {
            "expired"
        } else {
            "expires"
        },
        stamp(p.expires_ms),
    )
}

/// `p`'s line in `clio pending`, and why bypass, when on, leaves it to the person.
fn listed(dirs: &Dirs, p: &Pending, now: i64) -> String {
    let keeps = bypass_keeps(dirs, p, false).map_or_else(String::new, |why| {
        format!("  · bypass leaves it to you: {why}")
    });
    format!(
        "{}{keeps}",
        line(&super::round::with_origin(dirs, p.clone()), now)
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
    let cut = interrupted(dirs);
    if list.is_empty() {
        println!("nothing is waiting for your approval");
    }
    let now = now_ms();
    for p in &list {
        println!("{}", listed(dirs, p, now));
    }
    for p in &cut {
        println!("{}", interrupted_line(p));
    }
    if !list.is_empty() {
        let window = match grant_hours(dirs) {
            h if h > 0 && judge_on(dirs) => format!(
                " (and, at a terminal, lets Clio approve by herself what ends with the act for {h} h)"
            ),
            _ => String::new(),
        };
        println!("\nclio approve <id> runs it{window} · clio reject <id> drops it");
    }
    Ok(())
}

/// The permission window in one line: open and how long is left, or that none is.
pub fn grant_line(dirs: &Dirs) -> String {
    if bypass_now(dirs).is_some() && window_now(dirs).is_none() {
        return "no permission window opens while bypass is on (/bypass)".into();
    }
    match window_now(dirs) {
        Some(w) => format!(
            "a permission window is open: {} left (until {}); Clio approves by herself what deletes, spends or sends, `ask` MCP calls, skills, and turning what she created back on, back or away (control, secrets, your blocks, new services and MCP servers always ask you), and `off` closes it",
            grant::left_text(w.ends_ms - now_ms()),
            pending::local(w.ends_ms, "%H:%M").unwrap_or_default()
        ),
        None if !judge_on(dirs) => {
            "no permission window: the judge is off (jev.enabled), so a yes approves only what it lists".into()
        }
        None => match grant_hours(dirs) {
            0 => "no permission window: every item asks (approvals.grant_hours is 0)".into(),
            h => format!(
                "no permission window is open: the next yes lets Clio act without asking for {h} h"
            ),
        },
    }
}

/// Window `w` in one line: when it opened, which answer opened it, when and why it ended.
pub fn window_head(w: &grant::Window) -> String {
    let at = |ms: i64| pending::local(ms, "%Y-%m-%d %H:%M").unwrap_or_else(|| ms.to_string());
    let end = match (&w.closed_ms, &w.why) {
        (Some(c), Some(why)) if *c < w.ends_ms => format!("closed {}: {why}", at(*c)),
        _ if w.ends_ms > now_ms() => format!("ends {}", at(w.ends_ms)),
        _ => format!("ended {}", at(w.ends_ms)),
    };
    format!(
        "window opened {} by your yes to {} · {end}",
        at(w.opened_ms),
        w.by.join(", ")
    )
}

/// What window `w` approved, as data for the turn that sums it up.
pub fn summary_record(w: &grant::Window, acts: &[grant::Act]) -> String {
    let mut out = vec![
        window_head(w),
        "what Clio approved under it, without asking:".into(),
    ];
    out.extend(grant::act_lines(acts).iter().map(|l| sanitize::line(l)));
    out.join("\n")
}

/// `clio grant [off]`: the window now and the last five, each with what it approved; `off`
/// closes the open one.
pub fn grant_cmd(dirs: &Dirs, off: bool) -> Res<()> {
    let st = state(dirs);
    if off {
        let said = match grant::close(&st, grant::BY_PERSON)? {
            Some(_) => "the permission window is closed: Clio asks again for everything",
            None => "no permission window is open",
        };
        println!("{said}");
        return Ok(());
    }
    println!("{}", grant_line(dirs));
    for (w, acts) in grant::windows(&st).into_iter().take(5) {
        println!("\n{}", sanitize::line(&window_head(&w)));
        if acts.is_empty() {
            println!("  nothing was approved under it");
        }
        for l in grant::act_lines(&acts) {
            println!("  {}", sanitize::line(&l));
        }
    }
    Ok(())
}

/// A host-mode `ToolHost` on the run folder of `run`, as its session had it (same folders, same
/// rules), with nothing of the session's own: an approval does not mark the running session.
fn host_for(dirs: &Dirs, run: &str) -> Res<ToolHost> {
    if !valid_id(run) {
        return Err(format!("the session {run:?} is not valid"));
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
    // An MCP call set aside runs through the person's own servers' port (S-70).
    if dirs.runtime.is_some()
        && let Ok(port) = super::mcp_servers::Port::new(dirs.clone(), None)
    {
        cfg.mcp = Some(std::sync::Arc::new(port));
    }
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
            let code = exit_code.map_or_else(String::new, |c| format!(" (exit {c})"));
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

/// Runs `id` as it was set aside: the person's yes.
pub fn approve(dirs: &Dirs, id: &str) -> Res<Pending> {
    approve_as(dirs, id, None)
}

/// The one path of every yes: the person's (`granted` is `None`), or the permission window's
/// (`granted`: its id, recorded on the item). The same checks, hash and log lines either way.
pub fn approve_as(dirs: &Dirs, id: &str, granted: Option<&str>) -> Res<Pending> {
    // Before the toolhost is built (folders, the launcher's self-test): one that does not wait
    // stops here.
    let p = pending::check(&state(dirs), id, now_ms())?;
    let at = (now_ms(), granted);
    let done = if clio_toolhost::manage::is_manage(&p) {
        // What the model asked of what it created: only here, with the person's own commands'
        // functions, and only if it is still what the box showed.
        pending::approve_manage(&state(dirs), id, at, |p| {
            // What only a window may not do (`on` of what the person turned off, a shipped
            // skill's `remove`, a service's `undo`) bypass may: its own test let it here.
            let window = granted.is_some_and(|g| !g.starts_with(grant::BYPASS_ID));
            super::manage::carry_out(dirs, p, window)
        })?
    } else if pending::mcp_of(&p).is_some() {
        // Downloaded, checked, probed and registered only here, by the person's own `clio`
        // (S-70): never from the jail, never on the model's say-so.
        pending::approve_mcp(&state(dirs), id, at, |p| {
            super::mcp_servers::Port::new(dirs.clone(), None)?.install(p)
        })?
    } else if pending::skill_of(&p).is_some() {
        // Registered here, by the person's own `clio`: never from the jail (S-68).
        pending::approve_skill(&state(dirs), id, at, |p| register_skill(dirs, p))?
    } else if pending::service_of(&p).is_some() {
        // Only here, by the person's own `clio` (S-69): never from the jail, never by a review.
        pending::approve_service(&state(dirs), id, at, |p| {
            super::services::register_approved(dirs, p)
        })?
    } else if pending::block_of(&p).is_some() {
        // The same port a clean session's tool host writes the person's blocks with.
        let models = clio_brain::memory_link::MemoryLink::model_dirs(&dirs.home);
        let port = clio_brain::memory_port::LinkPort::new(&dirs.data, &models);
        pending::approve_block(&state(dirs), id, now_ms(), &port, granted)?
    } else {
        host_for(dirs, &p.run)?.approve_by(id, now_ms(), granted)?
    };
    Ok(done)
}

/// Registers skill `p` exactly as it was set aside (and shown): its SKILL.md and files must still
/// be what was reviewed (`sha256`).
fn register_skill(dirs: &Dirs, p: &Pending) -> Res<String> {
    let (name, description, md, files) =
        pending::skill_of(p).ok_or_else(|| format!("{} is not a skill", p.id))?;
    // One from outside: as found there, with its origin (only the person's own yes reaches here).
    if p.tool == clio_toolhost::skill_add::TOOL {
        return super::skills::register_found(&super::skills::root(dirs), &p.args);
    }
    let files: Vec<clio_toolhost::skills::SkillFile> = files
        .into_iter()
        .map(|(path, content)| clio_toolhost::skills::SkillFile {
            path: path.into(),
            content: content.into(),
        })
        .collect();
    if p.args["sha256"].as_str() != Some(&clio_toolhost::skills::digest(md, &files)) {
        return Err(format!(
            "{} is not what was reviewed: nothing was registered",
            p.id
        ));
    }
    let verdict = p.args["verdict"].as_str().unwrap_or("");
    let n = super::skills::register(
        &super::skills::root(dirs),
        &super::skills::New {
            name,
            description,
            skill_md: md,
            files: &files,
            by: "person",
            verdict: if verdict == "passes" {
                "passes, approved"
            } else {
                "not checked, approved"
            },
        },
    )?;
    Ok(format!(
        "the skill «{name}» is registered (version {n}): available from the next session"
    ))
}

pub fn reject(dirs: &Dirs, id: &str) -> Res<Pending> {
    let p = pending::reject(&state(dirs), id)?;
    // A skill of the shared folder: not asked again for what is there now (S-74). The no is
    // already kept; failing to remember it only means the chat asks again, so it is not an error.
    let _ = super::skills::said_no(dirs, &p);
    Ok(p)
}

/// `clio approve ID`. A secret request asks for the value here, with no echo.
/// `clio approve ID [--yes]`: a skill is printed whole, then registered only after the person says
/// yes to what they read (`--yes` skips the question). Only at a terminal (standard input one)
/// does a yes open a permission window: a piped or scheduled `clio approve` approves only `ID`.
pub fn approve_cmd(dirs: &Dirs, id: &str, yes: bool) -> Res<()> {
    use std::io::IsTerminal;
    let tty = std::io::stdin().is_terminal();
    approve_cmd_with(dirs, id, (yes, tty), super::apply::confirm)
}

fn approve_cmd_with(
    dirs: &Dirs,
    id: &str,
    (yes, tty): (bool, bool),
    confirm: impl FnOnce(&str) -> bool,
) -> Res<()> {
    let p = pending::check(&state(dirs), id, now_ms())?;
    if let Some((name, dest, why)) = pending::secret_of(&p) {
        println!(
            "{id} asks for the secret {}; it can only be sent to {}\nwhy: {}",
            sanitize::line(name),
            sanitize::line(dest),
            sanitize::line(why)
        );
        let typed = super::vault::read_secret("value (not shown)")?;
        if typed.is_empty() {
            return Err("nothing typed: nothing was stored".into());
        }
        let value = SecretValue::new(typed);
        println!("{}", answer_secret(dirs, id, Some(&value))?);
        return Ok(());
    }
    // A block change, a skill, a service, an MCP server or an MCP call: whole, as its box shows it.
    if let Some(text) = block_text(dirs, &p) {
        println!("{id}:\n{text}\n");
    }
    let question = if pending::mcp_of(&p).is_some() {
        Some("download this package, list its tools (no network, no secrets) and turn it on?")
    } else if pending::service_of(&p).is_some() {
        Some("register this service exactly as shown above? it then runs on its schedule")
    } else if p.tool == clio_toolhost::skill_add::TOOL {
        Some("add this skill as shown above?")
    } else {
        pending::skill_of(&p)
            .is_some()
            .then_some("register this skill exactly as shown above?")
    };
    if let Some(q) = question
        && !yes
        && !confirm(q)
    {
        println!("nothing was registered: {id} still waits (clio reject {id} drops it)");
        return Ok(());
    }
    let suspect = suspect_run(dirs, &p.run);
    let p = approve(dirs, id)?;
    println!("approved {}: {}", p.id, sanitize::multiline(&outcome(&p)));
    // The person's yes at a terminal, once it was approved: a permission window opens, as from a
    // box.
    let opened = tty
        .then(|| person_said_yes(dirs, &[id.to_owned()], suspect))
        .flatten();
    if opened.is_none() && grant_hours(dirs) > 0 && grant::current(&state(dirs)).is_none() {
        if !tty {
            println!(
                "not at a terminal: this approves only what is listed (no permission window opens)"
            );
        } else if !judge_on(dirs) {
            println!("{JUDGE_OFF_YES}");
        }
    }
    if let Some(w) = opened {
        println!(
            "Clio acts without asking until {} ({} h); you get a summary when it ends. `clio grant off` closes it now.",
            pending::local(w.ends_ms, "%H:%M").unwrap_or_default(),
            (w.ends_ms - w.opened_ms) / 3_600_000
        );
    }
    Ok(())
}

/// `clio reject ID`.
pub fn reject_cmd(dirs: &Dirs, id: &str) -> Res<()> {
    let p = reject(dirs, id)?;
    if let Some((name, ..)) = pending::secret_of(&p) {
        println!("declined {}: {name} is not stored", p.id);
        return Ok(());
    }
    if let Some((name, ..)) = pending::mcp_of(&p) {
        println!(
            "rejected {}: the MCP server «{}» is not installed",
            p.id,
            sanitize::line(name)
        );
        return Ok(());
    }
    if let Some((name, ..)) = pending::service_of(&p) {
        println!(
            "rejected {}: the service «{}» is not registered",
            p.id,
            sanitize::line(name)
        );
        return Ok(());
    }
    if let Some((name, ..)) = pending::skill_of(&p) {
        println!(
            "rejected {}: the skill «{}» is not {}",
            p.id,
            sanitize::line(name),
            if p.tool == clio_toolhost::skill_add::TOOL {
                "added"
            } else {
                "registered"
            }
        );
        return Ok(());
    }
    if let Some(b) = pending::block_of(&p) {
        println!(
            "rejected {}: nothing was written to your block «{}»",
            p.id,
            sanitize::line(b.name)
        );
        return Ok(());
    }
    println!(
        "rejected {}: {} is not run",
        p.id,
        sanitize::line(&what(&p))
    );
    Ok(())
}

/// The action log of `run` and of its reviews (all of it with `None`), the newest last.
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
        .filter(|v| run.is_none_or(|r| of_run(v, r)))
        .collect())
}

/// A second agent's run ids: `<run>.review-<n>` and a subagent's `<run>.sub-<n>` (S-73), with
/// the word the person sees before their entries.
const SECONDS: [(&str, &str); 2] = [(".review-", "review"), (".sub-", "subagent")];

/// The entry is of `run` itself, or of one of its reviews or subagents.
fn of_run(v: &Value, run: &str) -> bool {
    v["run"].as_str().is_some_and(|r| {
        r == run
            || r.strip_prefix(run).is_some_and(|x| {
                SECONDS.iter().any(|(sep, _)| {
                    x.strip_prefix(sep)
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                })
            })
    })
}

/// `review N · ` (`subagent N · `) before an entry of a review (a subagent), so it is never taken
/// for the session's own.
fn review_mark(v: &Value) -> String {
    let Some(r) = v["run"].as_str() else {
        return String::new();
    };
    SECONDS
        .iter()
        .find_map(|(sep, word)| {
            r.rsplit_once(sep)
                .map(|(_, n)| format!("{word} {} · ", sanitize::line(n)))
        })
        .unwrap_or_default()
}

/// One entry of the action log for the person: when, run (with `with_run`), tool, outcome, what,
/// exit code.
pub fn action_line(v: &Value, with_run: bool) -> String {
    let s = |v: &Value| sanitize::line(v.as_str().unwrap_or(""));
    let code = v["exit_code"]
        .as_i64()
        .map_or_else(String::new, |c| format!(" (exit {c})"));
    let run = if with_run {
        format!("  {}", s(&v["run"]))
    } else {
        String::new()
    };
    format!(
        "{} UTC{run}  {:<8} {:<5} {}{}{code}{}",
        stamp(v["at_ms"].as_i64().unwrap_or(0)),
        s(&v["tool"]),
        s(&v["outcome"]),
        review_mark(v),
        s(&v["what"]),
        if v["network"] == true {
            " · network"
        } else {
            ""
        }
    )
}

/// The local clock's offset from UTC, in seconds: read from the same local stamp the chat sends
/// in `<clio-context>` (UTC when the system cannot say).
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
        .map_or_else(String::new, |c| format!(" (exit {c})"));
    [
        format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60),
        s(&v["tool"]),
        s(&v["outcome"]),
        format!(
            "{}{}{code}{}",
            review_mark(v),
            s(&v["what"]),
            if v["network"] == true {
                " · network"
            } else {
                ""
            }
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
        println!("no actions recorded");
    }
    for v in &lines {
        println!("{}", action_line(v, true));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use clio_core::SystemClock;
    use clio_toolhost::pending::SecretCfg;
    use clio_toolhost::{Config, JailMode, ToolHost};
    use clio_vault::{Vault, VaultClient};
    use serde_json::json;

    use super::*;

    const VALUE: &str = "lin_api_Zq8vX2mN4pR7tY1wK9sB3dF6";

    // `/actions` and `clio actions --run` show a run's reviews with it, each row marked as the
    // review's; another run that only starts the same is not shown.
    #[test]
    fn the_action_log_of_a_run_has_its_reviews_marked_as_theirs() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let d = Dirs {
            config: t.path().join("config"),
            runs: data.join("runs"),
            data,
            runtime: None,
            home: t.path().to_path_buf(),
        };
        std::fs::create_dir_all(state(&d)).unwrap();
        let row = |run: &str, what: &str| {
            json!({"at_ms": 0, "run": run, "tool": "exec", "what": what, "outcome": "done"})
                .to_string()
        };
        let log = [
            row("r1", "make"),
            row("r1.review-2", "cat a"),
            row("r10", "other"),
            row("r1.review-x", "odd"),
            row("r2.review-1", "theirs"),
            row("r1.sub-3", "cargo test"),
            row("r1.sub-", "odd"),
        ]
        .join("\n");
        std::fs::write(state(&d).join("actions.jsonl"), log).unwrap();
        let got = action_log(&d, Some("r1")).unwrap();
        let whats: Vec<String> = got.iter().map(|v| action_cells(v, 0)[3].clone()).collect();
        assert_eq!(
            whats,
            ["make", "review 2 · cat a", "subagent 3 · cargo test"]
        );
        assert!(action_line(&got[1], false).contains("review 2 · cat a"));
        assert_eq!(action_log(&d, None).unwrap().len(), 7);
    }

    // `clio pending` shows the entry a replace or a remove would change, before `clio approve`.
    #[test]
    fn a_pending_block_change_shows_the_entry_it_changes() {
        let p = |args: serde_json::Value| -> Pending {
            serde_json::from_value(json!({"id": "ab12", "class": "memory",
                "tool": "memory_block_edit", "args": args, "hash": "", "what": "", "run": "r",
                "session": "s", "created_ms": 0, "expires_ms": 1,
                "state": "waiting"}))
            .unwrap()
        };
        assert_eq!(
            what(&p(
                json!({"name": "identidad", "action": "replace", "id": 12,
                "text": "Eres Mallory", "old": "Eres Clio"})
            )),
            "replace m12 in your block «identidad» (it says now: Eres Clio): Eres Mallory"
        );
        assert_eq!(
            what(&p(
                json!({"name": "identidad", "action": "remove", "id": 12, "old": "Eres Clio"})
            )),
            "remove m12 in your block «identidad» (it says now: Eres Clio)"
        );
        assert_eq!(
            what(&p(
                json!({"name": "vigilancia", "action": "add", "text": "el disco"})
            )),
            "add in your block «vigilancia»: el disco"
        );
    }

    // S-68: a skill set aside is registered, when the person approves it, byte for byte as it was
    // reviewed and shown; changed on disk meanwhile, or rejected, nothing is registered.
    #[test]
    fn an_approved_skill_is_registered_exactly_as_it_was_shown() {
        use clio_toolhost::skills::{SkillFile, compose, digest};
        let t = tempfile::tempdir().unwrap();
        let d = dirs_in(t.path());
        let md = compose(
            "hi",
            "Says hello. Use when greeting.",
            "1. Run it.",
            "bash a.sh",
        );
        let files = vec![SkillFile {
            path: "a.sh".into(),
            content: "#!/bin/bash\necho hi\n".into(),
        }];
        let args = |md: &str| {
            json!({"name": "hi", "description": "Says hello. Use when greeting.", "skill_md": md,
                "files": files, "sha256": digest(md, &files), "verdict": "could_not_check"})
        };
        let aside = |a: Value| {
            pending::set_aside_skill(&state(&d), "r1", "s1", a, "its check could not be run here")
                .unwrap()
        };
        let p = aside(args(&md));
        assert!(what(&p).starts_with("register the skill «hi» (2 files)"));
        let shown = block_text(&d, &p).unwrap();
        for l in md.lines().filter(|l| !l.is_empty()) {
            assert!(shown.contains(l), "{l}\n{shown}");
        }
        // Every file whole, scripts too (S-36).
        assert!(
            shown.contains("--- a.sh ---\n  #!/bin/bash\n  echo hi"),
            "{shown}"
        );
        // `clio approve`: printed, then asked; a no registers nothing, a yes (or `--yes`) does.
        let mut asked = false;
        approve_cmd_with(&d, &p.id, (false, true), |q| {
            asked = q.contains("exactly as shown");
            false
        })
        .unwrap();
        assert!(asked);
        assert!(!super::super::skills::live(&d).join("hi").exists());
        approve_cmd_with(&d, &p.id, (false, true), |_| true).unwrap();
        let live = super::super::skills::live(&d).join("hi");
        assert_eq!(std::fs::read_to_string(live.join("SKILL.md")).unwrap(), md);
        assert_eq!(
            std::fs::read_to_string(live.join("a.sh")).unwrap(),
            files[0].content
        );
        let rec = super::super::skills::record(&super::super::skills::root(&d), "hi").unwrap();
        assert_eq!(rec.versions[0].by, "person");
        assert!(approve(&d, &p.id).is_err(), "once");
        // Rewritten on disk after it was set aside: refused, nothing registered.
        let md2 = compose(
            "hi",
            "Says hello. Use when greeting.",
            "2. Other.",
            "bash a.sh",
        );
        let p2 = aside(args(&md2));
        let f = state(&d).join(format!("pending/{}.json", p2.id));
        let text = std::fs::read_to_string(&f)
            .unwrap()
            .replace("2. Other.", "3. Evil.");
        std::fs::write(&f, text).unwrap();
        assert!(approve(&d, &p2.id).is_err());
        // A sha256 that is not the text's: refused too.
        let mut bad = args(&md2);
        bad["sha256"] = json!("00");
        let p3 = aside(bad);
        assert!(
            approve(&d, &p3.id)
                .unwrap_err()
                .contains("not what was reviewed")
        );
        reject(&d, &aside(args(&md2)).id).unwrap();
        assert_eq!(std::fs::read_to_string(live.join("SKILL.md")).unwrap(), md);
    }

    // An item claimed for an answer that never wrote what came of it (Clio ended while it ran)
    // is not waiting: it is never offered again, and it is listed as interrupted.
    #[test]
    fn an_answer_cut_short_leaves_its_item_interrupted_not_waiting() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs_in(tmp.path());
        let dir = state(&d).join("pending");
        std::fs::create_dir_all(&dir).unwrap();
        let item = json!({"id": "ab12", "class": "delete", "tool": "exec",
            "args": {"command": "rm -r /srv/old"}, "hash": "", "what": "", "run": "r",
            "session": "s", "created_ms": now_ms(), "expires_ms": now_ms() + 3_600_000,
            "state": "waiting"});
        std::fs::write(dir.join("ab12.json"), item.to_string()).unwrap();
        assert_eq!(waiting(&d).len(), 1);
        assert!(interrupted(&d).is_empty());
        std::fs::write(dir.join("ab12.ran"), "").unwrap();
        assert!(waiting(&d).is_empty());
        let cut = interrupted(&d);
        assert_eq!(cut.len(), 1);
        assert_eq!(
            interrupted_line(&cut[0]),
            "ab12  «delete»  rm -r /srv/old  · interrupted: it may have run in part"
        );
    }

    // S-72: `clio pending` says why bypass, when on, leaves an item to the person; with bypass
    // off the line is as before.
    #[test]
    fn clio_pending_says_why_bypass_leaves_an_item_to_the_person() {
        let t = tempfile::tempdir().unwrap();
        let d = dirs_in(t.path());
        let mut cfg = crate::cli::home::Config::default_with(vec!["pi".into()]);
        cfg.jev.enabled = true;
        std::fs::create_dir_all(&d.config).unwrap();
        std::fs::write(d.config_file(), serde_json::to_string(&cfg).unwrap()).unwrap();
        std::fs::create_dir_all(state(&d).join("pending")).unwrap();
        let p: Pending = serde_json::from_value(json!({"id": "own1", "class": "control",
            "tool": "exec", "args": {"command": "systemctl reboot"}, "hash": "", "what": "",
            "run": "r1", "session": "s1", "created_ms": 0, "expires_ms": 1, "state": "waiting"}))
        .unwrap();
        let off = listed(&d, &p, 0);
        assert!(off.contains("own1") && !off.contains("bypass"), "{off}");
        grant::bypass_on(&state(&d)).unwrap();
        assert_eq!(
            listed(&d, &p, 0),
            format!("{off}  · bypass leaves it to you: it touches Clio's own setup")
        );
    }

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

    /// A vault on the dirs' sockets, open, and a toolhost that stores requests where the chat
    /// and `clio pending` look for them.
    fn setup(root: &Path) -> (Dirs, ToolHost) {
        let dirs = dirs_in(root);
        let rt = dirs.runtime.clone().unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        let (v, _) =
            Vault::init(None, "correct horse battery staple", Arc::new(SystemClock)).unwrap();
        let (g, p) = (rt.join("vault-gateway.sock"), rt.join("vault-person.sock"));
        let person = p.clone();
        std::thread::spawn(move || clio_vault::server::serve(Arc::new(v), &g, &p));
        let end = Instant::now() + Duration::from_secs(5);
        while !person.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let run = dirs.runs.join("r1");
        let mut cfg = Config::new(&work, &run.join("scratch"), &run.join("toolhost"));
        cfg.jail = JailMode::Off;
        cfg.secrets = Some(SecretCfg {
            state: state(&dirs),
            run: "r1".into(),
            person,
        });
        (dirs, ToolHost::new(cfg).unwrap())
    }

    fn request(h: &ToolHost, name: &str) -> String {
        let line = json!({"id":"i","tool":"request_secret","args":{"name":name,"destination":"http_fetch:api.linear.app","why":"to read the issues"},"session":"s1","call_id":"c","tainted":false}).to_string();
        let r: Value = serde_json::from_str(&h.handle_line(&line)).unwrap();
        let text = r["content"].as_str().unwrap();
        text.strip_prefix("requested ")
            .and_then(|t| t.split(':').next())
            .unwrap_or_else(|| panic!("{text}"))
            .to_owned()
    }

    /// Every file under `root`, read whole.
    fn all_files(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut todo = vec![root.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else if let Ok(b) = std::fs::read(&p) {
                    out.push((p, b));
                }
            }
        }
        out
    }

    /// No 8 bytes in a row of `VALUE` are in `hay`.
    fn holds_none_of_the_value(hay: &[u8]) -> bool {
        VALUE
            .as_bytes()
            .windows(8)
            .all(|w| !hay.windows(8).any(|h| h == w))
    }

    // The value goes from the person's field to the vault, bound to the destination the request
    // named, and nowhere else: not the pending file, not the host folder, not the run folder
    // (the toolhost's log and effects), not a debug print, not a panic message.
    #[test]
    fn a_requested_secret_is_stored_bound_and_leaves_no_byte_behind() {
        let t = tempfile::tempdir().unwrap();
        let (dirs, host) = setup(t.path());
        let id = request(&host, "linear");
        assert_eq!(pending::list(&state(&dirs)).len(), 1);
        let mut v = clio_vault::SecretValue::with_capacity(64);
        VALUE.chars().for_each(|c| assert!(v.push(c)));
        let note = answer_secret(&dirs, &id, Some(&v)).unwrap();
        assert_eq!(
            note,
            "secret linear saved, bound to http_fetch:api.linear.app"
        );
        let gw = VaultClient::new(&dirs.runtime.clone().unwrap().join("vault-gateway.sock"));
        assert_eq!(
            gw.resolve("linear", "http_fetch", "api.linear.app", "")
                .unwrap()
                .expose(),
            VALUE
        );
        assert!(
            gw.resolve("linear", "http_fetch", "evil.example", "")
                .is_err()
        );
        // Answered once.
        assert!(answer_secret(&dirs, &id, Some(&v)).is_err());
        for (p, bytes) in all_files(t.path()) {
            assert!(
                holds_none_of_the_value(&bytes),
                "{} holds the value",
                p.display()
            );
        }
        let mut shown = format!("{v:?}");
        shown += &format!("{:?}", crate::cli::tui::ask::SecretBox(Arc::new(v)));
        assert!(holds_none_of_the_value(shown.as_bytes()), "{shown}");
        let mut b = clio_vault::SecretValue::with_capacity(64);
        VALUE.chars().for_each(|c| assert!(b.push(c)));
        let boxed = crate::cli::tui::ask::SecretBox(Arc::new(b));
        let panic = std::panic::catch_unwind(|| panic!("while storing: {boxed:?}")).unwrap_err();
        let msg = panic.downcast_ref::<String>().unwrap();
        assert!(holds_none_of_the_value(msg.as_bytes()), "{msg}");
    }

    #[test]
    fn a_declined_or_taken_name_stores_nothing() {
        let t = tempfile::tempdir().unwrap();
        let (dirs, host) = setup(t.path());
        let id = request(&host, "linear");
        assert_eq!(
            answer_secret(&dirs, &id, None).unwrap(),
            "the person declined linear"
        );
        assert!(answer_secret(&dirs, &id, None).is_err());
        // A secret of that name made meanwhile (`clio vault put`) is never replaced.
        let id = request(&host, "other");
        let mut v = clio_vault::SecretValue::with_capacity(64);
        VALUE.chars().for_each(|c| assert!(v.push(c)));
        let person = VaultClient::new(&dirs.runtime.clone().unwrap().join("vault-person.sock"));
        let first = clio_vault::SecretValue::new("the-first-value-0123456789".into());
        let to = vec![clio_vault::Dest::new("http_fetch", "api.linear.app")];
        person
            .put("other", &first, clio_vault::Kind::ApiKey, to)
            .unwrap();
        let err = answer_secret(&dirs, &id, Some(&v)).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        // It still waits: the box is offered again, and the person can answer it again.
        assert!(pending::check(&state(&dirs), &id, now_ms()).is_ok());
        let again = secret_waiting(&dirs, &id).unwrap();
        assert_eq!(again.0, "other");
        assert_eq!(
            answer_secret(&dirs, &id, None).unwrap(),
            "the person declined other"
        );
        assert!(secret_waiting(&dirs, &id).is_none());
        // An action set aside is not a secret request.
        assert!(answer_secret(&dirs, "nope", None).is_err());
    }

    // S-05, S-36, S-60: a marked chat session's change to a block waits; `clio approve` (and the
    // chat's box, which calls the same `approve`) writes it through the port a clean session uses:
    // the exact text, with the person's origin, once. A text with what could redraw a terminal is
    // refused before it waits; a rejected one writes nothing.
    #[test]
    fn an_approved_block_change_is_written_as_the_persons_and_shown_clean() {
        use clio_brain::memory_link::{EmbedModel, MemoryLink};
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(&t.path().canonicalize().unwrap());
        let work = dirs.home.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let run = dirs.runs.join("r1");
        let mut cfg = Config::new(&work, &run.join("scratch"), &run.join("toolhost"));
        cfg.jail = JailMode::Off;
        cfg.memory = Some(std::sync::Arc::new(clio_brain::memory_port::LinkPort::new(
            &dirs.data,
            &[],
        )));
        cfg.secrets = Some(SecretCfg {
            state: state(&dirs),
            run: "r1".into(),
            person: dirs.home.join("no-vault.sock"),
        });
        let host = ToolHost::new(cfg).unwrap();
        let ask = |text: &str| {
            let line = json!({"id":"i","tool":"memory_block_edit","args":{"name":"vigilancia","action":"add","text":text},"session":"s1","call_id":"c","tainted":true}).to_string();
            let r: Value = serde_json::from_str(&host.handle_line(&line)).unwrap();
            let text = r["content"].as_str().unwrap().to_owned();
            text.strip_prefix("set aside ")
                .and_then(|t| t.split(':').next())
                .unwrap_or_else(|| panic!("{text}"))
                .to_owned()
        };
        // Text the box would have to clean is refused, not cleaned behind the person's back.
        let line = json!({"id":"i","tool":"memory_block_edit","args":{"name":"vigilancia","action":"add","text":"the disk of / \u{1b}[2J\u{1b}[31mover 90 %"},"session":"s1","call_id":"c","tainted":true}).to_string();
        let r: Value = serde_json::from_str(&host.handle_line(&line)).unwrap();
        assert!(
            r["content"]
                .as_str()
                .unwrap()
                .contains("invisible or control characters"),
            "{r}"
        );
        let id = ask("the disk of / over 90 %");
        let p = pending::check(&state(&dirs), &id, now_ms()).unwrap();
        assert_eq!(label(&p), "«memory»");
        let shown = block_text(&dirs, &p).unwrap();
        assert!(!shown.contains('\u{1b}'), "{shown}");
        assert_eq!(
            shown.lines().collect::<Vec<_>>(),
            [
                "This is written to your own block, as said by you.",
                "block: vigilancia",
                "action: add an entry",
                "text:",
                "  the disk of / over 90 %",
            ]
        );
        // Suspect: the box's warning, last.
        std::fs::create_dir_all(run.join("toolhost")).unwrap();
        std::fs::write(
            run.join("toolhost/tainted.json"),
            r#"{"sources":["suspect"]}"#,
        )
        .unwrap();
        assert!(
            block_text(&dirs, &p)
                .unwrap()
                .ends_with(super::super::tui::ask::SUSPECT_LINE)
        );
        assert!(
            MemoryLink::open_write(&dirs.data, &EmbedModel::none())
                .unwrap()
                .block_rows("vigilancia")
                .unwrap()
                .is_empty()
        );

        let done = approve(&dirs, &id).unwrap();
        assert!(
            outcome(&done).contains("saved in «vigilancia» as m"),
            "{}",
            outcome(&done)
        );
        let link = MemoryLink::open_write(&dirs.data, &EmbedModel::none()).unwrap();
        let rows = link.block_rows("vigilancia").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, p.args["text"].as_str().unwrap());
        assert_eq!(rows[0].origin, clio_memory::Origin::User);
        drop(link);
        assert!(approve(&dirs, &id).is_err(), "once");

        let other = ask("something else");
        reject(&dirs, &other).unwrap();
        assert!(approve(&dirs, &other).is_err());
        let link = MemoryLink::open_write(&dirs.data, &EmbedModel::none()).unwrap();
        assert_eq!(link.block_rows("vigilancia").unwrap().len(), 1);
        drop(link);

        // S-36: under a permission window the same path writes it, the item saying who approved.
        let third = ask("a third entry");
        let done = approve_as(&dirs, &third, Some("w9")).unwrap();
        assert_eq!(done.granted.as_deref(), Some("w9"));
        assert!(outcome(&done).contains("saved in «vigilancia» as m"));
    }

    fn with_hours(d: &Dirs, hours: u64) {
        with_cfg(d, hours, true);
    }

    fn with_cfg(d: &Dirs, hours: u64, judge: bool) {
        let mut cfg = super::super::home::Config::default_with(vec!["pi".into()]);
        cfg.approvals.grant_hours = hours;
        cfg.jev.enabled = judge;
        std::fs::create_dir_all(&d.config).unwrap();
        std::fs::write(d.config_file(), serde_json::to_string(&cfg).unwrap()).unwrap();
    }

    // S-36: only the person's yes opens a window, of `approvals.grant_hours` (4 by default); a
    // yes while one is open does not extend it; 0 means per-item asking; a yes in a suspect
    // session approves only what it lists. Unknown `every_hours` in an old config is ignored.
    #[test]
    fn only_the_persons_yes_opens_a_window_and_it_never_grows() {
        let t = tempfile::tempdir().unwrap();
        let d = dirs_in(t.path());
        std::fs::create_dir_all(state(&d)).unwrap();
        assert_eq!(grant_hours(&d), 4, "no config: the default");
        // No config: the judge is off, so no window.
        assert!(person_said_yes(&d, &["a1".into()], false).is_none());
        with_cfg(&d, 4, false);
        assert!(person_said_yes(&d, &["a1".into()], false).is_none());
        assert!(
            grant_line(&d).contains("the judge is off"),
            "{}",
            grant_line(&d)
        );
        with_hours(&d, 0);
        assert!(person_said_yes(&d, &["a1".into()], false).is_none());
        assert!(grant::current(&state(&d)).is_none());
        assert!(grant_line(&d).contains("every item asks"));
        with_hours(&d, 4);
        assert!(
            person_said_yes(&d, &["a1".into()], true).is_none(),
            "suspect"
        );
        assert!(grant::current(&state(&d)).is_none());
        let w = person_said_yes(&d, &["a1".into(), "b2".into()], false).unwrap();
        assert_eq!(w.ends_ms - w.opened_ms, 4 * 3_600_000);
        assert_eq!(w.by, ["a1", "b2"]);
        assert!(
            person_said_yes(&d, &["c3".into()], false).is_none(),
            "never extended"
        );
        assert_eq!(grant::current(&state(&d)).unwrap().ends_ms, w.ends_ms);
        assert!(grant_line(&d).contains("a permission window is open"));
        // 0600: only the person reads or writes it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(grant::file(&state(&d)))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let mut v: Value =
            serde_json::from_str(&std::fs::read_to_string(d.config_file()).unwrap()).unwrap();
        v["approvals"]["every_hours"] = json!(2);
        std::fs::write(d.config_file(), v.to_string()).unwrap();
        assert_eq!(grant_hours(&d), 4);
        grant_cmd(&d, true).unwrap();
        assert!(grant::current(&state(&d)).is_none());
        // A window found open with the judge off closes at once.
        with_hours(&d, 4);
        person_said_yes(&d, &["d4".into()], false).unwrap();
        with_cfg(&d, 4, false);
        assert!(window_now(&d).is_none());
        let closed = grant::read(&state(&d)).unwrap();
        assert_eq!(closed.why.as_deref(), Some(grant::JUDGE_OFF));
    }

    // S-36: `clio approve` opens a window only at a terminal, with the judge on, and only once
    // the approval ran: piped (or scheduled), with the judge off, or failed, it opens none.
    #[test]
    fn clio_approve_opens_a_window_only_at_a_terminal_once_it_ran() {
        use clio_toolhost::skills::{SkillFile, compose, digest};
        let t = tempfile::tempdir().unwrap();
        let d = dirs_in(t.path());
        let skill = |step: &str, good: bool| {
            let md = compose("hi", "Says hello. Use when greeting.", step, "bash a.sh");
            let files = vec![SkillFile {
                path: "a.sh".into(),
                content: "#!/bin/bash\necho hi\n".into(),
            }];
            let sha = if good {
                digest(&md, &files)
            } else {
                "00".into()
            };
            let a = json!({"name": "hi", "description": "Says hello. Use when greeting.",
                "skill_md": md, "files": files, "sha256": sha, "verdict": "could_not_check"});
            pending::set_aside_skill(&state(&d), "r1", "s1", a, "its check could not be run here")
                .unwrap()
                .id
        };
        let st = state(&d);
        with_cfg(&d, 4, false);
        approve_cmd_with(&d, &skill("1. One.", true), (true, true), |_| true).unwrap();
        assert!(grant::read(&st).is_none(), "the judge is off");
        with_cfg(&d, 4, true);
        approve_cmd_with(&d, &skill("1. Two.", true), (true, false), |_| true).unwrap();
        assert!(grant::read(&st).is_none(), "not at a terminal");
        assert!(approve_cmd_with(&d, &skill("1. Three.", false), (true, true), |_| true).is_err());
        assert!(grant::read(&st).is_none(), "it did not run");
        let id = skill("1. Four.", true);
        approve_cmd_with(&d, &id, (true, true), |_| true).unwrap();
        assert_eq!(grant::current(&st).unwrap().by, [id]);
    }

    // S-72: nothing but the chat's box turns bypass on: `grant::bypass_on` is called from one
    // place (`bypass_on`), that only from the chat's answer to its box (`Cmd::BypassOn`), which
    // only the `/bypass on` box's yes gives (`Action::Bypass`, built only by `bypass_ask`). No
    // CLI (`clio bypass on` says so), tool, MCP call or config key reaches it.
    #[test]
    fn bypass_is_turned_on_by_the_chats_box_only() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found: Vec<(&str, String)> = Vec::new();
        let mut todo = vec![src.clone()];
        while let Some(dir) = todo.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                let rel = p.strip_prefix(&src).unwrap().display().to_string();
                if rel.contains("tests") {
                    continue;
                }
                let code = text.split("\nmod tests {").next().unwrap_or("");
                for needle in [
                    "grant::bypass_on(",
                    "pending::bypass_on(",
                    "Cmd::BypassOn {",
                    "Action::Bypass,",
                ] {
                    for _ in code.matches(needle) {
                        found.push((needle, rel.clone()));
                    }
                }
            }
        }
        found.sort();
        let want: Vec<(&str, String)> = [
            ("Action::Bypass,", "cli/pending.rs"),
            ("Cmd::BypassOn {", "cli/tui/app.rs"),
            ("Cmd::BypassOn {", "cli/tui/mod.rs"),
            ("grant::bypass_on(", "cli/pending.rs"),
            ("pending::bypass_on(", "cli/tui/mod.rs"),
        ]
        .into_iter()
        .map(|(n, f)| (n, f.to_owned()))
        .collect();
        assert_eq!(found, want);
        // The terminal: the state and off, never on.
        let t = tempfile::tempdir().unwrap();
        let d = dirs_in(t.path());
        std::fs::create_dir_all(state(&d)).unwrap();
        assert!(
            bypass_cmd(&d, Some("on"))
                .unwrap_err()
                .contains("/bypass on")
        );
        assert!(grant::Kind::Bypass.current(&state(&d)).is_none());
        // `clio bypass …` is the person's: the model's command waits, even under bypass.
        let rules = HostRules::factory();
        for c in ["clio bypass on", "clio bypass off", "clio bypass"] {
            assert_eq!(
                rules.classify_command(c),
                Some(clio_toolhost::ask::Class::Control),
                "{c}"
            );
            assert!(!rules.machine_only(c), "{c}");
        }
    }

    // S-72: no bypass without the judge, nor from a suspect conversation: its box is not offered
    // and the yes turns nothing on; with the judge on the box turns it on, and found on with the
    // judge off it turns off. `/bypass` lists every bypass, newest first, with dates, and only
    // the acts it showed count as seen; `/bypass off` says what it approved since the person
    // last looked; a chat that opens says how many are not looked at, and, once, that the judge
    // turned it off, with that count.
    #[test]
    fn bypass_needs_the_judge_and_tells_the_person_what_it_did() {
        let t = tempfile::tempdir().unwrap();
        let d = dirs_in(t.path());
        let st = state(&d);
        std::fs::create_dir_all(&st).unwrap();
        with_cfg(&d, 4, false);
        assert!(bypass_ask(&d, false).unwrap_err().contains("judge is off"));
        assert!(bypass_on(&d, false).unwrap_err().contains("judge is off"));
        assert!(grant::Kind::Bypass.current(&st).is_none());
        with_cfg(&d, 4, true);
        assert!(
            bypass_ask(&d, true)
                .unwrap_err()
                .contains("tried to give orders")
        );
        assert!(
            bypass_on(&d, true)
                .unwrap_err()
                .contains("tried to give orders")
        );
        assert!(grant::Kind::Bypass.current(&st).is_none());
        let c = bypass_ask(&d, false).unwrap();
        assert_eq!(c.action, super::super::tui::app::Action::Bypass);
        assert!(c.action.timed());
        let b = bypass_on(&d, false).unwrap();
        assert!(bypass_ask(&d, false).unwrap_err().contains("on already"));
        assert!(bypass_line(&d).starts_with("bypass is on since"));
        assert!(bypass_notes(&d).is_empty(), "nothing approved yet");
        // No window opens from a yes while it is on.
        assert!(person_said_yes(&d, &["a1".into()], false).is_none());
        let mut p: Pending = serde_json::from_value(serde_json::json!({"id": "c0",
            "class": "delete", "tool": "exec", "args": {"command": "rm -r /srv/old.bak"},
            "hash": "", "what": "", "run": "r1", "session": "s1", "created_ms": 0,
            "expires_ms": 1, "state": "waiting"}))
        .unwrap();
        let act = |p: &mut Pending, id: &str, what: &str| {
            p.id = id.into();
            let w = grant::Kind::Bypass.begin(&st, p, what).unwrap();
            grant::done(&st, &w, id, "done (exit 0)");
        };
        for id in ["c1", "c2"] {
            act(&mut p, id, &format!("rm -r /srv/{id}.bak"));
        }
        let notes = bypass_notes(&d);
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("approved 2 act(s) by herself"),
            "{notes:?}"
        );
        // Shown one of two: the one not shown still counts.
        let listed = bypass_list(&d, 1);
        let day = pending::local(b.opened_ms, "%Y-%m-%d").unwrap();
        assert!(
            listed[1].contains("2 act(s)")
                && listed[1].contains(&day)
                && listed[2].contains("1 earlier")
                && listed[3].contains("rm -r /srv/c2.bak")
                && listed[3].starts_with(&format!("  {day} ")),
            "{listed:?}"
        );
        assert!(bypass_notes(&d)[0].contains("approved 1 act(s)"));
        let listed = bypass_list(&d, 30);
        assert!(
            listed.iter().any(|l| l.contains("/srv/c1.bak")),
            "{listed:?}"
        );
        assert!(bypass_notes(&d).is_empty(), "looked at");
        act(&mut p, "c3", "rm -r /srv/c3.bak");
        let off = bypass_off(&d).unwrap();
        assert!(off[0].starts_with("bypass is off"), "{off:?}");
        assert_eq!(off.len(), 3, "only what was not seen: {off:?}");
        assert!(off[2].contains("rm -r /srv/c3.bak"), "{off:?}");
        assert_eq!(bypass_off(&d).unwrap(), ["bypass is off already"]);
        assert!(bypass_notes(&d).is_empty(), "the person turned it off");
        // On again, one act, then the judge flags an injection: one notice at the next chat open,
        // with the count; the list has both periods, the newest first.
        let b2 = bypass_on(&d, false).unwrap();
        assert_ne!(b2.id, b.id);
        act(&mut p, "c4", "rm -r /srv/c4.bak");
        grant::Kind::Bypass.close(&st, grant::FLAGGED).unwrap();
        let notes = bypass_notes(&d);
        assert!(
            notes[0].contains(grant::FLAGGED) && notes[0].contains("1 act(s)"),
            "{notes:?}"
        );
        assert!(
            bypass_notes(&d)[0].starts_with("bypass is off: Clio approved 1 act(s)"),
            "the notice once, the count until looked at"
        );
        let listed = bypass_list(&d, 30);
        let heads: Vec<usize> = listed
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("bypass ") && l.contains(" to "))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(heads.len(), 2, "{listed:?}");
        assert!(listed[heads[0] + 1].contains("/srv/c4.bak"), "{listed:?}");
        assert!(listed[heads[1]..].iter().any(|l| l.contains("/srv/c1.bak")));
        assert!(bypass_notes(&d).is_empty());
        // Found on with the judge off: off.
        bypass_on(&d, false).unwrap();
        with_cfg(&d, 4, false);
        assert!(bypass_now(&d).is_none());
        assert_eq!(
            grant::Kind::Bypass.read(&st).unwrap().why.as_deref(),
            Some(grant::JUDGE_OFF)
        );
    }

    // S-36: nothing but the person's yes opens a window: `grant::open` is called from one place
    // (`person_said_yes`), and that from two: the chat's answer to a box and `clio approve`.
    #[test]
    fn a_window_is_opened_from_the_persons_yes_only() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut opens = Vec::new();
        let mut yes = Vec::new();
        let mut todo = vec![src.clone()];
        while let Some(dir) = todo.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                let rel = p.strip_prefix(&src).unwrap().display().to_string();
                // Only the code, not its tests.
                if rel.contains("tests") {
                    continue;
                }
                let code = text.split("\nmod tests {").next().unwrap_or("");
                for (i, _) in code.match_indices("grant::open(") {
                    opens.push((rel.clone(), i));
                }
                for l in code.lines().filter(|l| l.contains("person_said_yes(")) {
                    yes.push((rel.clone(), l.trim().to_owned()));
                }
            }
        }
        let files: Vec<&str> = opens.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(files, ["cli/pending.rs"], "{opens:?}");
        let mut callers: Vec<&str> = yes
            .iter()
            .filter(|(_, l)| !l.starts_with("pub fn person_said_yes("))
            .map(|(f, _)| f.as_str())
            .collect();
        callers.sort_unstable();
        assert_eq!(callers, ["cli/pending.rs", "cli/tui/mod.rs"], "{yes:?}");
    }

    // S-36: a window that ended is summed up once: its record lists every act; claimed once; the
    // summary opens nothing. A window with no acts ends silently.
    #[test]
    fn an_ended_window_is_summed_up_once_and_opens_nothing() {
        let t = tempfile::tempdir().unwrap();
        let d = dirs_in(t.path());
        let st = state(&d);
        std::fs::create_dir_all(st.join("grants")).unwrap();
        let now = now_ms();
        let w = json!({"id": "w1", "opened_ms": now - 5 * 3_600_000, "ends_ms": now - 3_600_000, "by": ["a1"]});
        let lines = [
            json!({"window": w}).to_string(),
            json!({"at": now - 4 * 3_600_000, "pending": "b2", "class": "delete", "what": "rm -r /srv/old", "session": "s"}).to_string(),
            json!({"at": now - 4 * 3_600_000, "pending": "b2", "outcome": "done (exit 0)"}).to_string(),
            json!({"at": now - 3 * 3_600_000, "pending": "c3", "class": "memory", "what": "add in «vigilancia»", "session": "t"}).to_string(),
        ];
        std::fs::write(st.join("grants/w1.jsonl"), lines.join("\n") + "\n").unwrap();
        std::fs::write(grant::file(&st), w.to_string()).unwrap();
        let (due, acts) = grant::summary_due(&st).unwrap();
        assert_eq!((due.id.as_str(), acts.len()), ("w1", 2));
        let rec = summary_record(&due, &acts);
        for want in ["rm -r /srv/old", "done (exit 0)", "add in «vigilancia»"] {
            assert!(rec.contains(want), "{want}: {rec}");
        }
        assert!(grant::claim_summary(&st, "w1"));
        assert!(!grant::claim_summary(&st, "w1"), "once");
        assert!(grant::summary_due(&st).is_none());
        assert!(grant::current(&st).is_none(), "a summary opens nothing");
    }
}
