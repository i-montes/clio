//! OS-specific code, isolated per decision 0002. Linux and macOS share one Unix module.

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use unix::*;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!(
    "clio-memory has no platform module for this OS; see docs/decisions/0002-platform-compatibility.md"
);
