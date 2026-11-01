//! Pi as the agent harness (ADR 0007): the Rust side that installs Pi, launches it hardened,
//! talks to it over RPC and lists its sessions. Pi runs the loop, the providers, the sessions and
//! the compaction; Clio never rebuilds those.
//!
//! - [`install`]: the pinned version (from the embedded `adapters/pi/package.json`), Node check,
//!   `npm ci --ignore-scripts` from the embedded lockfile into `~/.local/share/clio/pi/<version>/`.
//! - [`launch`]: ONE function that starts `pi --mode rpc` with the flags and the clean
//!   environment of S-16/S-42, writes `models.json` (no key inside), and fails closed.
//! - [`rpc`]: JSONL framing (LF only), commands, the tolerant [`rpc::Event`] type, timeouts,
//!   cancellation, `Drop` that kills the process group, sanitized stderr, a usage meter.
//! - [`sessions`]: fast listing of Pi's `.jsonl` session files.
//! - [`text`]: sanitizing of everything that comes from outside.
//!
//! This is a library: the TUI and `clio run` do not use it yet (the `builtin` harness stays).

pub mod install;
pub mod launch;
pub mod rpc;
pub mod sessions;
pub mod text;

use std::fmt;
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Everything that can go wrong around Pi. `Display` is in Spanish (it reaches the person).
#[derive(Debug)]
pub enum PiError {
    /// Node is missing or too old.
    Node(String),
    /// Pi is not installed (or is not the pinned version).
    NotInstalled(String),
    /// A file or folder problem.
    Io(String),
    /// The process could not be started.
    Spawn(String),
    /// A wait ran out: `what` says which one.
    Timeout { what: String, secs: u64 },
    /// The process ended (or closed its output); `stderr` is the sanitized tail.
    Exited { stderr: String },
    /// Pi answered a command with `success: false`.
    Command { command: String, message: String },
    /// Pi sent something that breaks the protocol.
    Protocol(String),
    /// What the process reported is not what Clio asked for: it was killed.
    Verification(String),
    /// The caller cancelled.
    Cancelled,
}

impl fmt::Display for PiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(m) | Self::NotInstalled(m) | Self::Io(m) | Self::Spawn(m) => f.write_str(m),
            Self::Timeout { what, secs } => {
                write!(f, "Pi no respondió a tiempo ({what}, {secs} s)")
            }
            Self::Exited { stderr } if stderr.is_empty() => {
                f.write_str("Pi terminó inesperadamente")
            }
            Self::Exited { stderr } => write!(f, "Pi terminó inesperadamente: {stderr}"),
            Self::Command { command, message } => write!(f, "Pi rechazó `{command}`: {message}"),
            Self::Protocol(m) => write!(f, "protocolo de Pi roto: {m}"),
            Self::Verification(m) => write!(f, "Pi no cumple lo pedido, se detuvo: {m}"),
            Self::Cancelled => f.write_str("cancelado"),
        }
    }
}

impl std::error::Error for PiError {}

pub(crate) fn io_err(path: &Path, e: &std::io::Error) -> PiError {
    PiError::Io(format!("{}: {e}", path.display()))
}

/// Where Pi lives for Clio: `<root>/<version>/` (the install), `agent/` (`PI_CODING_AGENT_DIR`),
/// `sessions/` (`PI_CODING_AGENT_SESSION_DIR`) and `home/` (the `HOME` Pi sees). All 0700.
#[derive(Debug, Clone)]
pub struct PiPaths {
    pub root: PathBuf,
}

impl PiPaths {
    /// `data` is `~/.local/share/clio`.
    pub fn under_data(data: &Path) -> Self {
        Self {
            root: data.join("pi"),
        }
    }
    pub fn install_dir(&self, version: &str) -> PathBuf {
        self.root.join(version)
    }
    pub fn agent_dir(&self) -> PathBuf {
        self.root.join("agent")
    }
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }
    pub fn home_dir(&self) -> PathBuf {
        self.root.join("home")
    }
    /// Creates the folders (0700). Safe to repeat.
    pub fn ensure(&self) -> Result<(), PiError> {
        for d in [
            self.root.clone(),
            self.agent_dir(),
            self.sessions_dir(),
            self.home_dir(),
        ] {
            private_dir(&d)?;
        }
        Ok(())
    }
}

/// A folder (and its missing parents) for the owner only.
pub(crate) fn private_dir(path: &Path) -> Result<(), PiError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| io_err(path, &e))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| io_err(path, &e))
}

/// A file born 0600 (replaced if it exists).
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), PiError> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| io_err(path, &e))?;
    f.write_all(bytes).map_err(|e| io_err(path, &e))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| io_err(path, &e))
}
