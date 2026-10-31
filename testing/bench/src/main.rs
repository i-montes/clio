//! `clio-bench <replay|record|real> --tasks DIR [options]`: runs the bench and prints one line.
//! Exit 0 when the gate passes, 1 when the run is worse than the baseline allows, 2 on a usage
//! or setup error.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clio_bench::{
    Check, CommandLlm, Report, SingleCall, Thresholds, load_tasks, regressions, request_for, run,
};
use clio_fake_llm::{Llm, Record, Replay, Response, Scripted, Tape};
use clio_trace::{FixedClock, SystemClock, Trace};

const HELP: &str = "usage: clio-bench <replay|record|real|synthetic-tape|agent> --tasks DIR [--tape FILE] [--prices FILE] \
[--repeats N] [--trace DB] [--provider P] [--model M] [--cmd PROGRAM [ARG...]] \
[--baseline FILE --thresholds FILE] [--save-baseline FILE] \
(agent: [--prompt FILE] [--tags A,B] [--only ID,ID] [--keep DIR]) \
(--cmd takes the rest of the line, so it goes last)";

const FLAGS: [&str; 15] = [
    "prompt",
    "tags",
    "only",
    "keep",
    "tasks",
    "tape",
    "prices",
    "repeats",
    "trace",
    "provider",
    "model",
    "cmd",
    "baseline",
    "thresholds",
    "save-baseline",
];

fn main() -> ExitCode {
    match go() {
        Ok(code) => code,
        Err(m) => {
            eprintln!("error: {m}\nhelp: clio-bench --help");
            ExitCode::from(2)
        }
    }
}

