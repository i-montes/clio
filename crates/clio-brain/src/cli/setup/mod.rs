//! `clio setup`: the guided installer (`docs/specs/2026-11-19-installer-design.md`). Screens in
//! order: language, what it found, what it will do, doing it, done, and then the chat. The same
//! steps run without screens with `--yes` (tests and scripts) and from `clio update`.
//!
//! Secrets (the vault's passphrase, the model's key, Temis's key) come only from a masked field,
//! a terminal prompt or a file a flag names: never from an argument or the environment, and they
//! go from this process to the vault and nowhere else. `sudo` asks for its own password on the
//! terminal; the installer never reads it.

pub mod fetch;
mod screen;
pub mod steps;
#[cfg(test)]
mod tests;
pub mod text;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clio_vault::SecretValue;

use self::fetch::Pins;
use self::steps::{Action, Run};
use self::text::{Lang, M};
use super::home::Dirs;
use super::{Args, Res};

/// A row's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum St {
    Waiting,
    /// Working; with a download, its bytes so far and the total (0: unknown).
    Working(Option<(u64, u64)>),
    Done(String),
    /// Done, but the person should know something.
    Warn(String),
    /// Left out (by choice, or nothing to do on this system).
    Skip(String),
    /// One plain sentence, and what to do.
    Failed(String, String),
}

/// A question for the person.
#[derive(Debug, Clone)]
pub enum Ask {
    Choice {
        title: String,
        lines: Vec<String>,
        options: Vec<String>,
        default: usize,
    },
    /// Masked; two labels: typed twice, and both must match.
    Secret {
        title: String,
        lines: Vec<String>,
        labels: Vec<String>,
        error: Option<String>,
    },
    Text {
        title: String,
        lines: Vec<String>,
        label: String,
        value: String,
        error: Option<String>,
    },
}

pub enum Answer {
    Choice(usize),
    Secret(SecretValue),
    Text(String),
}

/// What the steps show and ask through: the screens, or plain lines.
pub trait Ui {
    /// Row `i` is now `st`.
    fn set(&mut self, i: usize, st: St);
    /// `None`: the person left the question, or there is nobody to ask.
    fn ask(&mut self, q: Ask) -> Option<Answer>;
    /// Runs `f` with the terminal given back (`sudo`, Claude Code's login).
    fn outside(&mut self, f: &mut dyn FnMut());
    /// Runs `f` while the screen keeps moving.
    fn busy(&mut self, f: &mut (dyn FnMut() + Send));
}

/// What the flags said, and where Clio is.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub yes: bool,
    pub no_model: bool,
    pub no_vault: bool,
    /// Run the doctor's checks at the end (the tests turn it off: it runs the machine's tools).
    pub doctor: bool,
    /// An update: the timer is restarted, not enabled.
    pub update: bool,
    /// This process is the new binary an update started: it replaces nothing again.
    pub reexec: bool,
    /// The running `clio`, and the folder it and `clio-vault` live in.
    pub exe: PathBuf,
    pub bin: PathBuf,
    pub passphrase_file: Option<PathBuf>,
    pub connector: Option<String>,
    pub key_file: Option<PathBuf>,
    pub temis_url: Option<String>,
    pub temis_key_file: Option<PathBuf>,
}

impl Opts {
    pub fn here() -> Res<Self> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let bin = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok(Self {
            doctor: true,
            reexec: std::env::var_os(REEXEC).is_some(),
            exe,
            bin,
            ..Self::default()
        })
    }
}

/// Set by an update in the new binary it starts.
const REEXEC: &str = "CLIO_UPDATE_REEXEC";

/// In a debug build, `CLIO_TEST_SYSTEM`: a folder of fake system programs (`sudo`, a package
/// manager, `systemctl`, `loginctl`, `bwrap`) used in place of the real ones, and its `linger`
/// folder in place of systemd's. Release builds never read it.
pub fn test_system() -> Option<PathBuf> {
    std::env::var_os("CLIO_TEST_SYSTEM")
        .filter(|_| cfg!(debug_assertions))
        .map(PathBuf::from)
}

/// The system programs the installer may run, where they are.
#[derive(Debug, Clone)]
pub struct Sys {
    pub dirs: Vec<PathBuf>,
    pub linger: PathBuf,
    pub root: bool,
    pub user: Option<String>,
    pub linux: bool,
}

impl Sys {
    pub fn from_env() -> Self {
        let user = super::services::user_name();
        let linux = cfg!(target_os = "linux");
        if let Some(d) = test_system() {
            return Self {
                linger: d.join("linger"),
                dirs: vec![d],
                root: false,
                user,
                linux,
            };
        }
        Self {
            dirs: [
                "/usr/local/sbin",
                "/usr/local/bin",
                "/usr/sbin",
                "/usr/bin",
                "/sbin",
                "/bin",
            ]
            .map(PathBuf::from)
            .to_vec(),
            linger: "/var/lib/systemd/linger".into(),
            root: rustix::process::getuid().is_root(),
            user,
            linux,
        }
    }

