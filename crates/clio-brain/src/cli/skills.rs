//! Clio's skills folder (S-68), written only here, in the person's own `clio` process: never from
//! the jail, never by the tool host. `<data>/skills`:
//!
//! - `versions/<name>/<n>/`: every version registered, as it was reviewed (or shipped), byte for
//!   byte. Nothing here is ever changed; `undo` goes back to the one before.
//! - `live/<name>/`: a copy of the current version of each enabled skill: what Pi loads
//!   (`--skill`), what the model reads unwrapped and what host commands see, read-only.
//! - `records/<name>.json`: name, description, on or off, current version, reports.
//! - `removed/<name>-<ms>/`: what `remove` took away (versions and record), kept.
//!
//! `clio skills` and `/skills` list them and turn them off, on, back or away.

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use clio_toolhost::skills::{self as th, SkillFile};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::home::Dirs;
use super::{Res, io_err, mkdir_private, now_ms, write_private};

/// The skill Clio ships: installed when missing, without a review (it is Clio's own text); a
/// later change to it goes through `skill_write` like any other.
pub const BUNDLED: &str = "skill-creator";
const BUNDLED_FILES: [(&str, &str); 2] = [
    (
        "SKILL.md",
        include_str!("../../../../skills/skill-creator/SKILL.md"),
    ),
    (
        "scripts/check-skill.sh",
        include_str!("../../../../skills/skill-creator/scripts/check-skill.sh"),
    ),
];
/// Failed reports in a row that turn a skill off.
pub const FAILS_TO_OFF: u32 = 3;
/// Reports kept in a record.
const KEPT_REPORTS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub n: u32,
    pub sha256: String,
    pub at_ms: i64,
    /// `review` (passed, registered by itself), `person` (approved), `bundled`.
    pub by: String,
    pub verdict: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub at_ms: i64,
    pub outcome: String,
    pub note: String,
    /// The session had read outside content when it reported.
    pub marked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    /// Why it is off: `None` while on.
    #[serde(default)]
    pub off: Option<String>,
    pub current: u32,
    pub versions: Vec<Version>,
    /// Failed reports since the last one that worked or the last version.
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub reports: Vec<Report>,
}

/// A skill to register: what was reviewed (or shipped), exactly.
pub struct New<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub skill_md: &'a str,
    pub files: &'a [SkillFile],
    pub by: &'a str,
    pub verdict: &'a str,
}

pub fn root(d: &Dirs) -> PathBuf {
    d.data.join("skills")
}

/// The folder Pi loads and commands see: the enabled skills.
pub fn live(d: &Dirs) -> PathBuf {
    root(d).join(th::LIVE)
}

fn record_path(root: &Path, name: &str) -> PathBuf {
    root.join(th::RECORDS).join(format!("{name}.json"))
}

fn version_dir(root: &Path, name: &str, n: u32) -> PathBuf {
    root.join(th::VERSIONS).join(name).join(n.to_string())
}

pub fn record(root: &Path, name: &str) -> Option<Record> {
    if !th::valid_name(name) {
        return None;
    }
    serde_json::from_str(&fs::read_to_string(record_path(root, name)).ok()?).ok()
}

fn save(root: &Path, r: &Record) -> Res<()> {
    let p = record_path(root, &r.name);
    let tmp = p.with_extension("json.tmp");
    write_private(
        &tmp,
        &serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?,
    )?;
    fs::rename(&tmp, &p).map_err(|e| io_err(&p, &e))
}

/// Every registered skill, by name.
pub fn records(root: &Path) -> Vec<Record> {
    let mut all: Vec<Record> = fs::read_dir(root.join(th::RECORDS))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            record(root, n.strip_suffix(".json")?)
        })
        .collect();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

