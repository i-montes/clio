//! Where Clio lives on this machine (ADR 0006), its configuration, and `clio init`.

use std::path::{Path, PathBuf};

use clio_brain::connector::{CONNECTORS, Connector, by_provider};
use clio_brain::memory_link::{EmbedModel, MemoryLink};
use clio_brain::pi::launch::Secret;
use serde::{Deserialize, Serialize};

use super::ui::{Level, Style};
use super::{Res, io_err, mkdir_private, write_private};

/// The adapter that ships with the repository; `init` installs this copy when it cannot find the
/// repository's own file.
const ADAPTER_SOURCE: &str = include_str!("../../../../adapters/models/openai_compat.py");
const CLAUDE_ADAPTER_SOURCE: &str = include_str!("../../../../adapters/models/claude_code.py");
const API_ADAPTER_SOURCE: &str = include_str!("../../../../adapters/models/anthropic_api.py");
/// The default system prompt, compiled in so the binary works without the repository.
pub const DEFAULT_PROMPT_NAME: &str = "clio-agent@2";
const DEFAULT_PROMPT: &str = include_str!("../../../../prompts/system/clio-agent@2.md");
/// The same prompt plus `run_tests`: used instead of the default when the jail is proven.
pub const JAIL_PROMPT_NAME: &str = "clio-agent@3";
const JAIL_PROMPT: &str = include_str!("../../../../prompts/system/clio-agent@3.md");

pub struct Dirs {
    pub config: PathBuf,
    pub data: PathBuf,
    pub runs: PathBuf,
    /// `$XDG_RUNTIME_DIR/clio`; `None` when the variable is not set.
    pub runtime: Option<PathBuf>,
    pub home: PathBuf,
}

impl Dirs {
    pub fn from_env() -> Res<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
        if !home.is_absolute() {
            return Err("HOME must be an absolute path".into());
        }
        let data = home.join(".local/share/clio");
        Ok(Self {
            config: home.join(".config/clio"),
            runs: data.join("runs"),
            data,
            runtime: std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|v| !v.is_empty())
                .map(|v| PathBuf::from(v).join("clio")),
            home,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.json")
    }

    /// Clio's own folders (canonical when they exist): in host mode the file tools refuse them and
    /// commands see them empty. clio-memory's folder is Clio's too when it lives elsewhere.
    pub fn control(&self) -> Vec<PathBuf> {
        let mut v = vec![self.config.clone(), self.data.clone()];
        v.extend(self.runtime.clone());
        v.extend(clio_memory::agent::default_data().filter(|m| !m.starts_with(&self.data)));
        for p in &mut v {
            if let Ok(c) = p.canonicalize() {
                *p = c;
            }
        }
        v.dedup();
        v
    }

    /// `host.json`, the person's additions to the host-mode rules, when there is one.
    pub fn host_rules(&self) -> Option<PathBuf> {
        Some(self.config.join("host.json")).filter(|p| p.is_file())
    }

