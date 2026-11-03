//! The three read-only views of a run: the CONTEXT in layers (`c`), the USE per step (`U`), and
//! the help (`?`); plus the compact column the live screen shows at the right on a wide terminal.
//! Sizes in tokens are approximate (four characters per token) and say so.

use clio_brain::agent::ContextReport;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{App, Cmd, Screen, clean, section};
use super::debug_view::fit;
use super::keys::{self, Act, lookup};
use super::theme::Theme;
use crate::cli::usage::{self, RunUsage, Snapshot, percent, tokens};

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let Some(act) = lookup(app.ctx(), k) else {
        return vec![];
    };
    let max = app.view_max.get();
    let page = 8;
    match act {
        Act::Back => {
            app.screen = if app.screen == Screen::Help {
                app.before_help
            } else {
                app.back
            };
            app.view_scroll = 0;
        }
        Act::Usage => {
            app.screen = Screen::Usage;
            app.view_scroll = 0;
        }
        Act::Context => {
            app.screen = Screen::Context;
            app.view_scroll = 0;
        }
        Act::Down => app.view_scroll = (app.view_scroll + 1).min(max),
        Act::Up => app.view_scroll = app.view_scroll.saturating_sub(1),
        Act::PageDown => app.view_scroll = (app.view_scroll + page).min(max),
        Act::PageUp => app.view_scroll = app.view_scroll.saturating_sub(page),
        Act::Top => app.view_scroll = 0,
        Act::Bottom => app.view_scroll = max,
        Act::Help => app.open_help(),
        _ => {}
    }
    vec![]
}

/// Draws `lines` scrolled, and remembers how far scrolling can go.
fn scrolled(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>, app: &App) {
    let max = lines.len().saturating_sub(usize::from(area.height));
    app.view_max.set(max);
    let scroll = app.view_scroll;
    let from = scroll.min(max);
    f.render_widget(
        Paragraph::new(lines.into_iter().skip(from).collect::<Vec<_>>()),
        area,
    );
}

/// `0,9 %`: the share of the window with one decimal.
fn share(tokens: u64, window: u64) -> String {
    let per_mille = if window == 0 {
        0
    } else {
        u64::try_from(u128::from(tokens) * 1000 / u128::from(window)).unwrap_or(u64::MAX)
    };
    format!("{},{} %", per_mille / 10, per_mille % 10)
}

fn warning(t: Theme, p: u64) -> Option<Line<'static>> {
    match p {
        90.. => Some(Line::from(Span::styled(
            format!(
                "{} la ventana está casi llena (más del 90 %): el modelo puede recortar o perder contexto",
                t.bang()
            ),
            t.fail(),
        ))),
        70..=89 => Some(Line::from(Span::styled(
            format!("{} el contexto pasa del 70 % de la ventana", t.bang()),
            t.warn(),
        ))),
        _ => None,
    }
}

/// One row of the layers table.
struct Layer {
    label: &'static str,
    /// For the narrow column.
    short: &'static str,
    detail: String,
    tokens: u64,
}

fn layers(c: &ContextReport) -> Vec<Layer> {
    let n = c.external.len();
    vec![
        Layer {
            label: "prompt de sistema",
            short: "sistema",
            detail: clean(&c.prompt_version),
            tokens: c.system,
        },
        Layer {
            label: "objetivo / contrato",
            short: "objetivo",
            detail: "tu pedido".into(),
            tokens: c.objective,
        },
        Layer {
            label: "herramientas",
            short: "herramientas",
            detail: "descripciones".into(),
            tokens: c.tools,
        },
        Layer {
            label: "memoria",
            short: "memoria",
            detail: format!(
                "{} {}",
                c.memory_items,
                if c.memory_items == 1 {
                    "recuerdo"
                } else {
                    "recuerdos"
                }
            ),
            tokens: c.memory,
        },
        Layer {
            label: "conversación",
            short: "conversación",
            detail: format!(
                "{} {}",
                c.turns,
                if c.turns == 1 { "turno" } else { "turnos" }
            ),
            tokens: c.conversation,
        },
        Layer {
            label: "externo",
            short: "externo",
            detail: format!("{n} {}", if n == 1 { "lectura" } else { "lecturas" }),
            tokens: c.external_tokens(),
        },
        Layer {
            label: "salidas de herramientas",
            short: "salidas",
            detail: String::new(),
            tokens: c.tool_outputs,
        },
    ]
}

/// The title line: `contexto ▕███░░░░░░░▏ 31 %  39,7k de 128k tokens (aprox.)`.
fn context_head(t: Theme, s: &Snapshot, bar: usize, compact: bool) -> Vec<Line<'static>> {
    let Some(total) = s.ctx else {
        return vec![Line::from(Span::styled("contexto: sin datos", t.dim()))];
    };
    let p = percent(total, s.window);
    let of = format!("{} de {} tokens (aprox.)", tokens(total), tokens(s.window));
    let mut head = vec![
        Span::styled("contexto ", t.bold()),
        Span::styled(format!("{} {p} %", t.bar(p, bar)), t.level(p)),
    ];
    if compact {
        return vec![Line::from(head), Line::from(Span::styled(of, t.dim()))];
    }
    head.push(Span::styled(format!("  {of}"), t.dim()));
    vec![Line::from(head)]
}

