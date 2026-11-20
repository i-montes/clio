//! The installer's steps. Each one is idempotent (what is already right is said and left alone),
//! and `clio update` runs the same functions as the wizard's Update. Every question comes first
//! (`Run::ask_all`, the sudo moment last); then the steps run (`Run::all`) and ask nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clio_brain::connector::{Connector, by_name, by_provider};
use clio_vault::SecretValue;

use super::fetch::{self, FRes, Fail, Pins, Release};
use super::text::{Lang, M};
use super::{Answer, Ask, Opts, St, Sys, Ui};
use crate::cli::home::{Config, Dirs};
use crate::cli::tui::sanitize;
use crate::cli::{mkdir_private, vault};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Binaries,
    Folders,
    System,
    Node,
    Pi,
    Memory,
    Vault,
    Model,
    Judge,
    Timer,
    Skills,
    /// The MCP catalog: the next stage (the design's "Default MCP servers"). Empty, no row.
    Mcp,
    Doctor,
    StopVault,
    RemoveTimer,
    RemoveNodePi,
    RemoveBinaries,
    RemoveData,
    RemoveModel,
}

impl Step {
    pub fn label(self) -> M {
        match self {
            Self::Binaries => M::StBinaries,
            Self::Folders => M::StFolders,
            Self::System => M::StSystem,
            Self::Node => M::StNode,
            Self::Pi => M::StPi,
            Self::Memory => M::StMemory,
            Self::Vault => M::StVault,
            Self::Model => M::StModel,
            Self::Judge => M::StJudge,
            Self::Timer => M::StTimer,
            Self::Skills => M::StSkills,
            Self::Mcp => M::StSkills,
            Self::Doctor => M::StDoctor,
            Self::StopVault => M::StStopVault,
            Self::RemoveTimer => M::StRemoveTimer,
            Self::RemoveNodePi => M::StRemoveNodePi,
            Self::RemoveBinaries => M::StRemoveBinaries,
            Self::RemoveData => M::StRemoveData,
            Self::RemoveModel => M::StRemoveModel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Update,
    Repair,
    RemoveKeep,
    RemoveAll,
}

impl Action {
    /// The steps, in order (the hidden MCP step included).
    pub fn steps(self) -> Vec<Step> {
        use Step::*;
        match self {
            Self::Install => vec![
                Folders, System, Node, Pi, Memory, Vault, Model, Judge, Timer, Skills, Mcp, Doctor,
            ],
            // The vault and the model too: an install left half done (a skipped passphrase, a
            // failed run, a reboot that stopped the vault) is finished by running it again.
            Self::Update => vec![
                Binaries, Node, Pi, Memory, Vault, Model, Skills, Timer, Doctor,
            ],
            Self::Repair => vec![
                Folders, System, Node, Pi, Memory, Vault, Model, Timer, Skills, Mcp, Doctor,
            ],
            Self::RemoveKeep => vec![StopVault, RemoveTimer, RemoveNodePi, RemoveBinaries],
            Self::RemoveAll => vec![
                StopVault,
                RemoveTimer,
                RemoveNodePi,
                RemoveBinaries,
                RemoveData,
                RemoveModel,
            ],
        }
    }

    /// The steps the person sees.
    pub fn shown(self) -> Vec<Step> {
        self.steps()
            .into_iter()
            .filter(|s| *s != Step::Mcp)
            .collect()
    }
}

/// One step's ending, before it becomes a row.
enum Out {
    Done(String),
    Warn(String),
    Skip(String),
}

/// What the person answered before the work started, for the steps to use.
#[derive(Default)]
pub struct Answers {
    /// A new vault's passphrase.
    pass: Option<SecretValue>,
    /// Opening the existing vault failed.
    vault: Option<Fail>,
    /// The connector chosen, and its key (`None`: later).
    model: Option<(&'static Connector, Option<SecretValue>)>,
    /// Temis's address, and its key.
    temis: Option<(String, Option<SecretValue>)>,
    /// How the sudo moment ended: the system step's row.
    system: Option<Out>,
    /// The person chose to sign in to Claude when the install finishes (`Run::sign_in`).
    pub(super) sign_in: bool,
    /// The model step found everything ready for that sign-in.
    pub(super) sign_in_ready: Option<&'static Connector>,
}

/// A run of the installer's steps.
pub struct Run<'a> {
    pub dirs: Dirs,
    pub lang: Lang,
    pub sys: Sys,
    pub pins: Pins,
    pub opts: Opts,
    pub ui: &'a mut dyn Ui,
    /// The one thing left for the person, said on the last screen.
    pub left: Option<M>,
    /// The jail cannot start here: what to ask an administrator for. The chat does not open.
    pub blocked: Option<M>,
    pub answers: Answers,
    pub failed: bool,
    /// What changed, `name from → to`, for `clio update`.
    pub changes: Vec<String>,
    /// The binaries were replaced: the rest must run under the new ones.
    pub replaced: bool,
    /// The doctor's line.
    pub doctor: Option<String>,
}

impl Run<'_> {
    fn t(&self, m: M) -> String {
        m.get(self.lang).to_owned()
    }

    fn f(&self, m: M, args: &[&str]) -> String {
        m.fill(self.lang, args)
    }

    /// Notes `m` as the thing left, unless one that comes first is already noted.
    fn leave(&mut self, m: M) {
        let rank = |m: M| {
            [M::LeftVault, M::LeftModelAgain, M::LeftModel]
                .iter()
                .position(|x| *x == m)
                .unwrap_or(0)
        };
        if self.left.is_none_or(|l| rank(m) < rank(l)) {
            self.left = Some(m);
        }
    }

    /// Every question of `steps`, before any of them runs: the vault's passphrase, the model,
    /// the judge, and last the sudo moment (which runs its commands right then, while the person
    /// is at the terminal). An update that will replace this binary asks nothing: the new one
    /// asks, after it took over.
    pub fn ask_all(&mut self, steps: &[Step]) {
        if steps.contains(&Step::Binaries)
            && !self.opts.reexec
            && !self.opts.fresh
            && Release::find(&self.opts.bin).is_ok_and(|r| r.is_newer())
        {
            return;
        }
        if steps.contains(&Step::Vault) {
            self.ask_vault();
        }
        if steps.contains(&Step::Model) {
            self.ask_model();
        }
        if steps.contains(&Step::Judge) {
            self.ask_judge();
        }
        if steps.contains(&Step::System) {
            let out = self.sudo_moment();
            self.answers.system = Some(out);
        }
    }

    /// Runs `steps`, each row updated as it goes; a failed one does not stop the others.
    pub fn all(&mut self, steps: &[Step]) {
        let shown: Vec<Step> = steps.iter().copied().filter(|s| *s != Step::Mcp).collect();
        for step in steps {
            let Some(i) = shown.iter().position(|s| s == step) else {
                self.mcp();
                continue;
            };
            self.ui.set(i, St::Working(None));
            let st = match self.one(*step, i) {
                Ok(Out::Done(n)) => St::Done(n),
                Ok(Out::Warn(n)) => St::Warn(n),
                Ok(Out::Skip(n)) => St::Skip(n),
                Err(e) => {
                    self.failed = true;
                    let (why, fix) = self.said(&e);
                    St::Failed(why, fix)
                }
            };
            self.ui.set(i, st);
            if self.replaced {
                return;
            }
        }
    }

    fn said(&self, e: &Fail) -> (String, String) {
        said(self.lang, e)
    }

    fn one(&mut self, step: Step, i: usize) -> FRes<Out> {
        match step {
            Step::Binaries => self.binaries(i),
            Step::Folders => self.folders(),
            Step::System => Ok(self
                .answers
                .system
                .take()
                .unwrap_or_else(|| Out::Skip(self.t(M::NoteSkipped)))),
            Step::Node => self.node(i),
            Step::Pi => self.pi(),
            Step::Memory => self.memory(i),
            Step::Vault => self.vault(),
            Step::Model => self.model(),
            Step::Judge => self.judge(),
            Step::Timer => self.timer(),
            Step::Skills => crate::cli::skills::ensure(&self.dirs)
                .map(|()| Out::Done(self.t(M::NoteDone)))
                .map_err(Fail::Other),
            Step::Mcp => Ok(Out::Skip(String::new())),
            Step::Doctor => Ok(self.doctor()),
            Step::StopVault => {
                // Without a runtime folder no vault of this home can be running.
                if self.dirs.runtime.is_some() {
                    vault::stop_server(&self.dirs).map_err(Fail::Other)?;
                }
                Ok(Out::Done(self.t(M::NoteDone)))
            }
            Step::RemoveTimer => self.remove_timer(),
            Step::RemoveNodePi => self.remove_node_pi(),
            Step::RemoveBinaries => self.remove_binaries(),
            Step::RemoveData => self.remove_data(),
            Step::RemoveModel => {
                rm(&self.dirs.home.join(".cache/clio-models"))?;
                Ok(Out::Done(self.t(M::NoteRemoved)))
            }
        }
    }

    /// The Claude sign-in the person chose for the end: after the last step, the screen is
    /// handed to Claude Code's own `setup-token`, and the token pasted back goes to the vault.
    /// Whatever goes wrong leaves one thing: choosing the model again.
    pub fn sign_in(&mut self, steps: &[Step]) {
        let Some(c) = self.answers.sign_in_ready.take() else {
            return;
        };
        let row = steps
            .iter()
            .filter(|s| **s != Step::Mcp)
            .position(|s| *s == Step::Model);
        let mut token = None;
        if !self.failed && self.blocked.is_none() && !self.replaced {
            let dirs = self.dirs.clone();
            let before = format!("{}\n{}", self.t(M::ClaudeBefore), self.t(M::ClaudeLeave));
            let ask = self.t(M::ClaudeCopy);
            self.ui.outside(&mut || {
                token = crate::cli::connect::claude_sign_in(&dirs, &before, &ask);
            });
        }
        let stored = token.is_some_and(|k| {
            crate::cli::connect::store_key(&self.dirs, c, &k)
                .and_then(|()| crate::cli::connect::switch_config(&self.dirs, c, c.default_model))
                .is_ok()
        });
        let st = if stored {
            St::Done(c.name.into())
        } else {
            self.leave(M::LeftModelAgain);
            St::Warn(self.t(M::NoteLater))
        };
        if let Some(i) = row {
            self.ui.set(i, st);
        }
    }

    /// The MCP catalog's step: chosen servers installed with `clio mcp add <name>` (the next
    /// stage). Nothing yet.
    fn mcp(&mut self) {}

    fn progress(ui: &mut dyn Ui, i: usize, got: u64, total: u64) {
        ui.set(i, St::Working(Some((got, total))));
    }

    // ---- Clio's own binaries -----------------------------------------------------------------

    fn binaries(&mut self, i: usize) -> FRes<Out> {
        // install.sh just put them in place: nothing to look for.
        if self.opts.fresh {
            return Ok(Out::Done(self.t(M::NoteThere)));
        }
        let rel = match Release::find(&self.opts.bin) {
            Ok(r) => r,
            // Looking for a new version is `clio update`'s whole job; for the setup it is one
            // step of many.
            Err(e) if self.opts.release_required => return Err(e),
            Err(_) => return Ok(Out::Warn(self.t(M::NoteNoRelease))),
        };
        if !rel.is_newer() || self.opts.reexec {
            return Ok(Out::Done(self.t(M::NoteUpToDate)));
        }
        let ui = &mut *self.ui;
        let changed = fetch::install_binaries(&rel, &self.opts.bin, &mut |_, n| {
            Self::progress(ui, i, n, 0);
        })?;
        let from = env!("CARGO_PKG_VERSION");
        if changed {
            self.changes.push(format!("clio {from} → {}", rel.version));
            self.replaced = true;
        }
        Ok(Out::Done(self.f(M::NoteFromTo, &[from, &rel.version])))
    }

    // ---- the home ----------------------------------------------------------------------------

    fn folders(&mut self) -> FRes<Out> {
        let runtime = self
            .dirs
            .need_runtime()
            .map_err(|_| Fail::Said(M::FailRuntime, M::FixRuntime))?
            .to_path_buf();
        let mut created = false;
        for d in crate::cli::home::home_folders(&self.dirs)
            .into_iter()
            .chain([runtime])
        {
            created |= !d.is_dir();
            mkdir_private(&d)?;
        }
        if !clio_brain::memory_link::MemoryLink::db_path(&self.dirs.data).exists() {
            let model = clio_brain::memory_link::EmbedModel::none();
            clio_brain::memory_link::MemoryLink::open_write(&self.dirs.data, &model)?;
            created = true;
        }
        created |= crate::cli::home::write_default_config(&self.dirs)?;
        // The language the person chose, for Clio's first conversation; one chosen before stays.
        if Config::load(&self.dirs).is_ok_and(|c| c.ui.language.is_none()) {
            let code = self.lang.code();
            crate::cli::connect::edit_section(&self.dirs, "ui", true, |m| {
                m.insert("language".into(), code.into());
            })?;
            created = true;
        }
        Ok(Out::Done(self.t(if created {
            M::NoteCreated
        } else {
            M::NoteThere
        })))
    }

    // ---- the system: bubblewrap and lingering, with one sudo moment --------------------------

    /// Bubblewrap, lingering and, where the kernel needs it, bubblewrap's AppArmor profile: one
    /// moment with the exact commands shown, then the jail is tried. When it still cannot start,
    /// `blocked` says what to ask an administrator for.
    fn sudo_moment(&mut self) -> Out {
        if !self.sys.linux {
            return Out::Warn(self.t(M::NoteMac));
        }
        let bwrap = self.sys.find("bwrap").is_none();
        let linger = self.sys.linger_off();
        // Only where the kernel restricts user namespaces, no profile names bwrap yet, and the jail
        // does not start already: a second profile for the same path breaks bwrap.
        let profile = self.sys.userns_restricted()
            && !self.sys.bwrap_profiled()
            && (bwrap || !self.sys.jail_ready());
        let pm = self.sys.package_manager();
        let mut cmds: Vec<Vec<String>> = Vec::new();
        if bwrap && let Some((_, install)) = &pm {
            cmds.extend(install.iter().cloned());
        }
        if linger && let (Some(lc), Some(u)) = (self.sys.find("loginctl"), self.sys.user.clone()) {
            cmds.push(vec![lc.display().to_string(), "enable-linger".into(), u]);
        }
        let file = self.sys.apparmor.join("clio-bwrap").display().to_string();
        let profile_cmds = match (
            profile,
            self.sys.find("tee"),
            self.sys.find("apparmor_parser"),
        ) {
            (true, Some(tee), Some(parser)) => vec![
                vec![tee.display().to_string(), file.clone()],
                vec![parser.display().to_string(), "-r".into(), file],
            ],
            _ => Vec::new(),
        };
        let out = self.moment(bwrap, linger, pm.as_ref(), &cmds, &profile_cmds);
        if self.sys.jail_ready() {
            return out;
        }
        self.blocked = Some(if self.sys.find("bwrap").is_none() {
            M::LeftJailBwrap
        } else if self.sys.userns_restricted() {
            M::LeftJailUserns
        } else {
            M::LeftJailOther
        });
        match out {
            Out::Done(_) => Out::Warn(self.t(M::NoteNoJail)),
            o => o,
        }
    }

    fn moment(
        &mut self,
        bwrap: bool,
        linger: bool,
        pm: Option<&(String, Vec<Vec<String>>)>,
        cmds: &[Vec<String>],
        profile_cmds: &[Vec<String>],
    ) -> Out {
        if !bwrap && !linger && profile_cmds.is_empty() {
            return Out::Done(self.t(M::NoteThere));
        }
        if cmds.is_empty() && profile_cmds.is_empty() {
            return Out::Warn(self.t(M::NoteNoPkg));
        }
        let sudo = if self.sys.root {
            None
        } else {
            match self.sys.find("sudo") {
                Some(s) => Some(s),
                None if bwrap => return Out::Warn(self.t(M::NoteNoSudo)),
                None if linger => return Out::Warn(self.t(M::NoteNoSudoLinger)),
                None => return Out::Warn(self.t(M::NoteNoJail)),
            }
        };
        let shown = |c: &[String]| {
            format!(
                "{}{}",
                if sudo.is_some() { "sudo " } else { "" },
                c.join(" ")
            )
        };
        if !self.opts.yes {
            let mut lines = Vec::new();
            if bwrap {
                lines.push(self.f(M::SudoBwrap, &[pm.map_or("", |(n, _)| n.as_str())]));
            }
            if linger {
                lines.push(self.t(M::SudoLinger));
            }
            lines.push(self.t(M::SudoRuns));
            lines.extend(cmds.iter().map(|c| format!("  {}", shown(c))));
            if !profile_cmds.is_empty() {
                lines.push(self.t(M::SudoProfile));
                lines.extend(profile_cmds.iter().map(|c| format!("  {}", shown(c))));
            }
            lines.push(self.t(M::SudoHow));
            let q = Ask::Choice {
                title: self.t(M::SudoTitle),
                lines,
                options: vec![self.t(M::SudoGo), self.t(M::SudoSkip)],
                default: 0,
            };
            if !matches!(self.ui.ask(q), Some(Answer::Choice(0))) {
                return Out::Warn(self.t(M::NoteSysSkipped));
            }
        }
        // The terminal is the person's while these run: `sudo` asks there, once (its own
        // timestamp covers the next command), and Clio never reads the password. Each command is
        // printed before it runs.
        let mut failed = None;
        let sys = &self.sys;
        self.ui.outside(&mut || {
            for c in cmds {
                println!("$ {}", shown(c));
                // One broken repository fails `apt-get update`; the install may still work.
                if !as_root(sudo.as_deref(), c, None) && !c.iter().any(|a| a == "update") {
                    failed = Some(fetch::name_of(&c[0]));
                    break;
                }
            }
            if failed.is_none() && !profile_cmds.is_empty() && !sys.jail_ready() {
                for (i, c) in profile_cmds.iter().enumerate() {
                    println!("$ {}", shown(c));
                    let text = (i == 0).then_some(super::BWRAP_PROFILE);
                    if !as_root(sudo.as_deref(), c, text) {
                        failed = Some(fetch::name_of(&c[0]));
                        break;
                    }
                }
            }
        });
        match failed {
            Some(what) => Out::Warn(self.f(M::NoteSysFailed, &[&what])),
            None => Out::Done(self.t(M::NoteDone)),
        }
    }

    // ---- Node, Pi, clio-memory ---------------------------------------------------------------

    fn node(&mut self, i: usize) -> FRes<Out> {
        let Some(a) = self.pins.node.clone() else {
            return Ok(Out::Warn(self.t(M::NoteNoTarget)));
        };
        let ui = &mut *self.ui;
        let size = 30_000_000;
        match fetch::install_node(&self.dirs.data, &a, &mut |n| {
            Self::progress(ui, i, n, size.max(n));
        })? {
            None => Ok(Out::Done(a.version.clone())),
            Some(old) => Ok(Out::Done(self.changed("Node", &old, &a.version))),
        }
    }

    /// `name from → to` recorded, and the row's note.
    fn changed(&mut self, name: &str, from: &str, to: &str) -> String {
        if from.is_empty() {
            self.changes.push(format!("{name} {to}"));
            self.f(M::NoteInstalled, &[to])
        } else {
            self.changes.push(format!("{name} {from} → {to}"));
            self.f(M::NoteFromTo, &[from, to])
        }
    }

    fn pi(&mut self) -> FRes<Out> {
        use clio_brain::pi::install;
        // As `clio init`: the tests (and a machine that installs Pi on its own) leave it out.
        if std::env::var_os("CLIO_SKIP_PI_INSTALL").is_some() {
            return Ok(Out::Skip("CLIO_SKIP_PI_INSTALL".into()));
        }
        let paths = crate::cli::pi::paths(&self.dirs);
        if let Ok(i) = install::locate(&paths)
            && install::is_current(&i)
        {
            return Ok(Out::Done(i.version));
        }
        let before = std::fs::read_dir(&paths.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| install::installed_version(&e.path()))
            .max();
        let node = crate::cli::pi::node_path(&self.dirs);
        let mut r = None;
        self.ui.busy(&mut || {
            r = Some(
                paths
                    .ensure()
                    .and_then(|()| install::install(&paths, &node))
                    .map_err(|e| e.to_string()),
            );
        });
        let i = r.unwrap_or_else(|| Err("Pi was not installed".into()))?;
        // An earlier version's folder goes once the pinned one is in place.
        for e in std::fs::read_dir(&paths.root)
            .into_iter()
            .flatten()
            .flatten()
        {
            let p = e.path();
            if p != i.dir && install::installed_version(&p).is_some() {
                let _ = std::fs::remove_dir_all(&p);
            }
        }
        Ok(Out::Done(self.changed(
            "Pi",
            before.as_deref().unwrap_or(""),
            &i.version,
        )))
    }

    fn memory(&mut self, i: usize) -> FRes<Out> {
        let Some(m) = self.pins.memory.clone() else {
            return Ok(Out::Warn(self.t(M::NoteNoTarget)));
        };
        let bin = self.dirs.home.join(".local/bin");
        let ui = &mut *self.ui;
        let replaced = fetch::install_memory(&bin, &m, &mut |n| Self::progress(ui, i, n, 0))?;
        // Without its model the memory searches by text: a failure here is said and left for the
        // next update, which tries again (what was verified stays).
        let model_ok = self.pins.model.clone().is_none_or(|model| {
            let total = fetch::model_size(&model);
            let ui = &mut *self.ui;
            fetch::install_model(&self.dirs.home, &model, &mut |n| {
                Self::progress(ui, i, n, total.max(n));
            })
            .is_ok()
        });
        let have = fetch::memory_installed(&bin).unwrap_or_else(|| m.version.clone());
        let note = match replaced {
            Some(old) => self.changed("clio-memory", &old, &m.version),
            None if fetch::version(&have) > fetch::version(&m.version) => {
                self.f(M::NoteNewer, &[&have])
            }
            None => have,
        };
        Ok(if model_ok {
            Out::Done(note)
        } else {
            Out::Warn(self.t(M::NoteModelLater))
        })
    }

    // ---- the vault, the model and the judge --------------------------------------------------

    /// A secret from the file the flags named, else from the person (`None`: later).
    fn secret(&mut self, file: Option<&PathBuf>, q: Ask) -> Option<SecretValue> {
        if let Some(f) = file {
            let mut text = std::fs::read_to_string(f).ok()?;
            let v = SecretValue::new(text.trim_end_matches(['\n', '\r']).to_owned());
            clio_vault::wipe(&mut text);
            return (!v.is_empty()).then_some(v);
        }
        match self.ui.ask(q) {
            Some(Answer::Secret(v)) if !v.is_empty() => Some(v),
            _ => None,
        }
    }

    /// Whether there will be an open vault to keep a key in.
    fn vault_coming(&self) -> bool {
        self.answers.pass.is_some() || vault::unlocked(&self.dirs).is_ok()
    }

    /// A new vault's passphrase (kept for the vault step), or the existing one's, opened right
    /// away: a mistyped one is asked again.
    fn ask_vault(&mut self) {
        if self.opts.no_vault || self.dirs.runtime.is_none() {
            return;
        }
        let new = match vault::state(&self.dirs) {
            vault::State::Open => return,
            vault::State::Missing => true,
            _ => false,
        };
        let file = self.opts.passphrase_file.clone();
        let mut error = None;
        for _ in 0..3 {
            let (title, hint) = if new {
                (M::VaultNewTitle, M::VaultNewHint)
            } else {
                (M::VaultOpenTitle, M::VaultOpenHint)
            };
            let q = Ask::Secret {
                title: self.t(title),
                lines: vec![self.t(hint)],
                labels: if new {
                    vec![self.t(M::FieldPass), self.t(M::FieldAgain)]
                } else {
                    vec![self.t(M::FieldPass)]
                },
                error: error.take(),
            };
            let Some(pass) = self.secret(file.as_ref(), q) else {
                return;
            };
            if new {
                self.answers.pass = Some(pass);
                return;
            }
            match vault::start_with(&self.dirs, &pass) {
                Ok(_) => return,
                Err(e) if e.contains("wrong passphrase") && file.is_none() => {
                    error = Some(self.t(M::ErrWrongPass));
                }
                Err(e) if e.contains("wrong passphrase") => break,
                Err(e) => {
                    self.answers.vault = Some(Fail::Other(e));
                    return;
                }
            }
        }
        self.answers.vault = Some(Fail::Said(M::FailPass, M::FixPass));
    }

    fn vault(&mut self) -> FRes<Out> {
        if self.opts.no_vault {
            self.leave(M::LeftVault);
            return Ok(Out::Skip(self.t(M::NoteSkipped)));
        }
        self.dirs
            .need_runtime()
            .map_err(|_| Fail::Said(M::FailRuntime, M::FixRuntime))?;
        if let Some(e) = self.answers.vault.take() {
            self.leave(M::LeftVault);
            return Err(e);
        }
        if vault::state(&self.dirs) == vault::State::Open {
            return Ok(Out::Done(self.t(M::NoteOpened)));
        }
        let Some(pass) = self.answers.pass.take() else {
            self.leave(M::LeftVault);
            return Ok(Out::Warn(self.t(M::NoteLater)));
        };
        vault::start_with(&self.dirs, &pass).map_err(Fail::Other)?;
        Ok(Out::Done(self.t(M::NoteNewVault)))
    }

    /// The connector of the config, when the vault already has its key.
    fn connected(&self) -> Option<&'static Connector> {
        let c = Config::load(&self.dirs)
            .ok()
            .and_then(|c| by_provider(&c.model.provider))?;
        vault::has_model_key(&self.dirs, c).then_some(c)
    }

