//! A run: `clio run`, `diff`, `runs` and `show`. Everything of a run lives in
//! `~/.local/share/clio/runs/<id>/`:
//! `sandbox/` (the copy the agent worked on), `manifest.json` (hash of each copied file),
//! `meta.json`, `contract.json`, `effects.jsonl`, `transcript.json`, `report.json`,
//! `summary.json`, and later `backup/` and `applied.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clio_brain::agent::{AgentConfig, AgentLoop, CommandModel, Ending};
use clio_core::SystemClock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::diff::file_diff;
use super::home::{Config, Dirs};
use super::work::{self, Change, Kind};
use super::{Res, io_err, mkdir_private, now_ms, stamp, write_private};

pub struct Run {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub workspace: String,
    pub instruction: String,
    pub created_ms: i64,
}

impl Run {
    pub fn sandbox(&self) -> PathBuf {
        self.path.join("sandbox")
    }

    pub fn meta(&self) -> Res<Meta> {
        read_json(&self.path.join("meta.json"))
    }

    pub fn manifest(&self) -> Res<BTreeMap<String, String>> {
        read_json(&self.path.join("manifest.json"))
    }
}

pub fn read_json<T: for<'a> Deserialize<'a>>(p: &Path) -> Res<T> {
    let text = std::fs::read_to_string(p).map_err(|e| io_err(p, &e))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn write_json<T: Serialize>(p: &Path, v: &T) -> Res<()> {
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    write_private(p, text.as_bytes())
}

/// A run by exact id or unique prefix. The text never becomes a path: only listed names do.
pub fn resolve(dirs: &Dirs, id: &str) -> Res<Run> {
    let names = run_ids(dirs)?;
    let hits: Vec<&String> = names.iter().filter(|n| n.starts_with(id)).collect();
    let pick = names.iter().find(|n| *n == id).or(match hits.as_slice() {
        [one] => Some(*one),
        _ => None,
    });
    match pick {
        Some(n) if !id.is_empty() => Ok(Run {
            id: n.clone(),
            path: dirs.runs.join(n),
        }),
        _ if hits.len() > 1 => Err(format!(
            "{id:?} matches {} runs; give more of the id",
            hits.len()
        )),
        _ => Err(format!("no run {id:?} (see `clio runs`)")),
    }
}

/// Newest first.
fn run_ids(dirs: &Dirs) -> Res<Vec<String>> {
    let mut v: Vec<String> = match std::fs::read_dir(&dirs.runs) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(io_err(&dirs.runs, &e)),
    };
    v.sort_by(|a, b| b.cmp(a));
    Ok(v)
}

fn letter(k: Kind) -> char {
    match k {
        Kind::Modified => 'M',
        Kind::Created => 'A',
        Kind::Deleted => 'D',
    }
}

fn trim(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > n {
        format!("{}...", one.chars().take(n).collect::<String>())
    } else {
        one
    }
}

pub struct RunOpts {
    pub workspace: PathBuf,
    pub max_steps: Option<u32>,
    pub allow_broad: bool,
    pub instruction: String,
}

