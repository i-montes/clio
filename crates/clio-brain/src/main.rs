//! `clio`: use Clio for real on this machine. The agent works on a COPY of your project; you
//! decide what goes back to the original. See `docs/uso-local.md`.

mod cli;

use std::io::IsTerminal;

use cli::chat;
use cli::home::Dirs;
use cli::{Args, Res, apply, debug, doctor, home, memory, pi, runs, tui, vault};

const USAGE: &str = "clio: un agente personal que trabaja en una copia de tu proyecto

  clio [--debug] [--harness pi|builtin]       abre la interfaz (en una terminal); --debug o CLIO_DEBUG=1: modo debug;
                                              con Pi instalado abre el selector de sesiones y el chat (CLIO_HARNESS=builtin lo apaga)
  clio init                                   crea ~/.config/clio, ~/.local/share/clio y la carpeta de runtime
  clio run [--workspace DIR] [--max-steps N] [--allow-broad] [--debug] [--harness pi|builtin] \"instrucción\"
  clio diff <id>                              qué cambió el agente, contra el original
  clio apply <id> [--force]                   pone los cambios en el original (pregunta antes)
  clio undo <id> [--force]                    devuelve el original a como estaba antes de apply
  clio runs [--all] [--json] | clio show <id> [--json]
  clio vault start|stop|status|put NAME --dest TOOL:DOMAIN|list|revoke NAME
  clio memory search|list|show|remember|promote|archive|feedback|export|stats
                                              lo que Clio recuerda (clio memory --help)
  clio doctor [--json]                        revisa permisos, clave, sockets y el equipo
  clio --version                              la versión
  clio pi doctor|install|sessions [--json]    Pi (el arnés del agente): estado, instalación, sesiones

Sin color: NO_COLOR=1 o TERM=dumb. Debug: --debug, CLIO_DEBUG=1 o Ctrl-D en la interfaz. Más: docs/uso-local.md";

fn id_arg(args: Args) -> Res<String> {
    let mut rest = args.rest()?;
    if rest.len() != 1 {
        return Err("falta el id del run (mira `clio runs`)".into());
    }
    Ok(rest.remove(0))
}

