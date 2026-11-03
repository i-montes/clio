#![allow(clippy::unwrap_used)]
//! The vault's short-lived tokens (S-21, S-47): they are short-lived, they are few, and the ones that
//! expired are forgotten instead of piling up for as long as the vault runs.

use std::sync::Arc;

use clio_core::FixedClock;
use clio_vault::{Dest, GatewayCap, Kind, Scope, SecretSpec, Vault, VaultError};

fn vault() -> (Vault, Arc<FixedClock>) {
    let clock = Arc::new(FixedClock::new(1_000));
    let (v, cap) = Vault::init(None, "correct horse battery staple", clock.clone()).unwrap();
    v.put(
        &cap,
        SecretSpec {
            name: "notion".into(),
            value: "a-long-enough-value-123".into(),
            kind: Kind::ApiKey,
            destinations: vec![Dest::new("notion-api", "api.notion.com")],
            scope: Scope::Personal,
            rotate_every_ms: None,
            expires_at_ms: None,
        },
    )
    .unwrap();
    (v, clock)
}

fn issue(v: &Vault, ttl_ms: i64) -> Result<(), VaultError> {
    v.issue_token(
        &GatewayCap::for_gateway(),
        "notion",
        "notion-api",
        "node",
        ttl_ms,
    )
    .map(|_| ())
}

// S-21: "short-lived" means it has a limit. A token with a ttl of a year, or of i64::MAX (which also
// overflowed the sum with the clock), or of nothing, is not one.
#[test]
fn a_token_lives_between_a_millisecond_and_an_hour() {
    let (v, _) = vault();
    for bad in [0, -5, 3_600_001, 86_400_000, i64::MAX] {
        assert!(
            matches!(issue(&v, bad), Err(VaultError::Denied(_))),
            "ttl {bad}"
        );
    }
    assert!(issue(&v, 1).is_ok());
    assert!(issue(&v, 3_600_000).is_ok());
}

// S-47: the table of tokens was never emptied (an expired one left only when it was used again) and
// had no limit. It is bounded, and an expired token makes room.
#[test]
fn live_tokens_are_bounded_and_expired_ones_make_room() {
    let (v, clock) = vault();
    for _ in 0..1024 {
        issue(&v, 600_000).unwrap();
    }
    assert!(
        matches!(issue(&v, 600_000), Err(VaultError::Denied(why)) if why.contains("too many")),
        "the 1025th live token is refused"
    );
    clock.advance(600_001);
    assert!(
        issue(&v, 600_000).is_ok(),
        "the expired ones were forgotten"
    );
}
