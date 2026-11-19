//! Listing Pi's sessions. Pi owns them (ADR 0007): Clio only reads each `.jsonl` file's header and
//! the least it needs, never the whole conversation of a big file, and trusts nothing in it:
//! names and titles are sanitized, unreadable files are skipped with a warning.
//!
//! Opening, creating, forking and renaming go through Pi itself: `SessionTarget` in
//! [`super::launch`] (`--session`, `--session-id`, `--fork`, `--name`) and the RPC commands
//! `fork`, `new_session`, `switch_session`, `set_session_name`.

use std::fs::File;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::text;

/// A file up to this size is read whole (exact message count); a bigger one is sampled.
const SMALL_FILE: u64 = 512 * 1024;
/// Longest single record parsed (a system message with all the tools can be large).
const MAX_RECORD: u64 = 512 * 1024;
const HEAD_RECORDS: usize = 200;
const TAIL_BYTES: u64 = 64 * 1024;
pub const TITLE_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub path: PathBuf,
    /// The working folder from the header, sanitized for display.
    pub cwd: String,
    /// The same folder as a path, only when it is absolute and has no control characters.
    pub cwd_path: Option<PathBuf>,
    pub created_ms: Option<i64>,
    /// The file's modification time.
    pub last_activity_ms: i64,
    /// The name set with `--name` or `/name`, sanitized.
    pub name: Option<String>,
    /// The name, or the first thing the person said, trimmed; never empty.
    pub title: String,
    /// Messages other than the system prompt.
    pub messages: u32,
    /// True when `messages` is an estimate (big file).
    pub approx: bool,
    /// The session this one was forked from, sanitized.
    pub parent: Option<String>,
}

#[derive(Debug, Default)]
pub struct SessionList {
    /// Most recent activity first.
    pub sessions: Vec<SessionInfo>,
    /// One line per file that was skipped (sanitized).
    pub warnings: Vec<String>,
}

/// `2024-12-03T14:00:00.000Z` to epoch milliseconds.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let ms = if b.get(19) == Some(&b'.') {
        let frac: String = s[20..]
            .chars()
            .take_while(char::is_ascii_digit)
            .take(3)
            .collect();
        format!("{frac:0<3}").parse::<i64>().ok()?
    } else {
        0
    };
    // days from civil
    let (yy, mm) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = yy.div_euclid(400);
    let yoe = yy.rem_euclid(400);
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + se * 1000 + ms)
}

