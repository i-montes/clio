//! Deleting Pi sessions, for the person: nothing is removed for good. The conversation file in Pi's
//! sessions folder and, when the session is one of Clio's, its working copy (`runs/<id>`, as
//! `<entry>/run`) and the copies of the conversation Claude Code kept (the claude connector, in
//! Pi's HOME, as `<entry>/claude/<name>`) are renamed into one trash entry,
//! `<data>/trash/sessions/<unix-ms>-<id>/`, for 30 days; `clio sessions restore` puts each piece
//! back where it was. The memory is not touched. A copy with changes not applied to the person's
//! folder is said, so it is never lost by surprise.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use clio_brain::pi::launch::valid_id;
use clio_brain::pi::sessions::{SessionInfo, read_session};
use clio_core::Clock;

use super::chat::local::OPEN_ELSEWHERE;
use super::home::Dirs;
use super::runs::Run;
use super::tui::sanitize;
use super::work;
use super::{Res, io_err, mkdir_private};

/// What deleting one session removes.
pub struct Plan {
    pub file: PathBuf,
    /// Where the conversation goes (`<data>/trash/sessions`), and the id its folder is named with.
    pub trash: PathBuf,
    pub id: String,
    /// The run folder's lock (`session.lock`): a conversation open in another `clio` holds it.
    pub lock: Option<PathBuf>,
    pub title: String,
    /// Clio's working copy of the session, when it has one.
    pub run: Option<Run>,
    /// Files changed in the copy and not applied to the person's folder (`None`: unknown).
    pub unapplied: Option<usize>,
    /// Claude Code's copies of the conversation (the claude connector), in Pi's HOME.
    pub claude: Vec<PathBuf>,
}

/// The plan for `s`, or why it cannot be deleted: only a regular `.jsonl` file directly inside
/// Pi's sessions folder (never a link, never anything outside it).
pub fn plan(dirs: &Dirs, sessions_dir: &Path, s: &SessionInfo) -> Res<Plan> {
    let meta = std::fs::symlink_metadata(&s.path).map_err(|e| e.to_string())?;
    let inside = s
        .path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .zip(sessions_dir.canonicalize().ok())
        .is_some_and(|(p, d)| p == d);
    if !meta.is_file() || !inside || s.path.extension().is_none_or(|x| x != "jsonl") {
        return Err("only session files in Pi's sessions folder are deleted".into());
    }
    let run = clio_copy(dirs, s);
    let unapplied = run.as_ref().map_or(Some(0), unapplied_changes);
    // Only for a Clio copy: its folder is the session's alone, so its Claude Code folder is too.
    let claude = match (&run, &s.cwd_path) {
        (Some(_), Some(cwd)) => claude_copies(&super::pi::paths(dirs).home_dir(), cwd),
        _ => vec![],
    };
    Ok(Plan {
        file: s.path.clone(),
        trash: trash_dir(dirs),
        id: entry_id(&s.id, &s.path),
        lock: valid_id(&s.id).then(|| dirs.runs.join(&s.id).join("session.lock")),
        title: s.title.clone(),
        run,
        unapplied,
        claude,
    })
}

/// Claude Code's folder for a conversation held in `cwd`: `<HOME>/.claude/projects/<name>`, the
/// real path with every UTF-16 unit but `[A-Za-z0-9]` as `-` (how Claude Code and cc-session-io,
/// which the bridge uses, name it). Past 200 characters both add a hash, each its own: every
/// folder that starts with the first 200 is taken.
/// shortcut: two copies whose real paths agree in their first 200 characters (a data folder path
/// of over ~150) would share that prefix; compute the hash if that ever matters.
fn claude_copies(home: &Path, cwd: &Path) -> Vec<PathBuf> {
    let Ok(real) = cwd.canonicalize() else {
        return vec![];
    };
    let name: String = real
        .to_string_lossy()
        .encode_utf16()
        .map(|u| match char::from_u32(u.into()) {
            Some(c) if c.is_ascii_alphanumeric() => c,
            _ => '-',
        })
        .collect();
    let projects = home.join(".claude/projects");
    if name.len() <= 200 {
        let p = projects.join(name);
        return if p.exists() { vec![p] } else { vec![] };
    }
    let head = format!("{}-", &name[..200]);
    std::fs::read_dir(&projects)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(&head))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

/// The run whose copy is the session's folder (the same test the chat uses to open it).
fn clio_copy(dirs: &Dirs, s: &SessionInfo) -> Option<Run> {
    if !valid_id(&s.id) {
        return None;
    }
    let path = dirs.runs.join(&s.id);
    let (runs, want) = (
        dirs.runs.canonicalize().ok()?,
        path.join("sandbox").canonicalize().ok()?,
    );
    let have = s.cwd_path.as_ref()?.canonicalize().ok()?;
    (want.starts_with(&runs) && have == want).then(|| Run {
        id: s.id.clone(),
        path,
    })
}

