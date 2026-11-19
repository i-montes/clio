//! The permission window: a yes of the person in an approval box (or `clio approve`) lets Clio act
//! without asking for `approvals.grant_hours`. It lives in one file of Clio's data folder,
//! `<state>/grant.json` (`<data>/host`), read by every tool host and by the brain, and written
//! only by the person's `clio` when the person says yes (`open`); closing it (`close`) is the
//! person's (`/grant off`) or the judge's (the first `suspect` mark of any session). Nothing the
//! jail reaches can write it: it is under Clio's data, refused to the file tools and hidden from
//! commands.
//!
//! A window answers only acts whose effect ends with the act (`covers`): commands of class
//! `delete`, `spend` and `send`, an `ask` MCP tool call, a skill that would wait. It never answers
//! `control` (Clio's own commands and folders, the service manager, D-Bus, units, sudoers, keys,
//! schedulers: from there a fooled model could rewrite this file or schedule a yes), a secret, a
//! change to the person's blocks, a service or an MCP server: those always ask the person. No
//! window opens, and an open one closes, while the judge is off.
//!
//! What a window approves is recorded before it runs (`begin`) and its outcome after (`done`), in
//! `<state>/grants/<id>.jsonl`, whose first line is the window itself. A missing or damaged file
//! is no window; a window never outlives the boot it opened in, and never lasts longer than its
//! length by the wall clock or by the time since boot.

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
/// class delete, spend or send, an `ask` MCP tool call, a skill that would wait). Never control,
/// a secret, a change to the person's blocks, a service, an MCP server, a tool that changed since
/// the person confirmed it, or a suspect session's reach out: those always ask the person.
pub fn covers(p: &Pending) -> bool {
    match p.class {
        Class::Delete | Class::Spend | Class::Skill => true,
        Class::Send => p
            .reason
            .as_deref()
            .is_none_or(|r| r == crate::mcp::ASK_TOOL),
        Class::Control | Class::Secret | Class::Memory | Class::Service | Class::Mcp => false,
    }
}

/// `covers`, of an item set aside while window `w` was open: what waited from before it keeps
/// its box (the person never saw it). The one test both the tool host and the chat apply.
pub fn may_answer(w: &Window, p: &Pending) -> bool {
    covers(p) && p.created_ms >= w.opened_ms
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
    state.join("grant.json")
}

fn dir(state: &Path) -> PathBuf {
    state.join("grants")
}

fn record_file(state: &Path, id: &str) -> PathBuf {
    dir(state).join(format!("{id}.jsonl"))
}

/// The last window as written, open or not; `None` for a missing or damaged file.
pub fn read(state: &Path) -> Option<Window> {
    serde_json::from_str(&fs::read_to_string(file(state)).ok()?).ok()
}

/// The window open now, if any.
pub fn current(state: &Path) -> Option<Window> {
    read(state).filter(Window::open_now)
}

fn write(state: &Path, w: &Window) -> Result<(), String> {
    let e = |x: std::io::Error| format!("{}: {x}", file(state).display());
    private_dir(state).map_err(e)?;
    let text = serde_json::to_string_pretty(w).map_err(|x| x.to_string())?;
    let tmp = state.join("grant.json.tmp");
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
        .and_then(|()| fs::rename(&tmp, file(state)))
        .map_err(e)
}

fn append(state: &Path, id: &str, line: &Value) -> Result<(), String> {
    let path = record_file(state, id);
    let e = |x: std::io::Error| format!("{}: {x}", path.display());
    private_dir(&dir(state)).map_err(e)?;
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

/// The person said yes to `by`: a window of `hours` opens. Only the person's `clio` calls this,
/// for a yes typed in an approval box or `clio approve` (see `crates/clio-brain/src/cli/pending.rs`,
/// `person_said_yes`). `hours` 0, or a window already open (it is not stretched): none.
pub fn open(state: &Path, hours: u64, by: &[String]) -> Result<Option<Window>, String> {
    if hours == 0 || current(state).is_some() {
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
    };
    // The record first: a window with no record of its own never opens.
    append(state, &w.id, &json!({ "window": w }))?;
    write(state, &w)?;
    Ok(Some(w))
}

/// Closes the open window now, saying why: the person (`/grant off`) or the judge (a session
/// marked `suspect`). Nothing open: nothing changes.
pub fn close(state: &Path, why: &str) -> Result<Option<Window>, String> {
    let Some(mut w) = current(state) else {
        return Ok(None);
    };
    let now = now_ms();
    w.closed_ms = Some(now);
    w.why = Some(why.to_owned());
    write(state, &w)?;
    append(state, &w.id, &json!({ "at": now, "closed": why }))?;
    Ok(Some(w))
}

/// An act of the window is about to run: when the window is still open now, it is recorded
/// (when, the item, its class, what it does, the session) before anything runs, and the window
/// is returned; else (closed, ended, the record not writable) `None`, and the item waits for the
/// person as it would without a window.
pub fn begin(state: &Path, p: &Pending, what: &str) -> Option<Window> {
    let w = current(state)?;
    let line = json!({
        "at": now_ms(),
        "pending": p.id,
        "class": p.class.to_string(),
        "what": what,
        "session": p.session,
        "run": p.run,
    });
    append(state, &w.id, &line).ok()?;
    Some(w)
}

/// What came of an act `begin` recorded: `outcome`'s words or `NOT_RUN`.
pub fn done(state: &Path, w: &Window, id: &str, outcome: &str) {
    let line = json!({ "at": now_ms(), "pending": id, "outcome": first_line(outcome, 300) });
    if let Err(e) = append(state, &w.id, &line) {
        eprintln!("clio: the permission window's record lost a line: {e}");
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
    let text = fs::read_to_string(record_file(state, id)).ok()?;
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

/// Every window recorded, newest first.
pub fn windows(state: &Path) -> Vec<(Window, Vec<Act>)> {
    let mut all: Vec<(Window, Vec<Act>)> = fs::read_dir(dir(state))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            record(state, name.strip_suffix(".jsonl")?)
        })
        .collect();
    all.sort_by_key(|(w, _)| std::cmp::Reverse(w.opened_ms));
    all
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
            && !dir(state).join(format!("{}.told", w.id)).exists()
    })
}

/// The summary of window `id` is given now, once: the first to claim it (any conversation, any
/// process) gives it, just before it sends it; `release_summary` when the send failed.
pub fn claim_summary(state: &Path, id: &str) -> bool {
    private_dir(&dir(state)).is_ok()
        && fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir(state).join(format!("{id}.told")))
            .is_ok()
}

/// The summary of window `id` could not be given after all: it is owed again.
pub fn release_summary(state: &Path, id: &str) {
    let _ = fs::remove_file(dir(state).join(format!("{id}.told")));
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
    acts.iter()
        .map(|a| {
            let at = crate::pending::local(a.at, "%H:%M").unwrap_or_else(|| a.at.to_string());
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
}
