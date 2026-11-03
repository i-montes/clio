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
            Level::Direct => "nivel 0 · directo",
            Level::Simple => "nivel 1 · simple",
            Level::Full => "nivel 2 · completo",
            Level::Long => "nivel 3 · largo",
        };
        let why = match self.reason {
            Reason::Forced => "lo pediste con /rápido o /plan",
            Reason::HardRule => "regla dura",
            Reason::VerbRule => "por lo que pides",
            Reason::Doubt => "ante la duda",
        };
        let irreversible = if self.maybe_irreversible {
            "; puede ser irreversible"
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
    text: &str,
    cancel: &Cancel,
    guard: &mut RunGuard,
    on_event: &mut dyn FnMut(&Event),
) -> Result<RunSummary, PiError> {
    // S-07: decided now, from the person's text, before Pi is asked anything.
    let assessment = assessor.assess(text);
    let mut summary = client.run_prompt_limited(text, cancel, guard, on_event)?;
    summary.assessment = Some(assessment);
    Ok(summary)
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
        assert!(n.contains("nivel 2"), "{n}");
        assert!(n.contains("irreversible"), "{n}");
        assert!(a.assess("¿Qué hora es?").notice().contains("nivel 0"));
    }
}
