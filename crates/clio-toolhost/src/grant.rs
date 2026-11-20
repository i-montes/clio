//! The permission window: a yes of the person in an approval box (or `clio approve`) lets Clio act
//! without asking for `approvals.grant_hours`. It lives in one file of Clio's data folder,
//! `<state>/grant.json` (`<data>/host`), read by every tool host and by the brain, and written
//! only by the person's `clio` when the person says yes (`open`); closing it (`close`) is the
//! person's (`/grant off`) or the judge's (the first `suspect` mark of any session). Nothing the
//! jail reaches can write it: it is under Clio's data, refused to the file tools and hidden from
//! commands.
//!
//! A window answers only acts whose effect ends with the act (`covers`): commands of class
//! `delete`, `spend` and `send`, an `ask` MCP tool call, a skill that would wait, and turning what
//! Clio created back on, back or away (`manage`: `on`, `undo`, `remove`, kept in `removed/`). It
//! never answers `control` (Clio's own commands and folders, the service manager, D-Bus, units,
//! sudoers, keys, schedulers: from there a fooled model could rewrite this file or schedule a
//! yes), a secret, a change to the person's blocks, a new service or MCP server, `on` of what the
//! person turned off or `remove` of a skill Clio ships: those always ask the person. No window
//! opens, and an open one closes, while the judge is off.
//!
//! What a window approves is recorded before it runs (`begin`) and its outcome after (`done`), in
//! `<state>/grants/<id>.jsonl`, whose first line is the window itself. A missing or damaged file
//! is no window; a window never outlives the boot it opened in, and never lasts longer than its
//! length by the wall clock or by the time since boot.
//!
//! Bypass (`2026-11-20-bypass-design.md`) is the person's standing yes, kept the same way in
//! `<state>/bypass.json` and recorded in `<state>/bypass/<id>.jsonl` (`Kind::Bypass`): a window
//! with no end or clock that outlives restarts, on only while its record starts with it (a file
//! written by hand is off), turned on only by the chat's box (`bypass_on`), off by the person or
//! the judge. It answers what a window does and more (`bypass_answers`), never what would change
//! Clio's own guards or run new code outside the jail, a change to the person's blocks, a secret
//! or a suspect session's act. While it is on no window opens.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ask::Class;
use crate::host::{now_ms, private_dir};
use crate::pending::Pending;

/// Why a window closed when the judge flagged an injection.
pub const FLAGGED: &str = "the judge flagged an injection in a conversation";
/// Why a window closed when the person closed it.
pub const BY_PERSON: &str = "closed by you";
/// Why a window closed when an act found the judge off.
pub const JUDGE_OFF: &str = "the judge is off";
/// The outcome recorded for an act that could not run (never its error's words: outside text).
pub const NOT_RUN: &str = "failed: it could not run";
/// What bypass's ids start with: an item approved under it says so (`Pending::granted`).
pub const BYPASS_ID: &str = "bypass-";
/// Who turned bypass on: the chat's box, the only way it turns on.
pub const BY_BOX: &str = "the chat's /bypass on box";

/// Which of the two a file holds: a permission window or bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Window,
    Bypass,
}

/// How long a summary is still owed for a window that ended: older ones are only listed.
const OWED_MS: i64 = 7 * 24 * 3600 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub id: String,
    pub opened_ms: i64,
    pub ends_ms: i64,
    /// The time since boot when it opened (`mono_ms`): a wall clock set back cannot stretch it.
    #[serde(default)]
    pub mono_ms: Option<i64>,
    /// The boot it opened in (`boot_id`): another boot ends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<String>,
    /// The approvals whose yes opened it.
    pub by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Bypass, not a window: it has no end and outlives restarts (`Kind::Bypass`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass: bool,
}

impl Window {
    /// Open at wall time `now`, time since boot `mono`, on boot `boot`: after it opened, before
    /// its end, not closed, on the boot it opened in (another boot id, or less time since boot
    /// than at the opening, ends it), and not longer than its length since boot either.
    pub fn open_at(&self, now: i64, mono: Option<i64>, boot: Option<&str>) -> bool {
        if self.closed_ms.is_some() || now < self.opened_ms || now >= self.ends_ms {
            return false;
        }
        if let (Some(a), Some(b)) = (self.boot.as_deref(), boot)
            && a != b
        {
            return false;
        }
        let len = self.ends_ms - self.opened_ms;
        !matches!((self.mono_ms, mono), (Some(a), Some(b)) if b < a || b - a >= len)
    }

