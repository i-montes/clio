//! What a run used (tokens, cost, steps, time) and what was in the model's context. The numbers
//! come from the agent's events; they are kept in `usage.json` inside the run folder so the
//! screens can show them for a run that is already over. Money is integer micro-USD everywhere;
//! the helpers here turn the numbers into short Spanish text.

use std::path::Path;

use clio_brain::agent::{CallUsage, ContextReport, Event};
use clio_brain::models::Catalog;
use serde::{Deserialize, Serialize};

use super::home::{Config, Dirs};
use super::runs::{RunSummary, read_json};

/// The context window used when the catalog does not know the model.
pub const DEFAULT_WINDOW: u64 = 128_000;

/// The catalog that ships with the repository: it knows the default model.
const BUILTIN_CATALOG: &str = include_str!("../../catalog.example.json");

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunLimits {
    pub max_steps: u32,
    pub max_seconds: u64,
    pub max_cost_micro: u64,
}

impl RunLimits {
    pub fn of(c: &Config) -> Self {
        Self {
            max_steps: c.limits.max_steps,
            max_seconds: c.limits.max_seconds,
            max_cost_micro: c.limits.max_cost_micro_usd,
        }
    }
}

/// Saved as `usage.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunUsage {
    pub window: u64,
    pub window_known: bool,
    pub limits: RunLimits,
    pub calls: Vec<CallUsage>,
    /// The context as it was before the last model call.
    pub context: Option<ContextReport>,
}

impl RunUsage {
    pub fn new(window: u64, window_known: bool, limits: RunLimits) -> Self {
        Self {
            window,
            window_known,
            limits,
            ..Self::default()
        }
    }

    /// Keeps what the numbers need from an agent event.
    pub fn record(&mut self, e: &Event) {
        match e {
            Event::Usage(u) => self.calls.push(*u),
            Event::Context(c) => self.context = Some(c.clone()),
            _ => {}
        }
    }

    /// (input tokens, output tokens, cost in micro-USD).
    pub fn totals(&self) -> (u64, u64, u64) {
        self.calls.iter().fold((0, 0, 0), |(i, o, c), u| {
            (
                i + u64::from(u.input_tokens),
                o + u64::from(u.output_tokens),
                c + u.cost_micro,
            )
        })
    }

    pub fn load(run_dir: &Path) -> Self {
        read_json(&run_dir.join("usage.json")).unwrap_or_default()
    }
}

/// Everything the status bar and the usage screen need, from a live run or a finished one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_micro: u64,
    pub steps: u32,
    pub secs: u64,
    pub limits: RunLimits,
    /// Approximate tokens in the context; `None` when it was not recorded (an older run).
    pub ctx: Option<u64>,
    pub window: u64,
    pub window_known: bool,
    /// The session read external content (files, test output).
    pub tainted: bool,
}

impl Snapshot {
    /// A percentage of the window, 0 when there is no window.
    pub fn ctx_percent(&self) -> Option<u64> {
        self.ctx.map(|c| percent(c, self.window))
    }

    /// A finished run: the summary has the totals; `usage` the context and the limits it ran with.
    /// `fallback` is used for what an older run did not record.
    pub fn of_summary(s: &RunSummary, usage: &RunUsage, fallback: (u64, bool, RunLimits)) -> Self {
        let (window, window_known, limits) = if usage.window > 0 {
            (usage.window, usage.window_known, usage.limits)
        } else {
            fallback
        };
        Self {
            tokens_in: s.input_tokens,
            tokens_out: s.output_tokens,
            cost_micro: s.cost_micro,
            steps: s.steps,
            secs: u64::try_from(s.elapsed_ms).unwrap_or(0) / 1000,
            limits,
            ctx: usage.context.as_ref().map(ContextReport::total),
            window,
            window_known,
            tainted: s.tainted,
        }
    }
}

pub fn percent(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        0
    } else {
        u64::try_from(u128::from(part) * 100 / u128::from(whole)).unwrap_or(u64::MAX)
    }
}

/// The context window of a model: from `CLIO_CATALOG` or `<config>/catalog.json` if the person
/// has one, else from the catalog that ships with the repository; `(DEFAULT_WINDOW, false)` when
/// the model is not in it.
pub fn window_for(dirs: &Dirs, provider: &str, model: &str) -> (u64, bool) {
    let path = std::env::var_os("CLIO_CATALOG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dirs.config.join("catalog.json"));
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| BUILTIN_CATALOG.to_owned());
    window_in(&text, provider, model)
}

