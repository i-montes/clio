//! A run: `clio run`, `diff`, `runs` and `show`. Everything of a run lives in
//! `~/.local/share/clio/runs/<id>/`:
//! `sandbox/` (the copy the agent worked on), `manifest.json` (hash of each copied file),
//! `meta.json`, `contract.json`, `effects.jsonl`, `transcript.json`, `report.json`,
//! `summary.json`, and later `backup/` and `applied.json`.
//!
//! The work is split so the plain commands and the TUI share it: `prepare` (validate, scan, copy),
//! `execute` (the agent, then the files of the run), and readers (`RunInfo`, `list_info`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use clio_brain::agent::{AgentConfig, AgentLoop, CommandModel, Event, Observer};
use clio_core::SystemClock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::debug::{self, DebugCtl, Tee};
use super::diff::file_diff;
use super::home::{Config, Dirs};
use super::tui::sanitize;
use super::ui::{self, Style, clip, pad};
use super::usage::{self, RunLimits, RunUsage};
use super::work::{self, Change, Kind, Scan};
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
            "{id:?} coincide con {} runs; da más del id",
            hits.len()
        )),
        _ => Err(format!("no hay un run {id:?} (mira `clio runs`)")),
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

pub fn letter(k: Kind) -> char {
    match k {
        Kind::Modified => 'M',
        Kind::Created => 'A',
        Kind::Deleted => 'D',
    }
}

/// Everything about a run that ends up in `summary.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunSummary {
    pub id: String,
    pub workspace: String,
    pub instruction: String,
    /// The `Debug` form of `Ending`: `Final`, `Stopped(Steps)`, `Cancelled`, ...
    pub ending: String,
    pub level: String,
    pub steps: u32,
    pub elapsed_ms: i64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micro: u64,
    pub denied: u32,
    pub protocol_errors: u32,
    pub tainted: bool,
    pub contract_state: Option<String>,
    pub modified: Vec<String>,
    pub created: Vec<String>,
    pub deleted: Vec<String>,
    pub excluded_secrets: Vec<String>,
    pub final_message: Option<String>,
    pub pending: Vec<String>,
}

impl RunSummary {
    pub fn files(&self) -> usize {
        self.modified.len() + self.created.len() + self.deleted.len()
    }

    /// Changed files as (letter, path), in the order M, A, D.
    pub fn changed(&self) -> Vec<(char, &str)> {
        let mut out = Vec::new();
        out.extend(self.modified.iter().map(|p| ('M', p.as_str())));
        out.extend(self.created.iter().map(|p| ('A', p.as_str())));
        out.extend(self.deleted.iter().map(|p| ('D', p.as_str())));
        out
    }

    pub fn succeeded(&self) -> bool {
        self.ending == "Final"
    }
}

/// Spanish words for an `Ending` in its `Debug` form.
pub fn ending_label(ending: &str) -> String {
    match ending {
        "Final" => "terminó".into(),
        "Cancelled" => "cancelado".into(),
        "NeedsApproval" => "necesita tu aprobación".into(),
        e if e.starts_with("Stopped(") => match e {
            "Stopped(Steps)" => "detenido: límite de pasos".into(),
            "Stopped(Time)" => "detenido: límite de tiempo".into(),
            "Stopped(Cost)" | "Stopped(DayCost)" => "detenido: límite de costo".into(),
            "Stopped(Repeats)" => "detenido: repetía la misma acción".into(),
            "Stopped(Retries)" => "detenido: demasiados reintentos".into(),
            other => format!("detenido ({other})"),
        },
        e if e.starts_with("ModelError") => "falló el modelo".into(),
        other => other.to_lowercase(),
    }
}

/// What a person sees about a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Applied,
    Undone,
    Pending,
    NoChanges,
    Failed,
    Cancelled,
    Incomplete,
}

