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

pub const LIVE: &str = "live";
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
    let mut h = Sha256::new();
    for part in std::iter::once(("SKILL.md", skill_md))
        .chain(files.iter().map(|f| (f.path.as_str(), f.content.as_str())))
    {
        for s in [part.0, part.1] {
            h.update((s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A file's path inside the skill: relative, plain names (`A-Z a-z 0-9 . _ -`, none starting with
/// a dot, so no `..`), at most four deep, never SKILL.md itself.
pub fn valid_path(p: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("file path {:?}: {why}", one_line(p)));
    if p.is_empty() || p.len() > MAX_PATH {
        return bad("empty or too long");
    }
    let parts: Vec<&str> = p.split('/').collect();
    if parts.len() > MAX_DEPTH {
        return bad("more than four folders deep");
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

/// What the judge reads of a draft: SKILL.md and every file, whole, in one text.
pub fn whole_text(skill_md: &str, files: &[SkillFile]) -> String {
    let mut t = skill_md.to_owned();
    for f in files {
        t.push_str(&format!("\n--- file {} ---\n{}", f.path, f.content));
    }
    t
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

/// Every draft in `run_dir`, oldest first.
pub fn drafts(run_dir: &Path) -> Vec<Draft> {
    let mut all: Vec<Draft> = fs::read_dir(drafts_dir(run_dir))
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
fn files_of(args: &Map<String, Value>) -> Result<Vec<SkillFile>, String> {
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
    fn skill_answer(&self, req: &Request, ok: bool, rule: &str, content: String) -> Response {
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
        let dir = drafts_dir(&self.run_dir);
        let recent = drafts(&self.run_dir)
            .iter()
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
                };
                let mut out = [Value::String(text.clone())];
                self.withhold(req, &mut out, &[0], &w, &text);
                return fail(
                    "Clio's judge flagged its text as an attempt to give instructions to an AI agent; the person can read it",
                );
            }
            TextVerdict::NoJudge => {
                return fail(
                    "a skill must be read by Clio's judge first, and the judge is off (jev.enabled in config.json)",
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
        let live = root.join(LIVE).canonicalize().ok()?;
        // Copy mode: relative to the copy, so `../../skills/...` lands there too.
        let p = real(&self.work.join(raw))?;
        let rest = p.strip_prefix(&live).ok()?;
        let name = match rest.components().next() {
            Some(Component::Normal(n)) => n.to_str()?,
            _ => return None,
        };
        if record(root, name).is_none_or(|r| r["enabled"] != Value::Bool(true)) {
            return None;
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
                    Ok(b) if b.len() <= MAX_TOTAL => String::from_utf8_lossy(&b).into_owned(),
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
}
