//! Model catalog (spec: models): pinned ids, routing by level and privacy, sanitized payloads
//! (S-44), per-provider rate limit and a cost ledger. Money is integer micro-USD.

use clio_core::{Clock, Level};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// What the provider does with our data. Declared worst to best, so `<` means "fewer guarantees".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    Retains,
    NoTraining,
    NoRetention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Fast,
    Strong,
}

/// The bench run that approved this model (spec: no model change without the bench).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchRun {
    pub run: String,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub provider: String,
    /// Exact id as the provider takes it.
    pub id: String,
    /// Fixed version or snapshot. Never an alias.
    pub version: String,
    pub context_window: u32,
    pub input_micro_per_mtok: u64,
    pub output_micro_per_mtok: u64,
    pub tools: bool,
    pub vision: bool,
    pub retention: Retention,
    pub tier: Tier,
    pub levels: Vec<Level>,
    pub bench: Option<BenchRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    Public,
    Personal,
    Secret,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CatalogError {
    Json(String),
    Unpinned { model: String, why: &'static str },
}

impl ModelEntry {
    fn validate(&self) -> Result<(), CatalogError> {
        let bad = |why| {
            Err(CatalogError::Unpinned {
                model: format!("{}/{}", self.provider, self.id),
                why,
            })
        };
        let alias = |s: &str| {
            let s = s.to_lowercase();
            s.contains("latest") || s == "stable" || s == "default" || s == "*"
        };
        if alias(&self.id) || alias(&self.version) {
            return bad("alias that changes by itself");
        }
        if self.version.trim().is_empty() {
            return bad("no pinned version");
        }
        if !self.version.chars().any(|c| c.is_ascii_digit()) {
            return bad("version has no number or date");
        }
        Ok(())
    }

    /// Only a model that passed the bench can be deployed.
    pub fn deployable(&self) -> bool {
        self.bench.as_ref().is_some_and(|b| b.passed)
    }

    /// Cost of a call, rounded up so a cap is never undercounted.
    pub fn cost_micro(&self, input_tokens: u64, output_tokens: u64) -> u64 {
        let total = u128::from(input_tokens) * u128::from(self.input_micro_per_mtok)
            + u128::from(output_tokens) * u128::from(self.output_micro_per_mtok);
        u64::try_from(total.div_ceil(1_000_000)).unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub models: Vec<ModelEntry>,
    /// Which providers may receive each class of data. A class not listed goes nowhere.
    pub data_routes: BTreeMap<DataClass, Vec<String>>,
}

/// Where a request goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Route<'a> {
    Primary(&'a ModelEntry),
    Alternate(&'a ModelEntry),
    /// Levels 2 and 3 when nothing can serve now: queued, and the person is told.
    Queue,
    /// Levels 0 and 1: answer that it cannot be done now.
    Unavailable,
}

impl Catalog {
    pub fn from_json(s: &str) -> Result<Self, CatalogError> {
        let c: Catalog = serde_json::from_str(s).map_err(|e| CatalogError::Json(e.to_string()))?;
        c.models.iter().try_for_each(ModelEntry::validate)?;
        Ok(c)
    }

    /// Deployable models for a level: fast ones for 0 and 1, strong for 2 and 3, best privacy first.
    pub fn candidates(&self, level: Level) -> Vec<&ModelEntry> {
        let tier = if level <= Level::Simple {
            Tier::Fast
        } else {
            Tier::Strong
        };
        let mut v: Vec<_> = self
            .models
            .iter()
            .filter(|m| m.tier == tier && m.levels.contains(&level) && m.deployable())
            .collect();
        v.sort_by_key(|m| std::cmp::Reverse(m.retention)); // stable: keeps catalog order on ties
        v
    }

    /// Primary, else an alternate from another provider, else queue. With personal data the
    /// alternate may not have fewer privacy guarantees than the primary.
    pub fn route(
        &self,
        level: Level,
        personal_data: bool,
        down: &[&str],
        ledger: &mut Ledger,
    ) -> Route<'_> {
        let up = |m: &&ModelEntry| !down.contains(&m.provider.as_str());
        let cands = self.candidates(level);
        let Some(primary) = cands.first().copied() else {
            return Self::nothing(level, ledger);
        };
        if up(&primary) {
            return Route::Primary(primary);
        }
        let alt = cands
            .iter()
            .filter(|m| m.provider != primary.provider)
            .find(|m| up(m) && (!personal_data || m.retention >= primary.retention));
        match alt {
            Some(m) => {
                ledger.fallbacks += 1;
                Route::Alternate(m)
            }
            None => Self::nothing(level, ledger),
        }
    }

    fn nothing(level: Level, ledger: &mut Ledger) -> Route<'static> {
        if level <= Level::Simple {
            Route::Unavailable
        } else {
            ledger.queued += 1;
            Route::Queue
        }
    }
}

/// Cleans text before it leaves (the vault implements this; here it is only the contract).
pub trait Sanitizer {
    fn sanitize(&self, text: &str) -> String;
}

/// Text that already went through a `Sanitizer`. The only way to build one is `prepare_payload`,
/// so a transport that takes a `Payload` cannot be handed raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload(String);

impl Payload {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// The data-routing table does not allow this class of data for this provider.
    NotAllowed { class: DataClass, provider: String },
}

pub fn prepare_payload(
    catalog: &Catalog,
    sanitizer: &dyn Sanitizer,
    class: DataClass,
    model: &ModelEntry,
    text: &str,
) -> Result<Payload, SendError> {
    let allowed = catalog
        .data_routes
        .get(&class)
        .is_some_and(|ps| ps.contains(&model.provider));
    if !allowed {
        return Err(SendError::NotAllowed {
            class,
            provider: model.provider.clone(),
        });
    }
    Ok(Payload(sanitizer.sanitize(text)))
}

/// Calls allowed per window, per provider.
pub struct RateLimiter<C: Clock> {
    clock: C,
    max_calls: usize,
    window_ms: i64,
    calls: HashMap<String, VecDeque<i64>>,
}

impl<C: Clock> RateLimiter<C> {
    pub fn new(clock: C, max_calls: usize, window_ms: i64) -> Self {
        Self {
            clock,
            max_calls,
            window_ms,
            calls: HashMap::new(),
        }
    }

    /// Ok records the call; Err gives the milliseconds to wait.
    pub fn try_acquire(&mut self, provider: &str) -> Result<(), i64> {
        let now = self.clock.now_ms();
        let q = self.calls.entry(provider.into()).or_default();
        while q.front().is_some_and(|t| now - t >= self.window_ms) {
            q.pop_front();
        }
        if q.len() >= self.max_calls {
            return Err(q.front().map_or(0, |t| t + self.window_ms - now));
        }
        q.push_back(now);
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Usage {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micro: u64,
}

/// Cost and tokens per provider and model, plus how often the alternate or the queue was used.
#[derive(Debug, Default)]
pub struct Ledger {
    pub usage: BTreeMap<(String, String), Usage>,
    pub fallbacks: u32,
    pub queued: u32,
}

impl Ledger {
    /// Returns the cost of the call in micro-USD.
    pub fn record(&mut self, m: &ModelEntry, input_tokens: u64, output_tokens: u64) -> u64 {
        let cost = m.cost_micro(input_tokens, output_tokens);
        let u = self
            .usage
            .entry((m.provider.clone(), m.id.clone()))
            .or_default();
        u.calls += 1;
        u.input_tokens += input_tokens;
        u.output_tokens += output_tokens;
        u.cost_micro += cost;
        cost
    }
}
