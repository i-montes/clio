//! Which project a session works on, for the memory (spec `2026-11-04-memory-engram-design.md`):
//! decided by the brain from the folder, never by the model. The name of the `origin` remote's
//! repository when there is one, else the repository's folder, else the folder itself; in one
//! normal form, so the same project is the same name however it was reached (`Clio`, ` clio `,
//! `git@host:me/clio.git`).

use std::path::{Path, PathBuf};

/// The longest project name kept (a scope name is `proyecto:` and this).
const MAX_NAME: usize = 80;

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

/// Lowercase, blanks to `-`, nothing but letters, digits, `-`, `_` and `.`, no dots at the ends,
/// at most `MAX_NAME` characters.
pub fn normal(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join("-");
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
        let gitdir = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
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
    }

    #[test]
    fn names_have_one_form() {
        assert_eq!(normal("  Clio  "), "clio");
        assert_eq!(normal("Mi Proyecto Grande"), "mi-proyecto-grande");
        assert_eq!(normal("../evil\u{1b}[31m"), "evil31m");
        assert_eq!(normal("Ñandú"), "ñandú");
        assert_eq!(normal(&"a".repeat(200)).len(), MAX_NAME);
        assert_eq!(normal("..."), "");
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
