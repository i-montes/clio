use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::rc::Rc;

use clio_canaries::Canaries;
use clio_trace::{Attr, Error, FixedClock, Kind, Status, Trace, Traceparent, Usage, names};
use rusqlite::Connection;

fn db(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("clio-trace-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn usage(input: u32, output: u32) -> Usage<'static> {
    Usage {
        provider: "acme",
        model: "fast-2026-01",
        input_tokens: input,
        output_tokens: output,
    }
}

fn no_attrs() -> BTreeMap<String, Attr> {
    BTreeMap::new()
}

/// A trace with a clock the test can move and a price of $3 in / $15 out per million tokens.
fn setup(name: &str) -> (Trace, Rc<FixedClock>, PathBuf) {
    let path = db(name);
    let clock = Rc::new(FixedClock::new(1_000));
    let trace = Trace::open(&path, clock.clone()).expect("open");
    trace
        .set_price("acme", "fast-2026-01", 0, 3_000_000, 15_000_000)
        .expect("price");
    (trace, clock, path)
}

#[test]
fn cost_is_computed_in_whole_micro_dollars() {
    let (t, _, path) = setup("cost");
    let id = t.new_trace_id();
    let span = t
        .start_span(&id, None, Kind::Model, "answer")
        .expect("start");
    // 2000 in * 3 + 1000 out * 15 = 21 000 micro-USD
    t.end_span(&span, Status::Ok, Some(usage(2_000, 1_000)), &no_attrs())
        .expect("end");
    let s = t.summary(&id).expect("summary");
    assert_eq!(
        (s.cost_micro_usd, s.input_tokens, s.output_tokens),
        (21_000, 2_000, 1_000)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_new_price_does_not_change_the_cost_of_older_spans() {
    let (t, clock, path) = setup("price");
    let id = t.new_trace_id();
    let old = t.start_span(&id, None, Kind::Model, "old").expect("start");
    t.end_span(&old, Status::Ok, Some(usage(1_000_000, 0)), &no_attrs())
        .expect("end");
    clock.advance(10_000);
    t.set_price(
        "acme",
        "fast-2026-01",
        clock_now(&clock),
        6_000_000,
        30_000_000,
    )
    .expect("new price");
    let new = t.start_span(&id, None, Kind::Model, "new").expect("start");
    t.end_span(&new, Status::Ok, Some(usage(1_000_000, 0)), &no_attrs())
        .expect("end");
    assert_eq!(
        t.summary(&id).expect("summary").cost_micro_usd,
        3_000_000 + 6_000_000
    );
    let _ = std::fs::remove_file(path);
}

fn clock_now(c: &FixedClock) -> i64 {
    use clio_trace::Clock;
    c.now_ms()
}

#[test]
fn a_model_with_no_price_is_an_error_not_a_zero_cost() {
    let (t, _, path) = setup("noprice");
    let id = t.new_trace_id();
    let span = t
        .start_span(&id, None, Kind::Model, "answer")
        .expect("start");
    let unknown = Usage {
        provider: "acme",
        model: "mystery",
        input_tokens: 1,
        output_tokens: 1,
    };
    assert!(matches!(
        t.end_span(&span, Status::Ok, Some(unknown), &no_attrs()),
        Err(Error::NoPrice { .. })
    ));
    let _ = std::fs::remove_file(path);
}

#[test]
fn two_runs_with_a_fixed_clock_give_the_same_summary() {
    let run = |name: &str| {
        let (t, clock, path) = setup(name);
        let id = t.new_trace_id();
        let agent = t.start_span(&id, None, Kind::Agent, "task").expect("agent");
        let model = t.child(&agent, Kind::Model, "answer").expect("model");
        clock.advance(250);
        t.end_span(&model, Status::Ok, Some(usage(500, 100)), &no_attrs())
            .expect("end model");
        clock.advance(50);
        t.end_span(&agent, Status::Ok, None, &no_attrs())
            .expect("end agent");
        let summary = t.summary(&id).expect("summary");
        let _ = std::fs::remove_file(path);
        summary
    };
    let (a, b) = (run("det-a"), run("det-b"));
    assert_eq!(a, b);
    assert_eq!((a.spans, a.duration_ms, a.cost_micro_usd), (2, 300, 3_000));
    assert!(a.clean());
}

#[test]
fn errors_and_spans_left_open_make_the_summary_unclean() {
    let (t, _, path) = setup("unclean");
    let id = t.new_trace_id();
    let failed = t.start_span(&id, None, Kind::Tool, "run").expect("start");
    t.end_span(&failed, Status::Error, None, &no_attrs())
        .expect("end");
    let _hung = t.start_span(&id, None, Kind::Tool, "hang").expect("start");
    let s = t.summary(&id).expect("summary");
    assert_eq!((s.errors, s.open), (1, 1));
    assert!(!s.clean());
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_span_cannot_be_ended_twice() {
    let (t, _, path) = setup("twice");
    let id = t.new_trace_id();
    let span = t.start_span(&id, None, Kind::Tool, "run").expect("start");
    t.end_span(&span, Status::Ok, None, &no_attrs())
        .expect("end");
    assert!(matches!(
        t.end_span(&span, Status::Ok, None, &no_attrs()),
        Err(Error::UnknownSpan(_))
    ));
    let _ = std::fs::remove_file(path);
}

#[test]
fn there_is_no_column_that_could_hold_content_and_long_text_is_refused() {
    let (t, _, path) = setup("nocontent");
    let columns: Vec<String> = Connection::open(&path)
        .expect("open")
        .prepare("PRAGMA table_info(spans)")
        .expect("pragma")
        .query_map([], |r| r.get::<_, String>(1))
        .expect("rows")
        .map(|c| c.expect("col"))
        .collect();
    for forbidden in ["content", "prompt", "text", "message", "body"] {
        assert!(
            !columns.iter().any(|c| c == forbidden),
            "column {forbidden}"
        );
    }
    let id = t.new_trace_id();
    let span = t
        .start_span(&id, None, Kind::Model, "answer")
        .expect("start");
    let mut attrs = no_attrs();
    attrs.insert("note".into(), Attr::Text("x".repeat(500)));
    assert!(matches!(
        t.end_span(&span, Status::Ok, None, &attrs),
        Err(Error::Rejected(_))
    ));
    assert!(matches!(
        t.start_span(&id, None, Kind::Tool, &"y".repeat(500)),
        Err(Error::Rejected(_))
    ));
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_decoy_secret_in_an_attribute_is_refused_and_never_reaches_the_file() {
    let (t, _, path) = setup("decoy");
    let canaries = Canaries::from_seed(3);
    let decoy = canaries.value("notion").expect("decoy");
    let id = t.new_trace_id();
    let span = t.start_span(&id, None, Kind::Tool, "call").expect("start");
    let mut attrs = no_attrs();
    attrs.insert(
        "header".into(),
        Attr::Text(format!("Authorization {decoy}")),
    );
    assert!(matches!(
        t.end_span(&span, Status::Ok, None, &attrs),
        Err(Error::Rejected(_))
    ));
    assert!(matches!(
        t.start_span(&id, None, Kind::Tool, decoy),
        Err(Error::Rejected(_))
    ));
    drop(t);
    let bytes = std::fs::read(&path).expect("read");
    canaries.assert_clean("clio-trace.db", &String::from_utf8_lossy(&bytes));
    let _ = std::fs::remove_file(path);
}

#[test]
fn the_file_is_created_0600_and_a_readable_one_is_refused() {
    let (t, _, path) = setup("perm");
    drop(t);
    assert_eq!(
        std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777,
        0o600
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(
        Trace::open(&path, FixedClock::new(0)),
        Err(Error::Permissions(_))
    ));
    let _ = std::fs::remove_file(path);
}

#[test]
fn traceparent_round_trips_and_refuses_garbage() {
    let (t, _, path) = setup("tp");
    let id = t.new_trace_id();
    let span = t.start_span(&id, None, Kind::Agent, "task").expect("start");
    let header = span.traceparent().to_string();
    assert_eq!(
        Traceparent::parse(&header).expect("parse"),
        span.traceparent()
    );
    let remote = t
        .start_span(
            &span.traceparent().trace_id,
            Some(&span.traceparent().span_id),
            Kind::Tool,
            "remote",
        )
        .expect("remote");
    assert_eq!(remote.trace_id, span.trace_id);
    for bad in [
        "",
        "garbage",
        "01-aa-bb-01",
        &format!("00-{}-{}-01", "0".repeat(32), "1".repeat(16)),
        &format!("00-{}-{}-01", "A".repeat(32), "1".repeat(16)),
        &format!("00-{}-{}", "1".repeat(32), "1".repeat(16)),
    ] {
        assert!(Traceparent::parse(bad).is_err(), "{bad}");
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_span_is_stored_under_the_gen_ai_operation_name() {
    let (t, _, path) = setup("names");
    let id = t.new_trace_id();
    let span = t
        .start_span(&id, None, Kind::Model, "answer")
        .expect("start");
    t.end_span(&span, Status::Ok, Some(usage(1, 1)), &no_attrs())
        .expect("end");
    drop(t);
    let kind: String = Connection::open(&path)
        .expect("open")
        .query_row("SELECT kind FROM spans", [], |r| r.get(0))
        .expect("kind");
    assert_eq!(kind, names::OPERATION_MODEL);
    let _ = std::fs::remove_file(path);
}

#[test]
fn every_model_call_can_carry_its_prompt_version_and_the_gate_can_count_the_ones_that_do_not() {
    let (t, _, path) = setup("prompt-version");
    let id = t.new_trace_id();
    let root = t.start_span(&id, None, Kind::Agent, "task").expect("root");
    let versioned = |version: &str| {
        BTreeMap::from([(
            names::ATTR_PROMPT_VERSION.to_string(),
            Attr::Text(version.to_string()),
        )])
    };
    for version in ["triage@3", "answer@7", "triage@3"] {
        let call = t.child(&root, Kind::Model, "call").expect("call");
        t.end_span(&call, Status::Ok, Some(usage(10, 5)), &versioned(version))
            .expect("end");
    }
    assert_eq!(t.model_spans_without_prompt_version(&id).expect("count"), 0);
    assert_eq!(
        t.prompt_versions(&id).expect("versions"),
        ["answer@7", "triage@3"]
    );

    let bare = t.child(&root, Kind::Model, "call").expect("call");
    t.end_span(&bare, Status::Ok, Some(usage(10, 5)), &no_attrs())
        .expect("end");
    assert_eq!(t.model_spans_without_prompt_version(&id).expect("count"), 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn every_contract_state_change_is_in_the_trace_in_order() {
    let (t, clock, path) = setup("contract-state");
    let id = t.new_trace_id();
    let root = t.start_span(&id, None, Kind::Agent, "task").expect("root");
    for (from, to) in [
        ("borrador", "aprobado"),
        ("aprobado", "en curso"),
        ("en curso", "cerrado con éxito"),
    ] {
        clock.advance(10);
        t.contract_state(&root, "c-1", from, to).expect("state");
    }
    t.contract_state(&root, "c-2", "borrador", "cancelado")
        .expect("other contract");
    assert_eq!(
        t.contract_history("c-1").expect("history"),
        [
            ("borrador".to_string(), "aprobado".to_string()),
            ("aprobado".to_string(), "en curso".to_string()),
            ("en curso".to_string(), "cerrado con éxito".to_string()),
        ]
    );
    assert_eq!(t.contract_history("c-2").expect("history").len(), 1);
    let _ = std::fs::remove_file(path);
}
