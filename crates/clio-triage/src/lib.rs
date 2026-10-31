//! Rule-based triage: how much structure a request needs. No network, no LLM: the decider is a
//! trait and the rules decider is the default, so triage works without Jev.
//! The input type can only carry the person's message (S-07), never external content.

use clio_core::{Clock, Effect, Level};
use std::sync::Arc;

/// A message typed by the person. It is the only text triage accepts: pages, files and tool
/// results have no way into `TriageInput` (S-07). The caller builds it from the person's channel only.
#[derive(Debug, Clone)]
pub struct PersonMessage(String);

impl PersonMessage {
    pub fn from_person(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

#[derive(Debug, Clone)]
pub struct TriageInput {
    message: PersonMessage,
    node: String,
    identity: String,
}

impl TriageInput {
    pub fn new(
        message: PersonMessage,
        node: impl Into<String>,
        identity: impl Into<String>,
    ) -> Self {
        Self {
            message,
            node: node.into(),
            identity: identity.into(),
        }
    }
    pub fn node(&self) -> &str {
        &self.node
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn text(&self) -> &str {
        &self.message.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelClass {
    /// Fast and cheap (levels 0 and 1).
    Fast,
    /// Strong (levels 2 and 3).
    Strong,
}

/// Why a level was chosen; useful in the trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Forced,
    HardRule,
    VerbRule,
    Doubt,
}

/// The one-pass output of triage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Triage {
    pub level: Level,
    pub effort: Effort,
    pub ambiguous: bool,
    pub maybe_irreversible: bool,
    pub tools: Vec<String>,
    pub model: ModelClass,
    pub reason: Reason,
    /// True at level 3: the contract needs the person's approval.
    pub needs_contract_approval: bool,
}

/// Something that proposes a level from the input. Jev can implement it later; the rules
/// implementation below needs nothing else. `None` means "no opinion": triage falls back to level 1.
pub trait Decider: Send + Sync {
    fn decide(&self, input: &TriageInput) -> Option<Level>;
}

const IRREVERSIBLE: &[&str] = &[
    "borra",
    "borrar",
    "elimina",
    "eliminar",
    "despliega",
    "desplegar",
    "formatea",
    "envía",
    "envia",
    "publica",
    "paga",
    "transfiere",
    "delete",
    "deploy",
    "drop",
    "rm",
    "push",
    "send",
    "pay",
];
const HEAVY: &[&str] = &[
    "crea",
    "crear",
    "borra",
    "borrar",
    "despliega",
    "desplegar",
    "create",
    "delete",
    "deploy",
];
const ACTION: &[&str] = &[
    "escribe",
    "edita",
    "modifica",
    "cambia",
    "instala",
    "implementa",
    "refactoriza",
    "mueve",
    "renombra",
    "guarda",
    "actualiza",
    "arregla",
    "corrige",
    "ejecuta",
    "write",
    "edit",
    "install",
    "run",
    "fix",
];
const LONG_WORDS: usize = 120;

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn has_any(ws: &[String], list: &[&str]) -> bool {
    ws.iter().any(|w| list.contains(&w.as_str()))
}

/// The default decider: verbs and length, nothing else.
pub struct RulesDecider;

impl Decider for RulesDecider {
    fn decide(&self, input: &TriageInput) -> Option<Level> {
        let ws = words(input.text());
        if has_any(&ws, HEAVY) {
            return Some(if ws.len() >= LONG_WORDS {
                Level::Long
            } else {
                Level::Full
            });
        }
        if ws.len() >= LONG_WORDS {
            return Some(Level::Long);
        }
        if has_any(&ws, ACTION) {
            return Some(Level::Simple);
        }
        // shortcut: "short question" is length plus no action verb; a classifier replaces this when Jev arrives.
        if ws.len() <= 12 {
            return Some(Level::Direct);
        }
        None
    }
}

/// Lowest level a tool effect allows (hard rule: possible irreversible => at least 2).
pub fn min_level_for(effect: Effect) -> Level {
    match effect {
        Effect::Read => Level::Direct,
        Effect::LocalReversible => Level::Simple,
        Effect::ExternalWrite | Effect::Irreversible => Level::Full,
    }
}

fn idx(l: Level) -> usize {
    l as usize
}

/// Counters only; no text from the person is kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Telemetry {
    pub requests_by_level: [u64; 4],
    pub escalations: u64,
    pub low_level_failures: u64,
    pub cost_by_level: [u64; 4],
}

/// Record of one rise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escalation {
    pub from: Level,
    pub to: Level,
    pub why: EscalationReason,
    pub at_ms: i64,
    pub needs_contract_approval: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationReason {
    ToolOutsideLevel(Effect),
    BudgetExhausted,
    IrreversibleFound,
    TooManySteps,
}

/// A running task's level: it only goes up, and every rise is logged.
#[derive(Debug, Clone)]
pub struct Session {
    level: Level,
    log: Vec<Escalation>,
}

impl Session {
    pub fn level(&self) -> Level {
        self.level
    }
    pub fn log(&self) -> &[Escalation] {
        &self.log
    }
}

pub struct Triager {
    decider: Box<dyn Decider>,
    clock: Arc<dyn Clock>,
    telemetry: Telemetry,
}

impl Triager {
    pub fn new(decider: Box<dyn Decider>, clock: Arc<dyn Clock>) -> Self {
        Self {
            decider,
            clock,
            telemetry: Telemetry::default(),
        }
    }

    /// Triage with the rules decider only (no Jev).
    pub fn rules_only(clock: Arc<dyn Clock>) -> Self {
        Self::new(Box::new(RulesDecider), clock)
    }

    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    pub fn triage(&mut self, input: &TriageInput) -> (Triage, Session) {
        let ws = words(input.text());
        let maybe_irreversible = has_any(&ws, IRREVERSIBLE);
        let raw = input.text().trim_start().to_lowercase();
        let (level, reason) = if raw.starts_with("/rápido") || raw.starts_with("/rapido") {
            (Level::Simple, Reason::Forced)
        } else if raw.starts_with("/plan") {
            (Level::Long, Reason::Forced)
        } else {
            let decided = self.decider.decide(input);
            match (decided, maybe_irreversible) {
                (d, true) if d.is_none_or(|l| l < Level::Full) => (Level::Full, Reason::HardRule),
                (Some(l), _) => (l, Reason::VerbRule),
                (None, _) => (Level::Simple, Reason::Doubt),
            }
        };
        let ambiguous = reason == Reason::Doubt;
        let tools = if ws.iter().any(|w| matches!(w.as_str(), "hora" | "time")) {
            vec!["time".to_owned()]
        } else {
            Vec::new()
        };
        let out = Triage {
            level,
            effort: match level {
                Level::Direct | Level::Simple => Effort::Low,
                Level::Full => Effort::Medium,
                Level::Long => Effort::High,
            },
            ambiguous,
            maybe_irreversible,
            tools,
            model: if level >= Level::Full {
                ModelClass::Strong
            } else {
                ModelClass::Fast
            },
            reason,
            needs_contract_approval: level == Level::Long,
        };
        self.telemetry.requests_by_level[idx(level)] += 1;
        (
            out,
            Session {
                level,
                log: Vec::new(),
            },
        )
    }

    /// Ask for a higher level. Lower or equal levels change nothing (a task never goes down).
    pub fn escalate(
        &mut self,
        s: &mut Session,
        to: Level,
        why: EscalationReason,
    ) -> Option<Escalation> {
        if to <= s.level {
            return None;
        }
        let e = Escalation {
            from: s.level,
            to,
            why,
            at_ms: self.clock.now_ms(),
            needs_contract_approval: to == Level::Long,
        };
        s.level = to;
        s.log.push(e.clone());
        self.telemetry.escalations += 1;
        Some(e)
    }

    /// The task needs a tool with this effect: rise to the level that allows it.
    pub fn need_effect(&mut self, s: &mut Session, effect: Effect) -> Option<Escalation> {
        let why = if effect == Effect::Irreversible {
            EscalationReason::IrreversibleFound
        } else {
            EscalationReason::ToolOutsideLevel(effect)
        };
        self.escalate(s, min_level_for(effect), why)
    }

    /// A failure traced to a level that was too low.
    pub fn record_low_level_failure(&mut self) {
        self.telemetry.low_level_failures += 1;
    }

    pub fn add_cost(&mut self, level: Level, micro_cents: u64) {
        self.telemetry.cost_by_level[idx(level)] += micro_cents;
    }
}
