use clio_core::{Effect, Level};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Allow < Ask < Deny: combining two outcomes is `max`, so a deny always wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Allow,
    Ask,
    Deny,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Allow => "allow",
            Outcome::Ask => "ask",
            Outcome::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub outcome: Outcome,
    /// Stable id of the rule that decided: `S-12`, `step-8-reversible-in-sandbox`, ...
    pub rule: String,
    pub reason: String,
    pub id: String,
    /// Hash of the normalized action; absent when the request never got normalized.
    pub action_hash: Option<String>,
}

impl Decision {
    pub fn fail_closed(reason: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Deny,
            rule: "fail-closed".into(),
            reason: reason.into(),
            id: "dec-failclosed".into(),
            action_hash: None,
        }
    }

    /// An advisor (Jev) can only raise the outcome, never lower it (spec test 6).
    #[must_use]
    pub fn raised_by_advice(mut self, advice: Outcome) -> Self {
        if advice > self.outcome {
            self.outcome = advice;
            self.rule = "advisor-raise".into();
            self.reason = format!("{} (raised by the advisor)", self.reason);
        }
        self
    }
}

/// What the gateway needs to know about the session; the taint mark is also remembered on the
/// gateway side, so a request that arrives with it cleared does not clear it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub level: Level,
    #[serde(default)]
    pub tainted: bool,
    #[serde(default)]
    pub subagent: bool,
}

/// Budget state comes in as input; the gateway only checks it (S-47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub spent: u64,
    pub limit: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            spent: 0,
            limit: u64::MAX,
        }
    }
}

impl Budget {
    pub fn exhausted(&self) -> bool {
        self.spent >= self.limit
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRequest {
    pub tool: String,
    pub version: String,
    /// The description hash the caller saw (S-15).
    pub description_hash: String,
    /// Secrets travel as `{{secreto:name}}` markers inside the arguments.
    pub args: Map<String, Value>,
    pub node: String,
    pub session: SessionInfo,
    pub actor: String,
    #[serde(default)]
    pub budget: Budget,
    /// Tools the plan foresees (levels 2 and 3).
    #[serde(default)]
    pub plan: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgKind {
    Text,
    Bool,
    Int,
    /// Canonicalized and confined to the sandbox root (S-13).
    Path,
    /// Its host is the destination.
    Url,
    /// A binary plus already-split arguments, never a shell string (S-12).
    Argv,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgSpec {
    pub name: String,
    pub kind: ArgKind,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Egress {
    None,
    /// Can only reach these domains (and their subdomains).
    Domains(Vec<String>),
    /// The destination comes from the arguments; it is trusted only if it is in `trusted_domains`.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDecl {
    pub name: String,
    /// Pinned version (S-15).
    pub version: String,
    pub effect: Effect,
    pub egress: Egress,
    /// Hash of the tool description, pinned (S-15).
    pub description_hash: String,
    pub args: Vec<ArgSpec>,
}
