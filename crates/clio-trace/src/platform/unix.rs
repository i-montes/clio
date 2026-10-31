use std::fs::OpenOptions;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::{Error, Result};

/// Creates `path` with mode 0600 if missing; refuses an existing file that group or others can
/// read (S-23).
pub fn prepare_private_file(path: &Path) -> Result<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let mode = std::fs::metadata(path)
                .map_err(|e| Error::Io(e.to_string()))?
                .mode();
            if mode & 0o077 != 0 {
                return Err(Error::Permissions(path.to_path_buf()));
            }
            Ok(())
        }
        Err(e) => Err(Error::Io(e.to_string())),
    }
}
