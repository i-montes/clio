//! Skills the person installs (design `2026-11-20-skills-add-design.md`, S-68): from a folder that
//! holds a skill or several, or from a GitHub repository's archive at a ref. Shared by the tool
//! host's `skill_add` (the model proposes; each skill waits for the person) and the person's
//! `clio skills add` (asked at the terminal). Nothing here writes the skills folder: it finds the
//! skills, checks them, has the judge read every file, and gives each one as the arguments of an
//! item of class `skill` (tool `skill_add`) that only the person's own yes registers: no
//! permission window and no bypass answers it (`grant`).
//!
//! - GitHub only over https (`codeload.github.com`), at a branch or a tag of that repository (or
//!   its default branch), never a commit hash: GitHub serves a fork's commits through the parent's
//!   name. The archive at most `MAX_ARCHIVE`, at most `MAX_UNPACKED` unpacked, read here in
//!   memory: nothing in it runs and nothing of it is written but the download itself, in a
//!   staging folder under Clio's data, removed at once. The commit is the archive's own.
//! - A folder only for the person (`clio skills add`): the model can write folders. It is walked
//!   from a handle on its top folder, no link followed even if one is swapped in while it is read.
//! - Every file goes through the vault's filter: a secret shape refuses the skill.
//! - A path outside the archive's folder, or an entry that is not a file, a folder or a link,
//!   refuses the whole archive. A link is never followed or kept: one inside a skill refuses it.
//! - In a skill, a file that is not text, whose name Clio does not take (`valid_path`) or that
//!   holds characters the box would not show as written is skipped, and the box says so; the rest
//!   is held to `MAX_ADDED_FILES` and `MAX_ADDED_BYTES` (`skills::check_added`).
//! - Only skills are taken: a plugin's hooks, commands and agents are named, never installed.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
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
/// Folders deep, at most, in a folder read.
const MAX_FOLDER_DEPTH: usize = 16;
/// The model's `skill_add` never takes a folder: fixed words.
pub const FOLDER_PERSON: &str = "a folder is added only by the person, with `clio skills add` at a terminal; skill_add takes a GitHub repository (owner/repo, owner/repo@branch-or-tag)";
const WHERE: &str = "a folder with SKILL.md, skills/*/SKILL.md, .claude/skills/*/SKILL.md or plugins/*/skills/*/SKILL.md";

/// Where skills come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Folder(PathBuf),
    GitHub {
        owner: String,
        repo: String,
        /// A branch or a tag; `HEAD` (the default branch) when none is given.
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
    /// Its bytes, and when it was last written (seconds since the epoch).
    File(Vec<u8>, i64),
    Dir,
    /// A link (or anything that is not a plain file or folder): never followed, never kept.
    Link,
}

type Tree = BTreeMap<String, Item>;

/// Folders a skill's tools leave behind, never part of a skill: skipped, never read, the same
/// in `clio skills add`, `skill_add` and the shared folder's look (S-74), with every name that
/// starts with a dot (`.git`, `.venv`).
const LEFT_BEHIND: [&str; 5] = [".git", "node_modules", "venv", ".venv", "__pycache__"];

/// A name skipped wherever it is under a skill's folder.
fn left_behind(part: &str) -> bool {
    part.starts_with('.') || LEFT_BEHIND.contains(&part)
}

/// Whether `rel` can matter: the top's own entries, and what is under the layouts' folders (or,
/// when the top is a skill itself, all of it but names that start with a dot); never a folder
/// `LEFT_BEHIND` names.
fn relevant(rel: &str, whole: bool) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.iter().any(|p| LEFT_BEHIND.contains(p)) {
        return false;
    }
    if whole {
        // A repository that is one skill: its plugin parts are named (the folder), never taken.
        let part = PLUGIN_PARTS.contains(&parts[0]);
        return !parts.iter().any(|p| p.starts_with('.')) && (!part || parts.len() == 1);
    }
    match parts.as_slice() {
        [_] | ["skills", ..] | [".claude", _] | [".claude", "skills", ..] => true,
        ["plugins", rest @ ..] => rest.len() <= 2 || rest[1] == "skills",
        _ => false,
    }
}

/// The relevant entries of folder `top`, walked from a handle on it (`openat`, `O_NOFOLLOW`): a
/// link is kept as one and never followed, even one swapped in for a folder or a file while it is
/// read (that open fails, and nothing is taken).
fn read_folder(top: &Path) -> Result<Tree, String> {
    use rustix::fs::{AtFlags, Mode, statat};
    let fd = rustix::fs::open(top, dir_flags(), Mode::empty())
        .map_err(|e| format!("{}: {e}", one_line(&top.display().to_string())))?;
    let whole = statat(&fd, "SKILL.md", AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|st| rustix::fs::FileType::from_raw_mode(st.st_mode).is_file());
    walk(&fd, whole, false)
}

/// Every entry under `fd` (`pi`: as Pi finds skills, every name but what `left_behind` names;
/// else `relevant`), no link followed.
fn walk(fd: &rustix::fd::OwnedFd, whole: bool, pi: bool) -> Result<Tree, String> {
    walk_within(fd, whole, pi, (MAX_ENTRIES, MAX_UNPACKED))
}

/// `walk`, refused before anything past `files` files or `bytes` bytes (by their size) is read.
fn walk_within(
    fd: &rustix::fd::OwnedFd,
    whole: bool,
    pi: bool,
    (files, bytes): (usize, usize),
) -> Result<Tree, String> {
    let mut w = Walk {
        tree: Tree::new(),
        bytes: 0,
        files: 0,
        most: (files, bytes),
        whole,
        pi,
    };
    w.dir(fd, "", 0)?;
    Ok(w.tree)
}

/// When a file was last written, in seconds since the epoch.
fn mtime(st: &rustix::fs::Stat) -> i64 {
    st.st_mtime
}