    /// Open now, by every clock this machine has.
    pub fn open_now(&self) -> bool {
        self.open_at(now_ms(), mono_ms(), boot_id().as_deref())
    }

    /// When it ended, or ends.
    pub fn end(&self) -> i64 {
        self.closed_ms.unwrap_or(self.ends_ms).min(self.ends_ms)
    }
}

/// The time since boot, in ms: Linux counts suspend too (`CLOCK_BOOTTIME`), as macOS's
/// `CLOCK_MONOTONIC` does.
pub fn mono_ms() -> Option<i64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let id = rustix::time::ClockId::Boottime;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let id = rustix::time::ClockId::Monotonic;
    let t = rustix::time::clock_gettime(id);
    Some(t.tv_sec.checked_mul(1000)? + (t.tv_nsec / 1_000_000))
}

/// This boot's id (Linux); `None` where the system does not say.
pub fn boot_id() -> Option<String> {
    let id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(id.trim().to_owned()).filter(|s| !s.is_empty())
}

/// May a window answer `p` by itself: only an act whose effect ends with the act (a command of
/// class delete, spend or send, an `ask` MCP tool call, a skill that would wait, `on`, `undo` or
/// `remove` of what Clio created). Never control, a secret, a change to the person's blocks, a new
/// service or MCP server, a tool that changed since the person confirmed it, a suspect session's
/// act, `on` of what the person turned off or `remove` of a shipped skill: those always ask.
pub fn covers(p: &Pending) -> bool {
    // Turning back on, back or away what Clio created ends with the act and can be recovered
    // (`removed/`); one that carries a reason (the person turned it off, a skill Clio ships, a
    // suspect session) always asks. Registering a service or a server never comes here.
    let manage = crate::manage::is_manage(p);
    match p.class {
        Class::Delete | Class::Spend => true,
        Class::Skill => !manage || p.reason.is_none(),
        Class::Service | Class::Mcp => manage && p.reason.is_none(),
        Class::Send => p
            .reason
            .as_deref()
            .is_none_or(|r| r == crate::mcp::ASK_TOOL),
        Class::Control | Class::Secret | Class::Memory => false,
    }
}

/// `covers`, of an item set aside while window `w` was open: what waited from before it keeps
/// its box (the person never saw it). The one test both the tool host and the chat apply.
pub fn may_answer(w: &Window, p: &Pending) -> bool {
    covers(p) && p.created_ms >= w.opened_ms
}

/// May bypass answer `p` by itself: what a window answers and what a window never does (`control`
/// on the machine, a new service or MCP server or a new version, `on` of what the person turned
/// off, `remove` or `undo` of a shipped skill, a service's `undo`, a changed MCP tool, what waited
/// from before). Never what would change Clio's own guards or run new code outside the jail (a
/// `control` the list did not mark `ask::MACHINE` when it classified it), a secret, a change to
/// the person's blocks (only a marked session's waits), a suspect session's act or a round's
/// reach out: those always ask.
pub fn bypass_answers(p: &Pending) -> bool {
    match p.class {
        Class::Control => p.reason.as_deref() == Some(crate::ask::MACHINE),
        Class::Secret | Class::Memory => false,
        _ => !p
            .reason
            .as_deref()
            .is_some_and(|r| r == crate::SUSPECT_REASON || r == crate::ROUND_REACHES),
    }
}

/// What answers `p` by itself now: bypass when it is on and may (it answers all a window does),
/// else the open window when it may. The one test the tool host and the chat apply; the judge,
/// a suspect session, a review and a round are theirs to check first.
pub fn answering(state: &Path, p: &Pending) -> Option<Window> {
    Kind::Bypass
        .current(state)
        .filter(|_| bypass_answers(p))
        .or_else(|| current(state).filter(|w| may_answer(w, p)))
}

/// What came of an act, in fixed words: never its output, which may be outside text.
pub fn outcome(ok: bool, exit: Option<i64>) -> String {
    match exit {
        Some(0) => "done (exit 0)".into(),
        Some(c) => format!("failed (exit {c})"),
        None if ok => "done".into(),
        None => "failed: the tool reported an error".into(),
    }
}

