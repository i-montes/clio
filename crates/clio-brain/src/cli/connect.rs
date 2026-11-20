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

const CLAUDE_WARNING: &str = "Clio uses your subscription through Claude Code (official). Paste a token from `claude setup-token`, or press Enter to sign in now.";
const CLAUDE_BEFORE: &str = "Sign in in the browser. When Claude shows the token, copy it and press Ctrl-C, then paste it.\nTo leave without signing in: Ctrl-C, then Enter.";
const CLAUDE_ASK: &str =
    "Paste the token and press Enter (an empty line leaves without connecting).";

pub fn run(dirs: &Dirs, args: Vec<String>) -> Res<i32> {
    let mut args = Args::new(args);
    let new_key = args.flag("--new-key");
    let model_arg = args.value("--model")?;
    let rest = args.rest()?;
    let [name] = rest.as_slice() else {
        return Err(
            "usage: clio connect opencode|claude|anthropic-api [--model M] [--new-key]".into(),
        );
    };
    let c = by_name(name).ok_or_else(|| {
        format!("unknown connector {name:?}: use clio connect opencode|claude|anthropic-api")
    })?;
    let model = model_arg.unwrap_or_else(|| c.default_model.to_owned());

    vault::unlocked(dirs)?;
    // Read now, so a broken config stops us before the vault changes; its provider is what we
    // are about to replace, so it may be anything.
    let cfg = Config::load_with_provider(dirs, c.provider)?;
    // Without the bridge the subscription cannot be used at all: nothing is stored.
    if lacks_bridge(dirs, c) {
        return Err("the Claude bridge (pi-claude-agent-sdk) is missing: clio pi install".into());
    }

    let mut key = None;
    if new_key || !vault::has_model_key(dirs, c) {
        let (value, old_file) = ask_key(dirs, c)?;
        let value = clio_vault::SecretValue::new(value);
        store_key(dirs, c, &value)?;
        println!(
            "key for {} stored in the vault (model:{})",
            c.name, c.domain
        );
        if let Some(f) = old_file
            && ask(&format!("Delete {}? [y/N]", f.display()))?
        {
            std::fs::remove_file(&f).map_err(|e| io_err(&f, &e))?;
            println!("deleted {}", f.display());
        }
        key = Some(Secret::new(value.expose()));
    } else if c.name == "opencode" {
        let f = dirs.config.join("opencode-api-key");
        if f.exists() {
            println!(
                "the key is already in the vault; {} is no longer used: rm {}",
                f.display(),
                f.display()
            );
        }
    }

    switch_config(dirs, c, &model)?;
    println!("active model: {} / {model}", c.name);
    if !names_builtin_adapter(&cfg.model.command) {
        println!(
            "model.command is a program of yours: it will receive the model of {} but not its key",
            c.name
        );
    }
    if !usage::window_for(dirs, c.provider, &model).1 {
        println!("unknown context window for {model}");
    }

    // The probe always goes through the connector's own adapter, whatever model.command says.
    let key = match key {
        Some(k) => k,
        None => match vault::model_key(dirs, c) {
            Ok(k) => k,
            Err(e) => {
                println!("stored, but {} did not answer: {e}", c.name);
                return Ok(0);
            }
        },
    };
    match probe(dirs, cfg, c, &model, &key) {
        Ok(()) => println!("connected: {} / {model} answered", c.name),
        Err(e) => println!("stored, but {} did not answer: {e}", c.name),
    }
    Ok(0)
}

/// Puts `value` in the vault as connector `c`'s key, allowed only to `model:<its domain>`: what
/// `clio connect` and `clio setup` store.
pub fn store_key(dirs: &Dirs, c: &Connector, value: &clio_vault::SecretValue) -> Res<()> {
    vault::put(dirs, c.secret, &format!("model:{}", c.domain), value)
}

/// Claude Code's own sign-in (`setup-token`, the bridge's bundled copy, with Pi's home so
/// `~/.claude` is not touched), then the token it showed, pasted back. `before` says what to do
/// (and how to leave), `ask` asks for the token. `None`: the sign-in could not start, was left
/// (an empty line), or what was pasted is not a subscription token.
pub fn claude_sign_in(dirs: &Dirs, before: &str, ask: &str) -> Option<clio_vault::SecretValue> {
    let claude = super::home::claude_code(dirs).ok()?;
    let prefix = by_name("claude")?.key_prefix;
    let mut cmd = std::process::Command::new(&claude);
    cmd.arg("setup-token")
        .env("HOME", super::pi::paths(dirs).home_dir());
    let mut line = sign_in_with(cmd, before, ask)?;
    let mut key = line.trim().to_owned();
    clio_vault::wipe(&mut line);
    if key.starts_with(prefix) {
        Some(clio_vault::SecretValue::new(key))
    } else {
        clio_vault::wipe(&mut key);
        None
    }
}

