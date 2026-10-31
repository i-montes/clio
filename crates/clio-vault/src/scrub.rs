//! The one output filter (S-20, S-44): known vault values (plain, base64 at any alignment, URL
//! encoded) and provider key patterns become `[secreto:name tachado]`. Every redaction is logged,
//! because a redaction usually means somebody tried to leak.

use clio_core::{Clock, SystemClock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Shorter values would shred ordinary text; the vault refuses to store them.
pub(crate) const MIN_VALUE_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// A variant of a secret that is not loaded yet (task 272): only its length and a digest keyed
/// with this process's random key, so the value cannot be read back from it.
struct Masked {
    name: String,
    form: Form,
    len: usize,
    digest: [u8; 32],
}

/// What the vault stores (sealed) for each secret so a lazy unlock never decrypts its value.
#[derive(Serialize, Deserialize)]
pub(crate) struct Print {
    form: Form,
    len: usize,
    digest: [u8; 32],
}

struct Shared {
    clock: Arc<dyn Clock>,
    pkey: [u8; 32],
    masked: Mutex<Vec<Masked>>,
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
            // shortcut: if the system has no randomness the key is zero; the masked digests are
            // then only unkeyed SHA-256 held in memory, which still never holds a value.
            pkey: crate::crypto::random().unwrap_or([0; 32]),
            masked: Mutex::new(Vec::new()),
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
        lock(&self.0.masked).retain(|m| m.name != name);
        let mut needles = lock(&self.0.needles);
        for (form, text) in variants(value) {
            // A repeated variant is already covered.
            if !needles.iter().any(|n| n.text == text) {
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

    /// Teach it a secret it has not loaded: only digests, no value (task 272).
    pub(crate) fn add_masked(&self, name: &str, prints: &[Print]) {
        let mut masked = lock(&self.0.masked);
        for p in prints {
            masked.push(Masked {
                name: name.to_string(),
                form: p.form,
                len: p.len,
                digest: self.keyed(&p.digest),
            });
        }
    }

    /// The secret was just loaded: from now on the scrubber knows its full value.
    pub(crate) fn promote(&self, name: &str, value: &str) {
        let had = {
            let mut m = lock(&self.0.masked);
            let n = m.len();
            m.retain(|x| x.name != name);
            m.len() != n
        };
        if had {
            self.add(name, value);
        }
    }

    fn keyed(&self, digest: &[u8; 32]) -> [u8; 32] {
        Sha256::new()
            .chain_update(self.0.pkey)
            .chain_update(digest)
            .finalize()
            .into()
    }

    /// Replaces every window of `text` whose keyed digest matches a masked variant.
    /// shortcut: one SHA-256 per byte offset and distinct length, so cost grows with output size
    /// times the number of distinct secret lengths; upgrade with a rolling hash if outputs get big.
    fn scrub_masked(&self, text: &str) -> (String, Vec<(String, Form)>) {
        let masked = lock(&self.0.masked);
        let mut lens: Vec<usize> = masked.iter().map(|m| m.len).collect();
        lens.sort_unstable();
        lens.dedup();
        let mut found: Vec<(usize, usize, String, Form)> = Vec::new();
        for len in lens {
            for i in (0..=text.len().saturating_sub(len)).filter(|&i| text.is_char_boundary(i)) {
                let Some(w) = text.get(i..i + len) else {
                    continue;
                };
                let d = self.keyed(&Sha256::digest(w.as_bytes()).into());
                if let Some(m) = masked.iter().find(|m| m.len == len && m.digest == d) {
                    found.push((i, i + len, m.name.clone(), m.form));
                }
            }
        }
        found.sort_by_key(|f| f.0);
        let (mut out, mut at, mut hits) = (String::new(), 0, Vec::new());
        for (start, end, name, form) in found {
            if start >= at {
                out.push_str(&text[at..start]);
                out.push_str(&marker(&name));
                at = end;
                hits.push((name, form));
            }
        }
        out.push_str(&text[at..]);
        (out, hits)
    }

    /// What `scrub` would redact in `text`, without redacting or logging (task 257: searching
    /// files for vault values).
    pub fn find(&self, text: &str) -> Vec<(String, Form)> {
        self.apply(text).1
    }

    pub fn scrub(&self, text: &str) -> String {
        let (out, hits) = self.apply(text);
        let at_ms = self.0.clock.now_ms();
        let mut log = lock(&self.0.log);
        for (name, form) in hits {
            self.0.count.fetch_add(1, Ordering::Relaxed);
            log.push(Redaction { at_ms, name, form });
        }
        out
    }

    fn apply(&self, text: &str) -> (String, Vec<(String, Form)>) {
        let mut out = text.to_string();
        let mut hits = Vec::new();
        for n in lock(&self.0.needles).iter() {
            if out.contains(&n.text) {
                out = out.replace(&n.text, &marker(&n.name));
                hits.push((n.name.clone(), n.form));
            }
        }
        let (out, masked_hits) = self.scrub_masked(&out);
        hits.extend(masked_hits);
        let (out, patterns) = redact_patterns(&out);
        hits.extend(std::iter::repeat_n(
            ("patrón".to_string(), Form::Pattern),
            patterns,
        ));
        (out, hits)
    }

    pub fn redactions(&self) -> Vec<Redaction> {
        lock(&self.0.log).clone()
    }

    pub fn redaction_count(&self) -> u64 {
        self.0.count.load(Ordering::Relaxed)
    }
}

/// Every form a value can take that the filter looks for.
fn variants(value: &str) -> Vec<(Form, String)> {
    let mut forms = vec![(Form::Plain, value.to_string())];
    forms.extend(url_variants(value).map(|v| (Form::UrlEncoded, v)));
    forms.extend(
        base64_variants(value)
            .into_iter()
            .map(|v| (Form::Base64, v)),
    );
    // A variant too short would match unrelated text.
    forms.retain(|(_, t)| t.len() >= MIN_VALUE_LEN);
    forms
}

pub(crate) fn prints(value: &str) -> Vec<Print> {
    variants(value)
        .into_iter()
        .map(|(form, t)| Print {
            form,
            len: t.len(),
            digest: Sha256::digest(t.as_bytes()).into(),
        })
        .collect()
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
