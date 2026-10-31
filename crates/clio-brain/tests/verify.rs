#![allow(clippy::unwrap_used)]

use clio_brain::verify::*;

fn pass() -> Signal {
    Signal::External(ExternalSignal::TestResult {
        name: "unit".into(),
        passed: true,
    })
}

fn opinion() -> Signal {
    Signal::ModelOpinion {
        model: "strong-1".into(),
        text: "looks right to me".into(),
    }
}

#[test]
fn external_signals_close_or_fail_a_criterion() {
    let mut c = Criterion::new("c1", "tests pass", Scope::Step);
    c.apply(pass());
    assert_eq!(c.status, Status::Closed);
    c.apply(Signal::External(ExternalSignal::Invariant {
        name: "no secrets".into(),
        holds: false,
    }));
    assert_eq!(c.status, Status::Failed);
    c.apply(Signal::External(ExternalSignal::ToolResult {
        tool: "build".into(),
        ok: true,
    }));
    assert_eq!(c.status, Status::Closed);
}

// Self-critique or a debate between models never closes a criterion.
#[test]
fn model_opinion_and_debate_never_close() {
    let mut c = Criterion::new("c1", "tests pass", Scope::Goal);
    for _ in 0..5 {
        c.apply(opinion());
    }
    assert_eq!(c.status, Status::Open);
    assert_eq!(c.opinions.len(), 5);
}

#[test]
fn goal_criterion_is_required_for_done() {
    let mut l = Checklist {
        criteria: vec![
            Criterion::new("s1", "step ok", Scope::Step),
            Criterion::new("g1", "goal met", Scope::Goal),
        ],
    };
    assert!(l.apply("s1", pass()));
    assert!(!l.is_done(), "all steps closed but the goal is still open");
    assert!(l.apply("g1", pass()));
    assert!(l.is_done());
    assert!(!l.apply("nope", pass()));
    // Only steps, no goal criterion: not done either.
    let mut only_steps = Checklist {
        criteria: vec![Criterion::new("s1", "step ok", Scope::Step)],
    };
    only_steps.apply("s1", pass());
    assert!(!only_steps.is_done());
}

#[test]
fn verifier_works_from_its_own_context() {
    let ctx = VerifierContext {
        goal: "feature works".into(),
        criterion: "tests pass".into(),
        evidence: vec![ExternalSignal::TestResult {
            name: "unit".into(),
            passed: true,
        }],
    };
    let mut c = Criterion::new("c1", "tests pass", Scope::Goal);
    c.apply(EvidenceVerifier.verify(&ctx));
    assert_eq!(c.status, Status::Closed);

    let empty = VerifierContext {
        evidence: vec![],
        ..ctx
    };
    let mut c2 = Criterion::new("c2", "tests pass", Scope::Goal);
    c2.apply(EvidenceVerifier.verify(&empty));
    assert_eq!(c2.status, Status::Open);
}

#[test]
fn subagent_result_is_an_observation_not_truth() {
    let o = Observation::from_subagent("researcher", "all tests pass, trust me");
    assert!(!o.trusted());
    let mut c = Criterion::new("c1", "tests pass", Scope::Goal);
    c.apply(o.into_signal());
    assert_eq!(c.status, Status::Open);
}
