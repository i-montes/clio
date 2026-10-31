//! The workspace and its copy: validation, what is excluded, the copy with its hash manifest, and
//! the set of changes between the copy and the manifest.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use super::home::Dirs;
use super::{Res, io_err, mkdir_private, set_mode, sha_hex, write_private};

const SKIP_DIRS: [&str; 5] = [".git", "node_modules", "target", ".venv", "__pycache__"];

/// A name that looks like it holds a secret.
pub fn is_secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with(".env")
        || n.ends_with(".pem")
        || n.ends_with(".key")
        || n.starts_with("id_rsa")
        || n.starts_with("id_ed25519")
        || n == ".ssh"
        || n == ".aws"
        || n.starts_with("credentials")
}

#[derive(Debug, Default)]
pub struct Scan {
    pub files: Vec<String>,
    pub bytes: u64,
    pub secrets: Vec<String>,
    pub skipped_dirs: Vec<String>,
    pub symlinks: Vec<String>,
}

/// Refuses `/`, the home folder and anything holding Clio's own folders.
pub fn validate(ws: &Path, dirs: &Dirs) -> Res<PathBuf> {
    let ws = ws
        .canonicalize()
        .map_err(|e| format!("workspace {}: {e}", ws.display()))?;
    if !ws.is_dir() {
        return Err(format!("{} is not a folder", ws.display()));
    }
    let home = dirs
        .home
        .canonicalize()
        .unwrap_or_else(|_| dirs.home.clone());
    if ws == Path::new("/") {
        return Err("refusing to use / as a workspace".into());
    }
    if ws == home {
        return Err("refusing to use your whole home folder as a workspace; pick a project".into());
    }
    for own in [&dirs.config, &dirs.data] {
        let own = own.canonicalize().unwrap_or_else(|_| own.clone());
        if own.starts_with(&ws) || ws.starts_with(&own) {
            return Err("the workspace overlaps Clio's own folders".into());
        }
    }
    Ok(ws)
}

/// Walks the workspace deciding what is copied. Errors when it is bigger than the limits.
pub fn scan(root: &Path, max_files: u64, max_bytes: u64) -> Res<Scan> {
    fn walk(dir: &Path, rel: &str, s: &mut Scan, lim: (u64, u64)) -> Res<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| io_err(dir, &e))?
            .collect::<Result<_, _>>()
            .map_err(|e| io_err(dir, &e))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let kind = e.file_type().map_err(|x| io_err(&e.path(), &x))?;
            if kind.is_symlink() {
                s.symlinks.push(r);
            } else if kind.is_dir() {
                if SKIP_DIRS.contains(&name.as_str()) {
                    s.skipped_dirs.push(r);
                } else if is_secret_name(&name) {
                    s.secrets.push(r);
                } else {
                    walk(&e.path(), &r, s, lim)?;
                }
            } else if kind.is_file() {
                if is_secret_name(&name) {
                    s.secrets.push(r);
                    continue;
                }
                s.bytes += e.metadata().map_err(|x| io_err(&e.path(), &x))?.len();
                s.files.push(r);
                if s.files.len() as u64 > lim.0 || s.bytes > lim.1 {
                    return Err(format!(
                        "the workspace is bigger than {} files / {} MB; choose a smaller folder or pass --allow-broad",
                        lim.0,
                        lim.1 / (1024 * 1024)
                    ));
                }
            }
        }
        Ok(())
    }
    let mut s = Scan::default();
    walk(root, "", &mut s, (max_files, max_bytes))?;
    Ok(s)
}

/// Copies `files` from `root` into `dest` (owner-only permissions) and returns path -> sha256.
pub fn copy_tree(root: &Path, files: &[String], dest: &Path) -> Res<BTreeMap<String, String>> {
    mkdir_private(dest)?;
    let mut manifest = BTreeMap::new();
    for rel in files {
        let from = root.join(rel);
        let to = dest.join(rel);
        if let Some(p) = to.parent() {
            mkdir_private(p)?;
        }
        let bytes = std::fs::read(&from).map_err(|e| io_err(&from, &e))?;
        write_private(&to, &bytes)?;
        let exec = std::fs::metadata(&from)
            .map_err(|e| io_err(&from, &e))?
            .permissions()
            .mode()
            & 0o100
            != 0;
        if exec {
            set_mode(&to, 0o700)?;
        }
        manifest.insert(rel.clone(), sha_hex(&bytes));
    }
    Ok(manifest)
}

/// After the agent ran: every folder 0700 and every file 0600 (0700 if the owner can run it).
pub fn lock_down(dir: &Path) -> Res<()> {
    set_mode(dir, 0o700)?;
    for e in std::fs::read_dir(dir).map_err(|e| io_err(dir, &e))? {
        let e = e.map_err(|x| io_err(dir, &x))?;
        let p = e.path();
        let kind = e.file_type().map_err(|x| io_err(&p, &x))?;
        if kind.is_dir() {
            lock_down(&p)?;
        } else if kind.is_file() {
            let m = std::fs::metadata(&p)
                .map_err(|x| io_err(&p, &x))?
                .permissions()
                .mode();
            set_mode(&p, if m & 0o100 != 0 { 0o700 } else { 0o600 })?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Modified,
    Created,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub rel: String,
    pub kind: Kind,
}

fn walk_files(dir: &Path, rel: &str, out: &mut Vec<String>) -> Res<()> {
    for e in std::fs::read_dir(dir).map_err(|e| io_err(dir, &e))? {
        let e = e.map_err(|x| io_err(dir, &x))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let kind = e.file_type().map_err(|x| io_err(&e.path(), &x))?;
        if kind.is_dir() {
            walk_files(&e.path(), &r, out)?;
        } else if kind.is_file() {
            out.push(r);
        }
    }
    Ok(())
}

/// The sandbox against the manifest taken at copy time. Sorted by path.
pub fn changes(sandbox: &Path, manifest: &BTreeMap<String, String>) -> Res<Vec<Change>> {
    let mut now = Vec::new();
    walk_files(sandbox, "", &mut now)?;
    let mut out = Vec::new();
    for rel in &now {
        let bytes = std::fs::read(sandbox.join(rel)).map_err(|e| io_err(&sandbox.join(rel), &e))?;
        match manifest.get(rel) {
            None => out.push(Change {
                rel: rel.clone(),
                kind: Kind::Created,
            }),
            Some(h) if *h != sha_hex(&bytes) => {
                out.push(Change {
                    rel: rel.clone(),
                    kind: Kind::Modified,
                });
            }
            Some(_) => {}
        }
    }
    for rel in manifest.keys() {
        if !now.contains(rel) {
            out.push(Change {
                rel: rel.clone(),
                kind: Kind::Deleted,
            });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// A path from a record that is safe to join to a root: only plain components.
pub fn plain_rel(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn secret_names() {
        for n in [
            ".env",
            ".env.local",
            "a.pem",
            "x.KEY",
            "id_rsa",
            "id_rsa.pub",
            "id_ed25519",
            ".ssh",
            ".aws",
            "credentials.json",
        ] {
            assert!(is_secret_name(n), "{n}");
        }
        for n in ["main.rs", "environment.md", "key.txt", "README"] {
            assert!(!is_secret_name(n), "{n}");
        }
    }

    #[test]
    fn plain_rel_rejects_escapes() {
        assert!(plain_rel("a/b.txt"));
        for r in ["", "/etc/x", "../x", "a/../b", "./a"] {
            assert!(!plain_rel(r), "{r}");
        }
    }
}
