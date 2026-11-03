//! Acceptance tests and rules whose feature the spec moved to v2 (retention module).
//! PURGE moved to v1: its tests are in `purge.rs`.

#[test]
#[ignore = "spec-clio-memory #23: after raw spans expire, daily totals and p95 stay the same (v2, retention module)"]
fn t23_rollups_keep_totals_and_p95_after_raw_spans_expire() {
    todo!()
}