    fn ask_model(&mut self) {
        if self.opts.no_model || !self.vault_coming() || self.connected().is_some() {
            return;
        }
        let c = if self.opts.yes {
            self.opts.connector.as_deref().and_then(by_name)
        } else {
            let q = Ask::Choice {
                title: self.t(M::ModelTitle),
                lines: vec![self.t(M::ModelHint)],
                options: vec![
                    self.t(M::ModelClaude),
                    self.t(M::ModelApi),
                    self.t(M::ModelOpencode),
                    self.t(M::ModelLater),
                ],
                default: 0,
            };
            match self.ui.ask(q) {
                Some(Answer::Choice(0)) => by_name("claude"),
                Some(Answer::Choice(1)) => by_name("anthropic-api"),
                Some(Answer::Choice(2)) => by_name("opencode"),
                _ => None,
            }
        };
        let Some(c) = c else {
            return;
        };
        // A Claude subscription: paste a token now, or sign in when the install finishes.
        if c.name == "claude" && !self.opts.yes {
            let q = Ask::Choice {
                title: self.t(M::ClaudeHowTitle),
                lines: vec![],
                options: vec![self.t(M::ClaudeSignIn), self.t(M::ClaudePaste)],
                default: 0,
            };
            match self.ui.ask(q) {
                Some(Answer::Choice(0)) => {
                    self.answers.sign_in = true;
                    self.answers.model = Some((c, None));
                    return;
                }
                Some(Answer::Choice(_)) => {}
                // Esc: later, said on the last screen.
                _ => {
                    self.answers.model = Some((c, None));
                    return;
                }
            }
        }
        let hint = match c.name {
            "claude" => M::KeyClaudeHint,
            "anthropic-api" => M::KeyApiHint,
            _ => M::KeyOpencodeHint,
        };
        let file = self.opts.key_file.clone();
        let mut error = None;
        let key = loop {
            let q = Ask::Secret {
                title: self.f(M::KeyTitle, &[c.name]),
                lines: vec![self.t(hint)],
                labels: vec![self.t(M::FieldKey)],
                error: error.take(),
            };
            let Some(k) = self.secret(file.as_ref(), q) else {
                break None;
            };
            // A file's value is checked when it is stored: the step says what is wrong.
            if file.is_some() || k.expose().trim().starts_with(c.key_prefix) {
                break Some(k);
            }
            error = Some(self.f(M::ErrPrefix, &[c.key_prefix]));
        };
        self.answers.model = Some((c, key));
    }

