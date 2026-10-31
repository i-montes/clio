//! Debug mode (`--debug`, `CLIO_DEBUG=1`, Ctrl-D in the TUI). It adds a raw but SAFE view of a
//! run: the request and answer of each model call, every gateway decision with its full rule and
//! reason, how the context was built, times and retries.
//!
//! Rules: everything that is shown or written passes through the `Scrubber` first (it knows the
//! model's API key; decoys are caught plain, in base64 and in URL form) and then through the
//! terminal sanitizer; `debug.log` is created with mode 0600 only while debug is on; without
//! debug nothing extra is written. `Tee` is the one place all this happens: it sits between the
//! agent and whoever watches it (the screen, the progress lines) and also keeps the numbers for
//! `usage.json`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clio_brain::agent::{ContextReport, DecisionDebug, Event, Exchange, Msg, Observer};
use clio_gateway::Outcome;
use clio_vault::Scrubber;
use serde_json::{Value, json};

use super::home::Dirs;
use super::tui::sanitize;
use super::usage::RunUsage;
use super::{Res, set_mode};

/// Longest text of one field in `debug.log` and in the debug view, in characters.
pub const LOG_FIELD_MAX: usize = 8_000;
pub const VIEW_FIELD_MAX: usize = 1_500;

/// Whether debug is on, and where its lines go besides `debug.log`.
#[derive(Clone)]
pub struct DebugCtl {
    pub on: Arc<AtomicBool>,
    /// Print the debug lines (sanitized) on stderr: `clio run --debug`.
    pub stderr: bool,
}

impl DebugCtl {
    pub fn new(on: bool, stderr: bool) -> Self {
        Self {
            on: Arc::new(AtomicBool::new(on)),
            stderr,
        }
    }

    pub fn enabled(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    pub fn set(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }
}

/// `CLIO_DEBUG` set to anything but empty, `0`, `false` or `no`.
pub fn env_enabled() -> bool {
    std::env::var("CLIO_DEBUG").is_ok_and(|v| {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no"
        )
    })
}

/// A scrubber that knows the model's API key (from `OPENCODE_API_KEY` or the key file), so the
/// key can never reach the debug view, the log or stderr. The key is read into memory only here.
pub fn scrubber(dirs: &Dirs) -> Scrubber {
    let s = Scrubber::default();
    let from_env = std::env::var("OPENCODE_API_KEY").ok();
    let key = from_env
        .filter(|k| !k.trim().is_empty())
        .or_else(|| std::fs::read_to_string(dirs.key_file()).ok());
    if let Some(k) = key {
        s.add("model-api-key", k.trim());
    }
    s
}

/// `text` cut to `max` characters, with how much was left out.
pub fn cut(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}… (faltan {} caracteres)", n - max)
}

fn outcome_code(o: Outcome) -> &'static str {
    match o {
        Outcome::Allow => "allow",
        Outcome::Ask => "ask",
        Outcome::Deny => "deny",
    }
}

fn ctx_json(c: &ContextReport) -> Value {
    json!({
        "call": c.call,
        "prompt_version": c.prompt_version,
        "system": c.system,
        "objective": c.objective,
        "tools": c.tools,
        "conversation": c.conversation,
        "turns": c.turns,
        "external": c.external.iter().map(|i| json!({"name": i.name, "tokens": i.tokens})).collect::<Vec<_>>(),
        "tool_outputs": c.tool_outputs,
        "total": c.total(),
    })
}

