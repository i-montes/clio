//! `clio run --harness pi "instrucción"`: one prompt on a new session, no terminal. The same
//! session code as the interface (a copy of the project, the toolhost, Pi with the guard); the
//! text streams to stdout, one line per tool, and the run is a normal run folder, so
//! `clio diff|apply|undo <id>` work on what Pi changed.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use clio_brain::pi::rpc::{Cancel, Event};

use super::local::LocalSource;
use super::{SessionSource, Target};
use crate::cli::Res;
use crate::cli::home::Dirs;
use crate::cli::tui::sanitize;
use crate::cli::usage;

pub fn run(dirs: Dirs, workspace: PathBuf, instruction: &str) -> Res<i32> {
    let source = LocalSource::new(Arc::new(dirs));
    let opened = source.open(&Target::New { project: workspace })?;
    println!("run: {}", opened.id);
    let mut out = std::io::stdout();
    let mut mid_line = false;
    let r = opened
        .session
        .run_prompt(instruction, &Cancel::new(), &mut |e| match e {
            Event::MessageUpdate {
                kind,
                delta: Some(d),
                ..
            } if kind == "text_delta" => {
                let d = sanitize::multiline(d);
                mid_line = !d.ends_with('\n');
                let _ = write!(out, "{d}");
                let _ = out.flush();
            }
            Event::ToolEnd {
                name,
                is_error,
                duration_ms,
                ..
            } => {
                if mid_line {
                    println!();
                    mid_line = false;
                }
                println!(
                    "  [{}] {}{}",
                    sanitize::line(name),
                    if *is_error { "falló" } else { "ok" },
                    duration_ms.map_or_else(String::new, |m| format!(" ({})", usage::ms_label(m)))
                );
            }
            _ => {}
        });
    if mid_line {
        println!();
    }
    let refresh = opened.session.refresh().ok();
    opened.session.shutdown();
    let r = r.map_err(|e| e.to_string())?;
    if let Some(a) = &r.assessment {
        println!("pedido juzgado como {}", a.notice());
    }
    if let Some(notice) = r.stop_notice() {
        println!("{notice}");
    }
    if let Some(f) = refresh {
        println!(
            "uso: ↑{} ↓{} tok · {} · {} herramientas{}",
            usage::tokens(f.stats.usage.input),
            usage::tokens(f.stats.usage.output),
            usage::usd((f.stats.usage.cost * 1e6).round() as u64),
            r.tool_calls,
            if f.tainted {
                " · leyó contenido externo"
            } else {
                ""
            }
        );
    }
    println!("siguiente: clio diff {}", opened.id);
    Ok(exit_code(&r))
}

/// 0 only for a run that ended by itself: aborted, with errors from the model, or cut by a budget
/// cap (S-47) is a failure for whoever scripts `clio run`.
fn exit_code(r: &clio_brain::pi::rpc::RunSummary) -> i32 {
    i32::from(r.aborted || r.stopped.is_some() || !r.errors.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clio_brain::control::Stop;
    use clio_brain::pi::rpc::RunSummary;

    #[test]
    fn only_a_run_that_ended_by_itself_exits_zero() {
        assert_eq!(exit_code(&RunSummary::default()), 0);
        let stopped = RunSummary {
            stopped: Some(Stop::Steps),
            ..RunSummary::default()
        };
        assert_eq!(
            exit_code(&stopped),
            1,
            "cut by a cap, even if Pi had settled"
        );
        let aborted = RunSummary {
            aborted: true,
            ..RunSummary::default()
        };
        assert_eq!(exit_code(&aborted), 1);
        let errors = RunSummary {
            errors: vec!["x".into()],
            ..RunSummary::default()
        };
        assert_eq!(exit_code(&errors), 1);
    }
}
