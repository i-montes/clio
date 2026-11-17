//! The one way Clio starts Pi (S-16, S-42): the same hardened flags and clean environment every
//! time, `models.json` written from Clio's configuration without the API key inside, and a
//! check at startup that fails closed.
//!
//! Flags, always: `--mode rpc --no-builtin-tools --no-extensions` (plus `-e <guard>` when given,
//! and `-e <provider's extension>` for a provider an extension registers)
//! `--no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve
//! --offline --provider P --model M` and the session (`--session-id`, `--session`, `--fork`),
//! `--name`. Environment, only: `PATH` (Node's folder + `/usr/bin:/bin`), `HOME` (Clio's own
//! folder for Pi), `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, `PI_OFFLINE=1`,
//! `PI_TELEMETRY=0`, `PI_SKIP_VERSION_CHECK=1`, `NO_COLOR=1`, `CLIO_SESSION_ID`, the API key
//! under its variable name, and the provider's `child_env`. The key is never written to disk nor put on the command line.

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
    /// Price in US dollars per million tokens, `(input, output)`: Pi computes the cost from it.
    pub dollars_per_mtok: (f64, f64),
    /// A Pi extension that registers this provider, relative to the Pi install folder; then
    /// `models.json` does not declare it.
    pub extension: Option<String>,
    /// Extra variables for Pi's environment (what the extension's child processes need).
    pub child_env: Vec<(String, String)>,
}

impl Provider {
    /// The `models.json` content. The key appears only as a reference to its variable.
    pub fn models_json(&self) -> Value {
        if self.extension.is_some() {
            return json!({"providers": {}});
        }
        json!({"providers": {self.id.clone(): {
            "baseUrl": self.base_url,
            "api": self.api,
            "apiKey": format!("${{{}}}", self.api_key_env),
            "headers": self.headers,
            "models": [{"id": self.model, "name": self.model, "reasoning": self.reasoning, "contextWindow": self.context_window,
                "cost": {"input": self.dollars_per_mtok.0, "output": self.dollars_per_mtok.1, "cacheRead": 0, "cacheWrite": 0}}],
        }}})
    }
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
    /// Extra variables for the guard (`CLIO_GUARD_PROMPT_*`, `CLIO_TOOLHOST_SOCKET`). Empty
    /// without a guard.
    pub extra_env: Vec<(String, String)>,
    /// The `clio` binary: inside Pi's jail it runs `clio pi-net`, the forwarder to the egress
    /// proxy (ADR 0009). It must be a binary that has that command.
    pub helper: PathBuf,
}