pub fn run(dirs: &Dirs, o: &RunOpts) -> Res<i32> {
    let cfg = Config::load(dirs)?;
    let ws = work::validate(&o.workspace, dirs)?;
    let (mf, mb) = if o.allow_broad {
        (u64::MAX, u64::MAX)
    } else {
        (cfg.workspace.max_files, cfg.workspace.max_bytes)
    };
    let scan = work::scan(&ws, mf, mb)?;
    let prompt = cfg.prompt_text(dirs)?;
    let program = cfg
        .model
        .command
        .first()
        .ok_or("config: model.command is empty")?
        .clone();

    let started = now_ms();
    let id = format!(
        "{}-{:04x}",
        stamp(started),
        started.rem_euclid(65_536) ^ i64::from(std::process::id() & 0xffff)
    );
    let path = dirs.runs.join(&id);
    if path.exists() {
        return Err(format!("run {id} already exists; try again"));
    }
    mkdir_private(&path)?;
    let manifest = work::copy_tree(&ws, &scan.files, &path.join("sandbox"))?;
    write_json(&path.join("manifest.json"), &manifest)?;
    write_json(
        &path.join("meta.json"),
        &Meta {
            id: id.clone(),
            workspace: ws.display().to_string(),
            instruction: o.instruction.clone(),
            created_ms: started,
        },
    )?;
    println!(
        "run {id}: copied {} files from {}",
        scan.files.len(),
        ws.display()
    );
    if !scan.secrets.is_empty() {
        println!(
            "NOT copied, they look like secrets: {}",
            scan.secrets.join(", ")
        );
    }
    if !scan.skipped_dirs.is_empty() {
        println!(
            "not copied (heavy or internal folders): {}",
            scan.skipped_dirs.join(", ")
        );
    }
    if !scan.symlinks.is_empty() {
        println!("not copied (symbolic links): {}", scan.symlinks.join(", "));
    }

    let mut acfg = AgentConfig::new(&id, &prompt, &path);
    acfg.prompt_version.clone_from(&cfg.model.prompt);
    acfg.budget.max_steps = o.max_steps.unwrap_or(cfg.limits.max_steps);
    acfg.budget.max_ms =
        i64::try_from(cfg.limits.max_seconds.saturating_mul(1000)).unwrap_or(i64::MAX);
    acfg.budget.max_cost_micro = cfg.limits.max_cost_micro_usd;
    acfg.price_in_micro_per_mtok = cfg.model.price_in_micro_per_mtok;
    acfg.price_out_micro_per_mtok = cfg.model.price_out_micro_per_mtok;
    // The person typed this instruction in this terminal; that is the approval a level-3 contract needs.
    acfg.person_approves_long = true;
    let mut agent = AgentLoop::new(
        Arc::new(SystemClock),
        acfg,
        &path.join("sandbox"),
        &path.join("scratch"),
    )?;
    agent.scrubber = Some(clio_vault::Scrubber::default());
    let mut model = CommandModel::new(&program, &cfg.model.command[1..], &cfg.model.model);
    println!("working in the copy (this can take a few minutes)...");
    let outcome = agent.run(&mut model, &o.instruction)?;

    work::lock_down(&path)?;
    write_json(&path.join("transcript.json"), &outcome.transcript)?;
    write_json(
        &path.join("report.json"),
        &json!({
            "done": outcome.report.done,
            "pending": outcome.report.pending,
            "next_step": outcome.report.next_step,
        }),
    )?;
    let ch = work::changes(&path.join("sandbox"), &manifest)?;
    let names = |k: Kind| -> Vec<&str> {
        ch.iter()
            .filter(|c| c.kind == k)
            .map(|c| c.rel.as_str())
            .collect()
    };
    let ending = format!("{:?}", outcome.ending);
    let summary = json!({
        "id": id,
        "workspace": ws.display().to_string(),
        "instruction": o.instruction,
        "ending": ending,
        "level": format!("{:?}", outcome.level),
        "steps": outcome.steps,
        "elapsed_ms": outcome.elapsed_ms,
        "input_tokens": outcome.input_tokens,
        "output_tokens": outcome.output_tokens,
        "cost_micro": outcome.cost_micro,
        "denied": outcome.denied,
        "protocol_errors": outcome.protocol_errors,
        "tainted": outcome.tainted,
        "contract_state": outcome.contract_state.map(|s| format!("{s:?}")),
        "modified": names(Kind::Modified),
        "created": names(Kind::Created),
        "deleted": names(Kind::Deleted),
        "excluded_secrets": scan.secrets,
        "final_message": outcome.final_message,
    });
    write_json(&path.join("summary.json"), &summary)?;

    println!();
    println!("run:      {id}");
    println!(
        "level:    {:?}   steps: {}   time: {:.1}s   ending: {ending}",
        outcome.level,
        outcome.steps,
        outcome.elapsed_ms as f64 / 1000.0
    );
    println!(
        "changes:  {} modified, {} created, {} deleted",
        names(Kind::Modified).len(),
        names(Kind::Created).len(),
        names(Kind::Deleted).len()
    );
    for c in &ch {
        println!("  {} {}", letter(c.kind), c.rel);
    }
    match &outcome.final_message {
        Some(m) => println!("agent:    {}", trim(m, 600)),
        None => println!(
            "agent:    (no final message) {}",
            outcome.report.pending.join("; ")
        ),
    }
    println!("The original was NOT touched. Review:  clio diff {id}   then  clio apply {id}");
    Ok(i32::from(outcome.ending != Ending::Final))
}

