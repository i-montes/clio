//! Skills (S-68): Pi's Agent Skills, in one folder of Clio's (`<data>/skills`). What the tool host
//! does with them, and nothing more:
//!
//! - `skill_write`: the model's only way to create or change a skill. It checks the draft (name,
//!   sizes, paths, characters, secrets), has the judge read all of it, and stores it as a draft in
//!   the run folder. It registers nothing: the person's `clio` reviews the draft and registers it
//!   (or sets it aside for the person), and only that process writes the skills folder.
//! - `skill_report`: the model says following a skill worked or failed; it is kept in the run
//!   folder for the person's `clio`, which counts it against the skill.
//! - Reading a registered, enabled skill (`live/<name>/...`) with `read` or `ls`: served from that
//!   folder as Clio's own instructions, not outside text, so it does not mark the session. Every
//!   other path, a skill that is off or gone included, is what it always was.
//!
//! Layout of the skills folder (written only by the person's `clio`): `live/<name>/` is what Pi
//! loads (the enabled skills, each a copy of its current version), `versions/<name>/<n>/` every
//! version registered, `records/<name>.json` each skill's record.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use clio_vault::count_masks;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::host::{now_ms, private_dir, real};
use crate::judge::{ToJudge, one_line};
use crate::{DecisionOut, Request, Response, TextVerdict, ToolHost};

/// The enabled skills' folder inside the skills folder, as it was up to 0.9.0: `live` moves it.
pub const LIVE: &str = "live";

/// Where the enabled skills are, for the skills folder `root` (`<data>/skills`): beside Clio's
/// data (`<data>-skills`), not in it. A path in Clio's data is `control` wherever a command names
/// it, so a skill's own folder had to be told apart from the rest word by word, and ordinary
/// steps of a skill (a glob, `$(pwd)`, a pipe into its script) waited for the person. Out of it,
/// a skill's folder is a folder like any other, read-only in every jail (S-68).
pub fn live(root: &Path) -> PathBuf {
    match root.parent().and_then(|d| Some((d, d.file_name()?))) {
        Some((data, name)) => {
            let mut n = name.to_owned();
            n.push("-skills");
            data.with_file_name(n)
        }
        None => root.join(LIVE),
    }
}
pub const RECORDS: &str = "records";
pub const VERSIONS: &str = "versions";
/// The section every skill of Clio's ends with; `skill_write` adds it from `verify`.
pub const VERIFY_HEADING: &str = "## How to verify";
/// SKILL.md as written (frontmatter, body and the check), at most.
pub const MAX_SKILL_MD: usize = 20_000;
pub const MAX_FILE: usize = 32 * 1024;
pub const MAX_FILES: usize = 10;
/// SKILL.md and every file together.
pub const MAX_TOTAL: usize = 128 * 1024;
pub const MAX_VERIFY: usize = 4000;
/// The description says what it does and when to use it: bounds only (Pi's own maximum).
pub const MIN_DESCRIPTION: usize = 20;
pub const MAX_DESCRIPTION: usize = 1024;
const MAX_PATH: usize = 200;
const MAX_DEPTH: usize = 4;
const MAX_REASON: usize = 300;
const MAX_NOTE: usize = 500;
/// Drafts a session may store in an hour: each one costs a review.
pub const DRAFTS_PER_HOUR: usize = 6;
const HOUR_MS: i64 = 3_600_000;

/// One of a skill's files besides SKILL.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFile {
    pub path: String,
    pub content: String,
}

/// A draft, as `skill_write` stored it: judged clean, not reviewed, not registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    pub session: String,
    pub name: String,
    pub description: String,
    pub reason: String,
    pub verify: String,
    pub skill_md: String,
    pub files: Vec<SkillFile>,
    pub created_ms: i64,
    /// A skill of that name was registered already: this is a new version of it.
    pub update: bool,
    /// `digest(skill_md, files)`: what is reviewed, shown and registered is exactly this.
    pub sha256: String,
    /// The session's marks when it was written (outside content read; flagged by the judge):
    /// its registration counts them even if the mark file is lost.
    #[serde(default)]
    pub marked: bool,
    #[serde(default)]
    pub suspect: bool,
}

/// Pi's rules for a name: `a-z`, `0-9` and `-`, at most 64, no hyphen first, last or twice.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !n.starts_with('-')
        && !n.ends_with('-')
        && !n.contains("--")
}

/// A character the display would change (the Approval box, `clio approve`): a control character
/// other than the line end, or one of the tool host's deceptive ones. A tab is refused too: it is
/// drawn as spaces.
pub fn hidden(c: char) -> bool {
    c != '\n' && (c.is_control() || crate::is_deceptive(c))
}

/// SKILL.md as Pi reads it: the frontmatter (the description as a JSON string, which YAML reads
/// as it is), the body, and the check under `## How to verify`.
pub fn compose(name: &str, description: &str, body: &str, verify: &str) -> String {
    let d = serde_json::to_string(description).unwrap_or_default();
    format!(
        "---\nname: {name}\ndescription: {d}\n---\n\n{}\n\n{VERIFY_HEADING}\n\n{}\n",
        body.trim(),
        verify.trim()
    )
}

/// Over SKILL.md and every file, path and content, each length-prefixed: one value for exactly
/// these bytes.
pub fn digest(skill_md: &str, files: &[SkillFile]) -> String {
    digest_all(skill_md, files, &[])
}

