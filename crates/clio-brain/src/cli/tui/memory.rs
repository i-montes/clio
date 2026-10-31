//! The Memory screen (`m` from the home): what is waiting for a person (quarantine, procedures
//! to approve) and what was remembered lately. `/` filters, `p` approves, `a` archives, both
//! after a question. Every text here comes from the memory file and is sanitized before it is
//! drawn. No boxes, like the rest.

use clio_brain::memory_link::{MemoryLink, kind_label, origin_label, state_label};
use clio_memory::State;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{Action, App, Cmd, Confirm, Screen, clean, plural, section};
use super::keys::{Act, Ctx, lookup};
use super::sanitize;
use crate::cli::home::Dirs;

/// One memory as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MRow {
    pub id: i64,
    pub kind: String,
    pub state: State,
    pub origin: String,
    pub actor: String,
    pub content: String,
    pub source: Option<String>,
}

/// What the worker found in the memory file.
#[derive(Debug, Clone, Default)]
pub struct MemoryData {
    /// Waiting for a person, newest first.
    pub pending: Vec<MRow>,
    /// Active, newest first.
    pub recent: Vec<MRow>,
    pub archived: u64,
    /// About the embedding model, when it is not in use.
    pub note: Option<String>,
    /// The file does not exist yet.
    pub missing: bool,
}

impl MemoryData {
    /// Reads the file read-only. An error comes back as text for the screen.
    pub fn load(dirs: &Dirs) -> Result<Self, String> {
        // Listing needs no embeddings: the model is not loaded here, only looked for.
        let Some(link) = MemoryLink::open_read(&dirs.data, None)? else {
            return Ok(Self {
                missing: true,
                ..Self::default()
            });
        };
        let row = |m: &clio_memory::MemoryRow| MRow {
            id: m.id.0,
            kind: kind_label(m.kind).into(),
            state: m.state,
            origin: origin_label(m.origin).into(),
            actor: link.actor_name(m.actor),
            content: m.content.clone(),
            source: m.source.clone(),
        };
        let mut pending: Vec<MRow> = link.pending()?.iter().map(row).collect();
        let mut recent: Vec<MRow> = link.list(State::Active)?.iter().map(row).collect();
        // shortcut: only the newest rows are shown; `clio memory list` and search reach the rest.
        pending.truncate(200);
        recent.truncate(100);
        Ok(Self {
            pending,
            recent,
            archived: link.counts()?.2,
            note: (!MemoryLink::model_dir(&dirs.home).is_dir())
                .then(|| "sin modelo de embeddings: busca solo por texto".to_owned()),
            missing: false,
        })
    }
}

#[derive(Default)]
pub struct MemoryView {
    pub data: Option<MemoryData>,
    pub sel: usize,
    pub filter: String,
    pub filtering: bool,
    pub loading: bool,
    /// The load failed: the message.
    pub error: Option<String>,
}

impl MemoryView {
    /// The rows that pass the filter: pending first, then recent. `true` marks pending.
    pub fn visible(&self) -> Vec<(&MRow, bool)> {
        let Some(d) = &self.data else {
            return vec![];
        };
        let f = self.filter.trim().to_lowercase();
        let ok = |r: &MRow| {
            f.is_empty()
                || [&r.content, &r.kind, &r.origin, &format!("m{}", r.id)]
                    .iter()
                    .any(|s| s.to_lowercase().contains(&f))
        };
        d.pending
            .iter()
            .filter(|r| ok(r))
            .map(|r| (r, true))
            .chain(d.recent.iter().filter(|r| ok(r)).map(|r| (r, false)))
            .collect()
    }

    pub fn selected(&self) -> Option<(&MRow, bool)> {
        self.visible().get(self.sel).copied()
    }
}

pub fn opened(app: &mut App) -> Vec<Cmd> {
    app.screen = Screen::Memory;
    app.memory.loading = true;
    app.memory.error = None;
    vec![Cmd::LoadMemory]
}

fn filter_key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let n = app.memory.visible().len();
    let m = &mut app.memory;
    match lookup(Ctx::MemoryFilter, k) {
        Some(Act::Open) => m.filtering = false,
        Some(Act::Back) => {
            m.filtering = false;
            m.filter.clear();
            m.sel = 0;
        }
        Some(Act::Down) => m.sel = (m.sel + 1).min(n.saturating_sub(1)),
        Some(Act::Up) => m.sel = m.sel.saturating_sub(1),
        _ => match k.code {
            KeyCode::Backspace => {
                m.filter.pop();
                m.sel = 0;
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !k
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                m.filter.push(c);
                m.sel = 0;
            }
            _ => {}
        },
    }
    vec![]
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    if app.memory.filtering {
        return filter_key(app, k);
    }
    let n = app.memory.visible().len();
    let Some(act) = lookup(Ctx::Memory, k) else {
        return vec![];
    };
    match act {
        Act::Back => {
            if app.memory.filter.is_empty() {
                return app.go_home();
            }
            app.memory.filter.clear();
            app.memory.sel = 0;
        }
        Act::Down => app.memory.sel = (app.memory.sel + 1).min(n.saturating_sub(1)),
        Act::Up => app.memory.sel = app.memory.sel.saturating_sub(1),
        Act::Top => app.memory.sel = 0,
        Act::Bottom => app.memory.sel = n.saturating_sub(1),
        Act::Filter => app.memory.filtering = true,
        Act::Refresh => {
            app.flash = Some("memoria actualizada".into());
            return opened(app);
        }
        Act::Help => app.open_help(),
        Act::Open => {
            if let Some((r, pending)) = app.memory.selected() {
                let text = format!(
                    "m{} · {} · {} · origen {} · la escribió {}\n{}\n\n{}",
                    r.id,
                    if pending {
                        "pendiente de revisión"
                    } else {
                        state_label(r.state)
                    },
                    r.kind,
                    r.origin,
                    r.actor,
                    r.source.as_deref().map_or(String::new(), |s| format!("fuente {s}")),
                    r.content
                );
                app.notice = Some(text);
            }
        }
        Act::Promote | Act::Archive => return ask(app, act),
        _ => {}
    }
    vec![]
}