    fn model(&mut self) -> FRes<Out> {
        if self.opts.no_model {
            self.leave(M::LeftModel);
            return Ok(Out::Skip(self.t(M::NoteSkipped)));
        }
        if vault::unlocked(&self.dirs).is_err() {
            self.leave(M::LeftVault);
            return Ok(Out::Skip(self.t(M::NoteNoVault)));
        }
        if let Some(c) = self.connected() {
            return Ok(Out::Done(c.name.into()));
        }
        let Some((c, key)) = self.answers.model.take() else {
            self.leave(M::LeftModel);
            return Ok(Out::Warn(self.t(M::NoteLater)));
        };
        if crate::cli::home::lacks_bridge(&self.dirs, c) {
            return Err(Fail::Other(
                "the Claude bridge (pi-claude-agent-sdk) is missing: clio pi install".into(),
            ));
        }
        if std::mem::take(&mut self.answers.sign_in) {
            self.answers.sign_in_ready = Some(c);
            return Ok(Out::Skip(self.t(M::NoteAtEnd)));
        }
        let Some(key) = key else {
            self.leave(if c.name == "claude" {
                M::LeftModelAgain
            } else {
                M::LeftModel
            });
            return Ok(Out::Warn(self.t(M::NoteLater)));
        };
        if !key.expose().trim().starts_with(c.key_prefix) {
            return Err(Fail::Other(self.f(M::ErrPrefix, &[c.key_prefix])));
        }
        let key = SecretValue::new(key.expose().trim().to_owned());
        crate::cli::connect::store_key(&self.dirs, c, &key)?;
        crate::cli::connect::switch_config(&self.dirs, c, c.default_model)?;
        Ok(Out::Done(c.name.into()))
    }

