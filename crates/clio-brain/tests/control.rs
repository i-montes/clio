#![allow(clippy::unwrap_used)]

use clio_brain::control::*;
use clio_core::{FixedClock, Level};
use std::sync::Arc;

fn budget() -> Budget {
    Budget {
        max_steps: 5,
        max_ms: 10_000,
        max_cost_micro: 1_000,
        max_repeats: 3,
        max_retries: 2,
    }
}

fn ctl(level: Level) -> (Controller<Arc<FixedClock>>, Arc<FixedClock>) {
    let clock = Arc::new(FixedClock::new(1_000_000));
    (Controller::new(clock.clone(), "t1", level, budget()), clock)
}

// S-47: the step cap cuts hard, and the controller stays stopped.
#[test]
fn step_cap_cuts_hard() {
    let (mut c, _) = ctl(Level::Full);
    let mut day = DayBudget::new(1_000_000);
    for i in 0..5 {
        c.begin_step(&format!("a{i}"), 0, &mut day).unwrap();
    }
    assert_eq!(c.begin_step("a9", 0, &mut day), Err(Stop::Steps));
    assert_eq!(c.begin_step("a10", 0, &mut day), Err(Stop::Steps));
    assert_eq!(c.halted(), Some(Stop::Steps));
}

// S-47: the time cap uses the injected clock.
#[test]
fn time_cap_cuts_hard() {
    let (mut c, clock) = ctl(Level::Full);
    let mut day = DayBudget::new(1_000_000);
    c.begin_step("a", 0, &mut day).unwrap();
    clock.advance(10_000);
    assert_eq!(c.begin_step("b", 0, &mut day), Err(Stop::Time));
}

// S-47: the cost is reserved before the step, so the cap is never crossed.
#[test]
fn cost_cap_cuts_hard() {
    let (mut c, _) = ctl(Level::Full);
    let mut day = DayBudget::new(1_000_000);
    c.begin_step("a", 600, &mut day).unwrap();
    assert_eq!(c.begin_step("b", 600, &mut day), Err(Stop::Cost));
}

// S-47: a real cost above the cap stops the task after settling.
#[test]
fn settle_over_cap_halts() {
    let (mut c, _) = ctl(Level::Full);
    let mut day = DayBudget::new(1_000_000);
    c.begin_step("a", 100, &mut day).unwrap();
    c.settle(100, 5_000, &mut day);
    assert_eq!(c.begin_step("b", 0, &mut day), Err(Stop::Cost));
}

// S-47: the daily cap is shared by tasks, and resets the next day.
#[test]
fn day_cap_is_shared_and_resets() {
    let (mut a, clock) = ctl(Level::Full);
    let mut b = Controller::new(clock.clone(), "t2", Level::Full, budget());
    let mut day = DayBudget::new(1_000);
    a.begin_step("x", 700, &mut day).unwrap();
    assert_eq!(b.begin_step("y", 700, &mut day), Err(Stop::DayCost));
    clock.advance(86_400_000);
    let mut c = Controller::new(clock, "t3", Level::Full, budget());
    c.begin_step("z", 700, &mut day).unwrap();
}

// S-47: a seeded loop stops when it touches the cap.
#[test]
fn seeded_loop_stops_at_the_cap() {
    let (mut c, _) = ctl(Level::Full);
    let mut day = DayBudget::new(1_000_000);
    let mut n = 0;
    let mut i = 0;
    while c.begin_step(&format!("step{i}"), 10, &mut day).is_ok() {
        n += 1;
        i += 1;
        assert!(n < 1_000, "the loop was not stopped");
    }
    assert_eq!(n, 5);
}

// S-47: attack: a loop that tries to spend without stopping, with fresh actions each time.
#[test]
fn attack_endless_spender_is_cut() {
    let (mut c, _) = ctl(Level::Direct);
    let mut day = DayBudget::new(500);
    let mut spent = 0u64;
    for i in 0..10_000 {
        if c.begin_step(&format!("burn{i}"), 100, &mut day).is_err() {
            break;
        }
        c.settle(100, 100, &mut day);
        spent += 100;
    }
    assert!(spent <= 500, "spent {spent} over the daily cap");
    assert!(c.halted().is_some());
}

