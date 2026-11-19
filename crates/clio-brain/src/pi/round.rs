//! The round (`docs/specs/2026-11-19-the-round-design.md`): when one is due, how its answer is
//! read, and what of it the person is told. The answer is read strictly, from one fenced
//! ```` ```round ```` block, as a review's is (`review.rs`); anything else is "the round could not
//! report", its text kept as data for the person and sent nowhere.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::text;

/// More than this is not read as an answer.
pub const MAX_ANSWER: usize = 16 * 1024;
/// More entries than this is not a round's answer.
pub const MAX_ENTRIES: usize = 20;
const MAX_KEY: usize = 40;
const MAX_NOTICE: usize = 600;
const MAX_RAW: usize = 4000;
/// A problem already told is told again ("still") after this long.
pub const STILL_MS: i64 = 24 * 3_600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Problem,
    Resolved,
    Info,
}

impl State {
    fn from_word(w: &str) -> Option<Self> {
        match w {
            "problem" => Some(Self::Problem),
            "resolved" => Some(Self::Resolved),
            "info" => Some(Self::Info),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub state: State,
    pub notice: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// `nothing`: nothing to tell.
    Nothing,
    Entries(Vec<Entry>),
    /// Not the block: why (Clio's words) and its text, cleaned and capped (data, sent nowhere).
    CouldNot {
        why: String,
        raw: Option<String>,
    },
}

/// `max` characters at most, an ellipsis marking the cut.
fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut c: String = s.chars().take(max.saturating_sub(1)).collect();
    c.push('…');
    c
}

/// A key: lowercase letters, digits and `-`, starting with a letter or digit, at most 40.
fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= MAX_KEY
        && k.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Reads the round's final message. The whole message must be one ```` ```round ```` block
/// holding either the single line `nothing`, or entries, each `- key:` then `state:` then
/// `notice:` (at any indentation; indented lines after `notice:` continue it). A key appears once;
/// at most `MAX_ENTRIES`. Control and invisible characters are removed first.
pub fn parse(answer: &str) -> Report {
    let bad = |why: &str| Report::CouldNot {
        why: why.to_owned(),
        raw: Some(cap(text::data(answer).trim(), MAX_RAW)).filter(|r| !r.is_empty()),
    };
    if answer.len() > MAX_ANSWER {
        return bad("its answer was too long to read");
    }
    let clean = text::data(&answer.replace("\r\n", "\n")).replace('\t', " ");
    let Some(body) = clean
        .trim()
        .split_once('\n')
        .filter(|(fence, _)| fence.trim_end() == "```round")
        .and_then(|(_, b)| b.strip_suffix("```"))
    else {
        return bad("its answer was not the round block");
    };
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.iter().map(|l| l.trim()).eq(["nothing"]) {
        return Report::Nothing;
    }
    let mut out: Vec<Entry> = Vec::new();
    let mut it = lines.into_iter().peekable();
    while let Some(l) = it.next() {
        let Some(key) = l.trim_start().strip_prefix("- key:").map(str::trim) else {
            return bad("an entry of the round block does not start with - key:");
        };
        if !valid_key(key) {
            return bad("an entry of the round block has a key that is not a short name");
        }
        if out.iter().any(|e| e.key == key) {
            return bad("the round block names a key twice");
        }
        let Some(state) = it
            .next()
            .and_then(|l| l.trim().strip_prefix("state:"))
            .and_then(|s| State::from_word(s.trim()))
        else {
            return bad("an entry of the round block has no valid state:");
        };
        let Some(first) = it.next().and_then(|l| l.trim().strip_prefix("notice:")) else {
            return bad("an entry of the round block has no notice:");
        };
        let mut notice = first.trim().to_owned();
        while let Some(more) = it.next_if(|l| {
            let t = l.trim_start();
            l.starts_with(' ')
                && !["- key:", "state:", "notice:"]
                    .iter()
                    .any(|f| t.starts_with(f))
        }) {
            notice.push(' ');
            notice.push_str(more.trim());
        }
        if notice.is_empty() {
            return bad("an entry of the round block has an empty notice");
        }
        if out.len() == MAX_ENTRIES {
            return bad("the round block has too many entries");
        }
        out.push(Entry {
            key: key.to_owned(),
            state,
            notice: cap(&notice, MAX_NOTICE),
        });
    }
    if out.is_empty() {
        return bad("the round block is empty");
    }
    Report::Entries(out)
}

/// What the person was last told of one thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Given {
    pub state: State,
    pub notice: String,
    pub at_ms: i64,
}