fn dispatch(argv: Vec<String>) -> Res<i32> {
    let mut it = argv.into_iter().peekable();
    // `clio` alone, or with only its own flags, opens the interface.
    if it
        .peek()
        .is_none_or(|a| matches!(a.as_str(), "--debug" | "--harness"))
    {
        let mut args = Args::new(it.collect());
        let dbg = args.flag("--debug") || debug::env_enabled();
        let harness = args.value("--harness")?;
        if !args.rest()?.is_empty() {
            return Err("--debug y --harness van solos (abren la interfaz) o después de un comando: clio run --debug \"...\"".into());
        }
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            let dirs = Dirs::from_env()?;
            let h = chat::resolve(harness.as_deref(), &dirs)?;
            return tui::run(dirs, dbg, h);
        }
        println!("{USAGE}");
        return Ok(0);
    }
    let Some(cmd) = it.next() else {
        return Ok(0);
    };
    let mut args = Args::new(it.collect());
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(0);
    }
    if matches!(cmd.as_str(), "-V" | "--version" | "version") {
        println!("clio {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if !matches!(
        cmd.as_str(),
        "init"
            | "run"
            | "diff"
            | "apply"
            | "undo"
            | "runs"
            | "show"
            | "vault"
            | "doctor"
            | "memory"
            | "pi"
            | "toolhost"
            | "pi-net"
    ) {
        eprintln!("clio: comando desconocido {cmd:?}\n\n{USAGE}");
        return Ok(2);
    }
    // Internal: the tool host of a Pi session, started by `clio` from its own executable.
    if cmd == "toolhost" {
        // The memory tools exist only here: the loose `clio-toolhost` binary has no memory.
        let memory = |data: &std::path::Path, model: &[std::path::PathBuf]| {
            // The project comes from `clio` (which decided it from the person's folder).
            let project = std::env::var("CLIO_MEMORY_PROJECT")
                .ok()
                .and_then(clio_brain::project::checked);
            let mut port = clio_brain::memory_port::LinkPort::new(data, model).in_project(project);
            // Only when `clio` started this service with Jev on: it then decides what becomes of
            // each proposal; without it every proposal waits for the person.
            if let Some(judge) = cli::jev::judge_from_env() {
                port = port.with_judge(judge);
            }
            Ok(std::sync::Arc::new(port) as std::sync::Arc<dyn clio_toolhost::MemoryPort>)
        };
        return clio_toolhost::cli::run_with(args.into_raw(), &memory).map(|()| 0);
    }
    // Internal: inside Pi's jail, the forwarder to the egress proxy that starts Pi (ADR 0009).
    if cmd == "pi-net" {
        return clio_brain::pi::net::forward_main(args.into_raw());
    }
    let dirs = Dirs::from_env()?;
    match cmd.as_str() {
        "init" => {
            args.rest()?;
            home::init(&dirs)?;
        }
        "run" => {
            let workspace = args.value("--workspace")?.map_or_else(
                || std::env::current_dir().map_err(|e| e.to_string()),
                |w| Ok(w.into()),
            )?;
            let max_steps = args
                .value("--max-steps")?
                .map(|n| n.parse::<u32>().map_err(|e| format!("--max-steps: {e}")))
                .transpose()?;
            let allow_broad = args.flag("--allow-broad");
            let dbg = args.flag("--debug") || debug::env_enabled();
            // Pi when it is installed (the bench: 57/66 against 55/66 for the builtin loop, and no
            // JSON-in-text protocol failures), the builtin loop otherwise or with --harness builtin.
            let harness = args.value("--harness")?;
            let instruction = args.rest()?.join(" ");
            if instruction.trim().is_empty() {
                return Err(
                    "falta la instrucción: clio run [--workspace DIR] \"qué hacer\"".into(),
                );
            }
            if chat::resolve(harness.as_deref(), &dirs)? == chat::Harness::Pi {
                return chat::run::run(dirs, workspace, &instruction);
            }
            return runs::run(
                &dirs,
                &runs::RunOpts {
                    workspace,
                    max_steps,
                    allow_broad,
                    instruction,
                },
                dbg,
            );
        }
        "diff" => runs::diff(&dirs, &id_arg(args)?)?,
        "apply" => {
            let (force, yes) = (args.flag("--force"), args.flag("--yes"));
            apply::apply(&dirs, &id_arg(args)?, force, yes)?;
        }
        "undo" => {
            let force = args.flag("--force");
            apply::undo(&dirs, &id_arg(args)?, force)?;
        }
        "runs" => {
            let (all, json) = (args.flag("--all"), args.flag("--json"));
            args.rest()?;
            runs::list(&dirs, all, json)?;
        }
        "show" => {
            let json = args.flag("--json");
            runs::show(&dirs, &id_arg(args)?, json)?;
        }
        "vault" => vault::run(&dirs, args.into_raw())?,
        "memory" => return memory::run(&dirs, args.into_raw()),
        "pi" => return pi::run(&dirs, args.into_raw()),
        "doctor" => {
            let json = args.flag("--json");
            args.rest()?;
            return Ok(i32::from(!doctor::doctor(&dirs, json)));
        }
        _ => unreachable!("checked above"),
    }
    Ok(0)
}

fn main() {
    match dispatch(std::env::args().skip(1).collect()) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{}", error_line(&e));
            std::process::exit(1);
        }
    }
}

/// What the person sees when a command fails. An error can carry a file name, and the names in a
/// working copy are whatever the model (or a hostile project) made them: an escape sequence in one
/// would otherwise reach the terminal (write the clipboard, clear the screen).
fn error_line(e: &str) -> String {
    format!("clio: {}", cli::tui::sanitize::multiline(e))
}

#[cfg(test)]
mod tests {
    use super::error_line;

    // S-36 (second audit, finding 2): an error that names a file never carries terminal control.
    #[test]
    fn an_error_with_a_hostile_file_name_reaches_the_terminal_without_escapes() {
        let hostile = "no se pudo leer a\u{1b}]52;c;ZXZpbA==\u{7}\u{1b}[2J.txt: permiso denegado";
        let shown = error_line(hostile);
        assert!(
            !shown.chars().any(|c| c.is_control() && c != '\n'),
            "{shown:?}"
        );
        assert!(shown.starts_with("clio: no se pudo leer a"), "{shown:?}");
        assert!(shown.ends_with("permiso denegado"), "{shown:?}");
        // A plain error is untouched, and a second line stays a second line.
        assert_eq!(
            error_line("falta el id\ncon ayuda"),
            "clio: falta el id\ncon ayuda"
        );
    }