/// `digest`, then every resource (its path and its bytes' SHA-256) past a separator no path can
/// spell: one value for a whole added skill. Without resources it is `digest`, so a skill
/// registered before resources existed keeps its digest.
pub fn digest_all(skill_md: &str, files: &[SkillFile], resources: &[Resource]) -> String {
    let mut h = Sha256::new();
    let mut part = |s: &str| {
        h.update((s.len() as u64).to_le_bytes());
        h.update(s.as_bytes());
    };
    for (path, content) in std::iter::once(("SKILL.md", skill_md))
        .chain(files.iter().map(|f| (f.path.as_str(), f.content.as_str())))
    {
        part(path);
        part(content);
    }
    if !resources.is_empty() {
        part("\0resources");
        for r in resources {
            part(&r.path);
            part(&r.sha256);
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of `b`, in hex.
pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// A file's path inside the skill: relative, plain names (`A-Z a-z 0-9 . _ -`, none starting with
/// a dot, so no `..`), at most four deep, never SKILL.md itself.
pub fn valid_path(p: &str) -> Result<(), String> {
    path_within(p, MAX_DEPTH)
}

/// `valid_path` for a skill the person adds: at most `MAX_ADDED_DEPTH` deep.
pub fn valid_added_path(p: &str) -> Result<(), String> {
    path_within(p, MAX_ADDED_DEPTH)
}

fn path_within(p: &str, depth: usize) -> Result<(), String> {
    let bad = |why: &str| Err(format!("file path {:?}: {why}", one_line(p)));
    if p.is_empty() || p.len() > MAX_PATH {
        return bad("empty or too long");
    }
    let parts: Vec<&str> = p.split('/').collect();
    if parts.len() > depth {
        return bad(&format!("more than {depth} folders deep"));
    }
    for part in &parts {
        if part.is_empty()
            || part.starts_with('.')
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return bad(
                "each part must be a plain name (letters, digits, . _ -), relative, no '..'",
            );
        }
    }
    if p.eq_ignore_ascii_case("SKILL.md") {
        return bad("SKILL.md is written from name, description, body and verify");
    }
    Ok(())
}

/// The shape of a draft: everything that does not need the vault or the judge. `Ok` with SKILL.md.
pub fn check(
    name: &str,
    description: &str,
    body: &str,
    verify: &str,
    files: &[SkillFile],
) -> Result<String, String> {
    if !valid_name(name) {
        return Err(
            "name: lowercase letters, digits and single hyphens, at most 64 (like pdf-tools)"
                .into(),
        );
    }
    let dl = description.chars().count();
    if !(MIN_DESCRIPTION..=MAX_DESCRIPTION).contains(&dl) || description.contains('\n') {
        return Err(format!(
            "description: one line of {MIN_DESCRIPTION} to {MAX_DESCRIPTION} characters that says what it does and when to use it"
        ));
    }
    if body.trim().is_empty() {
        return Err("body: the instructions are empty".into());
    }
    if verify.trim().is_empty() || verify.len() > MAX_VERIFY {
        return Err(format!(
            "verify: a check that can be run, at most {MAX_VERIFY} bytes (it is required)"
        ));
    }
    if no_check(verify) {
        return Err(
            "verify: that succeeds whatever the skill does, so it is no check; give commands that fail when the skill does not do what it says (run its script on a sample and compare the output)"
                .into(),
        );
    }
    if body
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case(VERIFY_HEADING))
    {
        return Err(format!(
            "body: leave out `{VERIFY_HEADING}`; Clio writes it from `verify`"
        ));
    }
    if files.len() > MAX_FILES {
        return Err(format!("files: at most {MAX_FILES}"));
    }
    let mut seen = std::collections::HashSet::new();
    for f in files {
        valid_path(&f.path)?;
        if !seen.insert(f.path.as_str()) {
            return Err(format!("file path {:?} is given twice", one_line(&f.path)));
        }
        if f.content.len() > MAX_FILE {
            return Err(format!(
                "file {:?}: at most {MAX_FILE} bytes",
                one_line(&f.path)
            ));
        }
    }
    let skill_md = compose(name, description, body, verify);
    if skill_md.len() > MAX_SKILL_MD {
        return Err(format!(
            "SKILL.md would be {} bytes: at most {MAX_SKILL_MD} (put long references in files)",
            skill_md.len()
        ));
    }
    let total = skill_md.len() + files.iter().map(|f| f.content.len()).sum::<usize>();
    if total > MAX_TOTAL {
        return Err(format!(
            "the skill would be {total} bytes: at most {MAX_TOTAL}"
        ));
    }
    for (what, t) in std::iter::once(("SKILL.md", skill_md.as_str()))
        .chain(files.iter().map(|f| (f.path.as_str(), f.content.as_str())))
    {
        if t.chars().any(hidden) {
            return Err(format!(
                "{}: it has a control, invisible or tab character the person would not see as written; use plain text and spaces",
                one_line(what)
            ));
        }
    }
    Ok(skill_md)
}

/// A `verify` that succeeds whatever the skill does: only `true`, `:`, `exit 0`, or one `echo`
/// (no pipe, list or second line).
pub fn no_check(verify: &str) -> bool {
    let v = verify.trim();
    matches!(v, "true" | ":" | "exit 0" | "echo")
        || (v.starts_with("echo ") && !v.contains(['|', ';', '&', '\n']))
}

/// A skill as it is stored or registered (SKILL.md already written): the same limits as a draft
/// (name, paths, sizes, characters), checked again by whoever writes it to the skills folder.
pub fn check_stored(name: &str, skill_md: &str, files: &[SkillFile]) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("{:?} is not a skill name", one_line(name)));
    }
    if !skill_md.starts_with(&format!("---\nname: {name}\ndescription: ")) {
        return Err("SKILL.md does not start with its name and description".into());
    }
    if skill_md.len() > MAX_SKILL_MD || files.len() > MAX_FILES {
        return Err("too large".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut total = skill_md.len();
    for f in files {
        valid_path(&f.path)?;
        if !seen.insert(f.path.as_str()) || f.content.len() > MAX_FILE {
            return Err(format!(
                "file {:?}: given twice or too large",
                one_line(&f.path)
            ));
        }
        total += f.content.len();
    }
    if total > MAX_TOTAL {
        return Err("too large".into());
    }
    if std::iter::once(skill_md)
        .chain(files.iter().map(|f| f.content.as_str()))
        .any(|t| t.chars().any(hidden))
    {
        return Err("it has characters the person would not see as written".into());
    }
    Ok(())
}

/// The instructions of a skill the person adds from outside (`skill_add`, `clio skills add`),
/// SKILL.md included: at most this many files and bytes (what the judge reads and the box shows).
/// `skill_write`'s limits are the ones above.
pub const MAX_ADDED_FILES: usize = 100;
pub const MAX_ADDED_BYTES: usize = 1 << 20;
/// A whole added skill, instructions and resources: at most this many files and bytes. About
/// 2.5 times the files and 1.8 times the bytes of the largest real skill measured (taste-skill:
/// 1 631 files, 34.9 MiB), and held once in memory while it is read.
pub const MAX_SKILL_FILES: usize = 4000;
pub const MAX_SKILL_BYTES: usize = 64 << 20;
/// Folders deep, at most, in an added skill (taste-skill goes 5 deep).
pub const MAX_ADDED_DEPTH: usize = 8;

/// A file of an added skill that is not an instruction (S-68): stored byte for byte, never read
/// as Clio's own text. Its bytes travel apart (staged); this is what is shown and digested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub path: String,
    pub size: u64,
    /// Of its bytes, hex.
    pub sha256: String,
    /// It starts with `#!` or its name is a script's: the box names it.
    #[serde(default)]
    pub script: bool,
}

/// A name an instruction may have: a document of markdown or plain text. Anything else of an
/// added skill is a resource, whatever it holds or says.
pub fn instruction_kind(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    path.eq_ignore_ascii_case("SKILL.md")
        || [".md", ".markdown", ".txt"].iter().any(|e| p.ends_with(e))
}

/// A script's name, by its extension.
pub fn script_name(path: &str) -> bool {
    [
        ".sh", ".bash", ".py", ".js", ".mjs", ".cjs", ".ts", ".rb", ".pl", ".ps1",
    ]
    .iter()
    .any(|e| path.to_ascii_lowercase().ends_with(e))
}

/// `n` bytes for a person: B, KiB or MiB.
pub fn size_text(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1 << 20 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", n as f64 / f64::from(1 << 20))
    }
}

