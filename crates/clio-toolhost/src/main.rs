//! `clio-toolhost --socket-dir D --work W --scratch S --run-dir R [--rules FILE] [--jail auto|off]
//! [--vault-socket P]`
//!
//! Prints `READY <socket path>` on stdout once it listens, then runs until its stdin closes (the
//! process that started it is gone or asked it to stop), so it can not outlive its parent.
//! `CLIO_TOOLHOST_SCRUB_JSON` (`{"name":"value"}`) teaches the output filter extra values.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use clio_core::SystemClock;
use clio_toolhost::{Config, JailMode, Server, ToolHost};
use clio_vault::{Scrubber, VaultClient};

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (mut dir, mut work, mut scratch, mut run_dir) = (None, None, None, None);
    let (mut rules, mut vault, mut jail) = (None, None, JailMode::Auto);
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--socket-dir" => dir = Some(PathBuf::from(value()?)),
            "--work" => work = Some(PathBuf::from(value()?)),
            "--scratch" => scratch = Some(PathBuf::from(value()?)),
            "--run-dir" => run_dir = Some(PathBuf::from(value()?)),
            "--rules" => rules = Some(PathBuf::from(value()?)),
            "--vault-socket" => vault = Some(PathBuf::from(value()?)),
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
    match host.run_tests_available() {
        Ok(()) => {
            eprintln!("clio-toolhost: run_tests is available (the jail passed its self-test)")
        }
        Err(why) => eprintln!("clio-toolhost: run_tests is NOT offered: {why}"),
    }
    let server = Server::start(host, &dir)?;
    println!("READY {}", server.path().display());
    // Runs until the parent closes our stdin or dies.
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
    drop(server);
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("clio-toolhost: {e}");
        std::process::exit(2);
    }
}
