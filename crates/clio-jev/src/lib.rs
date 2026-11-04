//! Jev (TypeSafe's System One model) as an optional decider. It does not write: it answers closed
//! questions with probabilities, so callers use it to pick, never to draft. Everything here fails
//! closed toward "no answer", and a caller that gets no answer carries on with its own rules: Jev
//! is never required (spec `docs/specs/2026-11-03-jev-design.md`).
//!
//! - [`wire`]: the questions and the checked answers.
//! - [`client`]: the call, with a time limit, a breaker and a daily token budget. The HTTP goes
//!   through `adapters/jev/jev_systemone.py`, the way the model adapter does it, so this crate has
//!   no network dependency; the key is handed over on stdin and is never in an argument or a variable.
//! - [`catalog`]: the questions about tools, generated from the catalog the guard reports.

pub mod catalog;
pub mod client;
pub mod wire;

pub use catalog::{ToolHint, parse_catalog, suggested, tool_questions};
pub use client::{Config, Jev, JevError, Process, Transport};
pub use wire::{Answer, Answers, Clean, Question, Questions};