/// Changed files of the copy not applied yet; 0 once applied; `None` when it cannot be told.
pub(super) fn unapplied_changes(run: &Run) -> Option<usize> {
    if run.path.join("applied.json").exists() {
        return Some(0);
    }
    let manifest = run.manifest().ok()?;
    work::changes(&run.sandbox(), &manifest)
        .ok()
        .map(|c| c.len())
}

/// How long a deleted conversation waits in the trash.
const TRASH_MS: i64 = 30 * 86_400_000;

pub fn trash_dir(dirs: &Dirs) -> PathBuf {
    dirs.data.join("trash/sessions")
}

/// `<digits>-<session id>`: the only names in the trash that Clio ever touches.
fn entry_name(name: &str) -> Option<(i64, &str)> {
    let (ms, id) = name.split_once('-')?;
    (ms.bytes().all(|b| b.is_ascii_digit()) && valid_id(id)).then_some((ms.parse().ok()?, id))
}

/// What is safe in a folder name, at most 128 characters.
fn clean_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c))
        .take(128)
        .collect()
}

/// The id a trash folder is named with: the header's (untrusted) id cleaned, else the file's
/// cleaned name, else `session`.
fn entry_id(id: &str, file: &Path) -> String {
    [id, file.file_stem().and_then(|s| s.to_str()).unwrap_or("")]
        .into_iter()
        .map(clean_id)
        .find(|c| !c.is_empty())
        .unwrap_or_else(|| "session".into())
}

/// A rename that never replaces what is at `to` (the kernel's `RENAME_NOREPLACE`).
fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use rustix::{fs::CWD, fs::RenameFlags, io::Errno};
    match rustix::fs::renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        // A filesystem without the flag: the plain rename after looking.
        Err(Errno::INVAL | Errno::NOSYS) => {
            if std::fs::symlink_metadata(to).is_ok() {
                return Err(ErrorKind::AlreadyExists.into());
            }
            std::fs::rename(from, to)
        }
        Err(e) => Err(e.into()),
    }
}

/// Opens a file without following a link.
fn open_nofollow(p: &Path, write: bool) -> std::io::Result<File> {
    OpenOptions::new()
        .read(!write)
        .write(write)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
        .open(p)
}

/// 0600 on a regular file, through the open file: a link put in its place is never followed.
fn make_private(p: &Path) -> Res<()> {
    let f = open_nofollow(p, false).map_err(|e| io_err(p, &e))?;
    if !f.metadata().is_ok_and(|m| m.is_file()) {
        return Err(format!("{} is not a regular file", p.display()));
    }
    f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .map_err(|e| io_err(p, &e))
}

/// Moves a file without ever following a link, and never over another file: a rename, and only
/// when that fails for being another filesystem, a copy (born 0600) and then the removal.
fn move_file(from: &Path, to: &Path) -> Res<()> {
    let e = match rename_noreplace(from, to) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == ErrorKind::CrossesDevices => e,
        Err(e) => return Err(format!("{} to {}: {e}", from.display(), to.display())),
    };
    let fail = |e: std::io::Error| format!("{} to {}: {e}", from.display(), to.display());
    let mut src = open_nofollow(from, false).map_err(fail)?;
    if !src.metadata().is_ok_and(|m| m.is_file()) {
        return Err(fail(e));
    }
    // Created here or not at all: what is already at `to` is never touched.
    let mut dst = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)
        .map_err(fail)?;
    if let Err(e) = std::io::copy(&mut src, &mut dst).and_then(|_| dst.sync_all()) {
        let _ = std::fs::remove_file(to);
        return Err(fail(e));
    }
    std::fs::remove_file(from).map_err(|e| io_err(from, &e))
}

/// A file with `move_file`, a folder (a run, Claude Code's copy) by a rename alone.
fn move_any(from: &Path, to: &Path) -> Res<()> {
    if std::fs::symlink_metadata(from).is_ok_and(|m| m.is_file()) {
        return move_file(from, to);
    }
    rename_noreplace(from, to).map_err(|e| format!("{} to {}: {e}", from.display(), to.display()))
}

/// Moves each piece (`from`, `to`); if one fails, the ones already moved go back and the error
/// is returned: all or nothing.
fn move_all(pieces: &[(PathBuf, PathBuf)]) -> Res<()> {
    for (i, (from, to)) in pieces.iter().enumerate() {
        let r = match to.parent() {
            Some(d) if !d.exists() => mkdir_private(d),
            _ => Ok(()),
        }
        .and_then(|()| move_any(from, to));
        if let Err(e) = r {
            for (f, t) in pieces[..i].iter().rev() {
                let _ = move_any(t, f);
            }
            return Err(e);
        }
    }
    Ok(())
}

/// Takes the run folder's lock without waiting; held until the file is dropped. `None` when
/// there is no lock file (nobody has opened that conversation).
fn hold_lock(lock: &Path) -> Res<Option<File>> {
    let f = match open_nofollow(lock, true) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(lock, &e)),
    };
    rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| OPEN_ELSEWHERE.to_owned())?;
    Ok(Some(f))
}

