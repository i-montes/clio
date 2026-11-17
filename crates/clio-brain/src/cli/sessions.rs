//! Deleting Pi sessions, for the person: the conversation file in Pi's sessions folder and, when
//! the session is one of Clio's, its working copy (`runs/<id>`) and the copies of the
//! conversation Claude Code kept (the claude connector). The memory is not touched. A copy
//! with changes not applied to the person's folder is said, so it is never lost by surprise.

use std::path::{Path, PathBuf};

use clio_brain::pi::launch::valid_id;
use clio_brain::pi::sessions::SessionInfo;

use super::Res;
use super::home::Dirs;
use super::runs::Run;
use super::work;

/// What deleting one session removes.
pub struct Plan {
    pub file: PathBuf,
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
        return Err("solo se borran archivos de sesión de la carpeta de sesiones de Pi".into());
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
fn unapplied_changes(run: &Run) -> Option<usize> {
    if run.path.join("applied.json").exists() {
        return Some(0);
    }
    let manifest = run.manifest().ok()?;
    work::changes(&run.sandbox(), &manifest)
        .ok()
        .map(|c| c.len())
}

/// Removes the session file, then its copy and Claude Code's. A link inside is removed, not followed.
pub fn delete(p: &Plan) -> Res<()> {
    std::fs::remove_file(&p.file).map_err(|e| format!("{}: {e}", p.file.display()))?;
    if let Some(run) = &p.run {
        std::fs::remove_dir_all(&run.path).map_err(|e| format!("{}: {e}", run.path.display()))?;
    }
    for d in &p.claude {
        std::fs::remove_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    Ok(())
}

/// One line for the person about what a plan removes.
pub fn describe(p: &Plan) -> String {
    match (&p.run, p.unapplied) {
        (None, _) => "solo la conversación".into(),
        (Some(_), Some(0)) => "la conversación y su copia de trabajo".into(),
        (Some(_), Some(n)) => format!(
            "la conversación y su copia de trabajo, con {n} archivo(s) cambiados SIN APLICAR a tu carpeta"
        ),
        (Some(_), None) => {
            "la conversación y su copia de trabajo (no se pudo saber si tiene cambios sin aplicar)"
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
        assert!(describe(&p).contains("SIN APLICAR"));
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
        assert_eq!(describe(&p), "solo la conversación");
        delete(&p).unwrap();
        assert!(!theirs.exists() && elsewhere.exists());
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