/// One change at a time, across processes (the chat and `clio skills` may both write).
fn lock(root: &Path) -> Res<fs::File> {
    for d in [th::LIVE, th::VERSIONS, th::RECORDS] {
        mkdir_private(&root.join(d))?;
    }
    let p = root.join(".lock");
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&p)
        .map_err(|e| io_err(&p, &e))?;
    rustix::fs::flock(&f, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(f)
}

/// The skills folder exists (Pi's jail mounts `live`), with the shipped skill when it never was
/// there (one the person removed stays removed).
pub fn ensure(d: &Dirs) -> Res<()> {
    let root = root(d);
    let _l = lock(&root)?;
    let removed = fs::read_dir(root.join("removed"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("{BUNDLED}-"))
        });
    if record(&root, BUNDLED).is_some() || removed {
        return Ok(());
    }
    let files: Vec<SkillFile> = BUNDLED_FILES[1..]
        .iter()
        .map(|(p, c)| SkillFile {
            path: (*p).into(),
            content: (*c).into(),
        })
        .collect();
    let md = BUNDLED_FILES[0].1;
    let description = md
        .lines()
        .nth(2)
        .and_then(|l| l.strip_prefix("description: "))
        .and_then(|d| serde_json::from_str::<String>(d).ok())
        .unwrap_or_default();
    register_locked(
        &root,
        &New {
            name: BUNDLED,
            description: &description,
            skill_md: md,
            files: &files,
            by: "bundled",
            verdict: "shipped with Clio",
        },
    )
    .map(drop)
}

/// Registers `s` as a new version (the first, or the next): written to `versions`, then to `live`
/// unless the person turned the skill off. Returns the version number.
pub fn register(root: &Path, s: &New<'_>) -> Res<u32> {
    let _l = lock(root)?;
    register_locked(root, s)
}

fn register_locked(root: &Path, s: &New<'_>) -> Res<u32> {
    th::check_stored(s.name, s.skill_md, s.files)?;
    let old = record(root, s.name);
    let n = old
        .as_ref()
        .and_then(|r| r.versions.iter().map(|v| v.n).max())
        .unwrap_or(0)
        + 1;
    let dir = version_dir(root, s.name, n);
    let tmp = dir.with_file_name(format!(".tmp-{n}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    write_tree(&tmp, s.skill_md, s.files)?;
    fs::rename(&tmp, &dir).map_err(|e| io_err(&dir, &e))?;
    let version = Version {
        n,
        sha256: th::digest(s.skill_md, s.files),
        at_ms: now_ms(),
        by: s.by.into(),
        verdict: s.verdict.into(),
    };
    let mut r = old.unwrap_or_else(|| Record {
        name: s.name.into(),
        description: String::new(),
        enabled: true,
        off: None,
        current: n,
        versions: vec![],
        failures: 0,
        reports: vec![],
    });
    r.description = s.description.into();
    r.current = n;
    r.versions.push(version);
    r.failures = 0;
    // A new version, reviewed, turns back on a skill that failed; not one the person turned off.
    if r.off.as_deref() != Some(OFF_BY_PERSON) {
        r.enabled = true;
        r.off = None;
    }
    if r.enabled {
        install(root, &r.name, n)?;
    }
    save(root, &r)?;
    Ok(n)
}

/// SKILL.md and the files under `dir` (new): folders 0700, a script (`#!`) 0700, the rest 0600.
pub(crate) fn write_tree(dir: &Path, skill_md: &str, files: &[SkillFile]) -> Res<()> {
    mkdir_private(dir)?;
    for (path, content) in std::iter::once(("SKILL.md", skill_md))
        .chain(files.iter().map(|f| (f.path.as_str(), f.content.as_str())))
    {
        th::valid_path(path).or_else(|e| if path == "SKILL.md" { Ok(()) } else { Err(e) })?;
        let p = dir.join(path);
        if let Some(parent) = p.parent() {
            mkdir_private(parent)?;
        }
        let mode = if content.starts_with("#!") {
            0o700
        } else {
            0o600
        };
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&p)
            .and_then(|mut f| std::io::Write::write_all(&mut f, content.as_bytes()))
            .map_err(|e| io_err(&p, &e))?;
    }
    Ok(())
}

/// Copies a folder of plain files and folders (what `write_tree` made), modes kept.
fn copy_tree(from: &Path, to: &Path) -> Res<()> {
    mkdir_private(to)?;
    for e in fs::read_dir(from).map_err(|e| io_err(from, &e))?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let meta = fs::symlink_metadata(&src).map_err(|e| io_err(&src, &e))?;
        if meta.is_dir() {
            copy_tree(&src, &dst)?;
        } else if meta.is_file() {
            fs::copy(&src, &dst).map_err(|e| io_err(&dst, &e))?;
            fs::set_permissions(
                &dst,
                fs::Permissions::from_mode(meta.permissions().mode() & 0o700),
            )
            .map_err(|e| io_err(&dst, &e))?;
        }
    }
    Ok(())
}

