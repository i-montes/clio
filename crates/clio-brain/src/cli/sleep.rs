//! `clio memory sleep`: the memory's night (clio-memory's sleep) plus the merge with the person's
//! own model, whatever `config.model` names. Without a model, a key or the vault, it still sleeps
//! and only that step is skipped. `clio memory setup sleep` installs the timer that runs it.

use std::sync::Arc;
use std::time::Duration;

use clio_brain::memory_judge::MemoryJudge;
use clio_brain::memory_link::{EmbedModel, Res};
use clio_brain::sleep_merger::{ModelMerger, SLEEP_TOKENS};
use clio_memory::sleep::{Options, Slept, last_report, morning_line, run as sleep_run};
use clio_memory::{Clock, SystemClock};
use clio_vault::VaultClient;

use super::home::{Config, Dirs};
use super::{debug, jev, vault};

/// The model step, when it can be had: a configured model and the vault to clean what it is sent.
fn merger(dirs: &Dirs) -> Option<ModelMerger> {
    let cfg = Config::load(dirs).ok()?;
    let socket = vault::gateway_socket(dirs).ok()?;
    let client = VaultClient::new(&socket).with_timeout(Duration::from_secs(2));
    // The vault's cleaning, then the model's own key (it lives outside the vault).
    let key = debug::scrubber(dirs);
    let scrub: clio_brain::pi::preprompt::Scrub = Arc::new(move |t| {
        client
            .scrub(t)
            .map(|c| key.scrub(&c))
            .map_err(|e| e.to_string())
    });
    let judge = judge_for(cfg.jev.enabled && cfg.jev.memory, || {
        jev::client(dirs, &cfg.jev)
    })?
    .map(|j| Arc::new(MemoryJudge::new(j, scrub.clone())));
    ModelMerger::from_command(
        &cfg.model.command,
        &cfg.model.model,
        judge,
        scrub,
        SLEEP_TOKENS,
    )
}

/// Jev for the merges: `Some(None)` when it is off, `Some(Some(_))` when it is on and there, and
/// `None` when it is on but out of reach: a doubt, so no merges that night.
fn judge_for<J>(on: bool, client: impl FnOnce() -> Option<J>) -> Option<Option<J>> {
    if !on {
        return Some(None);
    }
    client().map(Some)
}

pub fn run(dirs: &Dirs, now: bool, dry_run: bool, report: bool, json: bool) -> Res<i32> {
    if report {
        match last_report(&dirs.data) {
            Some(r) => println!(
                "{}",
                serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?
            ),
            None => println!("sueño: todavía no durmió"),
        }
        return Ok(0);
    }
    let merger = if dry_run { None } else { merger(dirs) };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let out = sleep_run(
        &dirs.data,
        &EmbedModel::of_home(&dirs.home),
        merger
            .as_ref()
            .map(|m| m as &dyn clio_memory::sleep::Merger),
        &Options {
            now,
            dry_run,
            clock: clock.clone(),
        },
    )?;
    let r = match out {
        Slept::Skipped(why) => {
            println!("no durmió: {why}");
            return Ok(0);
        }
        Slept::Done(r) => r,
    };
    if json {
        println!("{}", serde_json::to_string(&r).map_err(|e| e.to_string())?);
        return Ok(0);
    }
    if dry_run {
        println!("solo mirar: no se cambió nada");
    }
    if merger.is_none() && !dry_run {
        println!("sin modelo para unir esta noche (falta el modelo configurado o el baúl)");
    }
    println!(
        "{}",
        morning_line(&r, clock.now()).map_or_else(
            || "nada que hacer".to_owned(),
            |l| l.replacen("Anoche (sueño)", "sueño", 1)
        )
    );
    if !r.complete {
        println!("la noche quedó incompleta: el próximo turno la sigue");
    }
    Ok(0)
}

/// Installs (or removes) the timer, pointing at this `clio`.
pub fn setup(dirs: &Dirs, remove: bool) -> Res<i32> {
    use clio_memory::setup::{install_sleep, remove_sleep, service_command};
    if remove {
        remove_sleep(&dirs.home, &service_command)?;
        println!("sueño nocturno quitado");
        return Ok(0);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let at = install_sleep(&dirs.home, &exe, &["memory", "sleep"], &service_command)?;
    println!("sueño nocturno instalado: {}", at.display());
    Ok(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn jev_on_but_out_of_reach_means_no_merges() {
        // Off: merges without it. On and there: with it. On and not there: a doubt, no merges.
        assert!(matches!(judge_for(false, || Some(1)), Some(None)));
        assert!(matches!(judge_for(true, || Some(1)), Some(Some(1))));
        assert!(judge_for::<u8>(true, || None).is_none());
    }
}
