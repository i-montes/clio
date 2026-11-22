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
    /// A checklist: `checked` ticked at first, one per option; each the person's to take or leave.
    Checks {
        title: String,
        lines: Vec<String>,
        options: Vec<String>,
        checked: Vec<bool>,
    },
}

pub enum Answer {
    Choice(usize),
    Secret(SecretValue),
    Text(String),
    /// What was ticked, one per option.
    Checks(Vec<bool>),
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
    /// install.sh just installed clio and clio-vault: the binaries step looks for nothing.
    pub fresh: bool,
    /// Not finding the release is a failure (`clio update`), not a warning.
    pub release_required: bool,
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
            exe,
            bin,
            ..Self::default()
        })
    }
}

/// In a debug build, `CLIO_TEST_SYSTEM`: a folder of fake system programs (`sudo`, a package
/// manager, `systemctl`, `loginctl`, `bwrap`, `tee`, `apparmor_parser`) used in place of the real
/// ones; its `linger` folder in place of systemd's, its `apparmor.d` in place of
/// `/etc/apparmor.d`, its `userns_restrict` file in place of the sysctl, and its `bwrap` run to
/// tell whether the jail starts. Release builds never read it.
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
    /// `kernel.apparmor_restrict_unprivileged_userns`, and the folder of AppArmor's profiles.
    pub userns: PathBuf,
    pub apparmor: PathBuf,
    pub root: bool,
    pub user: Option<String>,
    pub linux: bool,
    /// `/etc/os-release`: which names the system's packages have.
    pub os_release: PathBuf,
    /// The fake system of the tests: whether the jail starts is its `bwrap`'s answer.
    pub fake: bool,
}

/// The AppArmor profile that lets `/usr/bin/bwrap` make its user namespace where the kernel
/// restricts them (Ubuntu 23.10 and newer). Fixed text: nothing of this machine goes in it.
pub const BWRAP_PROFILE: &str = "abi <abi/4.0>,\ninclude <tunables/global>\n\nprofile clio-bwrap /usr/bin/bwrap flags=(unconfined) {\n  userns,\n}\n";

