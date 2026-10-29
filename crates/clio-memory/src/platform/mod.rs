//! OS-specific code, isolated per decision 0002. Only Linux is implemented.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(target_os = "linux"))]
compile_error!(
    "clio-memory has no platform module for this OS; see docs/decisions/0002-platform-compatibility.md"
);