    fn ask_judge(&mut self) {
        if Config::load(&self.dirs).is_ok_and(|c| c.jev.enabled) || !self.vault_coming() {
            return;
        }
        let url = if self.opts.yes {
            match (&self.opts.temis_url, &self.opts.temis_key_file) {
                (Some(u), Some(_)) => Some(u.clone()),
                _ => None,
            }
        } else {
            let q = Ask::Choice {
                title: self.t(M::JudgeTitle),
                lines: vec![self.t(M::JudgeHint)],
                options: vec![self.t(M::JudgeSkip), self.t(M::JudgeTemis)],
                default: 0,
            };
            if matches!(self.ui.ask(q), Some(Answer::Choice(1))) {
                let mut error = None;
                loop {
                    let q = Ask::Text {
                        title: self.t(M::TemisUrlTitle),
                        lines: vec![self.f(M::TemisUrlHint, &[crate::cli::jev::TEMIS_URL])],
                        label: self.t(M::FieldUrl),
                        value: crate::cli::jev::TEMIS_URL.into(),
                        error: error.take(),
                    };
                    match self.ui.ask(q) {
                        Some(Answer::Text(u)) if temis_ok(&u) => break Some(u),
                        Some(Answer::Text(_)) => error = Some(self.t(M::ErrUrl)),
                        _ => break None,
                    }
                }
            } else {
                None
            }
        };
        let Some(url) = url else {
            return;
        };
        let key = if temis_ok(&url) {
            let q = Ask::Secret {
                title: self.t(M::TemisKeyTitle),
                lines: vec![self.t(M::TemisKeyHint)],
                labels: vec![self.t(M::FieldKey)],
                error: None,
            };
            let file = self.opts.temis_key_file.clone();
            self.secret(file.as_ref(), q)
        } else {
            None
        };
        self.answers.temis = Some((url, key));
    }

