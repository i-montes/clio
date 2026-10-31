#![allow(clippy::unwrap_used)]

use clio_brain::models::*;
use clio_canaries::Canaries;
use clio_core::{FixedClock, Level};
use std::sync::Arc;

fn entry(
    provider: &str,
    id: &str,
    tier: &str,
    retention: &str,
    levels: &str,
    bench: &str,
) -> String {
    format!(
        r#"{{"provider":"{provider}","id":"{id}","version":"2026-01-01","context_window":8000,
        "input_micro_per_mtok":1000000,"output_micro_per_mtok":2000000,"tools":true,"vision":false,
        "retention":"{retention}","tier":"{tier}","levels":{levels},{bench}}}"#
    )
}

const BENCH_OK: &str = r#""bench":{"run":"r1","passed":true}"#;
const FAST: &str = r#"["Direct","Simple"]"#;
const STRONG: &str = r#"["Full","Long"]"#;

fn catalog(models: &[String]) -> Result<Catalog, CatalogError> {
    Catalog::from_json(&format!(
        r#"{{"models":[{}],"data_routes":{{"public":["a","b"],"personal":["a"]}}}}"#,
        models.join(",")
    ))
}

fn two_fast(ret_a: &str, ret_b: &str) -> Catalog {
    catalog(&[
        entry("a", "fast-a-1", "fast", ret_a, FAST, BENCH_OK),
        entry("b", "fast-b-1", "fast", ret_b, FAST, BENCH_OK),
    ])
    .unwrap()
}

#[test]
fn t01_primary_down_uses_the_alternate_and_records_it() {
    let c = two_fast("retains", "retains");
    let mut l = Ledger::default();
    assert!(
        matches!(c.route(Level::Direct, false, &[], &mut l), Route::Primary(m) if m.provider == "a")
    );
    assert!(
        matches!(c.route(Level::Direct, false, &["a"], &mut l), Route::Alternate(m) if m.provider == "b")
    );
    assert_eq!(l.fallbacks, 1);
}

#[test]
fn t02_both_down_goes_to_the_queue_and_warns() {
    let c = catalog(&[
        entry("a", "strong-a-1", "strong", "retains", STRONG, BENCH_OK),
        entry("b", "strong-b-1", "strong", "retains", STRONG, BENCH_OK),
    ])
    .unwrap();
    let mut l = Ledger::default();
    assert_eq!(
        c.route(Level::Full, false, &["a", "b"], &mut l),
        Route::Queue
    );
    assert_eq!(l.queued, 1);
    // Levels 0 and 1: say it cannot be done now.
    let f = two_fast("retains", "retains");
    assert_eq!(
        f.route(Level::Simple, false, &["a", "b"], &mut l),
        Route::Unavailable
    );
}

#[test]
fn t03_a_model_without_a_pinned_version_is_not_accepted() {
    let no_version = r#"{"models":[{"provider":"a","id":"m","version":"","context_window":1,
        "input_micro_per_mtok":0,"output_micro_per_mtok":0,"tools":false,"vision":false,
        "retention":"retains","tier":"fast","levels":["Direct"],"bench":null}],"data_routes":{}}"#;
    assert!(matches!(
        Catalog::from_json(no_version),
        Err(CatalogError::Unpinned { .. })
    ));
    let no_number = no_version.replace(r#""version":"""#, r#""version":"stable-ish""#);
    assert!(matches!(
        Catalog::from_json(&no_number),
        Err(CatalogError::Unpinned { .. })
    ));
}

#[test]
fn t04_a_model_without_a_passing_bench_is_not_deployed() {
    let none = catalog(&[entry(
        "a",
        "fast-a-1",
        "fast",
        "retains",
        FAST,
        r#""bench":null"#,
    )])
    .unwrap();
    let failed = catalog(&[entry(
        "a",
        "fast-a-1",
        "fast",
        "retains",
        FAST,
        r#""bench":{"run":"r","passed":false}"#,
    )])
    .unwrap();
    let mut l = Ledger::default();
    for c in [none, failed] {
        assert!(c.candidates(Level::Direct).is_empty());
        assert_eq!(
            c.route(Level::Direct, false, &[], &mut l),
            Route::Unavailable
        );
    }
}

#[test]
fn t05_personal_data_does_not_fall_to_weaker_privacy() {
    let c = catalog(&[
        entry(
            "a",
            "strong-a-1",
            "strong",
            "no_retention",
            STRONG,
            BENCH_OK,
        ),
        entry("b", "strong-b-1", "strong", "retains", STRONG, BENCH_OK),
    ])
    .unwrap();
    let mut l = Ledger::default();
    assert_eq!(c.route(Level::Full, true, &["a"], &mut l), Route::Queue);
    // Without personal data the weaker alternate is fine.
    assert!(matches!(
        c.route(Level::Full, false, &["a"], &mut l),
        Route::Alternate(_)
    ));
}

