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

const HELP: &str = "usage: clio-bench <replay|record|real|synthetic-tape> --tasks DIR [--tape FILE] [--prices FILE] \
[--repeats N] [--trace DB] [--provider P] [--model M] [--cmd PROGRAM [ARG...]] \
[--baseline FILE --thresholds FILE] [--save-baseline FILE]";

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
        if let Some(name) = a.strip_prefix("--") {
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
    let tasks = load_tasks(&PathBuf::from(need("tasks")?))?;
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
    let trace_path = one("trace").map_or_else(
        || std::env::temp_dir().join(format!("clio-bench-{}.db", std::process::id())),
        PathBuf::from,
    );
    let trace = if mode == "replay" {
        Trace::open(&trace_path, FixedClock::new(0))
    } else {
        Trace::open(&trace_path, SystemClock)
    }
    .map_err(|e| e.to_string())?;
    for p in serde_json::from_str::<Vec<Price>>(
        &std::fs::read_to_string(need("prices")?).map_err(|e| e.to_string())?,
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
            Check::NotContains(_) => None,
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