/// A session or run id that is safe to use as one path component.
pub fn valid_id(id: &str) -> bool {
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
    if let Some(x) = &l.provider.extension {
        a.extend(["-e".into(), l.install.dir.join(x).into_os_string()]);
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
        // A reasoning model thinks for thousands of tokens on a simple step at Pi's default
        // ("medium"): measured 46 s against 7 s at "low" for the same answer.
        "--thinking".into(),
        "low".into(),
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
    e.extend(l.provider.child_env.iter().cloned());
    e.extend(l.extra_env.iter().cloned());
    e
}

/// A running, verified Pi.
pub struct PiProcess {
    pub client: RpcClient,
    pub session_id: String,
    pub state: State,
    /// Pi's jail, with the egress proxy. After `client` on purpose: Pi is stopped first.
    pub jail: Option<super::jail::PiJail>,
}

/// Pi ready to spawn: the command (inside the jail when there is one), the jail that must outlive
/// the process, and the session id.
pub struct Built {
    pub cmd: Command,
    pub jail: Option<super::jail::PiJail>,
    pub session_id: String,
}

/// Everything `start` does before it spawns Pi. Public so the tests that attack the whole stack
/// launch Pi exactly as the product does (same flags, environment and jail) and not with a
/// command line of their own.
pub fn build(l: &Launch) -> Result<Built, PiError> {
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
    if let Some(x) = &l.provider.extension
        && !l.install.dir.join(x).is_file()
    {
        return Err(PiError::NotInstalled(
            "falta el puente de Claude (pi-claude-agent-sdk): clio pi install".into(),
        ));
    }
    let r = resolve(l)?;
    l.paths.ensure()?;
    write_private(
        &l.paths.agent_dir().join("models.json"),
        serde_json::to_string_pretty(&l.provider.models_json())
            .unwrap_or_default()
            .as_bytes(),
    )?;
    if l.provider.extension.is_some() {
        reset_claude_settings(&l.paths)?;
    }
    // The jail first (ADR 0009): if it cannot be built or fails its self-test, Pi does not start.
    let jail = super::jail::prepare(l)?;
    // `umask 077` so the session files Pi creates are born 0600; `exec` keeps the pid; `env -u`
    // drops the variables the shell itself adds (PWD, SHLVL, _).
    let mut inner: Vec<OsString> = vec![
        "/bin/sh".into(),
        "-c".into(),
        "umask 077; exec env -u PWD -u OLDPWD -u SHLVL -u _ \"$0\" \"$@\"".into(),
        l.install.bin.clone().into_os_string(),
    ];
    inner.extend(hardened_args(l, &r.args));
    let cmd = match &jail {
        Some(j) => j.command(l, &r.session_id, &inner),
        None => {
            let mut c = Command::new(&inner[0]);
            c.args(&inner[1..])
                .env_clear()
                .envs(hardened_env(l, &r.session_id))
                .current_dir(&l.cwd);
            c
        }
    };
    Ok(Built {
        cmd,
        jail,
        session_id: r.session_id,
    })
}

/// Claude Code's managed settings inside the jail (ADR 0009): no source can widen them, so no hook
/// runs, whoever wrote it (the working copy, a user setting, a plugin), and no instructions file
/// of the copy reaches the model as Claude Code's own: `CLAUDE.local.md` arrives as "private
/// instructions" that "OVERRIDE any default behavior" (the bridge excludes only the other two).
pub const CLAUDE_POLICY: &str = r#"{"disableAllHooks": true, "allowManagedHooksOnly": true, "disableSkillShellExecution": true, "claudeMdExcludes": ["**/CLAUDE.md", "**/CLAUDE.local.md", "**/.claude/rules/**"]}"#;

/// What Claude Code loads from its user folder (`~/.claude`) besides the settings: instructions
/// (`CLAUDE.md`, `rules/`), skills, subagents, commands, hooks' scripts, output styles, plugins.
/// None is Clio's. `projects/` stays: the bridge resumes its sessions from there.
const CLAUDE_USER_EXTRAS: [&str; 8] = [
    "CLAUDE.md",
    "rules",
    "skills",
    "agents",
    "commands",
    "hooks",
    "output-styles",
    "plugins",
];

/// Before every bridge launch: the policy written, and the settings Claude Code and the bridge
/// read from Pi's own folders (writable inside the jail, so a past session could have planted
/// something there: hooks, `env`, `apiKeyHelper`, another Claude Code) back to `{}`, and Claude
/// Code's other user customizations removed. A link in their place is removed, never followed.
pub fn reset_claude_settings(paths: &PiPaths) -> Result<(), PiError> {
    paths.ensure()?;
    write_private(&paths.claude_policy(), CLAUDE_POLICY.as_bytes())?;
    let dir = paths.home_dir().join(".claude");
    if std::fs::symlink_metadata(&dir).is_ok_and(|m| !m.is_dir()) {
        std::fs::remove_file(&dir).map_err(|e| super::io_err(&dir, &e))?;
    }
    super::private_dir(&dir)?;
    for f in [
        dir.join("settings.json"),
        dir.join("settings.local.json"),
        paths.agent_dir().join("claude-bridge.json"),
    ] {
        match std::fs::remove_file(&f) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(super::io_err(&f, &e));
            }
            _ => write_private(&f, b"{}")?,
        }
    }
    for name in CLAUDE_USER_EXTRAS {
        let p = dir.join(name);
        // `symlink_metadata`: a link is not a folder, so the link goes and not what it points at.
        let gone = match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
            Ok(_) => std::fs::remove_file(&p),
            Err(_) => continue,
        };
        gone.map_err(|e| super::io_err(&p, &e))?;
    }
    Ok(())
}

