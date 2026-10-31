use crate::{Error, Result};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Dir,
    File,
    Link(PathBuf),
}

/// A folder tools may read and write. `scratch` (outside the folder) holds the snapshots.
pub struct Sandbox {
    root: PathBuf,
    scratch: PathBuf,
    next: AtomicU64,
    /// Snapshots copy every file instead of hard-linking it.
    copy_snapshots: bool,
}

/// State of the folder at one moment: files are hard links (copies when the filesystem refuses).
/// A process that edits a file in place (not through `Sandbox::write`, which replaces the file)
/// also edits a hard-linked snapshot: whoever lets programs run in the folder must call
/// `Sandbox::set_copy_snapshots(true)` first.
pub struct Snapshot {
    store: PathBuf,
    entries: Vec<(PathBuf, Kind)>,
}

impl Snapshot {
    /// Frees the snapshot's storage.
    pub fn discard(self) {
        let _ = fs::remove_dir_all(&self.store);
    }
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Kind)>) -> Result<()> {
    let mut items: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    items.sort_by_key(|e| e.file_name());
    for e in items {
        let p = e.path();
        let rel = p.strip_prefix(base).unwrap_or(&p).to_path_buf();
        let t = e.file_type()?;
        if t.is_symlink() {
            out.push((rel, Kind::Link(fs::read_link(&p)?)));
        } else if t.is_dir() {
            out.push((rel, Kind::Dir));
            walk(base, &p, out)?;
        } else {
            out.push((rel, Kind::File));
        }
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) else {
        return false;
    };
    (ma.dev() == mb.dev() && ma.ino() == mb.ino())
        || (ma.len() == mb.len() && matches!((fs::read(a), fs::read(b)), (Ok(x), Ok(y)) if x == y))
}

impl Sandbox {
    /// `root` must exist; `scratch` is created and must not be inside `root`.
    pub fn new(root: &Path, scratch: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        fs::create_dir_all(scratch)?;
        let scratch = scratch.canonicalize()?;
        if scratch.starts_with(&root) {
            return Err(Error::Escape("scratch inside the sandbox".into()));
        }
        Ok(Self {
            root,
            scratch,
            next: AtomicU64::new(0),
            copy_snapshots: false,
        })
    }

    /// Snapshots become real copies. Needed once a program can run in the folder (the jail): it
    /// can edit a file in place, and with hard links that would change the snapshot too.
    pub fn set_copy_snapshots(&mut self, on: bool) {
        self.copy_snapshots = on;
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// S-13: turn a relative path into an absolute one that is surely inside the root.
    /// Rejects `..`, absolute paths and any symlink (even a dangling one) that leads out.
    /// Shortcut: checked at resolve time, not at open time; a racing local process could swap a
    /// component for a symlink in between. Upgrade to openat-style resolution if that matters.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let bad = || Error::Escape(rel.to_string());
        let mut cur = self.root.clone();
        let mut missing = false; // once a component is missing, nothing deeper can be a symlink
        for c in Path::new(rel).components() {
            match c {
                Component::CurDir => {}
                Component::Normal(n) => {
                    cur.push(n);
                    if missing {
                        continue;
                    }
                    match fs::symlink_metadata(&cur) {
                        Ok(m) if m.is_symlink() => {
                            cur = cur.canonicalize().map_err(|_| bad())?;
                            if !cur.starts_with(&self.root) {
                                return Err(bad());
                            }
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing = true,
                        Err(e) => return Err(e.into()),
                    }
                }
                _ => return Err(bad()),
            }
        }
        Ok(cur)
    }

    pub fn read(&self, rel: &str) -> Result<Vec<u8>> {
        Ok(fs::read(self.resolve(rel)?)?)
    }

    pub fn list(&self, rel: &str) -> Result<Vec<String>> {
        let mut v = fs::read_dir(self.resolve(rel)?)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<_>>>()?;
        v.sort();
        Ok(v)
    }

    pub fn mkdir(&self, rel: &str) -> Result<()> {
        Ok(fs::create_dir_all(self.resolve(rel)?)?)
    }

    /// Replaces the file with a new one (write + rename), so a hard-linked snapshot never changes.
    pub fn write(&self, rel: &str, data: &[u8]) -> Result<()> {
        let p = self.resolve(rel)?;
        let dir = p.parent().ok_or_else(|| Error::Escape(rel.into()))?;
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(
            ".clio-tmp-{}-{}",
            std::process::id(),
            self.next.fetch_add(1, Ordering::SeqCst)
        ));
        fs::write(&tmp, data)?;
        if let Err(e) = fs::rename(&tmp, &p) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let store = self
            .scratch
            .join(format!("snap-{}", self.next.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&store)?;
        let mut entries = Vec::new();
        walk(&self.root, &self.root, &mut entries)?;
        for (rel, k) in &entries {
            if *k == Kind::File {
                let dst = store.join(rel);
                if let Some(d) = dst.parent() {
                    fs::create_dir_all(d)?;
                }
                let src = self.root.join(rel);
                if self.copy_snapshots || fs::hard_link(&src, &dst).is_err() {
                    fs::copy(&src, &dst)?;
                }
            }
        }
        Ok(Snapshot { store, entries })
    }

    /// Leaves the folder as it was at `snap`: new entries removed, deleted and changed ones back.
    /// (Directory modification times are not restored.)
    pub fn restore(&self, snap: &Snapshot) -> Result<()> {
        let want: HashMap<&Path, &Kind> =
            snap.entries.iter().map(|(p, k)| (p.as_path(), k)).collect();
        let mut now = Vec::new();
        walk(&self.root, &self.root, &mut now)?;
        let mut removed: Vec<PathBuf> = Vec::new();
        for (rel, k) in &now {
            if removed.iter().any(|r| rel.starts_with(r)) {
                continue;
            }
            let keep = match (want.get(rel.as_path()), k) {
                (Some(Kind::Dir), Kind::Dir) => true,
                (Some(Kind::Link(a)), Kind::Link(b)) => a == b,
                (Some(Kind::File), Kind::File) => {
                    same_file(&self.root.join(rel), &snap.store.join(rel))
                }
                _ => false,
            };
            if !keep {
                let p = self.root.join(rel);
                if *k == Kind::Dir {
                    fs::remove_dir_all(&p)?;
                    removed.push(rel.clone());
                } else {
                    fs::remove_file(&p)?;
                }
            }
        }
        for (rel, k) in &snap.entries {
            let p = self.root.join(rel);
            match k {
                Kind::Dir => fs::create_dir_all(&p)?,
                _ if fs::symlink_metadata(&p).is_ok() => {}
                Kind::File => {
                    let src = snap.store.join(rel);
                    if fs::hard_link(&src, &p).is_err() {
                        fs::copy(&src, &p)?;
                    }
                }
                Kind::Link(t) => symlink(t, &p)?,
            }
        }
        Ok(())
    }
}
