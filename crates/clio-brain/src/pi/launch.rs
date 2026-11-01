//! The one way Clio starts Pi (S-16, S-42): the same hardened flags and clean environment every
//! time, `models.json` written from Clio's configuration without the API key inside, and a
//! check at startup that fails closed.
//!
//! Flags, always: `--mode rpc --no-builtin-tools --no-extensions` (plus `-e <guard>` when given)
//! `--no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve
//! --offline --provider P --model M` and the session (`--session-id`, `--session`, `--fork`),
//! `--name`. Environment, only: `PATH` (Node's folder + `/usr/bin:/bin`), `HOME` (Clio's own
//! folder for Pi), `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, `PI_OFFLINE=1`,
//! `PI_TELEMETRY=0`, `PI_SKIP_VERSION_CHECK=1`, `NO_COLOR=1`, `CLIO_SESSION_ID`, and the API key
//! under its variable name. The key is never written to disk nor put on the command line.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use super::install::{NodeInfo, PiInstall};
use super::rpc::{RpcClient, State};
use super::{PiError, PiPaths, text, write_private};

/// A secret that never prints itself.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<oculta>)")
    }
}

/// The model provider as Pi's `models.json` needs it.
#[derive(Debug, Clone)]
pub struct Provider {
    /// Key under `providers` and the value of `--provider`.
    pub id: String,
    pub base_url: String,
    /// Pi's API kind, for example `openai-completions`.
    pub api: String,
    pub model: String,
    pub context_window: u64,
    pub reasoning: bool,
    /// Header values may use `${CLIO_SESSION_ID}`.
    pub headers: BTreeMap<String, String>,
    /// Name of the environment variable that carries the API key.
    pub api_key_env: String,
}

impl Provider {
    /// The provider Clio uses by default (see the feasibility run).
    pub fn opencode_go(model: &str) -> Self {
        Self {
            id: "opencode-go".into(),
            base_url: "https://opencode.ai/inference/go/openai/v1".into(),
            api: "openai-completions".into(),
            model: model.into(),
            context_window: 128_000,
            reasoning: true,
            headers: BTreeMap::from([
                ("User-Agent".to_owned(), "clio/0.1".to_owned()),
                (
                    "x-opencode-session".to_owned(),
                    "${CLIO_SESSION_ID}".to_owned(),
                ),
            ]),
            api_key_env: "OPENCODE_API_KEY".into(),
        }
    }

    /// The `models.json` content. The key appears only as a reference to its variable.
    pub fn models_json(&self) -> Value {
        json!({"providers": {self.id.clone(): {
            "baseUrl": self.base_url,
            "api": self.api,
            "apiKey": format!("${{{}}}", self.api_key_env),
            "headers": self.headers,
            "models": [{"id": self.model, "name": self.model, "reasoning": self.reasoning, "contextWindow": self.context_window}],
        }}})
    }
}

/// The API key: the variable if set and not empty, else the first line of the file.
pub fn read_api_key(env_var: &str, key_file: &Path) -> Option<Secret> {
    let from_env = std::env::var(env_var).ok().filter(|v| !v.trim().is_empty());
    let v = from_env.or_else(|| std::fs::read_to_string(key_file).ok())?;
    let k = v.lines().next().unwrap_or("").trim().to_owned();
    (!k.is_empty()).then(|| Secret::new(k))
}

#[derive(Debug, Clone)]
pub enum SessionTarget {
    /// A new session; the id is generated unless given (letters, digits, `.`, `_`, `-`).
    New { id: Option<String> },
    /// An existing one: a `.jsonl` path inside Pi's sessions folder, or an id.
    Open(String),
    /// A new session forked from an existing one (same kinds of reference).
    Fork(String),
}