/// What a limit refuses, in fixed words: what was found, the limit and by how much it is over.
fn over(what: &str, found: u64, limit: u64, bytes: bool) -> String {
    let n = |v: u64| {
        if bytes { size_text(v) } else { v.to_string() }
    };
    format!(
        "{} {what}: past the limit of {} by {}",
        n(found),
        n(limit),
        n(found - limit)
    )
}

/// `name` and `description` of a SKILL.md's front matter (the YAML between its first two `---`
/// lines): top-level keys only, each a plain, quoted or block (`>`, `|`) scalar, as plain text.
pub fn front_matter(md: &str) -> (Option<String>, Option<String>) {
    let mut lines = md.lines();
    if lines.next() != Some("---") {
        return (None, None);
    }
    let yaml: Vec<&str> = lines.take_while(|l| l.trim_end() != "---").collect();
    (scalar(&yaml, "name"), scalar(&yaml, "description"))
}

fn scalar(yaml: &[&str], key: &str) -> Option<String> {
    let i = yaml
        .iter()
        .position(|l| l.strip_prefix(key).is_some_and(|r| r.starts_with(':')))?;
    let first = yaml[i][key.len() + 1..].trim();
    // What follows it and belongs to it: indented lines, and empty ones between them.
    let more: Vec<&str> = yaml[i + 1..]
        .iter()
        .take_while(|l| l.trim().is_empty() || l.starts_with([' ', '\t']))
        .map(|l| l.trim())
        .collect();
    let v = if first.starts_with(['>', '|']) {
        let fold = first.starts_with('>');
        let mut out = String::new();
        for l in &more {
            if l.is_empty() {
                out.push('\n');
            } else {
                if fold && !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(l);
                if !fold {
                    out.push('\n');
                }
            }
        }
        out
    } else {
        let joined = std::iter::once(first)
            .chain(more.iter().copied())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if joined.starts_with('"') {
            serde_json::from_str::<String>(&joined).ok()?
        } else if let Some(inner) = joined.strip_prefix('\'').and_then(|j| j.strip_suffix('\'')) {
            inner.replace("''", "'")
        } else {
            joined.split(" #").next().unwrap_or("").to_owned()
        }
    };
    let v = v.trim().to_owned();
    (!v.is_empty()).then_some(v)
}

/// Why an added skill's front matter is refused: fixed words, nothing of its text.
const NOT_SAME: &str = "SKILL.md: its front matter must be read the same by Clio and by Pi (plain `name: value` lines; name and description plain, quoted on one line, or `>`/`|` with lines of one indent): this one is not";

/// The `name` (when it has one) and the `description` of a skill the person adds, read only from
/// front matter that Clio's reading and a YAML parser (Pi's) cannot read differently: every line
/// at the left a `key: value` (unique keys), the rest indented; `name` and `description` plain
/// (no indicator first, nothing YAML would read as another type, on one line), quoted on one line
/// (no escapes), or a `>`, `>-`, `|`, `|-` block of lines of one indent. Anything else refuses it.
pub fn added_front_matter(md: &str) -> Result<(Option<String>, String), String> {
    let bad = || Err(NOT_SAME.to_owned());
    let lines: Vec<&str> = md.split('\n').collect();
    if lines.first() != Some(&"---") {
        return Err("SKILL.md: it must start with a front matter (a line `---`)".into());
    }
    // Pi ends it at the first line that starts with `---`: here it must be that line alone.
    let Some(end) = lines[1..].iter().position(|l| l.starts_with("---")) else {
        return bad();
    };
    if lines[end + 1] != "---" {
        return bad();
    }
    let yaml = &lines[1..=end];
    let key_re = |l: &str| -> Option<(String, String)> {
        let (k, v) = l.split_once(':')?;
        let ok = !k.is_empty()
            && k.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            && (v.is_empty() || v.starts_with(' '));
        ok.then(|| (k.to_owned(), v.trim().to_owned()))
    };
    let (mut keys, mut name, mut description) = (Vec::<String>::new(), None, None);
    for (i, l) in yaml.iter().enumerate() {
        if l.trim().is_empty() || l.starts_with(' ') || l.starts_with('#') {
            // An indented line belongs to the key above it: none above, and YAML reads another
            // mapping.
            if l.starts_with(' ') && !l.trim().is_empty() && keys.is_empty() {
                return bad();
            }
            continue;
        }
        let Some((k, v)) = key_re(l) else {
            return bad();
        };
        if keys.contains(&k) {
            return bad();
        }
        keys.push(k.clone());
        // What follows the key and is not at the left: its lines.
        let more: Vec<&str> = yaml[i + 1..]
            .iter()
            .take_while(|m| m.trim().is_empty() || m.starts_with(' '))
            .copied()
            .collect();
        let content: Vec<&str> = more
            .iter()
            .copied()
            .filter(|m| !m.trim().is_empty())
            .collect();
        if k != "name" && k != "description" {
            // Another key: a quoted or flow value closes on its line (else its next lines are
            // still it for YAML, and a `name:` among them would be read here).
            let closed = match v.chars().next() {
                Some('"') => quoted(&v, '"')
                    .is_some_and(|(_, rest)| rest.is_empty() || rest.starts_with('#')),
                Some('\'') => quoted(&v, '\'')
                    .is_some_and(|(_, rest)| rest.is_empty() || rest.starts_with('#')),
                Some('[') => v.ends_with(']'),
                Some('{') => v.ends_with('}'),
                _ => true,
            };
            if !closed {
                return bad();
            }
            continue;
        }
        let value = match v.as_str() {
            ">" | ">-" | "|" | "|-" => {
                // One indent for every line, no empty line among them: then YAML's reading is this.
                let Some(first) = content.first() else {
                    return bad();
                };
                let indent = first.len() - first.trim_start_matches(' ').len();
                let last = more.iter().rposition(|m| !m.trim().is_empty()).unwrap_or(0);
                if more[..=last].iter().any(|m| m.trim().is_empty())
                    || content
                        .iter()
                        .any(|m| m.len() - m.trim_start_matches(' ').len() != indent)
                {
                    return bad();
                }
                let parts: Vec<&str> = content.iter().map(|m| &m[indent..]).collect();
                let mut out = parts.join(if v.starts_with('>') { " " } else { "\n" });
                if !v.ends_with('-') {
                    out.push('\n');
                }
                out
            }
            _ if !content.is_empty() => return bad(),
            _ => match v.chars().next() {
                Some('"') => match quoted(&v, '"') {
                    Some((inner, "")) if !inner.contains('\\') => inner,
                    _ => return bad(),
                },
                Some('\'') => match quoted(&v, '\'') {
                    Some((inner, "")) => inner,
                    _ => return bad(),
                },
                None => return bad(),
                Some(c) if "-?:,[]{}#&*!|>%@`".contains(c) => return bad(),
                Some(_) => {
                    let plain = v.split(" #").next().unwrap_or("").trim_end();
                    let other_type = matches!(
                        plain,
                        "null"
                            | "Null"
                            | "NULL"
                            | "~"
                            | "true"
                            | "True"
                            | "TRUE"
                            | "false"
                            | "False"
                            | "FALSE"
                    ) || plain.parse::<f64>().is_ok()
                        || plain.starts_with("0x")
                        || plain.starts_with("0o")
                        || plain.starts_with('.');
                    if other_type || plain.contains(": ") || plain.ends_with(':') {
                        return bad();
                    }
                    plain.to_owned()
                }
            },
        };
        if k == "name" {
            name = Some(value);
        } else {
            description = Some(value);
        }
    }
    let d = description.unwrap_or_default();
    if d.trim().is_empty() || d.chars().count() > MAX_DESCRIPTION {
        return Err(format!(
            "SKILL.md: its front matter needs a description (at most {MAX_DESCRIPTION} characters)"
        ));
    }
    Ok((name, d))
}

