//! Shared pieces: the clock, the level of a request and the effect of a tool. Everything that
//! depends on time takes a `Clock`: everything that depends on time takes a `Clock`, so a
//! test or a benchmark run can move time by hand and get the same result twice.
//! clio-memory and clio-trace keep their own clocks on purpose: they depend on no other Clio crate.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    }
}

/// A clock that only moves when told to.
pub struct FixedClock(AtomicI64);

impl FixedClock {
    pub fn new(ms: i64) -> Self {
        Self(AtomicI64::new(ms))
    }

    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl<C: Clock + ?Sized> Clock for Arc<C> {
    fn now_ms(&self) -> i64 {
        (**self).now_ms()
    }
}

/// How much structure a request needs (spec: triage). The order is the point: a task only goes
/// up, never down, so levels compare with `<` and `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Level {
    /// 0: answered directly, read-only tools at most.
    Direct,
    /// 1: a few reversible steps; no contract and no planner.
    Simple,
    /// 2: contract, planner, verification and a memory write at the end.
    Full,
    /// 3: long or wide; the contract needs the person's approval.
    Long,
}

/// What a tool does to the world. The gateway decides by it (spec: gateway); the order runs from
/// harmless to not undoable, so `max` of two effects is the worse one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Effect {
    Read,
    /// A write the sandbox can undo.
    LocalReversible,
    /// A write that leaves the machine (network, a message sent).
    ExternalWrite,
    Irreversible,
}

/// Seconds since the epoch as `2026-11-03 23:42 +00:00` (proleptic Gregorian, days-from-civil).
pub fn utc_stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} +00:00",
        rem / 3_600,
        rem % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_clock_only_moves_when_advanced() {
        let c = Arc::new(FixedClock::new(1_000));
        assert_eq!(c.now_ms(), 1_000);
        c.advance(250);
        assert_eq!(c.now_ms(), 1_250);
    }

    #[test]
    fn levels_and_effects_order_from_light_to_heavy() {
        assert!(
            Level::Direct < Level::Simple
                && Level::Simple < Level::Full
                && Level::Full < Level::Long
        );
        assert!(
            Effect::Read < Effect::LocalReversible && Effect::ExternalWrite < Effect::Irreversible
        );
        assert_eq!(Level::Simple.max(Level::Direct), Level::Simple);
    }

    #[test]
    fn the_system_clock_is_after_2026() {
        assert!(SystemClock.now_ms() > 1_767_225_600_000);
    }
}
