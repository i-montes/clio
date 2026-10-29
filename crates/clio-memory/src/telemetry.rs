//! Optional telemetry (the `telemetry` feature): a trace id on every operation, and spans and
//! events through the `tracing` facade. The crate never installs a subscriber and never exports
//! anything: the program that embeds it decides where spans go.
//!
//! Spans and events carry ids, sizes, counts, outcomes and times, never memory text, topic
//! keys, names or paths (test 22). Without the feature the `span!`, `record!`, `event!` and
//! `outcome!` macros expand to nothing: their arguments are not even evaluated.

#[cfg(feature = "telemetry")]
use std::collections::hash_map::RandomState;
#[cfg(feature = "telemetry")]
use std::hash::{BuildHasher, Hasher};
#[cfg(feature = "telemetry")]
use std::sync::Arc;
#[cfg(feature = "telemetry")]
use std::time::{SystemTime, UNIX_EPOCH};

/// What `span!` returns without the feature: a value to bind, nothing more.
#[cfg(not(feature = "telemetry"))]
pub(crate) struct Off;

/// `let _span = span!("name", field = value, late = ::tracing::field::Empty);` opens an info span
/// that closes when the binding goes out of scope.
#[cfg(feature = "telemetry")]
macro_rules! span {
    ($name:literal $(, $k:ident = $v:expr)* $(,)?) => {
        ::tracing::info_span!($name $(, $k = $v)*).entered()
    };
}
#[cfg(not(feature = "telemetry"))]
macro_rules! span {
    ($name:literal $(, $k:ident = $v:expr)* $(,)?) => {
        $crate::telemetry::Off
    };
}

/// Fills in a field the span was opened with as `Empty`.
#[cfg(feature = "telemetry")]
macro_rules! record {
    ($span:expr, $k:literal, $v:expr) => {
        $span.record($k, $v)
    };
}
#[cfg(not(feature = "telemetry"))]
macro_rules! record {
    ($span:expr, $k:literal, $v:expr) => {
        let _ = &$span;
    };
}

/// Fills in a field of the span that is current right now (for values only known deep inside).
#[cfg(feature = "telemetry")]
macro_rules! record_current {
    ($k:literal, $v:expr) => {
        ::tracing::Span::current().record($k, $v)
    };
}
#[cfg(not(feature = "telemetry"))]
macro_rules! record_current {
    ($k:literal, $v:expr) => {};
}

/// `event!(INFO, "name", field = value)`.
#[cfg(feature = "telemetry")]
macro_rules! event {
    ($level:ident, $name:literal $(, $k:ident = $v:expr)* $(,)?) => {
        ::tracing::event!(::tracing::Level::$level, event = $name $(, $k = $v)*)
    };
}
#[cfg(not(feature = "telemetry"))]
macro_rules! event {
    ($level:ident, $name:literal $(, $k:ident = $v:expr)* $(,)?) => {};
}

/// Evaluates a `Result` and records `ok` or the error's kind as the span's `outcome`.
#[cfg(feature = "telemetry")]
macro_rules! outcome {
    ($span:expr, $result:expr) => {{
        let result = $result;
        $span.record("outcome", $crate::telemetry::outcome_name(&result));
        result
    }};
}
#[cfg(not(feature = "telemetry"))]
macro_rules! outcome {
    ($span:expr, $result:expr) => {{
        let _ = &$span;
        $result
    }};
}

/// Closes a `propose` span: what was decided and the state it left the memory in, or the kind
/// of error that stopped it.
#[cfg(feature = "telemetry")]
macro_rules! finish_propose {
    ($span:expr, $result:expr) => {
        $crate::telemetry::finish_propose(&$span, &$result)
    };
}
#[cfg(not(feature = "telemetry"))]
macro_rules! finish_propose {
    ($span:expr, $result:expr) => {
        let _ = &$span;
    };
}

#[cfg(feature = "telemetry")]
pub(crate) fn finish_propose(
    span: &tracing::Span,
    result: &crate::error::Result<crate::model::Outcome>,
) {
    match result {
        Ok(o) => {
            span.record("outcome", format!("{:?}", o.kind).as_str());
            span.record("state", format!("{:?}", o.state).as_str());
        }
        Err(_) => {
            span.record("outcome", outcome_name(result));
        }
    }
}

/// `ok`, or which kind of error: a fixed word, never the message (messages can carry paths).
#[cfg(feature = "telemetry")]
pub(crate) fn outcome_name<T>(result: &crate::error::Result<T>) -> &'static str {
    use crate::error::Error;
    match result {
        Ok(_) => "ok",
        Err(e) => match e {
            Error::EmptyContent => "empty_content",
            Error::TooLong => "too_long",
            Error::QuotaExceeded => "quota_exceeded",
            Error::SecretDetected => "secret_detected",
            Error::InvalidValidity => "invalid_validity",
            Error::UnknownScope | Error::UnknownActor | Error::NotFound => "not_found",
            Error::InvalidState => "invalid_state",
            Error::NotAllowed => "not_allowed",
            Error::NotRevertible(_) => "not_revertible",
            Error::WriterLocked => "writer_locked",
            Error::ReadOnly => "read_only",
            Error::Embedding(_) | Error::NoEmbedder | Error::BadVector => "embedding",
            Error::Sqlite(_) | Error::Io(_) => "storage",
            _ => "error",
        },
    }
}

/// Supplies the trace id of the caller's current trace (the embedding program connects its
/// OpenTelemetry context here), or `None` when there is none. It runs inside the write lock, so it
/// must not call back into the engine.
#[cfg(feature = "telemetry")]
pub type TraceIdProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// A 128-bit id as 32 lowercase hex digits, like an OpenTelemetry trace id. Not cryptographic:
/// two `RandomState` hashes (std seeds them from the OS) mixed with the clock.
#[cfg(feature = "telemetry")]
pub(crate) fn random_trace_id() -> String {
    loop {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let half = |salt: u64| {
            let mut h = RandomState::new().build_hasher();
            h.write_u128(nanos);
            h.write_u64(salt);
            h.finish()
        };
        let id = format!("{:016x}{:016x}", half(1), half(2));
        if id.bytes().any(|b| b != b'0') {
            return id; // all zeros is not a valid trace id
        }
    }
}

/// A trace id (a provider's answer, or one read from an import) is kept only if it is a plain identifier: it ends up in the log and in
/// exports, so no spaces, control characters or long text.
pub(crate) fn acceptable(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}
