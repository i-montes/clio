use crate::{Error, Result};
use clio_core::Clock;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub resource: String,
    pub owner: String,
    pub until_ms: i64,
}

/// Serializes tasks over a resource (`"repo:x"`, `"path:/a/b"`). A second owner is rejected
/// (`Error::Held`) until the lease is released or expires; the same owner renews.
/// Shortcut: no blocking wait; the caller retries after `until_ms`.
pub struct LeaseTable<C: Clock> {
    clock: C,
    held: Mutex<HashMap<String, Lease>>,
}

impl<C: Clock> LeaseTable<C> {
    pub fn new(clock: C) -> Self {
        Self {
            clock,
            held: Mutex::new(HashMap::new()),
        }
    }

    pub fn acquire(&self, resource: &str, owner: &str, ttl_ms: i64) -> Result<Lease> {
        let now = self.clock.now_ms();
        let mut g = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = g.get(resource)
            && l.until_ms > now
            && l.owner != owner
        {
            return Err(Error::Held {
                resource: resource.into(),
                owner: l.owner.clone(),
                until_ms: l.until_ms,
            });
        }
        let lease = Lease {
            resource: resource.into(),
            owner: owner.into(),
            until_ms: now + ttl_ms,
        };
        g.insert(resource.into(), lease.clone());
        Ok(lease)
    }

    /// Only the owner can release; returns whether something was released.
    pub fn release(&self, resource: &str, owner: &str) -> bool {
        let mut g = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if g.get(resource).is_some_and(|l| l.owner == owner) {
            g.remove(resource);
            true
        } else {
            false
        }
    }
}
