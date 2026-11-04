//! The command line of the tool host, shared by the `clio-toolhost` binary and by `clio toolhost`
//! (the same process, started by the TUI from its own executable).
//!
//! `--socket-dir D --work W --scratch S --run-dir R [--rules FILE] [--jail auto|off]
//! [--vault-socket P] [--memory-data D [--memory-model M]]`. Prints `READY <socket path>` on stdout once it listens, then runs until
//! its stdin closes (the process that started it is gone or asked it to stop), so it can not
//! outlive its parent. `CLIO_TOOLHOST_SCRUB_JSON` (`{"name":"value"}`) teaches the output filter
//! extra values. With `--memory-data` the memory tools are offered, over what the binary's
//! [`MemoryFactory`] makes of the two folders (the loose `clio-toolhost` has none). The vector
//! model's folder comes from the parent: this process runs with another `HOME`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clio_core::SystemClock;
use clio_vault::{Scrubber, VaultClient};

use crate::{Config, JailMode, MemoryPort, Server, ToolHost};

/// Builds the memory from its data folder and, if there is one, the vector model's folder.
pub type MemoryFactory<'a> =
    &'a dyn Fn(&Path, Option<&Path>) -> Result<Arc<dyn MemoryPort>, String>;

pub fn run(args: Vec<String>) -> Result<(), String> {
    run_with(args, &|_, _| Err("this binary has no memory".into()))
}

pub fn run_with(args: Vec<String>, memory: MemoryFactory<'_>) -> Result<(), String> {
    let mut args = args.into_iter();
    let (mut dir, mut work, mut scratch, mut run_dir) = (None, None, None, None);
    let (mut rules, mut vault, mut jail) = (None, None, JailMode::Auto);
    let (mut memory_data, mut memory_model) = (None, None);
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--socket-dir" => dir = Some(PathBuf::from(value()?)),
            "--work" => work = Some(PathBuf::from(value()?)),
            "--scratch" => scratch = Some(PathBuf::from(value()?)),
            "--run-dir" => run_dir = Some(PathBuf::from(value()?)),
            "--rules" => rules = Some(PathBuf::from(value()?)),
            "--vault-socket" => vault = Some(PathBuf::from(value()?)),
            "--memory-data" => memory_data = Some(PathBuf::from(value()?)),
            "--memory-model" => memory_model = Some(PathBuf::from(value()?)),
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
    let mut cfg = Config::new(
        &need(work, "--work")?,
        &need(scratch, "--scratch")?,
        &need(run_dir, "--run-dir")?,
    );
    cfg.jail = jail;
    cfg.vault = vault.map(|p| VaultClient::new(&p));
    cfg.said_token = std::env::var("CLIO_SAID_TOKEN")
        .ok()
        .filter(|t| t.len() >= 16);
    match (memory_data, memory_model) {
        (Some(data), model) => cfg.memory = Some(memory(&data, model.as_deref())?),
        (None, Some(_)) => return Err("--memory-model needs --memory-data".into()),
        (None, None) => {}
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
    let host = Arc::new(ToolHost::new(cfg)?);
    match host.exec_available() {
        Ok(()) => {
            eprintln!(
                "clio-toolhost: exec (bash in the jail) is available (the jail passed its self-test)"
            )
        }
        Err(why) => eprintln!("clio-toolhost: exec (bash) is NOT offered: {why}"),
    }
    let server = Server::start(host, &dir)?;
    println!("READY {}", server.path().display());
    // Runs until the parent closes our stdin or dies.
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
    drop(server);
    Ok(())
}