/// `live/<name>` becomes version `n`, whole or not at all: built aside (a dot folder, which
/// neither Pi nor the tool host takes for a skill), then put in place with renames.
fn install(root: &Path, name: &str, n: u32) -> Res<()> {
    let live = root.join(th::LIVE);
    let pid = std::process::id();
    let (tmp, old, at) = (
        live.join(format!(".tmp-{name}-{pid}")),
        live.join(format!(".old-{name}-{pid}")),
        live.join(name),
    );
    let _ = fs::remove_dir_all(&tmp);
    let _ = fs::remove_dir_all(&old);
    copy_tree(&version_dir(root, name, n), &tmp)?;
    if at.exists() {
        fs::rename(&at, &old).map_err(|e| io_err(&at, &e))?;
    }
    fs::rename(&tmp, &at).map_err(|e| io_err(&at, &e))?;
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

fn uninstall(root: &Path, name: &str) -> Res<()> {
    let at = root.join(th::LIVE).join(name);
    match fs::remove_dir_all(&at) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_err(&at, &e)),
        _ => Ok(()),
    }
}

pub const OFF_BY_PERSON: &str = "turned off by the person";

fn known(root: &Path, name: &str) -> Res<Record> {
    record(root, name)
        .ok_or_else(|| format!("there is no skill «{}»", super::tui::sanitize::line(name)))
}

/// `off` / `on`: what Pi loads from the next session, and whether its files read as Clio's.
pub fn set_enabled(root: &Path, name: &str, on: bool, why: &str) -> Res<String> {
    let _l = lock(root)?;
    let mut r = known(root, name)?;
    r.enabled = on;
    r.off = (!on).then(|| why.to_owned());
    if on {
        r.failures = 0;
        install(root, name, r.current)?;
    } else {
        uninstall(root, name)?;
    }
    save(root, &r)?;
    Ok(format!(
        "«{name}» is {}: from the next session",
        if on { "on" } else { "off" }
    ))
}

/// Back to the version before the current one, exactly as it was registered.
pub fn undo(root: &Path, name: &str) -> Res<String> {
    let _l = lock(root)?;
    let mut r = known(root, name)?;
    let prev = r
        .versions
        .iter()
        .map(|v| v.n)
        .filter(|n| *n < r.current && version_dir(root, name, *n).is_dir())
        .max()
        .ok_or_else(|| format!("«{name}» has no earlier version: `remove` takes it away"))?;
    let from = r.current;
    r.current = prev;
    r.failures = 0;
    if let Some(v) = r.versions.iter().find(|v| v.n == prev) {
        // The description of that version: read from its SKILL.md.
        r.description = description_of(&version_dir(root, name, v.n)).unwrap_or(r.description);
    }
    if r.enabled {
        install(root, name, prev)?;
    }
    save(root, &r)?;
    Ok(format!(
        "«{name}» is back to version {prev} (was {from}): from the next session"
    ))
}