#[derive(Debug, Clone)]
pub struct Launch {
    pub install: PiInstall,
    pub node: NodeInfo,
    pub paths: PiPaths,
    pub provider: Provider,
    pub api_key: Option<Secret>,
    /// The session's project folder (Pi's working directory).
    pub cwd: PathBuf,
    pub session: SessionTarget,
    pub name: Option<String>,
    /// The guard extension, as an absolute path.
    pub guard: Option<PathBuf>,
    /// EXACTLY the tools the guard registers (empty without a guard: Pi must have none).
    pub guard_tools: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// 16 random bytes as hex, from the kernel.
fn random_id() -> Result<String, PiError> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| PiError::Io(format!("/dev/urandom: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// What Pi is told to open, and the id that goes in `CLIO_SESSION_ID`.
struct Resolved {
    args: Vec<OsString>,
    session_id: String,
}

fn resolve_ref(paths: &PiPaths, r: &str) -> Result<(OsString, String), PiError> {
    if r.contains('/') {
        let p = Path::new(r)
            .canonicalize()
            .map_err(|e| PiError::Io(format!("{r}: {e}")))?;
        let root = paths
            .sessions_dir()
            .canonicalize()
            .map_err(|e| PiError::Io(format!("sesiones de Pi: {e}")))?;
        if !p.starts_with(&root) || p.extension().is_none_or(|x| x != "jsonl") {
            return Err(PiError::Verification(format!(
                "la sesión {} no está en la carpeta de sesiones de Pi",
                text::line(r, 120)
            )));
        }
        let id = p
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit_once('_').map(|(_, id)| id.to_owned()))
            .filter(|id| valid_id(id))
            .ok_or_else(|| PiError::Io("el nombre del archivo de sesión no trae un id".into()))?;
        Ok((p.into_os_string(), id))
    } else if valid_id(r) {
        Ok((r.into(), r.to_owned()))
    } else {
        Err(PiError::Io(format!(
            "referencia de sesión inválida: {}",
            text::line(r, 60)
        )))
    }
}

fn resolve(l: &Launch) -> Result<Resolved, PiError> {
    let mut args: Vec<OsString> = Vec::new();
    let session_id = match &l.session {
        SessionTarget::New { id } => {
            let id = match id {
                Some(i) if valid_id(i) => i.clone(),
                Some(i) => {
                    return Err(PiError::Io(format!(
                        "id de sesión inválido: {}",
                        text::line(i, 60)
                    )));
                }
                None => random_id()?,
            };
            args.extend(["--session-id".into(), id.clone().into()]);
            id
        }
        SessionTarget::Open(r) => {
            let (arg, id) = resolve_ref(&l.paths, r)?;
            args.extend(["--session".into(), arg]);
            id
        }
        SessionTarget::Fork(r) => {
            let (arg, _) = resolve_ref(&l.paths, r)?;
            let id = random_id()?;
            args.extend([
                "--fork".into(),
                arg,
                "--session-id".into(),
                id.clone().into(),
            ]);
            id
        }
    };
    Ok(Resolved { args, session_id })
}

/// The arguments after the program name. Pure, so the tests can pin them.
pub fn hardened_args(l: &Launch, session_args: &[OsString]) -> Vec<OsString> {
    let mut a: Vec<OsString> = ["--mode", "rpc", "--no-builtin-tools", "--no-extensions"]
        .map(Into::into)
        .into();
    if let Some(g) = &l.guard {
        a.extend(["-e".into(), g.clone().into_os_string()]);
    }
    a.extend(
        [
            "--no-mcp",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
            "--no-context-files",
            "--no-approve",
            "--offline",
        ]
        .map(OsString::from),
    );
    a.extend([
        "--provider".into(),
        l.provider.id.clone().into(),
        "--model".into(),
        l.provider.model.clone().into(),
    ]);
    a.extend(session_args.iter().cloned());
    if let Some(n) = &l.name {
        a.extend(["--name".into(), n.clone().into()]);
    }
    a
}

/// The whole environment of the process, and nothing else.
pub fn hardened_env(l: &Launch, session_id: &str) -> Vec<(String, String)> {
    let mut e = vec![
        (
            "PATH".to_owned(),
            format!("{}:/usr/bin:/bin", l.node.dir().display()),
        ),
        ("HOME".to_owned(), l.paths.home_dir().display().to_string()),
        (
            "PI_CODING_AGENT_DIR".to_owned(),
            l.paths.agent_dir().display().to_string(),
        ),
        (
            "PI_CODING_AGENT_SESSION_DIR".to_owned(),
            l.paths.sessions_dir().display().to_string(),
        ),
        ("PI_OFFLINE".to_owned(), "1".to_owned()),
        ("PI_TELEMETRY".to_owned(), "0".to_owned()),
        ("PI_SKIP_VERSION_CHECK".to_owned(), "1".to_owned()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("CLIO_SESSION_ID".to_owned(), session_id.to_owned()),
    ];
    if let Some(k) = &l.api_key {
        e.push((l.provider.api_key_env.clone(), k.expose().to_owned()));
    }
    e
}

/// A running, verified Pi.
pub struct PiProcess {
    pub client: RpcClient,
    pub session_id: String,
    pub state: State,
}

/// Starts Pi hardened and verifies it. On any failure the process is killed and nothing is left.
pub fn start(l: &Launch) -> Result<PiProcess, PiError> {
    if !l.cwd.is_absolute() || !l.cwd.is_dir() {
        return Err(PiError::Io(format!(
            "la carpeta del proyecto {} no existe",
            text::line(&l.cwd.display().to_string(), 120)
        )));
    }
    if let Some(g) = &l.guard
        && !(g.is_absolute() && g.is_file())
    {
        return Err(PiError::NotInstalled(format!(
            "no encuentro la extensión de guardia {}",
            text::line(&g.display().to_string(), 120)
        )));
    }
    let r = resolve(l)?;
    l.paths.ensure()?;
    write_private(
        &l.paths.agent_dir().join("models.json"),
        serde_json::to_string_pretty(&l.provider.models_json())
            .unwrap_or_default()
            .as_bytes(),
    )?;
    // `umask 077` so the session files Pi creates are born 0600; `exec` keeps the pid; `env -u`
    // drops the variables the shell itself adds (PWD, SHLVL, _).
    let mut cmd = Command::new("/bin/sh");
    cmd.args([
        "-c",
        "umask 077; exec env -u PWD -u OLDPWD -u SHLVL -u _ \"$0\" \"$@\"",
    ])
    .arg(&l.install.bin)
    .args(hardened_args(l, &r.args))
    .env_clear()
    .envs(hardened_env(l, &r.session_id))
    .current_dir(&l.cwd);
    let client = RpcClient::spawn(&mut cmd)?;
    client.set_allowed_tools(
        l.guard_tools.clone(),
        matches!(
            l.session,
            SessionTarget::New { .. } | SessionTarget::Fork(_)
        ),
    );
    let state = verify_startup(&client, l, &r.session_id)?; // on Err, `client` drops and kills
    Ok(PiProcess {
        client,
        session_id: r.session_id,
        state,
    })
}

fn same_file(a: &str, b: &Path) -> bool {
    Path::new(a).canonicalize().ok() == b.canonicalize().ok()
}

/// The checks that make a launch fail closed. Pi is killed (by the caller dropping the client)
/// if any of them fails.
pub fn verify_startup(c: &RpcClient, l: &Launch, session_id: &str) -> Result<State, PiError> {
    let st = c.get_state()?;
    let fail = |m: String| {
        c.kill();
        Err(PiError::Verification(m))
    };
    if st.provider != l.provider.id || st.model != l.provider.model {
        return fail(format!(
            "el modelo activo es {}/{}, se pidió {}/{}",
            text::line(&st.provider, 40),
            text::line(&st.model, 60),
            l.provider.id,
            l.provider.model
        ));
    }
    if st.session_id != session_id && !matches!(l.session, SessionTarget::Open(_)) {
        return fail(format!(
            "la sesión activa es {}, se pidió {session_id}",
            text::line(&st.session_id, 64)
        ));
    }
    if st.is_streaming {
        return fail("Pi arrancó ya ocupado".into());
    }
    // Extension commands, skills and prompt templates all show up here: only the guard's allowed.
    for cmd in c.get_commands()? {
        let ok = cmd["source"] == "extension"
            && match (&l.guard, cmd["sourceInfo"]["path"].as_str()) {
                (Some(g), Some(p)) => same_file(p, g),
                _ => false,
            };
        if !ok {
            return fail(format!(
                "comando inesperado /{} (origen {})",
                text::line(cmd["name"].as_str().unwrap_or("?"), 40),
                text::line(cmd["source"].as_str().unwrap_or("?"), 20)
            ));
        }
    }
    // A reopened session already carries its system messages: the tools they declare must be
    // inside the allowed list too.
    if let Err(e) = check_loadout(&c.get_messages()?, &l.guard_tools) {
        return fail(e);
    }
    Ok(st)
}

/// Every tool any system message of the session declared must be in `allowed`.
pub fn check_loadout(messages: &[Value], allowed: &[String]) -> Result<(), String> {
    for m in messages.iter().filter(|m| m["role"] == "system") {
        for t in m["toolsAdded"].as_array().into_iter().flatten() {
            let n = t["name"].as_str().unwrap_or("?");
            if !allowed.iter().any(|a| a == n) {
                return Err(format!(
                    "la sesión declara la herramienta {}, fuera de la lista permitida",
                    text::line(n, 40)
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn launch(guard: Option<&str>) -> Launch {
        Launch {
            install: PiInstall {
                version: "1.4.0".into(),
                dir: "/p/1.4.0".into(),
                bin: "/p/1.4.0/node_modules/.bin/pi".into(),
            },
            node: NodeInfo {
                path: "/n/bin/node".into(),
                version: (22, 23, 2),
            },
            paths: PiPaths {
                root: "/d/pi".into(),
            },
            provider: Provider::opencode_go("beam-free"),
            api_key: Some(Secret::new("decoy-key")),
            cwd: "/proj".into(),
            session: SessionTarget::New { id: None },
            name: Some("n".into()),
            guard: guard.map(Into::into),
            guard_tools: vec![],
        }
    }

    #[test]
    fn the_flags_are_always_there() {
        let a: Vec<String> = hardened_args(
            &launch(Some("/g/guard.ts")),
            &["--session-id".into(), "abc".into()],
        )
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
        for f in [
            "--mode",
            "rpc",
            "--no-builtin-tools",
            "--no-extensions",
            "--no-mcp",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
            "--no-context-files",
            "--no-approve",
            "--offline",
        ] {
            assert!(a.iter().any(|x| x == f), "{f}");
        }
        let i = a.iter().position(|x| x == "-e").unwrap();
        assert_eq!(a[i + 1], "/g/guard.ts");
        assert!(a.iter().position(|x| x == "--no-extensions").unwrap() < i);
        assert!(!hardened_args(&launch(None), &[]).iter().any(|x| x == "-e"));
        assert!(!a.join(" ").contains("decoy-key"));
    }

    #[test]
    fn the_environment_is_exactly_this() {
        let e = hardened_env(&launch(None), "sid");
        let mut names: Vec<&str> = e.iter().map(|(k, _)| k.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "CLIO_SESSION_ID",
                "HOME",
                "NO_COLOR",
                "OPENCODE_API_KEY",
                "PATH",
                "PI_CODING_AGENT_DIR",
                "PI_CODING_AGENT_SESSION_DIR",
                "PI_OFFLINE",
                "PI_SKIP_VERSION_CHECK",
                "PI_TELEMETRY"
            ]
        );
        assert!(e.contains(&("HOME".into(), "/d/pi/home".into())));
    }

    #[test]
    fn models_json_has_no_key_only_its_reference() {
        let t = Provider::opencode_go("beam-free").models_json().to_string();
        assert!(t.contains("${OPENCODE_API_KEY}") && t.contains("${CLIO_SESSION_ID}"));
        assert!(!t.contains("decoy-key") && t.contains("beam-free"));
        assert_eq!(
            format!("{:?}", Secret::new("decoy-key")),
            "Secret(<oculta>)"
        );
    }

    #[test]
    fn loadout_check_rejects_unlisted_tools() {
        let m = vec![json!({"role":"system","toolsAdded":[{"name":"read"},{"name":"bash"}]})];
        assert!(check_loadout(&m, &["read".into(), "bash".into()]).is_ok());
        assert!(
            check_loadout(&m, &["read".into()])
                .unwrap_err()
                .contains("bash")
        );
    }

    #[test]
    fn session_refs_are_validated() {
        assert!(valid_id("abc-1.2_x") && !valid_id("a b") && !valid_id("../x") && !valid_id(""));
    }
}
