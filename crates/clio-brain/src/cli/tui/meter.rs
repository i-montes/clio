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
use super::text;
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

/// `0.9 %`: the share of the window with one decimal.
fn share(tokens: u64, window: u64) -> String {
    let per_mille = if window == 0 {
        0
    } else {
        u64::try_from(u128::from(tokens) * 1000 / u128::from(window)).unwrap_or(u64::MAX)
    };
    format!("{}.{} %", per_mille / 10, per_mille % 10)
}

fn warning(t: Theme, p: u64) -> Option<Line<'static>> {
    match p {
        90.. => Some(Line::from(Span::styled(
            format!(
                "{} the window is almost full (over 90 %): the model may cut or lose context",
                t.bang()
            ),
            t.fail(),
        ))),
        70..=89 => Some(Line::from(Span::styled(
            format!("{} the context is over 70 % of the window", t.bang()),
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
            label: "system prompt",
            short: "system",
            detail: clean(&c.prompt_version),
            tokens: c.system,
        },
        Layer {
            label: "objective / contract",
            short: "objective",
            detail: "your request".into(),
            tokens: c.objective,
        },
        Layer {
            label: "tools",
            short: "tools",
            detail: "descriptions".into(),
            tokens: c.tools,
        },
        Layer {
            label: "memory",
            short: "memory",
            detail: format!(
                "{} {}",
                c.memory_items,
                if c.memory_items == 1 {
                    "memory"
                } else {
                    "memories"
                }
            ),
            tokens: c.memory,
        },
        Layer {
            label: "conversation",
            short: "conversation",
            detail: format!(
                "{} {}",
                c.turns,
                if c.turns == 1 { "turn" } else { "turns" }
            ),
            tokens: c.conversation,
        },
        Layer {
            label: "external",
            short: "external",
            detail: format!("{n} {}", if n == 1 { "read" } else { "reads" }),
            tokens: c.external_tokens(),
        },
        Layer {
            label: "tool outputs",
            short: "outputs",
            detail: String::new(),
            tokens: c.tool_outputs,
        },
    ]
}

/// The title line: `context ▕███░░░░░░░▏ 31 %  39.7k of 128k tokens (approx.)`.
fn context_head(t: Theme, s: &Snapshot, bar: usize, compact: bool) -> Vec<Line<'static>> {
    let Some(total) = s.ctx else {
        return vec![Line::from(Span::styled("context: no data", t.dim()))];
    };
    let p = percent(total, s.window);
    let of = format!("{} of {} tokens (approx.)", tokens(total), tokens(s.window));
    let mut head = vec![
        Span::styled("context ", t.bold()),
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
            "This run did not save the context layers (it predates this feature).",
            t.dim(),
        )));
        return out;
    };
    if !s.window_known {
        out.push(Line::from(Span::styled(
            format!(
                "default window of {}: the model is not in the catalog",
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
        if l.label == "external" {
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
                    Span::styled("external", t.warn()),
                ]));
            }
            if c.external.len() > shown {
                out.push(Line::from(Span::styled(
                    format!("    {} and {} more", t.ellipsis(), c.external.len() - shown),
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
            "Approx.: four characters per token. The layers add up to what the model sees.",
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

/// A limit with how much of it is used: `8 of 40  ▕██░░░░░░░░▏ 20 %`.
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
            v.push(Span::styled(format!(" of {l}  "), t.dim()));
            v.push(Span::styled(format!("{} {p} %", t.bar(p, 10)), t.level(p)));
        }
        _ => v.push(Span::styled("  no limit", t.dim())),
    }
    Line::from(v)
}

fn limit_lines(t: Theme, s: &Snapshot) -> Vec<Line<'static>> {
    let l = s.limits;
    let some = |n: u64| (n > 0).then_some(n);
    vec![
        limit_line(
            t,
            "steps",
            s.steps.to_string(),
            some(u64::from(l.max_steps)).map(|n| n.to_string()),
            some(u64::from(l.max_steps)).map(|n| percent(u64::from(s.steps), n)),
        ),
        limit_line(
            t,
            "time",
            usage::duration(s.secs),
            some(l.max_seconds).map(usage::duration),
            some(l.max_seconds).map(|n| percent(s.secs, n)),
        ),
        limit_line(
            t,
            "cost",
            usage::usd(s.cost_micro),
            some(l.max_cost_micro).map(usage::usd),
            some(l.max_cost_micro).map(|n| percent(s.cost_micro, n)),
        ),
    ]
}

pub fn usage_lines(t: Theme, s: &Snapshot, u: &RunUsage, width: u16) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![
        Span::styled("usage ", t.bold()),
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
            "This run did not save the usage per step (it predates this feature); the totals come from its summary.",
            t.dim(),
        )));
    } else {
        // Wide: long headings and two spaces between columns; narrow: short ones, one space.
        let wide = width >= 76;
        let gap = if wide { "  " } else { " " };
        let (w_in, w_ms, w_cc) = if wide { (7, 8, 10) } else { (6, 7, 9) };
        let (h_in, h_out, h_cc) = if wide {
            ("input", "output", "cum.cost")
        } else {
            ("in", "out", "cum.$")
        };
        let head = format!(
            "{:>4}{gap}{h_in:>w_in$}{gap}{h_out:>w_in$}{gap}{:>9}{gap}{:>w_ms$}{gap}{:>8}{gap}{h_cc:>w_cc$}",
            "step", "cost", "model", "cum.tok"
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
                format!("{retries} model retries"),
                t.warn(),
            )));
        }
    }
    out.push(Line::from(""));
    out.push(section(t, "against the run limit", width));
    out.extend(limit_lines(t, s));
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        "Tokens and cost are reported by the model; the context is approximate.",
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
    let mut lines = vec![section(t, "context", area.width)];
    lines.extend(context_lines(t, &snap, usage, area.width, true));
    lines.push(Line::from(""));
    lines.push(section(t, "usage", area.width));
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