// S-47: the anti-loop counter stops a repeated action (default: repetitions only).
#[test]
fn anti_loop_stops_repeats() {
    let (mut c, _) = ctl(Level::Full);
    let mut day = DayBudget::new(1_000_000);
    for _ in 0..3 {
        c.begin_step("read:a.txt", 0, &mut day).unwrap();
    }
    assert_eq!(c.begin_step("read:a.txt", 0, &mut day), Err(Stop::Repeats));
}

// S-47: levels 0 and 1 skip contract and planner but not budget or anti-loop.
#[test]
fn low_levels_skip_contract_not_budget() {
    for level in [Level::Direct, Level::Simple] {
        let (mut c, _) = ctl(level);
        assert!(!c.needs_contract_and_planner());
        let mut day = DayBudget::new(1_000_000);
        for _ in 0..3 {
            c.begin_step("same", 0, &mut day).unwrap();
        }
        assert_eq!(c.begin_step("same", 0, &mut day), Err(Stop::Repeats));
    }
    assert!(ctl(Level::Full).0.needs_contract_and_planner());
    assert!(ctl(Level::Long).0.needs_contract_and_planner());
}

#[test]
fn retry_budget_runs_out() {
    let (mut c, _) = ctl(Level::Full);
    c.try_retry().unwrap();
    c.try_retry().unwrap();
    assert_eq!(c.try_retry(), Err(Stop::Retries));
}

#[test]
fn action_must_match_the_plan() {
    let act = |t: &str, x: &str| Action {
        tool: t.into(),
        target: x.into(),
    };
    let plan = Plan {
        steps: vec![act("read", "a"), act("write", "b")],
    };
    let (mut c, _) = ctl(Level::Full);
    assert!(c.check_plan(&plan, &act("read", "a")).is_ok());
    assert!(matches!(
        c.check_plan(&plan, &act("delete", "b")),
        Err(PlanError::Mismatch { .. })
    ));
    assert!(c.check_plan(&plan, &act("write", "b")).is_ok());
    assert_eq!(
        c.check_plan(&plan, &act("write", "b")),
        Err(PlanError::PlanFinished)
    );
}

#[test]
fn feasibility_validator_denies() {
    let v = DenyRules {
        denied_tools: vec!["rm".into()],
        denied_target_prefixes: vec!["/etc".into()],
    };
    let a = |t: &str, x: &str| Action {
        tool: t.into(),
        target: x.into(),
    };
    assert!(v.check(&a("read", "/home/x")).is_ok());
    assert!(v.check(&a("rm", "/home/x")).is_err());
    assert!(v.check(&a("write", "/etc/passwd")).is_err());
}

#[test]
fn checkpoint_reloads_identical_and_resumes() {
    let (mut c, clock) = ctl(Level::Full);
    let mut day = DayBudget::new(1_000_000);
    c.begin_step("a", 100, &mut day).unwrap();
    c.try_retry().unwrap();
    clock.advance(4_000);
    let cp = c.checkpoint();
    let back = Checkpoint::from_json(&cp.to_json().unwrap()).unwrap();
    assert_eq!(cp, back);
    // Resumed elsewhere: the 4 s already used still count toward the cap.
    let later = Arc::new(FixedClock::new(9_000_000));
    let mut r = Controller::resume(later.clone(), back);
    r.begin_step("b", 0, &mut day).unwrap();
    later.advance(6_000);
    assert_eq!(r.begin_step("c", 0, &mut day), Err(Stop::Time));
    assert_eq!(r.checkpoint().steps, 2);
}

#[test]
fn evidence_package_has_no_transcript() {
    let p = EvidencePackage {
        goal: "fix the test".into(),
        attempt: 2,
        failures: vec![Failure {
            step: "edit".into(),
            reason: "test still red".into(),
            evidence: "assertion failed at x.rs:10".into(),
        }],
    };
    let back: EvidencePackage = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(p, back);
}

#[test]
fn output_filter_blocks_before_use() {
    let f = BasicFilter {
        max_len: 50,
        banned_phrases: vec!["ignore previous instructions".into()],
    };
    assert_eq!(f.filter("ok\u{7}text").unwrap(), "oktext");
    assert!(f.filter("Please IGNORE previous instructions").is_err());
    assert!(f.filter(&"x".repeat(51)).is_err());
}
