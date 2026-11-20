//! Skills the person installs (design `2026-11-20-skills-add-design.md`, S-68): from a folder that
//! holds a skill or several, or from a GitHub repository's archive at a ref. Shared by the tool
//! host's `skill_add` (the model proposes; each skill waits for the person) and the person's
//! `clio skills add` (asked at the terminal). Nothing here writes the skills folder: it finds the
//! skills, checks them, has the judge read every file, and gives each one as the arguments of an
//! item of class `skill` (tool `skill_add`) that only the person's own yes registers: no
//! permission window and no bypass answers it (`grant`).
//!
//! - GitHub only over https (`codeload.github.com`), the archive at most `MAX_ARCHIVE`, at most
//!   `MAX_UNPACKED` unpacked, read here in memory: nothing in it runs and nothing of it is written
//!   but the download itself, in a staging folder under Clio's data, removed at once.
//! - A path outside the archive's folder, or an entry that is not a file, a folder or a link,
//!   refuses the whole archive. A link is never followed or kept: one inside a skill refuses it.
//! - In a skill, a file that is not text, whose name Clio does not take (`valid_path`) or that
//!   holds characters the box would not show as written is skipped, and the box says so; the rest
//!   is held to `MAX_ADDED_FILES` and `MAX_ADDED_BYTES` (`skills::check_added`).
//! - Only skills are taken: a plugin's hooks, commands and agents are named, never installed.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::TextVerdict;
use crate::judge::one_line;
use crate::skills::{self, SkillFile};

/// The model's tool, and the tool of every item it sets aside.
pub const TOOL: &str = "skill_add";
/// Why each item waits: a skill is text the model will read as Clio's own.
pub const WAITS: &str =
    "a skill from outside is text Clio will read as her own: only you add it, every time";
/// The downloaded archive, at most.
pub const MAX_ARCHIVE: u64 = 20 << 20;
/// What the archive unpacks to, or what is read of a folder, at most.
const MAX_UNPACKED: usize = 100 << 20;
/// Entries of an archive or a folder read at most.
const MAX_ENTRIES: usize = 20_000;
/// The parts of a plugin that are not skills: named, never installed.
const PLUGIN_PARTS: [&str; 3] = ["hooks", "commands", "agents"];
const WHERE: &str = "a folder with SKILL.md, skills/*/SKILL.md, .claude/skills/*/SKILL.md or plugins/*/skills/*/SKILL.md";

/// Where skills come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Folder(PathBuf),
    GitHub {
        owner: String,
        repo: String,
        /// A branch, a tag or a commit; `HEAD` (the default branch) when none is given.
        reference: String,
    },
}

impl Source {
    /// What the record keeps as where a skill came from: a folder's path, or
    /// `github.com/<owner>/<repo>` (lowercase: GitHub does not tell them apart). The ref is not
    /// part of it: the same repository at another ref is a new version.
    pub fn origin(&self) -> String {
        match self {
            Self::Folder(p) => p.display().to_string(),
            Self::GitHub { owner, repo, .. } => format!(
                "github.com/{}/{}",
                owner.to_lowercase(),
                repo.to_lowercase()
            ),
        }
    }
}

/// `source` as the person or the model gave it: a folder (`/…`, `./…`, `../…`, `~/…`; relative
/// to `base`), or a GitHub repository, `owner/repo` or `https://github.com/owner/repo`, with
/// `@ref` or not.
pub fn parse(source: &str, home: &Path, base: &Path) -> Result<Source, String> {
    let s = source.trim();
    if s == "~" || s.starts_with("~/") {
        return Ok(Source::Folder(home.join(s[1..].trim_start_matches('/'))));
    }
    if s.starts_with('/') || s.starts_with("./") || s.starts_with("../") || s == "." {
        return Ok(Source::Folder(base.join(s)));
    }
    let bad = || {
        Err(format!(
            "{:?} is not a source: a folder (/…, ./… or ~/…) or a GitHub repository (owner/repo, or https://github.com/owner/repo, with @ref or not); only GitHub, only over https",
            one_line(s)
        ))
    };
    let (repo_at, reference) = match s.rsplit_once('@') {
        Some((r, at)) => (r, at),
        None => (s, "HEAD"),
    };
    let path = repo_at
        .strip_prefix("https://github.com/")
        .unwrap_or(repo_at)
        .trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let Some((owner, repo)) = path.split_once('/') else {
        return bad();
    };
    let word = |w: &str, max: usize, more: &[u8]| {
        !w.is_empty()
            && w.len() <= max
            && !w.starts_with(['-', '.'])
            && w.bytes()
                .all(|b| b.is_ascii_alphanumeric() || more.contains(&b))
    };
    let ref_ok = word(reference, 100, b"._-/")
        && !reference.contains("..")
        && !reference.contains("//")
        && !reference.ends_with('/');
    if !(word(owner, 39, b"-") && word(repo, 100, b"._-") && ref_ok) {
        return bad();
    }
    Ok(Source::GitHub {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        reference: reference.to_owned(),
    })
}

