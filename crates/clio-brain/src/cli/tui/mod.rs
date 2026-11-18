//! `clio` with no arguments in a terminal: the TUI.
//!
//! `app` holds the state and the pure `update`; the screens (`home`, `new_run`, `live`, `detail`,
//! `doctor`, and the views of `meter`) hold their state, keys and drawing; `keys` is the one key
//! map; `status` is the status bar; `sanitize` cleans every outside text before it is drawn;
//! `theme` has the colors and symbols. This file is the only one that touches the terminal and starts
//! threads: it turns `Cmd`s into work on worker threads and their answers into `Input::Bg`.

pub mod app;
pub mod ask;
pub mod chat;
pub mod commands;
pub mod composer;
pub mod debug_view;
pub mod detail;
pub mod doctor;
pub mod home;
pub mod keys;
pub mod live;
pub mod markdown;
pub mod memory;
pub mod meter;
pub mod new_run;
pub mod sanitize;
pub mod sessions;
pub mod status;
pub mod text;
pub mod theme;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ask;
#[cfg(test)]
mod tests_commands;
#[cfg(test)]
mod tests_gallery;
#[cfg(test)]
mod tests_memory;
#[cfg(test)]
mod tests_views;

use std::io::{IsTerminal, Stdout};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clio_brain::agent::{Event, Observer};
use clio_brain::memory_link::{EmbedModel, MemoryLink};
use clio_brain::pi::rpc::Cancel;
use clio_memory::Signal;
use crossterm::event::{self, Event as Term, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use self::app::{App, Bg, ChatInfo, Cmd, Input, OpenTarget, Planned, draw, update};
use self::detail::Detail;
use self::home::{Header, KeyState};
use self::memory::MemoryData;
use self::new_run::Preview;
use super::apply;
use super::chat::{Harness, Session, SessionSource, Target};
use super::debug::DebugCtl;
use super::home::{Config, Dirs, Mode};
use super::runs::{self, RunOpts};
use super::usage::{self, RunLimits, RunUsage};
use super::{Res, work};

fn memory_id(s: &str) -> Result<clio_memory::MemoryId, String> {
    s.parse::<i64>()
        .map(clio_memory::MemoryId)
        .map_err(|_| format!("id de recuerdo no válido: {s}"))
}

static IN_TUI: AtomicBool = AtomicBool::new(false);

/// Gives the terminal back: normal screen, cooked mode, visible cursor. Safe to call twice.
pub fn restore_terminal() {
    let mut out = std::io::stdout();
    // Once is enough: leaving the alternate screen again would jump the cursor back to where it
    // was saved and could overwrite the panic message that was just printed.
    if !out.is_terminal() || !IN_TUI.swap(false, Ordering::SeqCst) {
        return;
    }
    let _ = execute!(
        out,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = disable_raw_mode();
}

/// A panic on the main thread must not leave the terminal in the alternate screen in raw mode:
/// restore it first, then let the usual message print on the normal screen. A panic in a worker
/// thread is caught where it happens and reported on screen, so it prints nothing here.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            restore_terminal();
            previous(info);
        }
    }));
}

/// The terminal's "set clipboard" sequence (OSC 52) for `text`, at most 64 KiB of it: base64, so
/// nothing in the text can end the sequence or reach the terminal as a command.
pub fn osc52(text: &str) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut end = text.len().min(64 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let bytes = &text.as_bytes()[..end];
    let mut out = String::from("\x1b]52;c;");
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out.push('\x07');
    out
}

/// Restores the terminal when dropped, on every way out of `run`.
struct Guard;

