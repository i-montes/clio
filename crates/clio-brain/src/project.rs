//! Which project a session works on, for the memory (spec `2026-11-04-memory-engram-design.md`):
//! decided by the brain from the folder, never by the model. The name of the `origin` remote's
//! repository when there is one, else the repository's folder, else the folder itself; in one
//! normal form, so the same project is the same name however it was reached (`Clio`, ` clio `,
//! `git@host:me/clio.git`).

use std::path::{Path, PathBuf};

/// The longest project name kept (a scope name is `proyecto:` and this).
const MAX_NAME: usize = 80;

/// The file that names a folder's project by hand (`clio memory project set NAME`).
pub const PROJECT_FILE: &str = ".clio-project";
/// The variable that names it for one process.
pub const PROJECT_ENV: &str = "CLIO_PROJECT";

/// Where a project name came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    File,
    Detected,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Env => "CLIO_PROJECT",
            Source::File => PROJECT_FILE,
            Source::Detected => "detectado de la carpeta",
        }
    }
}

/// The project of `dir` as Clio uses it: `CLIO_PROJECT`, else a `.clio-project` in the folder or
/// above it (up to the repository's root), else what [`detect`] finds. A name that is empty
/// once normalized is skipped.
pub fn resolve(dir: &Path) -> Option<(String, Source)> {
    resolve_with(dir, std::env::var(PROJECT_ENV).ok().as_deref())
}

pub fn resolve_with(dir: &Path, env: Option<&str>) -> Option<(String, Source)> {
    if let Some(n) = env.map(normal).filter(|n| !n.is_empty()) {
        return Some((n, Source::Env));
    }
    let top = git_root(dir);
    for d in dir.ancestors() {
        if let Some(n) = read_name(&d.join(PROJECT_FILE)) {
            return Some((n, Source::File));
        }
        if top.as_deref() == Some(d) {
            break;
        }
    }
    detect(dir).map(|n| (n, Source::Detected))
}

/// Where the project each folder was first detected as is kept, in the person's data folder.
const SEEN_FILE: &str = "projects.json";

/// The project of `dir` for the memory, with a word when detection changed: a name set by hand
/// (`CLIO_PROJECT`, `.clio-project`) always wins; a detected one is remembered per repository (or
/// folder) the first time, and if detection later says something else (a renamed remote, another
/// repository cloned at the same path) the third value says so. The current detection is used:
/// two repositories at one path must not share a memory; merging is the person's choice
/// (`clio memory project set OLD`). The record is best effort and never rewritten from a file it
/// could not read.
pub fn settle(data: &Path, dir: &Path) -> Option<(String, Source, Option<String>)> {
    settle_with(data, dir, std::env::var(PROJECT_ENV).ok().as_deref())
}

