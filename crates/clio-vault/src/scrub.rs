//! The one output filter (S-20, S-44): known vault values (plain, base64 at any alignment, URL
//! encoded) and provider key patterns become `[secreto:name tachado]`. Every redaction is logged,
//! because a redaction usually means somebody tried to leak.

use clio_core::{Clock, SystemClock};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Shorter values would shred ordinary text; the vault refuses to store them.
pub(crate) const MIN_VALUE_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Plain,
    Base64,
    UrlEncoded,
    /// A provider key pattern, not a value the vault knows.
    Pattern,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redaction {
    pub at_ms: i64,
    /// The secret name, or `patrón` for a provider pattern.
    pub name: String,
    pub form: Form,
}

struct Needle {
    name: String,
    form: Form,
    text: String,
}

struct Shared {
    clock: Arc<dyn Clock>,
    needles: Mutex<Vec<Needle>>,
    log: Mutex<Vec<Redaction>>,
    count: AtomicU64,
}

/// Cheap to clone; every clone shares the same known values and the same log.
#[derive(Clone)]
pub struct Scrubber(Arc<Shared>);

// No values in Debug (S-18).
impl fmt::Debug for Scrubber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Scrubber {{ redactions: {} }}", self.redaction_count())
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Default for Scrubber {
    fn default() -> Self {
        Self::new(Arc::new(SystemClock))
    }
}

impl Scrubber {
    /// A scrubber that knows no values yet: it only catches provider patterns until `add`.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self(Arc::new(Shared {
            clock,
            needles: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
            count: AtomicU64::new(0),
        }))
    }

    /// Teach it a value. Old values stay known on purpose: a rotated or revoked key that leaks
    /// is still a leak.
    pub fn add(&self, name: &str, value: &str) {
        if value.len() < MIN_VALUE_LEN {
            return;
        }
        let mut forms = vec![(Form::Plain, value.to_string())];
        forms.extend(url_variants(value).map(|v| (Form::UrlEncoded, v)));
        forms.extend(
            base64_variants(value)
                .into_iter()
                .map(|v| (Form::Base64, v)),
        );
        let mut needles = lock(&self.0.needles);
        for (form, text) in forms {
            // A variant too short would match unrelated text; a repeated one is already covered.
            let dup = needles.iter().any(|n| n.text == text);
            if text.len() >= MIN_VALUE_LEN && !dup {
                needles.push(Needle {
                    name: name.to_string(),
                    form,
                    text,
                });
            }
        }
        // Longest first, so a value that contains another one is replaced whole.
        needles.sort_by_key(|n| std::cmp::Reverse(n.text.len()));
    }

    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        let mut hits = Vec::new();
        for n in lock(&self.0.needles).iter() {
            if out.contains(&n.text) {
                out = out.replace(&n.text, &marker(&n.name));
                hits.push((n.name.clone(), n.form));
            }
        }
        let (out, patterns) = redact_patterns(&out);
        hits.extend(std::iter::repeat_n(
            ("patrón".to_string(), Form::Pattern),
            patterns,
        ));
        let at_ms = self.0.clock.now_ms();
        let mut log = lock(&self.0.log);
        for (name, form) in hits {
            self.0.count.fetch_add(1, Ordering::Relaxed);
            log.push(Redaction { at_ms, name, form });
        }
        out
    }

    pub fn redactions(&self) -> Vec<Redaction> {
        lock(&self.0.log).clone()
    }

    pub fn redaction_count(&self) -> u64 {
        self.0.count.load(Ordering::Relaxed)
    }
}

fn marker(name: &str) -> String {
    format!("[secreto:{name} tachado]")
}

/// shortcut: only the usual and the all-bytes encodings; a mix (some bytes escaped, some not) slips
/// through, upgrade by percent-decoding the text before matching.
fn url_variants(s: &str) -> [String; 2] {
    let keep =
        |b: u8| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~');
    let usual = s
        .bytes()
        .map(|b| {
            if keep(b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    [usual, s.bytes().map(|b| format!("%{b:02X}")).collect()]
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// A value inside a larger base64 string sits at one of three alignments; for each, keep only
/// the characters that depend on the value alone. Standard and URL-safe alphabets.
/// shortcut: the 3 trailing characters are dropped, so values under ~12 bytes get short
/// variants (kept only if >= MIN_VALUE_LEN); upgrade if the vault must hold short PINs.
fn base64_variants(value: &str) -> Vec<String> {
    let skip = [0, 2, 3];
    let std: Vec<String> = (0..3)
        .filter_map(|pad| {
            let mut bytes = vec![b'x'; pad];
            bytes.extend_from_slice(value.as_bytes());
            let enc = base64(&bytes);
            enc.get(skip[pad]..enc.len().checked_sub(3)?)
                .map(String::from)
        })
        .collect();
    let url: Vec<String> = std
        .iter()
        .map(|s| s.replace('+', "-").replace('/', "_"))
        .filter(|u| !std.contains(u))
        .collect();
    [std, url].concat()
}

/// Known provider key shapes: (prefix, minimum token length).
const PREFIXES: [(&str, usize); 11] = [
    ("sk-", 20),
    ("ghp_", 30),
    ("gho_", 30),
    ("ghs_", 30),
    ("github_pat_", 30),
    ("xoxb-", 20),
    ("xoxp-", 20),
    ("AKIA", 20),
    ("AIza", 35),
    ("ntn_", 20),
    ("secret_", 30),
];

fn redact_patterns(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut hits = 0;
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(block) = private_key_block(rest) {
            out.push_str("[secreto:patrón tachado]");
            hits += 1;
            rest = &rest[block..];
            continue;
        }
        let is_tok = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        let end = rest.find(|c| !is_tok(c)).unwrap_or(rest.len());
        if end == 0 {
            let c = rest.chars().next().map_or(1, char::len_utf8);
            out.push_str(&rest[..c]);
            rest = &rest[c..];
            continue;
        }
        let token = &rest[..end];
        if PREFIXES
            .iter()
            .any(|(p, min)| token.starts_with(p) && token.len() >= *min)
        {
            out.push_str("[secreto:patrón tachado]");
            hits += 1;
        } else {
            out.push_str(token);
        }
        rest = &rest[end..];
    }
    (out, hits)
}

/// Length of a PEM private key block starting exactly at the start of `s`, if there is one.
fn private_key_block(s: &str) -> Option<usize> {
    if !s.starts_with("-----BEGIN ") {
        return None;
    }
    let header_end = s.find("-----\n").or_else(|| s.find("-----\r"))?;
    if !s[..header_end].contains("PRIVATE KEY") {
        return None;
    }
    Some(match s.find("-----END ") {
        Some(i) => s[i + 9..].find("-----").map_or(s.len(), |j| i + 9 + j + 5),
        None => s.len(), // unterminated: everything after the header goes
    })
}
