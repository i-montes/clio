//! The control layer on the Pi path (S-47): one prompt is one task, and a task has hard caps on
//! tool calls, time and cost, plus an anti-loop for the same call repeated. The caps are the
//! `Controller` of the builtin loop, fed from Pi's events instead of from the model's turns, so
//! both harnesses stop for the same reasons. Budget and anti-loop apply to every level: Pi runs
//! everything as level 1 (no contract, no planner), and that never skips them.
//!
//! Stopping is a request: [`rpc::RpcClient::run_prompt_limited`] aborts Pi as soon as a cap is
//! reached and, if Pi does not settle in the grace time, kills it. The abort is not instant: a call
//! that Pi had already started when it lands still runs. That is the residual of cutting from the
//! outside; a count inside the toolhost would close it.

use std::sync::{Arc, Mutex};

use clio_core::{Clock, Level};

use super::rpc::Event;
use crate::control::{Budget, Controller, DayBudget, Stop};

/// Micro-USD per dollar: Pi reports cost in dollars.
const MICRO: f64 = 1_000_000.0;

pub struct RunGuard {
    ctl: Controller<Arc<dyn Clock>>,
    day: Arc<Mutex<DayBudget>>,
}

impl RunGuard {
    pub fn new(
        clock: Arc<dyn Clock>,
        task_id: &str,
        budget: Budget,
        day: Arc<Mutex<DayBudget>>,
    ) -> Self {
        Self {
            ctl: Controller::new(clock, task_id, Level::Simple, budget),
            day,
        }
    }

    /// Counts what one Pi event spends. `Some` is the reason to stop; once a cap is hit every
    /// later call answers the same.
    pub fn observe(&mut self, event: &Event) -> Option<Stop> {
        if let Some(s) = self.ctl.halted() {
            return Some(s);
        }
        let mut day = self.day.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            // A step is a tool call, as in the builtin loop (one action per step); the key is the
            // same, so the same call with the same arguments is the same action.
            Event::ToolStart { name, args, .. } => self
                .ctl
                .begin_step(&format!("{name}:{args}"), 0, &mut day)
                .err(),
            Event::MessageEnd(m) if m.role == "assistant" => {
                if let Some(u) = &m.usage {
                    // `max` also turns a NaN into 0; the cast saturates.
                    self.ctl
                        .settle(0, (u.cost.max(0.0) * MICRO).round() as u64, &mut day);
                }
                self.ctl.halted()
            }
            _ => None,
        }
    }

    /// The caps that move without events (time). Call it on every pass of the wait loop.
    pub fn tick(&mut self) -> Option<Stop> {
        let mut day = self.day.lock().unwrap_or_else(|e| e.into_inner());
        self.ctl.check_limits(0, &mut day).err()
    }

    pub fn stopped(&self) -> Option<Stop> {
        self.ctl.halted()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::pi::rpc::{MessageInfo, Usage};
    use clio_core::FixedClock;
    use serde_json::json;

    fn budget() -> Budget {
        Budget {
            max_steps: 3,
            max_ms: 60_000,
            max_cost_micro: 1_000_000,
            max_repeats: 2,
            max_retries: 3,
        }
    }

    fn guard_with(b: Budget, clock: Arc<FixedClock>, day: Arc<Mutex<DayBudget>>) -> RunGuard {
        RunGuard::new(clock, "t", b, day)
    }

    fn guard(b: Budget) -> RunGuard {
        let day = Arc::new(Mutex::new(DayBudget::new(u64::MAX)));
        guard_with(b, Arc::new(FixedClock::new(0)), day)
    }

    fn tool(name: &str, path: &str) -> Event {
        Event::ToolStart {
            id: "c".into(),
            name: name.into(),
            args: json!({ "path": path }),
        }
    }

    fn message_costing(dollars: f64) -> Event {
        Event::MessageEnd(MessageInfo {
            role: "assistant".into(),
            usage: Some(Usage {
                cost: dollars,
                ..Usage::default()
            }),
            ..MessageInfo::default()
        })
    }

    // S-47: the tool-call cap cuts at the first call over it, and stays cut.
    #[test]
    fn the_tool_call_cap_stops_the_run_at_the_first_call_over_it() {
        let mut g = guard(budget());
        for i in 0..3 {
            assert_eq!(g.observe(&tool("read", &format!("f{i}"))), None);
        }
        assert_eq!(g.observe(&tool("read", "f3")), Some(Stop::Steps));
        assert_eq!(g.stopped(), Some(Stop::Steps));
        assert_eq!(g.observe(&message_costing(0.0)), Some(Stop::Steps));
    }

    // S-47: the same call over and over is a loop, different calls are not.
    #[test]
    fn the_same_call_repeated_is_a_loop_and_different_calls_are_not() {
        let mut g = guard(Budget {
            max_steps: 100,
            ..budget()
        });
        assert_eq!(g.observe(&tool("read", "a")), None);
        assert_eq!(g.observe(&tool("read", "b")), None);
        assert_eq!(g.observe(&tool("read", "a")), None);
        assert_eq!(g.observe(&tool("read", "a")), Some(Stop::Repeats));
    }

    // S-47: the cost is the real one Pi reports for each message, in dollars.
    #[test]
    fn the_cost_cap_counts_what_each_message_really_cost() {
        let mut g = guard(Budget {
            max_cost_micro: 250_000,
            ..budget()
        });
        assert_eq!(g.observe(&message_costing(0.1)), None);
        assert_eq!(g.observe(&message_costing(0.1)), None);
        assert_eq!(g.observe(&message_costing(0.1)), Some(Stop::Cost));
    }

    // S-47: time is cut by the clock even if Pi sends nothing.
    #[test]
    fn the_time_cap_stops_a_silent_run() {
        let clock = Arc::new(FixedClock::new(1_000));
        let day = Arc::new(Mutex::new(DayBudget::new(u64::MAX)));
        let mut g = guard_with(budget(), clock.clone(), day);
        assert_eq!(g.tick(), None);
        clock.advance(60_000);
        assert_eq!(g.tick(), Some(Stop::Time));
    }

    // S-47: the day's cap is shared by the runs of a session, not per prompt.
    #[test]
    fn the_day_cap_is_shared_by_the_runs_that_use_the_same_day() {
        let clock = Arc::new(FixedClock::new(0));
        let day = Arc::new(Mutex::new(DayBudget::new(150_000)));
        let mut first = guard_with(budget(), clock.clone(), day.clone());
        assert_eq!(first.observe(&message_costing(0.1)), None);
        let mut second = guard_with(budget(), clock, day);
        assert_eq!(second.observe(&message_costing(0.1)), Some(Stop::DayCost));
    }

    // Other events spend nothing: text, tool ends, a user message.
    #[test]
    fn events_that_spend_nothing_are_not_counted() {
        let mut g = guard(budget());
        for _ in 0..10 {
            assert_eq!(g.observe(&Event::TurnStart), None);
            assert_eq!(
                g.observe(&Event::ToolEnd {
                    id: "c".into(),
                    name: "read".into(),
                    result: json!({}),
                    is_error: false,
                    duration_ms: None
                }),
                None
            );
        }
        assert_eq!(g.stopped(), None);
    }
}