/// One entry of a source, by its path under the source's top folder.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    File(Vec<u8>),
    Dir,
    /// A link (or anything that is not a plain file or folder): never followed, never kept.
    Link,
}

type Tree = BTreeMap<String, Item>;

/// Whether `rel` can matter: the top's own entries, and what is under the layouts' folders (or,
/// when the top is a skill itself, all of it but names that start with a dot).
fn relevant(rel: &str, whole: bool) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    if whole {
        return !parts.iter().any(|p| p.starts_with('.'));
    }
    match parts.as_slice() {
        [_] | ["skills", ..] | [".claude", _] | [".claude", "skills", ..] => true,
        ["plugins", rest @ ..] => rest.len() <= 2 || rest[1] == "skills",
        _ => false,
    }
}

/// The relevant entries of folder `top`, no link followed (a link is kept as one).
fn read_folder(top: &Path) -> Result<Tree, String> {
    let whole = fs::symlink_metadata(top.join("SKILL.md")).is_ok_and(|m| m.is_file());
    let (mut tree, mut bytes, mut stack) = (Tree::new(), 0usize, vec![String::new()]);
    while let Some(dir) = stack.pop() {
        let at = top.join(&dir);
        let names: Vec<String> = fs::read_dir(&at)
            .map_err(|e| format!("{}: {e}", at.display()))?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        for n in names {
            let rel = if dir.is_empty() {
                n
            } else {
                format!("{dir}/{n}")
            };
            if !relevant(&rel, whole) {
                continue;
            }
            if tree.len() >= MAX_ENTRIES {
                return Err(format!(
                    "more than {MAX_ENTRIES} entries: name a narrower folder"
                ));
            }
            let p = top.join(&rel);
            let m = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let item = if m.is_dir() {
                stack.push(rel.clone());
                Item::Dir
            } else if m.is_file() {
                bytes += usize::try_from(m.len()).unwrap_or(usize::MAX);
                if bytes > MAX_UNPACKED {
                    return Err(format!(
                        "more than {} MiB to read: name a narrower folder",
                        MAX_UNPACKED >> 20
                    ));
                }
                let mut b = Vec::new();
                fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                    .open(&p)
                    .and_then(|mut f| f.read_to_end(&mut b))
                    .map_err(|e| format!("{}: {e}", p.display()))?;
                Item::File(b)
            } else {
                Item::Link
            };
            tree.insert(rel, item);
        }
    }
    Ok(tree)
}

/// A number of a tar header: octal, NUL or space ended. `None`: not one (base-256 too).
fn octal(field: &[u8]) -> Option<usize> {
    let t: Vec<u8> = field
        .iter()
        .copied()
        .take_while(|b| *b != 0)
        .filter(|b| *b != b' ')
        .collect();
    if t.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(std::str::from_utf8(&t).ok()?, 8).ok()
}

/// The records of a pax header (`<len> <key>=<value>\n`).
fn pax(b: &[u8]) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut rest = b;
    while let Some(sp) = rest.iter().position(|c| *c == b' ') {
        let Some(n) = std::str::from_utf8(&rest[..sp])
            .ok()
            .and_then(|n| n.parse::<usize>().ok())
        else {
            break;
        };
        if n <= sp + 1 || n > rest.len() {
            break;
        }
        let rec = &rest[sp + 1..n];
        let rec = rec.strip_suffix(b"\n").unwrap_or(rec);
        if let Some(eq) = rec.iter().position(|c| *c == b'=') {
            out.push((
                String::from_utf8_lossy(&rec[..eq]).into_owned(),
                String::from_utf8_lossy(&rec[eq + 1..]).into_owned(),
            ));
        }
        rest = &rest[n..];
    }
    out
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).into_owned()
}