/// Starts Pi hardened and verifies it. On any failure the process is killed and nothing is left.
pub fn start(l: &Launch) -> Result<PiProcess, PiError> {
    let Built {
        mut cmd,
        jail,
        session_id,
    } = build(l)?;
    let client = RpcClient::spawn(&mut cmd)?;
    client.set_allowed_tools(
        l.guard_tools.clone(),
        matches!(
            l.session,
            SessionTarget::New { .. } | SessionTarget::Fork(_)
        ),
    );
    let state = verify_startup(&client, l, &session_id)?; // on Err, `client` drops and kills
    Ok(PiProcess {
        client,
        session_id,
        state,
        jail,
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
            provider: crate::connector::by_name("opencode")
                .unwrap()
                .pi_launch_provider("beam-free"),
            api_key: Some(Secret::new("decoy-key")),
            cwd: "/proj".into(),
            session: SessionTarget::New { id: None },
            name: Some("n".into()),
            guard: guard.map(Into::into),
            guard_tools: vec![],
            extra_env: vec![],
            helper: "/p/clio".into(),
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
            "--thinking",
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
        let t = crate::connector::by_name("opencode")
            .unwrap()
            .pi_launch_provider("beam-free")
            .models_json()
            .to_string();
        assert!(t.contains("${OPENCODE_API_KEY}") && t.contains("${CLIO_SESSION_ID}"));
        assert!(!t.contains("decoy-key") && t.contains("beam-free"));
        assert_eq!(
            format!("{:?}", Secret::new("decoy-key")),
            "Secret(<oculta>)"
        );
    }

    fn claude(dir: &str) -> Launch {
        let mut l = launch(Some("/g/guard.ts"));
        l.install.dir = dir.into();
        l.provider = crate::connector::by_name("claude")
            .unwrap()
            .pi_launch_provider("claude-sonnet-5-5");
        l.api_key = Some(Secret::new("decoy-oat-token"));
        l
    }

    #[test]
    fn claude_runs_through_the_bridge() {
        let l = claude("/p/1.4.0");
        let a: Vec<String> = hardened_args(&l, &[])
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let g = a.iter().position(|x| x == "/g/guard.ts").unwrap();
        assert_eq!(
            a[g + 1..g + 3],
            [
                "-e",
                "/p/1.4.0/node_modules/pi-claude-agent-sdk/src/index.ts"
            ]
        );
        let p = a.iter().position(|x| x == "--provider").unwrap();
        assert_eq!(a[p + 1], "claude-bridge");
        assert!(!a.join(" ").contains("decoy-oat-token"));

        let e = hardened_env(&l, "sid");
        for want in [
            ("ANTHROPIC_OAUTH_TOKEN", "decoy-oat-token"),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
            ("DISABLE_AUTOUPDATER", "1"),
            ("DISABLE_TELEMETRY", "1"),
        ] {
            assert!(e.contains(&(want.0.into(), want.1.into())), "{want:?}");
        }
        assert!(!e.iter().any(|(k, _)| k == "CLAUDE_CODE_OAUTH_TOKEN"));

        let t = l.provider.models_json();
        assert_eq!(t, json!({"providers": {}}));
    }

    #[test]
    fn the_bridge_must_be_installed() {
        let d = tempfile::tempdir().unwrap();
        let mut l = claude(&d.path().display().to_string());
        l.guard = None;
        l.cwd = d.path().into();
        l.paths.root = d.path().join("pi");
        let Err(e) = build(&l) else {
            panic!("build started without the bridge")
        };
        assert_eq!(
            e.to_string(),
            "falta el puente de Claude (pi-claude-agent-sdk): clio pi install"
        );
    }

    // What a hook of a past session could have left in Pi's own (writable) folders does not
    // reach the next one; a link in place of `.claude` is removed, its target left alone.
    #[test]
    fn every_bridge_launch_starts_from_empty_claude_settings_and_the_policy() {
        let d = tempfile::tempdir().unwrap();
        let paths = PiPaths {
            root: d.path().join("pi"),
        };
        paths.ensure().unwrap();
        let cc = paths.home_dir().join(".claude");
        std::fs::create_dir_all(&cc).unwrap();
        let planted = r#"{"hooks":{"SessionStart":[]},"apiKeyHelper":"/x"}"#;
        std::fs::write(cc.join("settings.json"), planted).unwrap();
        std::fs::write(paths.agent_dir().join("claude-bridge.json"), planted).unwrap();
        // Every other user customization, as a folder, a file or a link (the link goes, its
        // target stays); the bridge's sessions stay.
        let theirs = d.path().join("their-skills");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("SKILL.md"), "mine").unwrap();
        for name in CLAUDE_USER_EXTRAS {
            let p = cc.join(name);
            match name {
                "CLAUDE.md" => std::fs::write(&p, "planted").unwrap(),
                "skills" => std::os::unix::fs::symlink(&theirs, &p).unwrap(),
                _ => {
                    std::fs::create_dir_all(p.join("deep")).unwrap();
                    std::fs::write(p.join("deep/x.md"), "planted").unwrap();
                }
            }
        }
        std::fs::create_dir_all(cc.join("projects/p")).unwrap();
        std::fs::write(cc.join("projects/p/s.jsonl"), "{}").unwrap();
        reset_claude_settings(&paths).unwrap();
        for name in CLAUDE_USER_EXTRAS {
            assert!(
                std::fs::symlink_metadata(cc.join(name)).is_err(),
                "{name} is still there"
            );
        }
        assert_eq!(
            std::fs::read_to_string(theirs.join("SKILL.md")).unwrap(),
            "mine"
        );
        assert!(cc.join("projects/p/s.jsonl").is_file());
        for f in [
            cc.join("settings.json"),
            cc.join("settings.local.json"),
            paths.agent_dir().join("claude-bridge.json"),
        ] {
            assert_eq!(
                std::fs::read_to_string(&f).unwrap(),
                "{}",
                "{}",
                f.display()
            );
        }
        let policy: Value =
            serde_json::from_str(&std::fs::read_to_string(paths.claude_policy()).unwrap()).unwrap();
        assert_eq!(policy["disableAllHooks"], true);
        assert_eq!(policy["allowManagedHooksOnly"], true);
        assert!(
            policy["claudeMdExcludes"]
                .as_array()
                .unwrap()
                .contains(&json!("**/CLAUDE.local.md"))
        );
        // `.claude` as a link to the person's own folder: the link goes, their settings stay.
        let theirs = d.path().join("theirs");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("settings.json"), "mine").unwrap();
        std::fs::remove_dir_all(&cc).unwrap();
        std::os::unix::fs::symlink(&theirs, &cc).unwrap();
        reset_claude_settings(&paths).unwrap();
        assert!(!std::fs::symlink_metadata(&cc).unwrap().is_symlink());
        assert_eq!(
            std::fs::read_to_string(theirs.join("settings.json")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn anthropic_api_is_a_plain_provider() {
        let t = crate::connector::by_name("anthropic-api")
            .unwrap()
            .pi_launch_provider("claude-sonnet-5-5")
            .models_json();
        let a = &t["providers"]["anthropic"];
        assert_eq!(a["api"], "anthropic-messages");
        assert_eq!(a["apiKey"], "${ANTHROPIC_API_KEY}");
        assert_eq!(a["headers"], json!({}));
        assert_eq!(t["providers"].as_object().unwrap().len(), 1);
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
