//! The command line of the tool host, shared by the `clio-toolhost` binary and by `clio toolhost`
//! (the same process, started by the TUI from its own executable).
//!
//! `--socket-dir D --work W --scratch S --run-dir R [--rules FILE] [--jail auto|off]
//! [--vault-socket P] [--memory-data D [--memory-model M]...]`. Prints `READY <socket path>` on stdout once it listens, then runs until
//! its stdin closes (the process that started it is gone or asked it to stop), so it can not
//! outlive its parent. `CLIO_TOOLHOST_SCRUB_JSON` (`{"name":"value"}`) teaches the output filter
//! extra values. With `--memory-data` the memory tools are offered, over what the binary's
//! [`MemoryFactory`] makes of the two folders (the loose `clio-toolhost` has none). The vector
//! models' folders come from the parent, best first: this process runs with another `HOME`.
//!
//! `request_secret`: `--secret-state S --secret-run ID --vault-person P` (where requests wait, the
//! run, the vault's person socket); without them nobody can be asked for a secret.
//!
//! `--review`: the service of a review, which cannot ask the person (see `Config::review`).
//!
//! `--skills D`: Clio's skills folder (`skill_write`, `skill_report`, reading a registered skill).
//!
//! `--show D` (host mode, repeatable): a folder inside Clio's masked ones that commands see
//! read-only (a skill draft's stage, for its reviewer).
//!
//! Host mode: `--host-home H --host-state S --host-run ID [--host-control P]... [--host-rules
//! FILE]` (the person's `host.json`). `--work` is then not needed: the tools work on `/`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clio_core::SystemClock;
use clio_vault::{Scrubber, VaultClient};

use crate::ask::HostRules;
use crate::{Config, HostCfg, JailMode, MemoryPort, Server, ToolHost};

/// Builds the memory from its data folder and the vector models' folders (best first; none: text).
pub type MemoryFactory<'a> = &'a dyn Fn(&Path, &[PathBuf]) -> Result<Arc<dyn MemoryPort>, String>;

pub fn run(args: Vec<String>) -> Result<(), String> {
    run_with(args, &|_, _| Err("this binary has no memory".into()))
}