    pub fn find(&self, name: &str) -> Option<PathBuf> {
        self.dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())
    }

    /// systemd is there and lingering is off for this user (the timer stops at logout).
    pub fn linger_off(&self) -> bool {
        self.find("systemctl").is_some()
            && self.find("loginctl").is_some()
            && self
                .user
                .as_ref()
                .is_some_and(|u| !self.linger.join(u).exists())
    }

    /// The package manager, and the commands that install bubblewrap with it.
    pub fn package_manager(&self) -> Option<(String, Vec<Vec<String>>)> {
        let cmd = |p: &Path, args: &[&str]| {
            std::iter::once(p.display().to_string())
                .chain(args.iter().map(|a| (*a).to_owned()))
                .collect::<Vec<_>>()
        };
        if let Some(p) = self.find("apt-get") {
            return Some((
                "apt".into(),
                vec![
                    cmd(&p, &["update", "-qq"]),
                    cmd(&p, &["install", "-y", "-qq", "bubblewrap"]),
                ],
            ));
        }
        for (name, args) in [
            ("dnf", &["install", "-y", "bubblewrap"][..]),
            (
                "pacman",
                &["-S", "--noconfirm", "--needed", "bubblewrap"][..],
            ),
            (
                "zypper",
                &["--non-interactive", "install", "bubblewrap"][..],
            ),
            ("apk", &["add", "bubblewrap"][..]),
        ] {
            if let Some(p) = self.find(name) {
                return Some((name.into(), vec![cmd(&p, args)]));
            }
        }
        None
    }
}

/// What is on this machine of Clio.
#[derive(Debug, Clone, Default)]
pub struct Found {
    pub installed: bool,
    pub version: Option<String>,
    pub vault: bool,
    pub timer: bool,
    pub bwrap: bool,
}

/// The version the installer last left, in `<data>/version`.
fn version_file(dirs: &Dirs) -> PathBuf {
    dirs.data.join("version")
}

impl Found {
    pub fn detect(dirs: &Dirs, sys: &Sys) -> Self {
        Self {
            installed: dirs.config_file().exists() || dirs.data.is_dir(),
            version: std::fs::read_to_string(version_file(dirs))
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| fetch::version(v).is_some()),
            vault: dirs.data.join("vault.bin").exists(),
            timer: super::services::UNITS
                .iter()
                .all(|u| dirs.home.join(".config/systemd/user").join(u).is_file()),
            bwrap: sys.find("bwrap").is_some(),
        }
    }
}

// ---- plain lines (`--yes`, `clio update`) ------------------------------------------------------

/// The steps as lines on standard output; questions only for a secret, at the terminal.
pub struct Plain {
    pub lang: Lang,
    pub rows: Vec<String>,
    /// Only failures and warnings are printed (`clio update` prints what changed itself).
    pub quiet: bool,
}

impl Ui for Plain {
    fn set(&mut self, i: usize, st: St) {
        use super::ui::{Level, Style};
        let s = Style::stdout();
        let label = self.rows.get(i).cloned().unwrap_or_default();
        let line = match &st {
            St::Waiting | St::Working(_) => return,
            St::Done(n) if !self.quiet => format!("{}  {label} · {n}", s.mark(Level::Pass)),
            St::Skip(n) if !self.quiet => format!("{}  {label} · {n}", s.dim("·")),
            St::Done(_) | St::Skip(_) => return,
            St::Warn(n) => format!("{}  {label} · {n}", s.mark(Level::Warn)),
            St::Failed(why, fix) => format!(
                "{}  {label} · {why}\n   {} {fix}",
                s.mark(Level::Fail),
                s.arrow()
            ),
        };
        println!("{}", super::tui::sanitize::multiline(&line));
    }

    fn ask(&mut self, q: Ask) -> Option<Answer> {
        let Ask::Secret { labels, .. } = q else {
            return None;
        };
        if !std::io::stdin().is_terminal() {
            return None;
        }
        let mut first = None;
        for label in labels {
            let v = SecretValue::new(super::vault::read_secret(&label).ok()?);
            match &first {
                None => first = Some(v),
                Some(f) if f.expose() == v.expose() => {}
                Some(_) => {
                    println!("{}", M::ErrMismatch.get(self.lang));
                    return None;
                }
            }
        }
        first.map(Answer::Secret)
    }

    fn outside(&mut self, f: &mut dyn FnMut()) {
        f();
    }

    fn busy(&mut self, f: &mut (dyn FnMut() + Send)) {
        f();
    }
}

// ---- the command -------------------------------------------------------------------------------