fn description_of(dir: &Path) -> Option<String> {
    let md = fs::read_to_string(dir.join("SKILL.md")).ok()?;
    let line = md.lines().nth(2)?.strip_prefix("description: ")?;
    serde_json::from_str(line).ok()
}

/// Takes the skill away: off Pi's list, its versions and record kept in `removed/`.
pub fn remove(root: &Path, name: &str) -> Res<String> {
    let _l = lock(root)?;
    known(root, name)?;
    uninstall(root, name)?;
    let to = root.join("removed").join(format!("{name}-{}", now_ms()));
    mkdir_private(&to)?;
    let versions = root.join(th::VERSIONS).join(name);
    if versions.exists() {
        fs::rename(&versions, to.join("versions")).map_err(|e| io_err(&versions, &e))?;
    }
    let rec = record_path(root, name);
    fs::rename(&rec, to.join("record.json")).map_err(|e| io_err(&rec, &e))?;
    Ok(format!(
        "«{name}» was removed (kept in {}): from the next session",
        to.display()
    ))
}

/// The reports the session's tool host kept (`skill_report`), counted once each: the file is
/// taken away before it is read. Returns what the person is told (a skill turned off).
pub fn apply_reports(root: &Path, toolhost_run: &Path) -> Vec<String> {
    let file = th::reports_file(toolhost_run);
    let taken = file.with_extension(format!("{}.applied", now_ms()));
    if fs::rename(&file, &taken).is_err() {
        return vec![];
    }
    let text = fs::read_to_string(&taken).unwrap_or_default();
    let Ok(_l) = lock(root) else {
        return vec![];
    };
    let mut notes = vec![];
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let s = |k: &str| v[k].as_str().unwrap_or("").to_owned();
        let Some(mut r) = record(root, &s("name")) else {
            continue;
        };
        let outcome = s("outcome");
        r.reports.push(Report {
            at_ms: v["at_ms"].as_i64().unwrap_or(0),
            outcome: outcome.clone(),
            note: s("note"),
            marked: v["marked"].as_bool().unwrap_or(false),
        });
        let extra = r.reports.len().saturating_sub(KEPT_REPORTS);
        r.reports.drain(..extra);
        match outcome.as_str() {
            "worked" => r.failures = 0,
            "failed" => r.failures += 1,
            _ => {}
        }
        if r.enabled && r.failures >= FAILS_TO_OFF {
            r.enabled = false;
            r.off = Some(format!("failed {FAILS_TO_OFF} times in a row"));
            if uninstall(root, &r.name).is_ok() {
                notes.push(format!(
                    "the skill «{}» failed {FAILS_TO_OFF} times in a row and was turned off (from the next session): /skills on {} turns it back on, a reviewed new version does too",
                    r.name, r.name
                ));
            }
        }
        if let Err(e) = save(root, &r) {
            notes.push(format!("a skill report was lost: {e}"));
        }
    }
    notes
}

/// The rows of `/skills` and `clio skills`.
pub fn lines(root: &Path) -> Vec<String> {
    let all = records(root);
    if all.is_empty() {
        return vec!["no skills yet".into()];
    }
    all.iter()
        .map(|r| {
            let state = if r.enabled {
                "on".to_owned()
            } else {
                format!("off ({})", r.off.as_deref().unwrap_or("off"))
            };
            let passed = r
                .versions
                .iter()
                .find(|v| v.n == r.current)
                .map_or_else(String::new, |v| {
                    format!(" · v{} {} {}", v.n, v.verdict, &super::stamp(v.at_ms)[..8])
                });
            let fails = if r.failures > 0 {
                format!(" · {} failed in a row", r.failures)
            } else {
                String::new()
            };
            super::tui::sanitize::line(&format!(
                "{} [{state}]{passed}{fails}: {}",
                r.name, r.description
            ))
        })
        .collect()
}

