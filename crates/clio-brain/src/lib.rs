//! The brain: control layer, verification and model catalog. No I/O: time comes from a `Clock`.

pub mod agent;
pub mod control;
pub mod memory_judge;
pub mod memory_link;
pub mod memory_port;
pub mod models;
pub mod pi;
/// Which project a folder is: the memory library's own detection (one for Clio and clio-memory).
pub use clio_memory::agent::project;
pub mod vault_link;
pub mod verify;
