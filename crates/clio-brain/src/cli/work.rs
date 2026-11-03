//! The workspace and its copy: validation, what is excluded, the copy with its hash manifest, and
//! the set of changes between the copy and the manifest.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use super::home::Dirs;
use super::{Res, io_err, mkdir_private, set_mode, sha_hex, write_private};

/// Folders the copy leaves out, at any depth. What the agent builds or installs there never goes
/// back to the original (`changes` ignores them; `apply` refuses them by name).
pub const SKIP_DIRS: [&str; 5] = [".git", "node_modules", "target", ".venv", "__pycache__"];

/// Names of files and folders that exist to hold a credential: the copy leaves them out whole,
/// because what the agent reads goes to the model's provider. A token inside any other file is
/// not left out: the filter masks it when the agent reads it (`clio_vault::has_mask`).
const SECRET_PREFIXES: [&str; 6] = [
    ".env",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "credentials",
];
const SECRET_SUFFIXES: [&str; 8] = [
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".kdbx",
    ".tfstate",
    ".tfstate.backup",
    ".keystore",
];
const SECRET_NAMES: [&str; 9] = [
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube",
    ".docker",
    ".netrc",
    ".pgpass",
    ".npmrc", // a legacy token here is a bare UUID, which no key pattern can recognise
    ".git-credentials",
];

/// A name that looks like it holds a secret.
pub fn is_secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_PREFIXES.iter().any(|p| n.starts_with(p))
        || SECRET_SUFFIXES.iter().any(|s| n.ends_with(s))
        || SECRET_NAMES.contains(&n.as_str())
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
        .map_err(|e| format!("carpeta {}: {e}", ws.display()))?;
    if !ws.is_dir() {
        return Err(format!("{} no es una carpeta", ws.display()));
    }
    let home = dirs
        .home
        .canonicalize()
        .unwrap_or_else(|_| dirs.home.clone());
    if ws == Path::new("/") {
        return Err("rechazo usar / como carpeta de trabajo".into());
    }
    if ws == home {
        return Err(
            "rechazo usar toda tu carpeta personal como proyecto; elige un proyecto".into(),
        );
    }
    for own in [&dirs.config, &dirs.data] {
        let own = own.canonicalize().unwrap_or_else(|_| own.clone());
        if own.starts_with(&ws) || ws.starts_with(&own) {
            return Err("la carpeta se cruza con las carpetas propias de Clio".into());
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
                        "la carpeta es más grande que {} archivos / {} MB; elige una más pequeña o usa --allow-broad",
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
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let kind = e.file_type().map_err(|x| io_err(&e.path(), &x))?;
        if kind.is_dir() {
            // `.git` is not skipped: the agent writing one is a change `apply` must refuse out loud.
            if name != ".git" && SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk_files(&e.path(), &r, out)?;
        } else if kind.is_file() {
            out.push(r);
        }
    }
    Ok(())
}

/// Reads a regular file that is not reached through a symbolic link. The opened file and the path
/// must be the same inode, so a link swapped in after the listing is refused, not followed.
///
/// The open never waits (`O_NONBLOCK`): the model can leave a FIFO in the copy, and opening one for
/// reading would block `clio diff` until somebody writes to it. Nothing larger than `READ_CAP` is
/// read: a file the model made huge is not something a person can review, and must not take all
/// the memory either.
pub fn read_regular(p: &Path) -> std::io::Result<Vec<u8>> {
    read_regular_capped(p, READ_CAP)
}

/// More than the copy itself may hold (200 MB by default), so no legitimate file is refused.
const READ_CAP: u64 = 256 * 1024 * 1024;

fn read_regular_capped(p: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let nonblock = i32::try_from(rustix::fs::OFlags::NONBLOCK.bits()).unwrap_or(0);
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nonblock)
        .open(p)?;
    let (open, path) = (f.metadata()?, std::fs::symlink_metadata(p)?);
    if !open.is_file()
        || !path.file_type().is_file()
        || (open.dev(), open.ino()) != (path.dev(), path.ino())
    {
        return Err(std::io::Error::other(
            "no es un archivo normal (enlace o cambió)",
        ));
    }
    let too_big = || {
        std::io::Error::other(format!(
            "demasiado grande para revisarlo aquí (más de {max} bytes)"
        ))
    };
    if open.len() > max {
        return Err(too_big());
    }
    let mut b = Vec::new();
    f.take(max + 1).read_to_end(&mut b)?;
    if b.len() as u64 > max {
        return Err(too_big());
    }
    Ok(b)
}

/// The sandbox against the manifest taken at copy time. Sorted by path.
pub fn changes(sandbox: &Path, manifest: &BTreeMap<String, String>) -> Res<Vec<Change>> {
    let mut now = Vec::new();
    walk_files(sandbox, "", &mut now)?;
    let mut out = Vec::new();
    for rel in &now {
        let bytes = read_regular(&sandbox.join(rel)).map_err(|e| io_err(&sandbox.join(rel), &e))?;
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
    fn changes_ignore_what_the_copy_leaves_out_but_not_git() {
        let sb = tempfile::tempdir().unwrap();
        for f in [
            "src/a.rs",
            "node_modules/x/i.js",
            "a/__pycache__/m.pyc",
            "target/debug/b",
            ".git/hooks/pre-commit",
        ] {
            let p = sb.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        }
        let got: Vec<String> = changes(sb.path(), &BTreeMap::new())
            .unwrap()
            .into_iter()
            .map(|c| c.rel)
            .collect();
        assert_eq!(got, [".git/hooks/pre-commit", "src/a.rs"]);
    }

    #[test]
    fn read_regular_refuses_a_link() {
        let d = tempfile::tempdir().unwrap();
        let (real, link) = (d.path().join("real"), d.path().join("link"));
        std::fs::write(&real, "ok").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(read_regular(&real).unwrap(), b"ok");
        assert!(read_regular(&link).is_err());
    }

    // S-13 (second audit, finding 6): a FIFO the model left in the copy must not stop `clio diff`
    // (opening one for reading waits for a writer), and a file that is too big is refused, not read.
    #[test]
    fn read_regular_does_not_wait_on_a_fifo_and_refuses_what_is_too_big() {
        let d = tempfile::tempdir().unwrap();
        let fifo = d.path().join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let f = fifo.clone();
        std::thread::spawn(move || tx.send(read_regular(&f).is_err()));
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(true),
            "reading a FIFO must fail at once, not wait"
        );
        let big = d.path().join("big");
        std::fs::write(&big, vec![b'x'; 100]).unwrap();
        assert!(read_regular_capped(&big, 100).is_ok());
        let e = read_regular_capped(&big, 99).unwrap_err().to_string();
        assert!(e.contains("demasiado grande"), "{e}");
    }

    #[test]
    fn plain_rel_rejects_escapes() {
        assert!(plain_rel("a/b.txt"));
        for r in ["", "/etc/x", "../x", "a/../b", "./a"] {
            assert!(!plain_rel(r), "{r}");
        }
    }
}