impl Sys {
    pub fn from_env() -> Self {
        let user = super::services::user_name();
        let linux = cfg!(target_os = "linux");
        if let Some(d) = test_system() {
            return Self {
                linger: d.join("linger"),
                userns: d.join("userns_restrict"),
                apparmor: d.join("apparmor.d"),
                os_release: d.join("os-release"),
                dirs: vec![d],
                root: false,
                user,
                linux,
                fake: true,
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
            userns: "/proc/sys/kernel/apparmor_restrict_unprivileged_userns".into(),
            apparmor: "/etc/apparmor.d".into(),
            os_release: "/etc/os-release".into(),
            root: rustix::process::getuid().is_root(),
            user,
            linux,
            fake: false,
        }
    }

    /// Whether Pi's jail starts here (switched off on purpose counts: the person chose it).
    pub fn jail_ready(&self) -> bool {
        use clio_brain::pi::jail::{Status, status};
        if self.fake {
            return self.find("bwrap").is_some_and(|b| {
                std::process::Command::new(b)
                    .args(["--ro-bind", "/", "/", "true"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
            });
        }
        matches!(status(), Status::Ready(_) | Status::Off)
    }

    /// The kernel keeps unprivileged programs from making user namespaces.
    pub fn userns_restricted(&self) -> bool {
        std::fs::read_to_string(&self.userns).is_ok_and(|v| v.trim() == "1")
    }

    /// A profile already names `/usr/bin/bwrap`: a second one for the same path breaks it.
    pub fn bwrap_profiled(&self) -> bool {
        std::fs::read_dir(&self.apparmor)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("/usr/bin/bwrap")))
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

    /// The package manager, and the commands that install `pkgs` with it.
    pub fn package_manager(&self, pkgs: &[String]) -> Option<(String, Vec<Vec<String>>)> {
        let cmd = |p: &Path, args: &[&str], pkgs: &[String]| {
            std::iter::once(p.display().to_string())
                .chain(args.iter().map(|a| (*a).to_owned()))
                .chain(pkgs.iter().cloned())
                .collect::<Vec<_>>()
        };
        if let Some(p) = self.find("apt-get") {
            // Right after boot unattended-upgrades holds the lock: wait for it, up to 5 minutes.
            let lock = ["-o", "DPkg::Lock::Timeout=300"];
            return Some((
                "apt".into(),
                vec![
                    cmd(&p, &[&lock[..], &["update", "-qq"]].concat(), &[]),
                    cmd(&p, &[&lock[..], &["install", "-y", "-qq"]].concat(), pkgs),
                ],
            ));
        }
        for (name, args) in [
            ("dnf", &["install", "-y"][..]),
            ("pacman", &["-S", "--noconfirm", "--needed"][..]),
            ("zypper", &["--non-interactive", "install"][..]),
            ("apk", &["add"][..]),
        ] {
            if let Some(p) = self.find(name) {
                return Some((name.into(), vec![cmd(&p, args, pkgs)]));
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

/// The steps as lines on standard output, each when it starts and when it ends (on a terminal the
/// first line, with a download's progress, is replaced by the second); questions only for a
/// secret, at the terminal.
pub struct Plain {
    pub lang: Lang,
    pub rows: Vec<String>,
    /// The line a terminal shows while a step works, which its end replaces.
    shown: String,
    /// The row whose start was said and whose end was not yet.
    started: Option<usize>,
}

impl Plain {
    pub fn new(lang: Lang, rows: Vec<String>) -> Self {
        Self {
            lang,
            rows,
            shown: String::new(),
            started: None,
        }
    }
}

impl Ui for Plain {
    fn set(&mut self, i: usize, st: St) {
        use super::ui::{Level, Style};
        use std::io::Write;
        let s = Style::stdout();
        let label = self.rows.get(i).cloned().unwrap_or_default();
        let working = |p: &str| {
            text::say(&super::tui::sanitize::line(&format!(
                "{}  {label}{p}",
                s.ellipsis()
            )))
        };
        let line = match &st {
            St::Waiting => return,
            St::Working(p) if s.tty => {
                let now = working(&p.map_or(String::new(), |(got, total)| {
                    if total > 0 {
                        format!(" · {}/{} MB", screen::mb(got), screen::mb(total))
                    } else {
                        format!(" · {} MB", screen::mb(got))
                    }
                }));
                // One row: a wrapped line would not be replaced.
                let now = super::ui::clip(s, &now, super::ui::columns().saturating_sub(1));
                // Only when it changed: a download calls this for every piece it gets.
                if now != self.shown {
                    print!(
                        "\r{}\r{}",
                        " ".repeat(self.shown.chars().count()),
                        s.dim(&now)
                    );
                    let _ = std::io::stdout().flush();
                    self.shown = now;
                }
                return;
            }
            // Once per step (a download's end sets its row working again).
            St::Working(None) => {
                if self.started.replace(i) != Some(i) {
                    println!("{}", working(""));
                }
                return;
            }
            St::Working(Some(_)) => return,
            St::Done(n) => format!("{}  {label} · {n}", s.mark(Level::Pass)),
            St::Skip(n) => format!("{}  {label} · {n}", s.dim("·")),
            St::Warn(n) => format!("{}  {label} · {n}", s.mark(Level::Warn)),
            St::Failed(why, fix) => format!(
                "{}  {label} · {why}\n   {} {fix}",
                s.mark(Level::Fail),
                s.arrow()
            ),
        };
        self.started = None;
        if !self.shown.is_empty() {
            print!(
                "\r{}\r",
                " ".repeat(std::mem::take(&mut self.shown).chars().count())
            );
        }
        println!("{}", text::say(&super::tui::sanitize::multiline(&line)));
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
                    println!("{}", text::say(M::ErrMismatch.get(self.lang)));
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
    // Hidden: the new binary an update started goes on where the old one stopped.
    let resume = a.flag("--resume-update");
    opts.reexec = resume;
    // Hidden: install.sh just put the binaries in place.
    opts.fresh = a.flag("--fresh");
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
    // A secret in a file anyone on the machine can read is already out: refused, not used.
    for f in [&opts.passphrase_file, &opts.key_file, &opts.temis_key_file]
        .into_iter()
        .flatten()
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(f).map_err(|e| super::io_err(f, &e))?;
        if m.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "{} can be read by other users: chmod 600 it first",
                f.display()
            ));
        }
    }
    let terminal = attach_terminal(!opts.yes);
    if !opts.yes && !terminal {
        println!(
            "{}\n{}",
            text::say(M::NoTerminal.get(Lang::Es)),
            text::say(M::NoTerminal.get(Lang::En))
        );
        return Ok(0);
    }
    let (sys, pins) = (Sys::from_env(), Pins::from_env());
    if opts.yes {
        let lang = lang.unwrap_or_else(|| Lang::saved_or_env(dirs));
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
        println!("{}", text::say(M::UpdateChatOpen.get(lang)));
        return Ok(1);
    }
    opts.update = action == Action::Update;
    let rows = action
        .shown()
        .iter()
        .map(|s| s.label().get(lang).to_owned())
        .collect();
    let mut ui = Plain::new(lang, rows);
    let (failed, replaced, left, blocked) = {
        let mut r = run_with(dirs, lang, sys, pins, opts.clone(), &mut ui);
        let steps = action.steps();
        r.ask_all(&steps);
        r.all(&steps);
        (r.failed, r.replaced, r.left, r.blocked)
    };
    if replaced {
        return reexec(
            &opts.exe,
            &["setup", "--yes", "--lang", lang.code(), "--resume-update"],
        );
    }
    // Not when the jail cannot start: the next run offers Repair, which tries it again.
    if !failed && blocked.is_none() && action != Action::RemoveAll {
        mark_version(dirs);
    }
    // The one thing left, in one line: what keeps the chat from starting first.
    if let Some(m) = blocked.or(left) {
        println!("{}", text::say(&M::DoneLeft.fill(lang, &[m.get(lang)])));
    }
    Ok(i32::from(failed || blocked.is_some()))
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
        blocked: None,
        answers: steps::Answers::default(),
        failed: false,
        changes: Vec::new(),
        replaced: false,
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
    let e = std::process::Command::new(exe).args(args).exec();
    Err(format!("{}: {e}", exe.display()))
}
