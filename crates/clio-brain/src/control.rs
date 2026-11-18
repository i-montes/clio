//! Control layer: budgets cut hard in code (S-47), anti-loop, retries, checkpoints, plan check.

use clio_core::{Clock, Level};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DAY_MS: i64 = 86_400_000;

/// Why a task was stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Steps,
    Time,
    Cost,
    DayCost,
    DaySteps,
    Repeats,
    Retries,
}

impl Stop {
    /// Why, in words for the person (it reaches the screen).
    pub fn reason(self) -> &'static str {
        match self {
            Self::Steps => "tope de pasos de la tarea",
            Self::Time => "tope de tiempo de la tarea",
            Self::Cost => "tope de costo de la tarea",
            Self::DayCost | Self::DaySteps => "presupuesto del día agotado: sigo mañana",
            Self::Repeats => "la misma acción se repitió demasiadas veces (posible bucle)",
            Self::Retries => "tope de reintentos de la tarea",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub max_steps: u32,
    pub max_ms: i64,
    pub max_cost_micro: u64,
    /// Times one action may run in a task (decision: only repetitions count, not "no progress").
    pub max_repeats: u32,
    pub max_retries: u32,
}

/// Spend and steps per UTC day, shared by every task. Resets when the day changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayBudget {
    pub max_cost_micro: u64,
    #[serde(default = "unlimited")]
    pub max_steps: u64,
    day: i64,
    spent_micro: u64,
    #[serde(default)]
    steps: u64,
    /// Where the day is kept between sessions (`kept`); `None`: in this process only.
    #[serde(skip)]
    file: Option<PathBuf>,
}

fn unlimited() -> u64 {
    u64::MAX
}

/// The day of a budget whose file is there but cannot be read: it keeps its spent state through
/// the first day it sees, and starts at zero only the day after.
const UNREADABLE: i64 = i64::MAX;

impl DayBudget {
    pub fn new(max_cost_micro: u64) -> Self {
        Self {
            max_cost_micro,
            max_steps: u64::MAX,
            day: i64::MIN,
            spent_micro: 0,
            steps: 0,
            file: None,
        }
    }

    /// The day as `file` keeps it, with these caps, saved there by `save`. No file starts the day
    /// at zero; a file that is there but cannot be read starts it spent (a broken file must not
    /// hand out a fresh budget).
    /// shortcut: two sessions open at once each keep their own count and the last save wins; a
    /// lock around load-add-save if they ever run side by side for long.
    pub fn kept(file: &Path, max_cost_micro: u64, max_steps: u64) -> Self {
        let mut d = match std::fs::read_to_string(file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::new(0),
            read => read
                .ok()
                .and_then(|t| serde_json::from_str::<Self>(&t).ok())
                .unwrap_or(Self {
                    day: UNREADABLE,
                    spent_micro: max_cost_micro,
                    steps: max_steps,
                    ..Self::new(0)
                }),
        };
        d.max_cost_micro = max_cost_micro;
        d.max_steps = max_steps;
        d.file = Some(file.to_path_buf());
        d
    }

    /// Best effort: a day that cannot be saved still counts in this process.
    pub fn save(&self) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Some(file) = &self.file else { return };
        let Ok(text) = serde_json::to_string(self) else {
            return;
        };
        let tmp = file.with_extension("tmp");
        let _ = file
            .parent()
            .map_or(Ok(()), |d| {
                crate::pi::private_dir(d).map_err(|e| std::io::Error::other(e.to_string()))
            })
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&tmp)
            })
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .and_then(|()| std::fs::rename(&tmp, file));
    }

    pub fn spent(&mut self, now_ms: i64) -> u64 {
        let d = now_ms.div_euclid(DAY_MS);
        if self.day == UNREADABLE {
            self.day = d;
        } else if d != self.day {
            self.day = d;
            self.spent_micro = 0;
            self.steps = 0;
        }
        self.spent_micro
    }

    /// The steps of the day so far (after `spent` saw the time).
    pub fn steps(&self) -> u64 {
        self.steps
    }
}

/// The state that survives a restart; plain data, so a reload is identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub task_id: String,
    pub level: Level,
    pub budget: Budget,
    pub steps: u32,
    pub cost_micro: u64,
    pub elapsed_ms: i64,
    pub retries: u32,
    pub action_counts: BTreeMap<String, u32>,
    /// Position in the plan, when there is one.
    pub plan_pos: usize,
}

impl Checkpoint {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

pub struct Controller<C: Clock> {
    clock: C,
    started_ms: i64,
    state: Checkpoint,
    halted: Option<Stop>,
}

impl<C: Clock> Controller<C> {
    pub fn new(clock: C, task_id: &str, level: Level, budget: Budget) -> Self {
        let started_ms = clock.now_ms();
        let state = Checkpoint {
            task_id: task_id.into(),
            level,
            budget,
            steps: 0,
            cost_micro: 0,
            elapsed_ms: 0,
            retries: 0,
            action_counts: BTreeMap::new(),
            plan_pos: 0,
        };
        Self {
            clock,
            started_ms,
            state,
            halted: None,
        }
    }

    /// Continue a task from a checkpoint; the time already used still counts.
    pub fn resume(clock: C, state: Checkpoint) -> Self {
        let started_ms = clock.now_ms() - state.elapsed_ms;
        Self {
            clock,
            started_ms,
            state,
            halted: None,
        }
    }

    pub fn checkpoint(&self) -> Checkpoint {
        let mut c = self.state.clone();
        c.elapsed_ms = self.clock.now_ms() - self.started_ms;
        c
    }

    pub fn halted(&self) -> Option<Stop> {
        self.halted
    }