pub fn run_with(args: Vec<String>, memory: MemoryFactory<'_>) -> Result<(), String> {
    let mut args = args.into_iter();
    let (mut dir, mut work, mut scratch, mut run_dir) = (None, None, None, None);
    let (mut rules, mut vault, mut jail) = (None, None, JailMode::Auto);
    let (mut memory_data, mut memory_model) = (None, Vec::new());
    let (mut host_home, mut host_state, mut host_run) = (None, None, None);
    let (mut host_control, mut host_rules) = (Vec::new(), None);
    let (mut secret_state, mut secret_run, mut vault_person) = (None, None, None);
    let mut review = false;
    let mut skills = None;
    let mut shows = Vec::new();
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--review" => review = true,
            "--skills" => skills = Some(PathBuf::from(value()?)),
            "--show" => shows.push(PathBuf::from(value()?)),
            "--socket-dir" => dir = Some(PathBuf::from(value()?)),
            "--work" => work = Some(PathBuf::from(value()?)),
            "--scratch" => scratch = Some(PathBuf::from(value()?)),
            "--run-dir" => run_dir = Some(PathBuf::from(value()?)),
            "--rules" => rules = Some(PathBuf::from(value()?)),
            "--vault-socket" => vault = Some(PathBuf::from(value()?)),
            "--memory-data" => memory_data = Some(PathBuf::from(value()?)),
            "--memory-model" => memory_model.push(PathBuf::from(value()?)),
            "--host-home" => host_home = Some(PathBuf::from(value()?)),
            "--host-state" => host_state = Some(PathBuf::from(value()?)),
            "--host-control" => host_control.push(PathBuf::from(value()?)),
            "--host-rules" => host_rules = Some(PathBuf::from(value()?)),
            "--host-run" => host_run = Some(value()?),
            "--secret-state" => secret_state = Some(PathBuf::from(value()?)),
            "--secret-run" => secret_run = Some(value()?),
            "--vault-person" => vault_person = Some(PathBuf::from(value()?)),
            "--jail" => {
                jail = match value()?.as_str() {
                    "auto" => JailMode::Auto,
                    "off" => JailMode::Off,
                    other => return Err(format!("--jail must be auto or off, not {other}")),
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let need = |v: Option<PathBuf>, n: &str| v.ok_or_else(|| format!("{n} is required"));
    let dir = need(dir, "--socket-dir")?;
    let host = match host_home {
        Some(home) => {
            let mut rules = HostRules::factory();
            if let Some(p) = host_rules {
                let text =
                    std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                rules = rules.with_file(&text)?;
            }
            Some(HostCfg {
                home,
                state: need(host_state, "--host-state")?,
                control: host_control,
                rules,
                run: host_run.ok_or("--host-run is required")?,
                launcher: None,
            })
        }
        None if host_state.is_some()
            || host_run.is_some()
            || host_rules.is_some()
            || !host_control.is_empty() =>
        {
            return Err("the --host-* options need --host-home".into());
        }
        None => None,
    };
    let work = match (&host, work) {
        (Some(h), None) => h.home.clone(),
        (_, w) => need(w, "--work")?,
    };
    let mut cfg = Config::new(
        &work,
        &need(scratch, "--scratch")?,
        &need(run_dir, "--run-dir")?,
    );
    cfg.secrets = match (secret_state, secret_run, vault_person) {
        (Some(state), Some(run), Some(person)) => Some(crate::pending::SecretCfg {
            state,
            run,
            person,
            // The day's questions are the person's: the same cap as the actions set aside.
            per_day: host.as_ref().map_or_else(
                || HostRules::factory().questions_per_day(),
                |h| h.rules.questions_per_day(),
            ),
        }),
        (None, None, None) => None,
        _ => return Err("--secret-state, --secret-run and --vault-person go together".into()),
    };
    if review && cfg.secrets.is_some() {
        return Err("a review (--review) cannot ask the person for a secret".into());
    }
    if review && skills.is_some() {
        return Err("a review (--review) cannot write skills".into());
    }
    cfg.review = review;
    cfg.skills = skills;
    cfg.shows = shows;
    cfg.host = host;
    cfg.jail = jail;
    cfg.vault = vault.map(|p| VaultClient::new(&p));
    cfg.said_token = std::env::var("CLIO_SAID_TOKEN")
        .ok()
        .filter(|t| t.len() >= 16);
    match memory_data {
        Some(data) => cfg.memory = Some(memory(&data, &memory_model)?),
        None if !memory_model.is_empty() => {
            return Err("--memory-model needs --memory-data".into());
        }
        None => {}
    }
    if let Some(p) = rules {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        cfg.rules = Some(serde_json::from_str(&text).map_err(|e| format!("rules: {e}"))?);
    }
    if let Ok(json) = std::env::var("CLIO_TOOLHOST_SCRUB_JSON") {
        let values: BTreeMap<String, String> =
            serde_json::from_str(&json).map_err(|e| format!("CLIO_TOOLHOST_SCRUB_JSON: {e}"))?;
        let s = Scrubber::new(Arc::new(SystemClock));
        for (name, value) in values {
            s.add(&name, &value);
        }
        cfg.scrubber = Some(s);
    }
    let where_ = if cfg.host.is_some() {
        "bash on the real machine, Clio's folders masked; the launcher"
    } else {
        "bash in the jail; the jail"
    };
    let host = Arc::new(ToolHost::new(cfg)?);
    match host.exec_available() {
        Ok(()) => eprintln!("clio-toolhost: exec ({where_} passed its self-test) is available"),
        Err(why) => eprintln!("clio-toolhost: exec (bash) is NOT offered: {why}"),
    }
    let server = Server::start(host.clone(), &dir)?;
    println!("READY {}", server.path().display());
    // Runs until the parent closes our stdin or dies.
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
    // A command still running is stopped, and its log line written, before the process goes.
    host.stop(std::time::Duration::from_secs(2));
    drop(server);
    Ok(())
}
