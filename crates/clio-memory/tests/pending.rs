//! Acceptance tests and rules whose feature the spec moved to v2 (PURGE, retention module).

#[test]
#[ignore = "spec-clio-memory #11: PURGE removes the content from every operation and leaves a tombstone (v2)"]
fn t11_purge_removes_content_and_leaves_a_tombstone() {
    todo!()
}

#[test]
#[ignore = "spec-clio-memory #23: after raw spans expire, daily totals and p95 stay the same (v2, retention module)"]
fn t23_rollups_keep_totals_and_p95_after_raw_spans_expire() {
    todo!()
}

#[test]
#[ignore = "S-24: after a PURGE no vector remains (v2)"]
fn s24_purge_also_removes_the_vector() {
    todo!()
}