impl Guard {
    /// `mouse`: the Pi harness takes clicks and the wheel (selecting text then needs Shift).
    fn enter(mouse: bool) -> Res<(Self, Terminal<CrosstermBackend<Stdout>>)> {
        let e = |x: std::io::Error| format!("terminal: {x}");
        enable_raw_mode().map_err(e)?;
        IN_TUI.store(true, Ordering::SeqCst);
        let guard = Self;
        let mut out = std::io::stdout();
        execute!(
            out,
            EnterAlternateScreen,
            crossterm::event::EnableBracketedPaste
        )
        .map_err(e)?;
        if mouse {
            execute!(out, crossterm::event::EnableMouseCapture).map_err(e)?;
        }
        let term = Terminal::new(CrosstermBackend::new(out)).map_err(e)?;
        Ok((guard, term))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Opens the TUI and returns the exit code.
pub fn run(dirs: Dirs, debug: bool, harness: Harness, mode: Option<Mode>) -> Res<i32> {
    install_panic_hook();
    // SIGTERM and SIGHUP (the window closes, `kill`) leave through the normal way out, so the
    // guard gives the terminal back; a handler cannot do that itself, it only sets this.
    let ended = Arc::new(AtomicUsize::new(0));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
        signal_hook::flag::register_usize(sig, ended.clone(), sig as usize)
            .map_err(|e| format!("señales: {e}"))?;
    }
    let (_guard, mut term) = Guard::enter(harness == Harness::Pi)?;
    let (tx, rx) = channel::<Input>();
    let dirs = Arc::new(dirs);
    let ctx = Ctx {
        sources: Arc::new(super::chat::sources(dirs.clone(), mode)),
        chat: Arc::default(),
        chat_cancel: Mutex::new(Cancel::new()),
        dirs,
        tx,
        cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
        debug: DebugCtl::new(debug, false),
        closing: Arc::default(),
        answered: AtomicBool::new(false),
    };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut app = App::new(&cwd, theme::Theme::from_env());
    app.debug = debug;
    if harness == Harness::Pi {
        app.use_pi();
        // As `LocalSource::open` decides it for a new session.
        app.host = mode
            .or_else(|| Config::load(&ctx.dirs).ok().map(|c| c.mode))
            .unwrap_or_default()
            == Mode::Host;
    }
    for c in app.start_cmds() {
        ctx.exec(c);
    }
    let t0 = Instant::now();
    // Only for the terminal-restoration test: a deliberate panic inside the alternate screen.
    // Test hook for the terminal-restore test; compiled out of release builds (what `just install` ships).
    if cfg!(debug_assertions) && std::env::var_os("CLIO_TUI_PANIC_TEST").is_some() {
        panic!("deliberate panic to test that the terminal is restored");
    }
    let to_ms = |t: Instant| u64::try_from(t.duration_since(t0).as_millis()).unwrap_or(u64::MAX);
    // Only for the pseudo-terminal tests (debug builds): the text of every frame, to a file.
    let dump = std::env::var_os("CLIO_TUI_SCREEN_DUMP").filter(|_| cfg!(debug_assertions));
    // The terminal is read on its own thread: crossterm's reader loops forever, at full speed and
    // without returning, once the terminal is gone (a read that gives 0 bytes or EIO is retried),
    // so this loop never waits inside it and can always see a signal or a dead terminal.
    let (gone, reading) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(true)),
    );
    read_terminal(ctx.tx.clone(), gone.clone(), reading.clone());
    // Drawn when something came in (a key, a click, a resize, a background event), and every
    // 100 ms only while something on screen moves or counts (`App::animating`).
    let (mut dirty, mut drawn) = (true, Instant::now());
    while !app.quit && ended.load(Ordering::SeqCst) == 0 && !gone.load(Ordering::SeqCst) {
        if dirty
            || (app.animating() && drawn.elapsed() >= Duration::from_millis(100))
            || app.stale_after().is_some_and(|t| drawn.elapsed() >= t)
        {
            match term.draw(|f| draw(f, &app)) {
                Ok(frame) => {
                    if let Some(path) = &dump {
                        dump_screen(frame.buffer, std::path::Path::new(path));
                    }
                }
                // Nothing can be shown any more: leave as if the window closed.
                Err(_) => break,
            }
            (dirty, drawn) = (false, Instant::now());
        }
        // A terminal that hung up says so here even when no signal came.
        if !std::io::stdout().is_terminal() {
            break;
        }
        // Waits for an input or the next frame; never spins.
        let first = rx.recv_timeout(Duration::from_millis(100)).ok();
        // The clock first: a key is timed when it comes (the quiet rule of a question).
        let mut inputs: Vec<Input> = vec![Input::Tick(to_ms(Instant::now()))];
        inputs.extend(first);
        while let Ok(i) = rx.try_recv() {
            inputs.push(i);
        }
        dirty |= inputs.len() > 1;
        for i in inputs {
            let (next, cmds) = update(app, i);
            app = next;
            for c in cmds {
                if c == Cmd::Redraw {
                    // Not `Terminal::clear`: it asks the terminal where the cursor is and waits
                    // for an answer that some terminals never give. A new `Terminal` has empty
                    // buffers, so the next draw repaints every cell.
                    let cleared = execute!(
                        std::io::stdout(),
                        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
                    );
                    match Terminal::new(CrosstermBackend::new(std::io::stdout())) {
                        Ok(t) if cleared.is_ok() => term = t,
                        _ => gone.store(true, Ordering::SeqCst),
                    }
                } else {
                    ctx.exec(c);
                }
            }
        }
    }
    reading.store(false, Ordering::SeqCst);
    // A dead terminal leaves like a signal: no summary (nobody is there to wait for it).
    let cut = ended.load(Ordering::SeqCst) != 0 || !app.quit;
    // Leaving on purpose (not by a signal, nor a terminal that went away): the session's summary
    // first, on the normal screen so the wait is said; a signal or a panic skips it.
    if !cut {
        let open = ctx.current_chat();
        let closing: Vec<_> = ctx
            .closing
            .lock()
            .map(|mut v| v.drain(..).filter(|h| !h.is_finished()).collect())
            .unwrap_or_default();
        if open.is_some() || !closing.is_empty() {
            restore_terminal();
            eprintln!(
                "clio: cerrando la sesión de Pi y guardando su resumen (unos 35 s como mucho; Ctrl-C para no esperar)…"
            );
            if let Some(s) = open {
                s.summarize();
            }
            for h in closing {
                let _ = h.join();
            }
        }
    }
    // Pi and its toolhost die here, before the terminal is given back (and in `Drop` on a panic).
    ctx.stop_chat_now();
    // An approval runs in this process, with its own host launcher: its units carry this pid and
    // outlive it (a signal can come while one runs).
    if ctx.answered.load(Ordering::SeqCst) {
        clio_node::host::stop_units_of(std::process::id());
    }
    // The shell convention for a process ended by a signal; a terminal that hung up is SIGHUP.
    Ok(match ended.load(Ordering::SeqCst) {
        0 if cut => 128 + signal_hook::consts::SIGHUP,
        0 => 0,
        sig => 128 + sig as i32,
    })
}

/// The terminal's events, to `tx`, until `reading` goes off. An error (the terminal is gone) sets
/// `gone` and ends the thread; the run loop notices on its next pass.
fn read_terminal(tx: Sender<Input>, gone: Arc<AtomicBool>, reading: Arc<AtomicBool>) {
    let flag = SetOnDrop(gone);
    // A thread that cannot start drops `flag` with its closure, so that sets `gone` too.
    let _ = std::thread::Builder::new()
        .name("clio-terminal".into())
        .spawn(move || {
            // Set however the thread ends, a panic too: the run loop then leaves the normal way
            // and gives the terminal back, instead of waiting for keys that never come.
            let _flag = flag;
            while reading.load(Ordering::SeqCst) {
                let ev = match event::poll(Duration::from_millis(100)) {
                    Ok(false) => continue,
                    Ok(true) => event::read(),
                    Err(e) => Err(e),
                };
                match ev {
                    Ok(ev) => {
                        if let Some(i) = input_of(ev) {
                            let _ = tx.send(i);
                        }
                    }
                    Err(_) => break,
                }
            }
        });
}