/// An unpacked tar (as GitHub makes them: one top folder, pax headers): the commit its global
/// header names, and its relevant entries under the top folder. Read here, nothing written.
fn untar(tar: &[u8]) -> Result<(Option<String>, Tree), String> {
    let damaged = || "the archive is damaged: nothing of it was taken".to_owned();
    let outside =
        || "the archive has a path outside its folder: nothing of it was taken".to_owned();
    let (mut at, mut commit, mut long) = (0usize, None, None::<String>);
    let mut all: Vec<(String, u8, std::ops::Range<usize>)> = vec![];
    let mut top: Option<String> = None;
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        let size = octal(&h[124..136]).ok_or_else(damaged)?;
        let start = at + 512;
        let end = start
            .checked_add(size)
            .filter(|e| *e <= tar.len())
            .ok_or_else(damaged)?;
        at = start + size.div_ceil(512) * 512;
        let kind = h[156];
        if matches!(kind, b'g' | b'x') {
            for (k, v) in pax(&tar[start..end]) {
                match (kind, k.as_str()) {
                    (b'g', "comment") => commit = Some(v),
                    (b'x', "path") => long = Some(v),
                    _ => {}
                }
            }
            continue;
        }
        if !matches!(kind, b'0' | 0 | b'7' | b'5' | b'1' | b'2') {
            return Err(
                "the archive has an entry that is not a file, a folder or a link: nothing of it was taken"
                    .into(),
            );
        }
        let name = long.take().unwrap_or_else(|| {
            let (name, prefix) = (cstr(&h[..100]), cstr(&h[345..500]));
            if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            }
        });
        let name = name.trim_end_matches('/').to_owned();
        if name.starts_with('/') || name.split('/').any(|p| p == ".." || p.is_empty()) {
            return Err(outside());
        }
        let (first, rest) = name.split_once('/').unwrap_or((&name, ""));
        match &top {
            None => top = Some(first.to_owned()),
            Some(t) if t != first => return Err(outside()),
            Some(_) => {}
        }
        if !rest.is_empty() {
            all.push((rest.to_owned(), kind, start..end));
        }
        if all.len() > MAX_ENTRIES {
            return Err(format!("the archive has more than {MAX_ENTRIES} entries"));
        }
    }
    let whole = all
        .iter()
        .any(|(n, k, _)| n == "SKILL.md" && matches!(k, b'0' | 0 | b'7'));
    let mut tree = Tree::new();
    for (n, k, r) in all {
        // The folders on its way that can matter (a plugin's `hooks/`), even when it does not.
        let mut up = n.as_str();
        while let Some((dir, _)) = up.rsplit_once('/') {
            if relevant(dir, whole) {
                tree.entry(dir.to_owned()).or_insert(Item::Dir);
            }
            up = dir;
        }
        if relevant(&n, whole) {
            let item = match k {
                b'5' => Item::Dir,
                b'1' | b'2' => Item::Link,
                _ => Item::File(tar[r].to_vec()),
            };
            tree.insert(n, item);
        }
    }
    let commit = commit.filter(|c| c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit()));
    Ok((commit, tree))
}

/// `file` gunzipped, at most `MAX_UNPACKED`.
fn gunzip(file: &Path) -> Result<Vec<u8>, String> {
    let mut child = Command::new("gzip")
        .arg("-dc")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("gzip could not start: {e}"))?;
    let mut out = Vec::new();
    let read = child
        .stdout
        .take()
        .map(|o| o.take(MAX_UNPACKED as u64 + 1).read_to_end(&mut out));
    if out.len() > MAX_UNPACKED {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "the archive unpacks to more than {} MiB: nothing of it was taken",
            MAX_UNPACKED >> 20
        ));
    }
    let ok = child.wait().is_ok_and(|s| s.success());
    match read {
        Some(Ok(_)) if ok => Ok(out),
        _ => Err("the download is not a gzip archive".into()),
    }
}