fn dir_flags() -> rustix::fs::OFlags {
    use rustix::fs::OFlags;
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

struct Walk {
    tree: Tree,
    bytes: usize,
    files: usize,
    /// Files and bytes at most.
    most: (usize, usize),
    whole: bool,
    pi: bool,
}

impl Walk {
    fn dir(&mut self, fd: &rustix::fd::OwnedFd, dir: &str, depth: usize) -> Result<(), String> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
        let changed = |rel: &str| {
            format!(
                "{}: it changed while it was read: nothing was taken",
                shown(rel)
            )
        };
        // Why it could not be read, as the system says it.
        let failed = |rel: &str, e: &dyn std::fmt::Display| {
            format!(
                "{}: it cannot be read ({e}): nothing was taken",
                if rel.is_empty() {
                    ".".into()
                } else {
                    shown(rel)
                }
            )
        };
        if depth > MAX_FOLDER_DEPTH {
            return Err(format!(
                "more than {MAX_FOLDER_DEPTH} folders deep: name a narrower folder"
            ));
        }
        let mut names = vec![];
        for e in rustix::fs::Dir::read_from(fd).map_err(|e| failed(dir, &e))? {
            let e = e.map_err(|e| failed(dir, &e))?;
            if let Ok(n) = e.file_name().to_str()
                && n != "."
                && n != ".."
            {
                names.push(n.to_owned());
            }
        }
        names.sort();
        for n in names {
            let rel = if dir.is_empty() {
                n.clone()
            } else {
                format!("{dir}/{n}")
            };
            let keep = if self.pi {
                !rel.split('/').any(left_behind)
            } else {
                relevant(&rel, self.whole)
            };
            if !keep {
                continue;
            }
            if self.tree.len() >= MAX_ENTRIES {
                return Err(format!(
                    "more than {MAX_ENTRIES} entries: name a narrower folder"
                ));
            }
            let st =
                statat(fd, n.as_str(), AtFlags::SYMLINK_NOFOLLOW).map_err(|e| failed(&rel, &e))?;
            let item = match FileType::from_raw_mode(st.st_mode) {
                FileType::Directory => {
                    let sub = openat(fd, n.as_str(), dir_flags(), Mode::empty()).map_err(|e| {
                        if e == rustix::io::Errno::LOOP || e == rustix::io::Errno::NOTDIR {
                            changed(&rel)
                        } else {
                            failed(&rel, &e)
                        }
                    })?;
                    self.tree.insert(rel.clone(), Item::Dir);
                    self.dir(&sub, &rel, depth + 1)?;
                    continue;
                }
                FileType::RegularFile => {
                    // NONBLOCK: a pipe swapped in does not hang the open; fstat says what it is.
                    let f = openat(
                        fd,
                        n.as_str(),
                        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                        Mode::empty(),
                    )
                    .map_err(|e| {
                        if e == rustix::io::Errno::LOOP {
                            changed(&rel)
                        } else {
                            failed(&rel, &e)
                        }
                    })?;
                    let st = rustix::fs::fstat(&f).map_err(|e| failed(&rel, &e))?;
                    if !FileType::from_raw_mode(st.st_mode).is_file() {
                        return Err(changed(&rel));
                    }
                    self.files += 1;
                    self.bytes = self
                        .bytes
                        .saturating_add(usize::try_from(st.st_size).unwrap_or(usize::MAX));
                    if self.files > self.most.0 || self.bytes > self.most.1 {
                        return Err(format!(
                            "more than {} files or {} KiB to read: nothing of it was read",
                            self.most.0,
                            self.most.1 >> 10
                        ));
                    }
                    let mut b = Vec::new();
                    fs::File::from(f)
                        .take(MAX_UNPACKED as u64)
                        .read_to_end(&mut b)
                        .map_err(|e| failed(&rel, &e))?;
                    Item::File(b, mtime(&st))
                }
                _ => Item::Link,
            };
            self.tree.insert(rel, item);
        }
        Ok(())
    }
}