/// One event as a JSON line for `debug.log`; long text is cut.
pub fn event_json(e: &Event, at_ms: u64) -> Value {
    let mut v = match e {
        Event::Started { level } => json!({"kind": "started", "level": format!("{level:?}")}),
        Event::Step { n } => json!({"kind": "step", "n": n}),
        Event::ProtocolError { reason } => {
            json!({"kind": "protocol_error", "reason": cut(reason, LOG_FIELD_MAX)})
        }
        Event::Final => json!({"kind": "final"}),
        Event::Tool {
            name,
            target,
            outcome,
            rule,
            ran,
            result_chars,
            tainted,
        } => json!({
            "kind": "tool", "name": name, "target": cut(target, 500),
            "outcome": outcome_code(*outcome), "rule": rule, "ran": ran,
            "result_chars": result_chars, "tainted": tainted,
        }),
        Event::Context(c) => {
            let mut v = ctx_json(c);
            v["kind"] = json!("context");
            v
        }
        Event::Usage(u) => json!({
            "kind": "usage", "call": u.call, "input_tokens": u.input_tokens,
            "output_tokens": u.output_tokens, "cost_micro_usd": u.cost_micro,
            "model_ms": u.model_ms, "at_ms": u.at_ms, "retries": u.retries,
        }),
        Event::Exchange(x) => json!({
            "kind": "exchange", "call": x.call, "messages": x.messages,
            "request_chars": x.request_chars,
            "request": x.request.iter().map(|m| json!({"role": m.role, "content": cut(&m.content, LOG_FIELD_MAX)})).collect::<Vec<_>>(),
            "response": cut(&x.response, LOG_FIELD_MAX),
        }),
        Event::Decision(d) => json!({
            "kind": "decision", "n": d.n, "tool": d.tool, "args": cut(&d.args, LOG_FIELD_MAX),
            "outcome": outcome_code(d.outcome), "rule": d.rule, "reason": cut(&d.reason, LOG_FIELD_MAX),
            "tool_ms": d.tool_ms,
        }),
    };
    v["at_ms"] = json!(at_ms);
    v
}

/// A human line for stderr. Sanitized by the caller.
fn stderr_line(e: &Event) -> Option<String> {
    Some(match e {
        Event::Context(c) => format!(
            "contexto #{}: {} tok aprox. (sistema {} {}, objetivo {}, herramientas {}, conversación {} en {} turnos, externo {} en {} lecturas, salidas {})",
            c.call,
            c.total(),
            c.system,
            c.prompt_version,
            c.objective,
            c.tools,
            c.conversation,
            c.turns,
            c.external_tokens(),
            c.external.len(),
            c.tool_outputs
        ),
        Event::Usage(u) => format!(
            "llamada #{}: {} ms, entrada {} tok, salida {} tok, costo {} micro-USD, reintentos {}",
            u.call, u.model_ms, u.input_tokens, u.output_tokens, u.cost_micro, u.retries
        ),
        Event::Exchange(x) => format!(
            "petición #{} ({} mensajes, {} car.):\n{}\nrespuesta:\n{}",
            x.call,
            x.messages,
            x.request_chars,
            x.request
                .iter()
                .map(|m| format!("  [{}] {}", m.role, cut(&m.content, VIEW_FIELD_MAX)))
                .collect::<Vec<_>>()
                .join("\n"),
            cut(&x.response, VIEW_FIELD_MAX)
        ),
        Event::Decision(d) => format!(
            "pasarela #{}: {} {} -> {} (regla {}) {} ms: {}",
            d.n,
            d.tool,
            cut(&d.args, 300),
            outcome_code(d.outcome),
            d.rule,
            d.tool_ms,
            d.reason
        ),
        Event::ProtocolError { reason } => format!("error de protocolo: {reason}"),
        _ => return None,
    })
}

/// The event with every piece of text scrubbed.
pub fn scrub_event(s: &Scrubber, e: &Event) -> Event {
    let c = |t: &str| s.scrub(t);
    match e {
        Event::ProtocolError { reason } => Event::ProtocolError { reason: c(reason) },
        Event::Tool {
            name,
            target,
            outcome,
            rule,
            ran,
            result_chars,
            tainted,
        } => Event::Tool {
            name: c(name),
            target: c(target),
            outcome: *outcome,
            rule: c(rule),
            ran: *ran,
            result_chars: *result_chars,
            tainted: *tainted,
        },
        Event::Context(r) => {
            let mut r = r.clone();
            for i in &mut r.external {
                i.name = c(&i.name);
            }
            Event::Context(r)
        }
        Event::Exchange(x) => Event::Exchange(Exchange {
            request: x
                .request
                .iter()
                .map(|m| Msg {
                    role: m.role.clone(),
                    content: c(&m.content),
                })
                .collect(),
            response: c(&x.response),
            ..x.clone()
        }),
        Event::Decision(d) => Event::Decision(DecisionDebug {
            tool: c(&d.tool),
            args: c(&d.args),
            rule: c(&d.rule),
            reason: c(&d.reason),
            ..d.clone()
        }),
        other => other.clone(),
    }
}

/// Between the agent and its watcher: scrubs every event, keeps the numbers for `usage.json` and,
/// while debug is on, writes `debug.log` and optionally prints on stderr.
pub struct Tee {
    pub inner: Box<dyn Observer>,
    pub usage: Arc<Mutex<RunUsage>>,
    pub scrubber: Scrubber,
    pub ctl: DebugCtl,
    pub log_path: PathBuf,
    log: Option<File>,
    started: std::time::Instant,
    write_error: bool,
}