    fn judge(&mut self) -> FRes<Out> {
        if Config::load(&self.dirs).is_ok_and(|c| c.jev.enabled) {
            return Ok(Out::Done(self.t(M::NoteThere)));
        }
        let Some((url, key)) = self.answers.temis.take() else {
            return Ok(Out::Skip(self.t(M::NoteSkipped)));
        };
        if !temis_ok(&url) {
            return Err(Fail::Other(self.t(M::ErrUrl)));
        }
        if vault::unlocked(&self.dirs).is_err() {
            return Ok(Out::Skip(self.t(M::NoteNoVault)));
        }
        let Some(key) = key else {
            return Ok(Out::Skip(self.t(M::NoteSkipped)));
        };
        let (secret, domain) = crate::cli::jev::key_for(Some(&url));
        vault::put(&self.dirs, secret, &format!("jev:{domain}"), &key)?;
        crate::cli::connect::edit_section(&self.dirs, "jev", true, |m| {
            m.insert("enabled".into(), true.into());
            m.insert("base_url".into(), url.trim_end_matches('/').into());
        })?;
        Ok(Out::Done(self.t(M::NoteTemis)))
    }

    // ---- the timer, the doctor ---------------------------------------------------------------

    fn timer(&mut self) -> FRes<Out> {
        if !self.sys.linux {
            return Ok(Out::Skip(self.t(M::NoteMacTimer)));
        }
        let Some(sc) = self.sys.find("systemctl") else {
            return Ok(Out::Skip(self.t(M::NoteSkipped)));
        };
        let home = &self.dirs.home;
        let on = crate::cli::services::timer_on(home, Some(&sc));
        if self.opts.update {
            if !on {
                return Ok(Out::Skip(self.t(M::NoteOff)));
            }
            let ok = Command::new(&sc)
                .args(["--user", "restart", crate::cli::services::UNITS[1]])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            return Ok(if ok {
                Out::Done(self.t(M::NoteRestarted))
            } else {
                Out::Warn(self.t(M::NoteOff))
            });
        }
        if on {
            return Ok(Out::Done(self.t(M::NoteOn)));
        }
        // No systemd user session (a container, WSL): said, and the unit files just written go.
        if crate::cli::services::enable_in(home, &self.opts.exe, Some(&sc)).is_err() {
            let _ = crate::cli::services::disable_in(home, Some(&sc));
            return Ok(Out::Warn(self.t(M::NoteTimerLater)));
        }
        Ok(Out::Done(self.t(M::NoteOn)))
    }