pub fn context_lines(
    t: Theme,
    s: &Snapshot,
    u: &RunUsage,
    width: u16,
    compact: bool,
) -> Vec<Line<'static>> {
    let mut out = context_head(t, s, if compact { 10 } else { 24 }, compact);
    let Some(c) = &u.context else {
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            "Este run no guardó las capas del contexto (es anterior a esta función).",
            t.dim(),
        )));
        return out;
    };
    if !s.window_known {
        out.push(Line::from(Span::styled(
            format!(
                "ventana de {} por defecto: el modelo no está en el catálogo",
                tokens(s.window)
            ),
            t.dim(),
        )));
    }
    out.push(Line::from(""));
    let wide = !compact && width >= 78;
    for l in layers(c) {
        let mut spans = if compact {
            vec![Span::raw(format!("{:<14}", l.short))]
        } else {
            vec![
                Span::raw(format!("{:<25}", l.label)),
                Span::styled(format!("{:<16}", l.detail), t.dim()),
            ]
        };
        spans.push(Span::raw(format!("{:>6}", tokens(l.tokens))));
        spans.push(Span::styled(
            format!(" {:>8}", share(l.tokens, s.window)),
            t.dim(),
        ));
        if wide {
            let p = percent(l.tokens, s.window);
            spans.push(Span::styled(format!("  {}", t.bar(p, 12)), t.dim()));
        }
        out.push(Line::from(spans));
        if l.label == "externo" {
            let shown = if compact { 3 } else { c.external.len() };
            for i in c.external.iter().take(shown) {
                let room = if compact { 14 } else { 41 };
                let name = fit(
                    &clean(&i.name),
                    room.min(usize::from(width).saturating_sub(18)),
                    t,
                );
                out.push(Line::from(vec![
                    Span::styled(format!("    {name:<room$}"), t.dim()),
                    Span::styled(format!("{:>6} ", tokens(i.tokens)), t.dim()),
                    Span::styled("externo", t.warn()),
                ]));
            }
            if c.external.len() > shown {
                out.push(Line::from(Span::styled(
                    format!("    {} y {} más", t.ellipsis(), c.external.len() - shown),
                    t.dim(),
                )));
            }
        }
    }
    if let Some(w) = s.ctx_percent().and_then(|p| warning(t, p)) {
        out.push(Line::from(""));
        out.push(w);
    }
    if !compact {
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            "Aprox.: cuatro caracteres por token. Las capas suman lo que ve el modelo.",
            t.dim(),
        )));
    }
    out
}

pub fn draw_context(f: &mut Frame, area: Rect, app: &App) {
    let Some((snap, usage)) = app.run_data() else {
        return;
    };
    let lines = context_lines(app.theme, &snap, usage, area.width, false);
    scrolled(f, area, lines, app);
}

/// A limit with how much of it is used: `8 de 40  ▕██░░░░░░░░▏ 20 %`.
fn limit_line(
    t: Theme,
    label: &str,
    used: String,
    limit: Option<String>,
    p: Option<u64>,
) -> Line<'static> {
    let mut v = vec![
        Span::styled(format!("{label:<8}"), t.dim()),
        Span::raw(used),
    ];
    match (limit, p) {
        (Some(l), Some(p)) => {
            v.push(Span::styled(format!(" de {l}  "), t.dim()));
            v.push(Span::styled(format!("{} {p} %", t.bar(p, 10)), t.level(p)));
        }
        _ => v.push(Span::styled("  sin límite", t.dim())),
    }
    Line::from(v)
}

fn limit_lines(t: Theme, s: &Snapshot) -> Vec<Line<'static>> {
    let l = s.limits;
    let some = |n: u64| (n > 0).then_some(n);
    vec![
        limit_line(
            t,
            "pasos",
            s.steps.to_string(),
            some(u64::from(l.max_steps)).map(|n| n.to_string()),
            some(u64::from(l.max_steps)).map(|n| percent(u64::from(s.steps), n)),
        ),
        limit_line(
            t,
            "tiempo",
            usage::duration(s.secs),
            some(l.max_seconds).map(usage::duration),
            some(l.max_seconds).map(|n| percent(s.secs, n)),
        ),
        limit_line(
            t,
            "costo",
            usage::usd(s.cost_micro),
            some(l.max_cost_micro).map(usage::usd),
            some(l.max_cost_micro).map(|n| percent(s.cost_micro, n)),
        ),
    ]
}

