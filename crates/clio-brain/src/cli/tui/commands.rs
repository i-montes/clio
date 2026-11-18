//! The chat's slash commands: ONE table (name, aliases, what it does, whether it waits for
//! the chat to be idle, what runs it), used by the menu, the help screen and the docs, as
//! `keys.rs` is for keys. The person's own commands are the `*.md` files of `<config>/commands/`.
//!
//! Only the person's typing reaches a command: `parse` reads the composer and nothing else, and
//! nothing the model or a tool writes is ever put through it. A command never replaces or answers
//! an open dialog: the dialog has the keys.

use std::cell::Cell;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::app::{Action, App, Cmd, Confirm, Screen, clean};
use super::text;

/// What a command does; `run` dispatches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    Model,
    Connector,
    Effort,
    New,
    Sessions,
    Name,
    Compact,
    Context,
    Usage,
    Pending,
    Actions,
    Undo,
    Copy,
    Memory,
    Doctor,
    Help,
    Quit,
}

pub struct Command {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// What it takes, as the menu shows it (`[name]`, `<title>`).
    pub args: &'static str,
    pub what: &'static str,
    /// It changes the session: refused while a turn, an open or an approval runs.
    pub idle: bool,
    pub run: Run,
}

const fn c(
    name: &'static str,
    aliases: &'static [&'static str],
    args: &'static str,
    what: &'static str,
    idle: bool,
    run: Run,
) -> Command {
    Command {
        name,
        aliases,
        args,
        what,
        idle,
        run,
    }
}

pub const COMMANDS: &[Command] = &[
    c(
        "model",
        &[],
        "[name]",
        "cambia el modelo del conector",
        true,
        Run::Model,
    ),
    c(
        "connector",
        &[],
        "[name]",
        "cambia el conector (vale para la próxima sesión)",
        true,
        Run::Connector,
    ),
    c(
        "effort",
        &[],
        "[level]",
        "cuánto piensa el modelo",
        true,
        Run::Effort,
    ),
    c(
        "new",
        &["clear"],
        "",
        "empieza una sesión nueva",
        true,
        Run::New,
    ),
    c(
        "sessions",
        &["resume"],
        "",
        "abre el panel de sesiones",
        false,
        Run::Sessions,
    ),
    c(
        "name",
        &[],
        "<title>",
        "pone título a la sesión",
        true,
        Run::Name,
    ),
    c(
        "compact",
        &[],
        "[instructions]",
        "resume la conversación para liberar contexto",
        true,
        Run::Compact,
    ),
    c(
        "context",
        &[],
        "",
        "cuánto contexto lleva la sesión",
        false,
        Run::Context,
    ),
    c(
        "usage",
        &["cost"],
        "",
        "tokens, costo, pasos y tiempo de la sesión",
        false,
        Run::Usage,
    ),
    c(
        "pending",
        &[],
        "",
        "lo que espera tu visto bueno",
        false,
        Run::Pending,
    ),
    c(
        "actions",
        &[],
        "",
        "lo que hizo esta sesión, acción por acción",
        false,
        Run::Actions,
    ),
    c(
        "undo",
        &[],
        "",
        "deshace los cambios de archivos de esta sesión",
        true,
        Run::Undo,
    ),
    c(
        "copy",
        &[],
        "",
        "copia la última respuesta (OSC 52)",
        false,
        Run::Copy,
    ),
    c("memory", &[], "", "abre la memoria", false, Run::Memory),
    c(
        "doctor",
        &[],
        "",
        "revisa la instalación",
        false,
        Run::Doctor,
    ),
    c("help", &[], "", "teclas y comandos", false, Run::Help),
    // Like Ctrl-C: not in the middle of a turn, an open or an approval.
    c("quit", &["exit"], "", "cierra Clio", true, Run::Quit),
];

/// Pi's level for what the person typed (`off` … `max`, as Pi calls them).
pub fn level_of(typed: &str) -> Option<&'static str> {
    let t = typed.trim().to_lowercase();
    clio_brain::pi::launch::THINKING
        .iter()
        .find(|l| **l == t)
        .copied()
}

/// One of the person's own commands: `<config>/commands/<name>.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Custom {
    pub name: String,
    /// The file's first line when it starts with `# `.
    pub what: String,
    /// The rest of the file: what is sent.
    pub body: String,
}

