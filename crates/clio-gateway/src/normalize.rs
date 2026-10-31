use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// The action after normalizing: this is what gets hashed, approved and executed.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedAction {
    pub tool: String,
    pub version: String,
    pub node: String,
    /// Paths replaced by their canonical form.
    pub args: Map<String, Value>,
    pub domains: Vec<String>,
    pub paths: Vec<PathBuf>,
    pub argv: Option<Vec<String>>,
    pub secrets: Vec<String>,
}

impl NormalizedAction {
    pub fn hash(&self) -> String {
        let mut s = String::from("[");
        for part in [&self.tool, &self.version, &self.node] {
            s.push_str(&Value::from(part.as_str()).to_string());
            s.push(',');
        }
        canon(&Value::Object(self.args.clone()), &mut s);
        s.push_str(&format!(",{:?}]", self.domains));
        Sha256::digest(s.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Keys sorted, so the hash does not depend on the JSON map's ordering.
fn canon(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            out.push('{');
            for k in keys {
                out.push_str(&Value::from(k.as_str()).to_string());
                out.push(':');
                canon(&m[k], out);
                out.push(',');
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for x in a {
                canon(x, out);
                out.push(',');
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// S-13: resolve `raw` against `root` (which must already be canonical) and make sure the real
/// location, after following every symbolic link, is inside it. `..` is refused outright.
/// shortcut: checks at decision time; a link swapped before execution is the node's job (S-30).
pub fn confine(root: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.contains('\0') {
        return Err("empty path or NUL byte".into());
    }
    // A node or tool that expands `~` would land in the home directory, not under the root.
    if raw.starts_with('~') {
        return Err(format!("{raw:?} starts with `~`"));
    }
    let p = Path::new(raw);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    if full.components().any(|c| c == Component::ParentDir) {
        return Err(format!("`..` in {raw:?}"));
    }
    // Canonicalize the longest existing prefix; a new file is its canonical parent plus the name.
    let mut existing = full.clone();
    let mut tail = Vec::new();
    let base = loop {
        // A dangling link is not "a missing file": writing through it would land outside.
        if std::fs::symlink_metadata(&existing).is_ok_and(|m| m.is_symlink())
            && existing.canonicalize().is_err()
        {
            return Err(format!("dangling symbolic link in {raw:?}"));
        }
        match existing.canonicalize() {
            Ok(c) => break c,
            Err(_) => {
                let name = existing
                    .file_name()
                    .ok_or_else(|| format!("cannot resolve {raw:?}"))?
                    .to_owned();
                tail.push(name);
                existing.pop();
            }
        }
    };
    let canonical = tail.iter().rev().fold(base, |acc, n| acc.join(n));
    if canonical.starts_with(root) {
        Ok(canonical)
    } else {
        Err(format!("{raw:?} resolves outside the allowed root"))
    }
}

/// The host of an http(s) URL, lowercase. Anything ambiguous (user info, IPv6 brackets,
/// backslashes, other schemes) is refused: `https://ok.com@evil.com/` must not read as ok.com.
pub fn host_of(url: &str) -> Result<String, String> {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .ok_or("only http(s) URLs")?;
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    if auth.contains(['@', '\\', '[', ']', ' ']) || auth.chars().any(char::is_control) {
        return Err(format!("ambiguous authority in {url:?}"));
    }
    let host = match auth.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        Some(_) => return Err(format!("bad port in {url:?}")),
        None => auth,
    };
    let host = host.trim_end_matches('.');
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err(format!("bad host in {url:?}"));
    }
    Ok(host.to_string())
}

/// `host` is `domain` or a subdomain of it.
pub fn domain_matches(host: &str, domain: &str) -> bool {
    let d = domain.to_ascii_lowercase();
    host == d || host.ends_with(&format!(".{d}"))
}

/// Every `{{secreto:name}}` marker in the strings of `v`.
pub(crate) fn markers(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("{{secreto:") {
                let after = &rest[i + "{{secreto:".len()..];
                let end = after.find("}}").unwrap_or(after.len());
                out.push(after[..end].to_string());
                rest = &after[end..];
            }
        }
        Value::Array(a) => a.iter().for_each(|x| markers(x, out)),
        Value::Object(m) => m.iter().for_each(|(k, x)| {
            markers(&Value::from(k.as_str()), out);
            markers(x, out);
        }),
        _ => {}
    }
}