impl Status {
    /// Stable word for scripts (`--json`).
    pub fn code(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Undone => "undone",
            Self::Pending => "pending_review",
            Self::NoChanges => "no_changes",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Incomplete => "incomplete",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Applied => "aplicado",
            Self::Undone => "deshecho",
            Self::Pending => "pendiente de revisar",
            Self::NoChanges => "sin cambios",
            Self::Failed => "fallido",
            Self::Cancelled => "cancelado",
            Self::Incomplete => "incompleto",
        }
    }

    fn paint(self, st: Style, s: &str) -> String {
        match self {
            Self::Applied => st.green(s),
            Self::Pending => st.yellow(s),
            Self::Failed | Self::Incomplete => st.red(s),
            Self::Undone | Self::NoChanges | Self::Cancelled => st.dim(s),
        }
    }
}

/// A run as the list and the screens show it. Tolerant: a half-written run still loads.
#[derive(Debug, Clone)]
pub struct RunInfo {
    pub id: String,
    pub status: Status,
    pub instruction: String,
    pub workspace: String,
    pub created_ms: Option<i64>,
    pub summary: Option<RunSummary>,
}

impl RunInfo {
    pub fn files(&self) -> usize {
        self.summary.as_ref().map_or(0, RunSummary::files)
    }

    pub fn load(run: &Run) -> Self {
        let summary: Option<RunSummary> = read_json(&run.path.join("summary.json")).ok();
        let meta = run.meta().ok();
        let status = if run.path.join("applied.json").exists() {
            Status::Applied
        } else if run.path.join("undone.json").exists() {
            Status::Undone
        } else {
            match &summary {
                None => Status::Incomplete,
                Some(s) if s.ending == "Cancelled" => Status::Cancelled,
                Some(s) if !s.succeeded() => Status::Failed,
                Some(s) if s.files() == 0 => Status::NoChanges,
                Some(_) => Status::Pending,
            }
        };
        Self {
            id: run.id.clone(),
            status,
            instruction: meta
                .as_ref()
                .map(|m| m.instruction.clone())
                .unwrap_or_default(),
            workspace: meta
                .as_ref()
                .map(|m| m.workspace.clone())
                .unwrap_or_default(),
            created_ms: meta.map(|m| m.created_ms),
            summary,
        }
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "status": self.status.code(),
            "ending": self.summary.as_ref().map(|s| s.ending.clone()),
            "files": self.files(),
            "instruction": self.instruction,
            "workspace": self.workspace,
            "created_ms": self.created_ms,
        })
    }
}

/// The newest `limit` runs (all when `None`).
pub fn list_info(dirs: &Dirs, limit: Option<usize>) -> Res<Vec<RunInfo>> {
    Ok(run_ids(dirs)?
        .into_iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(|id| {
            let path = dirs.runs.join(&id);
            RunInfo::load(&Run { id, path })
        })
        .collect())
}

pub struct RunOpts {
    pub workspace: PathBuf,
    pub max_steps: Option<u32>,
    pub allow_broad: bool,
    pub instruction: String,
}

/// A run that is validated and copied, ready for the agent.
pub struct Prepared {
    pub id: String,
    pub path: PathBuf,
    pub ws: PathBuf,
    pub scan: Scan,
    pub max_steps: u32,
    pub max_seconds: u64,
    pub max_cost_micro: u64,
    /// The model's context window in tokens, and whether the catalog knew it.
    pub window: u64,
    pub window_known: bool,
    /// Knows the model's API key.
    scrubber: clio_vault::Scrubber,
    manifest: BTreeMap<String, String>,
    cfg: Config,
    prompt: String,
    prompt_name: String,
    /// The jail was proven: `run_tests` is offered.
    pub jail: bool,
    pub jail_note: String,
    program: String,
}

