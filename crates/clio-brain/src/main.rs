//! `clio`: use Clio for real on this machine. The agent works on a COPY of your project; you
//! decide what goes back to the original. See `docs/uso-local.md`.

mod cli;

use std::io::IsTerminal;

use cli::home::Dirs;
use cli::{Args, Res, apply, debug, doctor, home, memory, runs, tui, vault};

const USAGE: &str = "clio: un agente personal que trabaja en una copia de tu proyecto

  clio [--debug]                              abre la interfaz (en una terminal); --debug o CLIO_DEBUG=1: modo debug
  clio init                                   crea ~/.config/clio, ~/.local/share/clio y la carpeta de runtime
  clio run [--workspace DIR] [--max-steps N] [--allow-broad] [--debug] \"instrucción\"
  clio diff <id>                              qué cambió el agente, contra el original
  clio apply <id> [--force]                   pone los cambios en el original (pregunta antes)
  clio undo <id> [--force]                    devuelve el original a como estaba antes de apply
  clio runs [--all] [--json] | clio show <id> [--json]
  clio vault start|stop|status|put NAME --dest TOOL:DOMAIN|list|revoke NAME
  clio memory search|list|show|remember|promote|archive|feedback|export|stats
                                              lo que Clio recuerda (clio memory --help)
  clio doctor [--json]                        revisa permisos, clave, sockets y el equipo

Sin color: NO_COLOR=1 o TERM=dumb. Debug: --debug, CLIO_DEBUG=1 o Ctrl-D en la interfaz. Más: docs/uso-local.md";

fn id_arg(args: Args) -> Res<String> {
    let mut rest = args.rest()?;
    if rest.len() != 1 {
        return Err("falta el id del run (mira `clio runs`)".into());
    }
    Ok(rest.remove(0))
}

fn dispatch(argv: Vec<String>) -> Res<i32> {
    let mut it = argv.into_iter();
    let Some(cmd) = it.next() else {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return tui::run(Dirs::from_env()?, debug::env_enabled());
        }
        println!("{USAGE}");
        return Ok(0);
    };
    // `clio --debug` alone opens the TUI with debug on.
    if cmd == "--debug" {
        let rest: Vec<String> = it.collect();
        if !rest.is_empty() {
            return Err("--debug va solo (abre la interfaz) o después de un comando: clio run --debug \"...\"".into());
        }
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return tui::run(Dirs::from_env()?, true);
        }
        println!("{USAGE}");
        return Ok(0);
    }
    let mut args = Args::new(it.collect());
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(0);
    }
    if !matches!(
        cmd.as_str(),
        "init" | "run" | "diff" | "apply" | "undo" | "runs" | "show" | "vault" | "doctor" | "memory"
    ) {
        eprintln!("clio: comando desconocido {cmd:?}\n\n{USAGE}");
        return Ok(2);
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
            let instruction = args.rest()?.join(" ");
            if instruction.trim().is_empty() {
                return Err(
                    "falta la instrucción: clio run [--workspace DIR] \"qué hacer\"".into(),
                );
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
            eprintln!("clio: {e}");
            std::process::exit(1);
        }
    }
}