pub fn settle_with(
    data: &Path,
    dir: &Path,
    env: Option<&str>,
) -> Option<(String, Source, Option<String>)> {
    let (name, source) = resolve_with(dir, env)?;
    if source != Source::Detected {
        return Some((name, source, None));
    }
    let Ok(key) = git_root(dir)
        .unwrap_or_else(|| dir.to_path_buf())
        .canonicalize()
    else {
        return Some((name, source, None));
    };
    let key = key.display().to_string();
    let path = data.join(SEEN_FILE);
    let seen: std::collections::BTreeMap<String, String> = match std::fs::read_to_string(&path) {
        Ok(t) => match serde_json::from_str(&t) {
            Ok(m) => m,
            // Damaged: left as it is, and nothing remembered this time.
            Err(_) => return Some((name, source, None)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(_) => return Some((name, source, None)),
    };
    match seen.get(&key).and_then(|b| checked(b.clone())) {
        Some(before) if before != name => {
            let note = format!(
                "esta carpeta era del proyecto {before} y ahora se detecta {name}: lo de {before} no se ve aquí; si es el mismo proyecto, clio memory project set {before}"
            );
            Some((name, source, Some(note)))
        }
        Some(_) => Some((name, source, None)),
        None => {
            let mut seen = seen;
            seen.insert(key, name.clone());
            if data.is_dir() {
                let _ = write_private(
                    &path,
                    &serde_json::to_string_pretty(&seen).unwrap_or_default(),
                );
            }
            Some((name, source, None))
        }
    }
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    // One temporary file per process: two writers never mix their bytes (the last one wins).
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// The first line of a project file, normalized; at most 4 KiB are read.
fn read_name(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(4096)
        .read_to_string(&mut text)
        .ok()?;
    let n = normal(text.lines().next().unwrap_or_default());
    (!n.is_empty()).then_some(n)
}

/// The project of `dir`, or `None` when there is nothing to call it.
pub fn detect(dir: &Path) -> Option<String> {
    let root = git_root(dir);
    let from_remote = root
        .as_deref()
        .and_then(origin_url)
        .and_then(|u| repo_name(&u));
    let raw = from_remote.or_else(|| {
        root.as_deref()
            .unwrap_or(dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    })?;
    let name = normal(&raw);
    (!name.is_empty()).then_some(name)
}

/// NFC, lowercase, blanks to `-`, nothing but letters, digits, `-`, `_` and `.`, no dots at the ends,
/// at most `MAX_NAME` characters.
pub fn normal(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfc: String = s.nfc().collect();
    let joined = nfc.split_whitespace().collect::<Vec<_>>().join("-");
    let kept: String = joined
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(MAX_NAME)
        .collect();
    kept.trim_matches(|c| c == '.' || c == '-').to_owned()
}

/// A name handed over from elsewhere, kept only if it is already in normal form: anything else
/// (a path, blanks, an escape) is no project at all rather than a different one.
pub fn checked(name: String) -> Option<String> {
    (!name.is_empty() && normal(&name) == name).then_some(name)
}

/// The folder that holds `.git` (a folder, or a file in a worktree), going up from `dir`.
fn git_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The URL of `[remote "origin"]` in the repository's config, read as text (no `git` is run).
fn origin_url(root: &Path) -> Option<String> {
    let git = root.join(".git");
    let dir = if git.is_dir() {
        git
    } else {
        // A worktree: `.git` is a file that says `gitdir: <path>`; its config is the main one's.
        let text = std::fs::read_to_string(&git).ok()?;
        // A relative `gitdir:` is relative to the worktree, not to wherever `clio` runs.
        let gitdir = root.join(text.strip_prefix("gitdir:")?.trim());
        let common = gitdir.join("commondir");
        match std::fs::read_to_string(&common) {
            Ok(rel) => gitdir.join(rel.trim()),
            Err(_) => gitdir,
        }
    };
    let config = std::fs::read_to_string(dir.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line.replace(' ', "") == "[remote\"origin\"]";
        } else if in_origin && let Some(v) = line.strip_prefix("url") {
            return Some(v.trim_start().strip_prefix('=')?.trim().to_owned());
        }
    }
    None
}

/// `git@github.com:me/clio.git`, `https://github.com/me/clio`, `/srv/git/clio.git` → `clio`.
fn repo_name(url: &str) -> Option<String> {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()?
        .trim_end_matches(".git");
    (!last.is_empty()).then(|| last.to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn repo(remote: Option<&str>) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Mi Proyecto");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        let mut config = "[core]\n\trepositoryformatversion = 0\n".to_owned();
        if let Some(url) = remote {
            config.push_str(&format!("[remote \"upstream\"]\n\turl = https://x/otro.git\n[remote \"origin\"]\n\turl = {url}\n"));
        }
        std::fs::write(root.join(".git/config"), config).unwrap();
        t
    }

    #[test]
    fn the_origin_remote_names_the_project_from_any_folder_inside() {
        for url in [
            "git@github.com:me/Clio.git",
            "https://github.com/me/clio",
            "https://github.com/me/clio.git/",
            "ssh://git@host:22/me/clio.git",
            "/srv/git/clio.git",
        ] {
            let t = repo(Some(url));
            let deep = t.path().join("Mi Proyecto/src/deep");
            assert_eq!(detect(&deep).as_deref(), Some("clio"), "{url}");
        }
    }

    #[test]
    fn without_a_remote_it_is_the_repository_folder_and_without_git_the_folder() {
        let t = repo(None);
        assert_eq!(
            detect(&t.path().join("Mi Proyecto/src")).as_deref(),
            Some("mi-proyecto")
        );
        let plain = tempfile::tempdir().unwrap();
        let dir = plain.path().join("Notas Personales");
        std::fs::create_dir(&dir).unwrap();
        assert_eq!(detect(&dir).as_deref(), Some("notas-personales"));
    }

    #[test]
    fn a_worktree_reads_the_main_repositorys_remote() {
        let t = repo(Some("git@github.com:me/clio.git"));
        let main_git = t.path().join("Mi Proyecto/.git");
        let wt_git = main_git.join("worktrees/e4");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = t.path().join("clio-e4");
        std::fs::create_dir(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        assert_eq!(detect(&wt).as_deref(), Some("clio"));
        // The same worktree with a relative `gitdir:` (`worktree.useRelativePaths`).
        std::fs::write(
            wt.join(".git"),
            "gitdir: ../Mi Proyecto/.git/worktrees/e4\n",
        )
        .unwrap();
        assert_eq!(detect(&wt.join(".")).as_deref(), Some("clio"));
    }

    #[test]
    fn names_have_one_form() {
        assert_eq!(normal("  Clio  "), "clio");
        assert_eq!(normal("Mi Proyecto Grande"), "mi-proyecto-grande");
        assert_eq!(normal("../evil\u{1b}[31m"), "evil31m");
        assert_eq!(normal("Ñandú"), "ñandú");
        assert_eq!(
            normal("N\u{303}andu\u{301}"),
            "ñandú",
            "NFD is the same project"
        );
        assert_eq!(normal(&"a".repeat(200)).len(), MAX_NAME);
        assert_eq!(normal("..."), "");
    }

    #[test]
    fn the_variable_then_the_file_then_the_folder() {
        let t = repo(Some("git@github.com:me/clio.git"));
        let root = t.path().join("Mi Proyecto");
        let deep = root.join("src/deep");
        assert_eq!(
            resolve_with(&deep, None),
            Some(("clio".into(), Source::Detected))
        );
        std::fs::write(root.join(PROJECT_FILE), "  Clio Web \nlo demás se ignora\n").unwrap();
        assert_eq!(
            resolve_with(&deep, None),
            Some(("clio-web".into(), Source::File))
        );
        assert_eq!(
            resolve_with(&deep, Some("Otro")),
            Some(("otro".into(), Source::Env))
        );
        // An empty name is skipped, not a project called "".
        assert_eq!(resolve_with(&deep, Some(" ")).unwrap().1, Source::File);
        std::fs::write(root.join(PROJECT_FILE), "...\n").unwrap();
        assert_eq!(resolve_with(&deep, None).unwrap().1, Source::Detected);
        // A file above the repository's root is not this repository's.
        std::fs::remove_file(root.join(PROJECT_FILE)).unwrap();
        std::fs::write(t.path().join(PROJECT_FILE), "fuera\n").unwrap();
        assert_eq!(resolve_with(&deep, None).unwrap().0, "clio");
    }

    #[test]
    fn a_detected_project_stays_when_detection_changes_and_a_hand_set_one_wins() {
        let t = repo(Some("git@github.com:me/clio.git"));
        let data = tempfile::tempdir().unwrap();
        let deep = t.path().join("Mi Proyecto/src");
        let got = |env: Option<&str>| settle_with(data.path(), &deep, env).unwrap();
        assert_eq!(got(None), ("clio".into(), Source::Detected, None));
        // Another repository at the same path (or a renamed remote): what is detected now is used,
        // never the other one's memory, and it says how to join them if they are the same.
        std::fs::write(
            t.path().join("Mi Proyecto/.git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:me/clio-nuevo.git\n",
        )
        .unwrap();
        let (name, _, note) = got(None);
        assert_eq!(name, "clio-nuevo");
        assert!(note.unwrap().contains("clio memory project set clio"));
        // A damaged record is left alone and nothing breaks.
        let file = data.path().join(SEEN_FILE);
        std::fs::write(&file, "{roto").unwrap();
        assert_eq!(got(None), ("clio-nuevo".into(), Source::Detected, None));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{roto");
        // A folder that is gone still has its detected name.
        let gone = t.path().join("ya-no-existe");
        assert_eq!(
            settle_with(data.path(), &gone, None).unwrap().0,
            "ya-no-existe"
        );
        // A name set by hand always wins.
        assert_eq!(got(Some("clio-nuevo")).0, "clio-nuevo");
        std::fs::write(
            t.path().join("Mi Proyecto").join(PROJECT_FILE),
            "clio-nuevo\n",
        )
        .unwrap();
        assert_eq!(got(None), ("clio-nuevo".into(), Source::File, None));
    }

    #[test]
    fn a_handed_over_name_is_kept_only_in_normal_form() {
        assert_eq!(checked("clio".into()).as_deref(), Some("clio"));
        for bad in [
            "",
            "Clio",
            "../clio",
            "a b",
            "clio\u{1b}[31m",
            "lecciones x",
        ] {
            assert_eq!(checked(bad.into()), None, "{bad:?}");
        }
    }
}