/// `clio skills [list|off|on|undo|remove NAME [--yes]]`. The person's: never the model's.
pub fn cmd(d: &Dirs, args: &[String]) -> Res<()> {
    let root = root(d);
    let name = || {
        args.get(1)
            .cloned()
            .ok_or_else(|| format!("clio skills {} <name>", args[0]))
    };
    match args.first().map(String::as_str) {
        None | Some("list") => {
            for l in lines(&root) {
                println!("{l}");
            }
            println!("\nclio skills off|on|undo|remove <name>");
            Ok(())
        }
        Some("off") => say(set_enabled(&root, &name()?, false, OFF_BY_PERSON)),
        Some("on") => say(set_enabled(&root, &name()?, true, "")),
        Some("undo") => say(undo(&root, &name()?)),
        Some("remove") => {
            let n = name()?;
            known(&root, &n)?;
            if !args.iter().any(|a| a == "--yes")
                && !super::apply::confirm(&format!(
                    "remove the skill «{n}»? (its versions are kept in removed/)"
                ))
            {
                println!("nothing was removed");
                return Ok(());
            }
            say(remove(&root, &n))
        }
        Some(other) => Err(format!(
            "clio skills: unknown {other:?} (list, off, on, undo, remove)"
        )),
    }
}

/// `/skills [off|on|undo|remove NAME --yes]` in the chat: the same as `clio skills`, in a box.
/// A removal reaches here only after the person said yes to it.
pub fn chat_command(d: &Dirs, args: &str) -> Res<super::tui::commands::Modal> {
    let root = root(d);
    let w: Vec<&str> = args.split_whitespace().collect();
    let done = match w.as_slice() {
        [] | ["list"] => None,
        ["off", n] => Some(set_enabled(&root, n, false, OFF_BY_PERSON)?),
        ["on", n] => Some(set_enabled(&root, n, true, "")?),
        ["undo", n] => Some(undo(&root, n)?),
        ["remove", n, "--yes"] => Some(remove(&root, n)?),
        _ => return Err("/skills [off|on|undo|remove <name>]".into()),
    };
    let mut lines = lines(&root);
    if let Some(m) = done {
        lines.insert(0, String::new());
        lines.insert(0, m);
    }
    let mut m = super::tui::commands::Modal::text("skills", lines);
    m.foot = Some("/skills off|on|undo|remove <name>".into());
    Ok(m)
}

