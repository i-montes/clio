use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use clio_core::Clock;
use serde_json::{Value, json};

use crate::wire::{Answers, Questions};

const MAX_STATE_BYTES: usize = 32 * 1024;
const MAX_REPLY_BYTES: u64 = 1 << 20;
const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone)]
pub struct Config {
    /// `jev-latest`: TypeSafe retires old versions, so a fixed one ends up failing. The version
    /// that answered comes back in every reply (see [`Jev::model_changed`]).
    pub model: String,
    /// Longest a call may take. The caller is waiting for it.
    pub timeout: Duration,
    /// This many failures in a row open the breaker.
    pub breaker_fails: u32,
    /// How long an open breaker refuses calls before one more try.
    pub breaker_open: Duration,
    /// Input tokens per day (UTC); Jev bills the input.
    pub daily_tokens: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "jev-latest".into(),
            timeout: Duration::from_secs(3),
            breaker_fails: 3,
            breaker_open: Duration::from_secs(60),
            daily_tokens: 2_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevError {
    /// The breaker is open: no call was made.
    Open,
    /// The day's token budget is spent: no call was made.
    Budget,
    /// The key could not be had (vault locked or stopped, no such secret).
    Key(String),
    /// The call failed: network, timeout, HTTP error.
    Call(String),
    /// Jev answered something that does not fit what was asked, or the request was refused here.
    Bad(String),
}

impl std::fmt::Display for JevError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "Jev no responde (disyuntor abierto)"),
            Self::Budget => write!(f, "presupuesto diario de Jev agotado"),
            Self::Key(e) => write!(f, "sin clave de Jev: {e}"),
            Self::Call(e) => write!(f, "falló la llamada a Jev: {e}"),
            Self::Bad(e) => write!(f, "respuesta de Jev no válida: {e}"),
        }
    }
}

impl std::error::Error for JevError {}

/// How a request reaches Jev. A fake in tests; [`Process`] for real.
pub trait Transport: Send + Sync {
    fn call(&self, key: &str, body: &Value, timeout: Duration) -> Result<Value, String>;
}

/// The Python adapter, run isolated (`-I`) with nothing of the environment but its own settings.
pub struct Process {
    pub python: PathBuf,
    pub script: PathBuf,
    /// Only for tests: a loopback URL in place of `https://api.typesafe.ai`.
    pub base_url: Option<String>,
}

impl Transport for Process {
    fn call(&self, key: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
        let mut c = Command::new(&self.python);
        c.arg("-I")
            .arg(&self.script)
            .env_clear()
            .env("JEV_TIMEOUT_SECONDS", timeout.as_secs_f64().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(b) = &self.base_url {
            c.env("JEV_BASE_URL", b);
        }
        let mut child = c
            .spawn()
            .map_err(|e| format!("{}: {e}", self.python.display()))?;
        let request = json!({"key": key, "body": body}).to_string();
        if let Some(mut si) = child.stdin.take() {
            // The adapter reads all of it before it does anything; a failed write shows up below.
            let _ = si.write_all(request.as_bytes());
        }
        let read = |s: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut out = Vec::new();
                if let Some(s) = s {
                    let _ = s.take(MAX_REPLY_BYTES + 1).read_to_end(&mut out);
                }
                out
            })
        };
        let out = read(child.stdout.take().map(|s| Box::new(s) as _));
        let err = read(child.stderr.take().map(|s| Box::new(s) as _));
        // A little over the adapter's own limit, so it reports its own timeout first.
        let deadline = Instant::now() + timeout + Duration::from_millis(1500);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let (out, err) = (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        );
        let Some(status) = status else {
            return Err("tiempo agotado".into());
        };
        if !status.success() {
            let msg = String::from_utf8_lossy(&err).replace(key, "***");
            return Err(msg.trim().chars().take(300).collect());
        }
        if out.len() as u64 > MAX_REPLY_BYTES {
            return Err("respuesta demasiado grande".into());
        }
        serde_json::from_slice(&out).map_err(|_| "la respuesta no es JSON".to_owned())
    }
}

#[derive(Default)]
struct State {
    fails: u32,
    open_until: i64,
    day: i64,
    tokens: u64,
    first_model: Option<String>,
    last_model: Option<String>,
}

pub struct Jev {
    cfg: Config,
    transport: Box<dyn Transport>,
    key: Box<dyn Fn() -> Result<String, String> + Send + Sync>,
    clock: Arc<dyn Clock>,
    st: Mutex<State>,
}