/// Of `entries`, what the person is told now, as `(key, text)`; `told` is what they were told
/// before, by key, and is brought up to date. A `problem` is told when it is new or its state
/// changed, and again ("still: …") once `STILL_MS` passed since it was last told; a `resolved` is
/// told once, only of a key that was told before, and the key is forgotten; an `info` is told once
/// per distinct text.
pub fn to_tell(
    told: &mut BTreeMap<String, Given>,
    entries: &[Entry],
    now: i64,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for e in entries {
        let prev = told.get(&e.key);
        let text = match e.state {
            State::Problem => match prev {
                Some(g) if g.state == State::Problem && now - g.at_ms < STILL_MS => None,
                Some(g) if g.state == State::Problem => Some(format!("still: {}", e.notice)),
                _ => Some(e.notice.clone()),
            },
            State::Resolved => {
                let known = told.remove(&e.key).is_some();
                if known {
                    out.push((e.key.clone(), format!("resolved: {}", e.notice)));
                }
                continue;
            }
            State::Info => match prev {
                Some(g) if g.state == State::Info && g.notice == e.notice => None,
                _ => Some(e.notice.clone()),
            },
        };
        if let Some(t) = text {
            told.insert(
                e.key.clone(),
                Given {
                    state: e.state,
                    notice: e.notice.clone(),
                    at_ms: now,
                },
            );
            out.push((e.key.clone(), t));
        }
    }
    out
}

/// `HH:MM-HH:MM` as minutes of the day (start, end); `None` when it does not read.
pub fn quiet_hours(s: &str) -> Option<(u32, u32)> {
    let minutes = |t: &str| -> Option<u32> {
        let (h, m) = t.trim().split_once(':')?;
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        (h < 24 && m < 60 && t.trim().len() == 5).then_some(h * 60 + m)
    };
    let (a, b) = s.split_once('-')?;
    Some((minutes(a)?, minutes(b)?))
}

/// Whether local time `now` (epoch ms, `offset_s` east of UTC) falls in `quiet` (start
/// included, end not; a window that crosses midnight wraps; start = end is no window).
pub fn in_quiet(quiet: Option<(u32, u32)>, now: i64, offset_s: i64) -> bool {
    let Some((a, b)) = quiet else { return false };
    let m = u32::try_from((now.div_euclid(1000) + offset_s).rem_euclid(86_400) / 60).unwrap_or(0);
    match a.cmp(&b) {
        std::cmp::Ordering::Less => a <= m && m < b,
        std::cmp::Ordering::Greater => m >= a || m < b,
        std::cmp::Ordering::Equal => false,
    }
}

/// Whether a round is due at `now`: rounds on (`every_hours` > 0), not in the quiet hours, and
/// `every_hours` since the last one started (none yet: due). Measured from the last start, a
/// machine that was off catches up once, never once per missed slot; a round that falls in the
/// quiet hours waits for their end.
pub fn due(
    every_hours: u64,
    quiet: Option<(u32, u32)>,
    last_start: Option<i64>,
    now: i64,
    offset_s: i64,
) -> bool {
    if every_hours == 0 || in_quiet(quiet, now, offset_s) {
        return false;
    }
    let every = i64::try_from(every_hours.saturating_mul(3_600_000)).unwrap_or(i64::MAX);
    last_start.is_none_or(|l| now.saturating_sub(l) >= every)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_continues_on_indented_lines_and_stops_at_the_next_entry() {
        let r = parse(
            "```round\n- key: disk-root\n  state: problem\n  notice: The disk is at 86 %\n    (df -h /).\n- key: b\n  state: info\n  notice: x\n```",
        );
        let Report::Entries(e) = r else {
            panic!("{r:?}")
        };
        assert_eq!(e[0].notice, "The disk is at 86 % (df -h /).");
        assert_eq!(e[1].key, "b");
    }

    #[test]
    fn quiet_hours_read_only_as_hh_mm() {
        assert_eq!(quiet_hours("23:00-07:00"), Some((1380, 420)));
        for bad in [
            "",
            "23-07",
            "24:00-07:00",
            "7:00-08:00",
            "23:00",
            "a:bc-07:00",
        ] {
            assert_eq!(quiet_hours(bad), None, "{bad}");
        }
    }
}