/// Moves the session into the trash: its file first, then its working copy and Claude Code's;
/// if any of them cannot move, the others go back and nothing is deleted. Refused while the
/// conversation is open in another `clio`.
pub fn delete(p: &Plan) -> Res<()> {
    let _lock = p.lock.as_deref().map(hold_lock).transpose()?;
    let name = p.file.file_name().ok_or("the session has no file name")?;
    mkdir_private(&p.trash)?;
    let dir = p.trash.join(format!("{}-{}", super::now_ms(), p.id));
    DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| io_err(&dir, &e))?;
    let file = dir.join(name);
    if let Err(e) = move_file(&p.file, &file) {
        let _ = std::fs::remove_dir(&dir);
        return Err(e);
    }
    // Pi can write the sessions folder: if a link took the file's place it is in the trash now,
    // and nothing is chmoded through it.
    make_private(&file)?;
    let mut rest: Vec<(PathBuf, PathBuf)> = p
        .run
        .iter()
        .map(|r| (r.path.clone(), dir.join("run")))
        .collect();
    for d in &p.claude {
        rest.push((
            d.clone(),
            dir.join("claude").join(d.file_name().unwrap_or_default()),
        ));
    }
    if let Err(e) = move_all(&rest) {
        let _ = move_file(&file, &p.file);
        let _ = std::fs::remove_dir(dir.join("claude"));
        let _ = std::fs::remove_dir(&dir);
        return Err(e);
    }
    Ok(())
}

/// Removes what has been in the trash more than 30 days. Only a real folder named
/// `<digits>-<session id>` directly in `root`; anything else (and a link) is left alone.
pub fn purge_trash(root: &Path, clock: &dyn Clock) {
    if !std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        return;
    }
    let cutoff = clock.now_ms() - TRASH_MS;
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let old = e
            .file_name()
            .to_str()
            .and_then(entry_name)
            .is_some_and(|(ms, _)| ms < cutoff);
        if old && e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A conversation in the trash.
#[derive(Debug)]
struct Trashed {
    dir: PathBuf,
    name: String,
    id: String,
    at_ms: i64,
    file: PathBuf,
    info: Option<SessionInfo>,
}

/// The trash, newest first: every well-named folder holding a regular `.jsonl` file.
fn trashed(root: &Path) -> Vec<Trashed> {
    let mut v: Vec<Trashed> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let (at_ms, id) = entry_name(&name)?;
            if !e.file_type().ok()?.is_dir() {
                return None;
            }
            let file = std::fs::read_dir(e.path())
                .ok()?
                .flatten()
                .find(|f| {
                    f.path().extension().is_some_and(|x| x == "jsonl")
                        && f.file_type().is_ok_and(|t| t.is_file())
                })?
                .path();
            Some(Trashed {
                info: read_session(&file).ok(),
                id: id.to_owned(),
                dir: e.path(),
                name,
                at_ms,
                file,
            })
        })
        .collect();
    v.sort_by_key(|t| std::cmp::Reverse(t.at_ms));
    v
}

/// The usage of `clio sessions`.
const USAGE: &str = "usage: clio sessions restore [id]
  restore           lists the conversations in the trash (deleted in the last 30 days), with the
                    date deleted (local time), the messages, the title and the entry name
  restore <id>      brings one back, whole: its conversation to the sessions folder and, if it
                    had them, its working copy and Claude Code's copies where they were. <id> is
                    the entry name, the id or the start of the id. It refuses, and restores
                    nothing, if any of the places is taken";

/// The entry `want` names: the entry name, else the id, else the start of an id; one only.
fn pick<'a>(all: &'a [Trashed], want: &str) -> Res<&'a Trashed> {
    if want.is_empty() {
        return Err("give the id (or the entry name) of what to restore".into());
    }
    let tiers: [Vec<&Trashed>; 3] = [
        all.iter().filter(|t| t.name == want).collect(),
        all.iter().filter(|t| t.id == want).collect(),
        all.iter().filter(|t| t.id.starts_with(want)).collect(),
    ];
    let Some(hits) = tiers.into_iter().find(|h| !h.is_empty()) else {
        return Err(format!(
            "nothing in the trash starts with {:?} (see `clio sessions restore`)",
            sanitize::line(want)
        ));
    };
    match hits.as_slice() {
        [one] => Ok(one),
        _ => Err(format!(
            "{:?} fits more than one entry in the trash: give more of the id, or the whole entry name",
            sanitize::line(want)
        )),
    }
}