pub fn window_in(catalog: &str, provider: &str, model: &str) -> (u64, bool) {
    Catalog::from_json(catalog)
        .ok()
        .and_then(|c| {
            c.models
                .iter()
                .find(|m| m.provider == provider && m.id == model)
                .map(|m| u64::from(m.context_window))
        })
        .filter(|w| *w > 0)
        .map_or((DEFAULT_WINDOW, false), |w| (w, true))
}

// ---------------------------------------------------------------- text

/// `842`, `12,4k`, `128k`, `1,2M`: short, with a decimal comma.
pub fn tokens(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    let (tenths, unit) = if n < 999_950 {
        ((n * 10 + 500) / 1_000, 'k')
    } else {
        ((n * 10 + 500_000) / 1_000_000, 'M')
    };
    let (whole, frac) = (tenths / 10, tenths % 10);
    if frac == 0 || (unit == 'k' && whole >= 100) {
        format!("{whole}{unit}")
    } else {
        format!("{whole},{frac}{unit}")
    }
}

/// Dollars with the precision the amount needs: `$0` for nothing, `$0,0015` for a fraction of a
/// cent, `$0,12` from one cent up. Input is integer micro-USD.
pub fn usd(micro: u64) -> String {
    if micro == 0 {
        return "$0".into();
    }
    if micro >= 10_000 {
        let cents = (micro + 5_000) / 10_000;
        return format!("${},{:02}", cents / 100, cents % 100);
    }
    let digits = format!("{micro:06}");
    format!("$0,{}", digits.trim_end_matches('0'))
}

/// `21 s`, `2 min 5 s`, `10 min`.
pub fn duration(secs: u64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

/// The shortest form: `21s`, `10m`, `2m5s`.
pub fn duration_short(secs: u64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s}s"),
        (m, 0) => format!("{m}m"),
        (m, s) => format!("{m}m{s}s"),
    }
}

/// Milliseconds as seconds with one decimal and a comma: `2,1 s`.
pub fn ms_label(ms: u64) -> String {
    format!("{},{} s", ms / 1000, (ms % 1000) / 100)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_short_with_a_decimal_comma() {
        for (n, s) in [
            (0, "0"),
            (842, "842"),
            (999, "999"),
            (1_000, "1k"),
            (3_100, "3,1k"),
            (12_400, "12,4k"),
            (99_949, "99,9k"),
            (128_000, "128k"),
            (999_949, "999k"),
            (999_950, "1M"),
            (1_200_000, "1,2M"),
        ] {
            assert_eq!(tokens(n), s, "{n}");
        }
    }

    #[test]
    fn dollars_have_the_precision_the_amount_needs() {
        for (m, s) in [
            (0, "$0"),
            (1, "$0,000001"),
            (1_500, "$0,0015"),
            (9_999, "$0,009999"),
            (10_000, "$0,01"),
            (123_456, "$0,12"),
            (1_000_000, "$1,00"),
            (1_234_567, "$1,23"),
            (100_000, "$0,10"),
        ] {
            assert_eq!(usd(m), s, "{m}");
        }
    }

    #[test]
    fn durations_and_percentages() {
        assert_eq!(duration(21), "21 s");
        assert_eq!(duration(600), "10 min");
        assert_eq!(duration(125), "2 min 5 s");
        assert_eq!(duration_short(125), "2m5s");
        assert_eq!(ms_label(2_150), "2,1 s");
        assert_eq!(percent(31, 100), 31);
        assert_eq!(percent(1, 0), 0);
        assert_eq!(percent(u64::MAX, 1), u64::MAX);
    }

    #[test]
    fn the_window_comes_from_the_catalog_or_the_default() {
        assert_eq!(
            window_in(BUILTIN_CATALOG, "opencode-go", "beam-free"),
            (128_000, true)
        );
        assert_eq!(
            window_in(BUILTIN_CATALOG, "x", "unknown"),
            (DEFAULT_WINDOW, false)
        );
        assert_eq!(window_in("not json", "a", "b"), (DEFAULT_WINDOW, false));
        let one = BUILTIN_CATALOG.replace("128000", "32000");
        assert_eq!(window_in(&one, "opencode-go", "beam-free"), (32_000, true));
    }

    #[test]
    fn usage_records_and_adds_up() {
        let mut u = RunUsage::new(1000, true, RunLimits::default());
        for (call, i, o, c) in [(1, 100, 10, 5), (2, 200, 20, 7)] {
            u.record(&Event::Usage(CallUsage {
                call,
                input_tokens: i,
                output_tokens: o,
                cost_micro: c,
                model_ms: 1,
                at_ms: 1,
                retries: 0,
            }));
        }
        assert_eq!(u.totals(), (300, 30, 12));
        u.record(&Event::Final);
        assert_eq!(u.calls.len(), 2);
        let back: RunUsage = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
        assert_eq!(back, u);
    }
}