    pub fn need_runtime(&self) -> Res<&Path> {
        self.runtime
            .as_deref()
            .ok_or_else(|| "XDG_RUNTIME_DIR is not set (log in through a normal session)".into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCfg {
    /// A connector's provider id (`crate::connector`): `opencode-go` or `claude`.
    pub provider: String,
    pub model: String,
    /// Program and arguments of the model adapter (reads a request on stdin, answers on stdout).
    /// Empty (what `init` writes): the adapter inside this binary (`Config::model_command`).
    #[serde(default)]
    pub command: Vec<String>,
    /// `name@version`: the built-in prompt, or `<config>/prompts/<name>@<version>.md`.
    pub prompt: String,
    pub price_in_micro_per_mtok: u64,
    pub price_out_micro_per_mtok: u64,
    /// Where Pi reaches the model (the `pi` harness; the builtin adapter has its own address).
    /// `None`: the connector's usual address. Meant for a local fake model in tests, so it may
    /// only be a loopback address (checked when the config is loaded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// How hard the model thinks in a Pi session (`/effort` changes it): one of Pi's levels.
    /// `low` by default: at Pi's own default ("medium") a reasoning model thinks for thousands
    /// of tokens on a simple step (measured 46 s against 7 s for the same answer).
    #[serde(default = "thinking_low")]
    pub thinking: String,
    /// The model last chosen for each connector (by its provider id), so `/connector` comes back
    /// to it instead of the connector's default.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub last: std::collections::BTreeMap<String, String>,
}

fn thinking_low() -> String {
    "low".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    pub max_steps: u32,
    pub max_seconds: u64,
    pub max_cost_micro_usd: u64,
    /// Host mode: what all sessions of one UTC day may spend and do together.
    #[serde(default = "day_cost")]
    pub day_cost_micro_usd: u64,
    #[serde(default = "day_steps")]
    pub day_steps: u64,
}

fn day_cost() -> u64 {
    3_000_000
}

fn day_steps() -> u64 {
    1000
}

/// Where the tools act: on the real machine (the factory one since 2026-11-18, ADR 0012), or on
/// a copy of the project.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Copy,
    #[default]
    Host,
}

impl Mode {
    /// What `--host` or `--copy` say, which decides over the configuration; `None`: neither.
    pub fn from_flags(host: bool, copy: bool) -> Res<Option<Mode>> {
        match (host, copy) {
            (true, true) => Err("--host and --copy do not go together".into()),
            (true, false) => Ok(Some(Mode::Host)),
            (false, true) => Ok(Some(Mode::Copy)),
            (false, false) => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceLimits {
    pub max_files: u64,
    pub max_bytes: u64,
}

/// Jev (TypeSafe System One), optional: off until the person turns it on, and everything works
/// without it (spec `docs/specs/2026-11-03-jev-design.md`). It needs the secret `jev` in the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JevCfg {
    #[serde(default)]
    pub enabled: bool,
    /// Put the memories that matter in front of each message (Jev picks them; without Jev the
    /// memory's own rules do).
    #[serde(default = "yes")]
    pub memory: bool,
    /// Suggest tools (only with Jev).
    #[serde(default = "yes")]
    pub tool_hints: bool,
    /// `https://api.imontes.dev/temis` to ask Temis instead (its key is the vault's `temis`), or
    /// for tests a loopback URL, in place of `https://api.typesafe.ai`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for JevCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            memory: true,
            tool_hints: true,
            base_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub mode: Mode,
    pub model: ModelCfg,
    pub limits: Limits,
    pub workspace: WorkspaceLimits,
    #[serde(default)]
    pub jev: JevCfg,
    #[serde(default)]
    pub memory: MemoryCfg,
    #[serde(default)]
    pub session: SessionCfg,
    #[serde(default)]
    pub review: ReviewCfg,
    #[serde(default)]
    pub ui: UiCfg,
    #[serde(default)]
    pub approvals: ApprovalsCfg,
}

/// When Clio opens the approval boxes by herself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalsCfg {
    /// At most once every this many hours (the rest of the time the items wait, quietly, in
    /// `/pending`); 0 asks at once.
    #[serde(default = "every_hours")]
    pub every_hours: u64,
}

fn every_hours() -> u64 {
    4
}

impl Default for ApprovalsCfg {
    fn default() -> Self {
        Self {
            every_hours: every_hours(),
        }
    }
}

/// The chat's screen.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiCfg {
    /// The chat takes the mouse (clicks, the wheel); off, the terminal selects and copies text.
    #[serde(default)]
    pub mouse: bool,
}

/// The reviewer: a second agent that checks a piece of work (`/review`, or when the judge says so).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCfg {
    /// Its model. Unset: `claude-opus-5-5` with the `claude` connector, the session's otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "review_thinking")]
    pub thinking: String,
    #[serde(default = "review_seconds")]
    pub max_seconds: u64,
    #[serde(default = "review_steps")]
    pub max_steps: u32,
    /// After a turn that changed something, the judge decides whether it is reviewed.
    #[serde(default = "yes")]
    pub auto: bool,
    #[serde(default = "review_threshold")]
    pub threshold: f64,
    #[serde(default = "review_per_hour")]
    pub max_per_hour: u32,
    /// What one review may spend (micro-dollars): its own cap, not `limits.max_cost_micro_usd`.
    #[serde(default = "review_cost")]
    pub max_cost_micro_usd: u64,
}

/// The reviewer's model with the `claude` connector when `review.model` is not set.
pub const REVIEW_MODEL_CLAUDE: &str = "claude-opus-5-5";

fn review_thinking() -> String {
    "medium".into()
}
fn review_seconds() -> u64 {
    600
}
fn review_steps() -> u32 {
    40
}
fn review_threshold() -> f64 {
    0.8
}
fn review_per_hour() -> u32 {
    3
}
fn review_cost() -> u64 {
    1_000_000
}

impl ReviewCfg {
    /// Values edited by hand are held to their ranges, and a thinking level Pi does not know is
    /// `medium`: a bad `review.*` key never keeps the config from loading.
    fn clamp(&mut self) {
        if !clio_brain::pi::launch::THINKING.contains(&self.thinking.as_str()) {
            self.thinking = review_thinking();
        }
        self.threshold = self.threshold.clamp(0.0, 1.0);
        self.max_per_hour = self.max_per_hour.min(60);
        self.max_seconds = self.max_seconds.clamp(30, 3600);
        self.max_steps = self.max_steps.clamp(1, 200);
    }
}

impl Default for ReviewCfg {
    fn default() -> Self {
        Self {
            model: None,
            thinking: review_thinking(),
            max_seconds: review_seconds(),
            max_steps: review_steps(),
            auto: true,
            threshold: review_threshold(),
            max_per_hour: review_per_hour(),
            max_cost_micro_usd: review_cost(),
        }
    }
}

/// One continuous conversation (`docs/specs/2026-11-18-continuous-conversation-design.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCfg {
    /// `clio` reopens the last conversation instead of a new session (`clio --new` does not).
    #[serde(default = "yes")]
    pub continuous: bool,
    /// Between turns, a context over this many tokens is compacted (`/compact N` sets it).
    #[serde(default = "context_cap", deserialize_with = "cap_in_range")]
    pub context_cap_tokens: u64,
}