/// The archive of `owner/repo` at `reference`, from GitHub over https only (TLS 1.2 or newer, no
/// redirect off https, at most `MAX_ARCHIVE`), downloaded into `staging` and removed there.
fn fetch(owner: &str, repo: &str, reference: &str, staging: &Path) -> Result<Vec<u8>, String> {
    crate::host::private_dir(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let file = staging.join(format!(
        "{}-{}.tar.gz",
        std::process::id(),
        crate::host::now_ms()
    ));
    let url = format!("https://codeload.github.com/{owner}/{repo}/tar.gz/{reference}");
    // The installer's flags (`setup::fetch::get`), and a cap on the size and the time.
    let got = Command::new("curl")
        .args([
            "-fsSL",
            "--retry",
            "3",
            "--connect-timeout",
            "20",
            "-y",
            "60",
            "-Y",
            "1",
            "--max-time",
            "300",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--max-filesize",
            &MAX_ARCHIVE.to_string(),
            "-o",
        ])
        .arg(&file)
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let r = match got {
        Err(e) => Err(format!("curl could not start: {e}")),
        Ok(s) if !s.success() => Err(format!(
            "GitHub did not give the archive of {owner}/{repo} at {reference} (no such repository or ref, larger than {} MiB, or no network)",
            MAX_ARCHIVE >> 20
        )),
        Ok(_) if !fs::metadata(&file).is_ok_and(|m| m.len() <= MAX_ARCHIVE) => Err(format!(
            "the archive is larger than {} MiB: nothing of it was taken",
            MAX_ARCHIVE >> 20
        )),
        Ok(_) => gunzip(&file),
    };
    let _ = fs::remove_file(&file);
    r
}

/// The folders of `tree` that hold a skill, and the layouts they were found in.
fn skill_dirs(tree: &Tree) -> (Vec<String>, Vec<&'static str>) {
    if matches!(tree.get("SKILL.md"), Some(Item::File(_))) {
        return (vec![String::new()], vec!["a folder with SKILL.md"]);
    }
    let (mut dirs, mut layouts) = (vec![], vec![]);
    for (k, v) in tree {
        let Some(dir) = k.strip_suffix("/SKILL.md") else {
            continue;
        };
        if *v == Item::Dir {
            continue;
        }
        let layout = match dir.split('/').collect::<Vec<_>>().as_slice() {
            ["skills", _] => "skills/*/SKILL.md",
            [".claude", "skills", _] => ".claude/skills/*/SKILL.md",
            ["plugins", _, "skills", _] => "plugins/*/skills/*/SKILL.md",
            _ => continue,
        };
        dirs.push(dir.to_owned());
        if !layouts.contains(&layout) {
            layouts.push(layout);
        }
    }
    (dirs, layouts)
}

/// The parts of a plugin `tree` holds that are not skills (never installed).
fn plugin_parts(tree: &Tree) -> Vec<&'static str> {
    PLUGIN_PARTS
        .into_iter()
        .filter(|part| {
            tree.keys().any(|k| {
                let p: Vec<&str> = k.split('/').collect();
                matches!(p.as_slice(), [x, ..] | [".claude", x, ..] | ["plugins", _, x, ..] if x == part)
            })
        })
        .collect()
}

fn text(b: &[u8]) -> Option<String> {
    String::from_utf8(b.to_vec())
        .ok()
        .filter(|t| !t.contains('\0'))
}

/// A skill found in a source, checked (`skills::check_added`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub description: String,
    /// Its folder in the source (empty: the source's top).
    pub from: String,
    pub skill_md: String,
    pub files: Vec<SkillFile>,
    /// What of its folder is not installed, and why.
    pub skipped: Vec<String>,
}

/// The name of the skill in `dir`: its front matter's `name`, or the folder's, made Clio's.
fn name_in(tree: &Tree, dir: &str, top: &str) -> String {
    let md = match tree.get(&format!(
        "{dir}{}SKILL.md",
        if dir.is_empty() { "" } else { "/" }
    )) {
        Some(Item::File(b)) => text(b).unwrap_or_default(),
        _ => String::new(),
    };
    let folder = if dir.is_empty() {
        top
    } else {
        dir.rsplit('/').next().unwrap_or(dir)
    };
    skills::clio_name(
        &skills::front_matter(&md)
            .0
            .unwrap_or_else(|| folder.to_owned()),
    )
}