/// A command file is at most this big.
pub const CUSTOM_MAX: u64 = 32 * 1024;

/// The person's commands in `dir`, by name, and the files left out (a name that is not
/// `[a-z0-9-]+`, a built-in name, too big, not text). A missing folder is no commands.
pub fn load_custom(dir: &Path) -> (Vec<Custom>, Vec<String>) {
    let (mut out, mut left) = (Vec::new(), Vec::new());
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        let file = e.file_name().to_string_lossy().into_owned();
        let Some(name) = file.strip_suffix(".md") else {
            continue;
        };
        let ok_name = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !ok_name || builtin(name).is_some() {
            left.push(file);
            continue;
        }
        // Only a regular file, never through a link: a FIFO or a device named `*.md` would
        // block the open or the read (the open itself never waits, see `read_regular_capped`).
        let regular = e.file_type().is_ok_and(|t| t.is_file());
        let read = regular
            .then(|| crate::cli::work::read_regular_capped(&path, CUSTOM_MAX).ok())
            .flatten();
        let Some(Ok(text)) = read.map(String::from_utf8) else {
            left.push(file);
            continue;
        };
        let (what, body) = match text.split_once('\n') {
            Some((first, rest)) if first.starts_with("# ") => (first[2..].trim(), rest),
            None if text.starts_with("# ") => (text[2..].trim(), ""),
            _ => ("", text.as_str()),
        };
        out.push(Custom {
            name: name.to_owned(),
            what: if what.is_empty() {
                "tu comando".to_owned()
            } else {
                what.to_owned()
            },
            body: body.trim().to_owned(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    left.sort();
    (out, left)
}

/// What a person's command sends: its text with `$ARGUMENTS` replaced, or the arguments on a
/// line of their own after it when it has no `$ARGUMENTS`.
pub fn expand(body: &str, args: &str) -> String {
    if body.contains("$ARGUMENTS") {
        body.replace("$ARGUMENTS", args)
    } else if args.is_empty() {
        body.to_owned()
    } else {
        format!("{body}\n{args}")
    }
}

/// `/word args` as the person typed it: the word (without the slash) and the rest.
pub fn parse(input: &str) -> Option<(&str, &str)> {
    let rest = input.strip_prefix('/')?;
    let (word, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    Some((word, args.trim()))
}

pub fn builtin(word: &str) -> Option<&'static Command> {
    let w = word.to_lowercase();
    COMMANDS
        .iter()
        .find(|c| c.name == w || c.aliases.contains(&w.as_str()))
}

/// A row of the menu: a built-in or one of the person's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Builtin(usize),
    Custom(usize),
}

/// The commands whose name, alias or description has `filter`: the ones whose name or alias
/// starts with it first, built-ins before the person's.
pub fn matches(app: &App, filter: &str) -> Vec<Entry> {
    let f = filter.to_lowercase();
    let mut first = Vec::new();
    let mut then = Vec::new();
    for (i, c) in COMMANDS.iter().enumerate() {
        if c.name.starts_with(&f) || c.aliases.iter().any(|a| a.starts_with(&f)) {
            first.push(Entry::Builtin(i));
        } else if c.what.to_lowercase().contains(&f) || c.aliases.iter().any(|a| a.contains(&f)) {
            then.push(Entry::Builtin(i));
        }
    }
    for (i, c) in app.chat.customs.iter().enumerate() {
        if c.name.starts_with(&f) {
            first.push(Entry::Custom(i));
        } else if c.what.to_lowercase().contains(&f) {
            then.push(Entry::Custom(i));
        }
    }
    first.extend(then);
    first
}

fn entry_name(app: &App, e: Entry) -> String {
    match e {
        Entry::Builtin(i) => COMMANDS[i].name.to_owned(),
        Entry::Custom(i) => app.chat.customs[i].name.clone(),
    }
}

/// What a command picks from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What is applied (`claude-sonnet-5-5`, `low`, `opencode`).
    pub value: String,
    /// How it is shown.
    pub label: String,
    /// Why it cannot be chosen, if it cannot.
    pub blocked: Option<String>,
    pub current: bool,
}

/// The menu over the composer: the commands that match what is typed, or the choices of one.
#[derive(Debug, Default)]
pub struct Menu {
    pub sel: usize,
    /// A command's choices; `None` while they load.
    pub pick: Option<(Run, Option<Vec<Choice>>)>,
    /// Where the last draw put the rows: the area, the first row shown, and what the rows said.
    pub rows_at: Cell<(Rect, usize, u64)>,
}

/// The most rows the menu shows; past them it scrolls.
pub const MENU_ROWS: usize = 8;

/// The menu opens when `/` is the first thing typed in an empty composer; the person's commands
/// are read again each time.
pub fn open(app: &mut App) -> Vec<Cmd> {
    app.chat.menu = Some(Menu::default());
    vec![Cmd::LoadCommands]
}

/// After the composer changed: the menu closes when the text no longer starts with `/`, and a
/// command's choices give way to the list again when the text changes.
pub fn sync(app: &mut App, before: &str) {
    let c = &mut app.chat;
    if !c.input.starts_with('/') {
        c.menu = None;
    } else if let Some(m) = &mut c.menu
        && c.input != before
    {
        m.pick = None;
        m.sel = 0;
    }
}

/// How many rows the menu has now.
fn len(app: &App) -> usize {
    match app.chat.menu.as_ref().map(|m| &m.pick) {
        Some(Some((_, Some(items)))) => items.len(),
        Some(Some((_, None))) => 0,
        Some(None) => matches(app, filter(app)).len(),
        None => 0,
    }
}

fn filter(app: &App) -> &str {
    parse(&app.chat.input).map_or("", |(w, _)| w)
}

/// The keys the open menu takes: ↑ ↓ move, Tab completes, Enter runs, Esc closes it (the text
/// stays). `None`: not a menu key, it goes on to the composer.
pub fn key(app: &mut App, k: &KeyEvent) -> Option<Vec<Cmd>> {
    app.chat.menu.as_ref()?;
    if k.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    let n = len(app);
    let m = app.chat.menu.as_mut()?;
    match k.code {
        KeyCode::Esc => app.chat.menu = None,
        KeyCode::Up => m.sel = m.sel.checked_sub(1).unwrap_or(n.saturating_sub(1)),
        KeyCode::Down => m.sel = if m.sel + 1 >= n { 0 } else { m.sel + 1 },
        KeyCode::Tab if m.pick.is_none() => {
            let sel = m.sel;
            if let Some(e) = matches(app, filter(app)).get(sel).copied() {
                let name = entry_name(app, e);
                app.chat.set_input(format!("/{name} "));
                let m = app.chat.menu.get_or_insert_with(Menu::default);
                m.sel = 0;
            }
        }
        KeyCode::Enter => return Some(enter(app)),
        _ => return None,
    }
    Some(vec![])
}

/// Enter: the choice under the cursor, or the command typed whole, or the one selected.
pub fn enter(app: &mut App) -> Vec<Cmd> {
    if let Some(Menu {
        pick: Some((run, Some(items))),
        sel,
        ..
    }) = &app.chat.menu
    {
        let Some(choice) = items.get(*sel) else {
            return vec![];
        };
        if let Some(why) = &choice.blocked {
            app.flash = Some(clean(why));
            return vec![];
        }
        let (run, value) = (*run, choice.value.clone());
        return start(app, Target::Builtin(run), &value);
    }
    if matches!(&app.chat.menu, Some(Menu { pick: Some(_), .. })) {
        return vec![];
    }
    let Some((word, args)) = parse(&app.chat.input) else {
        return vec![];
    };
    let (word, args) = (word.to_owned(), args.to_owned());
    if let Some(t) = find(app, &word) {
        return start(app, t, &args);
    }
    // With arguments only a whole name runs: `/m sonnet` must not become `/model sonnet`.
    let sel = app.chat.menu.as_ref().map_or(0, |m| m.sel);
    match app
        .chat
        .menu
        .as_ref()
        .filter(|_| args.is_empty())
        .and_then(|_| matches(app, &word).get(sel).copied())
    {
        Some(Entry::Builtin(i)) => start(app, Target::Builtin(COMMANDS[i].run), &args),
        Some(Entry::Custom(i)) => start(app, Target::Custom(i), &args),
        None => {
            app.flash = Some(format!("no existe el comando /{}", clean(&word)));
            vec![]
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Builtin(Run),
    Custom(usize),
}

/// The command called exactly `word` (name or alias); a built-in wins over a file of that name.
pub fn find(app: &App, word: &str) -> Option<Target> {
    builtin(word).map(|c| Target::Builtin(c.run)).or_else(|| {
        app.chat
            .customs
            .iter()
            .position(|c| c.name == word)
            .map(Target::Custom)
    })
}

fn command(run: Run) -> &'static Command {
    COMMANDS
        .iter()
        .find(|c| c.run == run)
        .unwrap_or(&COMMANDS[0])
}

/// Runs a command with what was typed after it. One that changes the session waits for the
/// chat to be idle (`App::busy` says why); the text then stays where it was.
pub fn start(app: &mut App, target: Target, args: &str) -> Vec<Cmd> {
    let custom = match target {
        Target::Custom(i) => Some(app.chat.customs[i].clone()),
        Target::Builtin(run) => {
            if command(run).idle && app.busy() {
                return vec![];
            }
            None
        }
    };
    let done = |app: &mut App| {
        app.chat.menu = None;
        app.chat.set_input(String::new());
    };
    if let Some(cmd) = custom {
        done(app);
        return super::chat::send_text(app, expand(&cmd.body, args));
    }
    let Target::Builtin(run) = target else {
        return vec![];
    };
    let draft = app.chat.session_id.is_empty();
    let pick = |app: &mut App, run| {
        let m = app.chat.menu.get_or_insert_with(Menu::default);
        m.pick = Some((run, None));
        m.sel = 0;
        vec![Cmd::Choices(run)]
    };
    match run {
        Run::Model | Run::Connector | Run::Effort if args.is_empty() => return pick(app, run),
        Run::Model => {
            done(app);
            return vec![Cmd::SetModel(args.to_owned())];
        }
        Run::Connector => {
            done(app);
            return vec![Cmd::SetConnector(args.to_owned())];
        }
        Run::Effort => {
            let Some(level) = level_of(args) else {
                app.flash = Some(format!(
                    "no conozco el esfuerzo «{}»: {}",
                    clean(args),
                    clio_brain::pi::launch::THINKING.join(", ")
                ));
                return vec![];
            };
            done(app);
            return vec![Cmd::SetEffort(level.to_owned())];
        }
        Run::Name if args.is_empty() => {
            app.flash = Some("escribe el título: /name <título>".into());
            return vec![];
        }
        Run::Name | Run::Compact | Run::Actions | Run::Undo if draft => {
            app.flash = Some("la sesión empieza con tu primer mensaje: todavía no hay nada".into());
            return vec![];
        }
        _ => {}
    }
    done(app);
    let id = app.chat.session_id.clone();
    match run {
        Run::New => super::sessions::open_new(app),
        Run::Sessions => {
            app.screen = Screen::Sessions;
            vec![]
        }
        Run::Name => vec![Cmd::SessionName(args.to_owned())],
        Run::Compact => {
            app.chat.compacting = true;
            app.chat
                .items
                .push(super::chat::Item::Note("compactando el contexto…".into()));
            vec![Cmd::Compact(args.to_owned())]
        }
        Run::Context | Run::Usage => {
            app.modal = Some(numbers(app, run == Run::Context));
            vec![]
        }
        Run::Pending => vec![Cmd::LoadPending { session: id }],
        Run::Actions => vec![Cmd::LoadActions { run: id }],
        Run::Undo => {
            let mut c = Confirm::new(Action::UndoSession, id, 0, app.chat.workspace.clone());
            c.note = Some("Lo que la sesión cambió vuelve a como estaba.".into());
            app.confirm = Some(c);
            vec![]
        }
        Run::Copy => match last_answer(app) {
            Some(text) => {
                app.flash = Some("copiado".into());
                vec![Cmd::Clipboard(text)]
            }
            None => {
                app.flash = Some("todavía no hay una respuesta que copiar".into());
                vec![]
            }
        },
        Run::Memory => super::memory::opened(app),
        Run::Doctor => {
            app.screen = Screen::Doctor;
            app.doctor.loading = true;
            vec![Cmd::RunDoctor]
        }
        Run::Help => {
            app.open_help();
            vec![]
        }
        Run::Quit => {
            app.quit = true;
            vec![]
        }
        Run::Model | Run::Connector | Run::Effort => vec![],
    }
}

/// The agent's last answer, as the screen shows it: what the model wrote can carry escapes,
/// control and invisible or bidi characters, and a clipboard is pasted into other terminals.
fn last_answer(app: &App) -> Option<String> {
    app.chat.items.iter().rev().find_map(|i| match i {
        super::chat::Item::Assistant { text, .. } if !text.trim().is_empty() => {
            Some(super::sanitize::multiline(text))
        }
        _ => None,
    })
}

/// The session's numbers, from Pi's own totals: what `/context` and `/usage` show (the context
/// and usage screens belong to the builtin loop's runs).
fn numbers(app: &App, context: bool) -> Modal {
    use crate::cli::usage::{self, tokens};
    let c = &app.chat;
    let s = c.snapshot(app.now_ms, app.fallback());
    let t = c.meter.total();
    let pct = s
        .ctx_percent()
        .map_or_else(|| "sin dato todavía".to_owned(), |p| format!("{p} %"));
    let lines = if context {
        vec![
            format!(
                "contexto      {} de {} tokens ({pct})",
                s.ctx.map_or_else(|| "?".to_owned(), tokens),
                tokens(s.window)
            ),
            format!("compactado    {} vez/veces", c.meter.compactions),
            String::new(),
            "/compact resume la conversación y libera contexto.".to_owned(),
        ]
    } else {
        vec![
            format!("entrada       {} tokens", tokens(t.input)),
            format!("de caché      {} tokens", tokens(t.cache_read)),
            format!("salida        {} tokens", tokens(t.output)),
            format!("costo         {}", usage::usd(s.cost_micro)),
            format!("pasos         {}", s.steps),
            format!("tiempo        {}", usage::duration(s.secs)),
            format!("contexto      {pct}"),
        ]
    };
    Modal::text(if context { "Contexto" } else { "Uso" }, lines)
}

/// A row of the menu: its head, what it does, whether it is the current one, whether it is blocked.
type Row = (String, String, bool, bool);

/// The menu's rows, and what it says when it has none.
fn rows(app: &App, m: &Menu) -> (Vec<Row>, Option<String>) {
    let mut rows: Vec<Row> = Vec::new();
    let mut empty = None;
    match &m.pick {
        Some((_, None)) => empty = Some("cargando…".to_owned()),
        Some((run, Some(items))) => {
            for ch in items {
                let note = ch.blocked.clone().unwrap_or_default();
                rows.push((ch.label.clone(), note, ch.current, ch.blocked.is_some()));
            }
            if items.is_empty() {
                empty = Some(format!("/{} no tiene opciones aquí", command(*run).name));
            }
        }
        None => {
            let f = filter(app);
            for e in matches(app, f) {
                let (name, args, what) = match e {
                    Entry::Builtin(i) => {
                        let c = &COMMANDS[i];
                        (c.name.to_owned(), c.args, c.what.to_owned())
                    }
                    Entry::Custom(i) => {
                        let c = &app.chat.customs[i];
                        (c.name.clone(), "", c.what.clone())
                    }
                };
                let head = if args.is_empty() {
                    format!("/{name}")
                } else {
                    format!("/{name} {args}")
                };
                rows.push((head, what, false, false));
            }
            if rows.is_empty() {
                empty = Some(format!("no hay comandos con «{}»", clean(f)));
            }
        }
    }
    (rows, empty)
}

/// What the rows say, so a click is matched against the rows that were drawn.
fn rows_id(rows: &[Row]) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    rows.hash(&mut h);
    h.finish()
}

/// The menu, drawn over the conversation (from `top` down) just above the status line and the
/// composer (`input`), as wide as it.
pub fn draw(f: &mut Frame, input: Rect, top: u16, app: &App) {
    let Some(m) = &app.chat.menu else {
        return;
    };
    let t = app.theme;
    let (rows, empty) = rows(app, m);
    // Over the conversation, leaving the status line under it (it says why a command did not run).
    let bottom = input.y.saturating_sub(1);
    let room = usize::from(bottom.saturating_sub(top).saturating_sub(2));
    let shown = rows.len().clamp(1, MENU_ROWS).min(room);
    if shown == 0 {
        return;
    }
    let h = u16::try_from(shown).unwrap_or(1) + 2;
    let area = Rect::new(input.x, bottom.saturating_sub(h), input.width, h);
    let first = (m.sel + 1).saturating_sub(shown);
    m.rows_at.set((
        Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            u16::try_from(shown).unwrap_or(1),
        ),
        first,
        rows_id(&rows),
    ));
    let w = usize::from(area.width.saturating_sub(2));
    let head_w = rows
        .iter()
        .map(|(h, ..)| text::width(&clean(h)))
        .max()
        .unwrap_or(0)
        .min(w / 2)
        + 2;
    let mut lines = Vec::new();
    if let Some(e) = empty {
        lines.push(Line::from(Span::styled(format!(" {e}"), t.dim())));
    }
    for (i, (head, what, current, blocked)) in rows.iter().enumerate().skip(first).take(shown) {
        let mark = if *current { t.open_mark() } else { " " };
        let head = text::fit(&clean(head), head_w - 2, t.ellipsis());
        let pad = " ".repeat(head_w - text::width(&head));
        let what = text::fit(&clean(what), w.saturating_sub(head_w + 4), t.ellipsis());
        let used = 3 + text::width(&head) + pad.len() + text::width(&what);
        let fill = " ".repeat(w.saturating_sub(used));
        let line = if i == m.sel {
            let s = t.selection();
            Line::from(vec![Span::styled(
                format!(" {mark} {head}{pad}{what}{fill}"),
                s,
            )])
        } else {
            let head_style = if *blocked { t.dim() } else { t.bold() };
            Line::from(vec![
                Span::styled(format!(" {mark} "), t.accent_line()),
                Span::styled(head, head_style),
                Span::raw(pad),
                Span::styled(what, if *blocked { t.warn() } else { t.dim() }),
            ])
        };
        lines.push(line);
    }
    f.render_widget(Clear, area);
    let (title, keys) = match &m.pick {
        Some((run, _)) => (
            format!(" /{} ", command(*run).name),
            " Enter elige · Esc cierra ",
        ),
        None => (String::new(), " Tab completa · Enter ejecuta · Esc cierra "),
    };
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_set(t.border())
        .border_style(t.accent_line())
        .title_bottom(Line::from(Span::styled(keys, t.dim())).right_aligned());
    if !title.is_empty() {
        block = block.title(Span::styled(title, t.bold()));
    }
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// A click on a row of the menu runs it, as Enter would; only when the rows are still the ones
/// that were drawn (a click on «cargando…» never picks a choice that came after it).
pub fn click(app: &mut App, x: u16, y: u16) -> Option<Vec<Cmd>> {
    let m = app.chat.menu.as_ref()?;
    let (area, first, drawn) = m.rows_at.get();
    if !area.contains(ratatui::layout::Position::new(x, y)) {
        return None;
    }
    let i = first + usize::from(y - area.y);
    if i >= len(app) || rows_id(&rows(app, m).0) != drawn {
        return Some(vec![]);
    }
    if let Some(m) = &mut app.chat.menu {
        m.sel = i;
    }
    Some(enter(app))
}

/// A box of lines over the chat (`/context`, `/usage`, `/pending`, `/actions`): ↑ ↓ and the
/// page keys move in it, Esc or q closes it. In `/pending` each line is an action set aside,
/// and Enter asks about the one selected, with the same question (and rules) as after a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modal {
    pub title: String,
    pub lines: Vec<String>,
    /// `/pending`: for each line, the action's id and its «class» and what.
    pub picks: Vec<(String, String)>,
    /// A last line, under the list.
    pub foot: Option<String>,
    pub sel: usize,
    pub scroll: usize,
    /// How far the last draw let it scroll.
    pub max: Cell<usize>,
    /// The first columns of each line are a lead (`/actions`: time, tool, outcome); the rest is
    /// a command, in the code color, and its wrapped rows start under it. 0: plain lines.
    pub hang: usize,
}