/// `clio sessions restore [id]`.
pub fn run(dirs: &Dirs, argv: &[String]) -> Res<i32> {
    match argv {
        [c] | [c, _] if c == "restore" => {}
        [] => {
            println!("{USAGE}");
            return Ok(0);
        }
        _ => return Err(USAGE.into()),
    }
    let all = trashed(&trash_dir(dirs));
    let Some(want) = argv.get(1) else {
        if all.is_empty() {
            println!("the trash is empty");
            return Ok(0);
        }
        println!("deleted (local time)  messages  title  entry");
        let off = super::pending::local_offset() * 1000;
        for t in &all {
            let (n, title) = t
                .info
                .as_ref()
                .map_or((0, "(unreadable)"), |i| (i.messages, &i.title));
            println!(
                "{}  {n:>8}  {}  {}",
                super::stamp(t.at_ms + off),
                sanitize::line(title),
                t.name
            );
        }
        return Ok(0);
    };
    let t = pick(&all, want)?;
    let sessions = super::pi::paths(dirs).sessions_dir();
    let file_name = t.file.file_name().ok_or("no file name")?;
    // Each piece back where it was; the folders of the entry say what it holds.
    let mut pieces = vec![(t.file.clone(), sessions.join(file_name))];
    if std::fs::symlink_metadata(t.dir.join("run")).is_ok_and(|m| m.is_dir()) {
        pieces.push((t.dir.join("run"), dirs.runs.join(&t.id)));
    }
    let projects = super::pi::paths(dirs).home_dir().join(".claude/projects");
    for e in std::fs::read_dir(t.dir.join("claude"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if e.file_type().is_ok_and(|f| f.is_dir()) {
            pieces.push((e.path(), projects.join(e.file_name())));
        }
    }
    if let Some((_, taken)) = pieces
        .iter()
        .find(|(_, to)| std::fs::symlink_metadata(to).is_ok())
    {
        return Err(format!(
            "{} is already there: nothing was restored",
            taken.display()
        ));
    }
    if !sessions.exists() {
        mkdir_private(&sessions)?;
    }
    make_private(&t.file)?;
    move_all(&pieces)?;
    let _ = std::fs::remove_dir(t.dir.join("claude"));
    let _ = std::fs::remove_dir(&t.dir);
    println!(
        "restored {} to {}",
        t.name,
        sanitize::line(&pieces[0].1.display().to_string())
    );
    Ok(0)
}

/// One line for the person about what a plan removes.
pub fn describe(p: &Plan) -> String {
    match (&p.run, p.unapplied) {
        (None, _) => "the conversation only".into(),
        (Some(_), Some(0)) => "the conversation and its working copy".into(),
        (Some(_), Some(n)) => format!(
            "the conversation and its working copy, with {n} changed file(s) NOT APPLIED to your folder"
        ),
        (Some(_), None) => {
            "the conversation and its working copy (it could not be told whether it has changes not applied)"
                .into()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn dirs_in(root: &Path) -> Dirs {
        let data = root.join("data");
        Dirs {
            config: root.join("config"),
            runs: data.join("runs"),
            data,
            runtime: None,
            home: root.to_path_buf(),
        }
    }

    fn info(path: PathBuf, id: &str, cwd: Option<PathBuf>) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            path,
            cwd: String::new(),
            cwd_path: cwd,
            created_ms: None,
            last_activity_ms: 0,
            name: None,
            title: "t".into(),
            messages: 0,
            approx: false,
            parent: None,
        }
    }

    #[test]
    fn a_session_of_clio_takes_its_copy_and_one_from_elsewhere_only_its_file() {
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let sessions = t.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::create_dir_all(dirs.runs.join("r1/sandbox")).unwrap();
        std::fs::write(dirs.runs.join("r1/manifest.json"), "{}").unwrap();
        std::fs::write(dirs.runs.join("r1/sandbox/nuevo.txt"), "x").unwrap();
        let ours = sessions.join("a.jsonl");
        std::fs::write(&ours, "{}").unwrap();
        // What the bridge's Claude Code left for this copy, and for another folder.
        let projects = t.path().join("data/pi/home/.claude/projects");
        let slug: String = dirs
            .runs
            .join("r1/sandbox")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        std::fs::create_dir_all(projects.join(&slug)).unwrap();
        std::fs::write(projects.join(&slug).join("cc.jsonl"), "{}").unwrap();
        std::fs::create_dir_all(projects.join("-otra")).unwrap();
        let p = plan(
            &dirs,
            &sessions,
            &info(ours.clone(), "r1", Some(dirs.runs.join("r1/sandbox"))),
        )
        .unwrap();
        assert_eq!(p.unapplied, Some(1), "a new file in the copy, not applied");
        assert!(describe(&p).contains("NOT APPLIED"));
        assert_eq!(p.claude, [projects.join(&slug)]);
        delete(&p).unwrap();
        assert!(!ours.exists() && !dirs.runs.join("r1").exists());
        assert!(!projects.join(&slug).exists() && projects.join("-otra").exists());
        // A session of another tool: only its file; its folder is not Clio's to remove.
        let elsewhere = t.path().join("proyecto");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let theirs = sessions.join("b.jsonl");
        std::fs::write(&theirs, "{}").unwrap();
        let p = plan(
            &dirs,
            &sessions,
            &info(theirs.clone(), "r2", Some(elsewhere.clone())),
        )
        .unwrap();
        assert!(p.run.is_none() && p.claude.is_empty());
        assert_eq!(describe(&p), "the conversation only");
        delete(&p).unwrap();
        assert!(!theirs.exists() && elsewhere.exists());
    }

    fn session_text(id: &str, cwd: &Path) -> String {
        format!(
            "{{\"type\":\"session\",\"id\":\"{id}\",\"cwd\":\"{}\"}}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":\"hello there\"}}}}\n",
            cwd.display()
        )
    }

    const SESSION: &str = "{\"type\":\"session\",\"id\":\"r1\",\"cwd\":\"/x\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"hello there\"}}\n";

    /// A session file in Pi's sessions folder of `dirs`, and its plan.
    fn session_in(dirs: &Dirs, name: &str) -> (PathBuf, Plan) {
        let sessions = super::super::pi::paths(dirs).sessions_dir();
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join(name);
        std::fs::write(&file, SESSION).unwrap();
        let s = read_session(&file).unwrap();
        (sessions.clone(), plan(dirs, &sessions, &s).unwrap())
    }

    fn entries(dirs: &Dirs) -> Vec<PathBuf> {
        std::fs::read_dir(trash_dir(dirs))
            .map(|d| d.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default()
    }

    fn mode(p: &Path) -> u32 {
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(p).unwrap().permissions())
            & 0o777
    }

    /// One of Clio's own sessions: a copy-mode one (its run folder with the sandbox, the seal, the
    /// lock and the marks) or a host-mode one (the run folder of the home). Returns its plan.
    fn clio_session(dirs: &Dirs, id: &str, host: bool) -> (PathBuf, Plan) {
        let run = dirs.runs.join(id);
        let cwd = if host {
            dirs.home.clone()
        } else {
            run.join("sandbox")
        };
        std::fs::create_dir_all(run.join("sandbox")).unwrap();
        std::fs::create_dir_all(run.join("toolhost")).unwrap();
        let meta = serde_json::json!({"id": id, "workspace": dirs.home, "instruction": "x", "created_ms": 1, "host": host});
        std::fs::write(run.join("meta.json"), meta.to_string()).unwrap();
        std::fs::write(run.join("manifest.json"), "{}").unwrap();
        std::fs::write(run.join("session-seal.json"), "{\"size\":1}").unwrap();
        std::fs::write(run.join("session.lock"), "").unwrap();
        std::fs::write(run.join("toolhost/tainted.json"), "{\"suspect\":true}").unwrap();
        let sessions = super::super::pi::paths(dirs).sessions_dir();
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join(format!("2026_{id}.jsonl"));
        std::fs::write(&file, session_text(id, &cwd.canonicalize().unwrap())).unwrap();
        let s = read_session(&file).unwrap();
        (sessions.clone(), plan(dirs, &sessions, &s).unwrap())
    }

    /// Whether the drawer would open the session (a row of Clio's own, not "another tool made it").
    fn opens(dirs: &Dirs) -> Vec<bool> {
        use super::super::chat::SessionSource;
        super::super::chat::local::LocalSource::new(std::sync::Arc::new(Dirs {
            config: dirs.config.clone(),
            data: dirs.data.clone(),
            runs: dirs.runs.clone(),
            runtime: None,
            home: dirs.home.clone(),
        }))
        .list()
        .iter()
        .map(|r| r.unavailable.is_none())
        .collect()
    }

    #[test]
    fn a_deleted_conversation_is_in_the_trash_byte_for_byte_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let (_, p) = session_in(&dirs, "2026_r1.jsonl");
        std::fs::set_permissions(&p.file, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::create_dir_all(dirs.runs.join("r1/sandbox")).unwrap();
        std::fs::write(dirs.runs.join("r1/sandbox/f"), "x").unwrap();
        let p = Plan {
            run: Some(Run {
                id: "r1".into(),
                path: dirs.runs.join("r1"),
            }),
            ..p
        };
        delete(&p).unwrap();
        assert!(!p.file.exists() && !dirs.runs.join("r1").exists());
        let e = entries(&dirs);
        assert_eq!(e.len(), 1);
        let name = e[0].file_name().unwrap().to_str().unwrap().to_owned();
        assert!(
            name.ends_with("-r1") && entry_name(&name).is_some(),
            "{name}"
        );
        assert_eq!(
            std::fs::read_to_string(e[0].join("2026_r1.jsonl")).unwrap(),
            SESSION
        );
        // The run folder is in the entry, whole: nothing is removed for good.
        assert_eq!(
            std::fs::read_to_string(e[0].join("run/sandbox/f")).unwrap(),
            "x"
        );
        assert_eq!(mode(&e[0].join("2026_r1.jsonl")), 0o600);
        assert_eq!(
            (
                mode(&e[0]),
                mode(&trash_dir(&dirs)),
                mode(&dirs.data.join("trash"))
            ),
            (0o700, 0o700, 0o700)
        );
    }

    #[test]
    fn a_failed_move_deletes_nothing_and_leaves_no_entry() {
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let (sessions, p) = clio_session(&dirs, "r1", false);
        let before = std::fs::read(&p.file).unwrap();
        // The rename of the file itself fails (it is not there): the entry folder is cleaned up.
        let gone = Plan {
            file: sessions.join("nope.jsonl"),
            ..clone_plan(&p)
        };
        assert!(delete(&gone).is_err());
        assert!(entries(&dirs).is_empty() && dirs.runs.join("r1/sandbox").exists());
        // The file moves, then a piece after it cannot (Claude Code's copy is not there): the
        // file and the run folder go back, and the entry folder is gone.
        let bad = Plan {
            claude: vec![t.path().join("no-such-folder")],
            ..clone_plan(&p)
        };
        assert!(delete(&bad).is_err());
        assert_eq!(std::fs::read(&p.file).unwrap(), before);
        assert!(dirs.runs.join("r1/toolhost/tainted.json").exists());
        assert!(entries(&dirs).is_empty(), "{:?}", entries(&dirs));
        // The trash cannot be made: its place is a file.
        std::fs::remove_dir_all(dirs.data.join("trash")).unwrap();
        std::fs::write(dirs.data.join("trash"), "x").unwrap();
        assert!(delete(&p).is_err());
        assert_eq!(std::fs::read(&p.file).unwrap(), before);
        assert!(dirs.runs.join("r1/sandbox").exists());
    }

    fn clone_plan(p: &Plan) -> Plan {
        Plan {
            file: p.file.clone(),
            trash: p.trash.clone(),
            id: p.id.clone(),
            lock: p.lock.clone(),
            title: p.title.clone(),
            run: p.run.as_ref().map(|r| Run {
                id: r.id.clone(),
                path: r.path.clone(),
            }),
            unapplied: p.unapplied,
            claude: p.claude.clone(),
        }
    }

    #[test]
    fn a_copy_mode_session_comes_back_whole_and_opens_again() {
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let (sessions, p) = clio_session(&dirs, "r1", false);
        assert_eq!(opens(&dirs), [true]);
        let (body, seal, marks) = (
            std::fs::read(&p.file).unwrap(),
            std::fs::read(dirs.runs.join("r1/session-seal.json")).unwrap(),
            std::fs::read(dirs.runs.join("r1/toolhost/tainted.json")).unwrap(),
        );
        // What Claude Code kept for this copy goes with it, and comes back where it was.
        let projects = super::super::pi::paths(&dirs)
            .home_dir()
            .join(".claude/projects");
        let slug: String = dirs
            .runs
            .join("r1/sandbox")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        std::fs::create_dir_all(projects.join(&slug)).unwrap();
        std::fs::write(projects.join(&slug).join("cc.jsonl"), "cc").unwrap();
        let p = plan(&dirs, &sessions, &read_session(&p.file).unwrap()).unwrap();
        assert_eq!(p.claude, [projects.join(&slug)]);
        delete(&p).unwrap();
        assert!(!dirs.runs.join("r1").exists() && !projects.join(&slug).exists());
        assert!(opens(&dirs).is_empty());
        let entry = entries(&dirs).remove(0);
        // No id: the list. A start that matches nothing, an empty one and another word fail.
        assert_eq!(run(&dirs, &["restore".into()]).unwrap(), 0);
        assert!(run(&dirs, &["restore".into(), "zz".into()]).is_err());
        assert!(run(&dirs, &["restore".into(), String::new()]).is_err());
        assert!(run(&dirs, &["other".into()]).is_err());
        // Any one place taken: refused, and nothing at all is restored.
        std::fs::create_dir_all(projects.join(&slug)).unwrap();
        let e = run(&dirs, &["restore".into(), "r".into()]).unwrap_err();
        assert!(e.contains("already there"), "{e}");
        assert!(!p.file.exists() && !dirs.runs.join("r1").exists());
        assert!(entry.join("run").exists() && entry.join("2026_r1.jsonl").exists());
        std::fs::remove_dir(projects.join(&slug)).unwrap();
        // The file's own name in the way, too.
        std::fs::write(&p.file, "mine").unwrap();
        assert!(run(&dirs, &["restore".into(), "r".into()]).is_err());
        assert_eq!(std::fs::read_to_string(&p.file).unwrap(), "mine");
        assert!(!dirs.runs.join("r1").exists());
        std::fs::remove_file(&p.file).unwrap();
        // The unique start of the id restores all of it.
        assert_eq!(run(&dirs, &["restore".into(), "r".into()]).unwrap(), 0);
        assert_eq!(std::fs::read(&p.file).unwrap(), body);
        assert_eq!(
            std::fs::read(dirs.runs.join("r1/session-seal.json")).unwrap(),
            seal
        );
        assert_eq!(
            std::fs::read(dirs.runs.join("r1/toolhost/tainted.json")).unwrap(),
            marks
        );
        assert_eq!(
            std::fs::read(projects.join(&slug).join("cc.jsonl")).unwrap(),
            b"cc"
        );
        assert!(!entry.exists());
        // It is one of Clio's again, and the drawer lists it.
        assert_eq!(opens(&dirs), [true]);
        let l = clio_brain::pi::sessions::list_sessions(&sessions);
        assert_eq!(
            (l.sessions.len(), l.sessions[0].title.as_str()),
            (1, "hello there")
        );
    }

    #[test]
    fn a_host_mode_session_keeps_its_run_folder_and_marks_and_opens_after_restore() {
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let (_, p) = clio_session(&dirs, "h1", true);
        assert_eq!(opens(&dirs), [true]);
        assert!(p.run.is_none());
        delete(&p).unwrap();
        // As always: a host session has no working copy, and its marks stay where they are.
        assert!(dirs.runs.join("h1/toolhost/tainted.json").exists());
        assert!(opens(&dirs).is_empty());
        assert!(!entries(&dirs)[0].join("run").exists());
        assert_eq!(run(&dirs, &["restore".into(), "h1".into()]).unwrap(), 0);
        assert_eq!(opens(&dirs), [true]);
        assert!(dirs.runs.join("h1/toolhost/tainted.json").exists());
    }

    #[test]
    fn a_conversation_open_in_another_clio_is_not_deleted() {
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        for (id, host) in [("r1", false), ("h1", true)] {
            let (_, p) = clio_session(&dirs, id, host);
            let kept = entries(&dirs).len();
            // Another `clio` holds the run folder's lock.
            let held = File::open(dirs.runs.join(id).join("session.lock")).unwrap();
            rustix::fs::flock(&held, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
            assert_eq!(delete(&p).unwrap_err(), OPEN_ELSEWHERE);
            assert!(p.file.exists() && dirs.runs.join(id).join("sandbox").exists());
            assert_eq!(entries(&dirs).len(), kept);
            // Unlocked, not only closed: a process another test forks meanwhile shares this
            // descriptor until it execs, and a close releases the lock only with the last copy.
            rustix::fs::flock(&held, rustix::fs::FlockOperation::Unlock).unwrap();
            drop(held);
            delete(&p).unwrap();
            assert!(!p.file.exists());
        }
    }

    #[test]
    fn a_link_put_in_the_files_place_is_never_chmoded_nor_restored_through() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let (_, p) = session_in(&dirs, "2026_r1.jsonl");
        // After the check and before the rename, the file becomes a link to a folder.
        let target = t.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_file(&p.file).unwrap();
        std::os::unix::fs::symlink(&target, &p.file).unwrap();
        assert!(delete(&p).is_err());
        assert_eq!(mode(&target), 0o755);
        // The link is in the trash (it is not a conversation: it is never listed).
        assert!(entries(&dirs).len() == 1 && trashed(&trash_dir(&dirs)).is_empty());
    }

    #[test]
    fn the_purge_removes_only_old_well_named_folders_inside_the_trash() {
        use clio_core::FixedClock;
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("data/trash/sessions");
        let day = 86_400_000;
        let now = 100 * day;
        let clock = FixedClock::new(now);
        let make = |n: &str| {
            std::fs::create_dir_all(root.join(n)).unwrap();
            std::fs::write(root.join(n).join("a.jsonl"), "x").unwrap();
        };
        let old = format!("{}-abc-1", now - 31 * day);
        let fresh = format!("{}-abc-2", now - 29 * day);
        for n in [
            old.as_str(),
            fresh.as_str(),
            "5-bad name",
            "5",
            "x5-abc",
            "-abc",
            "5-",
        ] {
            make(n);
        }
        // The old entry holds a run folder too: it goes whole.
        std::fs::create_dir_all(root.join(&old).join("run/sandbox")).unwrap();
        // What is not a folder, a link to a folder elsewhere, and a sibling of the trash.
        std::fs::write(root.join("6-file"), "x").unwrap();
        let outside = t.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("7-link")).unwrap();
        std::fs::create_dir_all(t.path().join("data/trash/other/1-old")).unwrap();
        std::fs::create_dir_all(t.path().join("data/runs/1-old")).unwrap();
        purge_trash(&root, &clock);
        let left = |n: &str| root.join(n).exists();
        assert!(!left(&old));
        for n in [
            fresh.as_str(),
            "5-bad name",
            "5",
            "x5-abc",
            "-abc",
            "5-",
            "6-file",
            "7-link",
        ] {
            assert!(left(n), "{n}");
        }
        assert!(outside.join("keep").exists());
        assert!(
            t.path().join("data/trash/other/1-old").exists()
                && t.path().join("data/runs/1-old").exists()
        );
        // A few days later the other one is old too; a missing trash is nothing.
        clock.advance(3 * day);
        purge_trash(&root, &clock);
        assert!(!left(&fresh));
        purge_trash(&t.path().join("nope"), &clock);
    }

    // Another filesystem (rename fails): copied private, then removed. What is at the destination
    // is never replaced nor removed, and a link is never followed or chmoded.
    #[test]
    fn a_move_is_private_never_replaces_and_never_follows_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let shm = Path::new("/dev/shm");
        let t = tempfile::tempdir().unwrap();
        // `from` is on another filesystem than `to` when the machine has /dev/shm.
        let from = tempfile::tempdir_in(if shm.is_dir() { shm } else { t.path() }).unwrap();
        let a = from.path().join("a.jsonl");
        std::fs::write(&a, SESSION).unwrap();
        std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o644)).unwrap();
        let to = t.path().join("a.jsonl");
        // A file that is there is kept: the move fails, the source stays.
        std::fs::write(&to, "mine").unwrap();
        assert!(move_file(&a, &to).is_err());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), SESSION);
        std::fs::remove_file(&to).unwrap();
        move_file(&a, &to).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), SESSION);
        // The mode is the caller's to set (through the file): it was 0644 or the copy's 0600.
        make_private(&to).unwrap();
        assert_eq!(mode(&to), 0o600);
        // A link: never chmoded through, and a link source is not copied.
        let target = t.path().join("target");
        std::fs::write(&target, "secret").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = from.path().join("l.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(make_private(&link).is_err());
        let into = t.path().join("l.jsonl");
        let _ = move_file(&link, &into);
        assert_eq!(mode(&target), 0o644);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "secret");
        assert!(!into.exists() || std::fs::symlink_metadata(&into).unwrap().is_symlink());
        // On one filesystem the same: a rename never replaces.
        let (b, c) = (t.path().join("b"), t.path().join("c"));
        std::fs::write(&b, "b").unwrap();
        std::fs::write(&c, "c").unwrap();
        assert!(move_file(&b, &c).is_err());
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "c");
        assert!(rename_noreplace(&b, &c).is_err() && b.exists());
    }

    #[test]
    fn the_folder_is_named_with_the_cleaned_id_else_the_file_name_else_session() {
        let f = Path::new("/s/2026-11-18_abc.jsonl");
        assert_eq!(entry_id("r1", f), "r1");
        assert_eq!(entry_id("a/b c", f), "abc");
        assert_eq!(entry_id("///", f), "2026-11-18_abc");
        assert_eq!(entry_id("", f), "2026-11-18_abc");
        assert_eq!(entry_id("///", Path::new("/s/ ?.jsonl")), "session");
        assert!(valid_id(&entry_id(&"x".repeat(300), f)));
        // And such a session is deleted into a folder that is listed and restored.
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let sessions = super::super::pi::paths(&dirs).sessions_dir();
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join("2026_x.jsonl");
        std::fs::write(&file, SESSION).unwrap();
        let p = plan(&dirs, &sessions, &info(file.clone(), "///", None)).unwrap();
        delete(&p).unwrap();
        assert_eq!(trashed(&trash_dir(&dirs))[0].id, "2026_x");
        run(&dirs, &["restore".into(), "2026".into()]).unwrap();
        assert!(file.exists());
    }

    #[test]
    fn an_entry_name_or_an_exact_id_wins_over_a_start_and_a_tie_is_told() {
        let t = |name: &str| {
            let (at_ms, id) = entry_name(name).unwrap();
            Trashed {
                dir: PathBuf::from(name),
                name: name.into(),
                id: id.into(),
                at_ms,
                file: PathBuf::new(),
                info: None,
            }
        };
        let all = [t("5-ab"), t("6-abc"), t("7-abc"), t("8-xyz")];
        assert!(pick(&all, "").is_err());
        assert_eq!(pick(&all, "5-ab").unwrap().name, "5-ab");
        // `ab` is the whole id of one and the start of two others: the exact id wins.
        assert_eq!(pick(&all, "ab").unwrap().name, "5-ab");
        assert_eq!(pick(&all, "x").unwrap().name, "8-xyz");
        // Two entries of one id: the id alone is a tie, the entry name breaks it.
        assert!(pick(&all, "abc").unwrap_err().contains("more than one"));
        assert_eq!(pick(&all, "7-abc").unwrap().name, "7-abc");
        assert!(pick(&all, "a").unwrap_err().contains("more than one"));
        assert!(
            pick(&all, "nothing")
                .unwrap_err()
                .contains("nothing in the trash")
        );
    }

    #[test]
    fn nothing_outside_the_sessions_folder_nor_a_link_is_deleted() {
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let sessions = t.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let outside = t.path().join("fuera.jsonl");
        std::fs::write(&outside, "{}").unwrap();
        assert!(plan(&dirs, &sessions, &info(outside.clone(), "x", None)).is_err());
        let link = sessions.join("enlace.jsonl");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(plan(&dirs, &sessions, &info(link, "x", None)).is_err());
        let other = sessions.join("notas.txt");
        std::fs::write(&other, "x").unwrap();
        assert!(plan(&dirs, &sessions, &info(other, "x", None)).is_err());
        assert!(outside.exists());
    }
}