    fn doctor(&mut self) -> Out {
        if !self.opts.doctor {
            return Out::Skip(self.t(M::NoteSkipped));
        }
        // In the person's language: the doctor's own lines are English, and `clio doctor` has
        // them all.
        let r = crate::cli::doctor::collect_with(&self.dirs, false);
        let bad = r
            .checks
            .iter()
            .any(|c| c.level == crate::cli::ui::Level::Fail);
        let line = self.t(if bad {
            M::NoteDoctorIssues
        } else {
            M::NoteDoctorOk
        });
        self.doctor = Some(line.clone());
        if bad {
            Out::Warn(line)
        } else {
            Out::Done(line)
        }
    }

    // ---- removing ----------------------------------------------------------------------------

    fn remove_timer(&mut self) -> FRes<Out> {
        let sc = self.sys.find("systemctl");
        let r = crate::cli::services::disable_in(&self.dirs.home, sc.as_deref());
        // systemd not answering (no user session) is said; the unit files are gone either way.
        Ok(match r {
            Ok(_) => Out::Done(self.t(M::NoteRemoved)),
            Err(e) if crate::cli::services::timer_files_exist(&self.dirs.home) => {
                return Err(Fail::Other(e));
            }
            Err(_) => Out::Warn(self.t(M::NoteRemovedNoSystemd)),
        })
    }

