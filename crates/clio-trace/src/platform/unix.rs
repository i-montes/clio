use std::fs::OpenOptions;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::{Error, Result};

/// Creates `path` with mode 0600 if missing; refuses an existing file that group or others can
/// read, and a symbolic link (S-23). Creation is `O_EXCL`, which never follows a link.
pub fn prepare_private_file(path: &Path) -> Result<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(path).map_err(|e| Error::Io(e.to_string()))?;
            if !meta.file_type().is_file() {
                return Err(Error::NotRegular(path.to_path_buf()));
            }
            if meta.mode() & 0o077 != 0 {
                return Err(Error::Permissions(path.to_path_buf()));
            }
            Ok(())
        }
        Err(e) => Err(Error::Io(e.to_string())),
    }
}
