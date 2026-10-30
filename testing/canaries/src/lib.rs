//! Decoy secrets: fake, unique values that must never show up in a trace, a log, a payload,
//! the memory or a prompt. If one does, the test (and the gate) fails.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

const NAMES: [&str; 4] = ["notion", "github", "llm-provider", "ssh"];

/// A decoy was found, and in which form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    pub name: String,
    pub form: Form,
    pub place: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Plain,
    Base64,
    UrlEncoded,
}

impl fmt::Display for Leak {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "canary {:?} leaked ({:?}) in {}",
            self.name, self.form, self.place
        )
    }
}

pub struct Canaries {
    secrets: Vec<(String, String)>,
}

impl Canaries {
    /// The same seed gives the same values, so a failure can be reproduced.
    pub fn from_seed(seed: u64) -> Self {
        let secrets = NAMES
            .iter()
            .map(|name| {
                let digest = Sha256::digest(format!("clio-canary:{seed}:{name}"));
                let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                (name.to_string(), format!("sk-canary-{}", &hex[..40]))
            })
            .collect();
        Self { secrets }
    }

    /// Unique per run: the seed comes from the clock and the process id.
    pub fn fresh() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self::from_seed((nanos as u64) ^ u64::from(std::process::id()))
    }

    /// The decoy for `name`, to plant where the real secret would be.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.secrets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.secrets.iter().map(|(n, _)| n.as_str())
    }

    /// Every decoy found in `text`, plain, in base64 (any alignment) or URL-encoded.
    pub fn scan(&self, place: &str, text: &str) -> Vec<Leak> {
        let mut found = Vec::new();
        for (name, value) in &self.secrets {
            let forms = [
                (Form::Plain, text.contains(value.as_str())),
                (
                    Form::Base64,
                    base64_variants(value)
                        .iter()
                        .any(|v| text.contains(v.as_str())),
                ),
                (
                    Form::UrlEncoded,
                    url_variants(value)
                        .iter()
                        .any(|v| v != value && text.contains(v.as_str())),
                ),
            ];
            for (form, hit) in forms {
                if hit {
                    found.push(Leak {
                        name: name.clone(),
                        form,
                        place: place.to_string(),
                    });
                }
            }
        }
        found
    }

    /// Panics with every leak found. This is what a test calls on a trace, a log or a payload.
    pub fn assert_clean(&self, place: &str, text: &str) {
        let leaks = self.scan(place, text);
        assert!(
            leaks.is_empty(),
            "{}",
            leaks
                .iter()
                .map(Leak::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    /// Scans every file under `dir` (a directory of logs, traces, exports). Unreadable files
    /// and non-text bytes are scanned lossily, never skipped.
    pub fn scan_dir(&self, dir: &Path) -> io::Result<Vec<Leak>> {
        let mut leaks = Vec::new();
        for file in files_under(dir)? {
            let text = String::from_utf8_lossy(&fs::read(&file)?).into_owned();
            leaks.extend(self.scan(&file.display().to_string(), &text));
        }
        Ok(leaks)
    }
}

fn files_under(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(files_under(&path)?);
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

/// The usual encoding (unreserved characters kept) and the one that escapes every byte.
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
        out.extend(std::iter::repeat_n('=', 3 - chunk.len()));
    }
    out
}

/// The value inside a larger base64 string can sit at three alignments. For each one, keep
/// only the characters that depend on the value alone.
/// shortcut: the last 3 characters are dropped, so a decoy shorter than ~12 bytes is not
/// covered; decoys here are 50 bytes.
fn base64_variants(value: &str) -> Vec<String> {
    let skip = [0, 2, 3];
    (0..3)
        .map(|pad| {
            let mut bytes = vec![b'x'; pad];
            bytes.extend_from_slice(value.as_bytes());
            let encoded = base64(&bytes);
            let trimmed = encoded.trim_end_matches('=');
            trimmed[skip[pad]..trimmed.len() - 3].to_string()
        })
        .collect()
}