impl Jev {
    /// `key` is asked for on every call (from the vault): it is not kept here.
    pub fn new(
        cfg: Config,
        transport: Box<dyn Transport>,
        key: Box<dyn Fn() -> Result<String, String> + Send + Sync>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            cfg,
            transport,
            key,
            clock,
            st: Mutex::new(State::default()),
        }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Asks `questions` about `state`. Any error means "no answer": the caller goes on with its
    /// own rules.
    pub fn ask(&self, state: &Value, questions: &Questions) -> Result<Answers, JevError> {
        if questions.is_empty() {
            return Err(JevError::Bad("no questions".into()));
        }
        if state.to_string().len() > MAX_STATE_BYTES {
            return Err(JevError::Bad("the state is too large".into()));
        }
        let now = self.clock.now_ms();
        {
            let mut s = self.st();
            if s.open_until != 0 && now >= s.open_until {
                // One more try after the pause: a single failure closes it again.
                s.open_until = 0;
                s.fails = self.cfg.breaker_fails.saturating_sub(1);
            }
            if now < s.open_until {
                return Err(JevError::Open);
            }
            if s.day != now.div_euclid(DAY_MS) {
                s.day = now.div_euclid(DAY_MS);
                s.tokens = 0;
            }
            if s.tokens >= self.cfg.daily_tokens {
                return Err(JevError::Budget);
            }
        }
        let result = self.call(state, questions);
        let mut s = self.st();
        match &result {
            Ok(a) => {
                s.fails = 0;
                s.tokens = s.tokens.saturating_add(a.input_tokens);
                s.first_model.get_or_insert_with(|| a.model.clone());
                s.last_model = Some(a.model.clone());
            }
            Err(_) => {
                s.fails += 1;
                if s.fails >= self.cfg.breaker_fails {
                    let open = i64::try_from(self.cfg.breaker_open.as_millis()).unwrap_or(i64::MAX);
                    s.open_until = now.saturating_add(open).max(1);
                }
            }
        }
        result
    }

    fn call(&self, state: &Value, questions: &Questions) -> Result<Answers, JevError> {
        let key = (self.key)().map_err(JevError::Key)?;
        let body = json!({
            "model": self.cfg.model,
            "state": state,
            "questions": questions.to_json(),
        });
        let reply = self
            .transport
            .call(&key, &body, self.cfg.timeout)
            .map_err(|e| JevError::Call(e.replace(&key, "***")))?;
        Answers::parse(questions, &reply).map_err(JevError::Bad)
    }

    /// The version of the model that answered last (`jev-1.14.0` when asked for `jev-latest`).
    pub fn model(&self) -> Option<String> {
        self.st().last_model.clone()
    }

    /// True when the model that answers is not the one that answered first in this process:
    /// TypeSafe updated it, and the thresholds were set on the other one.
    pub fn model_changed(&self) -> bool {
        let s = self.st();
        s.first_model != s.last_model
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clio_core::FixedClock;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Fake {
        calls: Arc<AtomicU32>,
        model: Arc<Mutex<String>>,
        fail: Arc<Mutex<bool>>,
    }

    impl Transport for Fake {
        fn call(&self, key: &str, body: &Value, _t: Duration) -> Result<Value, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(body["model"], "jev-latest");
            if *self.fail.lock().unwrap() {
                return Err(format!("HTTP 500 with the key {key}"));
            }
            Ok(json!({"model": *self.model.lock().unwrap(),
                "answers": {"x": {"type":"noul","noul":0.7}},
                "usage": {"input_tokens": 300, "output_tokens": 5}}))
        }
    }

    struct Rig {
        jev: Jev,
        clock: Arc<FixedClock>,
        calls: Arc<AtomicU32>,
        model: Arc<Mutex<String>>,
        fail: Arc<Mutex<bool>>,
    }

    fn rig(cfg: Config, key: Result<String, String>) -> Rig {
        let (calls, model, fail) = (
            Arc::new(AtomicU32::new(0)),
            Arc::new(Mutex::new("jev-1.14.0".to_owned())),
            Arc::new(Mutex::new(false)),
        );
        let clock = Arc::new(FixedClock::new(1_000));
        let fake = Fake {
            calls: calls.clone(),
            model: model.clone(),
            fail: fail.clone(),
        };
        let jev = Jev::new(
            cfg,
            Box::new(fake),
            Box::new(move || key.clone()),
            clock.clone(),
        );
        Rig {
            jev,
            clock,
            calls,
            model,
            fail,
        }
    }