impl Tee {
    pub fn new(
        inner: Box<dyn Observer>,
        usage: Arc<Mutex<RunUsage>>,
        scrubber: Scrubber,
        ctl: DebugCtl,
        log_path: PathBuf,
    ) -> Self {
        Self {
            inner,
            usage,
            scrubber,
            ctl,
            log_path,
            log: None,
            started: std::time::Instant::now(),
            write_error: false,
        }
    }

    /// Opens `debug.log` the first time, born 0600.
    fn log(&mut self) -> Res<&mut File> {
        if self.log.is_none() {
            let f = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&self.log_path)
                .map_err(|e| format!("{}: {e}", self.log_path.display()))?;
            set_mode(&self.log_path, 0o600)?;
            self.log = Some(f);
        }
        self.log.as_mut().ok_or_else(|| "no log".to_owned())
    }

    fn write(&mut self, e: &Event) {
        let at = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        // Scrub the finished line once more: the text of the JSON is what lands on disk.
        let line = self.scrubber.scrub(&event_json(e, at).to_string());
        let ok = self
            .log()
            .and_then(|f| writeln!(f, "{line}").map_err(|x| x.to_string()));
        if let Err(x) = ok
            && !self.write_error
        {
            self.write_error = true;
            eprintln!(
                "clio: no se pudo escribir debug.log: {}",
                sanitize::line(&x)
            );
        }
        if self.ctl.stderr
            && let Some(t) = stderr_line(e)
        {
            for l in sanitize::multiline(&t).lines() {
                eprintln!("[debug] {l}");
            }
        }
    }
}

impl Observer for Tee {
    fn event(&mut self, e: &Event) {
        let e = scrub_event(&self.scrubber, e);
        if let Ok(mut u) = self.usage.lock() {
            u.record(&e);
        }
        if self.ctl.enabled() {
            self.write(&e);
        }
        self.inner.event(&e);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clio_canaries::Canaries;

    struct Sink(Arc<Mutex<Vec<Event>>>);
    impl Observer for Sink {
        fn event(&mut self, e: &Event) {
            self.0.lock().unwrap().push(e.clone());
        }
    }

    fn exchange(text: &str) -> Event {
        Event::Exchange(Exchange {
            call: 1,
            messages: 2,
            request_chars: 10,
            request: vec![Msg {
                role: "user".into(),
                content: text.into(),
            }],
            response: text.into(),
        })
    }

    #[test]
    fn cut_says_how_much_is_missing() {
        assert_eq!(cut("abc", 5), "abc");
        assert_eq!(cut("abcdef", 4), "abcd… (faltan 2 caracteres)");
    }

    #[test]
    fn env_values() {
        // Parsing only: the variable itself is not touched.
        for (v, on) in [
            ("1", true),
            ("true", true),
            ("0", false),
            ("", false),
            ("No", false),
        ] {
            let r = !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no"
            );
            assert_eq!(r, on, "{v}");
        }
    }

    #[test]
    fn the_tee_scrubs_logs_only_when_on_and_the_log_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("debug.log");
        let canaries = Canaries::from_seed(3);
        let decoy = canaries.value("github").unwrap().to_owned();
        let scrubber = Scrubber::default();
        scrubber.add("github", &decoy);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let usage = Arc::new(Mutex::new(RunUsage::default()));
        let ctl = DebugCtl::new(false, false);
        let mut tee = Tee::new(
            Box::new(Sink(seen.clone())),
            usage,
            scrubber,
            ctl.clone(),
            log.clone(),
        );
        // Off: the watcher gets the event scrubbed, nothing is written.
        tee.event(&exchange(&format!("token {decoy}")));
        assert!(!log.exists(), "no debug.log without debug");
        // On: written, 0600, no decoy in any form.
        ctl.set(true);
        tee.event(&exchange(&format!("token {decoy}")));
        tee.event(&Event::ProtocolError {
            reason: format!("bad {decoy}"),
        });
        let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 2);
        canaries.assert_clean("debug.log", &text);
        for line in text.lines() {
            serde_json::from_str::<Value>(line).unwrap();
        }
        let got = format!("{:?}", seen.lock().unwrap());
        canaries.assert_clean("events for the screen", &got);
    }
}