    /// Jev on the relations bench of clio-memory (needs the vault open and Jev on): for each
    /// memory's 5 nearest by Granite, what Jev says, against the pairs a person marked.
    /// `CLIO_BENCH_RELATIONS=<absolute path to clio-memory>/bench/relations cargo test -p clio-brain --bin clio
    /// jev_relations_on_the_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "asks Jev over the network; needs the vault open"]
    fn jev_relations_on_the_bench() {
        use std::collections::{BTreeMap, BTreeSet};
        use std::path::{Path, PathBuf};
        use std::sync::Arc;

        use clio_brain::memory_judge::MemoryJudge;
        use clio_memory::agent::EmbedModel;
        use clio_vault::VaultClient;

        let bench = PathBuf::from(std::env::var("CLIO_BENCH_RELATIONS").unwrap());
        let lines = |p: &Path| -> Vec<serde_json::Value> {
            std::fs::read_to_string(p)
                .unwrap()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        };
        let mems: Vec<(i64, String)> = lines(&bench.join("../embeddings/memories.jsonl"))
            .into_iter()
            .chain(lines(&bench.join("extra.jsonl")))
            .map(|m| {
                (
                    m["id"].as_i64().unwrap(),
                    m["text"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let gold: BTreeMap<(i64, i64), String> = lines(&bench.join("pairs.jsonl"))
            .into_iter()
            .map(|p| {
                let (a, b) = (p["a"].as_i64().unwrap(), p["b"].as_i64().unwrap());
                ((a.min(b), a.max(b)), p["rel"].as_str().unwrap().to_owned())
            })
            .collect();

        let dirs = super::Dirs::from_env().unwrap();
        let home = dirs.home.clone();
        let model = EmbedModel::with_dirs(clio_brain::memory_link::MemoryLink::model_dirs(&home))
            .get()
            .unwrap()
            .unwrap();
        assert!(
            model.model_id().starts_with("granite"),
            "{}",
            model.model_id()
        );
        let texts: Vec<&str> = mems.iter().map(|(_, t)| t.as_str()).collect();
        let v = model.embed(&texts).unwrap();
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let mut cands = BTreeSet::new();
        for i in 0..mems.len() {
            let mut order: Vec<usize> = (0..mems.len()).filter(|&j| j != i).collect();
            order.sort_by(|&x, &y| dot(&v[i], &v[y]).total_cmp(&dot(&v[i], &v[x])));
            for &j in &order[..5] {
                cands.insert((i.min(j), i.max(j)));
            }
        }

        let cfg = super::cli::home::Config::load(&dirs).unwrap();
        let jev = super::cli::jev::client(&dirs, &cfg.jev).expect("Jev is off");
        let socket = super::cli::vault::gateway_socket(&dirs).unwrap();
        let vault = VaultClient::new(&socket);
        let judge = MemoryJudge::new(
            jev,
            Arc::new(move |t| vault.scrub(t).map_err(|e| e.to_string())),
        );

        // Per type Jev says: (said, of them right); per gold type: (gold among the candidates, found).
        let mut said: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        let mut found: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for &(i, j) in &cands {
            // The later memory is the one being saved, as in `propose`.
            let ((a, ta), (b, tb)) = (&mems[i], &mems[j]);
            let (new, old) = if a > b { (ta, tb) } else { (tb, ta) };
            let verdict = judge.relation(new, old);
            let (name, sure) = match verdict {
                Some((r, sure)) => (r.label().to_owned(), sure),
                None => ("ninguna".to_owned(), 0.0),
            };
            let truth = gold.get(&(*a.min(b), *a.max(b)));
            let e = said.entry(name.clone()).or_default();
            e.0 += 1;
            e.1 += u32::from(truth.is_some_and(|t| *t == name));
            if let Some(t) = truth {
                let f = found.entry(t.clone()).or_default();
                f.0 += 1;
                f.1 += u32::from(*t == name);
                println!("m{a}–m{b}: marcado {t}, Jev {name} {sure:.2}");
            } else if name != "ninguna" {
                println!("m{a}–m{b}: sin marcar, Jev {name} {sure:.2}");
            }
        }
        println!("\n{} candidatos", cands.len());
        println!("Jev dice | veces | aciertos (precisión)");
        for (k, (n, ok)) in &said {
            println!(
                "{k} | {n} | {ok} ({:.0} %)",
                100.0 * f64::from(*ok) / f64::from(*n)
            );
        }
        println!("\nmarcado | entre candidatos | Jev lo dice (cobertura)");
        for (k, (n, ok)) in &found {
            println!(
                "{k} | {n} | {ok} ({:.0} %)",
                100.0 * f64::from(*ok) / f64::from(*n)
            );
        }
    }
}