pub const CONTEXT_CAP: u64 = 200_000;

fn context_cap() -> u64 {
    CONTEXT_CAP
}

/// A cap edited by hand in `config.json` is held to what `/compact N` accepts (50k to 1M).
fn cap_in_range<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    u64::deserialize(d).map(|n| n.clamp(50_000, 1_000_000))
}

impl Default for SessionCfg {
    fn default() -> Self {
        Self {
            continuous: true,
            context_cap_tokens: CONTEXT_CAP,
        }
    }
}

/// The memory of the Pi sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCfg {
    /// Ask the model for the session's summary when the conversation is compacted and when the
    /// person closes a session of at least four messages.
    #[serde(default = "yes")]
    pub session_summary: bool,
}

impl Default for MemoryCfg {
    fn default() -> Self {
        Self {
            session_summary: true,
        }
    }
}

impl ModelCfg {
    /// The provider must be a connector, and `base_url` may only point at this machine: the key
    /// goes to the connector's own server, never to an address a config file names.
    fn check(&self, file: &Path) -> Res<()> {
        let at = file.display();
        if clio_brain::connector::by_provider(&self.provider).is_none() {
            return Err(format!(
                "{at}: model.provider {:?} is not a connector: use clio connect opencode|claude|anthropic-api",
                self.provider
            ));
        }
        if !clio_brain::pi::launch::THINKING.contains(&self.thinking.as_str()) {
            return Err(format!(
                "{at}: model.thinking {:?} is not a Pi level ({})",
                self.thinking,
                clio_brain::pi::launch::THINKING.join(", ")
            ));
        }
        if let Some(u) = &self.base_url
            && !clio_brain::connector::is_loopback(u)
        {
            return Err(format!(
                "{at}: model.base_url can only point at this machine (127.0.0.1, localhost, [::1])"
            ));
        }
        Ok(())
    }
}

impl Config {
    pub fn default_with(command: Vec<String>) -> Self {
        Self {
            mode: Mode::Host,
            model: ModelCfg {
                provider: "opencode-go".into(),
                model: "beam-free".into(),
                command,
                prompt: DEFAULT_PROMPT_NAME.into(),
                price_in_micro_per_mtok: 0,
                price_out_micro_per_mtok: 0,
                base_url: None,
                thinking: thinking_low(),
                last: std::collections::BTreeMap::new(),
            },
            limits: Limits {
                max_steps: 40,
                max_seconds: 600,
                max_cost_micro_usd: 100_000,
                day_cost_micro_usd: day_cost(),
                day_steps: day_steps(),
            },
            workspace: WorkspaceLimits {
                max_files: 5000,
                max_bytes: 200 * 1024 * 1024,
            },
            jev: JevCfg::default(),
            memory: MemoryCfg::default(),
            session: SessionCfg::default(),
            review: ReviewCfg::default(),
            ui: UiCfg::default(),
            approvals: ApprovalsCfg::default(),
        }
    }