impl Modal {
    pub fn text(title: &str, lines: Vec<String>) -> Self {
        Self {
            title: title.to_owned(),
            lines,
            picks: vec![],
            foot: None,
            sel: 0,
            scroll: 0,
            max: Cell::new(0),
            hang: 0,
        }
    }
}

pub fn modal_key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let Some(m) = app.modal.as_mut() else {
        return vec![];
    };
    let picks = m.picks.len();
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => app.modal = None,
        KeyCode::Enter if picks > 0 => {
            let (id, what) = m.picks[m.sel.min(picks - 1)].clone();
            app.modal = None;
            let mut c = Confirm::new(Action::Approve, id, 0, what);
            c.session.clone_from(&app.chat.session_id);
            app.confirm = Some(c);
        }
        KeyCode::Enter => app.modal = None,
        KeyCode::Up if picks > 0 => m.sel = m.sel.saturating_sub(1),
        KeyCode::Down if picks > 0 => m.sel = (m.sel + 1).min(picks - 1),
        KeyCode::Up => m.scroll = m.scroll.saturating_sub(1),
        KeyCode::Down => m.scroll = (m.scroll + 1).min(m.max.get()),
        KeyCode::PageUp => m.scroll = m.scroll.saturating_sub(10),
        KeyCode::PageDown => m.scroll = (m.scroll + 10).min(m.max.get()),
        _ => {}
    }
    vec![]
}