    /// Levels 0 and 1 skip contract and planner. Nothing else in this type looks at the level:
    /// budget and anti-loop apply to every level.
    pub fn needs_contract_and_planner(&self) -> bool {
        self.state.level >= Level::Full
    }

    fn stop(&mut self, s: Stop) -> Result<(), Stop> {
        self.halted = Some(s);
        Err(s)
    }

    /// The hard limits, checked without counting a step: call it before spending anything (a
    /// model call) when the action is not known yet. Once a limit is hit the controller stays stopped.
    pub fn check_limits(&mut self, est_cost_micro: u64, day: &mut DayBudget) -> Result<(), Stop> {
        if let Some(s) = self.halted {
            return Err(s);
        }
        let now = self.clock.now_ms();
        let b = self.state.budget;
        if self.state.steps >= b.max_steps {
            return self.stop(Stop::Steps);
        }
        if now - self.started_ms >= b.max_ms {
            return self.stop(Stop::Time);
        }
        if self.state.cost_micro.saturating_add(est_cost_micro) > b.max_cost_micro {
            return self.stop(Stop::Cost);
        }
        if day.spent(now).saturating_add(est_cost_micro) > day.max_cost_micro {
            return self.stop(Stop::DayCost);
        }
        if day.steps >= day.max_steps {
            return self.stop(Stop::DaySteps);
        }
        Ok(())
    }

    /// Call before every step. `est_cost_micro` is reserved up front so the cap is never crossed.
    /// Once a limit is hit the controller stays stopped.
    pub fn begin_step(
        &mut self,
        action: &str,
        est_cost_micro: u64,
        day: &mut DayBudget,
    ) -> Result<(), Stop> {
        self.check_limits(est_cost_micro, day)?;
        let b = self.state.budget;
        let n = self.state.action_counts.entry(action.into()).or_insert(0);
        if *n >= b.max_repeats {
            return self.stop(Stop::Repeats);
        }
        *n += 1;
        self.state.steps += 1;
        day.steps += 1;
        // Charge the reservation now; `settle` corrects it when the real cost is known.
        self.state.cost_micro += est_cost_micro;
        day.spent_micro += est_cost_micro;
        Ok(())
    }

    /// Replace the reservation with the real cost. A real cost above the cap stops the task.
    pub fn settle(&mut self, est_cost_micro: u64, real_cost_micro: u64, day: &mut DayBudget) {
        // Saturating: the real cost comes from numbers the adapter or Pi report, and a sum that wraps
        // (release) or panics (debug) would let a huge cost slip under the cap.
        self.state.cost_micro = self
            .state
            .cost_micro
            .saturating_sub(est_cost_micro)
            .saturating_add(real_cost_micro);
        day.spent_micro = day
            .spent_micro
            .saturating_sub(est_cost_micro)
            .saturating_add(real_cost_micro);
        if self.state.cost_micro > self.state.budget.max_cost_micro {
            self.halted = Some(Stop::Cost);
        } else if day.spent_micro > day.max_cost_micro {
            self.halted = Some(Stop::DayCost);
        }
    }

    pub fn try_retry(&mut self) -> Result<(), Stop> {
        if self.state.retries >= self.state.budget.max_retries {
            return self.stop(Stop::Retries);
        }
        self.state.retries += 1;
        Ok(())
    }

    /// Check `action` against the plan before it runs; advances the plan on a match.
    pub fn check_plan(&mut self, plan: &Plan, action: &Action) -> Result<(), PlanError> {
        match plan.steps.get(self.state.plan_pos) {
            None => Err(PlanError::PlanFinished),
            Some(want) if want == action => {
                self.state.plan_pos += 1;
                Ok(())
            }
            Some(want) => Err(PlanError::Mismatch {
                expected: want.clone(),
                got: action.clone(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub tool: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    PlanFinished,
    Mismatch { expected: Action, got: Action },
}

/// Can the rules allow this action at all? Runs on every action.
pub trait FeasibilityValidator {
    fn check(&self, action: &Action) -> Result<(), String>;
}

/// Simple validator: a list of denied tools and a list of denied target prefixes.
pub struct DenyRules {
    pub denied_tools: Vec<String>,
    pub denied_target_prefixes: Vec<String>,
}

impl FeasibilityValidator for DenyRules {
    fn check(&self, a: &Action) -> Result<(), String> {
        if self.denied_tools.contains(&a.tool) {
            return Err(format!("tool {} is not allowed", a.tool));
        }
        if let Some(p) = self
            .denied_target_prefixes
            .iter()
            .find(|p| a.target.starts_with(p.as_str()))
        {
            return Err(format!("target under {p} is not allowed"));
        }
        Ok(())
    }
}

/// What a fresh-context retry gets instead of the old transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidencePackage {
    pub goal: String,
    pub attempt: u32,
    pub failures: Vec<Failure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub step: String,
    pub reason: String,
    pub evidence: String,
}

/// Model output is untrusted until a filter lets it through.
pub trait OutputFilter {
    fn filter(&self, output: &str) -> Result<String, String>;
}

/// Example filter: length cap, no control characters, no known injection phrases.
pub struct BasicFilter {
    pub max_len: usize,
    pub banned_phrases: Vec<String>,
}

impl OutputFilter for BasicFilter {
    fn filter(&self, output: &str) -> Result<String, String> {
        if output.len() > self.max_len {
            return Err("output too long".into());
        }
        let low = output.to_lowercase();
        if let Some(p) = self
            .banned_phrases
            .iter()
            .find(|p| low.contains(p.as_str()))
        {
            return Err(format!("output contains {p:?}"));
        }
        Ok(output
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect())
    }
}