fn message_text(m: &Value) -> String {
    match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Reads one record (up to the LF, capped). `None` at EOF; `Some(None)` when it was too long.
fn read_record(r: &mut impl BufRead, buf: &mut Vec<u8>) -> Option<Option<Value>> {
    buf.clear();
    let n = (&mut *r).take(MAX_RECORD + 1).read_until(b'\n', buf).ok()?;
    if n == 0 {
        return None;
    }
    if buf.len() as u64 > MAX_RECORD {
        // Skip the rest of this huge line.
        let mut sink = Vec::new();
        let _ = r.read_until(b'\n', &mut sink);
        return Some(None);
    }
    Some(serde_json::from_slice(buf).ok())
}

fn is_message(line: &[u8]) -> bool {
    contains(line, br#""type":"message""#) && !contains(line, br#""role":"system""#)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Reads the summary of one session file, or says why it cannot.
pub fn read_session(path: &Path) -> Result<SessionInfo, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let meta = f.metadata().map_err(|e| e.to_string())?;
    let len = meta.len();
    let mut r = std::io::BufReader::new(&mut f);
    let mut buf = Vec::new();
    let header = match read_record(&mut r, &mut buf) {
        None => return Err("empty file".into()),
        Some(None) => return Err("the header is not valid JSON".into()),
        Some(Some(h)) => h,
    };
    if header["type"] != "session" {
        return Err("it does not start with a session header".into());
    }
    let id = header["id"]
        .as_str()
        .filter(|i| !i.is_empty())
        .ok_or("the header has no id")?;
    let cwd_raw = header["cwd"].as_str().unwrap_or("");
    let cwd_path = (cwd_raw.starts_with('/') && !cwd_raw.chars().any(char::is_control))
        .then(|| PathBuf::from(cwd_raw));

    let mut name: Option<String> = None;
    let mut first_user: Option<String> = None;
    let mut count: u64 = 0;
    let mut approx = false;
    let mut scan = |v: &Value| match v["type"].as_str() {
        Some("session_info") => {
            name = v["name"]
                .as_str()
                .map(|n| text::line(n, TITLE_CHARS))
                .filter(|n| !n.is_empty())
        }
        Some("message") if first_user.is_none() && v["message"]["role"] == "user" => {
            let typed = message_text(&v["message"]);
            let t = text::line(super::envelope::strip(&typed), TITLE_CHARS);
            if !t.is_empty() {
                first_user = Some(t);
            }
        }
        _ => {}
    };
    if len <= SMALL_FILE {
        // Whole file: exact count, latest name wins.
        loop {
            buf.clear();
            let Ok(n) = (&mut r).take(MAX_RECORD + 1).read_until(b'\n', &mut buf) else {
                break;
            };
            if n == 0 {
                break;
            }
            if is_message(&buf) {
                count += 1;
            }
            if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                scan(&v);
            }
        }
    } else {
        approx = true;
        let mut sampled: u64 = 0;
        for _ in 0..HEAD_RECORDS {
            buf.clear();
            let Ok(n) = (&mut r).take(MAX_RECORD + 1).read_until(b'\n', &mut buf) else {
                break;
            };
            if n == 0 {
                break;
            }
            sampled += n as u64;
            if is_message(&buf) {
                count += 1;
            }
            if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                scan(&v);
            }
        }
        count = (count * len).checked_div(sampled).unwrap_or(count);
        // The tail has the latest name, if it was changed late.
        drop(r);
        if f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
            .is_ok()
        {
            let mut tail = Vec::new();
            let _ = (&mut f).take(TAIL_BYTES).read_to_end(&mut tail);
            for line in tail.split(|b| *b == b'\n').skip(1) {
                if contains(line, br#""type":"session_info""#)
                    && let Ok(v) = serde_json::from_slice::<Value>(line)
                {
                    scan(&v);
                }
            }
        }
    }
    let last_activity_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
    let title = name
        .clone()
        .or(first_user)
        .unwrap_or_else(|| "(no messages)".to_owned());
    Ok(SessionInfo {
        id: text::line(id, 64),
        path: path.to_path_buf(),
        cwd: text::line(cwd_raw, 200),
        cwd_path,
        created_ms: header["timestamp"].as_str().and_then(parse_iso_ms),
        last_activity_ms,
        name,
        title,
        messages: u32::try_from(count).unwrap_or(u32::MAX),
        approx,
        parent: header["parentSession"].as_str().map(|p| text::line(p, 200)),
    })
}

/// Lists the sessions under `dir`: the `.jsonl` files in it and in its direct subfolders (Pi
/// groups by folder when no session dir is forced). Never fails: problems become warnings.
pub fn list_sessions(dir: &Path) -> SessionList {
    let mut out = SessionList::default();
    let mut files: Vec<PathBuf> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
            files.push(p);
        } else if t.is_dir()
            && let Ok(sub) = std::fs::read_dir(&p)
        {
            files.extend(
                sub.filter_map(Result::ok)
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "jsonl")),
            );
        }
    }
    for p in files {
        match read_session(&p) {
            Ok(s) => out.sessions.push(s),
            Err(why) => out.warnings.push(format!(
                "ignored {}: {}",
                text::line(
                    &p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    80
                ),
                why
            )),
        }
    }
    out.sessions.sort_by(|a, b| {
        b.last_activity_ms
            .cmp(&a.last_activity_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    out.warnings.sort();
    out
}

/// The turns (a message of the person and what followed it) the chat reads from a session file at
/// a time.
pub const HISTORY_TURNS: usize = 200;
/// A session file bigger than this is not read for the history (the chat shows Pi's own copy).
const MAX_HISTORY_FILE: u64 = 128 * 1024 * 1024;
/// The most entries one page holds, whatever its turns: a file with few or no messages of the
/// person does not become one page of everything.
const MAX_PAGE_ENTRIES: usize = 5_000;

/// A page of a session's conversation, as Pi's file recorded it.
#[derive(Debug, Default, PartialEq)]
pub struct History {
    /// The `message` (not `system`) and `compaction` entries of the page, oldest first, whole.
    pub entries: Vec<Value>,
    /// The line of the file the page starts at: the page before it ends there.
    pub first: usize,
    /// The file has messages before the page.
    pub more: bool,
}

/// What a line is, read without the rest of it.
#[derive(serde::Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: Option<Role>,
}

#[derive(serde::Deserialize)]
struct Role {
    #[serde(default)]
    role: String,
}

/// The last `turns` turns of the session file at `path` that end before line `before` (`None`: the
/// whole file), at most `MAX_PAGE_ENTRIES` entries. Clio only reads Pi's format, never writes it. A
/// line that is not JSON (the last one while Pi writes it) or is over `MAX_RECORD` is skipped; a
/// file that cannot be read, or is over `MAX_HISTORY_FILE`, is an error.
pub fn read_history(path: &Path, before: Option<usize>, turns: usize) -> Result<History, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(MAX_HISTORY_FILE + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_HISTORY_FILE {
        return Err("it is over 128 MiB".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.split('\n').collect();
    let end = before.unwrap_or(lines.len()).min(lines.len());
    // The lines the chat draws, and whether each is a message of the person.
    let kinds: Vec<(usize, bool)> = lines[..end]
        .iter()
        .enumerate()
        .filter(|(_, l)| l.len() as u64 <= MAX_RECORD)
        .filter_map(|(i, l)| {
            let k: Kind = serde_json::from_str(l).ok()?;
            let role = k.message.map(|m| m.role).unwrap_or_default();
            match k.kind.as_str() {
                "message" if role != "system" => Some((i, role == "user")),
                "compaction" => Some((i, false)),
                _ => None,
            }
        })
        .collect();
    let mut start = 0;
    let mut seen = 0;
    for (k, (_, user)) in kinds.iter().enumerate().rev() {
        seen += usize::from(*user);
        if seen == turns {
            start = k;
            break;
        }
    }
    let start = start.max(kinds.len().saturating_sub(MAX_PAGE_ENTRIES));
    let page = &kinds[start..];
    Ok(History {
        entries: page
            .iter()
            .filter_map(|(i, _)| serde_json::from_str(lines[*i]).ok())
            .collect(),
        first: page.first().map_or(end, |(i, _)| *i),
        more: start > 0,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates_parse() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso_ms("2024-12-03T14:00:00.500Z"),
            Some(1_733_234_400_500)
        );
        assert_eq!(parse_iso_ms("nonsense"), None);
        assert_eq!(parse_iso_ms("2024-13-03T14:00:00Z"), None);
    }

    // The chat's history: pages of whole turns from the end, each ending where the later one
    // starts; system messages and other entries are left out, a compaction kept where it sits, and
    // a torn last line (Pi writing it) skipped.
    #[test]
    fn the_history_comes_in_pages_of_whole_turns_from_the_end() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s.jsonl");
        let msg = |role: &str, text: &str| {
            serde_json::json!({"type":"message","message":{"role":role,"content":text}}).to_string()
        };
        let mut lines = vec![
            r#"{"type":"session","version":3,"id":"x","cwd":"/"}"#.to_owned(),
            msg("system", "the prompt"),
        ];
        for n in 0..5 {
            lines.push(msg("user", &format!("q{n}")));
            lines.push(msg("assistant", &format!("a{n}")));
            if n == 2 {
                lines.push(r#"{"type":"compaction","summary":"s","firstKeptEntryId":"e"}"#.into());
            }
            lines.push(r#"{"type":"custom","customType":"clio-guard.taint"}"#.into());
        }
        let mut file = lines.join("\n");
        file.push_str("\n{\"type\":\"message\",\"mess");
        std::fs::write(&p, file).unwrap();
        let said = |h: &History| -> Vec<String> {
            h.entries
                .iter()
                .map(|e| e["message"]["content"].as_str().unwrap_or("·").to_owned())
                .collect()
        };
        let last = read_history(&p, None, 2).unwrap();
        assert_eq!(said(&last), ["q3", "a3", "q4", "a4"]);
        assert!(last.more);
        let before = read_history(&p, Some(last.first), 2).unwrap();
        assert_eq!(said(&before), ["q1", "a1", "q2", "a2", "·"]);
        assert_eq!(before.entries[4]["type"], "compaction");
        let first = read_history(&p, Some(before.first), 2).unwrap();
        assert_eq!(said(&first), ["q0", "a0"]);
        assert!(!first.more);
        assert!(read_history(&t.path().join("gone.jsonl"), None, 2).is_err());
    }

    // The file is Pi's to write: a huge one is not read (the chat falls back to Pi's copy), a line
    // over the record cap is skipped, and a file with no message of the person comes in pages of
    // at most `MAX_PAGE_ENTRIES`, not one page of everything.
    #[test]
    fn the_history_reader_has_limits() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s.jsonl");
        let huge = File::create(&p).unwrap();
        huge.set_len(MAX_HISTORY_FILE + 1).unwrap();
        assert!(read_history(&p, None, 2).unwrap_err().contains("128 MiB"));

        let msg = |role: &str, text: &str| {
            serde_json::json!({"type":"message","message":{"role":role,"content":text}}).to_string()
        };
        let long = "x".repeat(usize::try_from(MAX_RECORD).unwrap());
        std::fs::write(
            &p,
            [
                msg("user", "q"),
                msg("assistant", &long),
                msg("assistant", "a"),
            ]
            .join("\n"),
        )
        .unwrap();
        let h = read_history(&p, None, 2).unwrap();
        let said: Vec<&str> = h
            .entries
            .iter()
            .map(|e| e["message"]["content"].as_str().unwrap())
            .collect();
        assert_eq!(said, ["q", "a"], "the long line is skipped");

        let lines: Vec<String> = (0..MAX_PAGE_ENTRIES + 10)
            .map(|n| msg("assistant", &n.to_string()))
            .collect();
        std::fs::write(&p, lines.join("\n")).unwrap();
        let last = read_history(&p, None, HISTORY_TURNS).unwrap();
        assert_eq!(last.entries.len(), MAX_PAGE_ENTRIES);
        assert!(last.more && last.first == 10);
        let rest = read_history(&p, Some(last.first), HISTORY_TURNS).unwrap();
        assert_eq!(rest.entries.len(), 10);
        assert!(!rest.more);
    }
}