/// The box, centered: a line too long for it wraps (one that fits keeps its spaces, so columns
/// stay lined up), a list keeps one row per line; it scrolls past the screen.
pub fn draw_modal(f: &mut Frame, area: Rect, app: &App, m: &Modal) {
    let t = app.theme;
    let w = area.width.saturating_sub(6).min(76);
    let inner = usize::from(w.saturating_sub(6)).max(1);
    let mut rows: Vec<Line<'static>> = Vec::new();
    for (i, l) in m.lines.iter().enumerate() {
        let l = clean(l);
        if !m.picks.is_empty() {
            let mark = if i == m.sel { t.sel() } else { " " };
            let row = text::fit(&format!("{mark} {l}"), inner, t.ellipsis());
            let pad = " ".repeat(inner.saturating_sub(text::width(&row)));
            let style = if i == m.sel { t.selection() } else { t.plain() };
            rows.push(Line::from(Span::styled(format!("{row}{pad}"), style)));
        } else if m.hang > 0 && inner > m.hang + 8 {
            let cut = l.char_indices().nth(m.hang).map_or(l.len(), |(b, _)| b);
            let (lead, cmd) = l.split_at(cut);
            for (j, row) in text::wrap(cmd, inner - m.hang).into_iter().enumerate() {
                let lead = if j == 0 {
                    Span::styled(lead.to_owned(), t.dim())
                } else {
                    Span::raw(" ".repeat(m.hang))
                };
                rows.push(Line::from(vec![lead, Span::styled(row, t.code())]));
            }
        } else if text::width(&l) <= inner {
            rows.push(Line::from(l));
        } else {
            rows.extend(text::wrap(&l, inner).into_iter().map(Line::from));
        }
    }
    // Room for the border, the foot and the keys.
    let fixed = if m.foot.is_some() { 6 } else { 4 };
    let room = usize::from(area.height.saturating_sub(4 + fixed)).max(1);
    let max = rows.len().saturating_sub(room);
    m.max.set(max);
    let first = if m.picks.is_empty() {
        m.scroll.min(max)
    } else {
        (m.sel + 1).saturating_sub(room)
    };
    let mut lines: Vec<Line<'static>> = rows.into_iter().skip(first).take(room).collect();
    if let Some(foot) = &m.foot {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            text::fit(&clean(foot), inner, t.ellipsis()),
            t.dim(),
        )));
    }
    let d = format!("   {}   ", t.dot());
    let mut hint = vec![];
    if !m.picks.is_empty() {
        hint.push(Span::styled("Enter", t.key()));
        hint.push(Span::styled(" preguntar por esta", t.dim()));
        hint.push(Span::styled(d.clone(), t.dim()));
    } else if max > 0 {
        hint.push(Span::styled(format!("{}{}", t.up(), t.down()), t.key()));
        hint.push(Span::styled(" moverse", t.dim()));
        hint.push(Span::styled(d.clone(), t.dim()));
    }
    hint.push(Span::styled("Esc", t.key()));
    hint.push(Span::styled(" cerrar", t.dim()));
    lines.push(Line::from(""));
    lines.push(Line::from(hint));
    let h = u16::try_from(lines.len() + 4).unwrap_or(u16::MAX);
    let r = super::app::centered(area, w, h);
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_set(t.border())
        .border_style(t.accent_line())
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 1))
        .title(Span::styled(format!(" {} ", m.title), t.bold()));
    f.render_widget(Paragraph::new(lines).block(block), r);
}