/// Validates the workspace, scans it and makes the copy. Nothing of the original is touched.
pub fn prepare(dirs: &Dirs, o: &RunOpts) -> Res<Prepared> {
    let cfg = Config::load(dirs)?;
    let ws = work::validate(&o.workspace, dirs)?;
    let (mf, mb) = if o.allow_broad {
        (u64::MAX, u64::MAX)
    } else {
        (cfg.workspace.max_files, cfg.workspace.max_bytes)
    };
    let scan = work::scan(&ws, mf, mb)?;
    let jail = super::jail::status();
    let (prompt_name, prompt) = cfg.prompt_text(dirs, jail.ready())?;
    let program = cfg
        .model
        .command
        .first()
        .ok_or("config: model.command está vacío (revisa ~/.config/clio/config.json)")?
        .clone();
    let started = now_ms();
    let id = format!(
        "{}-{:04x}",
        stamp(started),
        started.rem_euclid(65_536) ^ i64::from(std::process::id() & 0xffff)
    );
    let path = dirs.runs.join(&id);
    if path.exists() {
        return Err(format!("el run {id} ya existe; inténtalo de nuevo"));
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
    let (window, window_known) = usage::window_for(dirs, &cfg.model.provider, &cfg.model.model);
    Ok(Prepared {
        max_steps: o.max_steps.unwrap_or(cfg.limits.max_steps),
        max_seconds: cfg.limits.max_seconds,
        max_cost_micro: cfg.limits.max_cost_micro_usd,
        window,
        window_known,
        scrubber: debug::scrubber(dirs),
        id,
        path,
        ws,
        scan,
        manifest,
        cfg,
        prompt,
        prompt_name,
        jail: jail.ready(),
        jail_note: jail.summary(),
        program,
    })
}

/// Runs the agent in the copy and writes the files of the run. `observer` sees every step (its
/// events are scrubbed first); `cancel` set to true ends the run after the step in progress (the
/// run is kept as cancelled). `dbg` says whether to write `debug.log` (and print on stderr).
pub fn execute(
    p: &Prepared,
    instruction: &str,
    observer: Box<dyn Observer>,
    cancel: Option<Arc<AtomicBool>>,
    dbg: &DebugCtl,
) -> Res<RunSummary> {
    let (cfg, path, id) = (&p.cfg, &p.path, &p.id);
    let mut acfg = AgentConfig::new(id, &p.prompt, path);
    acfg.prompt_version.clone_from(&p.prompt_name);
    acfg.run_tests = p.jail;
    acfg.budget.max_steps = p.max_steps;
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
    let scrubber = p.scrubber.clone();
    agent.scrubber = Some(scrubber.clone());
    let used = Arc::new(Mutex::new(RunUsage::new(
        p.window,
        p.window_known,
        RunLimits {
            max_steps: p.max_steps,
            max_seconds: p.max_seconds,
            max_cost_micro: p.max_cost_micro,
        },
    )));
    agent.observer = Box::new(Tee::new(
        observer,
        used.clone(),
        scrubber,
        dbg.clone(),
        path.join("debug.log"),
    ));
    agent.debug = Some(dbg.on.clone());
    agent.cancel = cancel;
    let mut model = CommandModel::new(&p.program, &cfg.model.command[1..], &cfg.model.model);
    let outcome = agent.run(&mut model, instruction)?;

    work::lock_down(path)?;
    let usage = used.lock().map(|u| u.clone()).unwrap_or_default();
    write_json(&path.join("usage.json"), &usage)?;
    write_json(&path.join("transcript.json"), &outcome.transcript)?;
    write_json(
        &path.join("report.json"),
        &json!({
            "done": outcome.report.done,
            "pending": outcome.report.pending,
            "next_step": outcome.report.next_step,
        }),
    )?;
    let ch = work::changes(&path.join("sandbox"), &p.manifest)?;
    let names = |k: Kind| -> Vec<String> {
        ch.iter()
            .filter(|c| c.kind == k)
            .map(|c| c.rel.clone())
            .collect()
    };
    let summary = RunSummary {
        id: id.clone(),
        workspace: p.ws.display().to_string(),
        instruction: instruction.to_owned(),
        ending: format!("{:?}", outcome.ending),
        level: format!("{:?}", outcome.level),
        steps: outcome.steps,
        elapsed_ms: outcome.elapsed_ms,
        input_tokens: outcome.input_tokens,
        output_tokens: outcome.output_tokens,
        cost_micro: outcome.cost_micro,
        denied: outcome.denied,
        protocol_errors: outcome.protocol_errors,
        tainted: outcome.tainted,
        contract_state: outcome.contract_state.map(|s| format!("{s:?}")),
        modified: names(Kind::Modified),
        created: names(Kind::Created),
        deleted: names(Kind::Deleted),
        excluded_secrets: p.scan.secrets.clone(),
        final_message: outcome.final_message,
        pending: outcome.report.pending,
    };
    write_json(&path.join("summary.json"), &summary)?;
    Ok(summary)
}

fn exit_code(s: &RunSummary) -> i32 {
    i32::from(!s.succeeded())
}

/// One line per step for the plain `clio run`.
struct Progress {
    st: Style,
    max_steps: u32,
    step: u32,
    started: std::time::Instant,
}

impl Observer for Progress {
    fn event(&mut self, e: &Event) {
        use clio_gateway::Outcome;
        let secs = self.started.elapsed().as_secs();
        let line = match e {
            Event::Step { n } => {
                self.step = *n;
                return;
            }
            Event::Started { level } => format!("nivel de la tarea: {level:?}"),
            Event::ProtocolError { reason } => format!(
                "el modelo contestó algo que no es una acción ({})",
                clip(self.st, &sanitize::line(reason), 60)
            ),
            Event::Final => "el modelo dice que terminó".to_owned(),
            Event::Context(_) | Event::Usage(_) | Event::Exchange(_) | Event::Decision(_) => {
                return;
            }
            Event::Tool {
                name,
                target,
                outcome,
                rule,
                ran,
                result_chars,
                ..
            } => {
                let mark = match (outcome, ran) {
                    (Outcome::Deny, _) => self.st.red("negado"),
                    (_, false) => self.st.yellow("pedido, sin ejecutar"),
                    (Outcome::Ask, true) => self.st.yellow("pedido y aprobado"),
                    _ => self.st.green("permitido"),
                };
                let size = if *ran {
                    format!("  {result_chars} car.")
                } else {
                    String::new()
                };
                format!(
                    "{} {}  {mark} ({}){size}",
                    sanitize::line(name),
                    clip(self.st, &sanitize::line(target), 40),
                    sanitize::line(rule),
                )
            }
        };
        let time = self.st.dim(&format!("{secs:>3}s"));
        println!("  {time}  [{}/{}] {line}", self.step, self.max_steps);
    }
}

pub fn run(dirs: &Dirs, o: &RunOpts, debug: bool) -> Res<i32> {
    let st = Style::stdout();
    let p = prepare(dirs, o)?;
    if debug {
        eprintln!(
            "[debug] depuración activa: el detalle va a stderr y a {}",
            p.path.join("debug.log").display()
        );
    }
    println!(
        "{} {}: {} archivos copiados de {}",
        st.bold("run"),
        st.cyan(&p.id),
        p.scan.files.len(),
        sanitize::line(&p.ws.display().to_string())
    );
    let list = |v: &[String]| {
        v.iter()
            .map(|s| sanitize::line(s))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !p.scan.secrets.is_empty() {
        println!(
            "  {} NO se copió, parece un secreto: {}",
            st.mark(ui::Level::Warn),
            list(&p.scan.secrets)
        );
    }
    if !p.scan.skipped_dirs.is_empty() {
        println!(
            "  {} no se copió (carpetas pesadas o internas): {}",
            st.dim(st.bullet()),
            list(&p.scan.skipped_dirs)
        );
    }
    if !p.scan.symlinks.is_empty() {
        println!(
            "  {} no se copió (enlaces simbólicos): {}",
            st.dim(st.bullet()),
            list(&p.scan.symlinks)
        );
    }
    println!("  {} {}", st.dim(st.bullet()), p.jail_note);
    println!(
        "trabajando en la copia (puede tardar unos minutos; hasta {} pasos o {} s)...",
        p.max_steps, p.max_seconds
    );
    let progress = Progress {
        st,
        max_steps: p.max_steps,
        step: 0,
        started: std::time::Instant::now(),
    };
    let s = execute(
        &p,
        &o.instruction,
        Box::new(progress),
        None,
        &DebugCtl::new(debug, true),
    )?;
    println!();
    print_panel(st, &s);
    Ok(exit_code(&s))
}

/// The closing panel of a run.
fn print_panel(st: Style, s: &RunSummary) {
    let w = ui::width();
    let label = |l: &str| st.bold(&pad(l, 10));
    println!("{}{}", label("run:"), st.cyan(&s.id));
    println!(
        "{}{}   pasos: {}   tiempo: {:.1}s   fin: {}",
        label("nivel:"),
        s.level,
        s.steps,
        s.elapsed_ms as f64 / 1000.0,
        {
            let e = ending_label(&s.ending);
            if s.succeeded() {
                st.green(&e)
            } else {
                st.red(&e)
            }
        }
    );
    println!(
        "{}{} modificados, {} creados, {} borrados",
        label("cambios:"),
        s.modified.len(),
        s.created.len(),
        s.deleted.len()
    );
    for (c, p) in s.changed() {
        println!("  {c} {}", sanitize::line(p));
    }
    if s.tainted {
        println!(
            "{}{}",
            label("nota:"),
            st.yellow("externo: leyó archivos del proyecto; revisa el diff con cuidado")
        );
    }
    let msg = match &s.final_message {
        Some(m) => sanitize::multiline(m),
        None => format!(
            "(sin mensaje final) {}",
            sanitize::line(&s.pending.join("; "))
        ),
    };
    let lines = ui::wrap(&msg, w.saturating_sub(10));
    let shown = lines.len().min(14);
    for (i, l) in lines.iter().take(shown).enumerate() {
        println!(
            "{}{l}",
            if i == 0 {
                label("agente:")
            } else {
                " ".repeat(10)
            }
        );
    }
    if lines.len() > shown {
        println!(
            "{}{}",
            " ".repeat(10),
            st.dim(&format!("{} (más en `clio show {}`)", st.ellipsis(), s.id))
        );
    }
    println!();
    if s.files() > 0 {
        println!(
            "El original NO se tocó. Revisa:  {}   y luego  {}",
            st.bold(&format!("clio diff {}", s.id)),
            st.bold(&format!("clio apply {}", s.id))
        );
    } else {
        println!("El original NO se tocó. No hay cambios que aplicar.");
    }
}

/// The diff of one change against what is in the original now, with a note when the original
/// changed after the copy.
pub fn diff_text(ws: &Path, run: &Run, manifest: &BTreeMap<String, String>, c: &Change) -> String {
    let orig = std::fs::read(ws.join(&c.rel)).ok();
    let new = std::fs::read(run.sandbox().join(&c.rel)).ok();
    let mut out = String::new();
    if let (Some(o), Some(h)) = (&orig, manifest.get(&c.rel))
        && super::sha_hex(o) != *h
    {
        out += &format!(
            "# OJO: {} cambió en el original después de la copia\n",
            c.rel
        );
    }
    out + &file_diff(&c.rel, orig.as_deref(), new.as_deref())
}

/// Prints the changes of `ch`. On a terminal the text is sanitized and colored.
pub fn print_diff(
    st: Style,
    ws: &Path,
    run: &Run,
    manifest: &BTreeMap<String, String>,
    ch: &[Change],
) {
    for c in ch {
        let text = diff_text(ws, run, manifest, c);
        let text = if st.tty {
            sanitize::multiline(&text)
        } else {
            text
        };
        for l in text.lines() {
            let painted = if l.starts_with("+++") || l.starts_with("---") {
                st.bold(l)
            } else if l.starts_with('+') {
                st.green(l)
            } else if l.starts_with('-') {
                st.red(l)
            } else if l.starts_with("@@") {
                st.cyan(l)
            } else {
                l.to_owned()
            };
            println!("{painted}");
        }
    }
}

pub fn diff(dirs: &Dirs, id: &str) -> Res<()> {
    let st = Style::stdout();
    let run = resolve(dirs, id)?;
    let (meta, manifest) = (run.meta()?, run.manifest()?);
    let ch = work::changes(&run.sandbox(), &manifest)?;
    if ch.is_empty() {
        println!("el run {} no tiene cambios", run.id);
        return Ok(());
    }
    print_diff(st, Path::new(&meta.workspace), &run, &manifest, &ch);
    Ok(())
}

pub fn list(dirs: &Dirs, all: bool, json: bool) -> Res<()> {
    let st = Style::stdout();
    let rows = list_info(dirs, (!all).then_some(20))?;
    if json {
        let v: Vec<Value> = rows.iter().map(RunInfo::json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("todavía no hay runs. Empieza con: clio run \"qué hacer\"");
        return Ok(());
    }
    let sw = 20;
    let iw = ui::width()
        .saturating_sub(20 + 2 + sw + 2 + 8 + 2)
        .clamp(20, 70);
    println!(
        "{}",
        st.bold(&format!(
            "{}  {}  {}  {}",
            pad("RUN", 20),
            pad("ESTADO", sw),
            "ARCHIVOS",
            "INSTRUCCIÓN"
        ))
    );
    for r in &rows {
        println!(
            "{}  {}  {:>8}  {}",
            pad(&r.id, 20),
            r.status.paint(st, &pad(r.status.label(), sw)),
            r.files(),
            clip(st, &sanitize::line(&r.instruction), iw)
        );
    }
    Ok(())
}

pub fn show(dirs: &Dirs, id: &str, json: bool) -> Res<()> {
    let st = Style::stdout();
    let run = resolve(dirs, id)?;
    let info = RunInfo::load(&run);
    let report: Option<Value> = read_json(&run.path.join("report.json")).ok();
    if json {
        let mut v = info.json();
        v["summary"] = info.summary.as_ref().map_or(Value::Null, |s| {
            serde_json::to_value(s).unwrap_or(Value::Null)
        });
        v["report"] = report.unwrap_or(Value::Null);
        v["folder"] = json!(run.path.display().to_string());
        println!(
            "{}",
            serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let w = ui::width();
    let label = |l: &str| st.bold(&pad(l, 17));
    println!("{}{}", label("run:"), st.cyan(&run.id));
    println!(
        "{}{}",
        label("estado:"),
        info.status.paint(st, info.status.label())
    );
    println!("{}{}", label("carpeta:"), sanitize::line(&info.workspace));
    println!(
        "{}{}",
        label("instrucción:"),
        sanitize::line(&info.instruction)
    );
    match &info.summary {
        Some(s) => {
            println!(
                "{}{}   pasos: {}   tiempo: {:.1}s   fin: {}",
                label("nivel:"),
                s.level,
                s.steps,
                s.elapsed_ms as f64 / 1000.0,
                ending_label(&s.ending)
            );
            println!(
                "{}{} denegadas, {} errores de protocolo, {} tokens de entrada, {} de salida",
                label("modelo:"),
                s.denied,
                s.protocol_errors,
                s.input_tokens,
                s.output_tokens
            );
            println!("{}", label("cambios:"));
            for (c, p) in s.changed() {
                println!("  {c} {}", sanitize::line(p));
            }
            if let Some(m) = &s.final_message {
                println!("{}", label("agente:"));
                for l in ui::wrap(&sanitize::multiline(m), w.saturating_sub(2)) {
                    println!("  {l}");
                }
            }
            if !s.pending.is_empty() {
                println!(
                    "{}{}",
                    label("pendiente:"),
                    sanitize::line(&s.pending.join("; "))
                );
            }
        }
        None => println!("(sin resumen: el run no llegó a terminar)"),
    }
    if let Some(n) = report.as_ref().and_then(|r| r["next_step"].as_str()) {
        println!("{}{}", label("siguiente:"), sanitize::line(n));
    }
    println!("{}{}", label("carpeta del run:"), run.path.display());
    Ok(())
}
