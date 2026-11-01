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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: ModelCfg,
    pub limits: Limits,
    pub workspace: WorkspaceLimits,
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
        }
    }

    pub fn load(dirs: &Dirs) -> Res<Self> {
        let p = dirs.config_file();
        let text = std::fs::read_to_string(&p)
            .map_err(|e| format!("{}: {e} (ejecuta `clio init` primero)", p.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
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

/// The adapter script: `CLIO_ADAPTER`, or `adapters/models/openai_compat.py` found next to the
/// executable or in the working directory's ancestors, or else the copy installed by `init`.
fn find_adapter(dirs: &Dirs) -> Res<PathBuf> {
    let rel = Path::new("adapters/models/openai_compat.py");
    if let Some(p) = std::env::var_os("CLIO_ADAPTER") {
        return Ok(PathBuf::from(p));
    }
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        starts.extend(exe.parent().map(Path::to_path_buf));
    }
    starts.extend(std::env::current_dir().ok());
    for s in starts {
        for dir in s.ancestors() {
            let c = dir.join(rel);
            if c.is_file() {
                return c.canonicalize().map_err(|e| io_err(&c, &e));
            }
        }
    }
    let dir = dirs.data.join("adapters");
    mkdir_private(&dir)?;
    let p = dir.join("openai_compat.py");
    write_private(&p, ADAPTER_SOURCE.as_bytes())?;
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
        let adapter = find_adapter(dirs)?;
        let c = Config::default_with(vec!["python3".into(), adapter.display().to_string()]);
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