fn build(tree: &Tree, dir: &str, name: &str) -> Result<Found, String> {
    let who = |why: String| {
        format!(
            "the skill «{name}» ({}): {why}",
            if dir.is_empty() { "." } else { dir }
        )
    };
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let (mut md, mut files, mut skipped) = (None, vec![], vec![]);
    for (k, v) in tree.range(prefix.clone()..) {
        let Some(rel) = k.strip_prefix(&prefix) else {
            break;
        };
        match v {
            Item::Dir => {}
            Item::Link => {
                return Err(who(format!(
                    "{rel} is a link: a skill with a link is not taken"
                )));
            }
            Item::File(b) if rel == "SKILL.md" => {
                md = Some(text(b).ok_or_else(|| who("its SKILL.md is not text".into()))?);
            }
            Item::File(_) if skills::valid_path(rel).is_err() => {
                skipped.push(format!("{rel} (a name Clio does not take)"));
            }
            Item::File(b) => match text(b) {
                Some(t) if !t.chars().any(skills::hidden) => files.push(SkillFile {
                    path: rel.to_owned(),
                    content: t,
                }),
                Some(_) => {
                    skipped.push(format!(
                        "{rel} (characters the box would not show as written)"
                    ));
                }
                None => skipped.push(format!("{rel} (not text)")),
            },
        }
    }
    let md = md.ok_or_else(|| who("no SKILL.md".into()))?;
    let description = skills::front_matter(&md).1.unwrap_or_default();
    skills::check_added(name, &md, &files).map_err(who)?;
    Ok(Found {
        name: name.to_owned(),
        description,
        from: dir.to_owned(),
        skill_md: md,
        files,
        skipped,
    })
}

/// Who holds a skill's name: `None` nobody, `Some(None)` Clio's own (written by her, or shipped),
/// `Some(Some(origin))` where it was first added from.
pub fn owner(root: &Path, name: &str) -> Option<Option<String>> {
    let r = skills::record(root, name)?;
    Some(r["versions"][0]["origin"].as_str().map(str::to_owned))
}

/// What a skill's `judge` field says: `clean`, `flagged`, `off` (no judge) or `silent`.
pub fn judge_word(v: TextVerdict) -> &'static str {
    match v {
        TextVerdict::Clean => "clean",
        TextVerdict::Injection => "flagged",
        TextVerdict::NoJudge => "off",
        TextVerdict::NoAnswer => "silent",
    }
}

/// What `prepare` needs from its caller.
pub struct Ctx<'a> {
    pub home: &'a Path,
    /// What a relative folder is under.
    pub base: &'a Path,
    /// Why a folder may not be read, if it may not (the model's: its read rules; the person's:
    /// none).
    pub refuse: &'a dyn Fn(&Path) -> Option<String>,
    /// The skills folder (`<data>/skills`): who holds each name.
    pub root: &'a Path,
    /// Where an archive is downloaded (under Clio's data); it is removed at once.
    pub staging: &'a Path,
    /// The judge's verdict on a text and where it is from.
    pub judge: &'a dyn Fn(&str, &str) -> TextVerdict,
}

