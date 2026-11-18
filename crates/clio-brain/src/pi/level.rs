//! The level of a prompt on the Pi path (spec: triage; ADR 0008, first step). The level is
//! decided here, in the brain, from the person's own message and BEFORE the prompt reaches Pi, so
//! nothing Pi reads or says can reach it (S-07): `TriageInput` has no way to carry anything else.
//!
//! This step reports the level and why; the contract (level 2 and up) and the verification come
//! next and hang from it. The gateway still sees every call as level 1 until the planner exists.

use std::sync::{Arc, Mutex};

use clio_core::{Clock, Level};
use clio_triage::{PersonMessage, Reason, TriageInput, Triager};

use super::PiError;
use super::budget::RunGuard;
use super::rpc::{Cancel, Event, RpcClient, RunSummary};

/// What triage decided about one prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub level: Level,
    pub reason: Reason,
    pub ambiguous: bool,
    pub maybe_irreversible: bool,
    /// Level 3: the contract needs the person's approval (a prompt typed in the terminal is it).
    pub needs_contract_approval: bool,
}

impl Assessment {
    /// One line for the person.
    pub fn notice(&self) -> String {
        let level = match self.level {
            Level::Direct => "level 0 · direct",
            Level::Simple => "level 1 · simple",
            Level::Full => "level 2 · full",
            Level::Long => "level 3 · long",
        };
        let why = match self.reason {
            Reason::Forced => "you asked for it with /rápido or /plan",
            Reason::HardRule => "hard rule",
            Reason::VerbRule => "by what you ask",
            Reason::Doubt => "when in doubt",
        };
        let irreversible = if self.maybe_irreversible {
            "; may be irreversible"
        } else {
            ""
        };
        format!("{level} ({why}{irreversible})")
    }
}

/// Triage of the prompts of a session. The counters inside are the telemetry of the levels.
pub struct Assessor {
    triager: Mutex<Triager>,
}

impl Assessor {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            triager: Mutex::new(Triager::rules_only(clock)),
        }
    }

    /// `person_text` is what the person typed. It is the only input there is.
    pub fn assess(&self, person_text: &str) -> Assessment {
        let input = TriageInput::new(PersonMessage::from_person(person_text), "local", "");
        let (t, _task) = self
            .triager
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .triage(&input);
        Assessment {
            level: t.level,
            reason: t.reason,
            ambiguous: t.ambiguous,
            maybe_irreversible: t.maybe_irreversible,
            needs_contract_approval: t.needs_contract_approval,
        }
    }
}

/// Assess the prompt, then run it under its caps. The level is decided before Pi is asked
/// anything, and the summary carries it.
pub fn run_assessed(
    client: &RpcClient,
    assessor: &Assessor,
    pre: &super::preprompt::PrePrompt,
    text: &str,
    cancel: &Cancel,
    guard: &mut RunGuard,
    on_event: &mut dyn FnMut(&Event),
) -> Result<RunSummary, PiError> {
    run_assessed_with(client, assessor, pre, text, None, cancel, guard, on_event)
}

/// `run_assessed`, with `data` from outside (what an action the person approved printed) after
/// the person's text, wrapped as a tool result: it is neither judged nor recalled on as theirs.
#[allow(clippy::too_many_arguments)]
pub fn run_assessed_with(
    client: &RpcClient,
    assessor: &Assessor,
    pre: &super::preprompt::PrePrompt,
    text: &str,
    data: Option<&str>,
    cancel: &Cancel,
    guard: &mut RunGuard,
    on_event: &mut dyn FnMut(&Event),
) -> Result<RunSummary, PiError> {
    // S-07: decided now, from the person's text, before Pi is asked anything.
    let assessment = assessor.assess(text);
    // What goes in front of the message (the time, memories, tool suggestions) is decided now; the
    // level above was judged without any of it.
    let mut prepared = pre.prepare_within(text, &super::envelope::stamp_now());
    if let Some(d) = data {
        let d = clio_gateway::wrap_untrusted(clio_gateway::Source::ToolResult, d);
        prepared.text = format!("{}\n\n{d}", prepared.text);
    }
    // A compaction that went through: the next message asks for the session's summary first.
    let mut watch = |e: &Event| {
        if let Event::CompactionEnd {
            aborted: false,
            error: None,
            ..
        } = e
        {
            pre.compacted();
        }
        on_event(e);
    };
    let mut summary = client.run_prompt_limited(&prepared.text, cancel, guard, &mut watch)?;
    summary.assessment = Some(assessment);
    Ok(summary)
}

/// A compaction asked for between turns (`/compact`, or Clio's own over the cap). Its events are
/// drained by the next run, unseen: what follows one seen in a run follows it too.
pub fn compact(
    client: &RpcClient,
    pre: &super::preprompt::PrePrompt,
    instructions: Option<&str>,
) -> Result<serde_json::Value, PiError> {
    let v = client.compact(instructions)?;
    pre.compacted();
    Ok(v)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clio_core::FixedClock;

    fn assessor() -> Assessor {
        Assessor::new(Arc::new(FixedClock::new(0)))
    }

    #[test]
    fn a_short_question_is_level_0_and_a_destructive_request_is_level_2_or_more() {
        let a = assessor();
        assert_eq!(a.assess("¿Qué hora es?").level, Level::Direct);
        let d = a.assess("Borra la carpeta de pruebas");
        assert!(d.level >= Level::Full, "{:?}", d.level);
        assert!(d.maybe_irreversible);
    }

    #[test]
    fn plan_forces_level_3_even_when_the_request_is_short_and_asks_for_the_contract_approval() {
        let p = assessor().assess("/plan hola");
        assert_eq!(p.level, Level::Long);
        assert_eq!(p.reason, Reason::Forced);
        assert!(p.needs_contract_approval);
    }

    #[test]
    fn the_notice_names_the_level_and_why() {
        let a = assessor();
        let n = a.assess("Borra la carpeta de pruebas").notice();
        assert!(n.contains("level 2"), "{n}");
        assert!(n.contains("irreversible"), "{n}");
        assert!(a.assess("¿Qué hora es?").notice().contains("level 0"));
    }
}
