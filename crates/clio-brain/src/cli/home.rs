//! Where Clio lives on this machine (ADR 0006), its configuration, and `clio init`.

use std::path::{Path, PathBuf};

use clio_brain::memory_link::{EmbedModel, MemoryLink};
use serde::{Deserialize, Serialize};

use super::ui::{Level, Style};
use super::{Res, io_err, mkdir_private, mode, write_private};

/// The adapter that ships with the repository; `init` installs this copy when it cannot find the
/// repository's own file.
const ADAPTER_SOURCE: &str = include_str!("../../../../adapters/models/openai_compat.py");
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
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME no está definida")?);
        if !home.is_absolute() {
            return Err("HOME debe ser una ruta absoluta".into());
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

    pub fn key_file(&self) -> PathBuf {
        self.config.join("opencode-api-key")
    }

    pub fn need_runtime(&self) -> Res<&Path> {
        self.runtime
            .as_deref()
            .ok_or_else(|| "XDG_RUNTIME_DIR no está definida (entra por una sesión normal)".into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCfg {
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
    /// `None`: the provider's usual address. Meant for a local fake model in tests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    pub max_steps: u32,
    pub max_seconds: u64,
    pub max_cost_micro_usd: u64,
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
    /// Only for tests: a loopback URL in place of `https://api.typesafe.ai`.
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
    pub model: ModelCfg,
    pub limits: Limits,
    pub workspace: WorkspaceLimits,
    #[serde(default)]
    pub jev: JevCfg,
    #[serde(default)]
    pub memory: MemoryCfg,
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

impl Config {
    pub fn default_with(command: Vec<String>) -> Self {
        Self {
            model: ModelCfg {
                provider: "opencode-go".into(),
                model: "beam-free".into(),
                command,
                prompt: DEFAULT_PROMPT_NAME.into(),
                price_in_micro_per_mtok: 0,
                price_out_micro_per_mtok: 0,
                base_url: None,
            },
            limits: Limits {
                max_steps: 40,
                max_seconds: 600,
                max_cost_micro_usd: 100_000,
            },
            workspace: WorkspaceLimits {
                max_files: 5000,
                max_bytes: 200 * 1024 * 1024,
            },
            jev: JevCfg::default(),
            memory: MemoryCfg::default(),
        }
    }

    pub fn load(dirs: &Dirs) -> Res<Self> {
        let p = dirs.config_file();
        let text = std::fs::read_to_string(&p)
            .map_err(|e| format!("{}: {e} (ejecuta `clio init` primero)", p.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// The model adapter's program and arguments as a run starts it: the adapter inside this
    /// binary (`names_builtin_adapter`) as `python3 <its copy>`, any other command as written.
    pub fn model_command(&self, dirs: &Dirs) -> Res<Vec<String>> {
        if names_builtin_adapter(&self.model.command) {
            let p = builtin_adapter(dirs)?;
            return Ok(vec!["python3".into(), p.display().to_string()]);
        }
        Ok(self.model.command.clone())
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

pub enum KeyStatus {
    Env,
    File { open_mode: Option<u32> },
    Missing,
}

pub fn key_status(dirs: &Dirs) -> KeyStatus {
    if std::env::var_os("OPENCODE_API_KEY").is_some_and(|v| !v.is_empty()) {
        return KeyStatus::Env;
    }
    match mode(&dirs.key_file()) {
        Some(m) => KeyStatus::File {
            open_mode: (m & 0o077 != 0).then_some(m),
        },
        None => KeyStatus::Missing,
    }
}

/// The file name of the adapter that travels inside this binary.
const ADAPTER_FILE: &str = "openai_compat.py";

/// Whether `command` names the adapter inside this binary: empty (what `init` writes), or
/// `python3` and an `openai_compat.py` in an `adapters/` folder, which is what older versions of
/// `init` wrote (a checkout's file, or the copy in the data folder). That path is not run: it can
/// be gone (the checkout moved) or changed, and the adapter receives the model key on every call
/// and runs outside the jail.
pub fn names_builtin_adapter(command: &[String]) -> bool {
    match command {
        [] => true,
        [python, script] if python == "python3" => {
            let p = Path::new(script);
            p.file_name().is_some_and(|n| n == ADAPTER_FILE)
                && p.ancestors()
                    .any(|a| a.file_name().is_some_and(|n| n == "adapters"))
        }
        _ => false,
    }
}

/// Where the adapter inside this binary is written: `<data>/adapters/openai_compat.py`.
pub fn builtin_adapter_path(dirs: &Dirs) -> PathBuf {
    dirs.data.join("adapters").join(ADAPTER_FILE)
}

/// The adapter inside this binary, on disk: written private, and written again when the file
/// differs (a newer Clio brings its adapter along). `CLIO_ADAPTER` puts another script in its
/// place (for working on the adapter).
fn builtin_adapter(dirs: &Dirs) -> Res<PathBuf> {
    if let Some(p) = std::env::var_os("CLIO_ADAPTER") {
        return Ok(PathBuf::from(p));
    }
    let p = builtin_adapter_path(dirs);
    if std::fs::read_to_string(&p).ok().as_deref() != Some(ADAPTER_SOURCE) {
        mkdir_private(&dirs.data.join("adapters"))?;
        write_private(&p, ADAPTER_SOURCE.as_bytes())?;
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
                "ya estaba   "
            } else {
                "creada      "
            },
            d.display()
        );
        Ok(())
    };
    made(&dirs.config)?;
    made(&dirs.data)?;
    made(&dirs.runs)?;
    made(dirs.need_runtime()?)?;

    // The memory file lives here and nowhere else; it is created now so the first run finds it.
    match MemoryLink::open_write(&dirs.data, &EmbedModel::none()) {
        Ok(link) => {
            let (a, q, _) = link.counts().unwrap_or((0, 0, 0));
            println!(
                "{ok}  memoria      {} (0600; {a} recuerdos, {q} en cuarentena)",
                MemoryLink::db_path(&dirs.data).display()
            );
        }
        Err(e) => println!("{}  memoria: {e}", st.mark(Level::Warn)),
    }
    let cfg = dirs.config_file();
    if cfg.exists() {
        println!("{ok}  ya existe    {} (se deja como está)", cfg.display());
    } else {
        // No path in the file: an empty command is the adapter inside this binary, so the file
        // keeps working when Clio is updated or moved to another machine.
        let c = Config::default_with(Vec::new());
        let text = serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?;
        write_private(&cfg, text.as_bytes())?;
        println!("{ok}  creada       {} (0600)", cfg.display());
    }
    super::pi::init_lines(dirs);
    let key = dirs.key_file();
    match key_status(dirs) {
        KeyStatus::Env => {
            println!("{ok}  clave del modelo encontrada en OPENCODE_API_KEY (no se muestra)")
        }
        KeyStatus::File { open_mode: None } => {
            println!(
                "{ok}  clave del modelo encontrada en {} (0600, no se muestra)",
                key.display()
            );
        }
        KeyStatus::File { open_mode: Some(m) } => println!(
            "{}  ADVERTENCIA: {} tiene modo {m:o} y otros pueden leerla. Ejecuta: chmod 600 {}",
            st.mark(Level::Warn),
            key.display(),
            key.display()
        ),
        KeyStatus::Missing => println!(
            "{}  falta la clave del modelo. Ponla en OPENCODE_API_KEY o en {} (chmod 600)",
            st.mark(Level::Warn),
            key.display()
        ),
    }
    println!(
        "\nsiguiente: {}, y luego {}",
        st.bold("clio doctor"),
        st.bold("clio")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
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
        // Anything else is the person's own adapter, run as written.
        for other in [
            cmd(&["python3", "/home/ana/mi_adaptador.py"]),
            cmd(&["python3", "/tmp/openai_compat.py"]),
            cmd(&[
                "python3.12",
                "/home/ana/clio/adapters/models/openai_compat.py",
            ]),
            cmd(&["python3", "/x/adapters/models/openai_compat.py", "--debug"]),
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
        let copy = builtin_adapter_path(&dirs);
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
    }
}