/// A one-line quoted value: what is inside, and what is after it (trimmed). `''` is a `'` in a
/// single-quoted one; a double-quoted one ends at the first `"` not after a `\`.
fn quoted(v: &str, q: char) -> Option<(String, &str)> {
    let body = v.strip_prefix(q)?;
    let mut inner = String::new();
    let mut it = body.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if q == '"' && c == '\\' {
            inner.push(c);
            if let Some((_, n)) = it.next() {
                inner.push(n);
            }
        } else if c == q {
            if q == '\'' && it.peek().is_some_and(|(_, n)| *n == '\'') {
                it.next();
                inner.push('\'');
            } else {
                return Some((inner, body[i + 1..].trim()));
            }
        } else {
            inner.push(c);
        }
    }
    None
}

/// A name made Clio's: lowercase letters and digits, anything else one `-` between them, at most
/// 64 (Pi's rules). Empty when nothing of it is a letter or a digit.
pub fn clio_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let cut: String = out.chars().take(64).collect();
    cut.trim_end_matches('-').to_owned()
}

/// A skill the person adds, as found in its source (SKILL.md as written there, not composed):
/// Clio's name, a description in its front matter, its paths, instructions only of an
/// instruction's kind, held to `MAX_ADDED_FILES` and `MAX_ADDED_BYTES` and with no character the
/// box would not show as written; resources with paths of their own (none an instruction's), and
/// the whole held to `MAX_SKILL_FILES` and `MAX_SKILL_BYTES`. Checked when it is found and again
/// by whoever registers it.
pub fn check_added(
    name: &str,
    skill_md: &str,
    files: &[SkillFile],
    resources: &[Resource],
) -> Result<(), String> {
    check_instructions(name, skill_md, files)?;
    let mut seen: std::collections::HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let mut bytes = (skill_md.len() + files.iter().map(|f| f.content.len()).sum::<usize>()) as u64;
    for r in resources {
        valid_added_path(&r.path)?;
        if !seen.insert(r.path.as_str()) {
            return Err(format!("file {:?} is given twice", one_line(&r.path)));
        }
        if r.sha256.len() != 64 || !r.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("file {:?}: no digest", one_line(&r.path)));
        }
        bytes = bytes.saturating_add(r.size);
    }
    let count = (files.len() + 1 + resources.len()) as u64;
    if count > MAX_SKILL_FILES as u64 {
        return Err(over("files", count, MAX_SKILL_FILES as u64, false));
    }
    if bytes > MAX_SKILL_BYTES as u64 {
        return Err(over("in all", bytes, MAX_SKILL_BYTES as u64, true));
    }
    Ok(())
}

/// The instructions of `check_added`.
fn check_instructions(name: &str, skill_md: &str, files: &[SkillFile]) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("{:?} is not a skill name", one_line(name)));
    }
    if skill_md.chars().any(hidden) {
        return Err(
            "SKILL.md: it has a control, invisible or tab character the box would not show as written"
                .into(),
        );
    }
    // The name Pi reads is the one registered: its own, or (none given) its folder's, Clio's.
    if added_front_matter(skill_md)?.0.is_some_and(|n| n != name) {
        return Err(format!(
            "SKILL.md: the name in its front matter must be exactly «{name}» (lowercase letters, digits and single hyphens)"
        ));
    }
    if files.len() + 1 > MAX_ADDED_FILES {
        return Err(over(
            "instruction files",
            (files.len() + 1) as u64,
            MAX_ADDED_FILES as u64,
            false,
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut total = skill_md.len();
    for f in files {
        valid_added_path(&f.path)?;
        // Clio's rule decides what is an instruction, never the skill: a script, an image or a
        // template is never registered as one.
        if !instruction_kind(&f.path) {
            return Err(format!(
                "{:?} is not a document (.md, .markdown, .txt): only a resource, never an instruction",
                one_line(&f.path)
            ));
        }
        if !seen.insert(f.path.as_str()) {
            return Err(format!("file {:?} is given twice", one_line(&f.path)));
        }
        total += f.content.len();
    }
    if total > MAX_ADDED_BYTES {
        return Err(over(
            "of instructions",
            total as u64,
            MAX_ADDED_BYTES as u64,
            true,
        ));
    }
    for (what, t) in std::iter::once(("SKILL.md", skill_md))
        .chain(files.iter().map(|f| (f.path.as_str(), f.content.as_str())))
    {
        if t.chars().any(hidden) {
            return Err(format!(
                "{}: it has a control, invisible or tab character the box would not show as written",
                one_line(what)
            ));
        }
    }
    Ok(())
}

/// What the judge reads of a draft: SKILL.md and every file, whole, in one text.
pub fn whole_text(skill_md: &str, files: &[SkillFile]) -> String {
    let mut t = skill_md.to_owned();
    for f in files {
        t.push_str(&format!("\n--- file {} ---\n{}", f.path, f.content));
    }
    t
}

/// What the judge reads of an added skill: its instructions whole (`whole_text`), then the name of
/// every resource, never its bytes (a listing of the skill's folder is Clio's own text, so every
/// name in it has been read).
pub fn judged_text(skill_md: &str, files: &[SkillFile], resources: &[Resource]) -> String {
    let mut t = whole_text(skill_md, files);
    if !resources.is_empty() {
        t.push_str("\n--- the names of its other files (not read as instructions) ---\n");
        for r in resources {
            t.push_str(&r.path);
            t.push('\n');
        }
    }
    t
}

/// Whether `rel` (a path under the skill's folder) of registered skill `record` is one of its
/// instructions: its current version lists it in `instructions`. A version without that list
/// (written before resources existed, or by Clio) had every file judged and shown: all of it.
pub fn is_instruction(record: &Value, rel: &str) -> bool {
    let current = &record["current"];
    let listed = record["versions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["n"] == *current)
        .map(|v| &v["instructions"]);
    match listed {
        Some(Value::Array(a)) => a.iter().any(|p| p.as_str() == Some(rel)),
        _ => true,
    }
}

/// `records/<name>.json` of the skills folder `root`, if the skill is registered.
pub fn record(root: &Path, name: &str) -> Option<Value> {
    if !valid_name(name) {
        return None;
    }
    let t = fs::read_to_string(root.join(RECORDS).join(format!("{name}.json"))).ok()?;
    serde_json::from_str(&t).ok()
}

pub fn drafts_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("skills/drafts")
}

pub fn reports_file(run_dir: &Path) -> PathBuf {
    run_dir.join("skills/reports.jsonl")
}

/// Drafts the person's `clio` took for review (`<drafts>/taken/<id>.json`).
pub fn taken_dir(run_dir: &Path) -> PathBuf {
    drafts_dir(run_dir).join("taken")
}

/// `<run>/skills/dropped/<name>`: the time (ms) a draft of `name` was dropped after three failed
/// reviews; that name is refused for an hour after.
pub fn dropped_file(run_dir: &Path, name: &str) -> PathBuf {
    run_dir.join("skills/dropped").join(name)
}

/// When `name` was dropped within the hour before `now`: until when it is refused.
pub fn dropped_until(run_dir: &Path, name: &str, now: i64) -> Option<i64> {
    let at: i64 = fs::read_to_string(dropped_file(run_dir, name))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (at + HOUR_MS > now).then_some(at + HOUR_MS)
}

/// Every draft in `run_dir`, oldest first.
pub fn drafts(run_dir: &Path) -> Vec<Draft> {
    drafts_in(&drafts_dir(run_dir))
}

/// The drafts stored in `dir`, oldest first.
pub fn drafts_in(dir: &Path) -> Vec<Draft> {
    let mut all: Vec<Draft> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    all.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
    all
}

/// The files of `args["files"]`, or why they are not.
pub(crate) fn files_of(args: &Map<String, Value>) -> Result<Vec<SkillFile>, String> {
    match args.get("files") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|f| {
                let s = |k: &str| f.get(k).and_then(Value::as_str);
                match (s("path"), s("content")) {
                    (Some(path), Some(content)) => Ok(SkillFile {
                        path: path.to_owned(),
                        content: content.to_owned(),
                    }),
                    _ => Err("files: each one needs path and content".to_owned()),
                }
            })
            .collect(),
        Some(_) => Err("files must be a list of {path, content}".into()),
    }
}

