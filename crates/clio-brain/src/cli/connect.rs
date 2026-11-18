//! `clio connect opencode|claude|anthropic-api [--model M] [--new-key]`: makes a connector the
//! active one. The key goes into the vault (`model:<domain>`) and `config.json` says which
//! connector and model are used. Nothing is changed until every check that can fail has passed,
//! and no message carries the key.

use std::io::{BufRead, Write};

use clio_brain::agent::{CommandModel, Model, Msg};
use clio_brain::connector::{Connector, by_name};
use clio_brain::pi::launch::Secret;
use serde_json::Value;

use super::home::{Config, Dirs, lacks_bridge, names_builtin_adapter};
use super::{Args, Res, io_err, usage, vault, write_private};

const CLAUDE_WARNING: &str = "Clio usa tu suscripción a través de Claude Code (oficial). El token sale de: claude setup-token";

pub fn run(dirs: &Dirs, args: Vec<String>) -> Res<i32> {
    let mut args = Args::new(args);
    let new_key = args.flag("--new-key");
    let model_arg = args.value("--model")?;
    let rest = args.rest()?;
    let [name] = rest.as_slice() else {
        return Err(
            "uso: clio connect opencode|claude|anthropic-api [--model M] [--new-key]".into(),
        );
    };
    let c = by_name(name).ok_or_else(|| {
        format!("conector desconocido {name:?}: usa clio connect opencode|claude|anthropic-api")
    })?;
    let model = model_arg.unwrap_or_else(|| c.default_model.to_owned());

    vault::unlocked(dirs)?;
    // Read now, so a broken config stops us before the vault changes; its provider is what we
    // are about to replace, so it may be anything.
    let cfg = Config::load_with_provider(dirs, c.provider)?;
    // Without the bridge the subscription cannot be used at all: nothing is stored.
    if lacks_bridge(dirs, c) {
        return Err("falta el puente de Claude (pi-claude-agent-sdk): clio pi install".into());
    }

    let mut key = None;
    if new_key || !vault::has_model_key(dirs, c) {
        let (value, old_file) = ask_key(dirs, c)?;
        vault::put(dirs, c.secret, &format!("model:{}", c.domain), &value)?;
        println!(
            "llave de {} guardada en el baúl (model:{})",
            c.name, c.domain
        );
        if let Some(f) = old_file
            && ask(&format!("¿Borro {}? [s/N]", f.display()))?
        {
            std::fs::remove_file(&f).map_err(|e| io_err(&f, &e))?;
            println!("borrado {}", f.display());
        }
        key = Some(Secret::new(value));
    } else if c.name == "opencode" {
        let f = dirs.config.join("opencode-api-key");
        if f.exists() {
            println!(
                "la llave ya está en el baúl; {} ya no se usa: rm {}",
                f.display(),
                f.display()
            );
        }
    }

    switch_config(dirs, c, &model)?;
    println!("modelo activo: {} / {model}", c.name);
    if !names_builtin_adapter(&cfg.model.command) {
        println!(
            "model.command es un programa tuyo: recibirá el modelo de {} pero no su llave",
            c.name
        );
    }
    if !usage::window_for(dirs, c.provider, &model).1 {
        println!("ventana de contexto desconocida para {model}");
    }

    // The probe always goes through the connector's own adapter, whatever model.command says.
    let key = match key {
        Some(k) => k,
        None => match vault::model_key(dirs, c) {
            Ok(k) => k,
            Err(e) => {
                println!("guardado, pero {} no respondió: {e}", c.name);
                return Ok(0);
            }
        },
    };
    match probe(dirs, cfg, c, &model, &key) {
        Ok(()) => println!("conectado: {} / {model} respondió", c.name),
        Err(e) => println!("guardado, pero {} no respondió: {e}", c.name),
    }
    Ok(0)
}