/// Sets its flag when dropped, also while a panic unwinds.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// What a terminal event is to `update`, if anything. The terminal reports every move of the
/// pointer (crossterm turns on any-motion tracking): a move, a drag or a release changes nothing,
/// so it is dropped here and costs no frame.
fn input_of(ev: Term) -> Option<Input> {
    use crossterm::event::MouseEventKind as M;
    match ev {
        Term::Resize(..) => Some(Input::Resize),
        Term::Key(k) if k.kind != KeyEventKind::Release => Some(Input::Key(k)),
        Term::Paste(s) => Some(Input::Paste(s)),
        Term::Mouse(m) if !matches!(m.kind, M::Moved | M::Drag(_) | M::Up(_)) => {
            Some(Input::Mouse(m))
        }
        _ => None,
    }
}

/// The screen as plain text, replaced atomically.
fn dump_screen(buf: &ratatui::buffer::Buffer, path: &std::path::Path) {
    let a = buf.area;
    let mut text = String::new();
    for y in a.y..a.y + a.height {
        for x in a.x..a.x + a.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The open Pi session, if any, and which `ChatOpen` is the current one: an open that finishes
/// after the person left (or after a newer one started) shuts its own session down.
#[derive(Default)]
struct Slot {
    latest: u64,
    session: Option<Arc<dyn Session>>,
}

struct Ctx {
    sources: Arc<Vec<Box<dyn SessionSource>>>,
    chat: Arc<Mutex<Slot>>,
    chat_cancel: Mutex<Cancel>,
    dirs: Arc<Dirs>,
    tx: Sender<Input>,
    cancel: Mutex<Arc<AtomicBool>>,
    /// The debug switch the agent reads at every call; `Ctrl-D` flips it.
    debug: DebugCtl,
    /// Sessions being closed on their own thread (each may first save its summary): waited for
    /// when the person leaves, so leaving right after closing a chat does not cut it short.
    closing: Closing,
    /// An answer to a set-aside action ran here (see the end of `run`).
    answered: AtomicBool,
}

/// The threads closing a session (and saving its summary).
type Closing = Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>;

/// The session listed under `file` in Pi's sessions folder (and that folder).
fn session_by_file(
    d: &Dirs,
    file: &str,
) -> Result<(PathBuf, clio_brain::pi::sessions::SessionInfo), String> {
    let dir = crate::cli::pi::paths(d).sessions_dir();
    let s = clio_brain::pi::sessions::list_sessions(&dir)
        .sessions
        .into_iter()
        .find(|s| s.path.display().to_string() == file)
        .ok_or("esa sesión ya no está")?;
    Ok((dir, s))
}

/// Waits for every session being closed: a session is never opened again (two Pi on one file and
/// one working copy) while its last turn still runs.
fn wait_closing(closing: &Closing) {
    let pending: Vec<_> = closing
        .lock()
        .map(|mut v| v.drain(..).collect())
        .unwrap_or_default();
    for h in pending {
        let _ = h.join();
    }
}

/// Sends what the agent does to the screen.
struct Forward(Sender<Input>);

impl Observer for Forward {
    fn event(&mut self, e: &Event) {
        let _ = self.0.send(Input::Bg(Bg::Agent(e.clone())));
    }
}

fn expand(dir: &str, home: &std::path::Path) -> PathBuf {
    let d = dir.trim();
    match d.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if d == "~" => home.to_path_buf(),
        None => PathBuf::from(d),
    }
}

/// Whether a connector's key is there, without pulling its value.
fn has_key(dirs: &Dirs, c: &clio_brain::connector::Connector) -> bool {
    std::env::var_os(c.key_env).is_some_and(|v| !v.is_empty())
        || super::vault::has_model_key(dirs, c)
}

fn header(dirs: &Dirs) -> Header {
    let (provider, model, thinking, config_error, limits) = match Config::load(dirs) {
        Ok(c) => {
            let l = RunLimits::of(&c);
            (c.model.provider, c.model.model, c.model.thinking, None, l)
        }
        Err(e) => (
            String::new(),
            String::new(),
            String::new(),
            Some(e),
            RunLimits::default(),
        ),
    };
    let (window, window_known) = usage::window_for(dirs, &provider, &model);
    let active = clio_brain::connector::by_provider(&provider);
    let key = if active.is_some_and(|c| has_key(dirs, c)) {
        KeyState::Present
    } else {
        KeyState::Missing
    };
    let vault = if dirs
        .runtime
        .as_ref()
        .is_some_and(|r| r.join("vault-person.sock").exists())
    {
        "activa"
    } else if dirs.data.join("vault.bin").exists() {
        "creada, detenida (clio vault start)"
    } else {
        "sin crear (opcional)"
    };
    Header {
        model,
        thinking,
        key,
        connector: active.map_or(String::new(), |c| c.name.to_owned()),
        vault: vault.into(),
        config_error,
        limits,
        window,
        window_known,
    }
}

/// The picker's footer when a list is the catalog's: Pi was never asked for it.
const NO_PI_LIST: &str = "open a session once to see every model";

/// What `/model`, `/connector` and `/effort` pick from, and a footer for the picker. With a
/// session open, Pi says which models and levels it has; before the first message, the models
/// Pi offered last time (the catalog's before any) and every level.
fn choices(
    d: &Dirs,
    session: Option<&dyn Session>,
    run: commands::Run,
) -> Result<(Vec<commands::Choice>, Option<String>), String> {
    use clio_brain::connector::{self, CONNECTORS};
    use commands::{Choice, Run};
    let cfg = Config::load(d)?;
    let active = connector::by_provider(&cfg.model.provider).ok_or("sin conector")?;
    let choice = |value: &str, label: String, current: bool| Choice {
        value: value.to_owned(),
        label,
        blocked: None,
        current,
    };
    let mut note = None;
    let items = match run {
        Run::Model => {
            let usable = |c: &clio_brain::connector::Connector| {
                has_key(d, c) && !super::home::lacks_bridge(d, c)
            };
            let (v, n) = model_choices(d, session, active, &cfg.model.model, &usable)?;
            note = n;
            v
        }
        Run::Effort => levels(session)?
            .iter()
            .map(|l| choice(l, l.clone(), *l == cfg.model.thinking))
            .collect(),
        Run::Connector => CONNECTORS
            .iter()
            .map(|c| {
                let model = cfg
                    .model
                    .last
                    .get(c.provider)
                    .map_or(c.default_model, String::as_str);
                let mut ch = choice(
                    c.name,
                    format!("{} · {model}", c.name),
                    c.provider == active.provider,
                );
                ch.blocked = if !has_key(d, c) {
                    Some(format!("sin llave: clio connect {}", c.name))
                } else if super::home::lacks_bridge(d, c) {
                    Some("falta el puente de Claude: clio pi install".into())
                } else {
                    None
                };
                ch
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok((items, note))
}

/// `/model`'s choices: this session's connector first (the configured one before the first
/// message), then every other connector that is `usable` (its key in the vault), under its name:
/// choosing one of those is `/connector` and that model, for the next session. The footer says
/// when a list is the catalog's.
fn model_choices(
    d: &Dirs,
    session: Option<&dyn Session>,
    active: &'static clio_brain::connector::Connector,
    configured: &str,
    usable: &dyn Fn(&clio_brain::connector::Connector) -> bool,
) -> Result<(Vec<commands::Choice>, Option<String>), String> {
    let choice = |value: String, label: String, current: bool| commands::Choice {
        value,
        label,
        blocked: None,
        current,
    };
    let (here, current) = match session {
        Some(s) => session_model(s)?,
        None => (active, configured.to_owned()),
    };
    let (list, mut catalog) = models(d, session, here)?;
    let mut v: Vec<commands::Choice> = list
        .iter()
        .map(|m| choice(m.clone(), m.clone(), *m == current))
        .collect();
    for c in clio_brain::connector::CONNECTORS
        .iter()
        .filter(|c| c.provider != here.provider && usable(c))
    {
        let (list, from_catalog) = models(d, None, c)?;
        catalog |= from_catalog;
        v.extend(list.iter().map(|m| {
            choice(
                format!("{} {m}", c.name),
                format!("{} · {m}", c.name),
                false,
            )
        }));
    }
    Ok((v, catalog.then(|| NO_PI_LIST.to_owned())))
}

/// The connector and the model of the session open now, as Pi says them.
fn session_model(
    s: &dyn Session,
) -> Result<(&'static clio_brain::connector::Connector, String), String> {
    let st = s.command("get_state", serde_json::json!({}))?;
    let provider = st["model"]["provider"].as_str().unwrap_or_default();
    let c = clio_brain::connector::CONNECTORS
        .iter()
        .find(|c| c.pi_provider == provider)
        .ok_or("la sesión no tiene un conector de Clio")?;
    Ok((c, st["model"]["id"].as_str().unwrap_or_default().to_owned()))
}

/// The models of connector `c`, and whether the list is the catalog's: Pi's own list when a
/// session of it is open (kept for later), else the one Pi gave the last time a session of it
/// started, else the catalog's. The configured model is on it even when the list lacks it.
fn models(
    d: &Dirs,
    session: Option<&dyn Session>,
    c: &clio_brain::connector::Connector,
) -> Result<(Vec<String>, bool), String> {
    let (mut v, catalog) = match session {
        Some(s) => {
            let v = usage::pi_models_of(
                &s.command("get_available_models", serde_json::json!({}))?,
                c.pi_provider,
            );
            usage::save_pi_models(d, c.provider, &v);
            (v, false)
        }
        None => usage::pi_models(d, c.provider).map_or_else(
            || (usage::catalog_models(d, c.provider), true),
            |v| (v, false),
        ),
    };
    if session.is_none()
        && let Ok(cfg) = Config::load(d)
        && cfg.model.provider == c.provider
        && !v.contains(&cfg.model.model)
    {
        v.insert(0, cfg.model.model);
    }
    Ok((v, catalog))
}

/// The thinking levels: those the session's model has, or all of Pi's before it starts.
fn levels(session: Option<&dyn Session>) -> Result<Vec<String>, String> {
    let all = || {
        clio_brain::pi::launch::THINKING
            .iter()
            .map(|l| (*l).to_owned())
            .collect()
    };
    let Some(s) = session else {
        return Ok(all());
    };
    let v = s.command("get_available_thinking_levels", serde_json::json!({}))?;
    Ok(v["levels"].as_array().map_or_else(all, |a| {
        a.iter()
            .filter_map(|l| l.as_str())
            .filter(|l| clio_brain::pi::launch::THINKING.contains(l))
            .map(str::to_owned)
            .collect()
    }))
}

/// `/model <name>`: one of the connector's models, for the session open now (Pi switches it)
/// and as the default from now on.
fn set_model(d: &Dirs, session: Option<&dyn Session>, name: &str) -> Result<String, String> {
    // `<connector> <model>`: another connector's model is `/connector` and that model.
    if let Some((cn, model)) = name.trim().split_once(' ')
        && let Some(c) = clio_brain::connector::by_name(cn)
    {
        let (known, _) = models(d, None, c)?;
        let model = model.trim();
        if !known.iter().any(|m| m == model) {
            return Err(format!(
                "{} no tiene el modelo «{}»; tiene: {}",
                c.name,
                sanitize::line(model),
                known.join(", ")
            ));
        }
        return switch_connector(d, c.name, Some(model));
    }
    let c = match session {
        Some(s) => session_model(s)?.0,
        None => {
            let cfg = Config::load(d)?;
            clio_brain::connector::by_provider(&cfg.model.provider).ok_or("sin conector")?
        }
    };
    let (known, _) = models(d, session, c)?;
    // Only a name on the list: before the first message the catalog's (and the one configured),
    // with a session open Pi's. Anything else would be saved and break the next session.
    if !known.iter().any(|m| m == name) {
        return Err(format!(
            "{} no tiene el modelo «{}»; tiene: {}",
            c.name,
            sanitize::line(name),
            known.join(", ")
        ));
    }
    if let Some(s) = session {
        s.command(
            "set_model",
            serde_json::json!({ "provider": c.pi_provider, "modelId": name }),
        )?;
    }
    super::connect::edit_model(d, |m| super::connect::set_model(m, c.provider, name))?;
    Ok(format!("modelo: {name}"))
}

/// `/connector <name>`: saved for the next session (the one open keeps its connector: its
/// pending questions and its copy belong to it). Back to the model last chosen for it.
fn set_connector(d: &Dirs, name: &str) -> Result<String, String> {
    switch_connector(d, name, None)
}

/// `set_connector`, with `model` instead of the one last chosen for it.
fn switch_connector(d: &Dirs, name: &str, model: Option<&str>) -> Result<String, String> {
    let c = clio_brain::connector::by_name(name.trim()).ok_or_else(|| {
        format!(
            "no existe el conector «{}»: opencode, claude o anthropic-api",
            sanitize::line(name)
        )
    })?;
    if !has_key(d, c) {
        return Err(format!(
            "{} no tiene llave: clio connect {} en una terminal",
            c.name, c.name
        ));
    }
    if super::home::lacks_bridge(d, c) {
        return Err("falta el puente de Claude (pi-claude-agent-sdk): clio pi install".into());
    }
    let cfg = Config::load_with_provider(d, c.provider)?;
    let model = model.map(str::to_owned).unwrap_or_else(|| {
        cfg.model
            .last
            .get(c.provider)
            .cloned()
            .unwrap_or_else(|| c.default_model.to_owned())
    });
    super::connect::edit_model(d, |m| super::connect::set_model(m, c.provider, &model))?;
    Ok(format!(
        "conector: {} · {model}. Vale desde la próxima sesión: /new la empieza",
        c.name
    ))
}

/// `/effort <level>`: for the session open now (if its model has that level) and as the
/// default from now on.
fn set_effort(d: &Dirs, session: Option<&dyn Session>, level: &str) -> Result<String, String> {
    if !clio_brain::pi::launch::THINKING.contains(&level) {
        return Err(format!("«{}» no es un nivel de Pi", sanitize::line(level)));
    }
    if let Some(s) = session {
        let have = levels(Some(s))?;
        if !have.iter().any(|l| l == level) {
            return Err(format!(
                "este modelo no tiene el esfuerzo {level}; tiene: {}",
                have.join(", ")
            ));
        }
        s.command("set_thinking_level", serde_json::json!({ "level": level }))?;
    }
    super::connect::edit_model(d, |m| {
        m.insert("thinking".into(), level.into());
    })?;
    Ok(format!("esfuerzo: {level}"))
}

/// `/actions`: one row per action, the local time, the tool and the outcome in columns, and
/// what it did under the code color (see `Modal::hang`).
fn actions_box(log: &[serde_json::Value], offset: i64) -> commands::Modal {
    let rows: Vec<[String; 4]> = log
        .iter()
        .map(|v| super::pending::action_cells(v, offset))
        .collect();
    if rows.is_empty() {
        return commands::Modal::text(
            "Acciones",
            vec!["esta sesión no ejecutó ninguna acción en la máquina".into()],
        );
    }
    let w = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
    let (tool_w, out_w) = (w(1), w(2));
    let lines = rows
        .iter()
        .map(|[at, tool, out, what]| format!("{at}  {tool:<tool_w$}  {out:<out_w$}  {what}"))
        .collect();
    let mut m = commands::Modal::text("Acciones", lines);
    m.hang = 8 + 2 + tool_w + 2 + out_w + 2;
    m
}

fn check(dirs: &Dirs, dir: &str) -> Result<Preview, String> {
    if dir.trim().is_empty() {
        return Err("escribe la carpeta del proyecto".into());
    }
    let cfg = Config::load(dirs)?;
    let ws = work::validate(&expand(dir, &dirs.home), dirs)?;
    let scan = work::scan(&ws, cfg.workspace.max_files, cfg.workspace.max_bytes)?;
    Ok(Preview::new(ws.display().to_string(), scan))
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self.stop_chat_now();
    }
}

impl Ctx {
    /// Makes `latest` the current open and takes the open session out, if any.
    fn take_chat(&self, latest: u64) -> Option<Arc<dyn Session>> {
        let mut g = self
            .chat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.latest = latest;
        g.session.take()
    }

    /// Stops the session, here and now (the interface is leaving).
    fn stop_chat_now(&self) {
        if let Some(s) = self.take_chat(0) {
            s.shutdown();
        }
    }

    /// Stops the session on its own thread (shutting down can take a moment).
    fn stop_chat_later(&self, latest: u64) {
        if let Some(s) = self.take_chat(latest) {
            let h = std::thread::spawn(move || {
                s.summarize();
                s.shutdown();
            });
            if let Ok(mut v) = self.closing.lock() {
                v.retain(|h| !h.is_finished());
                v.push(h);
            }
        }
    }

    fn current_chat(&self) -> Option<Arc<dyn Session>> {
        self.chat.lock().ok().and_then(|g| g.session.clone())
    }

    fn send(&self, b: Bg) {
        let _ = self.tx.send(Input::Bg(b));
    }

    /// Runs `f` on its own thread; a panic inside it becomes an error message instead of
    /// a frozen screen.
    fn spawn<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Dirs, &Sender<Input>) -> T + Send + 'static,
        done: impl FnOnce(Result<T, String>) -> Bg + Send + 'static,
    ) {
        let (dirs, tx) = (self.dirs.clone(), self.tx.clone());
        let spawned = std::thread::Builder::new()
            .name("clio-worker".into())
            .spawn(move || {
                let r = catch_unwind(AssertUnwindSafe(|| f(&dirs, &tx))).map_err(|_| {
                    "error interno en un hilo de trabajo (es un fallo de Clio)".to_owned()
                });
                let _ = tx.send(Input::Bg(done(r)));
            });
        if let Err(e) = spawned {
            self.send(Bg::Done(Err(format!("no se pudo iniciar un hilo: {e}"))));
        }
    }

    fn exec(&self, cmd: Cmd) {
        match cmd {
            Cmd::LoadHome => self.spawn(
                |d, _| {
                    let list = runs::list_info(d, Some(100)).unwrap_or_default();
                    let last = list
                        .first()
                        .map_or_else(RunUsage::default, |r| RunUsage::load(&d.runs.join(&r.id)));
                    (header(d), list, last)
                },
                |r| match r {
                    Ok(v) => Bg::Home(Box::new(v)),
                    Err(e) => Bg::Done(Err(e)),
                },
            ),
            Cmd::Clipboard(text) => {
                use std::io::Write;
                let mut out = std::io::stdout();
                let _ = write!(out, "{}", osc52(&text));
                let _ = out.flush();
            }
            Cmd::RunDoctor => self.spawn(
                |d, _| super::doctor::collect(d),
                |r| match r {
                    Ok(v) => Bg::Doctor(v),
                    Err(e) => Bg::Done(Err(e)),
                },
            ),
            Cmd::Check { dir, seq } => self.spawn(
                move |d, _| check(d, &dir),
                move |r| Bg::Checked {
                    seq,
                    result: r.and_then(|x| x),
                },
            ),
            Cmd::LoadMemory => self.spawn(
                |d, _| MemoryData::load(d),
                |r| Bg::Memory(r.and_then(|x| x).map(Box::new)),
            ),
            Cmd::MemoryPromote { id } => self.spawn(
                move |d, _| {
                    let link = MemoryLink::open_write(&d.data, &EmbedModel::none())?.for_person();
                    link.promote(memory_id(&id)?)?;
                    Ok::<_, String>(format!("aprobado: m{id}; desde ahora se recuerda"))
                },
                |r| Bg::Done(r.and_then(|x| x)),
            ),
            Cmd::MemoryArchive { id } => self.spawn(
                move |d, _| {
                    let link = MemoryLink::open_write(&d.data, &EmbedModel::none())?.for_person();
                    link.archive(memory_id(&id)?)?;
                    Ok::<_, String>(format!("archivado: m{id}; ya no se recuerda"))
                },
                |r| Bg::Done(r.and_then(|x| x)),
            ),
            Cmd::MemoryFeedback { id, useful } => self.spawn(
                move |d, _| {
                    let link = MemoryLink::open_write(&d.data, &EmbedModel::none())?.for_person();
                    let signal = if useful {
                        Signal::Useful
                    } else {
                        Signal::Wrong
                    };
                    link.feedback(clio_memory::MemoryId(id), signal)?;
                    Ok::<_, String>(format!(
                        "anotado para m{id}: {} (nunca borra el recuerdo)",
                        if useful { "útil" } else { "equivocado" }
                    ))
                },
                |r| Bg::Done(r.and_then(|x| x)),
            ),
            Cmd::PlanSessionDelete { reference } => self.spawn(
                move |d, _| {
                    let (dir, s) = session_by_file(d, &reference)?;
                    let p = crate::cli::sessions::plan(d, &dir, &s)?;
                    Ok::<_, String>((reference, s.title, crate::cli::sessions::describe(&p)))
                },
                |r| Bg::SessionPlan(r.and_then(|x| x)),
            ),
            Cmd::DeleteSession { reference } => {
                let (sources, closing) = (self.sources.clone(), self.closing.clone());
                self.spawn(
                    move |d, _| {
                        // A session still closing (its summary) is never deleted under it.
                        wait_closing(&closing);
                        let done = session_by_file(d, &reference).and_then(|(dir, s)| {
                            let p = crate::cli::sessions::plan(d, &dir, &s)?;
                            crate::cli::sessions::delete(&p)?;
                            Ok(format!("sesión borrada: {}", sanitize::line(&s.title)))
                        });
                        (
                            done,
                            sources.iter().flat_map(|s| s.list()).collect::<Vec<_>>(),
                        )
                    },
                    |r| match r {
                        Ok((done, rows)) => Bg::SessionDeleted(done, rows),
                        Err(e) => Bg::Done(Err(e)),
                    },
                );
            }
            Cmd::Redraw => {}
            Cmd::LoadSessions => {
                let sources = self.sources.clone();
                self.spawn(
                    move |_, _| sources.iter().flat_map(|s| s.list()).collect::<Vec<_>>(),
                    |r| match r {
                        Ok(rows) => Bg::Sessions(rows),
                        Err(e) => Bg::Done(Err(e)),
                    },
                );
            }
            Cmd::ChatOpen { seq, target } => {
                self.stop_chat_later(seq);
                let (sources, slot) = (self.sources.clone(), self.chat.clone());
                let closing = self.closing.clone();
                self.spawn(
                    move |d, _| -> Result<Box<ChatInfo>, String> {
                        wait_closing(&closing);
                        let target = match target {
                            OpenTarget::New { project } => Target::New {
                                project: expand(&project, &d.home),
                            },
                            OpenTarget::Existing { reference } => Target::Existing { reference },
                        };
                        // The local source is the first; the remote rows are never openable.
                        let opened = sources
                            .first()
                            .ok_or("no hay fuente de sesiones")?
                            .open(&target)?;
                        let mut g = slot
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if g.latest != seq {
                            drop(g);
                            opened.session.shutdown();
                            return Err("cancelado".into());
                        }
                        g.session = Some(opened.session.clone());
                        Ok(Box::new(ChatInfo::from(&opened)))
                    },
                    move |r| Bg::ChatOpened {
                        seq,
                        result: r.and_then(|x| x),
                    },
                );
            }
            Cmd::ChatSend { text } => {
                let Some(session) = self.current_chat() else {
                    self.send(Bg::Done(Err("no hay una sesión abierta".into())));
                    return;
                };
                let cancel = Cancel::new();
                if let Ok(mut c) = self.chat_cancel.lock() {
                    *c = cancel.clone();
                }
                self.spawn(
                    move |_, tx| {
                        let mut forward = |e: &clio_brain::pi::rpc::Event| {
                            let _ = tx.send(Input::Bg(Bg::ChatEvent(e.clone())));
                        };
                        let result = session
                            .run_prompt(&text, &cancel, &mut forward)
                            .map_err(|e| e.to_string());
                        // Host mode: what the turn set aside is asked now, one question each.
                        let asks: Vec<(String, String)> = session
                            .new_pending()
                            .into_iter()
                            .map(|p| {
                                let what = super::pending::what(&p);
                                (
                                    p.id.clone(),
                                    format!("{} {what}", super::pending::label(&p)),
                                )
                            })
                            .collect();
                        if !asks.is_empty() {
                            let _ = tx.send(Input::Bg(Bg::ChatAsks(asks)));
                        }
                        (result, session.refresh().ok())
                    },
                    |r| match r {
                        Ok((result, refresh)) => Bg::ChatSettled { result, refresh },
                        Err(e) => Bg::ChatSettled {
                            result: Err(e),
                            refresh: None,
                        },
                    },
                );
            }
            Cmd::ChatSteer { text } => {
                if let Some(session) = self.current_chat() {
                    self.spawn(
                        move |_, _| session.steer(&text).map_err(|e| e.to_string()),
                        |r| match r.and_then(|x| x) {
                            Ok(()) => Bg::Done(Ok("mensaje enviado al agente".into())),
                            Err(e) => Bg::Done(Err(e)),
                        },
                    );
                }
            }
            Cmd::ChatAnswer { id, yes } => {
                let Some(session) = self.current_chat() else {
                    self.send(Bg::ChatAnswered(Err("no hay una sesión abierta".into())));
                    return;
                };
                self.answered.store(true, Ordering::SeqCst);
                self.spawn(
                    move |_, _| session.answer(&id, yes),
                    |r| Bg::ChatAnswered(r.and_then(|x| x)),
                );
            }
            // One line to Pi's stdin: no worker needed.
            Cmd::ChatUi { id, value } => {
                if let Some(session) = self.current_chat() {
                    session.ui_respond(&id, value.as_deref());
                }
            }
            Cmd::ChatAbort => {
                if let Ok(c) = self.chat_cancel.lock() {
                    c.cancel();
                }
            }
            Cmd::ChatClose => self.stop_chat_later(0),
            Cmd::LoadCommands => self.spawn(
                |d, _| commands::load_custom(&d.config.join("commands")),
                |r| match r {
                    Ok((customs, left)) => Bg::Commands(customs, left),
                    Err(e) => Bg::Done(Err(e)),
                },
            ),
            Cmd::Choices(run) => {
                let session = self.current_chat();
                self.spawn(
                    move |d, _| choices(d, session.as_deref(), run),
                    move |r| Bg::Choices(run, r.and_then(|x| x)),
                );
            }
            Cmd::SetModel(name) => {
                let session = self.current_chat();
                self.spawn(
                    move |d, _| set_model(d, session.as_deref(), &name),
                    |r| Bg::Done(r.and_then(|x| x)),
                );
            }
            Cmd::SetConnector(name) => self.spawn(
                move |d, _| set_connector(d, &name),
                |r| Bg::Done(r.and_then(|x| x)),
            ),
            Cmd::SetEffort(level) => {
                let session = self.current_chat();
                self.spawn(
                    move |d, _| set_effort(d, session.as_deref(), &level),
                    |r| Bg::Done(r.and_then(|x| x)),
                );
            }
            Cmd::SessionName(name) => {
                let session = self.current_chat();
                self.spawn(
                    move |_, _| {
                        let s = session.ok_or("no hay una sesión abierta")?;
                        s.command("set_session_name", serde_json::json!({ "name": name }))?;
                        Ok::<_, String>(name)
                    },
                    |r| Bg::Named(r.and_then(|x| x)),
                );
            }
            Cmd::Compact(instructions) => {
                let session = self.current_chat();
                self.spawn(
                    move |_, _| {
                        let Some(s) = session else {
                            return (Err("no hay una sesión abierta".to_owned()), None);
                        };
                        let fields = if instructions.trim().is_empty() {
                            serde_json::json!({})
                        } else {
                            serde_json::json!({ "customInstructions": instructions })
                        };
                        let done = s.command("compact", fields).map(|v| {
                            let n = |k: &str| v[k].as_u64().map(usage::tokens);
                            match (n("tokensBefore"), n("estimatedTokensAfter")) {
                                (Some(a), Some(b)) => {
                                    format!("contexto compactado: de {a} a unos {b} tokens")
                                }
                                _ => "contexto compactado".to_owned(),
                            }
                        });
                        (done, s.refresh().ok())
                    },
                    |r| match r {
                        Ok((done, refresh)) => Bg::Compacted(done, refresh),
                        Err(e) => Bg::Compacted(Err(e), None),
                    },
                );
            }
            Cmd::LoadPending { session } => self.spawn(
                move |d, _| {
                    let now = crate::cli::now_ms();
                    let all = super::pending::waiting(d);
                    let others = all.iter().filter(|p| p.session != session).count();
                    let mine = all
                        .into_iter()
                        .filter(|p| !session.is_empty() && p.session == session)
                        .map(|p| {
                            let what = format!(
                                "{} {}",
                                super::pending::label(&p),
                                super::pending::what(&p)
                            );
                            (p.id.clone(), super::pending::line(&p, now), what)
                        })
                        .collect();
                    (mine, others)
                },
                Bg::Pending,
            ),
            Cmd::LoadActions { run } => self.spawn(
                move |d, _| {
                    let log = super::pending::action_log(d, Some(&run))?;
                    Ok::<_, String>(actions_box(&log, super::pending::local_offset()))
                },
                |r| Bg::Modal(r.and_then(|x| x)),
            ),
            Cmd::UndoSession { id } => self.spawn(
                move |d, _| apply::undo_lines(d, &id, false).map(|(_, lines)| lines),
                |r| {
                    Bg::Modal(
                        r.and_then(|x| x)
                            .map(|l| commands::Modal::text("Deshacer", l)),
                    )
                },
            ),
            Cmd::Debug(on) => self.debug.set(on),
            Cmd::Cancel => {
                if let Ok(flag) = self.cancel.lock() {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            Cmd::Start { dir, instruction } => {
                let flag = Arc::new(AtomicBool::new(false));
                if let Ok(mut slot) = self.cancel.lock() {
                    *slot = flag.clone();
                }
                let dbg = self.debug.clone();
                self.spawn(
                    move |d, tx| {
                        let opts = RunOpts {
                            workspace: expand(&dir, &d.home),
                            max_steps: None,
                            allow_broad: false,
                            instruction: instruction.clone(),
                            adapter_key: true,
                        };
                        let p = runs::prepare(d, &opts)?;
                        let _ = tx.send(Input::Bg(Bg::Started {
                            id: p.id.clone(),
                            files: p.scan.files.len(),
                            workspace: p.ws.display().to_string(),
                            usage: RunUsage::new(
                                p.window,
                                p.window_known,
                                RunLimits {
                                    max_steps: p.max_steps,
                                    max_seconds: p.max_seconds,
                                    max_cost_micro: p.max_cost_micro,
                                },
                            ),
                        }));
                        runs::execute(
                            &p,
                            &instruction,
                            Box::new(Forward(tx.clone())),
                            Some(flag),
                            &dbg,
                        )
                    },
                    |r| Bg::Finished(r.and_then(|x| x).map(Box::new)),
                );
            }
            Cmd::LoadDetail { id, diff } => self.spawn(
                move |d, _| Detail::load(d, &id),
                move |r| Bg::Detail {
                    detail: r.and_then(|x| x).map(Box::new),
                    diff,
                },
            ),
            Cmd::PlanApply { id } => self.spawn(
                move |d, _| {
                    // The same checks as `clio apply`; nothing is written here.
                    apply::plan(d, &id, false).map(|p| Planned {
                        id: p.run.id.clone(),
                        files: p.changes.len(),
                        workspace: p.ws.display().to_string(),
                        names: p
                            .changes
                            .iter()
                            .map(|c| {
                                let mark = match c.kind {
                                    work::Kind::Modified => 'M',
                                    work::Kind::Created => '+',
                                    work::Kind::Deleted => '-',
                                };
                                format!("{mark} {}", c.rel)
                            })
                            .collect(),
                        tainted: runs::run_taint(&p.run.path),
                    })
                },
                |r| Bg::Planned(r.and_then(|x| x)),
            ),
            Cmd::Apply { id } => self.spawn(
                move |d, _| {
                    // Checked again right before writing: the original may have changed meanwhile.
                    let p = apply::plan(d, &id, false)?;
                    let n = apply::commit(&p)?;
                    Ok::<_, String>(format!("{n} archivo(s) aplicados. Para deshacer: u"))
                },
                |r| Bg::Done(r.and_then(|x| x)),
            ),
            Cmd::Undo { id } => self.spawn(
                move |d, _| {
                    let n = apply::undo_run(d, &id, false)?;
                    Ok::<_, String>(format!(
                        "deshecho: {} archivo(s) vuelven a como estaban",
                        n.values().sum::<usize>()
                    ))
                },
                |r| Bg::Done(r.and_then(|x| x)),
            ),
        }
    }
}
