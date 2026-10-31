//! The `clio` command line: small helpers shared by the commands.
//!
//! The rule behind all of it: the agent works on a COPY of the project, and only the person
//! decides what goes back to the original (S-14).

pub mod apply;
pub mod debug;
pub mod diff;
pub mod doctor;
pub mod home;
pub mod jail;
pub mod memory;
pub mod runs;
pub mod tui;
pub mod ui;
pub mod usage;
pub mod vault;
pub mod work;

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use sha2::{Digest, Sha256};

pub type Res<T> = Result<T, String>;

pub fn io_err(path: &Path, e: &std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

pub fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn mode(path: &Path) -> Option<u32> {
    std::fs::metadata(path).ok().map(|m| m.mode() & 0o777)
}

pub fn set_mode(path: &Path, mode: u32) -> Res<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| io_err(path, &e))
}

/// A directory (and its missing parents) readable by the owner only.
pub fn mkdir_private(path: &Path) -> Res<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| io_err(path, &e))?;
    set_mode(path, 0o700)
}

/// Creates or replaces a file that is born 0600 (no moment with open permissions).
pub fn write_private(path: &Path, bytes: &[u8]) -> Res<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| io_err(path, &e))?;
    f.write_all(bytes).map_err(|e| io_err(path, &e))?;
    set_mode(path, 0o600)
}

pub fn now_ms() -> i64 {
    use clio_core::Clock;
    clio_core::SystemClock.now_ms()
}

/// `YYYYMMDD-HHMMSS` in UTC, from epoch milliseconds (civil-from-days, no date crate).
pub fn stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Minimal argument reader: take what a command knows, then reject anything left that looks
/// like a flag.
pub struct Args(Vec<String>);

impl Args {
    pub fn new(v: Vec<String>) -> Self {
        Self(v)
    }

    pub fn flag(&mut self, name: &str) -> bool {
        let n = self.0.len();
        self.0.retain(|a| a != name);
        self.0.len() != n
    }

    pub fn value(&mut self, name: &str) -> Res<Option<String>> {
        let Some(i) = self.0.iter().position(|a| a == name) else {
            return Ok(None);
        };
        if i + 1 >= self.0.len() {
            return Err(format!("{name} needs a value"));
        }
        let v = self.0.remove(i + 1);
        self.0.remove(i);
        Ok(Some(v))
    }

    /// What is left; an unknown `--flag` is an error.
    pub fn rest(self) -> Res<Vec<String>> {
        match self.0.iter().find(|a| a.starts_with("--")) {
            Some(f) => Err(format!("unknown option {f}")),
            None => Ok(self.0),
        }
    }

    pub fn into_raw(self) -> Vec<String> {
        self.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn stamp_known_dates() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_782_864_000_000 + 3_661_000), "20260701-010101");
    }

    #[test]
    fn args_take_and_reject() {
        let mut a = Args::new(vec!["--yes".into(), "--n".into(), "3".into(), "id".into()]);
        assert!(a.flag("--yes"));
        assert_eq!(a.value("--n").unwrap().as_deref(), Some("3"));
        assert_eq!(a.rest().unwrap(), vec!["id".to_string()]);
        assert!(Args::new(vec!["--zzz".into()]).rest().is_err());
    }
}
