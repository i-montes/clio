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
        None => return Err("archivo vacío".into()),
        Some(None) => return Err("la cabecera no es JSON válido".into()),
        Some(Some(h)) => h,
    };
    if header["type"] != "session" {
        return Err("no empieza con una cabecera de sesión".into());
    }
    let id = header["id"]
        .as_str()
        .filter(|i| !i.is_empty())
        .ok_or("la cabecera no trae id")?;
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
            let t = text::line(&message_text(&v["message"]), TITLE_CHARS);
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
        .unwrap_or_else(|| "(sin mensajes)".to_owned());
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
                "se ignora {}: {}",
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
}