/// When standard input is not a terminal (`curl … | sh`), the terminal is `/dev/tty`: it becomes
/// standard input (and output, for the screens, when that is not a terminal either). Whether
/// there is a terminal at all.
fn attach_terminal(screens: bool) -> bool {
    let (i, o) = (
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    );
    if i && (o || !screens) {
        return true;
    }
    let Ok(tty) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
    else {
        return false;
    };
    let ok_in = i || rustix::stdio::dup2_stdin(&tty).is_ok();
    let ok_out = o || !screens || rustix::stdio::dup2_stdout(&tty).is_ok();
    ok_in && ok_out
}

pub const USAGE: &str = "usage: clio setup [--yes --lang es|en] [--no-model] [--no-vault] [--passphrase-file F] [--connector claude|anthropic-api|opencode --key-file F] [--temis-url URL --temis-key-file F]";

pub fn run(dirs: &Dirs, argv: Vec<String>) -> Res<i32> {
    let mut a = Args::new(argv);
    let mut opts = Opts::here()?;
    opts.yes = a.flag("--yes");
    opts.no_model = a.flag("--no-model");
    opts.no_vault = a.flag("--no-vault");
    let resume = a.flag("--resume-update");
    let lang = a
        .value("--lang")?
        .map(|l| Lang::from_code(&l).ok_or("--lang is es or en"))
        .transpose()?;
    let file = |a: &mut Args, f: &str| a.value(f).map(|v| v.map(PathBuf::from));
    opts.passphrase_file = file(&mut a, "--passphrase-file")?;
    opts.key_file = file(&mut a, "--key-file")?;
    opts.temis_key_file = file(&mut a, "--temis-key-file")?;
    opts.connector = a.value("--connector")?;
    opts.temis_url = a.value("--temis-url")?;
    if !a.rest()?.is_empty() {
        return Err(USAGE.into());
    }
    if let Some(c) = &opts.connector
        && clio_brain::connector::by_name(c).is_none()
    {
        return Err(USAGE.into());
    }
    let terminal = attach_terminal(!opts.yes);
    if !opts.yes && !terminal {
        println!(
            "{}\n{}",
            M::NoTerminal.get(Lang::Es),
            M::NoTerminal.get(Lang::En)
        );
        return Ok(0);
    }
    let (sys, pins) = (Sys::from_env(), Pins::from_env());
    if opts.yes {
        let lang = lang.unwrap_or_else(Lang::from_env);
        let found = Found::detect(dirs, &sys);
        let action = if resume || found.installed {
            Action::Update
        } else {
            Action::Install
        };
        return plain(dirs, lang, sys, pins, opts, action);
    }
    screen::wizard(dirs, lang, sys, pins, opts, resume)
}

/// The steps of `action` without screens. Exit 0 when nothing failed.
fn plain(
    dirs: &Dirs,
    lang: Lang,
    sys: Sys,
    pins: Pins,
    mut opts: Opts,
    action: Action,
) -> Res<i32> {
    if action != Action::Install && super::pending::any_open(dirs) {
        println!("{}", M::UpdateChatOpen.get(lang));
        return Ok(1);
    }
    opts.update = action == Action::Update;
    let rows = action
        .shown()
        .iter()
        .map(|s| s.label().get(lang).to_owned())
        .collect();
    let mut ui = Plain {
        lang,
        rows,
        quiet: false,
    };
    let (failed, replaced, left) = {
        let mut r = run_with(dirs, lang, sys, pins, opts.clone(), &mut ui);
        r.all(&action.steps());
        (r.failed, r.replaced, r.left)
    };
    if replaced {
        return reexec(
            &opts.exe,
            &["setup", "--yes", "--lang", lang.code(), "--resume-update"],
        );
    }
    if !failed && action != Action::RemoveAll {
        mark_version(dirs);
    }
    if let Some(m) = left {
        println!("{}", M::DoneLeft.fill(lang, &[m.get(lang)]));
    }
    Ok(i32::from(failed))
}

pub(crate) fn run_with<'a>(
    dirs: &Dirs,
    lang: Lang,
    sys: Sys,
    pins: Pins,
    opts: Opts,
    ui: &'a mut dyn Ui,
) -> Run<'a> {
    Run {
        dirs: dirs.clone(),
        lang,
        sys,
        pins,
        opts,
        ui,
        left: None,
        failed: false,
        changes: Vec::new(),
        replaced: false,
        doctor: None,
    }
}

/// Records the version this binary is, for the next `clio setup` to name.
pub(crate) fn mark_version(dirs: &Dirs) {
    if dirs.data.is_dir() {
        let _ = super::write_private(&version_file(dirs), env!("CARGO_PKG_VERSION").as_bytes());
    }
}

/// Runs the new binary an update put in place, to finish with what it pins (Node, Pi,
/// clio-memory). `exe` is the path the old one was started as: the same file name, now the new
/// binary. Only returns on failure.
pub(crate) fn reexec(exe: &Path, args: &[&str]) -> Res<i32> {
    use std::os::unix::process::CommandExt;
    let e = std::process::Command::new(exe)
        .args(args)
        .env(REEXEC, "1")
        .exec();
    Err(format!("{}: {e}", exe.display()))
}