/// The key to store, and the old key file to offer deleting when it was the source.
fn ask_key(dirs: &Dirs, c: &Connector) -> Res<(String, Option<std::path::PathBuf>)> {
    if c.name == "claude" {
        println!("{CLAUDE_WARNING}");
        let v = vault::read_secret("token")?.trim().to_owned();
        if !v.starts_with(c.key_prefix) {
            return Err(format!(
                "eso no es un token de suscripción (debe empezar por {}): nada se guardó. Sale de: claude setup-token",
                c.key_prefix
            ));
        }
        return Ok((v, None));
    }
    if c.name == "anthropic-api" {
        let v = vault::read_secret("llave de la API de Anthropic")?
            .trim()
            .to_owned();
        if !v.starts_with(c.key_prefix) {
            return Err(format!(
                "eso no es una llave de la API de Anthropic (debe empezar por {}): nada se guardó",
                c.key_prefix
            ));
        }
        return Ok((v, None));
    }
    // opencode: the variable or the old file may already hold the key.
    let file = dirs.config.join("opencode-api-key");
    let from_env = std::env::var(c.key_env).ok().map(|v| v.trim().to_owned());
    let from_file = std::fs::read_to_string(&file)
        .ok()
        .map(|v| v.trim().to_owned());
    if let Some(v) = from_env.filter(|v| !v.is_empty())
        && ask(&format!("¿Tomo la llave de {}? [s/N]", c.key_env))?
    {
        return Ok((v, None));
    }
    if let Some(v) = from_file.filter(|v| !v.is_empty())
        && ask(&format!("¿Tomo la llave de {}? [s/N]", file.display()))?
    {
        return Ok((v, Some(file)));
    }
    let v = vault::read_secret("llave de opencode")?.trim().to_owned();
    if v.is_empty() {
        return Err("no recibí ninguna llave: nada se guardó".into());
    }
    Ok((v, None))
}

/// A yes/no question on stdin (tests pipe it): only `s` is yes.
fn ask(question: &str) -> Res<bool> {
    print!("{question} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(matches!(
        line.trim().to_lowercase().as_str(),
        "s" | "si" | "sí"
    ))
}

/// Sets `model.provider` and `model.model` (remembered as that connector's last model).
fn switch_config(dirs: &Dirs, c: &Connector, model: &str) -> Res<()> {
    edit_model(dirs, |m| set_model(m, c.provider, model))
}

/// The connector and its model in `model`; the model is also remembered as the one last chosen
/// for that connector (`model.last`), for when the person comes back to it.
pub(crate) fn set_model(m: &mut serde_json::Map<String, Value>, provider: &str, model: &str) {
    m.insert("provider".into(), provider.into());
    m.insert("model".into(), model.into());
    let last = m
        .entry("last")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !last.is_object() {
        *last = Value::Object(serde_json::Map::new());
    }
    last[provider] = model.into();
}

/// Changes the `model` section of `config.json` with `f` and keeps every other field, atomically
/// and 0600. The values put in must already be checked: what `clio connect` and the chat's
/// commands use to save a choice.
pub(crate) fn edit_model(
    dirs: &Dirs,
    f: impl FnOnce(&mut serde_json::Map<String, Value>),
) -> Res<()> {
    let path = dirs.config_file();
    let text = std::fs::read_to_string(&path).map_err(|e| io_err(&path, &e))?;
    let mut v: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let m = v
        .get_mut("model")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("{}: falta \"model\"", path.display()))?;
    f(m);
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    write_private(&tmp, out.as_bytes())?;
    std::fs::rename(&tmp, &path).map_err(|e| io_err(&path, &e))
}

/// One short call through the connector's own adapter. A failure is a message, not a refusal.
fn probe(dirs: &Dirs, mut cfg: Config, c: &Connector, model: &str, key: &Secret) -> Res<()> {
    cfg.model.provider = c.provider.into();
    cfg.model.command = Vec::new(); // the connector's adapter, not the person's program
    let cmd = cfg.model_command(dirs)?;
    let (program, args) = cmd.split_first().ok_or("sin adaptador")?;
    let mut m = CommandModel::new(program, args, model).with_key(Secret::new(key.expose()));
    m.max_tokens = 16;
    m.backoff_ms = Vec::new();
    // Claude Code starts a large binary and signs in before it asks: give it room. On the
    // deadline the adapter is stopped with SIGTERM and stops Claude Code itself.
    m.timeout = std::time::Duration::from_secs(if c.pi_extension.is_some() { 90 } else { 30 });
    let msg = Msg {
        role: "user".into(),
        content: "Responde solo: ok".into(),
    };
    m.complete(&[msg])
        .map(|_| ())
        .map_err(|e| e.replace(key.expose(), "***"))
}