/// The help of the Pi chat: one titled section per part of the screen, its keys in an aligned
/// table and a short note under it when one is needed.
fn chat_help(app: &App, w: u16) -> Vec<Line<'static>> {
    let t = app.theme;
    let binds = |ctx, keep: &dyn Fn(Act) -> bool| -> Vec<(String, String)> {
        keys::table(ctx)
            .iter()
            .filter(|b| keep(b.act))
            .map(|b| (b.shown.to_owned(), b.what.to_owned()))
            .collect()
    };
    let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        v.iter()
            .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
            .collect()
    };
    let mut chat = binds(keys::Ctx::Chat, &|a| a != Act::Help);
    chat.extend(
        binds(keys::Ctx::ChatRun, &|a| {
            matches!(a, Act::Cancel | Act::Open)
        })
        .into_iter()
        .map(|(k, w)| (k, format!("{w} (while it works)"))),
    );
    chat.push(("F1".into(), "this help".into()));
    // Each section: its title, its keys and what they do, and its notes.
    type Section<'a> = (&'a str, Vec<(String, String)>, Vec<&'a str>);
    let mut sections: Vec<Section> = vec![
        ("the chat", chat, vec![]),
        (
            "the sessions",
            binds(keys::Ctx::Sessions, &|a| a != Act::Help),
            vec!["Ctrl-S or F2 opens them; while the agent works you cannot switch session."],
        ),
    ];
    if app.host {
        sections.push((
            "the approval question",
            pairs(&[("y then Enter", "run it"), ("n or Esc", "reject it")]),
            vec![
                "It hears the first key after a second on screen and a second without typing.",
                "With the y pressed, Enter runs it at once.",
                "Another key after the y undoes it and the second starts again.",
            ],
        ));
    }
    let cmds = super::commands::COMMANDS
        .iter()
        .map(|c| {
            let alias: Vec<String> = c.aliases.iter().map(|a| format!("/{a}")).collect();
            let what = if alias.is_empty() {
                c.what.to_owned()
            } else {
                format!("{} · {}", c.what, alias.join(" "))
            };
            (format!("/{}", c.name), what)
        })
        .collect();
    sections.push((
        "the commands",
        cmds,
        vec![
            "/ at the start of the message opens the list: Tab completes, Enter runs, Esc closes it.",
            "Your own: a .md in ~/.config/clio/commands/; $ARGUMENTS is what you type after it.",
            "/copy uses OSC 52: it only copies in terminals that allow it.",
        ],
    ));
    sections.push((
        "the mouse",
        pairs(&[
            ("click", "opens a session; outside the drawer, closes it"),
            ("wheel", "scrolls what is under it"),
            ("Shift and drag", "selects text"),
        ]),
        vec!["A click never answers a question: only the keys do."],
    ));
    let mut other = pairs(&[
        ("m", "the memory (in the sessions)"),
        ("D", "the doctor (in the sessions)"),
    ]);
    other.extend(
        keys::GLOBAL
            .iter()
            .map(|b| (b.shown.to_owned(), b.what.to_owned())),
    );
    sections.push(("other screens", other, vec![]));
    let key_w = sections
        .iter()
        .flat_map(|(_, rows, _)| rows.iter())
        .map(|(k, _)| text::width(k))
        .max()
        .unwrap_or(8)
        .min(20)
        + 2;
    let width = usize::from(w);
    let mut lines = vec![Line::from(vec![
        Span::styled("help ", t.bold()),
        Span::styled(
            if app.host {
                "· Clio works on this machine; every change is recorded"
            } else {
                "· Clio works on a copy; you decide what goes back to the original"
            },
            t.dim(),
        ),
    ])];
    for (title, rows, notes) in sections {
        lines.push(Line::from(""));
        lines.push(section(t, title, w));
        for (k, what) in rows {
            let k = text::fit(&k, key_w - 2, t.ellipsis());
            let pad = " ".repeat(key_w - text::width(&k));
            lines.push(Line::from(vec![
                Span::styled(format!(" {k}{pad}"), t.key()),
                Span::raw(text::fit(
                    &what,
                    width.saturating_sub(key_w + 1),
                    t.ellipsis(),
                )),
            ]));
        }
        for n in notes {
            lines.push(Line::from(Span::styled(
                format!(" {}", text::fit(n, width.saturating_sub(1), t.ellipsis())),
                t.dim(),
            )));
        }
    }
    // What the footer of the chat leaves out: the steps, the time, the session.
    let c = &app.chat;
    let snap = c.snapshot(app.now_ms, app.fallback());
    lines.push(Line::from(""));
    lines.push(section(t, "this session", w));
    let mut spans = vec![Span::raw(" ")];
    spans.extend(super::status::line(t, &snap, width.saturating_sub(1)).spans);
    lines.push(Line::from(spans));
    if !c.session_id.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                " {}",
                text::fit(
                    &format!("{}   {}", clean(&c.session_id), clean(&c.workspace)),
                    width.saturating_sub(1),
                    t.ellipsis()
                )
            ),
            t.dim(),
        )));
    }
    lines
}