    pub fn load(dirs: &Dirs) -> Res<Self> {
        Self::load_inner(dirs, None)
    }

    /// The config as `clio connect` will leave it: `provider` in place of the file's, so a
    /// provider that is not a connector (what connect repairs) does not stop it. Every other
    /// check still holds.
    pub fn load_with_provider(dirs: &Dirs, provider: &str) -> Res<Self> {
        Self::load_inner(dirs, Some(provider))
    }

    fn load_inner(dirs: &Dirs, provider: Option<&str>) -> Res<Self> {
        let p = dirs.config_file();
        let text = std::fs::read_to_string(&p)
            .map_err(|e| format!("{}: {e} (run `clio init` first)", p.display()))?;
        let mut cfg: Self =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        if let Some(provider) = provider {
            cfg.model.provider = provider.into();
        }
        cfg.model.check(&p)?;
        cfg.review.clamp();
        Ok(cfg)
    }

    /// The connector of `model.provider` (`load` checks that there is one).
    fn connector(&self) -> Res<&'static Connector> {
        by_provider(&self.model.provider).ok_or_else(|| {
            format!(
                "model.provider {:?} is not a connector",
                self.model.provider
            )
        })
    }

    /// The model adapter's program and arguments as a run starts it: the adapter of the active
    /// connector, inside this binary (`names_builtin_adapter`), as `python3 <its copy>` (the
    /// Claude one followed by `--claude <Claude Code of the installed Pi>`), any other command as
    /// written.
    pub fn model_command(&self, dirs: &Dirs) -> Res<Vec<String>> {
        if !names_builtin_adapter(&self.model.command) {
            return Ok(self.model.command.clone());
        }
        let c = self.connector()?;
        // Found first, so a missing Claude Code writes nothing.
        let claude = if c.adapter == CLAUDE_ADAPTER_FILE {
            Some(claude_code(dirs)?)
        } else {
            None
        };
        let mut cmd = vec![
            "python3".to_owned(),
            builtin_adapter(dirs, c)?.display().to_string(),
        ];
        if let Some(bin) = claude {
            cmd.extend(["--claude".to_owned(), bin.display().to_string()]);
        }
        Ok(cmd)
    }

    /// The model key for `command` (as `model_command` returned it): only the adapter inside this
    /// binary gets it, in its envelope; the person's own command never does.
    pub fn model_key(&self, dirs: &Dirs, command: &[String]) -> Res<Option<Secret>> {
        if !names_builtin_adapter(command) {
            return Ok(None);
        }
        super::vault::model_key(dirs, self.connector()?).map(Some)
    }

    /// The system prompt (name for the trace, text) for `model.prompt`. The default one becomes
    /// `clio-agent@3` when the jail is proven; a prompt the person chose is never swapped.
    pub fn prompt_text(&self, dirs: &Dirs, jail: bool) -> Res<(String, String)> {
        if self.model.prompt == DEFAULT_PROMPT_NAME {
            return Ok(if jail {
                (JAIL_PROMPT_NAME.to_owned(), JAIL_PROMPT.to_owned())
            } else {
                (DEFAULT_PROMPT_NAME.to_owned(), DEFAULT_PROMPT.to_owned())
            });
        }
        let name = &self.model.prompt;
        if name.contains('/') || name.contains("..") {
            return Err(format!("bad prompt name {name:?}"));
        }
        let p = dirs.config.join("prompts").join(format!("{name}.md"));
        let text = std::fs::read_to_string(&p).map_err(|e| io_err(&p, &e))?;
        Ok((name.clone(), text))
    }
}

/// The file name of the OpenCode adapter that travels inside this binary.
const ADAPTER_FILE: &str = "openai_compat.py";
/// The file name of the Claude adapter (it runs Claude Code).
const CLAUDE_ADAPTER_FILE: &str = "claude_code.py";
/// The file name of the Anthropic API adapter.
const API_ADAPTER_FILE: &str = "anthropic_api.py";
/// Built-in adapters older Clios wrote and this one no longer has; the only files cleaned up.
const RETIRED_ADAPTERS: [&str; 1] = ["anthropic_oauth.py"];
/// The file names of the adapters that travel inside this binary, one per connector.
const ADAPTER_FILES: [&str; 3] = [ADAPTER_FILE, CLAUDE_ADAPTER_FILE, API_ADAPTER_FILE];

