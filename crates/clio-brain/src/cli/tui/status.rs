//! The status bar: the last line of the screen, always the same, always dim. It carries the two
//! things that must never be out of sight: the USE of the run (tokens, cost, steps and time
//! against their limits) and the CONTEXT layer (how much of the model's window is taken). When
//! the session read external content, `externo` is added. Narrow terminals get shorter wording,
//! never fewer data.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::theme::Theme;
use crate::cli::usage::{self, Snapshot};

fn chars(parts: &[(String, Style)]) -> usize {
    parts.iter().map(|(s, _)| s.chars().count()).sum()
}

/// The bar for a width. The first wording that fits wins; the last one is always shown.
pub fn line(t: Theme, s: &Snapshot, width: usize) -> Line<'static> {
    let v = variants(t, s);
    let pick = v
        .iter()
        .find(|p| chars(p) <= width)
        .or_else(|| v.last())
        .cloned()
        .unwrap_or_default();
    Line::from(
        pick.into_iter()
            .map(|(text, style)| Span::styled(text, style))
            .collect::<Vec<_>>(),
    )
}

fn variants(t: Theme, s: &Snapshot) -> Vec<Vec<(String, Style)>> {
    let dim = t.dim();
    let tok = |word: bool| {
        format!(
            "{}{} {}{}{}",
            t.up(),
            usage::tokens(s.tokens_in),
            t.down(),
            usage::tokens(s.tokens_out),
            if word { " tok" } else { "" }
        )
    };
    let l = s.limits;
    let steps = |word: &str| {
        if l.max_steps > 0 {
            format!("{}/{}{word}", s.steps, l.max_steps)
        } else {
            format!("{}{word}", s.steps)
        }
    };
    let time = |short: bool| {
        let (a, b) = if short {
            (
                usage::duration_short(s.secs),
                usage::duration_short(l.max_seconds),
            )
        } else {
            (usage::duration(s.secs), usage::duration(l.max_seconds))
        };
        if l.max_seconds > 0 {
            format!("{a}/{b}")
        } else {
            a
        }
    };
    let pct = s.ctx_percent();
    let ctx_style = pct.map_or(dim, |p| t.level(p));
    let ctx = |word: &str, bar: usize, space: bool| -> Vec<(String, Style)> {
        match pct {
            None => vec![(format!("{word} {}", t.rule()), dim)],
            Some(p) => {
                let mut v = vec![(format!("{word} "), dim)];
                if bar > 0 {
                    v.push((format!("{} ", t.bar(p, bar)), ctx_style));
                }
                v.push((format!("{p}{}%", if space { " " } else { "" }), ctx_style));
                v
            }
        }
    };
    let sep = format!(" {} ", t.dot());
    let ext = |word: &str| -> Vec<(String, Style)> {
        if s.tainted {
            vec![(word.to_owned(), t.warn())]
        } else {
            vec![]
        }
    };
    let join = |mut head: Vec<(String, Style)>,
                gap: &str,
                tail: Vec<(String, Style)>,
                ext_gap: &str,
                e: Vec<(String, Style)>| {
        head.push((gap.to_owned(), dim));
        head.extend(tail);
        if !e.is_empty() {
            head.push((ext_gap.to_owned(), dim));
            head.extend(e);
        }
        head
    };
    let usage_text = |tokword: bool, stepword: &str, short: bool| -> Vec<(String, Style)> {
        vec![(
            format!(
                "{}{sep}{}{sep}{}{sep}{}",
                tok(tokword),
                usage::usd(s.cost_micro),
                steps(stepword),
                time(short)
            ),
            dim,
        )]
    };
    let compact = |ctx_word: &str| -> Vec<(String, Style)> {
        vec![(
            format!(
                "{} {} {} {}",
                tok(false),
                usage::usd(s.cost_micro),
                steps("p"),
                time(true)
            ),
            dim,
        )]
        .into_iter()
        .chain(std::iter::once((format!(" {ctx_word} "), dim)))
        .collect()
    };
    let mut out = vec![
        join(
            usage_text(true, " pasos", false),
            "   ",
            ctx("contexto", 10, true),
            "   ",
            ext("externo"),
        ),
        join(
            usage_text(false, " pasos", false),
            "   ",
            ctx("ctx", 6, true),
            "  ",
            ext("externo"),
        ),
        join(
            usage_text(false, "p", true),
            "  ",
            ctx("ctx", 0, false),
            " ",
            ext("ext"),
        ),
    ];
    // The shortest wording: no separators at all.
    let mut last = compact("ctx");
    last.extend(match pct {
        Some(p) => vec![(format!("{p}%"), ctx_style)],
        None => vec![(t.rule().to_owned(), dim)],
    });
    if s.tainted {
        last.push((" ext".to_owned(), t.warn()));
    }
    out.push(last);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::cli::usage::RunLimits;

    fn snap(tainted: bool) -> Snapshot {
        Snapshot {
            tokens_in: 12_400,
            tokens_out: 3_100,
            cost_micro: 0,
            steps: 8,
            secs: 21,
            limits: RunLimits {
                max_steps: 40,
                max_seconds: 600,
                max_cost_micro: 100_000,
            },
            ctx: Some(39_680),
            window: 128_000,
            window_known: true,
            tainted,
        }
    }

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_wide_bar_is_the_one_in_the_spec() {
        let t = Theme {
            color: false,
            unicode: true,
        };
        let s = text(&line(t, &snap(false), 120));
        assert_eq!(
            s,
            "↑12,4k ↓3,1k tok · $0 · 8/40 pasos · 21 s/10 min   contexto ▕███░░░░░░░▏ 31 %"
        );
        assert!(text(&line(t, &snap(true), 120)).ends_with("   externo"));
    }

    #[test]
    fn every_width_keeps_the_four_data_and_fits() {
        for unicode in [true, false] {
            let t = Theme {
                color: true,
                unicode,
            };
            for tainted in [false, true] {
                for w in [60usize, 61, 70, 79, 80, 90, 100, 120, 200] {
                    let s = text(&line(t, &snap(tainted), w));
                    assert!(s.chars().count() <= w, "{w}: {s:?}");
                    assert!(s.contains("12,4k") && s.contains("3,1k"), "{w}: {s:?}");
                    assert!(s.contains('$'), "{w}: {s:?}");
                    assert!(s.contains("8/40"), "{w}: {s:?}");
                    assert!(s.contains("21"), "{w}: {s:?}");
                    assert!(s.contains("31"), "{w}: {s:?}");
                    assert_eq!(s.contains(" ext"), tainted, "{w}: {s:?}");
                }
            }
        }
    }

    #[test]
    fn an_unknown_context_and_unknown_limits_do_not_break_it() {
        let t = Theme {
            color: false,
            unicode: true,
        };
        let mut s = snap(false);
        s.ctx = None;
        s.limits = RunLimits::default();
        let out = text(&line(t, &s, 80));
        assert!(
            out.contains("contexto ─") && out.contains("8 pasos") && out.contains("21 s"),
            "{out}"
        );
    }

    #[test]
    fn a_full_window_is_in_red() {
        let t = Theme {
            color: true,
            unicode: true,
        };
        let mut s = snap(false);
        s.ctx = Some(120_000);
        let l = line(t, &s, 120);
        assert!(l.spans.iter().any(|sp| sp.style == t.fail()), "{l:?}");
    }
}
