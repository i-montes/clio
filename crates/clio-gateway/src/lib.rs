//! The gateway: the one place every action goes through before it runs. It assumes the model
//! was already fooled, so the checks live in code, fail closed, and a deny always wins.
//! No network here: the gateway decides, the node executes.

mod client;
mod gateway;
mod normalize;
mod types;
mod untrusted;

pub use client::GatewayClient;
pub use gateway::{
    Approval, Approver, ExecResult, Executor, Gateway, NoPolicy, Policy, Rules, SecretBinding,
    SecretPolicy, StaticSecretPolicy, Telemetry, TerminalApprover,
};
pub use normalize::{NormalizedAction, confine, domain_matches, host_of};
pub use types::{
    ActionRequest, ArgKind, ArgSpec, Budget, Decision, Egress, Outcome, SessionInfo, ToolDecl,
};
pub use untrusted::{Html, ModelOutput, OutputError, Session, Source, wrap_untrusted};
