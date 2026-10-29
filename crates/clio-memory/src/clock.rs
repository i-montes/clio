use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Source of time for everything time-dependent (validity, quotas, forgetting).
pub trait Clock: Send + Sync {
    /// Unix seconds, UTC.
    fn now(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}

/// A clock tests can set, so time-dependent results are reproducible.
pub struct FixedClock(AtomicI64);

impl FixedClock {
    pub fn new(now: i64) -> Self {
        Self(AtomicI64::new(now))
    }

    pub fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_returns_set_value() {
        let c = FixedClock::new(100);
        assert_eq!(c.now(), 100);
        c.set(250);
        assert_eq!(c.now(), 250);
    }
}