fn say(r: Res<String>) -> Res<()> {
    println!("{}", r?);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn tree(d: &Path) -> Vec<(PathBuf, Vec<u8>, u32)> {
        let mut out = vec![];
        let mut stack = vec![d.to_path_buf()];
        while let Some(x) = stack.pop() {
            for e in fs::read_dir(&x).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let m = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
                    out.push((p.strip_prefix(d).unwrap().into(), fs::read(&p).unwrap(), m));
                }
            }
        }
        out.sort();
        out
    }

    fn new<'a>(name: &'a str, md: &'a str, files: &'a [SkillFile]) -> New<'a> {
        New {
            name,
            description: "Says hello. Use when greeting.",
            skill_md: md,
            files,
            by: "review",
            verdict: "passes",
        }
    }

    fn md(name: &str, body: &str) -> String {
        th::compose(name, "Says hello. Use when greeting.", body, "true")
    }

    #[test]
    fn the_bundled_skill_is_a_valid_skill_that_passes_its_own_check() {
        let md = BUNDLED_FILES[0].1;
        let files = [SkillFile {
            path: BUNDLED_FILES[1].0.into(),
            content: BUNDLED_FILES[1].1.into(),
        }];
        th::check_stored(BUNDLED, md, &files).unwrap();
        assert!(md.contains("\n## How to verify\n\nbash scripts/check-skill.sh .\n"));
        let t = tempfile::tempdir().unwrap();
        write_tree(&t.path().join("s"), md, &files).unwrap();
        let o = std::process::Command::new("bash")
            .arg("scripts/check-skill.sh")
            .arg(".")
            .current_dir(t.path().join("s"))
            .output()
            .unwrap();
        assert!(o.status.success(), "{o:?}");
    }

    // S-68: what is registered is byte for byte what was reviewed; `undo` brings the version
    // before back exactly; off takes it out of what Pi loads.
    #[test]
    fn register_undo_off_and_remove() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("skills");
        let one = [SkillFile {
            path: "scripts/a.sh".into(),
            content: "#!/bin/bash\necho one\n".into(),
        }];
        let two = [SkillFile {
            path: "ref.md".into(),
            content: "two\n".into(),
        }];
        let (m1, m2) = (md("hi", "First."), md("hi", "Second."));
        assert_eq!(register(&root, &new("hi", &m1, &one)).unwrap(), 1);
        let live = root.join("live/hi");
        let first = tree(&live);
        assert_eq!(first, tree(&root.join("versions/hi/1")));
        assert_eq!(fs::read_to_string(live.join("SKILL.md")).unwrap(), m1);
        assert_eq!(first[1].2, 0o700, "a script can run");
        assert_eq!(register(&root, &new("hi", &m2, &two)).unwrap(), 2);
        assert_eq!(fs::read_to_string(live.join("SKILL.md")).unwrap(), m2);
        assert!(!live.join("scripts").exists());
        undo(&root, "hi").unwrap();
        assert_eq!(
            tree(&live),
            first,
            "undo restores the version before exactly"
        );
        assert!(undo(&root, "hi").is_err());
        set_enabled(&root, "hi", false, OFF_BY_PERSON).unwrap();
        assert!(!live.exists());
        // A new version does not turn on what the person turned off.
        register(&root, &new("hi", &m2, &[])).unwrap();
        assert!(!live.exists() && !record(&root, "hi").unwrap().enabled);
        set_enabled(&root, "hi", true, "").unwrap();
        assert!(live.join("SKILL.md").exists());
        remove(&root, "hi").unwrap();
        assert!(!live.exists() && record(&root, "hi").is_none());
        // Only plain, checked text gets in.
        assert!(register(&root, &new("x", "no frontmatter", &[])).is_err());
        let bad = [SkillFile {
            path: "../escape".into(),
            content: String::new(),
        }];
        assert!(register(&root, &new("x", &md("x", "b"), &bad)).is_err());
        assert!(!t.path().join("escape").exists());
    }

    // S-68: three failed reports in a row turn a skill off; one that worked in between resets.
    #[test]
    fn three_failed_reports_in_a_row_turn_a_skill_off() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("skills");
        let run = t.path().join("run");
        let m = md("hi", "b");
        register(&root, &new("hi", &m, &[])).unwrap();
        let report = |outcomes: &[&str]| {
            fs::create_dir_all(run.join("skills")).unwrap();
            let text: String = outcomes
                .iter()
                .map(|o| {
                    format!(
                        "{}\n",
                        serde_json::json!({"name": "hi", "outcome": o, "note": "n", "marked": true, "at_ms": 1})
                    )
                })
                .collect();
            fs::write(th::reports_file(&run), text).unwrap();
            apply_reports(&root, &run)
        };
        assert!(report(&["failed", "failed", "worked", "failed", "failed"]).is_empty());
        assert!(record(&root, "hi").unwrap().enabled);
        // Read once: the same file is not counted again.
        assert!(apply_reports(&root, &run).is_empty());
        let notes = report(&["failed"]);
        assert_eq!(notes.len(), 1, "{notes:?}");
        let r = record(&root, "hi").unwrap();
        assert!(!r.enabled && !root.join("live/hi").exists());
        assert!(r.reports.iter().all(|x| x.marked));
        // A reviewed new version turns it back on.
        register(&root, &new("hi", &m, &[])).unwrap();
        assert!(record(&root, "hi").unwrap().enabled);
    }
}