pub fn draw_help(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let w = usize::from(area.width);
    let from_chat = app.pi && matches!(app.before_help, Screen::Chat | Screen::Sessions);
    if from_chat {
        let lines = chat_help(app, area.width);
        scrolled(f, area, lines, app);
        return;
    }
    // The keys of where help was opened from; from the chat, also the drawer's.
    let table = |ctx| keys::table(ctx).iter().map(|b| (b.shown, b.what)).collect();
    let groups: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("", table(app.ctx_of(app.before_help))),
        ("", keys::GLOBAL.iter().map(|b| (b.shown, b.what)).collect()),
    ];
    let key_w = groups
        .iter()
        .flat_map(|(_, e)| e.iter())
        .map(|(k, _)| text::width(k))
        .max()
        .unwrap_or(8)
        + 2;
    let widest = groups
        .iter()
        .flat_map(|(_, e)| e.iter())
        .map(|(_, what)| key_w + text::width(what))
        .max()
        .unwrap_or(20);
    let cols = if w >= 2 * (widest + 2) { 2 } else { 1 };
    let cell = |(k, what): &(&str, &str)| {
        vec![
            Span::styled(format!("{k:<key_w$}"), t.key()),
            Span::raw((*what).to_owned()),
        ]
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("keys ", t.bold()),
        Span::styled(
            if app.host {
                "· Clio works on this machine; every change is recorded (clio actions, clio undo)"
            } else {
                "· Clio works on a copy; you decide what goes back to the original"
            },
            t.dim(),
        ),
    ])];
    for (title, entries) in &groups {
        lines.push(Line::from(""));
        if !title.is_empty() {
            lines.push(section(t, title, area.width));
        }
        let rows = entries.len().div_ceil(cols);
        for r in 0..rows {
            let mut spans = cell(&entries[r]);
            if cols == 2 && r + rows < entries.len() {
                let used: usize = spans.iter().map(|s| text::width(&s.content)).sum();
                spans.push(Span::raw(" ".repeat((widest + 2).saturating_sub(used))));
                spans.extend(cell(&entries[r + rows]));
            }
            lines.push(Line::from(spans));
        }
    }
    scrolled(f, area, lines, app);
}
