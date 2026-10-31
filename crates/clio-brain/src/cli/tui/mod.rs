//! `clio` with no arguments in a terminal: the TUI.
//!
//! `app` holds the state and the pure `update`; the screens (`home`, `new_run`, `live`, `detail`,
//! `doctor`) hold their state, keys and drawing; `sanitize` cleans every outside text before it is
//! drawn; `theme` has the colors. This file is the only one that touches the terminal and starts
//! threads: it turns `Cmd`s into work on worker threads and their answers into `Input::Bg`.

pub mod app;
pub mod detail;
pub mod doctor;
pub mod home;
pub mod live;
pub mod new_run;
pub mod sanitize;
pub mod theme;

#[cfg(test)]
mod tests;

use std::io::{IsTerminal, Stdout};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clio_brain::agent::{Event, Observer};
use crossterm::event::{self, Event as Term, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use self::app::{App, Bg, Cmd, Input, Planned, draw, update};
use self::detail::Detail;
use self::home::{Header, KeyState};
use self::new_run::Preview;
use super::apply;
use super::home::{Config, Dirs, KeyStatus, key_status};
use super::runs::{self, RunOpts};
use super::{Res, work};

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

/// Restores the terminal when dropped, on every way out of `run`.
struct Guard;

impl Guard {
    fn enter() -> Res<(Self, Terminal<CrosstermBackend<Stdout>>)> {
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
pub fn run(dirs: Dirs) -> Res<i32> {
    install_panic_hook();
    let (_guard, mut term) = Guard::enter()?;
    let (tx, rx) = channel::<Input>();
    let ctx = Ctx {
        dirs: Arc::new(dirs),
        tx,
        cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
    };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut app = App::new(&cwd, theme::Theme::from_env());
    for c in App::start_cmds() {
        ctx.exec(c);
    }
    let t0 = Instant::now();
    // Only for the terminal-restoration test: a deliberate panic inside the alternate screen.
    if std::env::var_os("CLIO_TUI_PANIC_TEST").is_some() {
        panic!("deliberate panic to test that the terminal is restored");
    }
    let to_ms = |t: Instant| u64::try_from(t.duration_since(t0).as_millis()).unwrap_or(u64::MAX);
    while !app.quit {
        term.draw(|f| draw(f, &app))
            .map_err(|e| format!("terminal: {e}"))?;
        let mut inputs: Vec<Input> = Vec::new();
        if event::poll(Duration::from_millis(100)).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Term::Key(k) if k.kind != KeyEventKind::Release => inputs.push(Input::Key(k)),
                Term::Paste(s) => inputs.push(Input::Paste(s)),
                _ => {}
            }
        }
        inputs.push(Input::Tick(to_ms(Instant::now())));
        while let Ok(i) = rx.try_recv() {
            inputs.push(i);
        }
        for i in inputs {
            let (next, cmds) = update(app, i);
            app = next;
            for c in cmds {
                ctx.exec(c);
            }
        }
    }
    Ok(0)
}

struct Ctx {
    dirs: Arc<Dirs>,
    tx: Sender<Input>,
    cancel: Mutex<Arc<AtomicBool>>,
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

fn header(dirs: &Dirs) -> Header {
    let (provider, model, config_error) = match Config::load(dirs) {
        Ok(c) => (c.model.provider, c.model.model, None),
        Err(e) => (String::new(), String::new(), Some(e)),
    };
    let key = match key_status(dirs) {
        KeyStatus::Env | KeyStatus::File { open_mode: None } => KeyState::Present,
        KeyStatus::File { open_mode: Some(_) } => KeyState::OpenPermissions,
        KeyStatus::Missing => KeyState::Missing,
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
        provider,
        model,
        key,
        vault: vault.into(),
        config_error,
    }
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

impl Ctx {
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
                |d, _| (header(d), runs::list_info(d, Some(100)).unwrap_or_default()),
                |r| match r {
                    Ok(v) => Bg::Home(Box::new(v)),
                    Err(e) => Bg::Done(Err(e)),
                },
            ),
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
                self.spawn(
                    move |d, tx| {
                        let opts = RunOpts {
                            workspace: expand(&dir, &d.home),
                            max_steps: None,
                            allow_broad: false,
                            instruction: instruction.clone(),
                        };
                        let p = runs::prepare(d, &opts)?;
                        let _ = tx.send(Input::Bg(Bg::Started {
                            id: p.id.clone(),
                            files: p.scan.files.len(),
                            max_steps: p.max_steps,
                            max_seconds: p.max_seconds,
                        }));
                        runs::execute(&p, &instruction, Box::new(Forward(tx.clone())), Some(flag))
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
