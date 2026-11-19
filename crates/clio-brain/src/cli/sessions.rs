//! Deleting Pi sessions, for the person: the conversation file in Pi's sessions folder goes to the
//! trash (`<data>/trash/sessions/<unix-ms>-<id>/`, 30 days, `clio sessions restore` brings it
//! back) and, when the session is one of Clio's, its working copy (`runs/<id>`) and the copies of
//! the conversation Claude Code kept (the claude connector) are removed for good, as they always
//! were. The memory is not touched. A copy with changes not applied to the person's folder is
//! said, so it is never lost by surprise.

use std::fs::{DirBuilder, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use clio_brain::pi::launch::valid_id;
use clio_brain::pi::sessions::{SessionInfo, read_session};
use clio_core::Clock;

use super::home::Dirs;
use super::runs::Run;
use super::work;
use super::{Res, io_err, mkdir_private, set_mode};

/// What deleting one session removes.
pub struct Plan {
    pub file: PathBuf,
    /// Where the conversation goes (`<data>/trash/sessions`), and the id its folder is named with.
    pub trash: PathBuf,
    pub id: String,
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
        // The header's id is untrusted: only what is safe in a folder name.
        id: s
            .id
            .chars()
            .take(128)
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._-".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect(),
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

/// Moves a file without ever following a link, and never over another file: a rename, and only
/// when that fails for being another filesystem, a copy (born 0600) and then the removal.
fn move_file(from: &Path, to: &Path) -> Res<()> {
    let e = match std::fs::rename(from, to) {
        Ok(()) => return set_mode(to, 0o600),
        Err(e) if e.kind() == ErrorKind::CrossesDevices => e,
        Err(e) => return Err(format!("{} to {}: {e}", from.display(), to.display())),
    };
    let copied = (|| -> std::io::Result<()> {
        let mut src = OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
            .open(from)?;
        if !src.metadata()?.is_file() {
            return Err(e);
        }
        let mut dst = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(to)?;
        std::io::copy(&mut src, &mut dst)?;
        dst.sync_all()
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_file(to);
        return Err(format!("{} to {}: {e}", from.display(), to.display()));
    }
    std::fs::remove_file(from).map_err(|e| io_err(from, &e))
}

/// Removes the session file (into the trash: nothing else is removed if that fails), then its
/// copy and Claude Code's. A link inside is removed, not followed.
pub fn delete(p: &Plan) -> Res<()> {
    let name = p.file.file_name().ok_or("the session has no file name")?;
    mkdir_private(&p.trash)?;
    let dir = p.trash.join(format!("{}-{}", super::now_ms(), p.id));
    DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| io_err(&dir, &e))?;
    if let Err(e) = move_file(&p.file, &dir.join(name)) {
        let _ = std::fs::remove_dir(&dir);
        return Err(e);
    }
    if let Some(run) = &p.run {
        std::fs::remove_dir_all(&run.path).map_err(|e| format!("{}: {e}", run.path.display()))?;
    }
    for d in &p.claude {
        std::fs::remove_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
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
  restore           lists the conversations in the trash (deleted in the last 30 days)
  restore <id>      brings one back to the sessions folder (the start of its id is enough);
                    it refuses if a file of that name is already there. A working copy that
                    was deleted with it is not in the trash and does not come back";

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
        }
        for t in &all {
            let (n, title) = t
                .info
                .as_ref()
                .map_or((0, "(unreadable)"), |i| (i.messages, &i.title));
            println!("{}  {n:>5} msgs  {title}  {}", super::stamp(t.at_ms), t.id);
        }
        return Ok(0);
    };
    let hits: Vec<&Trashed> = all
        .iter()
        .filter(|t| t.name == *want || t.id.starts_with(want.as_str()))
        .collect();
    let [t] = hits.as_slice() else {
        return Err(match hits.len() {
            0 => format!("nothing in the trash starts with {want:?} (see `clio sessions restore`)"),
            _ => format!("{want:?} starts more than one id in the trash: give more of it"),
        });
    };
    let dir = super::pi::paths(dirs).sessions_dir();
    let to = dir.join(t.file.file_name().ok_or("no file name")?);
    // shortcut: a file made between this check and the rename would be replaced; a person at a
    // terminal does not race themselves, add a hard link if that ever matters.
    if std::fs::symlink_metadata(&to).is_ok() {
        return Err(format!(
            "{} is already there: nothing was restored",
            to.display()
        ));
    }
    mkdir_private(&dir)?;
    move_file(&t.file, &to)?;
    let _ = std::fs::remove_dir(&t.dir);
    println!("restored {} to {}", t.id, to.display());
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
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect()
    }

    #[test]
    fn a_deleted_conversation_is_in_the_trash_byte_for_byte_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let (_, p) = session_in(&dirs, "2026_r1.jsonl");
        std::fs::set_permissions(&p.file, std::fs::Permissions::from_mode(0o644)).unwrap();
        // The run folder goes for good, as it always did: the trash keeps the conversation only.
        std::fs::create_dir_all(dirs.runs.join("r1/sandbox")).unwrap();
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
        assert_eq!(std::fs::read_dir(&e[0]).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_to_string(e[0].join("2026_r1.jsonl")).unwrap(),
            SESSION
        );
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
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
    fn a_failed_move_deletes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let (_, p) = session_in(&dirs, "2026_r1.jsonl");
        std::fs::create_dir_all(dirs.runs.join("r1/sandbox")).unwrap();
        // The trash cannot be made: its place is a file.
        std::fs::create_dir_all(&dirs.data).unwrap();
        std::fs::write(dirs.data.join("trash"), "x").unwrap();
        let p = Plan {
            run: Some(Run {
                id: "r1".into(),
                path: dirs.runs.join("r1"),
            }),
            ..p
        };
        assert!(delete(&p).is_err());
        assert!(p.file.exists() && dirs.runs.join("r1/sandbox").exists());
    }

    #[test]
    fn restore_brings_it_back_byte_for_byte_and_the_drawer_lists_it_again() {
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(t.path());
        let (sessions, p) = session_in(&dirs, "2026_r1.jsonl");
        delete(&p).unwrap();
        assert!(
            clio_brain::pi::sessions::list_sessions(&sessions)
                .sessions
                .is_empty()
        );
        let entry = entries(&dirs).remove(0);
        // No id: the list. A start that matches nothing, and the whole entry name, are told apart.
        assert_eq!(run(&dirs, &["restore".into()]).unwrap(), 0);
        assert!(run(&dirs, &["restore".into(), "zz".into()]).is_err());
        assert!(run(&dirs, &["other".into()]).is_err());
        // A file of that name in the way: refused, and both stay as they are.
        std::fs::write(&p.file, "mine").unwrap();
        let e = run(&dirs, &["restore".into(), "r".into()]).unwrap_err();
        assert!(e.contains("already there"), "{e}");
        assert_eq!(std::fs::read_to_string(&p.file).unwrap(), "mine");
        assert!(entry.join("2026_r1.jsonl").exists());
        std::fs::remove_file(&p.file).unwrap();
        // The unique start of the id restores it.
        assert_eq!(run(&dirs, &["restore".into(), "r".into()]).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&p.file).unwrap(), SESSION);
        assert!(!entry.exists());
        let l = clio_brain::pi::sessions::list_sessions(&sessions);
        assert_eq!(
            (l.sessions.len(), l.sessions[0].title.as_str()),
            (1, "hello there")
        );
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
        // A day later the other one is old too; a missing trash is nothing.
        clock.advance(3 * day);
        purge_trash(&root, &clock);
        assert!(!left(&fresh));
        purge_trash(&t.path().join("nope"), &clock);
    }

    // Another filesystem (rename fails): copied private, then removed; a link is never followed.
    #[test]
    fn the_copy_fallback_is_private_and_never_follows_a_link() {
        let shm = Path::new("/dev/shm");
        let t = tempfile::tempdir().unwrap();
        let from = tempfile::tempdir_in(if shm.is_dir() { shm } else { t.path() }).unwrap();
        let a = from.path().join("a.jsonl");
        std::fs::write(&a, SESSION).unwrap();
        let to = t.path().join("a.jsonl");
        move_file(&a, &to).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), SESSION);
        // Never over a file already there (copy path); a link source is refused.
        let target = t.path().join("target");
        std::fs::write(&target, "secret").unwrap();
        let link = from.path().join("l.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let into = t.path().join("l.jsonl");
        let r = move_file(&link, &into);
        // On one filesystem a rename moves the link itself; across two, the copy refuses it.
        assert!(r.is_err() || std::fs::symlink_metadata(&into).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "secret");
        assert!(!into.exists() || std::fs::symlink_metadata(&into).unwrap().is_symlink());
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