/// Whether `command` names an adapter inside this binary: empty (what `init` writes), or
/// `python3` and one of their files in an `adapters/` folder, which is what older versions of
/// `init` wrote (a checkout's file, or the copy in the data folder), or the Claude one with its
/// `--claude <path>` (what `model_command` returns). That path is not run: it can be gone (the
/// checkout moved) or changed, and the adapter receives the model key on every call and runs
/// outside the jail.
pub fn names_builtin_adapter(command: &[String]) -> bool {
    let in_adapters = |script: &str, files: &[&str]| {
        let p = Path::new(script);
        p.file_name().is_some_and(|n| files.iter().any(|f| n == *f))
            && p.ancestors()
                .any(|a| a.file_name().is_some_and(|n| n == "adapters"))
    };
    match command {
        [] => true,
        [python, script] if python == "python3" => in_adapters(script, &ADAPTER_FILES),
        [python, script, flag, _] if python == "python3" && flag == "--claude" => {
            in_adapters(script, &[CLAUDE_ADAPTER_FILE])
        }
        _ => false,
    }
}

/// Whether connector `c` runs through the Claude bridge and the installed Pi lacks it (the
/// extension, or the Claude Code its SDK brings).
pub fn lacks_bridge(dirs: &Dirs, c: &Connector) -> bool {
    let Some(x) = c.pi_extension else {
        return false;
    };
    !clio_brain::pi::install::locate(&super::pi::paths(dirs)).is_ok_and(|i| {
        i.dir.join(x).is_file() && clio_brain::pi::install::claude_code_bin(&i.dir).is_some()
    })
}

/// The Claude Code binary inside the installed Pi (the Agent SDK brings it with the bridge).
pub fn claude_code(dirs: &Dirs) -> Res<PathBuf> {
    clio_brain::pi::install::locate(&super::pi::paths(dirs))
        .ok()
        .and_then(|i| clio_brain::pi::install::claude_code_bin(&i.dir))
        .ok_or_else(|| "Claude Code (pi-claude-agent-sdk) is missing: clio pi install".to_owned())
}

/// Where the adapter file `file` inside this binary is written: `<data>/adapters/<file>`.
fn adapter_copy(dirs: &Dirs, file: &str) -> PathBuf {
    dirs.data.join("adapters").join(file)
}

/// Where the adapter of connector `c` is written.
pub fn builtin_adapter_path(dirs: &Dirs, c: &Connector) -> PathBuf {
    adapter_copy(dirs, c.adapter)
}

/// The source of the adapter of connector `c`, which travels inside this binary.
fn adapter_source(c: &Connector) -> Res<&'static str> {
    match c.adapter {
        ADAPTER_FILE => Ok(ADAPTER_SOURCE),
        CLAUDE_ADAPTER_FILE => Ok(CLAUDE_ADAPTER_SOURCE),
        API_ADAPTER_FILE => Ok(API_ADAPTER_SOURCE),
        _ => Err(format!("the adapter of {} is not there yet", c.name)),
    }
}