/// Shows the changes of `ch` against what is in the original now.
pub fn print_diff(
    ws: &Path,
    run: &Run,
    manifest: &BTreeMap<String, String>,
    ch: &[Change],
) -> Res<()> {
    for c in ch {
        let orig = std::fs::read(ws.join(&c.rel)).ok();
        let new = std::fs::read(run.sandbox().join(&c.rel)).ok();
        if let (Some(o), Some(h)) = (&orig, manifest.get(&c.rel))
            && super::sha_hex(o) != *h
        {
            println!("# NOTE: {} changed in the original after the copy", c.rel);
        }
        print!("{}", file_diff(&c.rel, orig.as_deref(), new.as_deref()));
    }
    Ok(())
}

pub fn diff(dirs: &Dirs, id: &str) -> Res<()> {
    let run = resolve(dirs, id)?;
    let (meta, manifest) = (run.meta()?, run.manifest()?);
    let ch = work::changes(&run.sandbox(), &manifest)?;
    if ch.is_empty() {
        println!("no changes in run {}", run.id);
        return Ok(());
    }
    print_diff(Path::new(&meta.workspace), &run, &manifest, &ch)
}

fn status(run: &Run) -> String {
    let p = &run.path;
    if p.join("applied.json").exists() {
        return "applied".into();
    }
    if p.join("undone.json").exists() {
        return "undone".into();
    }
    match read_json::<Value>(&p.join("summary.json")) {
        Ok(s) => s["ending"].as_str().unwrap_or("?").to_lowercase(),
        Err(_) => "incomplete".into(),
    }
}

pub fn list(dirs: &Dirs, all: bool) -> Res<()> {
    let ids = run_ids(dirs)?;
    if ids.is_empty() {
        println!("no runs yet");
    }
    for id in ids.iter().take(if all { usize::MAX } else { 20 }) {
        let run = Run {
            id: id.clone(),
            path: dirs.runs.join(id),
        };
        let sum: Value = read_json(&run.path.join("summary.json")).unwrap_or(Value::Null);
        let n = ["modified", "created", "deleted"]
            .iter()
            .map(|k| sum[*k].as_array().map_or(0, Vec::len))
            .sum::<usize>();
        let instr = run
            .meta()
            .map(|m| trim(&m.instruction, 50))
            .unwrap_or_default();
        println!("{id}  {:<10} {n} files  {instr}", status(&run));
    }
    Ok(())
}

pub fn show(dirs: &Dirs, id: &str) -> Res<()> {
    let run = resolve(dirs, id)?;
    println!("run {}  status: {}", run.id, status(&run));
    let meta = run.meta()?;
    println!("workspace:   {}", meta.workspace);
    println!("instruction: {}", meta.instruction);
    match read_json::<Value>(&run.path.join("summary.json")) {
        Ok(s) => println!(
            "summary:\n{}",
            serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?
        ),
        Err(_) => println!("(no summary: the run did not finish)"),
    }
    if let Ok(r) = read_json::<Value>(&run.path.join("report.json")) {
        println!(
            "closing report:\n{}",
            serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?
        );
    }
    println!("folder: {}", run.path.display());
    Ok(())
}
