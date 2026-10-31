//! A value that leaves memory after a while without use and is loaded again when needed. It
//! holds the embedding model (about 1 GB) and the in-memory copy of the vectors of a store.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// How long the model stays in memory after its last use.
pub(crate) const DEFAULT_IDLE: Duration = Duration::from_secs(300);

struct Slot<T> {
    value: Option<T>,
    used: Instant,
}

/// A value that is dropped after `idle` without use and loaded again on the next use.
pub(crate) struct IdleCache<T> {
    slot: Arc<Mutex<Slot<T>>>,
    load: Box<dyn Fn() -> Result<T> + Send + Sync>,
}

impl<T: Send + 'static> IdleCache<T> {
    /// `idle: None` keeps the value for good. A watcher thread ends when the cache is dropped.
    pub(crate) fn new(
        first: T,
        idle: Option<Duration>,
        load: impl Fn() -> Result<T> + Send + Sync + 'static,
    ) -> Self {
        let slot = Arc::new(Mutex::new(Slot {
            value: Some(first),
            used: Instant::now(),
        }));
        if let Some(idle) = idle {
            let weak = Arc::downgrade(&slot);
            let poll = (idle / 4).max(Duration::from_millis(10));
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(poll);
                    let Some(slot) = weak.upgrade() else { break };
                    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
                    if slot.used.elapsed() >= idle {
                        slot.value = None;
                    }
                }
            });
        }
        Self {
            slot,
            load: Box::new(load),
        }
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.value.is_none() {
            slot.value = Some((self.load)()?);
        }
        slot.used = Instant::now();
        slot.value
            .as_mut()
            .map(f)
            .ok_or(Error::Embedding("value not loaded".into()))
    }

    #[cfg(test)]
    pub(crate) fn loaded(&self) -> bool {
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .value
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn idle_value_is_dropped_and_loaded_again_on_use() {
        let loads = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&loads);
        let cache = IdleCache::new(1, Some(Duration::from_millis(40)), move || {
            Ok(counter.fetch_add(1, Ordering::SeqCst) + 100)
        });
        assert_eq!(cache.with(|v| *v).unwrap(), 1, "the first value is kept");
        std::thread::sleep(Duration::from_millis(400));
        assert!(!cache.loaded(), "nobody used it, so it left memory");
        assert_eq!(cache.with(|v| *v).unwrap(), 100);
        assert_eq!(cache.with(|v| *v).unwrap(), 100, "used again: not reloaded");
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn without_an_idle_time_the_value_stays() {
        let cache = IdleCache::new(1, None, || Ok(2));
        std::thread::sleep(Duration::from_millis(50));
        assert!(cache.loaded());
    }

    #[test]
    fn a_failed_reload_is_an_error_not_a_panic() {
        let cache = IdleCache::new(1, Some(Duration::from_millis(20)), || {
            Err(Error::Embedding("gone".into()))
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(cache.with(|v| *v).is_err());
    }
}