pub fn usage_lines(t: Theme, s: &Snapshot, u: &RunUsage, width: u16) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![
        Span::styled("uso ", t.bold()),
        Span::raw(format!(
            "{}{} {}{} tokens {} {}",
            t.up(),
            tokens(s.tokens_in),
            t.down(),
            tokens(s.tokens_out),
            t.dot(),
            usage::usd(s.cost_micro)
        )),
    ])];
    out.push(Line::from(""));
    if u.calls.is_empty() {
        out.push(Line::from(Span::styled(
            "Este run no guardó el uso por paso (es anterior a esta función); los totales vienen de su resumen.",
            t.dim(),
        )));
    } else {
        // Wide: long headings and two spaces between columns; narrow: short ones, one space.
        let wide = width >= 76;
        let gap = if wide { "  " } else { " " };
        let (w_in, w_ms, w_cc) = if wide { (7, 8, 10) } else { (6, 7, 9) };
        let (h_in, h_out, h_cc) = if wide {
            ("entrada", "salida", "acum.costo")
        } else {
            ("entr.", "sal.", "acum.$")
        };
        let head = format!(
            "{:>4}{gap}{h_in:>w_in$}{gap}{h_out:>w_in$}{gap}{:>9}{gap}{:>w_ms$}{gap}{:>8}{gap}{h_cc:>w_cc$}",
            "paso", "costo", "modelo", "acum.tok"
        );
        out.push(Line::from(Span::styled(head, t.dim())));
        let (mut ci, mut co, mut cc) = (0u64, 0u64, 0u64);
        for c in &u.calls {
            ci += u64::from(c.input_tokens);
            co += u64::from(c.output_tokens);
            cc += c.cost_micro;
            out.push(Line::from(format!(
                "{:>4}{gap}{:>w_in$}{gap}{:>w_in$}{gap}{:>9}{gap}{:>w_ms$}{gap}{:>8}{gap}{:>w_cc$}",
                c.call,
                tokens(u64::from(c.input_tokens)),
                tokens(u64::from(c.output_tokens)),
                usage::usd(c.cost_micro),
                usage::ms_label(c.model_ms),
                tokens(ci + co),
                usage::usd(cc),
            )));
        }
        let retries: u32 = u.calls.iter().map(|c| c.retries).sum();
        if retries > 0 {
            out.push(Line::from(Span::styled(
                format!("{retries} reintentos del modelo"),
                t.warn(),
            )));
        }
    }
    out.push(Line::from(""));
    out.push(section(t, "frente al límite del run", width));
    out.extend(limit_lines(t, s));
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        "Los tokens y el costo los informa el modelo; el contexto es aproximado.",
        t.dim(),
    )));
    out
}

pub fn draw_usage(f: &mut Frame, area: Rect, app: &App) {
    let Some((snap, usage)) = app.run_data() else {
        return;
    };
    let lines = usage_lines(app.theme, &snap, usage, area.width);
    scrolled(f, area, lines, app);
}

/// The right column of the live screen: the layers and the limits.
pub fn draw_side(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let Some((snap, usage)) = app.run_data() else {
        return;
    };
    let mut lines = vec![section(t, "contexto", area.width)];
    lines.extend(context_lines(t, &snap, usage, area.width, true));
    lines.push(Line::from(""));
    lines.push(section(t, "uso", area.width));
    lines.push(Line::from(format!(
        "{}{} {}{} tokens {} {}",
        t.up(),
        tokens(snap.tokens_in),
        t.down(),
        tokens(snap.tokens_out),
        t.dot(),
        usage::usd(snap.cost_micro)
    )));
    lines.extend(limit_lines(t, &snap));
    let h = usize::from(area.height);
    f.render_widget(
        Paragraph::new(lines.into_iter().take(h).collect::<Vec<_>>()),
        area,
    );
}

pub fn draw_help(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let ctx = app.ctx_of(app.before_help);
    let mut entries: Vec<(&str, &str)> =
        keys::table(ctx).iter().map(|b| (b.shown, b.what)).collect();
    entries.extend(keys::GLOBAL.iter().map(|b| (b.shown, b.what)));
    let w = usize::from(area.width);
    let widest = entries
        .iter()
        .map(|(k, what)| 10 + what.chars().count() + k.chars().count().saturating_sub(9))
        .max()
        .unwrap_or(20);
    let cols = if w >= 2 * (widest + 2) { 2 } else { 1 };
    let rows = entries.len().div_ceil(cols);
    let cell = |(k, what): &(&str, &str)| {
        vec![
            Span::styled(format!("{k:<10}"), t.key()),
            Span::raw((*what).to_owned()),
        ]
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("atajos ", t.bold()),
            Span::styled(
                "· Clio trabaja en una copia; tú decides qué vuelve al original",
                t.dim(),
            ),
        ]),
        Line::from(""),
    ];
    for r in 0..rows {
        let mut spans = cell(&entries[r]);
        if cols == 2 && r + rows < entries.len() {
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            spans.push(Span::raw(" ".repeat((widest + 2).saturating_sub(used))));
            spans.extend(cell(&entries[r + rows]));
        }
        lines.push(Line::from(spans));
    }
    scrolled(f, area, lines, app);
}