    fn q() -> Questions {
        Questions::new().noul("x", "¿Sí?").unwrap()
    }

    fn ask(r: &Rig) -> Result<Answers, JevError> {
        r.jev.ask(&json!({"mensaje_de_la_persona": "hola"}), &q())
    }

    #[test]
    fn a_good_call_returns_checked_answers() {
        let r = rig(Config::default(), Ok("KEY".into()));
        assert_eq!(ask(&r).unwrap().noul("x"), Some(0.7));
        assert_eq!(r.jev.model().as_deref(), Some("jev-1.14.0"));
        assert!(!r.jev.model_changed());
    }

    #[test]
    fn three_failures_open_the_breaker_and_no_call_is_made_while_it_is_open() {
        let r = rig(Config::default(), Ok("KEY".into()));
        *r.fail.lock().unwrap() = true;
        for _ in 0..3 {
            assert!(matches!(ask(&r), Err(JevError::Call(_))));
        }
        assert_eq!(r.calls.load(Ordering::SeqCst), 3);
        assert_eq!(ask(&r), Err(JevError::Open));
        assert_eq!(r.calls.load(Ordering::SeqCst), 3, "no call while open");
        // After the pause one more try is made; a failure closes it again at once.
        r.clock.advance(60_000);
        assert!(matches!(ask(&r), Err(JevError::Call(_))));
        assert_eq!(r.calls.load(Ordering::SeqCst), 4);
        assert_eq!(ask(&r), Err(JevError::Open));
        // And a success after the pause closes it for good.
        r.clock.advance(60_000);
        *r.fail.lock().unwrap() = false;
        assert!(ask(&r).is_ok());
        assert!(ask(&r).is_ok());
    }

    #[test]
    fn a_success_in_between_resets_the_count() {
        let r = rig(Config::default(), Ok("KEY".into()));
        for _ in 0..2 {
            *r.fail.lock().unwrap() = true;
            assert!(ask(&r).is_err());
            *r.fail.lock().unwrap() = false;
            assert!(ask(&r).is_ok());
        }
        assert!(ask(&r).is_ok(), "never opened");
    }

    #[test]
    fn the_daily_budget_stops_calls_and_a_new_day_gives_it_back() {
        let r = rig(
            Config {
                daily_tokens: 600,
                ..Config::default()
            },
            Ok("KEY".into()),
        );
        assert!(ask(&r).is_ok());
        assert!(ask(&r).is_ok());
        assert_eq!(ask(&r), Err(JevError::Budget));
        assert_eq!(r.calls.load(Ordering::SeqCst), 2);
        r.clock.advance(DAY_MS);
        assert!(ask(&r).is_ok());
    }

    #[test]
    fn without_a_key_nothing_is_called_and_it_counts_as_a_failure() {
        let r = rig(Config::default(), Err("el baúl está bloqueado".into()));
        for _ in 0..3 {
            assert!(matches!(ask(&r), Err(JevError::Key(_))));
        }
        assert_eq!(ask(&r), Err(JevError::Open));
        assert_eq!(r.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_key_is_not_in_the_error_text() {
        let r = rig(Config::default(), Ok("KEY-SECRETA-123".into()));
        *r.fail.lock().unwrap() = true;
        let e = ask(&r).unwrap_err().to_string();
        assert!(
            e.contains("HTTP 500") && !e.contains("KEY-SECRETA-123"),
            "{e}"
        );
    }

    #[test]
    fn a_change_of_model_is_noticed() {
        let r = rig(Config::default(), Ok("KEY".into()));
        ask(&r).unwrap();
        *r.model.lock().unwrap() = "jev-1.15.0".into();
        ask(&r).unwrap();
        assert!(r.jev.model_changed());
        assert_eq!(r.jev.model().as_deref(), Some("jev-1.15.0"));
    }

    #[test]
    fn no_questions_and_a_huge_state_are_refused_before_any_call() {
        let r = rig(Config::default(), Ok("KEY".into()));
        assert!(matches!(
            r.jev.ask(&json!({}), &Questions::new()),
            Err(JevError::Bad(_))
        ));
        let big = json!({"mensaje_de_la_persona": "x".repeat(40_000)});
        assert!(matches!(r.jev.ask(&big, &q()), Err(JevError::Bad(_))));
        assert_eq!(r.calls.load(Ordering::SeqCst), 0);
    }
}