/// Runs the sign-in `cmd` on the terminal with a way out Clio owns, whatever the child does with
/// the terminal: Ctrl-C (or Clio being told to stop) ends it. Then the terminal is put back as it
/// was, the screen and its scrollback are cleared (the token was on them), and the token is asked
/// for with no echo; an empty line leaves. Only a child that could not start asks nothing.
fn sign_in_with(mut cmd: std::process::Command, before: &str, ask: &str) -> Option<String> {
    use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let stdin = std::io::stdin();
    let saved = tcgetattr(&stdin).ok();
    println!("{before}");
    let got = Arc::new(AtomicUsize::new(0));
    let register = |sigs: &[i32]| -> Vec<_> {
        sigs.iter()
            .filter_map(|&s| signal_hook::flag::register_usize(s, got.clone(), s as usize).ok())
            .collect()
    };
    let unregister = |ids: Vec<signal_hook::SigId>| {
        for id in ids {
            signal_hook::low_level::unregister(id);
        }
    };
    let (int, stop) = (register(&[SIGINT]), register(&[SIGTERM, SIGHUP]));
    let Ok(mut child) = cmd.spawn() else {
        unregister(int);
        unregister(stop);
        return None;
    };
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        // Claude Code's raw mode clears ISIG, and then Ctrl-C makes no signal: set it again.
        s.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                if let Ok(mut t) = tcgetattr(&stdin)
                    && !t.local_modes.contains(LocalModes::ISIG)
                {
                    t.local_modes.insert(LocalModes::ISIG);
                    let _ = tcsetattr(&stdin, OptionalActions::Now, &t);
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
        end_on_signal(&mut child, &got);
        done.store(true, Ordering::Relaxed);
    });
    if let Some(t) = &saved {
        let _ = tcsetattr(&stdin, OptionalActions::Now, t);
    }
    // What a killed full-screen program leaves on: cursor hidden, bracketed paste, kitty keyboard
    // flags, modifyOtherKeys, the mouse. Then the screen, then its scrollback.
    print!(
        "\x1b[?25h\x1b[?2004l\x1b[<u\x1b[>4;0m\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[H\x1b[2J\x1b[3J"
    );
    let _ = std::io::stdout().flush();
    unregister(stop);
    let sig = got.swap(0, Ordering::Relaxed);
    if sig == SIGTERM as usize || sig == SIGHUP as usize {
        unregister(int);
        let _ = signal_hook::low_level::emulate_default_handler(sig as i32);
        return None;
    }
    println!("{ask}");
    // Ctrl-C is still caught while asking: Enter on an empty line is the way to leave.
    let line = vault::read_secret("token").ok();
    unregister(int);
    line.filter(|l| !l.trim().is_empty())
}

/// Waits for `child`; once a signal is noted in `got`, ends it: SIGTERM, then SIGKILL after 2 s.
fn end_on_signal(child: &mut std::process::Child, got: &std::sync::atomic::AtomicUsize) {
    use std::time::{Duration, Instant};
    let tick = Duration::from_millis(50);
    while got.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(tick);
    }
    let _ = rustix::process::kill_process(
        rustix::process::Pid::from_child(child),
        rustix::process::Signal::TERM,
    );
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(tick);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The key to store, and the old key file to offer deleting when it was the source.
fn ask_key(dirs: &Dirs, c: &Connector) -> Res<(String, Option<std::path::PathBuf>)> {
    if c.name == "claude" {
        println!("{CLAUDE_WARNING}");
        let mut v = vault::read_secret("token")?.trim().to_owned();
        // Nothing pasted: the same sign-in the installer offers.
        if v.is_empty()
            && let Some(k) = claude_sign_in(dirs, CLAUDE_BEFORE, CLAUDE_ASK)
        {
            v = k.expose().to_owned();
        }
        if !v.starts_with(c.key_prefix) {
            return Err(format!(
                "that is not a subscription token (it must start with {}): nothing was stored. It comes from: claude setup-token",
                c.key_prefix
            ));
        }
        return Ok((v, None));
    }
    if c.name == "anthropic-api" {
        let v = vault::read_secret("Anthropic API key")?.trim().to_owned();
        if !v.starts_with(c.key_prefix) {
            return Err(format!(
                "that is not an Anthropic API key (it must start with {}): nothing was stored",
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
        && ask(&format!("Take the key from {}? [y/N]", c.key_env))?
    {
        return Ok((v, None));
    }
    if let Some(v) = from_file.filter(|v| !v.is_empty())
        && ask(&format!("Take the key from {}? [y/N]", file.display()))?
    {
        return Ok((v, Some(file)));
    }
    let v = vault::read_secret("opencode key")?.trim().to_owned();
    if v.is_empty() {
        return Err("no key was received: nothing was stored".into());
    }
    Ok((v, None))
}

/// A yes/no question on stdin (tests pipe it): only `y` is yes (`s` is still accepted).
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
        "y" | "yes" | "s" | "si" | "sí"
    ))
}

/// Sets `model.provider` and `model.model` (remembered as that connector's last model).
pub fn switch_config(dirs: &Dirs, c: &Connector, model: &str) -> Res<()> {
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
    edit_section(dirs, "model", false, f)
}

/// [`edit_model`] for any section of `config.json`; `create`: one that is missing starts empty.
pub(crate) fn edit_section(
    dirs: &Dirs,
    key: &str,
    create: bool,
    f: impl FnOnce(&mut serde_json::Map<String, Value>),
) -> Res<()> {
    let path = dirs.config_file();
    let text = std::fs::read_to_string(&path).map_err(|e| io_err(&path, &e))?;
    let mut v: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if create && let Some(o) = v.as_object_mut() {
        o.entry(key)
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
    let m = v
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("{}: \"{key}\" is missing", path.display()))?;
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
    let (program, args) = cmd.split_first().ok_or("no adapter")?;
    let mut m = CommandModel::new(program, args, model).with_key(Secret::new(key.expose()));
    m.max_tokens = 16;
    m.backoff_ms = Vec::new();
    // Claude Code starts a large binary and signs in before it asks: give it room. On the
    // deadline the adapter is stopped with SIGTERM and stops Claude Code itself.
    m.timeout = std::time::Duration::from_secs(if c.pi_extension.is_some() { 90 } else { 30 });
    let msg = Msg {
        role: "user".into(),
        content: "Reply only: ok".into(),
    };
    m.complete(&[msg])
        .map(|_| ())
        .map_err(|e| e.replace(key.expose(), "***"))
}