pub fn file(state: &Path) -> PathBuf {
    Kind::Window.file(state)
}

/// The last window as written, open or not; `None` for a missing or damaged file.
pub fn read(state: &Path) -> Option<Window> {
    Kind::Window.read(state)
}

/// The window open now, if any.
pub fn current(state: &Path) -> Option<Window> {
    Kind::Window.current(state)
}

impl Kind {
    /// Which `w` is.
    pub fn of(w: &Window) -> Self {
        if w.bypass { Self::Bypass } else { Self::Window }
    }

    /// What the person calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Window => "permission window",
            Self::Bypass => "bypass",
        }
    }

    pub fn file(self, state: &Path) -> PathBuf {
        state.join(match self {
            Self::Window => "grant.json",
            Self::Bypass => "bypass.json",
        })
    }

    fn dir(self, state: &Path) -> PathBuf {
        state.join(match self {
            Self::Window => "grants",
            Self::Bypass => "bypass",
        })
    }

    fn record_file(self, state: &Path, id: &str) -> PathBuf {
        self.dir(state).join(format!("{id}.jsonl"))
    }

    /// The last one as written, on or not; `None` for a missing or damaged file, or one that
    /// holds the other kind. Bypass is trusted only with its record: a file whose record does not
    /// start with this same bypass (a file written by hand) is none.
    pub fn read(self, state: &Path) -> Option<Window> {
        let w: Window = serde_json::from_str(&fs::read_to_string(self.file(state)).ok()?).ok()?;
        let recorded = || {
            let first = fs::read_to_string(self.record_file(state, &w.id)).ok()?;
            let first: Value = serde_json::from_str(first.lines().next()?).ok()?;
            serde_json::from_value::<Window>(first.get("window")?.clone()).ok()
        };
        let as_started = Window {
            closed_ms: None,
            why: None,
            ..w.clone()
        };
        (Self::of(&w) == self && (self == Self::Window || recorded() == Some(as_started)))
            .then_some(w)
    }

    /// The one open (on) now, if any. Bypass has no end and no clock: on until it is closed.
    pub fn current(self, state: &Path) -> Option<Window> {
        self.read(state).filter(|w| match self {
            Self::Window => w.open_now(),
            Self::Bypass => w.closed_ms.is_none(),
        })
    }

    fn write(self, state: &Path, w: &Window) -> Result<(), String> {
        let file = self.file(state);
        let e = |x: std::io::Error| format!("{}: {x}", file.display());
        private_dir(state).map_err(e)?;
        let text = serde_json::to_string_pretty(w).map_err(|x| x.to_string())?;
        let tmp = file.with_extension("json.tmp");
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
            .and_then(|()| fs::rename(&tmp, &file))
            .map_err(e)
    }

    fn append(self, state: &Path, id: &str, line: &Value) -> Result<(), String> {
        let path = self.record_file(state, id);
        let e = |x: std::io::Error| format!("{}: {x}", path.display());
        private_dir(&self.dir(state)).map_err(e)?;
        let mut text = line.to_string();
        text.push('\n');
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
            .map_err(e)
    }

    /// Writes `w` new: its record first (one with no record of its own never opens), then its
    /// file.
    fn start(self, state: &Path, w: &Window) -> Result<(), String> {
        self.append(state, &w.id, &json!({ "window": w }))?;
        self.write(state, w)
    }

    /// Closes the open one now, saying why: the person (`/grant off`, `/bypass off`) or the
    /// judge (a session marked `suspect`, or found off). Nothing open: nothing changes.
    pub fn close(self, state: &Path, why: &str) -> Result<Option<Window>, String> {
        let Some(mut w) = self.current(state) else {
            return Ok(None);
        };
        let now = now_ms();
        w.closed_ms = Some(now);
        w.why = Some(why.to_owned());
        if let Err(e) = self.write(state, &w) {
            // Bypass that cannot be written closed still ends: without its file it is off.
            if self == Self::Window || fs::remove_file(self.file(state)).is_err() {
                return Err(e);
            }
        }
        self.append(state, &w.id, &json!({ "at": now, "closed": why }))?;
        Ok(Some(w))
    }

    /// An act it answers is about to run: when it is still open now, the act is recorded (when,
    /// the item, its class, what it does, the session) before anything runs, and it is returned;
    /// else (closed, ended, the record not writable) `None`, and the item waits for the person.
    pub fn begin(self, state: &Path, p: &Pending, what: &str) -> Option<Window> {
        let w = self.current(state)?;
        let line = json!({
            "at": now_ms(),
            "pending": p.id,
            "class": p.class.to_string(),
            "what": what,
            "session": p.session,
            "run": p.run,
        });
        self.append(state, &w.id, &line).ok()?;
        Some(w)
    }

    /// Id `id` as its record has it, and its acts in order.
    pub fn record(self, state: &Path, id: &str) -> Option<(Window, Vec<Act>)> {
        let text = fs::read_to_string(self.record_file(state, id)).ok()?;
        let mut lines = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok());
        let mut w: Window = serde_json::from_value(lines.next()?.get("window")?.clone()).ok()?;
        let mut acts: Vec<Act> = Vec::new();
        let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_owned();
        for v in lines {
            if let Some(why) = v["closed"].as_str() {
                w.closed_ms = v["at"].as_i64();
                w.why = Some(why.to_owned());
            } else if let Some(o) = v["outcome"].as_str() {
                if let Some(a) = acts
                    .iter_mut()
                    .rev()
                    .find(|a| a.pending == s(&v, "pending"))
                {
                    a.outcome = Some(o.to_owned());
                }
            } else {
                acts.push(Act {
                    at: v["at"].as_i64().unwrap_or(0),
                    pending: s(&v, "pending"),
                    class: s(&v, "class"),
                    what: s(&v, "what"),
                    session: s(&v, "session"),
                    run: s(&v, "run"),
                    outcome: None,
                });
            }
        }
        Some((w, acts))
    }

    /// Every one recorded, newest first.
    pub fn all(self, state: &Path) -> Vec<(Window, Vec<Act>)> {
        let mut all: Vec<(Window, Vec<Act>)> = fs::read_dir(self.dir(state))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                self.record(state, name.strip_suffix(".jsonl")?)
            })
            .collect();
        all.sort_by_key(|(w, _)| std::cmp::Reverse(w.opened_ms));
        all
    }

    /// `<id>.<what>` is taken now, once: the first to claim it (any conversation, any process)
    /// does what it marks.
    fn claim(self, state: &Path, id: &str, what: &str) -> bool {
        private_dir(&self.dir(state)).is_ok()
            && fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(self.dir(state).join(format!("{id}.{what}")))
                .is_ok()
    }
}