    /// The private Node and Pi's installed versions; Pi's sessions, agent and home folders are
    /// the person's conversations and stay.
    fn remove_node_pi(&mut self) -> FRes<Out> {
        rm(&fetch::node_dir(&self.dirs.data))?;
        let root = crate::cli::pi::paths(&self.dirs).root;
        for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if fetch::version(&name).is_some() || name.starts_with(".install-") {
                rm(&e.path())?;
            }
        }
        Ok(Out::Done(self.t(M::NoteRemoved)))
    }

    /// The programs: clio, clio-vault, clio-memory, and the PATH line install.sh added.
    fn remove_binaries(&mut self) -> FRes<Out> {
        for name in fetch::BINARIES {
            rm(&self.opts.bin.join(name))?;
        }
        rm(&self.dirs.home.join(".local/bin/clio-memory"))?;
        // What the installer noted about the binaries goes with them.
        rm(&super::version_file(&self.dirs))?;
        rm(&crate::cli::update::cache(&self.dirs))?;
        for rc in [".bash_profile", ".profile", ".zshrc"] {
            unpath(&self.dirs.home.join(rc))?;
        }
        Ok(Out::Done(self.t(M::NoteRemoved)))
    }

    fn remove_data(&mut self) -> FRes<Out> {
        rm(&self.dirs.config)?;
        rm(&self.dirs.data)?;
        if let Some(r) = &self.dirs.runtime {
            rm(r)?;
        }
        Ok(Out::Done(self.t(M::NoteRemoved)))
    }
}