/// A name from a source as an error may carry it (to the model too): plain characters, short.
fn shown(s: &str) -> String {
    let t: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
        .take(80)
        .collect();
    if t.is_empty() { "?".into() } else { t }
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
                _ => Item::File(tar[r].to_vec(), 0),
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

/// Downloads `url` into `file`: whether the server gave it (curl, https only, at most
/// `MAX_ARCHIVE`, the installer's flags).
pub type Get<'a> = &'a dyn Fn(&str, &Path) -> bool;

/// curl, as `Get`.
pub fn curl(url: &str, file: &Path) -> bool {
    Command::new("curl")
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
        .arg(file)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The archive URLs of `owner/repo` at `reference`, in order: a branch, then a tag of that
/// repository (`refs/heads/`, `refs/tags/`), so only a real ref of it resolves, never a commit
/// (GitHub serves a fork's commit through the parent's name); the default branch when none.
pub fn archive_urls(owner: &str, repo: &str, reference: &str) -> Vec<String> {
    let base = format!("https://codeload.github.com/{owner}/{repo}/tar.gz");
    if reference == "HEAD" {
        vec![format!("{base}/HEAD")]
    } else {
        vec![
            format!("{base}/refs/heads/{reference}"),
            format!("{base}/refs/tags/{reference}"),
        ]
    }
}

/// The archive of `owner/repo` at `reference` (`archive_urls`, through `get`), downloaded into
/// `staging` and removed there, unpacked.
fn fetch(
    owner: &str,
    repo: &str,
    reference: &str,
    staging: &Path,
    get: Get,
) -> Result<Vec<u8>, String> {
    crate::host::private_dir(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let file = staging.join(format!(
        "{}-{}.tar.gz",
        std::process::id(),
        crate::host::now_ms()
    ));
    let got = archive_urls(owner, repo, reference)
        .iter()
        .any(|u| get(u, &file));
    let hex = reference.len() >= 7 && reference.bytes().all(|b| b.is_ascii_hexdigit());
    let r = if !got && hex {
        Err(format!(
            "{reference} is not a branch or a tag of {owner}/{repo}: name a branch or a tag"
        ))
    } else if !got {
        Err(format!(
            "GitHub did not give the archive of {owner}/{repo} at {reference} (no such repository, branch or tag, larger than {} MiB, or no network)",
            MAX_ARCHIVE >> 20
        ))
    } else if !fs::metadata(&file).is_ok_and(|m| m.len() <= MAX_ARCHIVE) {
        Err(format!(
            "the archive is larger than {} MiB: nothing of it was taken",
            MAX_ARCHIVE >> 20
        ))
    } else {
        gunzip(&file)
    };
    let _ = fs::remove_file(&file);
    r
}

/// The folders of `tree` that hold a skill, and the layouts they were found in.
fn skill_dirs(tree: &Tree) -> (Vec<String>, Vec<&'static str>) {
    if matches!(tree.get("SKILL.md"), Some(Item::File(..))) {
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
        Some(Item::File(b, _)) => text(b).unwrap_or_default(),
        _ => String::new(),
    };
    let folder = if dir.is_empty() {
        top
    } else {
        dir.rsplit('/').next().unwrap_or(dir)
    };
    // Its own name as written (`check_added` refuses one that is not exactly Clio's), else the
    // folder's made Clio's.
    match skills::added_front_matter(&md).ok().and_then(|f| f.0) {
        Some(n) => n,
        None => skills::clio_name(folder),
    }
}

fn build(tree: &Tree, dir: &str, name: &str) -> Result<Found, String> {
    let who = |why: String| {
        format!(
            "the skill «{}» ({}): {why}",
            shown(name),
            if dir.is_empty() {
                ".".into()
            } else {
                shown(dir)
            }
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
                    "{} is a link: a skill with a link is not taken",
                    shown(rel)
                )));
            }
            Item::File(b, _) if rel == "SKILL.md" => {
                md = Some(text(b).ok_or_else(|| who("its SKILL.md is not text".into()))?);
            }
            Item::File(..) if skills::valid_path(rel).is_err() => {
                skipped.push(format!("{rel} (a name Clio does not take)"));
            }
            Item::File(b, _) => match text(b) {
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
    if !skills::valid_name(name) {
        return Err(who(
            "its name is not one Clio takes (lowercase letters, digits and single hyphens, at most 64)"
                .into(),
        ));
    }
    skills::check_added(name, &md, &files).map_err(who)?;
    let description = skills::added_front_matter(&md)
        .map_err(who)?
        .1
        .trim()
        .to_owned();
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
    /// The skills folder (`<data>/skills`): who holds each name.
    pub root: &'a Path,
    /// Where an archive is downloaded (under Clio's data); it is removed at once.
    pub staging: &'a Path,
    /// The judge's verdict on a text and where it is from; an error stops the add (`REFUSING`).
    pub judge: &'a dyn Fn(&str, &str) -> Result<TextVerdict, String>,
    /// How many skills one add may take (`skill_add`: as many as may wait for the person).
    pub most: Option<usize>,
    /// A text as the vault's filter leaves it (`None`: it could not filter it).
    pub clean: &'a dyn Fn(&str) -> Option<String>,
    /// How an archive is downloaded (`curl`).
    pub get: Get<'a>,
}

/// The most text the judge reads in one add: about 700,000 tokens of its daily 2,000,000, so an
/// add that has every window asked twice still leaves the day's other judging some.
pub const MAX_JUDGED: usize = 2 << 20;

/// Something shaped like a secret, the same check as `skill_write`'s: fixed words.
const SECRET: &str = "it holds something shaped like a secret (a key, a token, a password): a skill never carries one, nothing was added";

/// Every skill of `source` (only those in `names`, when some are given), found, checked and
/// judged whole: each one the arguments of its item, in the source's order. One that fails a
/// check refuses the whole add, saying which and why (the others can be named).
pub fn prepare(source: &str, names: &[String], cx: &Ctx) -> Result<Vec<Value>, String> {
    let (src, tree, commit, top) = match parse(source, cx.home, cx.base)? {
        Source::Folder(p) => {
            let p = p
                .canonicalize()
                .map_err(|e| format!("{}: {e}", one_line(&p.display().to_string())))?;
            if !p.is_dir() {
                return Err(format!("{} is not a folder", p.display()));
            }
            // In the shared folder, read as the look reads it (S-74): every skill as Pi finds it.
            let tree = if shared_dir(cx.home, &p).is_some() {
                let fd = rustix::fs::open(&p, dir_flags(), rustix::fs::Mode::empty())
                    .map_err(|e| format!("{}: {e}", one_line(&p.display().to_string())))?;
                walk(&fd, false, true)?
            } else {
                read_folder(&p)?
            };
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
            let tar = fetch(&owner, &repo, &reference, cx.staging, cx.get)?;
            let (commit, tree) = untar(&tar)?;
            // The yes is to that repository at that commit: an archive that does not say which
            // is not taken.
            if commit.is_none() {
                return Err("the archive does not name its commit: nothing of it was taken".into());
            }
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
    // A skill of the shared folder is recorded as the look records it, each by its own folder
    // (`~/.agents/skills/<dir>`), read as the look reads it (`sub_tree`).
    let shared = match &src {
        Source::Folder(p) => shared_dir(cx.home, p),
        Source::GitHub { .. } => None,
    };
    let origin = shared
        .as_deref()
        .map_or_else(|| src.origin(), |d| shared_origin(&shown(d)));
    let origin_of = |dir: &str| match shared.as_deref() {
        Some(base) => shared_origin(&shown(
            &[base, dir]
                .iter()
                .filter(|x| !x.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("/"),
        )),
        None => origin.clone(),
    };
    let (dirs, layouts) = if shared.is_some() {
        (pi_dirs(&tree), vec!["~/.agents/skills"])
    } else {
        skill_dirs(&tree)
    };
    if dirs.is_empty() {
        return Err(format!("no skill in {origin}: Clio looks for {WHERE}"));
    }
    let left = plugin_parts(&tree);
    let named: Vec<(String, &String)> = dirs.iter().map(|d| (name_in(&tree, d, &top), d)).collect();
    if let Some(n) = names.iter().find(|n| !named.iter().any(|(m, _)| m == *n)) {
        return Err(format!(
            "there is no skill «{}» in {origin}; it has: {}",
            shown(n),
            named
                .iter()
                .map(|(m, _)| shown(m))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let chosen: Vec<_> = named
        .into_iter()
        .filter(|(name, _)| names.is_empty() || names.contains(name))
        .collect();
    if let Some(most) = cx.most.filter(|m| chosen.len() > *m) {
        return Err(format!(
            "{origin} has {} skills: at most {most} are proposed at once; ask the person which ones they want and give them in names",
            chosen.len()
        ));
    }
    // Every skill found and checked before the judge reads any.
    let mut found = vec![];
    for (name, dir) in chosen {
        if found
            .iter()
            .any(|(f, ..): &(Found, bool, String)| f.name == name)
        {
            return Err(format!("{origin} has two skills named «{}»", shown(&name)));
        }
        let f = if shared.is_some() {
            build(&sub_tree(&tree, dir), "", &name).map(|f| Found {
                from: dir.clone(),
                ..f
            })?
        } else {
            build(&tree, dir, &name)?
        };
        no_secret(&f, cx)?;
        let update = update_of(cx.root, &f.name, &origin_of(dir)).map_err(|o| {
            format!(
                "the name «{}» is taken by {o}: a skill of another origin is not added under it (remove that one first, or name the others)",
                f.name
            )
        })?;
        let whole = skills::whole_text(&f.skill_md, &f.files);
        found.push((f, update, whole));
    }
    let bytes: usize = found.iter().map(|(.., w)| w.len()).sum();
    if bytes > MAX_JUDGED {
        return Err(format!(
            "these skills of {origin} hold {} KiB: the judge reads at most {} KiB in one add; name the skills wanted, a few at a time",
            bytes >> 10,
            MAX_JUDGED >> 10
        ));
    }
    let mut out: Vec<Value> = vec![];
    for (f, update, whole) in found {
        let verdict = (cx.judge)(&whole, &format!("skill «{}» from {origin}", f.name))?;
        // Where the person reads exactly what would be installed: the repository at that commit.
        let url = match (&src, &commit) {
            (Source::GitHub { owner, repo, .. }, Some(c)) => Some(format!(
                "https://github.com/{owner}/{repo}/tree/{c}{}",
                if f.from.is_empty() {
                    String::new()
                } else {
                    format!("/{}", f.from)
                }
            )),
            _ => None,
        };
        out.push(json!({
            "name": f.name,
            "description": f.description,
            "skill_md": f.skill_md,
            "files": f.files,
            "sha256": skills::digest(&f.skill_md, &f.files),
            "update": update,
            "origin": origin_of(&f.from),
            "commit": commit,
            "ref": match &src { Source::GitHub { reference, .. } => Some(reference.as_str()), Source::Folder(_) => None },
            "from": f.from,
            "url": url,
            "layout": layouts.join(", "),
            "judge": judge_word(verdict),
            "skipped": f.skipped,
            "left": left,
        }));
    }
    Ok(out)
}

/// Every text of `f` is as the vault's filter leaves it: a secret's shape refuses it.
fn no_secret(f: &Found, cx: &Ctx) -> Result<(), String> {
    for t in std::iter::once(f.skill_md.as_str()).chain(f.files.iter().map(|x| x.content.as_str()))
    {
        match (cx.clean)(t) {
            None => return Err("the vault could not filter the text: nothing was added".into()),
            Some(c) if c != t || clio_vault::count_masks(&c) > clio_vault::count_masks(t) => {
                return Err(format!("the skill «{}»: {SECRET}", shown(&f.name)));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Whether `name` is a new version of a skill of `origin` (`false`: nobody holds it). `Err`: who
/// holds it, another origin or Clio's own.
fn update_of(root: &Path, name: &str, origin: &str) -> Result<bool, String> {
    match owner(root, name) {
        None => Ok(false),
        Some(o) if o.as_deref() == Some(origin) => Ok(true),
        Some(o) => Err(o.unwrap_or_else(|| "a skill of Clio's own".into())),
    }
}

/// The folder other agents read their skills from (Agent Skills), under the home. Clio looks at
/// it when a person's chat opens (design `2026-11-20-shared-skills-design.md`, S-74).
pub const SHARED: &str = ".agents/skills";
/// What a skill of the shared folder is recorded as coming from: `~/.agents/skills/<dir>`
/// (the folder itself when `dir` is empty).
pub fn shared_origin(dir: &str) -> String {
    if dir.is_empty() {
        format!("~/{SHARED}")
    } else {
        format!("~/{SHARED}/{dir}")
    }
}

/// `p` (canonical) is the home's own `~/.agents/skills` or a folder under it (not through a
/// link): its path under it (empty: the folder itself).
fn shared_dir(home: &Path, p: &Path) -> Option<String> {
    let rest = p
        .strip_prefix(home.canonicalize().ok()?.join(SHARED))
        .ok()?;
    Some(rest.to_str()?.to_owned())
}

/// The skills of `tree` as Pi finds them: every folder with a `SKILL.md` (empty: the top), not
/// under another one.
fn pi_dirs(tree: &Tree) -> Vec<String> {
    let mut dirs: Vec<String> = tree
        .iter()
        .filter(|(_, v)| matches!(v, Item::File(..)))
        .filter_map(|(k, _)| {
            if k == "SKILL.md" {
                Some(String::new())
            } else {
                k.strip_suffix("/SKILL.md").map(str::to_owned)
            }
        })
        .collect();
    let all = dirs.clone();
    dirs.retain(|d| {
        !all.iter()
            .any(|t| t != d && (t.is_empty() || d.starts_with(&format!("{t}/"))))
    });
    dirs
}

/// What a look at the shared folder found.
#[derive(Debug, Default)]
pub struct Look {
    /// New or changed since the person's last yes, read clean by the judge: each the arguments
    /// of its item (as `prepare`'s), with `change` (what changed since that yes).
    pub ask: Vec<Value>,
    /// Not part of the yes, each one line with why: flagged, not judged, refused, not read yet,
    /// a link not followed.
    pub apart: Vec<String>,
    /// What is listed apart for what is there (`(origin, seen)`): the judge flagged it, or it is
    /// refused (its checks, a name another origin holds, a link): not said again for that.
    pub told: Vec<(String, String)>,
    /// Every skill folder there (`origin`), with its digest when it was read whole; one that
    /// could not be read, by its folder at the top (what is under it is there too).
    pub found: Vec<(String, Option<String>)>,
    /// What the judge did not answer for (`(origin, seen)`): not judged again today.
    pub silent: Vec<(String, String)>,
}

/// The digest and version of the last yes to `origin` in the record of `name`.
pub fn last_yes(root: &Path, name: &str, origin: &str) -> Option<(u64, String)> {
    let r = skills::record(root, name)?;
    r["versions"]
        .as_array()?
        .iter()
        .filter(|v| v["origin"].as_str() == Some(origin))
        .filter_map(|v| Some((v["n"].as_u64()?, v["sha256"].as_str()?.to_owned())))
        .max_by_key(|(n, _)| *n)
}

/// `~/.agents/skills` opened from the home, neither `.agents` nor `skills` followed if a link:
/// `None` when there is none.
fn open_shared(home: &Path) -> Result<Option<rustix::fd::OwnedFd>, String> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
    let mut fd = rustix::fs::open(
        home,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| format!("the home: {e}"))?;
    let mut at = String::from("~");
    for part in [".agents", "skills"] {
        at = format!("{at}/{part}");
        match statat(&fd, part, AtFlags::SYMLINK_NOFOLLOW) {
            Err(e) if e == rustix::io::Errno::NOENT => return Ok(None),
            Err(e) => return Err(format!("{at}: {e}")),
            Ok(st) => match FileType::from_raw_mode(st.st_mode) {
                FileType::Directory => {}
                FileType::Symlink => {
                    return Err(format!(
                        "{at} is a link: Clio does not follow it, so she loads nothing from it"
                    ));
                }
                _ => return Err(format!("{at} is not a folder")),
            },
        }
        fd = openat(&fd, part, dir_flags(), Mode::empty())
            .map_err(|_| format!("{at} changed while it was read: nothing was taken"))?;
    }
    Ok(Some(fd))
}

/// Lines of diff shown per skill, at most: the rest is counted.
const MAX_DIFF_LINES: usize = 60;

/// `new` against `old`, line by line, as `diff -u` shows it with two lines of context: `@@ -a +b
/// @@` where a part starts, then ` `, `-` and `+` lines. A middle too long to compare line by
/// line is shown as all of it removed, then all of it added.
fn line_diff(old: &str, new: &str) -> Vec<String> {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut ops: Vec<(char, &str)> = a[..pre].iter().map(|l| (' ', *l)).collect();
    let (n, m) = (am.len(), bm.len());
    if n.saturating_mul(m) <= 1_000_000 {
        // The longest common subsequence, from the end.
        let w = m + 1;
        let mut t = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * w + j] = if am[i] == bm[j] {
                    t[(i + 1) * w + j + 1] + 1
                } else {
                    t[(i + 1) * w + j].max(t[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                ops.push((' ', am[i]));
                (i, j) = (i + 1, j + 1);
            } else if i < n && (j == m || t[(i + 1) * w + j] >= t[i * w + j + 1]) {
                ops.push(('-', am[i]));
                i += 1;
            } else {
                ops.push(('+', bm[j]));
                j += 1;
            }
        }
    } else {
        ops.extend(am.iter().map(|l| ('-', *l)));
        ops.extend(bm.iter().map(|l| ('+', *l)));
    }
    ops.extend(a[a.len() - suf..].iter().map(|l| (' ', *l)));
    let near = |k: usize| {
        ops[k.saturating_sub(2)..(k + 3).min(ops.len())]
            .iter()
            .any(|(c, _)| *c != ' ')
    };
    let (mut out, mut at_old, mut at_new, mut open) = (vec![], 1, 1, false);
    for (k, (c, l)) in ops.iter().enumerate() {
        if near(k) {
            if !open {
                out.push(format!("@@ -{at_old} +{at_new} @@"));
                open = true;
            }
            out.push(format!("{c}{l}"));
        } else {
            open = false;
        }
        at_old += usize::from(*c != '+');
        at_new += usize::from(*c != '-');
    }
    out
}

/// What changed in `f` (read from `sub`, which says when each file was written) since version
/// `n` of `name` (its folder in the skills folder): files added, removed and changed, by name,
/// size and time written, then a line diff of each changed text file (`MAX_DIFF_LINES` in all).
fn change(root: &Path, name: &str, n: u64, f: &Found, sub: &Tree) -> Vec<String> {
    let dir = root.join(skills::VERSIONS).join(name).join(n.to_string());
    let old = match rustix::fs::open(&dir, dir_flags(), rustix::fs::Mode::empty())
        .map_err(|e| e.to_string())
        .and_then(|fd| walk(&fd, true, true))
    {
        Ok(t) => t,
        Err(_) => return vec!["changed (the files of your last yes could not be read)".into()],
    };
    let new: BTreeMap<&str, &str> = std::iter::once(("SKILL.md", f.skill_md.as_str()))
        .chain(
            f.files
                .iter()
                .map(|x| (x.path.as_str(), x.content.as_str())),
        )
        .collect();
    let written = |path: &str| match sub.get(path) {
        Some(Item::File(_, t)) => format!("written {}", clio_core::utc_stamp(*t)),
        _ => "written ?".into(),
    };
    let (mut out, mut diff) = (vec![], vec![]);
    for (path, text) in &new {
        match old.get(*path) {
            Some(Item::File(b, _)) if b == text.as_bytes() => {}
            Some(Item::File(b, _)) => {
                out.push(format!(
                    "changed: {path} ({} bytes, was {}; {})",
                    text.len(),
                    b.len(),
                    written(path)
                ));
                if let Some(was) = self::text(b) {
                    diff.push(format!("{path}:"));
                    diff.extend(line_diff(&was, text).into_iter().map(|l| format!("  {l}")));
                }
            }
            _ => out.push(format!(
                "added: {path} ({} bytes; {})",
                text.len(),
                written(path)
            )),
        }
    }
    for (path, item) in &old {
        if matches!(item, Item::File(..)) && !new.contains_key(path.as_str()) {
            out.push(format!("removed: {path}"));
        }
    }
    let more = diff.len().saturating_sub(MAX_DIFF_LINES);
    diff.truncate(MAX_DIFF_LINES);
    out.extend(diff);
    if more > 0 {
        out.push(format!("and {more} more lines"));
    }
    out
}

/// The skill in folder `dir` of `tree` (empty: the tree's top) as `clio skills add` reads that
/// folder: only what is relevant to a folder that is one skill, by its path under it.
fn sub_tree(tree: &Tree, dir: &str) -> Tree {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    tree.range(prefix.clone()..)
        .map_while(|(k, v)| Some((k.strip_prefix(&prefix)?, v)))
        .filter(|(rel, _)| relevant(rel, true))
        .map(|(rel, v)| (rel.to_owned(), v.clone()))
        .collect()
}

/// One value for what is in a skill's folder as read (every path, every file's bytes): the
/// content the person said no to, or was told of, is known again by it.
fn tree_digest(tree: &Tree) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (k, v) in tree {
        let body: &[u8] = match v {
            Item::File(b, _) => b,
            Item::Dir => b"\0dir",
            Item::Link => b"\0link",
        };
        for part in [k.as_bytes(), body] {
            h.update((part.len() as u64).to_le_bytes());
            h.update(part);
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// What a look already knows, from what it remembered and what waits.
pub struct Known<'a> {
    /// `(origin, seen)`: the person said no to it, or was told why it is not asked.
    pub quiet: &'a dyn Fn(&str, &str) -> bool,
    /// `(origin, seen)`: the judge did not answer for it today.
    pub silent: &'a dyn Fn(&str, &str) -> bool,
    /// A skill's whole text that waits for the person, judged clean already.
    pub waiting: &'a dyn Fn(&str) -> bool,
}

/// Looks at the shared folder (`~/.agents/skills`), each folder at its top on its own (one that
/// cannot be read is said, with why, and counts as there; it never stops the others), read with a
/// skill's limits before anything is read (`MAX_ADDED_FILES`, `MAX_ADDED_BYTES`, 16 deep; no link
/// followed; what `left_behind` names skipped). Every folder with a `SKILL.md` in it is a skill
/// as Pi finds them (not under another skill), read as `clio skills add <that folder>` reads it,
/// checked and compared by digest with the person's last yes to it. Unchanged, or `quiet` for
/// what is there, it is left alone; new or changed, it is asked (`ask`, each with `seen`: what is
/// there) once judged clean: by the judge (at most an add's `MAX_JUDGED` in all, and as many as
/// can wait; none once the judge refuses calls, nor what it did not answer for today), or before,
/// when it waits already (`waiting`, not counted). Else it is listed apart. Nothing is written. A
/// home without the folder: an empty look. `Err`: the folder could not be opened.
// shortcut: a folder at the top holding several skills is read with one skill's limits; read
// each nested skill on its own if groups in ~/.agents/skills turn out to be common.
pub fn look_shared(cx: &Ctx, known: &Known) -> Result<Look, String> {
    use rustix::fs::{AtFlags, FileType, Mode, openat, statat};
    let mut look = Look::default();
    let Some(fd) = open_shared(cx.home)? else {
        return Ok(look);
    };
    let mut tops = vec![];
    for e in rustix::fs::Dir::read_from(&fd).map_err(|e| format!("~/{SHARED}: {e}"))? {
        let e = e.map_err(|e| format!("~/{SHARED}: {e}"))?;
        if let Ok(n) = e.file_name().to_str()
            && n != "."
            && n != ".."
            && !left_behind(n)
        {
            tops.push(n.to_owned());
        }
    }
    tops.sort();
    let (mut judged, mut names) = (0usize, Vec::<String>::new());
    let mut refused: Option<String> = None;
    for top in tops {
        let origin = shared_origin(&shown(&top));
        let read = statat(&fd, top.as_str(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| format!("it cannot be read ({e})"))
            .and_then(|st| match FileType::from_raw_mode(st.st_mode) {
                FileType::Directory => Ok(Some(())),
                FileType::Symlink => Err("link".into()),
                _ => Ok(None),
            })
            .and_then(|dir| {
                dir.map(|()| {
                    openat(&fd, top.as_str(), dir_flags(), Mode::empty())
                        .map_err(|e| format!("it cannot be read ({e})"))
                        .and_then(|sub| {
                            walk_within(
                                &sub,
                                false,
                                true,
                                (skills::MAX_ADDED_FILES, skills::MAX_ADDED_BYTES),
                            )
                        })
                })
                .transpose()
            });
        let tree = match read {
            // Not a folder: no skill (a file there is no skill of Clio's).
            Ok(None) => continue,
            Ok(Some(t)) => t,
            Err(why) if why == "link" => {
                if !(known.quiet)(&origin, "link") {
                    look.told.push((origin.clone(), "link".into()));
                    look.apart.push(format!(
                        "{} is a link: Clio does not follow links there",
                        shared_origin(&shown(&top))
                    ));
                }
                continue;
            }
            // There, but not read: said once for that reason, and never taken as gone.
            Err(why) => {
                look.found.push((origin.clone(), None));
                let seen = format!("unread: {why}");
                if !(known.quiet)(&origin, &seen) {
                    look.told.push((origin.clone(), seen));
                    look.apart
                        .push(format!("{}: {why}", shared_origin(&shown(&top))));
                }
                continue;
            }
        };
        // Pi does not look under a skill's folder for another.
        let dirs = pi_dirs(&tree);
        let under = |k: &str| {
            dirs.iter()
                .any(|d| d.is_empty() || k.starts_with(&format!("{d}/")))
        };
        for (k, v) in &tree {
            let origin = shared_origin(&shown(&format!("{top}/{k}")));
            if *v == Item::Link && !under(k) && !(known.quiet)(&origin, "link") {
                look.told.push((origin, "link".into()));
                look.apart.push(format!(
                    "{} is a link: Clio does not follow links there",
                    shared_origin(&shown(&format!("{top}/{k}")))
                ));
            }
        }
        for dir in dirs {
            let path = if dir.is_empty() {
                top.clone()
            } else {
                format!("{top}/{dir}")
            };
            let origin = shared_origin(&shown(&path));
            let sub = sub_tree(&tree, &dir);
            let seen = tree_digest(&sub);
            if (known.quiet)(&origin, &seen) {
                look.found.push((origin, None));
                continue;
            }
            let told = |look: &mut Look, line: String| {
                look.told.push((origin.clone(), seen.clone()));
                look.apart.push(line);
            };
            let name = name_in(&sub, "", path.rsplit('/').next().unwrap_or(&path));
            let f = match build(&sub, "", &name).and_then(|f| no_secret(&f, cx).map(|()| f)) {
                Ok(f) => f,
                Err(why) => {
                    look.found.push((origin.clone(), None));
                    told(&mut look, format!("{origin}: {why}"));
                    continue;
                }
            };
            let digest = skills::digest(&f.skill_md, &f.files);
            look.found.push((origin.clone(), Some(digest.clone())));
            if names.contains(&f.name) {
                look.apart.push(format!(
                    "{origin}: another folder there has a skill named «{}» too: neither is asked",
                    f.name
                ));
                look.ask.retain(|a| a["name"] != f.name.as_str());
                continue;
            }
            names.push(f.name.clone());
            let update = match update_of(cx.root, &f.name, &origin) {
                Ok(u) => u,
                // Said once while that holder keeps it; once it is gone, it is asked.
                Err(holder) => {
                    let key = format!("taken by {holder}: {seen}");
                    if !(known.quiet)(&origin, &key) {
                        look.told.push((origin.clone(), key));
                        look.apart.push(format!(
                            "{origin}: the name «{}» is taken by {holder}, which keeps it",
                            f.name
                        ));
                    }
                    continue;
                }
            };
            let last = last_yes(cx.root, &f.name, &origin);
            if last.as_ref().is_some_and(|(_, d)| *d == digest) {
                continue;
            }
            let by_name = format!("`clio skills add {origin}` adds it on its own");
            let whole = skills::whole_text(&f.skill_md, &f.files);
            // What waits was judged clean already: not judged again, not counted.
            if !(known.waiting)(&whole) {
                if (known.silent)(&origin, &seen) {
                    look.apart.push(format!(
                        "{origin}: not judged (the judge did not answer for it today), so it is not asked; {by_name}"
                    ));
                    continue;
                }
                if let Some(why) = &refused {
                    look.apart
                        .push(format!("{origin}: not read yet ({why}): {by_name}"));
                    continue;
                }
                if judged + whole.len() > MAX_JUDGED
                    || look.ask.len() >= crate::pending::MAX_WAITING
                {
                    look.apart
                        .push(format!("{origin}: not read yet: {by_name}"));
                    continue;
                }
                judged += whole.len();
                match (cx.judge)(&whole, &format!("skill «{}» from {origin}", f.name)) {
                    Ok(TextVerdict::Clean) => {}
                    // The judge takes no call now: this one and the rest wait for the next look.
                    Err(why) => {
                        look.apart
                            .push(format!("{origin}: not read yet ({why}): {by_name}"));
                        refused = Some(why);
                        continue;
                    }
                    Ok(TextVerdict::Injection) => {
                        told(
                            &mut look,
                            format!(
                                "{origin}: the judge found text in it that tries to give Clio orders, so it is not asked; {by_name} if you know why that text is there"
                            ),
                        );
                        continue;
                    }
                    Ok(v) => {
                        if v == TextVerdict::NoAnswer {
                            look.silent.push((origin.clone(), seen.clone()));
                        }
                        look.apart.push(format!(
                            "{origin}: not judged ({}), so it is not asked; {by_name}",
                            if v == TextVerdict::NoJudge {
                                "the judge is off"
                            } else {
                                "the judge did not answer"
                            }
                        ));
                        continue;
                    }
                }
            }
            let newest = sub
                .values()
                .filter_map(|v| match v {
                    Item::File(_, t) => Some(*t),
                    _ => None,
                })
                .max();
            let (change, written) = match last {
                Some((n, _)) => (change(cx.root, &f.name, n, &f, &sub), String::new()),
                None => (vec![], newest.map(clio_core::utc_stamp).unwrap_or_default()),
            };
            look.ask.push(json!({
                "name": f.name,
                "description": f.description,
                "skill_md": f.skill_md,
                "files": f.files,
                "sha256": digest,
                "update": update,
                "origin": origin,
                "commit": null,
                "ref": null,
                "from": "",
                "url": null,
                "layout": format!("~/{SHARED}"),
                "judge": judge_word(TextVerdict::Clean),
                "skipped": f.skipped,
                "left": plugin_parts(&sub),
                "change": change,
                "written": written,
                "seen": seen,
            }));
        }
    }
    Ok(look)
}

/// The model's reason, at most.
const MAX_REASON: usize = 300;

impl crate::ToolHost {
    /// `skill_add {source, names?, reason}`: finds, checks and judges the skills of `source` and
    /// sets each aside for the person (class `skill`, tool `skill_add`). Nothing is registered:
    /// only the person's own yes does that, never a permission window or bypass. The guard waits
    /// for it with no limit: once `cancel` says the caller went away, no skill is judged further
    /// and nothing is set aside.
    pub(crate) fn skill_add(
        &self,
        req: &crate::Request,
        args: &serde_json::Map<String, Value>,
        cancel: &dyn Fn() -> bool,
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
        // A folder is something the model can write: only the person adds one (`clio skills add`).
        if !matches!(parse(s("source"), home, home), Ok(Source::GitHub { .. })) {
            return fail(match parse(s("source"), home, home) {
                Ok(_) => FOLDER_PERSON,
                Err(_) => {
                    "source: a GitHub repository (owner/repo, owner/repo@branch-or-tag, or https://github.com/owner/repo); only GitHub, only over https"
                }
            });
        }
        // `verdict`, not `verdict_of`: a port without a judge is "not judged" here, never clean.
        let judge = |t: &str, from: &str| match &self.memory {
            _ if cancel() => Ok(TextVerdict::NoAnswer),
            Some(port) => crate::judge::verdict(port, t, from, cancel, &|_, _| {}),
            None => Ok(TextVerdict::NoJudge),
        };
        let staging = self.run_dir.join("skills/staging");
        let clean = |t: &str| self.try_clean(t);
        let cx = Ctx {
            home,
            base: home,
            root,
            staging: &staging,
            judge: &judge,
            most: Some(crate::pending::MAX_WAITING),
            clean: &clean,
            get: &self.archive_get,
        };
        let items = match prepare(s("source"), &names, &cx) {
            Ok(_) if cancel() => return fail("the call was stopped before every skill was judged"),
            Ok(i) => i,
            Err(why) => return fail(&why),
        };
        let origin = items
            .first()
            .and_then(|a| a["origin"].as_str())
            .unwrap_or("")
            .to_owned();
        // The person is told when this conversation read text from outside before proposing it.
        let marked = self.is_tainted(&req.session) || self.is_unjudged(&req.session);
        let mut set = vec![];
        for mut a in items {
            a["why"] = Value::String(self.clean(reason));
            a["marked"] = Value::Bool(marked);
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
        "---\nname: hello-world\ndescription: Says hello. Use when greeting.\n---\n\nSay hello.\n";

    fn gz(b: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut c = Command::new("gzip")
            .arg("-c")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(b).unwrap();
        c.wait_with_output().unwrap().stdout
    }

    // S-68: a ref is a branch or a tag of that repository, asked for as one (`refs/heads/`, then
    // `refs/tags/`), never a commit: GitHub serves a fork's commit through the parent's name. The
    // commit kept is the archive's own.
    #[test]
    fn a_ref_is_a_branch_or_a_tag_never_a_commit() {
        let c = "c".repeat(40);
        let body = gz(&tar(
            "r-x",
            &c,
            &[("skills/hello/SKILL.md", b'0', MD.as_bytes())],
        ));
        let base = "https://codeload.github.com/o/r/tar.gz";
        // The fake server: a branch `main`, a tag `v1`, the default branch; and a commit any
        // name would give (a fork's), which a well-formed request never asks for.
        let served = [
            format!("{base}/refs/heads/main"),
            format!("{base}/refs/tags/v1"),
            format!("{base}/HEAD"),
            format!("{base}/6e3b636d0000"),
        ];
        let asked = std::sync::Mutex::new(vec![]);
        let get = |url: &str, file: &Path| {
            asked.lock().unwrap().push(url.to_owned());
            served.contains(&url.to_owned()) && fs::write(file, &body).is_ok()
        };
        let t = tempfile::tempdir().unwrap();
        for r in ["main", "v1", "HEAD"] {
            let (commit, tree) = untar(&fetch("o", "r", r, t.path(), &get).unwrap()).unwrap();
            assert_eq!(commit.as_deref(), Some(c.as_str()));
            assert!(tree.contains_key("skills/hello/SKILL.md"), "{r}");
        }
        assert_eq!(
            asked.lock().unwrap().as_slice(),
            [
                format!("{base}/refs/heads/main"),
                format!("{base}/refs/heads/v1"),
                format!("{base}/refs/tags/v1"),
                format!("{base}/HEAD"),
            ]
        );
        let e = fetch("o", "r", "6e3b636d0000", t.path(), &get).unwrap_err();
        assert!(e.contains("name a branch or a tag"), "{e}");
        assert!(
            fetch("o", "r", "nope", t.path(), &get)
                .unwrap_err()
                .contains("no such")
        );
        assert!(
            !asked
                .lock()
                .unwrap()
                .contains(&format!("{base}/6e3b636d0000"))
        );
        assert_eq!(
            fs::read_dir(t.path()).unwrap().count(),
            0,
            "the download is removed"
        );
    }

    // S-68: a repository that is one skill (SKILL.md at its top): its hooks, commands and agents
    // are named as not installed, never taken as that skill's files.
    #[test]
    fn a_repository_that_is_one_skill_leaves_its_plugin_parts_out() {
        let c = "d".repeat(40);
        let t = tar(
            "o",
            &c,
            &[
                ("SKILL.md", b'0', MD.as_bytes()),
                ("ref.md", b'0', b"more"),
                ("hooks/hooks.json", b'0', b"{}"),
                ("commands/x.md", b'0', b"x"),
                ("agents/a.md", b'0', b"a"),
            ],
        );
        let (_, tree) = untar(&t).unwrap();
        let f = build(&tree, "", "hello-world").unwrap();
        let paths: Vec<&str> = f.files.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, ["ref.md"]);
        assert_eq!(plugin_parts(&tree), ["hooks", "commands", "agents"]);
    }

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
        // A folder that is a link (what a swap mid-read would leave): kept as a link, not entered.
        std::os::unix::fs::symlink(t.path().join("elsewhere"), t.path().join("skills/two"))
            .unwrap();
        let tree = read_folder(t.path()).unwrap();
        assert_eq!(tree.get("skills/one/pw"), Some(&Item::Link));
        assert_eq!(tree.get("skills/two"), Some(&Item::Link));
        assert!(!tree.keys().any(|k| k.starts_with("skills/two/")));
        assert!(!tree.contains_key("elsewhere/deep/x"));
        assert!(
            build(&tree, "skills/one", "one")
                .unwrap_err()
                .contains("link")
        );
    }
}
