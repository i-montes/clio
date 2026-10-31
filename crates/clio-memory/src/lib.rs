//! Generic memory engine on SQLite. See `docs/specs/clio-memory-spec-v0.2.md`.
//!
//! Content can only enter through [`Engine::propose`]: the storage layer is not public, so
//! nothing can skip the quota, the secret detector or the quarantine rules.
//!
//! ```compile_fail
//! use clio_memory::SqliteStore;
//! ```
//!
//! ```compile_fail
//! use clio_memory::Store;
//! ```

#[macro_use]
mod telemetry;
mod clock;
mod decider;
mod embed;
mod engine;
mod error;
mod format;
mod idle;
mod model;
#[cfg(feature = "model2vec")]
mod model2vec;
mod normalize;
mod platform;
mod recall;
mod rules;
mod secrets;
mod sqlite;
mod stem;
mod store;
mod token;
mod transfer;
mod vector;

pub use clock::{Clock, FixedClock, SystemClock};
pub use decider::{Decider, NearDuplicateDecider};
pub use embed::Embedder;
pub use engine::Engine;
pub use error::{Error, Result};
pub use model::*;
#[cfg(feature = "model2vec")]
pub use model2vec::Model2VecEmbedder;
#[cfg(feature = "telemetry")]
pub use telemetry::TraceIdProvider;
pub use token::{ApproxTokens, TokenCounter};
pub use transfer::EXPORT_FORMAT_VERSION;