/// A failure as one plain sentence and what to do, in the person's language.
pub(crate) fn said(lang: Lang, e: &Fail) -> (String, String) {
    match e {
        Fail::Net(what) => (M::FailNet.fill(lang, &[what]), M::FixNet.get(lang).into()),
        Fail::Sum(what) => (M::FailSum.fill(lang, &[what]), M::FixSum.get(lang).into()),
        Fail::Said(why, fix) => (why.get(lang).into(), fix.get(lang).into()),
        Fail::Other(s) => (sanitize::line(s), M::FixOther.get(lang).into()),
    }
}

/// Runs `c` as root (through `sudo` unless this is root), with `input` on its standard input.
fn as_root(sudo: Option<&Path>, c: &[String], input: Option<&str>) -> bool {
    let mut cmd = match sudo {
        Some(s) => {
            let mut x = Command::new(s);
            x.args(c);
            x
        }
        None => {
            let mut x = Command::new(&c[0]);
            x.args(&c[1..]);
            x
        }
    };
    let Some(text) = input else {
        return cmd.status().is_ok_and(|s| s.success());
    };
    let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).spawn() else {
        return false;
    };
    if let Some(mut i) = child.stdin.take() {
        let _ = std::io::Write::write_all(&mut i, text.as_bytes());
    }
    child.wait().is_ok_and(|s| s.success())
}

/// The line install.sh adds to a profile, after its mark.
const PATH_MARK: &str = "# Clio: ~/.local/bin on the PATH";
const PATH_LINE: &str = "export PATH=\"$HOME/.local/bin:$PATH\"";

/// Takes install.sh's PATH line (its mark, the line, and the blank line it put before them) out
/// of `rc`; the rest of the file stays as it was.
fn unpath(rc: &Path) -> FRes<()> {
    let Ok(text) = std::fs::read_to_string(rc) else {
        return Ok(());
    };
    let added = format!("\n{PATH_MARK}\n{PATH_LINE}\n");
    let mut out = text.replace(&added, "");
    if out == text {
        out = text.replace(&format!("{PATH_MARK}\n{PATH_LINE}\n"), "");
    }
    if out == text {
        return Ok(());
    }
    // Only the PATH line was there: install.sh made the file, so it goes.
    if out.trim().is_empty() {
        return rm(rc);
    }
    std::fs::write(rc, out).map_err(|e| Fail::Other(crate::cli::io_err(rc, &e)))
}

/// Removes a file or a folder; one that is not there is fine.
fn rm(p: &Path) -> FRes<()> {
    let r = match std::fs::symlink_metadata(p) {
        Err(_) => return Ok(()),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
    };
    r.map_err(|e| Fail::Other(crate::cli::io_err(p, &e)))
}

/// Only an address Clio's judge sends the Temis key to.
fn temis_ok(url: &str) -> bool {
    crate::cli::jev::key_for(Some(url.trim())).0 == "temis"
}
