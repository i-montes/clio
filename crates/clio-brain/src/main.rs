//! `clio`: use Clio for real on this machine. The agent works on a COPY of your project; you
//! decide what goes back to the original. See `docs/uso-local.md`.

mod cli;

use cli::home::Dirs;
use cli::{Args, Res, apply, doctor, home, runs, vault};

const USAGE: &str = "clio: a personal agent that works on a copy of your project

  clio init                                   create ~/.config/clio, ~/.local/share/clio and the runtime folder
  clio run [--workspace DIR] [--max-steps N] [--allow-broad] \"instruction\"
  clio diff <id>                              what the agent changed, against the original
  clio apply <id> [--force]                   put the changes in the original (asks first)
  clio undo <id> [--force]                    put the original back as it was before apply
  clio runs [--all] | clio show <id>
  clio vault start|stop|status|put NAME --dest TOOL:DOMAIN|list|revoke NAME
  clio doctor                                 check permissions, key, sockets and the host";

fn id_arg(args: Args) -> Res<String> {
    let mut rest = args.rest()?;
    if rest.len() != 1 {
        return Err("expected exactly one run id".into());
    }
    Ok(rest.remove(0))
}

fn dispatch(argv: Vec<String>) -> Res<i32> {
    let mut it = argv.into_iter();
    let Some(cmd) = it.next() else {
        println!("{USAGE}");
        return Ok(2);
    };
    let mut args = Args::new(it.collect());
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(0);
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
            let instruction = args.rest()?.join(" ");
            if instruction.trim().is_empty() {
                return Err(
                    "missing the instruction: clio run [--workspace DIR] \"what to do\"".into(),
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
            let all = args.flag("--all");
            args.rest()?;
            runs::list(&dirs, all)?;
        }
        "show" => runs::show(&dirs, &id_arg(args)?)?,
        "vault" => vault::run(&dirs, args.into_raw())?,
        "doctor" => return Ok(i32::from(!doctor::doctor(&dirs))),
        other => return Err(format!("unknown command {other:?}\n\n{USAGE}")),
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
