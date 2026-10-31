use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Holds the exclusive writer lock on a database file until dropped.
#[derive(Debug)]
pub struct WriterLock {
    _file: File,
}

/// Creates the file with mode 0600 if missing; refuses to use one that group or others can access,
/// or that is a symbolic link (S-23). Creation is `O_EXCL`, which never follows a link, so a link
/// planted at the path in a shared directory cannot redirect the write.
pub fn prepare_db_file(path: &Path) -> Result<()> {
    match create_private_file(path) {
        Ok(_) => Ok(()),
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => check_db_file(path),
        Err(e) => Err(e),
    }
}

/// For read-only use: the file must already exist and not be accessible by group or others (S-23).
/// Never creates or changes anything.
pub fn check_db_file(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound,
        _ => Error::Io(e),
    })?;
    if !meta.file_type().is_file() {
        return Err(Error::NotRegularFile(path.to_path_buf()));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(Error::InsecurePermissions(path.to_path_buf()));
    }
    Ok(())
}

/// Creates `path` for the first time with mode 0600 and returns it open for writing; fails with
/// `AlreadyExists` if it exists.
pub fn create_private_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

/// Takes an exclusive, non-blocking `flock` on `<db_path>.lock`.
pub fn try_lock_writer(db_path: &Path) -> Result<WriterLock> {
    let mut name = db_path.as_os_str().to_owned();
    name.push(".lock");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(PathBuf::from(name))?;
    match file.try_lock() {
        Ok(()) => Ok(WriterLock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(Error::WriterLocked),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &std::path::Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn check_db_file_never_creates() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        assert!(matches!(check_db_file(&db), Err(Error::NotFound)));
        assert!(!db.exists());
    }

    #[test]
    fn s23_new_file_gets_0600() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        prepare_db_file(&db).unwrap();
        assert_eq!(mode(&db), 0o600);
    }

    #[test]
    fn s23_existing_0644_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        std::fs::write(&db, b"").unwrap();
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            prepare_db_file(&db),
            Err(Error::InsecurePermissions(_))
        ));
    }

    #[test]
    fn second_lock_fails_until_first_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let first = try_lock_writer(&db).unwrap();
        assert!(matches!(try_lock_writer(&db), Err(Error::WriterLocked)));
        drop(first);
        assert!(try_lock_writer(&db).is_ok());
    }
}