/// The adapter of connector `c` inside this binary, on disk: written private, and written again
/// when the file differs (a newer Clio brings its adapter along). `CLIO_ADAPTER` puts another
/// script in its place (for working on the adapter).
fn builtin_adapter(dirs: &Dirs, c: &Connector) -> Res<PathBuf> {
    if let Some(p) = std::env::var_os("CLIO_ADAPTER") {
        return Ok(PathBuf::from(p));
    }
    let source = adapter_source(c)?;
    let p = adapter_copy(dirs, c.adapter);
    if std::fs::read_to_string(&p).ok().as_deref() != Some(source) {
        mkdir_private(&dirs.data.join("adapters"))?;
        write_private(&p, source.as_bytes())?;
    }
    // Copies an older Clio wrote of adapters it no longer has (`anthropic_oauth.py`) go away.
    for e in std::fs::read_dir(dirs.data.join("adapters"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let n = e.file_name();
        if RETIRED_ADAPTERS.iter().any(|f| n == *f) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(p)
}

pub fn init(dirs: &Dirs) -> Res<()> {
    let st = Style::stdout();
    let ok = st.mark(Level::Pass);
    let made = |d: &Path| -> Res<()> {
        let existed = d.is_dir();
        mkdir_private(d)?;
        println!(
            "{ok}  {} {} (0700)",
            if existed {
                "existed     "
            } else {
                "created     "
            },
            d.display()
        );
        Ok(())
    };
    made(&dirs.config)?;
    // Where the person's own slash commands go: born private, so `clio doctor` (S-23) has nothing
    // to say about a folder the person would otherwise make with the default permissions.
    made(&dirs.config.join("commands"))?;
    made(&dirs.data)?;
    made(&dirs.runs)?;
    made(dirs.need_runtime()?)?;

    // The memory file lives here and nowhere else; it is created now so the first run finds it.
    match MemoryLink::open_write(&dirs.data, &EmbedModel::none()) {
        Ok(link) => {
            let (a, q, _) = link.counts().unwrap_or((0, 0, 0));
            println!(
                "{ok}  memory       {} (0600; {a} memories, {q} in quarantine)",
                MemoryLink::db_path(&dirs.data).display()
            );
        }
        Err(e) => println!("{}  memory: {e}", st.mark(Level::Warn)),
    }
    let cfg = dirs.config_file();
    if cfg.exists() {
        println!("{ok}  exists       {} (left as it is)", cfg.display());
    } else {
        // No path in the file: an empty command is the adapter inside this binary, so the file
        // keeps working when Clio is updated or moved to another machine.
        let c = Config::default_with(Vec::new());
        let text = serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?;
        write_private(&cfg, text.as_bytes())?;
        println!("{ok}  created      {} (0600)", cfg.display());
    }
    super::pi::init_lines(dirs);
    // The active connector (a new config is OpenCode); the key is never shown.
    let c = Config::load(dirs)
        .ok()
        .and_then(|c| by_provider(&c.model.provider))
        .unwrap_or(&CONNECTORS[0]);
    if std::env::var_os(c.key_env).is_some_and(|v| !v.is_empty()) {
        println!("{ok}  model key: it comes from {} (not shown)", c.key_env);
    } else if super::vault::has_model_key(dirs, c) {
        println!("{ok}  model key: it is in the vault (not shown)");
    } else {
        println!(
            "{}  the model key is missing: start the vault (clio vault start) and run clio connect {}",
            st.mark(Level::Warn),
            c.name
        );
    }
    println!(
        "\nnext: {}, and then {}",
        st.bold("clio doctor"),
        st.bold("clio")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::mode;

    fn cmd(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
    }

    // A cap edited by hand is held to 50k–1M when read; a missing one is the default.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_context_cap_read_from_the_config_is_clamped() {
        let cap = |v: &str| {
            serde_json::from_str::<SessionCfg>(v)
                .unwrap()
                .context_cap_tokens
        };
        assert_eq!(cap(r#"{"context_cap_tokens": 1}"#), 50_000);
        assert_eq!(cap(r#"{"context_cap_tokens": 99999999999}"#), 1_000_000);
        assert_eq!(cap(r#"{"context_cap_tokens": 300000}"#), 300_000);
        assert_eq!(cap("{}"), CONTEXT_CAP);
    }

    // The `review.*` keys edited by hand are held to their ranges when read, and a bad thinking
    // level is `medium`: the config still loads.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_review_keys_are_clamped_and_a_bad_thinking_level_is_medium() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let dirs = Dirs {
            config: t.path().join("config"),
            runs: data.join("runs"),
            data,
            runtime: None,
            home: t.path().to_path_buf(),
        };
        std::fs::create_dir_all(&dirs.config).unwrap();
        let load = |review: serde_json::Value| {
            let mut v = serde_json::to_value(Config::default_with(vec![])).unwrap();
            v["review"] = review;
            std::fs::write(dirs.config_file(), v.to_string()).unwrap();
            Config::load(&dirs).unwrap().review
        };
        let r = load(serde_json::json!({}));
        assert_eq!(
            (r.thinking.as_str(), r.max_cost_micro_usd, r.max_steps),
            ("medium", 1_000_000, 40)
        );
        let r = load(
            serde_json::json!({"thinking": "ultra", "threshold": 7.5, "max_per_hour": 500,
            "max_seconds": 1, "max_steps": 0, "max_cost_micro_usd": 3}),
        );
        assert_eq!(r.thinking, "medium");
        assert!((r.threshold - 1.0).abs() < f64::EPSILON);
        assert_eq!((r.max_per_hour, r.max_seconds, r.max_steps), (60, 30, 1));
        assert_eq!(r.max_cost_micro_usd, 3);
        let r = load(
            serde_json::json!({"threshold": -1, "max_seconds": 99999, "max_steps": 999,
            "thinking": "high", "max_per_hour": 0}),
        );
        assert_eq!(r.thinking, "high");
        assert!(r.threshold.abs() < f64::EPSILON);
        assert_eq!((r.max_per_hour, r.max_seconds, r.max_steps), (0, 3600, 200));
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn writing_an_adapter_removes_copies_of_retired_ones() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let dirs = Dirs {
            config: t.path().join("config"),
            runs: data.join("runs"),
            data: data.clone(),
            runtime: None,
            home: t.path().into(),
        };
        let a = data.join("adapters");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("anthropic_oauth.py"), "old").unwrap();
        std::fs::write(a.join("notas.txt"), "x").unwrap();
        std::fs::write(a.join("mine.py"), "mine").unwrap();
        let c = clio_brain::connector::by_name("opencode").unwrap();
        let p = builtin_adapter(&dirs, c).unwrap();
        assert!(p.is_file() && !a.join("anthropic_oauth.py").exists());
        assert!(a.join("notas.txt").exists() && a.join("mine.py").exists());
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_mode_is_host_by_default_and_the_flags_decide_over_the_file() {
        assert_eq!(Config::default_with(vec![]).mode, Mode::Host);
        let mut v = serde_json::to_value(Config::default_with(vec![])).unwrap();
        // A file from before the mode and the day's limits existed still loads, as host.
        v.as_object_mut().unwrap().remove("mode");
        v["limits"].as_object_mut().unwrap().remove("day_steps");
        v["limits"]
            .as_object_mut()
            .unwrap()
            .remove("day_cost_micro_usd");
        let old: Config = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(old.mode, Mode::Host);
        assert_eq!(
            (old.limits.day_cost_micro_usd, old.limits.day_steps),
            (3_000_000, 1000)
        );
        v["mode"] = serde_json::json!("copy");
        let copy: Config = serde_json::from_value(v).unwrap();
        assert_eq!(copy.mode, Mode::Copy);
        assert_eq!(Mode::from_flags(false, false).unwrap(), None);
        assert_eq!(Mode::from_flags(true, false).unwrap(), Some(Mode::Host));
        assert_eq!(Mode::from_flags(false, true).unwrap(), Some(Mode::Copy));
        assert!(Mode::from_flags(true, true).is_err());
    }

    #[test]
    fn what_names_the_builtin_adapter() {
        assert!(names_builtin_adapter(&[]));
        // What older versions of `init` wrote: a checkout's file, or the copy in the data folder.
        assert!(names_builtin_adapter(&cmd(&[
            "python3",
            "/home/ana/clio/adapters/models/openai_compat.py"
        ])));
        assert!(names_builtin_adapter(&cmd(&[
            "python3",
            "/home/ana/.local/share/clio/adapters/openai_compat.py"
        ])));
        assert!(names_builtin_adapter(&cmd(&[
            "python3",
            "/home/ana/.local/share/clio/adapters/anthropic_api.py"
        ])));
        // The Claude one as `model_command` resolves it: with the path of Claude Code.
        assert!(names_builtin_adapter(&cmd(&[
            "python3",
            "/home/ana/.local/share/clio/adapters/claude_code.py",
            "--claude",
            "/home/ana/.local/share/clio/pi/1.4.0/node_modules/x/claude"
        ])));
        // Anything else is the person's own adapter, run as written.
        for other in [
            cmd(&["python3", "/home/ana/mi_adaptador.py"]),
            cmd(&["python3", "/tmp/openai_compat.py"]),
            cmd(&[
                "python3.12",
                "/home/ana/clio/adapters/models/openai_compat.py",
            ]),
            cmd(&["python3", "/x/adapters/models/openai_compat.py", "--debug"]),
            cmd(&[
                "python3",
                "/x/adapters/openai_compat.py",
                "--claude",
                "/x/claude",
            ]),
            cmd(&[
                "python3",
                "/x/adapters/claude_code.py",
                "--other",
                "/x/claude",
            ]),
            cmd(&["python3", "/x/adapters/anthropic_old.py"]),
            cmd(&["./adapter"]),
        ] {
            assert!(!names_builtin_adapter(&other), "{other:?}");
        }
    }

    #[test]
    fn the_builtin_adapter_is_written_and_refreshed_when_it_differs() {
        let t = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: t.path().join("config"),
            data: t.path().join("data"),
            runs: t.path().join("data/runs"),
            runtime: None,
            home: t.path().to_path_buf(),
        };
        let mut c = Config::default_with(Vec::new());
        let got = c.model_command(&dirs).unwrap();
        let copy = builtin_adapter_path(&dirs, &CONNECTORS[0]);
        assert_eq!(got, cmd(&["python3", &copy.display().to_string()]));
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), ADAPTER_SOURCE);
        assert_eq!(mode(&copy), Some(0o600));
        // A copy changed on disk (or left by an older Clio) is not what runs: it is written again.
        std::fs::write(&copy, "print('otro')").unwrap();
        c.model.command = cmd(&["python3", "/movido/adapters/models/openai_compat.py"]);
        assert_eq!(
            c.model_command(&dirs).unwrap()[1],
            copy.display().to_string()
        );
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), ADAPTER_SOURCE);
        // The person's own command is left as it is.
        c.model.command = cmd(&["python3", "/home/ana/mi_adaptador.py"]);
        assert_eq!(c.model_command(&dirs).unwrap(), c.model.command);
        // Only the adapter inside this binary is handed the key.
        assert!(c.model_key(&dirs, &c.model.command).unwrap().is_none());
        // The anthropic-api connector runs its own adapter, written beside the other.
        c.model.command = Vec::new();
        c.model.provider = "anthropic".into();
        let api = dirs.data.join("adapters/anthropic_api.py");
        assert_eq!(
            c.model_command(&dirs).unwrap(),
            cmd(&["python3", &api.display().to_string()])
        );
        assert_eq!(std::fs::read_to_string(&api).unwrap(), API_ADAPTER_SOURCE);
        assert_eq!(mode(&api), Some(0o600));
    }

    #[test]
    fn the_claude_adapter_gets_the_claude_code_of_the_installed_pi() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: t.path().join("config"),
            data: t.path().join("data"),
            runs: t.path().join("data/runs"),
            runtime: None,
            home: t.path().to_path_buf(),
        };
        let mut c = Config::default_with(Vec::new());
        c.model.provider = "claude".into();
        // No Pi: the message says how to get it.
        assert_eq!(
            c.model_command(&dirs).unwrap_err(),
            "Claude Code (pi-claude-agent-sdk) is missing: clio pi install"
        );
        // A Pi of the pinned version, with Claude Code for this machine.
        let v = clio_brain::pi::install::pinned_version().unwrap();
        let nm = super::super::pi::paths(&dirs)
            .install_dir(&v)
            .join("node_modules");
        let put = |rel: &str, body: &str, m: u32| {
            let p = nm.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(m)).unwrap();
            p
        };
        put(".bin/pi", "", 0o700);
        put(
            "@earendil-works/pi-coding-agent/package.json",
            &format!(r#"{{"version":"{v}"}}"#),
            0o600,
        );
        assert!(c.model_command(&dirs).is_err(), "Pi without Claude Code");
        // Not executable is not there; the musl one is taken when it is the one that runs.
        put("@anthropic-ai/claude-agent-sdk-linux-x64/claude", "", 0o600);
        let musl = put(
            "@anthropic-ai/claude-agent-sdk-linux-x64-musl/claude",
            "",
            0o700,
        );
        let copy = dirs.data.join("adapters/claude_code.py");
        let want = |bin: &std::path::Path| {
            cmd(&[
                "python3",
                &copy.display().to_string(),
                "--claude",
                &bin.display().to_string(),
            ])
        };
        let got = c.model_command(&dirs).unwrap();
        assert_eq!(got, want(&musl));
        assert_eq!(
            std::fs::read_to_string(&copy).unwrap(),
            CLAUDE_ADAPTER_SOURCE
        );
        // The resolved command is still the built-in adapter: it gets the key.
        assert!(names_builtin_adapter(&got));
        let x64 = put("@anthropic-ai/claude-agent-sdk-linux-x64/claude", "", 0o700);
        assert_eq!(c.model_command(&dirs).unwrap(), want(&x64));
    }
}
