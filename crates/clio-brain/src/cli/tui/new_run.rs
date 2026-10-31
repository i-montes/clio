//! "New job": the folder and the instruction, with the same checks as `clio run` shown before
//! anything starts, and what will be left out of the copy.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::app::{App, Cmd, Screen, clean, is_ctrl_c};
use super::sanitize;
use crate::cli::work::Scan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Dir,
    Instruction,
}

/// What `clio run` would do with the folder, from the same validation and scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preview {
    pub workspace: String,
    pub files: usize,
    pub bytes: u64,
    pub secrets: Vec<String>,
    pub skipped: Vec<String>,
    pub symlinks: Vec<String>,
}

impl Preview {
    pub fn new(workspace: String, s: Scan) -> Self {
        Self {
            workspace,
            files: s.files.len(),
            bytes: s.bytes,
            secrets: s.secrets,
            skipped: s.skipped_dirs,
            symlinks: s.symlinks,
        }
    }
}

pub struct NewRun {
    pub dir: String,
    pub instruction: String,
    pub focus: Field,
    /// The result of checking `dir`; `None` while it is being checked.
    pub check: Option<Result<Preview, String>>,
}

impl NewRun {
    pub fn new(cwd: &str) -> Self {
        Self {
            dir: cwd.to_owned(),
            instruction: String::new(),
            focus: Field::Instruction,
            check: None,
        }
    }

    fn field(&mut self) -> &mut String {
        match self.focus {
            Field::Dir => &mut self.dir,
            Field::Instruction => &mut self.instruction,
        }
    }

    pub fn can_launch(&self) -> bool {
        matches!(self.check, Some(Ok(_))) && !self.instruction.trim().is_empty()
    }
}

fn recheck(app: &mut App) -> Vec<Cmd> {
    app.check_seq += 1;
    app.new_run.check = None;
    vec![Cmd::Check {
        dir: app.new_run.dir.clone(),
        seq: app.check_seq,
    }]
}

/// The screen was just opened.
pub fn opened(app: &mut App) -> Vec<Cmd> {
    recheck(app)
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let n = &mut app.new_run;
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    match k.code {
        KeyCode::Esc => return app.go_home(),
        _ if is_ctrl_c(k) => return app.go_home(),
        KeyCode::Tab | KeyCode::BackTab => {
            n.focus = match n.focus {
                Field::Dir => Field::Instruction,
                Field::Instruction => Field::Dir,
            };
        }
        KeyCode::Enter if alt && n.focus == Field::Instruction => n.instruction.push('\n'),
        KeyCode::Char('j') if ctrl && n.focus == Field::Instruction => n.instruction.push('\n'),
        KeyCode::Enter => {
            if n.focus == Field::Dir {
                n.focus = Field::Instruction;
            } else if n.can_launch() {
                app.screen = Screen::Live;
                app.live = super::live::Live::starting(app.now_ms, &n.instruction);
                return vec![Cmd::Start {
                    dir: n.dir.clone(),
                    instruction: n.instruction.trim().to_owned(),
                }];
            } else if !matches!(n.check, Some(Ok(_))) {
                app.flash = Some("elige primero una carpeta válida".into());
            } else {
                app.flash = Some("escribe qué quieres que haga Clio".into());
            }
        }
        KeyCode::Char('u') if ctrl => {
            n.field().clear();
            return after_edit(app);
        }
        KeyCode::Backspace => {
            n.field().pop();
            return after_edit(app);
        }
        KeyCode::Char(c) if !ctrl && !alt => {
            if c.is_control() {
                return vec![];
            }
            n.field().push(c);
            return after_edit(app);
        }
        _ => {}
    }
    vec![]
}

fn after_edit(app: &mut App) -> Vec<Cmd> {
    if app.new_run.focus == Field::Dir {
        recheck(app)
    } else {
        vec![]
    }
}

/// Pasted text goes through the sanitizer: it can neither carry escape sequences nor press Enter.
pub fn paste(app: &mut App, text: &str) -> Vec<Cmd> {
    let n = &mut app.new_run;
    let clean = sanitize::multiline(text);
    match n.focus {
        Field::Dir => n.dir.push_str(&clean.replace('\n', "")),
        Field::Instruction => n.instruction.push_str(&clean),
    }
    after_edit(app)
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let n = &app.new_run;
    let [dir_a, instr_a, info_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Min(3),
    ])
    .areas(area);
    let field = |title: &str, focused: bool| {
        Block::new()
            .borders(Borders::ALL)
            .border_type(if focused { BorderType::Thick } else { BorderType::Rounded })
            .border_style(if focused { t.accent() } else { t.plain() })
            .title(Span::styled(format!(" {title} "), t.bold()))
    };
    let cursor = |focused: bool| Span::styled(if focused { "▏" } else { "" }, t.accent());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(clean(&n.dir)),
            cursor(n.focus == Field::Dir),
        ]))
        .block(field("Carpeta del proyecto", n.focus == Field::Dir)),
        dir_a,
    );
    let mut lines: Vec<Line> = sanitize::multiline(&n.instruction)
        .split('\n')
        .map(|l| Line::from(l.to_owned()))
        .collect();
    if n.instruction.is_empty() && n.focus != Field::Instruction {
        lines = vec![Line::from(Span::styled("(escribe qué quieres que haga Clio)", t.dim()))];
    }
    if let Some(last) = lines.last_mut() {
        last.spans.push(cursor(n.focus == Field::Instruction));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(field("Instrucción", n.focus == Field::Instruction)),
        instr_a,
    );

    let mut info: Vec<Line> = Vec::new();
    match &n.check {
        None => info.push(Line::from(Span::styled("comprobando la carpeta…", t.dim()))),
        Some(Err(e)) => info.push(Line::from(vec![
            Span::styled("✗ ", t.fail()),
            Span::styled(clean(e), t.fail()),
        ])),
        Some(Ok(p)) => {
            info.push(Line::from(vec![
                Span::styled("✓ ", t.ok()),
                Span::raw(format!("se copiarán {} archivos ({})", p.files, mb(p.bytes))),
            ]));
            info.push(Line::from(Span::styled(
                "NO se copiarán a la copia de trabajo:",
                t.bold(),
            )));
            let group = |label: &str, v: &[String], info: &mut Vec<Line>| {
                if !v.is_empty() {
                    let shown: Vec<String> = v.iter().take(8).map(|s| clean(s)).collect();
                    let more = if v.len() > 8 { format!(" … y {} más", v.len() - 8) } else { String::new() };
                    info.push(Line::from(vec![
                        Span::styled(format!("  {label}: "), t.warn()),
                        Span::raw(format!("{}{more}", shown.join(", "))),
                    ]));
                }
            };
            group("parecen secretos", &p.secrets, &mut info);
            group("carpetas pesadas o internas", &p.skipped, &mut info);
            group("enlaces simbólicos", &p.symlinks, &mut info);
            info.push(Line::from(Span::styled(
                "  siempre: .git, node_modules, target, .venv, __pycache__, .env*, *.pem, *.key, id_rsa*, id_ed25519*, .ssh, .aws, credentials*",
                t.dim(),
            )));
        }
    }
    f.render_widget(
        Paragraph::new(info).wrap(Wrap { trim: false }).block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(" Qué se va a copiar ", t.bold())),
        ),
        info_a,
    );
}
