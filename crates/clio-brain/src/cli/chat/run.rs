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
    Ok(i32::from(r.aborted || !r.errors.is_empty()))
}