fn go() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("missing mode")?;
    if mode == "--help" || mode == "-h" {
        println!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current = String::new();
    for a in args {
        if current == "cmd" {
            flags.entry(current.clone()).or_default().push(a);
        } else if let Some(name) = a.strip_prefix("--") {
            if !FLAGS.contains(&name) {
                return Err(format!("unknown flag --{name}"));
            }
            current = name.to_string();
            flags.entry(current.clone()).or_default();
        } else if current.is_empty() {
            return Err(format!("unexpected argument {a}"));
        } else {
            flags.entry(current.clone()).or_default().push(a);
        }
    }
    let one = |name: &str| flags.get(name).and_then(|v| v.first()).cloned();
    let need = |name: &str| one(name).ok_or(format!("missing --{name}"));
    let all = load_tasks(&PathBuf::from(need("tasks")?))?;
    if mode == "agent" {
        return agent_mode(&flags, &all);
    }
    // A task with a workspace needs an agent that edits files; the single-call baseline cannot run it.
    let (tasks, needs_agent): (Vec<_>, Vec<_>) =
        all.into_iter().partition(|t| t.workspace.is_none());
    if !needs_agent.is_empty() {
        eprintln!(
            "note: {} task(s) need an agent and were skipped",
            needs_agent.len()
        );
    }
    let provider = one("provider").unwrap_or_else(|| "fake".into());
    let model = one("model").unwrap_or_else(|| "replay-1".into());

    if mode == "synthetic-tape" {
        return synthetic_tape(&tasks, &need("tape")?, &model);
    }

    let llm: Box<dyn Llm> = match mode.as_str() {
        "replay" => Box::new(Replay::new(
            Tape::open(need("tape")?).map_err(|e| e.to_string())?,
        )),
        "record" => Box::new(Record::new(
            command(&flags)?,
            Tape::open(need("tape")?).map_err(|e| e.to_string())?,
        )),
        "real" => Box::new(command(&flags)?),
        _ => return Err(format!("unknown mode {mode}")),
    };
    // Replay keeps time still so two runs give the same report; the others measure it.
    let trace_path = match one("trace") {
        Some(p) => PathBuf::from(p),
        None => private_dir()?.join("trace.db"),
    };
    let trace = if mode == "replay" {
        Trace::open(&trace_path, FixedClock::new(0))
    } else {
        Trace::open(&trace_path, SystemClock)
    }
    .map_err(|e| e.to_string())?;
    set_prices(&trace, &need("prices")?)?;
    let repeats = one("repeats").map_or(Ok(1), |n| n.parse::<u32>().map_err(|e| e.to_string()))?;
    let agent = SingleCall {
        llm,
        provider,
        model,
    };
    let report = run(&tasks, &agent, repeats, &trace)?;
    println!("{}", report.line());

    if let Some(path) = one("save-baseline") {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())?;
        println!("baseline saved to {path}");
    }
    if let (Some(b), Some(t)) = (one("baseline"), one("thresholds")) {
        let baseline: Report =
            serde_json::from_str(&std::fs::read_to_string(&b).map_err(|e| format!("{b}: {e}"))?)
                .map_err(|e| format!("{b}: {e}"))?;
        let limits: Thresholds =
            serde_json::from_str(&std::fs::read_to_string(&t).map_err(|e| format!("{t}: {e}"))?)
                .map_err(|e| format!("{t}: {e}"))?;
        let bad = regressions(&baseline, &report, &limits);
        for m in &bad {
            eprintln!("gate: {m}");
        }
        if !bad.is_empty() {
            return Ok(ExitCode::from(1));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn set_prices(trace: &Trace, file: &str) -> Result<(), String> {
    for p in serde_json::from_str::<Vec<Price>>(
        &std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?,
    )
    .map_err(|e| e.to_string())?
    {
        trace
            .set_price(
                &p.provider,
                &p.model,
                0,
                p.input_per_mtok,
                p.output_per_mtok,
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `agent`: the real agent loop over the tasks that have a workspace. Exit 0 unless setup fails;
/// there is no gate yet (the baseline is a measurement, not a limit).
fn agent_mode(
    flags: &BTreeMap<String, Vec<String>>,
    all: &[clio_bench::Task],
) -> Result<ExitCode, String> {
    let one = |name: &str| flags.get(name).and_then(|v| v.first()).cloned();
    let need = |name: &str| one(name).ok_or(format!("missing --{name}"));
    let list = |name: &str| -> Vec<String> {
        one(name)
            .map(|v| v.split(',').map(str::to_owned).collect())
            .unwrap_or_default()
    };
    let cmd = flags
        .get("cmd")
        .filter(|v| !v.is_empty())
        .ok_or("missing --cmd PROGRAM")?;
    let provider = one("provider").unwrap_or_else(|| "opencode-go".into());
    let model = one("model").unwrap_or_else(|| "beam-free".into());
    let prompt_path =
        PathBuf::from(one("prompt").unwrap_or_else(|| "prompts/system/clio-agent@2.md".into()));
    let system_prompt = std::fs::read_to_string(&prompt_path)
        .map_err(|e| format!("{}: {e}", prompt_path.display()))?;
    // The first line is `version: N`; the prompt name is the file name before the `@`.
    let version = prompt_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("agent")
        .to_owned();
    let prices: Vec<Price> =
        serde_json::from_str(&std::fs::read_to_string(need("prices")?).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let price = prices
        .iter()
        .find(|p| p.provider == provider && p.model == model)
        .ok_or_else(|| format!("no price for {provider}/{model} in the prices file"))?;
    let keep = one("keep").map(PathBuf::from);
    let run_root = match &keep {
        Some(p) => {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            p.clone()
        }
        None => private_dir()?,
    };
    let trace_path = match one("trace") {
        Some(p) => PathBuf::from(p),
        None => private_dir()?.join("trace.db"),
    };
    let trace = Trace::open(&trace_path, SystemClock).map_err(|e| e.to_string())?;
    set_prices(&trace, &need("prices")?)?;
    let cfg = clio_bench::agent_bench::AgentBenchConfig {
        provider,
        model,
        system_prompt,
        prompt_version: version,
        program: cmd[0].clone(),
        args: cmd[1..].to_vec(),
        repeats: one("repeats").map_or(Ok(1), |n| n.parse::<u32>().map_err(|e| e.to_string()))?,
        tags: list("tags"),
        only: list("only"),
        run_root: run_root.clone(),
        cleanup: keep.is_none(),
        price_in_micro_per_mtok: price.input_per_mtok,
        price_out_micro_per_mtok: price.output_per_mtok,
        backoff_ms: vec![5_000, 20_000, 60_000],
    };
    let report = clio_bench::agent_bench::run_agent(all, &cfg, &trace)?;
    println!("{}", report.line());
    print!("{}", report.table());
    if let Some(path) = one("save-baseline") {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())?;
        println!("baseline saved to {path}");
    }
    if keep.is_none() {
        let _ = std::fs::remove_dir_all(&run_root);
    }
    Ok(ExitCode::SUCCESS)
}

/// A new directory in the temp dir that only this user can enter. The name is made here and
/// `create` fails if it exists, so nobody can plant a link at the path beforehand.
fn private_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("clio-bench-{}-{nanos}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

#[derive(serde::Deserialize)]
struct Price {
    provider: String,
    model: String,
    input_per_mtok: u64,
    output_per_mtok: u64,
}

fn command(flags: &BTreeMap<String, Vec<String>>) -> Result<CommandLlm, String> {
    let cmd = flags
        .get("cmd")
        .filter(|v| !v.is_empty())
        .ok_or("missing --cmd PROGRAM")?;
    Ok(CommandLlm {
        program: cmd[0].clone(),
        args: cmd[1..].to_vec(),
    })
}

/// Writes a tape whose answer to each task is the text its check looks for: plumbing for the
/// replay gate before a real recording exists. These are not real model answers.
fn synthetic_tape(tasks: &[clio_bench::Task], tape: &str, model: &str) -> Result<ExitCode, String> {
    let answers: Vec<(clio_bench::Task, String)> = tasks
        .iter()
        .filter_map(|t| match &t.check {
            Check::Equals(v) | Check::Contains(v) => Some((t.clone(), v.clone())),
            Check::NotContains(_) | Check::Command(_) => None,
        })
        .collect();
    let scripted = Scripted::new(answers.iter().map(|(t, a)| Response {
        content: a.clone(),
        input_tokens: (t.input.chars().count() / 4 + 1) as u32,
        output_tokens: (a.chars().count() / 4 + 1) as u32,
    }));
    let record = Record::new(scripted, Tape::open(tape).map_err(|e| e.to_string())?);
    for (t, _) in &answers {
        record
            .complete(&request_for(model, &t.input))
            .map_err(|e| e.to_string())?;
    }
    println!("synthetic tape: {} answers in {tape}", answers.len());
    Ok(ExitCode::SUCCESS)
}