/// Every skill of `source` (only those in `names`, when some are given), found, checked and
/// judged whole: each one the arguments of its item, in the source's order. One that fails a
/// check refuses the whole add, saying which and why (the others can be named).
pub fn prepare(source: &str, names: &[String], cx: &Ctx) -> Result<Vec<Value>, String> {
    let (src, tree, commit, top) = match parse(source, cx.home, cx.base)? {
        Source::Folder(p) => {
            let p = p
                .canonicalize()
                .map_err(|e| format!("{}: {e}", one_line(&p.display().to_string())))?;
            if let Some(why) = (cx.refuse)(&p) {
                return Err(why);
            }
            if !p.is_dir() {
                return Err(format!("{} is not a folder", p.display()));
            }
            let tree = read_folder(&p)?;
            let top = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (Source::Folder(p), tree, None, top)
        }
        Source::GitHub {
            owner,
            repo,
            reference,
        } => {
            let tar = fetch(&owner, &repo, &reference, cx.staging)?;
            let (commit, tree) = untar(&tar)?;
            let top = repo.clone();
            (
                Source::GitHub {
                    owner,
                    repo,
                    reference,
                },
                tree,
                commit,
                top,
            )
        }
    };
    let origin = src.origin();
    let (dirs, layouts) = skill_dirs(&tree);
    if dirs.is_empty() {
        return Err(format!("no skill in {origin}: Clio looks for {WHERE}"));
    }
    let left = plugin_parts(&tree);
    let named: Vec<(String, &String)> = dirs.iter().map(|d| (name_in(&tree, d, &top), d)).collect();
    if let Some(n) = names.iter().find(|n| !named.iter().any(|(m, _)| m == *n)) {
        return Err(format!(
            "there is no skill «{}» in {origin}; it has: {}",
            one_line(n),
            named
                .iter()
                .map(|(m, _)| m.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut out: Vec<Value> = vec![];
    for (name, dir) in named {
        if !names.is_empty() && !names.contains(&name) {
            continue;
        }
        if out.iter().any(|v| v["name"] == name.as_str()) {
            return Err(format!("{origin} has two skills named «{name}»"));
        }
        let f = build(&tree, dir, &name)?;
        let update = match owner(cx.root, &f.name) {
            None => false,
            Some(o) if o.as_deref() == Some(origin.as_str()) => true,
            Some(o) => {
                return Err(format!(
                    "the name «{}» is taken by {}: a skill of another origin is not added under it (remove that one first, or name the others)",
                    f.name,
                    o.unwrap_or_else(|| "a skill of Clio's own".into())
                ));
            }
        };
        let whole = skills::whole_text(&f.skill_md, &f.files);
        let verdict = (cx.judge)(&whole, &format!("skill «{}» from {origin}", f.name));
        out.push(json!({
            "name": f.name,
            "description": f.description,
            "skill_md": f.skill_md,
            "files": f.files,
            "sha256": skills::digest(&f.skill_md, &f.files),
            "update": update,
            "origin": origin,
            "commit": commit,
            "ref": match &src { Source::GitHub { reference, .. } => Some(reference.as_str()), Source::Folder(_) => None },
            "from": f.from,
            "layout": layouts.join(", "),
            "judge": judge_word(verdict),
            "skipped": f.skipped,
            "left": left,
        }));
    }
    Ok(out)
}

/// The model's reason, at most.
const MAX_REASON: usize = 300;

impl crate::ToolHost {
    /// `skill_add {source, names?, reason}`: finds, checks and judges the skills of `source` and
    /// sets each aside for the person (class `skill`, tool `skill_add`). Nothing is registered:
    /// only the person's own yes does that, never a permission window or bypass.
    pub(crate) fn skill_add(
        &self,
        req: &crate::Request,
        args: &serde_json::Map<String, Value>,
    ) -> crate::Response {
        let fail = |why: &str| {
            self.skill_answer(
                req,
                false,
                "skills/add",
                format!(
                    "skill_add failed: {}. Nothing was set aside.",
                    one_line(&self.clean(why))
                ),
            )
        };
        let (Some(root), Some(h)) = (self.skills.as_deref(), self.host.as_ref()) else {
            return fail(
                "skills from outside are added in a chat on the person's machine (host mode), or by the person with `clio skills add`",
            );
        };
        if self.review {
            return fail(crate::REVIEW_CANNOT_ASK);
        }
        if self.is_suspect(&req.session) {
            return fail(&format!(
                "{}: no skill is added from this session",
                crate::SUSPECT_REASON
            ));
        }
        if let Some(k) = args
            .keys()
            .find(|k| !matches!(k.as_str(), "source" | "names" | "reason"))
        {
            return fail(&format!("{:?} is not a field of skill_add", one_line(k)));
        }
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim();
        let reason = s("reason");
        if reason.is_empty() || reason.chars().count() > MAX_REASON || reason.contains('\n') {
            return fail(&format!(
                "reason: why these skills, one line of at most {MAX_REASON} characters"
            ));
        }
        let names: Vec<String> = match args.get("names") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(a)) if a.iter().all(Value::is_string) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            Some(_) => return fail("names must be a list of skill names"),
        };
        let home = h.cfg.home.as_path();
        let refuse =
            |p: &Path| crate::services::read_refused(&p.to_string_lossy(), home, &h.cfg.control);
        // `verdict`, not `verdict_of`: a port without a judge is "not judged" here, never clean.
        let judge = |t: &str, from: &str| match &self.memory {
            Some(port) => crate::judge::verdict(port, t, from),
            None => TextVerdict::NoJudge,
        };
        let staging = self.run_dir.join("skills/staging");
        let cx = Ctx {
            home,
            base: home,
            refuse: &refuse,
            root,
            staging: &staging,
            judge: &judge,
        };
        let items = match prepare(s("source"), &names, &cx) {
            Ok(i) => i,
            Err(why) => return fail(&why),
        };
        if items.len() > crate::pending::MAX_WAITING {
            return fail(&format!(
                "{} skills: at most {} wait at once; ask which ones the person wants and give them in names",
                items.len(),
                crate::pending::MAX_WAITING
            ));
        }
        let origin = items
            .first()
            .and_then(|a| a["origin"].as_str())
            .unwrap_or("")
            .to_owned();
        let mut set = vec![];
        for mut a in items {
            a["why"] = Value::String(self.clean(reason));
            let name = a["name"].as_str().unwrap_or("").to_owned();
            match crate::pending::set_aside_added(&h.cfg.state, &h.cfg.run, &req.session, a) {
                Ok(p) => set.push(format!("{} «{name}»", p.id)),
                Err(e) => {
                    return fail(&format!(
                        "«{name}» could not be set aside ({e}){}",
                        if set.is_empty() {
                            String::new()
                        } else {
                            format!("; set aside before it: {}", set.join(", "))
                        }
                    ));
                }
            }
        }
        self.skill_answer(
            req,
            true,
            "skills/add",
            format!(
                "set aside for the person, from {origin}: {}. Each is added only if the person says yes to it (never a permission window or bypass); then it is available from the next session. Nothing is installed yet. Do not add them again meanwhile; go on with the rest and say so at the end.",
                set.join(", ")
            ),
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A tar of `entries` (`path`, type, bytes) under `top/`, with a pax global header naming
    /// `commit`, as GitHub makes them.
    fn tar(top: &str, commit: &str, entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
        fn header(name: &str, kind: u8, size: usize) -> Vec<u8> {
            let mut h = vec![0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(b"0000644");
            h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
            h[156] = kind;
            h[257..262].copy_from_slice(b"ustar");
            h
        }
        fn push(out: &mut Vec<u8>, name: &str, kind: u8, body: &[u8]) {
            out.extend(header(name, kind, body.len()));
            out.extend(body);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        let mut out = vec![];
        let rec = format!("comment={commit}\n");
        let rec = format!("{} {rec}", rec.len() + 3);
        push(&mut out, "pax_global_header", b'g', rec.as_bytes());
        push(&mut out, &format!("{top}/"), b'5', b"");
        for (p, k, b) in entries {
            push(&mut out, &format!("{top}/{p}"), *k, b);
        }
        out.extend([0u8; 1024]);
        out
    }

    const MD: &str =
        "---\nname: Hello World\ndescription: Says hello. Use when greeting.\n---\n\nSay hello.\n";

    #[test]
    fn sources_are_folders_or_github_over_https() {
        let (h, b) = (Path::new("/h"), Path::new("/w"));
        assert_eq!(
            parse("obra/superpowers", h, b).unwrap(),
            Source::GitHub {
                owner: "obra".into(),
                repo: "superpowers".into(),
                reference: "HEAD".into()
            }
        );
        let g = parse("https://github.com/Obra/SuperPowers.git@v1.2/x", h, b).unwrap();
        assert_eq!(g.origin(), "github.com/obra/superpowers");
        assert!(matches!(g, Source::GitHub { ref reference, .. } if reference == "v1.2/x"));
        assert_eq!(parse("~/s", h, b).unwrap(), Source::Folder("/h/s".into()));
        assert_eq!(parse("./s", h, b).unwrap(), Source::Folder("/w/./s".into()));
        for bad in [
            "http://github.com/a/b",
            "https://gitlab.com/a/b",
            "git@github.com:a/b",
            "a/b@../x",
            "a/b@-x",
            "a/b/c",
            "a",
            "-a/b",
            "a/b@x y",
            "a/..",
        ] {
            assert!(parse(bad, h, b).is_err(), "{bad}");
        }
    }

    // S-68: an archive is read whole in memory: the commit from its pax header, and a path
    // outside its folder, or an entry that is neither a file, a folder nor a link, refuses it all.
    #[test]
    fn an_archive_is_read_without_writing_and_refused_when_it_escapes() {
        let c = "a".repeat(40);
        let t = tar(
            "o-r-1",
            &c,
            &[
                ("skills/hello/SKILL.md", b'0', MD.as_bytes()),
                ("skills/hello/run.sh", b'0', b"#!/bin/sh\necho hi\n"),
                ("README.md", b'0', b"readme"),
                ("docs/big.bin", b'0', &[0u8; 2000]),
                ("CLAUDE.md", b'2', b""),
            ],
        );
        let (commit, tree) = untar(&t).unwrap();
        assert_eq!(commit, Some(c.clone()));
        assert!(tree.contains_key("skills/hello/run.sh") && tree.contains_key("README.md"));
        assert!(
            !tree.contains_key("docs/big.bin"),
            "only what can matter is kept"
        );
        assert_eq!(tree.get("CLAUDE.md"), Some(&Item::Link));
        for bad in [
            tar("o", &c, &[("../x", b'0', b"x")]),
            tar("o", &c, &[("a/../../x", b'0', b"x")]),
            tar("o", &c, &[("dev", b'3', b"")]),
        ] {
            assert!(untar(&bad).is_err());
        }
        let mut two = tar("o", &c, &[("a", b'0', b"x")]);
        let other = tar("p", &c, &[("b", b'0', b"y")]);
        two.truncate(two.len() - 1024);
        two.extend(&other[1024..]);
        assert!(untar(&two).unwrap_err().contains("outside its folder"));
    }

    // S-68: a skill's files are held to the added limits; a link in it refuses it; a binary is
    // skipped and said; hooks, commands and agents are named, never taken.
    #[test]
    fn skills_are_found_checked_and_only_skills_are_taken() {
        let c = "b".repeat(40);
        let big = "x".repeat(skills::MAX_ADDED_BYTES);
        let t = tar(
            "o",
            &c,
            &[
                ("skills/hello/SKILL.md", b'0', MD.as_bytes()),
                ("skills/hello/logo.png", b'0', &[0x89, 0x50, 0, 1]),
                ("skills/hello/.DS_Store", b'0', b"x"),
                ("hooks/hooks.json", b'0', b"{}"),
                ("commands/x.md", b'0', b"x"),
            ],
        );
        let (_, tree) = untar(&t).unwrap();
        let (dirs, layouts) = skill_dirs(&tree);
        assert_eq!(
            (dirs, layouts),
            (vec!["skills/hello".to_owned()], vec!["skills/*/SKILL.md"])
        );
        assert_eq!(plugin_parts(&tree), ["hooks", "commands"]);
        let name = name_in(&tree, "skills/hello", "o");
        assert_eq!(name, "hello-world");
        let f = build(&tree, "skills/hello", &name).unwrap();
        assert!(f.files.is_empty());
        assert_eq!(
            f.skipped,
            [
                ".DS_Store (a name Clio does not take)",
                "logo.png (not text)"
            ]
        );
        let mut many: Vec<(String, u8, Vec<u8>)> =
            vec![("s/SKILL.md".into(), b'0', MD.as_bytes().to_vec())];
        for (case, extra) in [
            ("link", vec![("s/x".to_owned(), b'2', vec![])]),
            (
                "large",
                vec![("s/big.md".to_owned(), b'0', big.into_bytes())],
            ),
            (
                "many",
                (0..skills::MAX_ADDED_FILES)
                    .map(|i| (format!("s/f{i}.md"), b'0', b"x".to_vec()))
                    .collect(),
            ),
        ] {
            many.truncate(1);
            many.extend(extra);
            let entries: Vec<(&str, u8, &[u8])> = many
                .iter()
                .map(|(p, k, b)| (format!("skills/{p}").leak() as &str, *k, b.as_slice()))
                .collect();
            let (_, tree) = untar(&tar("o", &c, &entries)).unwrap();
            assert!(build(&tree, "skills/s", "s").is_err(), "{case}");
        }
    }

    #[test]
    fn a_folder_is_read_without_following_links() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("skills/one");
        fs::create_dir_all(&s).unwrap();
        fs::write(s.join("SKILL.md"), MD).unwrap();
        fs::create_dir_all(t.path().join("elsewhere/deep")).unwrap();
        fs::write(t.path().join("elsewhere/deep/x"), "not read").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", s.join("pw")).unwrap();
        let tree = read_folder(t.path()).unwrap();
        assert_eq!(tree.get("skills/one/pw"), Some(&Item::Link));
        assert!(!tree.contains_key("elsewhere/deep/x"));
        assert!(
            build(&tree, "skills/one", "one")
                .unwrap_err()
                .contains("link")
        );
    }
}
