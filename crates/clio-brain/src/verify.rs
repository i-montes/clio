//! Verification: a criterion closes only with an external signal. Model opinion never does.

use serde::{Deserialize, Serialize};

/// Proof that comes from outside the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalSignal {
    TestResult { name: String, passed: bool },
    ToolResult { tool: String, ok: bool },
    Invariant { name: String, holds: bool },
}

impl ExternalSignal {
    fn passed(&self) -> bool {
        match self {
            Self::TestResult { passed, .. } => *passed,
            Self::ToolResult { ok, .. } => *ok,
            Self::Invariant { holds, .. } => *holds,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    External(ExternalSignal),
    /// Self-critique, a second model's view or a debate: kept as a note, never evidence.
    ModelOpinion {
        model: String,
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Step,
    /// Checked against the final goal, not only step by step.
    Goal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Open,
    Closed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Criterion {
    pub id: String,
    pub text: String,
    pub scope: Scope,
    pub status: Status,
    pub opinions: Vec<String>,
}

impl Criterion {
    pub fn new(id: &str, text: &str, scope: Scope) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            scope,
            status: Status::Open,
            opinions: Vec::new(),
        }
    }

    /// External signals decide; an opinion is recorded and the status stays as it was.
    pub fn apply(&mut self, signal: Signal) {
        match signal {
            Signal::External(e) => {
                self.status = if e.passed() {
                    Status::Closed
                } else {
                    Status::Failed
                };
            }
            Signal::ModelOpinion { model, text } => self.opinions.push(format!("{model}: {text}")),
        }
    }
}

#[derive(Debug, Default)]
pub struct Checklist {
    pub criteria: Vec<Criterion>,
}

impl Checklist {
    /// False when no criterion has that id.
    pub fn apply(&mut self, id: &str, signal: Signal) -> bool {
        match self.criteria.iter_mut().find(|c| c.id == id) {
            Some(c) => {
                c.apply(signal);
                true
            }
            None => false,
        }
    }

    /// Done only when every criterion is closed and at least one is against the final goal.
    pub fn is_done(&self) -> bool {
        self.criteria.iter().any(|c| c.scope == Scope::Goal)
            && self.criteria.iter().all(|c| c.status == Status::Closed)
    }
}

/// Something seen, not something true: tool output, a subagent's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub source: String,
    pub content: String,
}

impl Observation {
    pub fn from_subagent(name: &str, content: &str) -> Self {
        Self {
            source: format!("subagent:{name}"),
            content: content.into(),
        }
    }

    /// An observation is never trusted by itself.
    pub fn trusted(&self) -> bool {
        false
    }

    /// The only signal an observation can become is an opinion, which cannot close a criterion.
    pub fn into_signal(self) -> Signal {
        Signal::ModelOpinion {
            model: self.source,
            text: self.content,
        }
    }
}

/// What the verifier sees: the goal, the criterion and external evidence. There is no field for
/// the builder's conversation, so the two contexts cannot be shared.
#[derive(Debug, Clone)]
pub struct VerifierContext {
    pub goal: String,
    pub criterion: String,
    pub evidence: Vec<ExternalSignal>,
}

/// A separate component with its own context; it returns a signal, it does not edit the checklist.
pub trait Verifier {
    fn verify(&self, ctx: &VerifierContext) -> Signal;
}

/// Example verifier: reports the first external evidence, else an opinion that cannot close.
pub struct EvidenceVerifier;

impl Verifier for EvidenceVerifier {
    fn verify(&self, ctx: &VerifierContext) -> Signal {
        match ctx.evidence.first() {
            Some(e) => Signal::External(e.clone()),
            None => Signal::ModelOpinion {
                model: "none".into(),
                text: "no external evidence".into(),
            },
        }
    }
}
