//! The debug view of a live run (Tab, while debug is on): per model call, the context as it was
//! built, the request and the answer (cut, with how much is missing), the gateway decisions with
//! their full rule and reason, times, retries and protocol errors. Everything stored here is
//! already sanitized for the terminal; the events it comes from were scrubbed of secrets before
//! they got here (`debug::Tee`).

use clio_brain::agent::{CallUsage, ContextReport, DecisionDebug, Exchange};
use clio_gateway::Outcome;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{App, section};
use super::sanitize;
use super::theme::Theme;
use crate::cli::debug::{VIEW_FIELD_MAX, cut};
use crate::cli::usage::{self, tokens};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Head,
    Dim,
    Plain,
    Ok,
    Warn,
    Fail,
}

/// Everything about one model call.
#[derive(Debug, Clone, Default)]
pub struct Block {
    pub lines: Vec<(Kind, String)>,
}

fn one(s: &str) -> String {
    sanitize::line(s)
}

impl Block {
    pub fn context(c: &ContextReport) -> Self {
        let mut b = Self {
            lines: vec![(Kind::Head, format!("llamada {}", c.call))],
        };
        b.lines.push((
            Kind::Dim,
            format!(
                "  contexto {} tok aprox.: sistema {} ({}) · objetivo {} · herramientas {} · conversación {} ({} turnos) · externo {} ({}) · salidas {} · memoria {} ({})",
                tokens(c.total()),
                tokens(c.system),
                one(&c.prompt_version),
                tokens(c.objective),
                tokens(c.tools),
                tokens(c.conversation),
                c.turns,
                tokens(c.external_tokens()),
                c.external.len(),
                tokens(c.tool_outputs),
                tokens(c.memory),
                c.memory_items
            ),
        ));
        for i in &c.external {
            b.lines.push((
                Kind::Dim,
                format!("    externo: {} {}", one(&i.name), tokens(i.tokens)),
            ));
        }
        b
    }

    /// What the memory recalled or proposed: one stored line per item.
    pub fn memory(title: &str, lines: &[String]) -> Self {
        let mut b = Self {
            lines: vec![(Kind::Head, format!("memoria: {}", one(title)))],
        };
        for l in lines {
            b.lines.push((Kind::Dim, format!("  {}", one(l))));
        }
        b
    }

    pub fn usage(&mut self, u: &CallUsage) {
        let retry = if u.retries > 0 {
            format!(" · {} reintentos", u.retries)
        } else {
            String::new()
        };
        self.lines.push((
            Kind::Plain,
            format!(
                "  modelo {} · entrada {} · salida {} tok · {}{retry}",
                usage::ms_label(u.model_ms),
                tokens(u64::from(u.input_tokens)),
                tokens(u64::from(u.output_tokens)),
                usage::usd(u.cost_micro)
            ),
        ));
    }

    pub fn exchange(&mut self, x: &Exchange) {
        self.lines.push((
            Kind::Plain,
            format!(
                "  petición: {} mensajes en total ({} car.), nuevos en esta llamada: {}",
                x.messages,
                x.request_chars,
                x.request.len()
            ),
        ));
        for m in &x.request {
            self.text(&format!("    [{}]", one(&m.role)), &m.content);
        }
        self.lines.push((
            Kind::Plain,
            format!("  respuesta: {} car.", x.response.chars().count()),
        ));
        self.text("   ", &x.response);
    }

    /// A possibly long, multi-line text, cut and sanitized, one stored line per source line.
    fn text(&mut self, prefix: &str, body: &str) {
        let clean = sanitize::multiline(&cut(body, VIEW_FIELD_MAX));
        for (i, l) in clean.lines().enumerate() {
            let head = if i == 0 { prefix } else { "     " };
            self.lines.push((Kind::Dim, format!("{head} {l}")));
        }
    }

    pub fn decision(&mut self, d: &DecisionDebug) {
        let (kind, word) = match d.outcome {
            Outcome::Allow => (Kind::Ok, "permitido"),
            Outcome::Ask => (Kind::Warn, "pedido"),
            Outcome::Deny => (Kind::Fail, "negado"),
        };
        self.lines.push((
            Kind::Plain,
            format!(
                "  pasarela #{} {} {}",
                d.n,
                one(&d.tool),
                one(&cut(&d.args, 300))
            ),
        ));
        self.lines.push((
            kind,
            format!("    {word} · regla {} · {} ms", one(&d.rule), d.tool_ms),
        ));
        self.text("    razón:", &d.reason);
    }

    pub fn note(&mut self, kind: Kind, text: &str) {
        self.lines.push((kind, format!("  {}", one(text))));
    }
}

fn style(t: Theme, k: Kind) -> Style {
    match k {
        Kind::Head => t.bold(),
        Kind::Dim => t.dim(),
        Kind::Plain => t.plain(),
        Kind::Ok => t.ok(),
        Kind::Warn => t.warn(),
        Kind::Fail => t.fail(),
    }
}

/// Fits one line in `w` characters.
pub fn fit(s: &str, w: usize, t: Theme) -> String {
    if s.chars().count() <= w {
        return s.to_owned();
    }
    let keep = w.saturating_sub(t.ellipsis().chars().count());
    format!(
        "{}{}",
        s.chars().take(keep).collect::<String>(),
        t.ellipsis()
    )
}

/// The stored lines broken to `w` columns: a long line continues indented on the next one, so
/// the notice at its end (`faltan N caracteres`) is never cut off. A width of 0 means unknown.
pub fn visual(blocks: &[Block], w: usize) -> Vec<(Kind, String)> {
    let w = if w == 0 { usize::MAX } else { w.max(24) };
    let mut out = Vec::new();
    for (k, text) in blocks.iter().flat_map(|b| b.lines.iter()) {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= w {
            out.push((*k, text.clone()));
            continue;
        }
        let mut i = 0;
        while i < chars.len() {
            let room = if i == 0 { w } else { w - 6 };
            let end = (i + room).min(chars.len());
            let seg: String = chars[i..end].iter().collect();
            out.push((*k, if i == 0 { seg } else { format!("      {seg}") }));
            i = end;
        }
    }
    out
}

pub fn total_lines(blocks: &[Block], w: usize) -> usize {
    visual(blocks, w).len()
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let l = &app.live;
    let rows = usize::from(area.height).saturating_sub(1).max(1);
    l.view_h.set(u16::try_from(rows).unwrap_or(u16::MAX));
    l.view_w.set(area.width);
    let all = visual(&l.blocks, usize::from(area.width));
    let end = all
        .len()
        .saturating_sub(l.scroll.min(all.len().saturating_sub(rows)));
    let start = end.saturating_sub(rows);
    let mut lines = vec![section(
        t,
        "debug: lo que vio y dijo el modelo (saneado)",
        area.width,
    )];
    if all.is_empty() {
        lines.push(Line::from(Span::styled(
            "todavía no hay llamadas al modelo; el detalle de petición y respuesta empieza con la próxima",
            t.dim(),
        )));
    }
    for (k, text) in &all[start..end] {
        lines.push(Line::from(Span::styled(text.clone(), style(t, *k))));
    }
    f.render_widget(Paragraph::new(lines), area);
}