#[test]
fn t274_t275_aliases_are_rejected() {
    for (id, version) in [
        ("model-latest", "2026-01-01"),
        ("latest", "2026-01-01"),
        ("model", "latest"),
        ("model", "Latest-2"),
    ] {
        let json = format!(
            r#"{{"models":[{{"provider":"a","id":"{id}","version":"{version}","context_window":1,
            "input_micro_per_mtok":0,"output_micro_per_mtok":0,"tools":false,"vision":false,
            "retention":"retains","tier":"fast","levels":["Direct"],"bench":null}}],"data_routes":{{}}}}"#
        );
        assert!(
            matches!(
                Catalog::from_json(&json),
                Err(CatalogError::Unpinned { .. })
            ),
            "{id} {version}"
        );
    }
}

#[test]
fn example_catalog_loads_with_beam_free() {
    let c = Catalog::from_json(include_str!("../catalog.example.json")).unwrap();
    let m = &c.models[0];
    assert_eq!(
        (m.provider.as_str(), m.id.as_str()),
        ("opencode-go", "beam-free")
    );
    assert_eq!(m.cost_micro(1_000_000, 1_000_000), 0);
    assert_eq!(m.retention, Retention::NoTraining);
}

#[test]
fn model_per_level_fast_for_low_strong_for_high() {
    let c = catalog(&[
        entry("a", "fast-a-1", "fast", "retains", FAST, BENCH_OK),
        entry("a", "strong-a-1", "strong", "retains", STRONG, BENCH_OK),
    ])
    .unwrap();
    for l in [Level::Direct, Level::Simple] {
        assert_eq!(c.candidates(l)[0].id, "fast-a-1");
    }
    for l in [Level::Full, Level::Long] {
        assert_eq!(c.candidates(l)[0].id, "strong-a-1");
    }
}

#[test]
fn prefers_no_retention_where_it_exists() {
    let c = two_fast("retains", "no_retention");
    let mut l = Ledger::default();
    assert!(
        matches!(c.route(Level::Direct, false, &[], &mut l), Route::Primary(m) if m.provider == "b")
    );
}

struct Redact(Vec<String>);
impl Sanitizer for Redact {
    fn sanitize(&self, text: &str) -> String {
        self.0
            .iter()
            .fold(text.to_string(), |t, v| t.replace(v, "[redacted]"))
    }
}

// S-44: outgoing payloads carry no vault values, and data goes only where the table allows.
#[test]
fn outgoing_payloads_have_no_vault_values() {
    let canaries = Canaries::from_seed(7);
    let values: Vec<String> = canaries
        .names()
        .map(|n| canaries.value(n).unwrap().to_string())
        .collect();
    let c = two_fast("retains", "retains");
    let san = Redact(values.clone());
    let text = format!("use {} and {} to log in", values[0], values[1]);
    let p = prepare_payload(&c, &san, DataClass::Personal, &c.models[0], &text).unwrap();
    canaries.assert_clean("payload", p.as_str());
    // Provider "b" is not allowed to get personal data.
    let err = prepare_payload(&c, &san, DataClass::Personal, &c.models[1], &text).unwrap_err();
    assert!(matches!(err, SendError::NotAllowed { .. }));
    // A class missing from the table goes nowhere.
    assert!(prepare_payload(&c, &san, DataClass::Secret, &c.models[0], "x").is_err());
}

#[test]
fn rate_limit_waits_per_provider() {
    let clock = Arc::new(FixedClock::new(0));
    let mut r = RateLimiter::new(clock.clone(), 2, 1_000);
    r.try_acquire("a").unwrap();
    clock.advance(100);
    r.try_acquire("a").unwrap();
    assert_eq!(r.try_acquire("a"), Err(900));
    r.try_acquire("b").unwrap();
    clock.advance(900);
    r.try_acquire("a").unwrap();
}

#[test]
fn ledger_counts_cost_in_micro_usd_rounded_up() {
    let c = two_fast("retains", "retains");
    let m = &c.models[0];
    let mut l = Ledger::default();
    // 1 input token at 1_000_000 micro per Mtok is 1 micro; 1 output token is 2.
    assert_eq!(l.record(m, 1, 1), 3);
    assert_eq!(l.record(m, 1_000, 500), 2_000);
    let u = &l.usage[&("a".to_string(), "fast-a-1".to_string())];
    assert_eq!(
        (u.calls, u.input_tokens, u.output_tokens, u.cost_micro),
        (2, 1_001, 501, 2_003)
    );
}