impl ToolHost {
    pub(crate) fn skill_answer(
        &self,
        req: &Request,
        ok: bool,
        rule: &str,
        content: String,
    ) -> Response {
        Response {
            id: req.id.clone(),
            ok,
            content: content.clone(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: if ok { "allow" } else { "deny" }.into(),
                rule: rule.into(),
                reason: content,
            },
            tainted: self.is_tainted(&req.session),
            external: false,
        }
    }

    /// `skill_write`: checks the draft, has the judge read all of it and stores it for review.
    pub(crate) fn skill_write(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let fail = |why: &str| {
            self.skill_answer(
                req,
                false,
                "skills/draft",
                format!(
                    "skill_write failed: {}. Nothing was stored.",
                    one_line(&self.clean(why))
                ),
            )
        };
        let Some(root) = self.skills.as_deref() else {
            return fail("skills are not available in this session (only in a chat)");
        };
        // Under bypass nothing waits for the person (S-72): a flagged or unjudged conversation's
        // draft would.
        if let Some(why) = self
            .grant_state()
            .and_then(|st| self.bypass_refuses(st, &req.session))
        {
            return fail(why);
        }
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
        let (name, description) = (s("name"), s("description").trim());
        let files = match files_of(args) {
            Ok(f) => f,
            Err(why) => return fail(&why),
        };
        let reason = s("reason").trim();
        if reason.is_empty() || reason.chars().count() > MAX_REASON {
            return fail(&format!(
                "reason: why this skill, one line of at most {MAX_REASON} characters"
            ));
        }
        let skill_md = match check(name, description, s("body"), s("verify"), &files) {
            Ok(m) => m,
            Err(why) => return fail(&why),
        };
        // Nothing shaped like a secret: what the vault's filter would change is refused, whole.
        for t in std::iter::once(skill_md.as_str()).chain(files.iter().map(|f| f.content.as_str()))
        {
            match self.try_clean(t) {
                None => return fail("the vault could not filter the text"),
                Some(c) if c != t || count_masks(&c) > count_masks(t) => {
                    return fail(
                        "it holds something shaped like a secret (a key, a token, a password): a skill never carries one",
                    );
                }
                Some(_) => {}
            }
        }
        let now = now_ms();
        if let Some(until) = dropped_until(&self.run_dir, name, now) {
            return fail(&format!(
                "a draft of «{name}» was dropped after three failed reviews; that name is refused for {} more minutes. Tell the person what failed instead of writing it again",
                (until - now) / 60_000 + 1
            ));
        }
        let dir = drafts_dir(&self.run_dir);
        // The ones the person's `clio` took for review count too: each one cost a review.
        let recent = drafts(&self.run_dir)
            .into_iter()
            .chain(drafts_in(&taken_dir(&self.run_dir)))
            .filter(|d| d.session == req.session && d.created_ms > now - HOUR_MS)
            .count();
        if recent >= DRAFTS_PER_HOUR {
            return fail(&format!(
                "this session already wrote {DRAFTS_PER_HOUR} drafts in the last hour; each one costs a review"
            ));
        }
        // The judge reads it all as outside text: an order inside is withheld and marks the
        // session `suspect`; without the judge's answer nothing is stored.
        let text = whole_text(&skill_md, &files);
        let source = format!("skill_write «{name}»");
        match self.verdict_of(&text, &source) {
            TextVerdict::Clean => {}
            TextVerdict::Injection => {
                let w = ToJudge {
                    tool: "skill_write",
                    source,
                    agent: crate::judge::Agent::No,
                };
                let mut out = [Value::String(text.clone())];
                self.withhold(req, &mut out, &[0], &w, &text);
                return fail(
                    "Clio's judge flagged its text as an attempt to give instructions to an AI agent; the person can read it",
                );
            }
            TextVerdict::NoJudge => {
                return fail(
                    "a skill must be read by Clio's judge first, and the judge is off. Tell the person that skills need the judge on (jev.enabled in config.json)",
                );
            }
            TextVerdict::NoAnswer => {
                return fail("Clio's judge did not answer; try again later");
            }
        }
        let sha256 = digest(&skill_md, &files);
        let id: String = Sha256::digest(format!("{}|{sha256}|{now}", req.session).as_bytes())[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let update = record(root, name).is_some();
        let draft = Draft {
            id: id.clone(),
            session: req.session.clone(),
            name: name.to_owned(),
            description: description.to_owned(),
            reason: reason.to_owned(),
            verify: s("verify").trim().to_owned(),
            skill_md,
            files,
            created_ms: now,
            update,
            sha256,
            marked: self.is_tainted(&req.session),
            suspect: self.is_suspect(&req.session),
        };
        let stored = private_dir(&dir).and_then(|()| {
            let body = serde_json::to_vec_pretty(&draft).map_err(std::io::Error::other)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(dir.join(format!("{id}.json")))
                .and_then(|mut f| f.write_all(&body))
        });
        if let Err(e) = stored {
            return fail(&format!("the draft could not be kept: {e}"));
        }
        if let Some(h) = &self.host
            && let Err(e) = h.log(
                &req.session,
                "skill_write",
                &self.clean(name),
                false,
                "draft",
                "skills/draft",
                None,
            )
        {
            eprintln!("clio-toolhost: the action log lost a line: {e}");
        }
        let what = if update {
            "a new version of the skill"
        } else {
            "the new skill"
        };
        self.skill_answer(
            req,
            true,
            "skills/draft",
            format!(
                "draft {id} of {what} «{name}» stored. It is not registered yet: a reviewer checks it now, in the background, and the result comes by itself with a later message. Go on with the rest; do not write it again until then."
            ),
        )
    }

    /// `skill_report`: the model says following a skill worked or failed. Kept for the person's
    /// `clio`, which counts it; a report from a marked session says so.
    pub(crate) fn skill_report(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let fail = |why: &str| {
            self.skill_answer(
                req,
                false,
                "skills/report",
                format!("skill_report failed: {}", one_line(&self.clean(why))),
            )
        };
        let Some(root) = self.skills.as_deref() else {
            return fail("skills are not available in this session (only in a chat)");
        };
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
        let name = s("name");
        if record(root, name).is_none() {
            return fail(&format!("there is no skill «{}»", one_line(name)));
        }
        let outcome = s("outcome");
        if !matches!(outcome, "worked" | "failed") {
            return fail("outcome must be worked or failed");
        }
        let note = one_line(&self.clean(s("note")));
        if note.chars().count() > MAX_NOTE {
            return fail(&format!("note: at most {MAX_NOTE} characters"));
        }
        let line = json!({
            "at_ms": now_ms(),
            "session": req.session,
            "name": name,
            "outcome": outcome,
            "note": note,
            "marked": self.is_tainted(&req.session),
        });
        let file = reports_file(&self.run_dir);
        let kept = file.parent().map_or(Ok(()), private_dir).and_then(|()| {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&file)
                .and_then(|mut f| writeln!(f, "{line}"))
        });
        if let Err(e) = kept {
            return fail(&format!("the report could not be kept: {e}"));
        }
        self.skill_answer(
            req,
            true,
            "skills/report",
            format!("recorded: following «{name}» {outcome}. Clio counts it against the skill."),
        )
    }

