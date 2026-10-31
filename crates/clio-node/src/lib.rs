//! Node-side execution: sandbox, effect log, leases, tools and the executor.
//! The gateway (another crate) decides; this crate only runs what it approved, and fails closed.

mod effects;
mod exec;
pub mod jail;
mod lease;
mod run_tests;
mod sandbox;
mod selftest;
mod tools;

pub use effects::EffectLog;
pub use exec::{Action, Approval, Executor, Outcome};
pub use jail::{Jail, JailOutput, Limits};
pub use lease::{Lease, LeaseTable};
pub use run_tests::{READ_ONLY, register_run_tests, run_tests_tool, valid_target};
pub use sandbox::{Sandbox, Snapshot};
pub use selftest::{CHECKS, SelfCheck, SelfTest};
pub use tools::{Found, Registry, Tool, basic_tools};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("path escapes the sandbox: {0}")]
    Escape(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad arguments: {0}")]
    BadArgs(String),
    #[error("tool {0} has no declared effect")]
    NoEffect(String),
    #[error("tool {0} has effect {1:?}, not allowed in this phase")]
    PhaseLimit(String, clio_core::Effect),
    #[error("tool {0} is already registered")]
    Duplicate(String),
    #[error("jail: {0}")]
    Jail(String),
    #[error("unknown tool {0}")]
    UnknownTool(String),
    #[error("no valid approval for this action")]
    NotApproved,
    #[error("action id {0} was already used with a different action")]
    IdReused(String),
    #[error("resource {resource} is held by {owner} until {until_ms}")]
    Held {
        resource: String,
        owner: String,
        until_ms: i64,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