/// The person said yes to `by`: a window of `hours` opens. Only the person's `clio` calls this,
/// for a yes typed in an approval box or `clio approve` (see `crates/clio-brain/src/cli/pending.rs`,
/// `person_said_yes`). `hours` 0, a window already open (it is not stretched), or bypass on:
/// none.
pub fn open(state: &Path, hours: u64, by: &[String]) -> Result<Option<Window>, String> {
    if hours == 0 || current(state).is_some() || Kind::Bypass.current(state).is_some() {
        return Ok(None);
    }
    let now = now_ms();
    let len = i64::try_from(hours.saturating_mul(3_600_000)).unwrap_or(i64::MAX);
    let w = Window {
        id: now.to_string(),
        opened_ms: now,
        ends_ms: now.saturating_add(len),
        mono_ms: mono_ms(),
        boot: boot_id(),
        by: by.to_vec(),
        closed_ms: None,
        why: None,
        bypass: false,
    };
    Kind::Window.start(state, &w)?;
    Ok(Some(w))
}

/// The person said yes in the chat's `/bypass on` box: bypass turns on, with no end, until the
/// person or the judge turns it off. Only that box calls this (see
/// `crates/clio-brain/src/cli/pending.rs`, `bypass_on`), with the judge on; on already: as it is.
pub fn bypass_on(state: &Path) -> Result<Window, String> {
    if let Some(w) = Kind::Bypass.current(state) {
        return Ok(w);
    }
    let now = now_ms();
    let w = Window {
        id: format!("{BYPASS_ID}{now}"),
        opened_ms: now,
        ends_ms: i64::MAX,
        mono_ms: None,
        boot: None,
        by: vec![BY_BOX.into()],
        closed_ms: None,
        why: None,
        bypass: true,
    };
    Kind::Bypass.start(state, &w)?;
    Ok(w)
}