    /// `read` of a resource of a registered, enabled skill (`p`, `rel` under skill `name`): never
    /// Clio's own text (S-05). Served from the skills folder as outside text: cleaned, marked
    /// `external` (the guard wraps it as data), the session marked as by a foreign file, and the
    /// text left for the judge, as a foreign file's is.
    fn skill_resource(&self, req: &Request, p: &Path, name: &str, rel: &str) -> Response {
        let refuse = |why: String| self.skill_answer(req, false, "skills/resource", why);
        if !fs::symlink_metadata(p).is_ok_and(|m| m.is_file()) {
            return refuse(format!("{}: not a file of the skill", req.tool));
        }
        let text = match fs::read(p) {
            Ok(b) if b.len() <= crate::MAX_RAW_BYTES => String::from_utf8_lossy(&b).into_owned(),
            Ok(b) => {
                return refuse(format!(
                    "[withheld] the output is {} bytes, more than the {} allowed",
                    b.len(),
                    crate::MAX_RAW_BYTES
                ));
            }
            Err(e) => return refuse(format!("{}: {e}", req.tool)),
        };
        let clean = self.clean(&text);
        let masked = count_masks(&clean).saturating_sub(count_masks(&text));
        self.mark(&req.session, "foreign");
        let what = format!("{rel} (a file of the skill «{name}» that is not its instructions)");
        self.judge_later_as(req, &what, crate::judge::Agent::No);
        Response {
            id: req.id.clone(),
            ok: true,
            content: clean,
            data: json!({ "masked": masked }),
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "skills/resource".into(),
                reason: "a skill's resource is outside text, never Clio's own".into(),
            },
            tainted: self.is_tainted(&req.session),
            external: true,
        }
    }

    /// `read`/`ls` of a file of a registered, enabled skill (`live/<name>/...`): served from the
    /// skills folder, cleaned but not marked as outside text (only the person's `clio` writes that
    /// folder, after the judge and a review). `None`: not such a path, and the call goes on as
    /// any other (an off or unknown skill, or another of Clio's folders, is refused as before).
    pub(crate) fn skill_read(&self, req: &Request, map: &Map<String, Value>) -> Option<Response> {
        let root = self.skills.as_deref()?;
        if !matches!(req.tool.as_str(), "fs_read" | "fs_stat" | "fs_list") {
            return None;
        }
        let raw = map.get("path").and_then(Value::as_str)?;
        let live = live(root).canonicalize().ok()?;
        // Copy mode: relative to the copy, so `../../skills/...` lands there too.
        let p = real(&self.work.join(raw))?;
        let rest = p.strip_prefix(&live).ok()?;
        let name = match rest.components().next() {
            Some(Component::Normal(n)) => n.to_str()?,
            _ => return None,
        };
        let rec = record(root, name)?;
        if rec["enabled"] != Value::Bool(true) {
            return None;
        }
        // A resource of the skill (S-05, S-68): outside text, as a foreign file is.
        let rel = rest.strip_prefix(name).ok()?.to_str()?;
        if req.tool == "fs_read" && !is_instruction(&rec, rel) {
            return Some(self.skill_resource(req, &p, name, rel));
        }
        let refuse = |why: &str| {
            Some(self.skill_answer(req, false, "skills/read", format!("{}: {why}", req.tool)))
        };
        let meta = fs::symlink_metadata(&p);
        let data = match req.tool.as_str() {
            "fs_stat" => {
                let mode = map.get("mode").and_then(Value::as_str).unwrap_or("exists");
                match (&meta, mode) {
                    (Err(_), "exists") => json!({"exists": false, "is_dir": false}),
                    (Err(_), _) => return refuse("no such file or folder"),
                    (Ok(_), "rw") => {
                        return refuse("a skill is read-only; change it with skill_write");
                    }
                    (Ok(m), _) => json!({"exists": true, "is_dir": m.is_dir()}),
                }
            }
            "fs_list" => {
                let mut v: Vec<String> = match fs::read_dir(&p) {
                    Ok(d) => d
                        .flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect(),
                    Err(e) => return refuse(&e.to_string()),
                };
                v.sort();
                json!({ "entries": v })
            }
            _ => {
                if !meta.as_ref().is_ok_and(fs::Metadata::is_file) {
                    return refuse("not a file of the skill");
                }
                let text = match fs::read(&p) {
                    // A skill the person added may be larger than one Clio wrote.
                    Ok(b) if b.len() <= MAX_ADDED_BYTES => String::from_utf8_lossy(&b).into_owned(),
                    Ok(_) => return refuse("too large"),
                    Err(e) => return refuse(&e.to_string()),
                };
                let clean = self.clean(&text);
                let masked = count_masks(&clean).saturating_sub(count_masks(&text));
                return Some(Response {
                    id: req.id.clone(),
                    ok: true,
                    content: clean,
                    data: json!({ "masked": masked, "skill": true }),
                    decision: DecisionOut {
                        outcome: "allow".into(),
                        rule: "skills/read".into(),
                        reason: "a registered skill is Clio's own instructions".into(),
                    },
                    tainted: self.is_tainted(&req.session),
                    external: false,
                });
            }
        };
        let mut data = data;
        data["skill"] = Value::Bool(true);
        Some(Response {
            id: req.id.clone(),
            ok: true,
            content: String::new(),
            data,
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "skills/read".into(),
                reason: "a registered skill is Clio's own instructions".into(),
            },
            tainted: self.is_tainted(&req.session),
            external: false,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_pis_rules() {
        for ok in ["pdf-tools", "a", "x1-y2", &"a".repeat(64)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "-a",
            "a-",
            "a--b",
            "A",
            "a_b",
            "a.b",
            "../x",
            &"a".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn file_paths_stay_inside_the_skill() {
        let f = |p: &str| {
            check(
                "x",
                &"d".repeat(30),
                "b",
                "v",
                &[SkillFile {
                    path: p.into(),
                    content: String::new(),
                }],
            )
        };
        for ok in ["scripts/check.sh", "a.md", "ref/a/b/c.txt"] {
            assert!(f(ok).is_ok(), "{ok}");
        }
        for bad in [
            "../x",
            "/etc/passwd",
            "a/../../b",
            "./a",
            ".hidden",
            "a//b",
            "a/",
            "SKILL.md",
            "skill.md",
            "a b",
            "a/b/c/d/e",
            "a\\b",
            "",
        ] {
            assert!(f(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_shape_is_checked_and_skill_md_is_composed() {
        let d = "Checks a thing. Use when the person asks to check it.";
        let md = check("x", d, "1. Do it.", "bash scripts/t.sh", &[]).unwrap();
        assert!(md.starts_with("---\nname: x\ndescription: \"Checks a thing."));
        assert!(md.ends_with("## How to verify\n\nbash scripts/t.sh\n"));
        assert!(check("x", "short", "b", "v", &[]).is_err());
        assert!(check("x", d, "b", "  ", &[]).is_err());
        assert!(check("x", d, "  ", "v", &[]).is_err());
        assert!(check("x", d, "a\n## How to verify\nb", "v", &[]).is_err());
        assert!(check("x", d, "tab\there", "v", &[]).is_err());
        assert!(check("x", d, "zero\u{200b}width", "v", &[]).is_err());
        assert!(check("x", &format!("{d}\nsecond"), "b", "v", &[]).is_err());
        // A check that succeeds whatever the skill does is refused, with what a check is.
        for none in [
            "true",
            " : ",
            "exit 0",
            "echo ok",
            "echo $(bash a.sh)",
            "echo",
        ] {
            let e = check("x", d, "b", none, &[]).unwrap_err();
            assert!(e.contains("no check"), "{none}: {e}");
        }
        for real in [
            "bash a.sh",
            "echo x | grep -q x",
            "true && bash a.sh",
            "echo a\nbash a.sh",
            "truex",
        ] {
            assert!(check("x", d, "b", real, &[]).is_ok(), "{real}");
        }
        let big = SkillFile {
            path: "a".into(),
            content: "x".repeat(MAX_FILE + 1),
        };
        assert!(check("x", d, "b", "v", &[big]).is_err());
        let many: Vec<SkillFile> = (0..=MAX_FILES)
            .map(|i| SkillFile {
                path: format!("f{i}"),
                content: String::new(),
            })
            .collect();
        assert!(check("x", d, "b", "v", &many).is_err());
        // A description with YAML in it stays one quoted string.
        let md = compose("x", "a: b # c \"q\"", "b", "v");
        assert!(md.contains("description: \"a: b # c \\\"q\\\"\"\n"));
    }

    // The front matter of skills as other harnesses write them: plain, quoted, folded.
    #[test]
    // S-68: an added skill's name and description are read only where Clio and YAML (Pi) read
    // them the same; anything else refuses it. (Pi's own parser agrees on the accepted ones:
    // `clio-brain/tests/skill_front_matter.rs`.)
    fn an_added_skills_front_matter_is_read_only_where_yaml_agrees() {
        let ok = |md: &str| added_front_matter(md).unwrap();
        assert_eq!(
            ok("---\nname: a-b\ndescription: Says hi # note\nlicense: MIT\n---\nbody"),
            (Some("a-b".into()), "Says hi".into())
        );
        assert_eq!(
            ok("---\ndescription: >\n  One\n  two.\nname: x\n---\n").1,
            "One two.\n"
        );
        assert_eq!(
            ok("---\ndescription: |-\n  One\n  two.\n---\n").1,
            "One\ntwo."
        );
        assert_eq!(ok("---\ndescription: 'It''s'\n---\n").1, "It's");
        assert_eq!(
            ok("---\ndescription: \"Quoted: yes\"\n---\n").1,
            "Quoted: yes"
        );
        assert_eq!(
            ok("---\ndescription: d\nmeta:\n  name: inner\n---\n").0,
            None
        );
        for bad in [
            "no front matter",
            "---\nname: &a skill-creator\ndescription: d\n---\n",
            "---\nname: *a\ndescription: d\n---\n",
            "---\nname: !!str skill-creator\ndescription: d\n---\n",
            "---\nname: [skill-creator]\ndescription: d\n---\n",
            "---\nname: {a: b}\ndescription: d\n---\n",
            "---\nname: \"skill-creator\ndescription: d\n---\n",
            "---\ndescription: \"a\\x41\"\n---\n",
            "---\ndescription: >2\n  d\n---\n",
            "---\ndescription: >\n  d\n    more\n---\n",
            "---\ndescription: >\n  d\n\n  e\n---\n",
            "---\ndescription: plain\n  continued\n---\n",
            "---\ndescription: a: b\n---\n",
            "---\ndescription: true\n---\n",
            "---\ndescription: 12\n---\n",
            "---\ndescription: d\ndescription: e\n---\n",
            "---\n\"name\": x\ndescription: d\n---\n",
            "---\nname : x\ndescription: d\n---\n",
            "---\n<<: *base\ndescription: d\n---\n",
            "---\n name: x\n description: d\n---\n",
            "---\nx: \"open\nname: evil\ndescription: d\n---\n",
            "---\nx: [a,\nname: evil]\ndescription: d\n---\n",
            "---\ndescription: d\n---x\n",
            "---\ndescription: d\n",
            "---\nname: x\n---\n",
        ] {
            assert!(added_front_matter(bad).is_err(), "{bad:?}");
        }
        // The name, when given, is exactly the one registered.
        let md = "---\nname: other\ndescription: d\n---\n";
        assert!(
            check_added("mine", md, &[], &[])
                .unwrap_err()
                .contains("exactly «mine»")
        );
        assert!(check_added("other", md, &[], &[]).is_ok());
    }

    #[test]
    fn the_front_matter_is_read_as_its_harnesses_write_it() {
        let fm = |y: &str| front_matter(&format!("---\n{y}\n---\nbody\n"));
        assert_eq!(
            fm("name: tdd\ndescription: Use when testing. Really # not this"),
            (Some("tdd".into()), Some("Use when testing. Really".into()))
        );
        assert_eq!(
            fm("name: \"a b\"\ndescription: \"Say \\\"hi\\\" - now\"").1,
            Some("Say \"hi\" - now".into())
        );
        assert_eq!(
            fm("description: >\n  Lazy mode: the\n  smallest change.\nlicense: MIT").1,
            Some("Lazy mode: the smallest change.".into())
        );
        assert_eq!(
            fm("description: |\n  one\n  two\n").1,
            Some("one\ntwo".into())
        );
        assert_eq!(fm("description: 'it''s'").1, Some("it's".into()));
        assert_eq!(fm("names: x\ndescription:").0, None);
        assert_eq!(fm("descriptions: x").1, None);
        assert_eq!(
            front_matter("no front matter\ndescription: x"),
            (None, None)
        );
        // Clio's own SKILL.md (a JSON string) reads the same way.
        let md = compose("x", "a: b # c \"q\"", "b", "v");
        assert_eq!(front_matter(&md).1.as_deref(), Some("a: b # c \"q\""));
        assert_eq!(
            clio_name("Test Driven_Development!"),
            "test-driven-development"
        );
        assert_eq!(clio_name("--ünï--"), "n");
        assert_eq!(clio_name(&"a".repeat(70)), "a".repeat(64));
        assert_eq!(clio_name("..."), "");
    }

    #[test]
    fn the_digest_is_of_exactly_these_bytes() {
        let f = |p: &str, c: &str| SkillFile {
            path: p.into(),
            content: c.into(),
        };
        let a = digest("md", &[f("a", "bc")]);
        assert_eq!(a, digest("md", &[f("a", "bc")]));
        assert_ne!(a, digest("md", &[f("ab", "c")]));
        assert_ne!(a, digest("md ", &[f("a", "bc")]));
        assert_ne!(a, digest("md", &[]));
    }

    fn res(path: &str, sha: char, size: u64) -> Resource {
        Resource {
            path: path.into(),
            size,
            sha256: sha.to_string().repeat(64),
            script: false,
        }
    }

    // S-68: the digest covers every resource (its path and its bytes' SHA-256); a skill without
    // resources keeps the digest it had before them; and an instruction whose text is a
    // resource's hash never digests like that resource (a separator no path can spell).
    #[test]
    fn the_digest_covers_every_resource_and_keeps_the_old_one_without_them() {
        let md = "---\nname: x\ndescription: d\n---\n";
        let doc = SkillFile {
            path: "a.md".into(),
            content: "doc".into(),
        };
        let base = digest_all(md, std::slice::from_ref(&doc), &[]);
        assert_eq!(base, digest(md, std::slice::from_ref(&doc)));
        let one = digest_all(md, std::slice::from_ref(&doc), &[res("a.png", 'a', 1)]);
        assert_ne!(one, base);
        assert_ne!(
            one,
            digest_all(md, std::slice::from_ref(&doc), &[res("a.png", 'b', 1)])
        );
        assert_ne!(
            one,
            digest_all(md, std::slice::from_ref(&doc), &[res("b.png", 'a', 1)])
        );
        let as_text = SkillFile {
            path: "a.png".into(),
            content: "a".repeat(64),
        };
        assert_ne!(one, digest(md, &[doc, as_text]));
    }

    // S-68: an added skill's limits say what was over and by how much; a non-document is never
    // an instruction; a resource's path is checked as an instruction's (eight deep).
    #[test]
    fn an_added_skills_limits_say_what_was_over_and_by_how_much() {
        let md = "---\nname: x\ndescription: d\n---\n";
        let many: Vec<Resource> = (0..MAX_SKILL_FILES)
            .map(|i| res(&format!("r{i}"), 'a', 1))
            .collect();
        let e = check_added("x", md, &[], &many).unwrap_err();
        assert_eq!(e, "4001 files: past the limit of 4000 by 1");
        assert!(check_added("x", md, &[], &many[1..]).is_ok());
        let big = [res("a.bin", 'a', MAX_SKILL_BYTES as u64)];
        let e = check_added("x", md, &[], &big).unwrap_err();
        assert!(
            e.starts_with("64.0 MiB in all: past the limit of 64.0 MiB by"),
            "{e}"
        );
        let docs: Vec<SkillFile> = (0..MAX_ADDED_FILES)
            .map(|i| SkillFile {
                path: format!("d{i}.md"),
                content: String::new(),
            })
            .collect();
        let e = check_added("x", md, &docs, &[]).unwrap_err();
        assert_eq!(e, "101 instruction files: past the limit of 100 by 1");
        let script = SkillFile {
            path: "run.sh".into(),
            content: "echo".into(),
        };
        let e = check_added("x", md, &[script], &[]).unwrap_err();
        assert!(e.contains("never an instruction"), "{e}");
        assert!(check_added("x", md, &[], &[res("a/b/c/d/e/f/g/h", 'a', 1)]).is_ok());
        assert!(check_added("x", md, &[], &[res("a/b/c/d/e/f/g/h/i", 'a', 1)]).is_err());
        assert!(check_added("x", md, &[], &[res("../x", 'a', 1)]).is_err());
        assert!(check_added("x", md, &[], &[res("/etc/x", 'a', 1)]).is_err());
        let twice = SkillFile {
            path: "a.md".into(),
            content: String::new(),
        };
        assert!(check_added("x", md, &[twice], &[res("a.md", 'a', 1)]).is_err());
        let mut nohash = res("a.png", 'a', 1);
        nohash.sha256 = "zz".into();
        assert!(check_added("x", md, &[], &[nohash]).is_err());
    }

    // S-05 S-68: the record decides what is Clio's own text: the current version's list of
    // instructions; a version without one (from before resources) had every file judged.
    #[test]
    fn only_a_listed_instruction_is_clios_own_text() {
        let r = json!({"current": 2, "versions": [
            {"n": 1},
            {"n": 2, "instructions": ["SKILL.md", "references/a.md"]},
        ]});
        assert!(is_instruction(&r, "SKILL.md") && is_instruction(&r, "references/a.md"));
        for not in ["scripts/run.sh", "assets/evil.md", "references", ""] {
            assert!(!is_instruction(&r, not), "{not}");
        }
        let old = json!({"current": 1, "versions": [{"n": 1}, {"n": 2, "instructions": []}]});
        assert!(is_instruction(&old, "scripts/run.sh"));
        assert!(is_instruction(&json!({"enabled": true}), "x.sh"));
    }
}