fn ask(app: &mut App, act: Act) -> Vec<Cmd> {
    let Some((r, pending)) = app.memory.selected() else {
        app.flash = Some("no hay ningún recuerdo seleccionado".into());
        return vec![];
    };
    if act == Act::Promote && !pending {
        app.flash = Some("solo lo pendiente de revisión se aprueba; ese ya está activo".into());
        return vec![];
    }
    let confirm = Confirm {
        action: if act == Act::Promote {
            Action::Promote
        } else {
            Action::Archive
        },
        id: r.id.to_string(),
        files: 0,
        workspace: r.content.chars().take(200).collect(),
    };
    app.confirm = Some(confirm);
    vec![]
}

/// `m12  cuarentena  episódico    externo   text…`
fn row_line(app: &App, r: &MRow, pending: bool, selected: bool, width: usize) -> Line<'static> {
    let t = app.theme;
    let base = if selected { t.selected() } else { t.plain() };
    let dim = if selected { base } else { t.dim() };
    let state_style = if selected {
        base
    } else if pending {
        t.warn()
    } else {
        t.dim()
    };
    let head = format!("{:<6}", format!("m{}", r.id));
    let state = if pending && r.state == State::Quarantine {
        "pendiente"
    } else {
        state_label(r.state)
    };
    let fixed = 2 + 6 + 11 + 14 + 9;
    let room = width.saturating_sub(fixed);
    let text: String = clean(&r.content).chars().take(room).collect();
    Line::from(vec![
        Span::styled(
            if selected {
                format!("{} ", t.sel())
            } else {
                "  ".into()
            },
            base,
        ),
        Span::styled(head, dim),
        Span::styled(format!("{state:<11}"), state_style),
        Span::styled(format!("{:<14}", clean(&r.kind)), dim),
        Span::styled(format!("{:<9}", clean(&r.origin)), dim),
        Span::styled(text, base),
    ])
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let m = &app.memory;
    let mut lines: Vec<Line> = Vec::new();
    let Some(d) = &m.data else {
        let msg = m.error.as_deref().map_or_else(
            || "cargando…".to_owned(),
            |e| format!("{} {}", t.bang(), clean(e)),
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg,
                if m.error.is_some() { t.warn() } else { t.dim() },
            ))),
            area,
        );
        return;
    };
    let mut state = format!(
        "{} {} {} {} {}",
        plural(d.recent.len(), "recuerdo activo", "recuerdos activos"),
        t.dot(),
        plural(d.pending.len(), "pendiente", "pendientes"),
        t.dot(),
        plural(usize::try_from(d.archived).unwrap_or(0), "archivado", "archivados"),
    );
    if let Some(n) = &d.note {
        state = format!("{state} {} {n}", t.dot());
    }
    lines.push(Line::from(Span::styled(
        sanitize::line(&state).chars().take(usize::from(area.width)).collect::<String>(),
        t.dim(),
    )));
    if d.missing {
        lines.push(Line::from(""));
        lines.push(Line::from("Todavía no hay memoria."));
        lines.push(Line::from(Span::styled(
            "Se crea con `clio init` o con el primer run.",
            t.dim(),
        )));
        f.render_widget(Paragraph::new(lines), area);
        return;
    }
    if m.filtering || !m.filter.is_empty() {
        let n = m.visible().len();
        lines.push(Line::from(vec![
            Span::styled("/", t.accent()),
            Span::raw(clean(&m.filter)),
            Span::styled(if m.filtering { t.cursor() } else { "" }, t.accent()),
            Span::styled(format!("   {n} coinciden"), t.dim()),
        ]));
    }
    let vis = m.visible();
    if vis.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            if m.filter.is_empty() {
                "Todavía no hay recuerdos."
            } else {
                "Ningún recuerdo coincide."
            },
            t.dim(),
        )));
        f.render_widget(Paragraph::new(lines), area);
        return;
    }
    // Rows with their section headings; the selected row stays in view.
    let mut rows: Vec<(Option<usize>, Line)> = Vec::new();
    let (mut seen_pending, mut seen_recent) = (false, false);
    let n_pending = vis.iter().filter(|(_, p)| *p).count();
    let n_recent = vis.len() - n_pending;
    for (i, (r, pending)) in vis.iter().enumerate() {
        if *pending && !seen_pending {
            seen_pending = true;
            rows.push((
                None,
                section(t, &format!("pendientes de revisión ({n_pending})"), area.width),
            ));
        }
        if !*pending && !seen_recent {
            seen_recent = true;
            rows.push((
                None,
                section(t, &format!("recientes ({n_recent})"), area.width),
            ));
        }
        rows.push((
            Some(i),
            row_line(app, r, *pending, i == m.sel, usize::from(area.width)),
        ));
    }
    let used = lines.len();
    let room = usize::from(area.height).saturating_sub(used).max(1);
    let at = rows
        .iter()
        .position(|(i, _)| *i == Some(m.sel))
        .unwrap_or(0);
    let skip = (at + 1).saturating_sub(room);
    lines.extend(rows.into_iter().skip(skip).take(room).map(|(_, l)| l));
    f.render_widget(Paragraph::new(lines), area);
}