/// What bypass `id` approved that the person has not been shown yet (`bypass_seen`).
pub fn bypass_unseen(state: &Path, id: &str) -> Vec<Act> {
    let seen =
        fs::read_to_string(Kind::Bypass.dir(state).join(format!("{id}.seen"))).unwrap_or_default();
    let seen: Vec<&str> = seen.lines().collect();
    Kind::Bypass
        .record(state, id)
        .map(|(_, acts)| {
            acts.into_iter()
                .filter(|a| !seen.contains(&a.pending.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// The person was shown `acts` of bypass `id` (`/bypass`, `/bypass off`, `clio bypass`): those,
/// and only those, are no longer counted.
pub fn bypass_seen(state: &Path, id: &str, acts: &[Act]) {
    if acts.is_empty() {
        return;
    }
    let path = Kind::Bypass.dir(state).join(format!("{id}.seen"));
    let text: String = acts.iter().map(|a| format!("{}\n", a.pending)).collect();
    let r = private_dir(&Kind::Bypass.dir(state)).and_then(|()| {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| f.write_all(text.as_bytes()))
    });
    if let Err(e) = r {
        eprintln!("clio: {}: {e}", path.display());
    }
}

/// Bypass the judge turned off (it flagged an injection, or was found off) whose notice was not
/// given yet: claimed now, once, for the next chat that opens.
pub fn bypass_off_notice(state: &Path) -> Option<Window> {
    let w = Kind::Bypass.read(state)?;
    (w.closed_ms.is_some()
        && w.why.as_deref() != Some(BY_PERSON)
        && Kind::Bypass.claim(state, &w.id, "told"))
    .then_some(w)
}

/// Closes the open window now, saying why (`Kind::close`).
pub fn close(state: &Path, why: &str) -> Result<Option<Window>, String> {
    Kind::Window.close(state, why)
}

/// An act of the window is about to run (`Kind::begin`).
pub fn begin(state: &Path, p: &Pending, what: &str) -> Option<Window> {
    Kind::Window.begin(state, p, what)
}

/// What came of an act `begin` recorded: `outcome`'s words or `NOT_RUN`.
pub fn done(state: &Path, w: &Window, id: &str, outcome: &str) {
    let line = json!({ "at": now_ms(), "pending": id, "outcome": first_line(outcome, 300) });
    if let Err(e) = Kind::of(w).append(state, &w.id, &line) {
        eprintln!("clio: the record of {} lost a line: {e}", w.id);
    }
}

fn first_line(s: &str, max: usize) -> String {
    let l = s.trim().lines().next().unwrap_or("").trim();
    let mut out: String = l.chars().take(max).collect();
    if out.len() < l.len() {
        out.push('…');
    }
    out
}

/// One act of a window: when it started, its item, class, what it did, its session, and what
/// came of it (`None`: nothing recorded yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Act {
    pub at: i64,
    pub pending: String,
    pub class: String,
    pub what: String,
    pub session: String,
    /// The conversation's run folder.
    pub run: String,
    pub outcome: Option<String>,
}

/// Window `id` as its record has it, and its acts in order.
pub fn record(state: &Path, id: &str) -> Option<(Window, Vec<Act>)> {
    Kind::Window.record(state, id)
}

/// Every window recorded, newest first.
pub fn windows(state: &Path) -> Vec<(Window, Vec<Act>)> {
    Kind::Window.all(state)
}

/// A window that ended (at its time or closed) in the last week, with acts, whose summary was not
/// given yet: the oldest first. Read only: `claim_summary` takes it.
pub fn summary_due(state: &Path) -> Option<(Window, Vec<Act>)> {
    let now = now_ms();
    windows(state).into_iter().rev().find(|(w, acts)| {
        // The record's own copy may lag the file (a close): the file says whether it is open.
        let closed_since = read(state).is_some_and(|c| c.id == w.id && !c.open_now());
        (closed_since || !w.open_now())
            && !acts.is_empty()
            && now.saturating_sub(w.end()) < OWED_MS
            && !Kind::Window
                .dir(state)
                .join(format!("{}.told", w.id))
                .exists()
    })
}

/// The summary of window `id` is given now, once: the first to claim it (any conversation, any
/// process) gives it, just before it sends it; `release_summary` when the send failed.
pub fn claim_summary(state: &Path, id: &str) -> bool {
    Kind::Window.claim(state, id, "told")
}

/// The summary of window `id` could not be given after all: it is owed again.
pub fn release_summary(state: &Path, id: &str) {
    let _ = fs::remove_file(Kind::Window.dir(state).join(format!("{id}.told")));
}

/// `3 h 12 min`, `25 min`, `less than a minute`.
pub fn left_text(ms: i64) -> String {
    let min = ms.max(0) / 60_000;
    match (min / 60, min % 60) {
        (0, 0) => "less than a minute".into(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// What the window approved, one line per act, for the person (`clio grant`) and for the model's
/// summary: the local time, the class, what it was and what came of it.
pub fn act_lines(acts: &[Act]) -> Vec<String> {
    lines_at(acts, "%H:%M")
}

/// `act_lines` with the date: bypass has no end, its acts span days.
pub fn bypass_act_lines(acts: &[Act]) -> Vec<String> {
    lines_at(acts, "%Y-%m-%d %H:%M")
}

fn lines_at(acts: &[Act], fmt: &str) -> Vec<String> {
    acts.iter()
        .map(|a| {
            let at = crate::pending::local(a.at, fmt).unwrap_or_else(|| a.at.to_string());
            let outcome = a
                .outcome
                .as_deref()
                .unwrap_or("no outcome recorded (it may still run, or Clio ended while it ran)");
            format!(
                "{at}  «{}»  {}  ({}) → {outcome}",
                a.class, a.what, a.pending
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn w(opened: i64, ends: i64, mono: Option<i64>) -> Window {
        Window {
            id: "1".into(),
            opened_ms: opened,
            ends_ms: ends,
            mono_ms: mono,
            boot: Some("b1".into()),
            by: vec!["a1".into()],
            closed_ms: None,
            why: None,
            bypass: false,
        }
    }

    const H: i64 = 3_600_000;

    // A clock set back never stretches a window: the time since boot ends it when its length has
    // passed, whatever the wall clock says; one set back before the opening closes it. A window
    // never outlives the boot it opened in, even with the clock set back and a reboot.
    #[test]
    fn a_clock_set_back_does_not_extend_a_window() {
        let b = Some("b1");
        let x = w(10 * H, 14 * H, Some(1_000));
        assert!(x.open_at(12 * H, Some(1_000 + 2 * H), b));
        // Five real hours later the wall clock says 13:00: closed all the same.
        assert!(!x.open_at(13 * H, Some(1_000 + 5 * H), b));
        // Before it opened: closed.
        assert!(!x.open_at(9 * H, Some(1_000 + H), b));
        // At its end, and after: closed.
        assert!(!x.open_at(14 * H, Some(1_000 + 4 * H), b));
        // Another boot: closed, whatever the wall clock says, by the boot id or, where there is
        // none, by less time since boot than at the opening.
        assert!(!x.open_at(11 * H, Some(5), b));
        assert!(!x.open_at(11 * H, Some(1_000 + H), Some("b2")));
        let mut y = x.clone();
        y.boot = None;
        assert!(!y.open_at(11 * H, Some(5), None));
        assert!(y.open_at(11 * H, Some(1_000 + H), None));
        // No time since boot known: the wall clock alone.
        assert!(w(0, H, None).open_at(H / 2, Some(7), b));
        let mut c = w(0, H, None);
        c.closed_ms = Some(1);
        assert!(!c.open_at(H / 2, None, b));
    }

    // Only what ends with the act: delete, spend, send (an `ask` MCP call too), a skill; never
    // control, a secret, a block change, a service, an MCP server, a changed tool or a suspect
    // session's reach out; and, inside a window, only what was set aside after it opened.
    #[test]
    fn a_window_answers_only_what_ends_with_the_act() {
        let mut p = crate::pending::tests_item("s");
        for (class, yes) in [
            (Class::Delete, true),
            (Class::Spend, true),
            (Class::Send, true),
            (Class::Skill, true),
            (Class::Control, false),
            (Class::Secret, false),
            (Class::Memory, false),
            (Class::Service, false),
            (Class::Mcp, false),
        ] {
            p.class = class;
            assert_eq!(covers(&p), yes, "{class}");
        }
        p.class = Class::Send;
        p.reason = Some(crate::mcp::ASK_TOOL.into());
        assert!(covers(&p));
        p.reason = Some("the tool changed since the person confirmed it".into());
        assert!(!covers(&p));
        p.reason = Some(crate::SUSPECT_REASON.into());
        assert!(!covers(&p));
        p.reason = None;
        let x = w(p.created_ms, p.created_ms + H, None);
        assert!(may_answer(&x, &p));
        let x = w(p.created_ms + 1, p.created_ms + H, None);
        assert!(!may_answer(&x, &p), "set aside before the window opened");
        assert_eq!(outcome(true, Some(0)), "done (exit 0)");
        assert_eq!(outcome(true, Some(2)), "failed (exit 2)");
        assert_eq!(outcome(true, None), "done");
        assert_eq!(outcome(false, None), "failed: the tool reported an error");
    }

    // A missing or damaged file is no window; `hours` 0 opens none; an open window is not
    // stretched by another yes; closing records why; the record has what began and its outcome.
    #[test]
    fn a_window_opens_closes_and_records_what_it_approved() {
        let t = tempfile::tempdir().unwrap();
        let st = t.path();
        assert!(current(st).is_none());
        fs::write(file(st), "{not json").unwrap();
        assert!(current(st).is_none());
        assert!(open(st, 0, &["a1".into()]).unwrap().is_none());
        assert!(current(st).is_none());
        let opened = open(st, 4, &["a1".into()]).unwrap().unwrap();
        assert_eq!(opened.ends_ms - opened.opened_ms, 4 * H);
        assert!(
            open(st, 9, &["b2".into()]).unwrap().is_none(),
            "not stretched"
        );
        assert_eq!(current(st).unwrap(), opened);
        let mode = fs::metadata(file(st)).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        let mut p = crate::pending::tests_item("s1");
        p.id = "c3".into();
        let w = begin(st, &p, "rm -r /srv/old").unwrap();
        done(st, &w, "c3", "done (exit 0)\nmore lines");
        assert_eq!(record(st, &opened.id).unwrap().1[0].run, "r");
        assert!(summary_due(st).is_none(), "still open");
        let closed = close(st, "closed by you").unwrap().unwrap();
        assert_eq!(closed.why.as_deref(), Some("closed by you"));
        assert!(current(st).is_none());
        assert!(
            begin(st, &p, "rm -r /srv/old").is_none(),
            "closed: it waits"
        );
        let (rw, acts) = record(st, &opened.id).unwrap();
        assert_eq!(rw.why.as_deref(), Some("closed by you"));
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].what, "rm -r /srv/old");
        assert_eq!(acts[0].outcome.as_deref(), Some("done (exit 0)"));
        let (due, _) = summary_due(st).unwrap();
        assert_eq!(due.id, opened.id);
        assert!(claim_summary(st, &due.id));
        assert!(!claim_summary(st, &due.id), "once");
        assert!(summary_due(st).is_none());
        // The send failed: it is owed again.
        release_summary(st, &due.id);
        assert_eq!(summary_due(st).unwrap().0.id, opened.id);
        assert!(claim_summary(st, &due.id));
        assert_eq!(left_text(3 * H + 12 * 60_000), "3 h 12 min");
        assert_eq!(left_text(25 * 60_000), "25 min");
    }

    // A window in which nothing was approved owes no summary: it ends silently.
    #[test]
    fn a_window_with_nothing_approved_ends_silently() {
        let t = tempfile::tempdir().unwrap();
        let st = t.path();
        open(st, 4, &["a1".into()]).unwrap().unwrap();
        close(st, "closed by you").unwrap();
        assert!(summary_due(st).is_none());
    }
    // S-72: bypass answers each class a window does and what a window never does (control on the
    // machine, a service, an MCP server, `on` of what the person turned off, a changed tool, what
    // waited from before), and never what would change Clio's own guards (control without the
    // list's machine mark), a secret, a block change, a suspect session's act or a round's.
    #[test]
    fn bypass_answers_more_than_a_window_and_never_clios_own_guards() {
        let mut p = crate::pending::tests_item("s");
        p.created_ms = 0;
        for (class, yes) in [
            (Class::Delete, true),
            (Class::Spend, true),
            (Class::Send, true),
            (Class::Skill, true),
            (Class::Service, true),
            (Class::Mcp, true),
            (Class::Control, false),
            (Class::Secret, false),
            (Class::Memory, false),
        ] {
            p.class = class;
            assert_eq!(bypass_answers(&p), yes, "{class}");
        }
        p.class = Class::Control;
        p.reason = Some(crate::ask::MACHINE.into());
        assert!(bypass_answers(&p), "control on the machine");
        assert!(!covers(&p), "never under a window");
        p.reason = Some("a round changes no service".into());
        assert!(!bypass_answers(&p));
        p.class = Class::Send;
        for (why, yes) in [
            (crate::mcp::ASK_TOOL, true),
            ("the tool changed since the person confirmed it", true),
            (crate::manage::PERSON_OFF, true),
            (crate::SUSPECT_REASON, false),
            (crate::ROUND_REACHES, false),
        ] {
            p.reason = Some(why.into());
            assert_eq!(bypass_answers(&p), yes, "{why}");
        }
        p.class = Class::Memory;
        p.reason = None;
        assert!(!bypass_answers(&p), "a marked session's block change");
    }

    // S-72: a missing or damaged bypass file, or one that holds a window, is off; only
    // `bypass_on` turns it on, with no end, on any boot and any clock since boot (a restart does
    // not turn it off); while it is on no window opens and it answers before one; `close` turns
    // it off; the record has what it approved; the person's look is remembered; the judge's
    // notice is given once, the person's own off has none.
    #[test]
    fn bypass_is_kept_like_a_window_has_no_end_and_closes() {
        let t = tempfile::tempdir().unwrap();
        let st = t.path();
        assert!(Kind::Bypass.current(st).is_none());
        fs::write(Kind::Bypass.file(st), "{not json").unwrap();
        assert!(Kind::Bypass.current(st).is_none(), "damaged");
        let w = open(st, 4, &["a1".into()]).unwrap().unwrap();
        fs::copy(file(st), Kind::Bypass.file(st)).unwrap();
        assert!(
            Kind::Bypass.current(st).is_none(),
            "a window's file is no bypass"
        );
        close(st, BY_PERSON).unwrap();
        let b = bypass_on(st).unwrap();
        assert!(b.bypass && b.id.starts_with(BYPASS_ID) && b.by == [BY_BOX]);
        assert_eq!(bypass_on(st).unwrap(), b, "on already: as it is");
        assert!(b.open_at(now_ms() + 400 * 24 * H, Some(5), Some("another boot")));
        assert_eq!(Kind::Bypass.current(st).unwrap(), b);
        let mode = fs::metadata(Kind::Bypass.file(st)).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        assert!(
            open(st, 4, &["a2".into()]).unwrap().is_none(),
            "no window opens"
        );
        assert!(current(st).is_none() && w.closed_ms.is_none());
        let mut p = crate::pending::tests_item("s1");
        p.id = "c3".into();
        p.created_ms = 0;
        assert_eq!(
            answering(st, &p).unwrap().id,
            b.id,
            "what waited from before too"
        );
        p.class = Class::Control;
        assert!(answering(st, &p).is_none(), "Clio's own guards");
        p.class = Class::Delete;
        let x = Kind::Bypass.begin(st, &p, "rm -r /srv/old").unwrap();
        done(st, &x, "c3", "done (exit 0)");
        assert_eq!(bypass_unseen(st, &b.id).len(), 1);
        let (_, acts) = Kind::Bypass.record(st, &b.id).unwrap();
        assert_eq!(acts[0].outcome.as_deref(), Some("done (exit 0)"));
        assert!(record(st, &b.id).is_none(), "not in the windows' record");
        bypass_seen(st, &b.id, &acts);
        assert!(bypass_unseen(st, &b.id).is_empty());
        assert!(bypass_off_notice(st).is_none(), "on: no notice");
        Kind::Bypass.close(st, FLAGGED).unwrap().unwrap();
        assert!(Kind::Bypass.current(st).is_none() && answering(st, &p).is_none());
        assert!(Kind::Bypass.begin(st, &p, "rm").is_none(), "off: it waits");
        assert_eq!(bypass_off_notice(st).unwrap().why.as_deref(), Some(FLAGGED));
        assert!(bypass_off_notice(st).is_none(), "once");
        let b2 = bypass_on(st).unwrap();
        assert_ne!(b2.id, b.id);
        Kind::Bypass.close(st, BY_PERSON).unwrap().unwrap();
        assert!(bypass_off_notice(st).is_none(), "the person turned it off");
        assert_eq!(Kind::Bypass.all(st).len(), 2);
        assert!(summary_due(st).is_none(), "bypass owes no window summary");
    }
}
