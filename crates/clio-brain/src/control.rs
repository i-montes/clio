//! Control layer: budgets cut hard in code (S-47), anti-loop, retries, checkpoints, plan check.

use clio_core::{Clock, Level};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const DAY_MS: i64 = 86_400_000;

/// Why a task was stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Steps,
    Time,
    Cost,
    DayCost,
    Repeats,
    Retries,
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

/// Spend per UTC day, shared by every task. Resets when the day changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayBudget {
    pub max_cost_micro: u64,
    day: i64,
    spent_micro: u64,
}

impl DayBudget {
    pub fn new(max_cost_micro: u64) -> Self {
        Self {
            max_cost_micro,
            day: i64::MIN,
            spent_micro: 0,
        }
    }

    pub fn spent(&mut self, now_ms: i64) -> u64 {
        let d = now_ms.div_euclid(DAY_MS);
        if d != self.day {
            self.day = d;
            self.spent_micro = 0;
        }
        self.spent_micro
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
        // Charge the reservation now; `settle` corrects it when the real cost is known.
        self.state.cost_micro += est_cost_micro;
        day.spent_micro += est_cost_micro;
        Ok(())
    }

    /// Replace the reservation with the real cost. A real cost above the cap stops the task.
    pub fn settle(&mut self, est_cost_micro: u64, real_cost_micro: u64, day: &mut DayBudget) {
        self.state.cost_micro =
            self.state.cost_micro.saturating_sub(est_cost_micro) + real_cost_micro;
        day.spent_micro = day.spent_micro.saturating_sub(est_cost_micro) + real_cost_micro;
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
