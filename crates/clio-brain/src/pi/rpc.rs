//! The RPC client for `pi --mode rpc`: one JSON object per line, records split on LF ONLY (a
//! U+2028 or U+2029 inside a string is not a boundary), an `id` per command, a tolerant
//! [`Event`] type, a deadline on every wait, cancellation, a `Drop` that kills the process
//! group, and stderr captured, capped and sanitized.
//!
//! A reader thread owns stdout. It routes responses to whoever waits for that `id`, answers
//! every extension dialog with "cancelled" (the person sees no extension dialogs in this version)
//! and queues everything else as events. [`RpcClient`] is `Sync`: another thread may `steer` or
//! `abort` while one waits in [`RpcClient::run_prompt`].

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{PiError, text};

/// Longest record accepted from Pi (a `get_messages` of a long session can be big).
const MAX_LINE: usize = 32 * 1024 * 1024;
/// How much of stderr is kept (the tail).
const STDERR_KEEP: usize = 32 * 1024;
/// How much of stderr is shown in an error.
const STDERR_SHOWN: usize = 800;

/// Token counts and cost. `cost` is in US dollars.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
    pub cost: f64,
}

impl Usage {
    pub fn from_value(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let n = |k: &str| o.get(k).and_then(Value::as_u64).unwrap_or(0);
        Some(Self {
            input: n("input"),
            output: n("output"),
            cache_read: n("cacheRead"),
            cache_write: n("cacheWrite"),
            total_tokens: n("totalTokens"),
            cost: o
                .get("cost")
                .and_then(|c| c.get("total"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        })
    }

    pub fn add(&mut self, o: &Self) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.total_tokens += o.total_tokens;
        self.cost += o.cost;
    }
}

/// A command's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub id: Option<String>,
    pub command: String,
    pub success: bool,
    pub error: Option<String>,
    pub data: Value,
}

/// What Pi streams. Never fails to parse: an unknown `type` is [`Event::Unknown`], a missing
/// field takes a default. Text fields hold what Pi sent: pass them through [`text`] before
/// drawing them.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    AgentStart,
    AgentEnd {
        will_retry: bool,
    },
    /// Pi will not continue by itself: this ends a run for the client.
    AgentSettled {
        aborted: bool,
    },
    TurnStart,
    TurnEnd,
    MessageStart(MessageInfo),
    /// `kind` is Pi's `assistantMessageEvent.type` (`text_delta`, `thinking_delta`,
    /// `toolcall_start`...); `delta` is its text when it has one. `usage` is cumulative for the
    /// message in flight.
    MessageUpdate {
        usage: Option<Usage>,
        kind: String,
        delta: Option<String>,
    },
    MessageEnd(MessageInfo),
    ToolStart {
        id: String,
        name: String,
        args: Value,
    },
    ToolUpdate {
        id: String,
        name: String,
        partial: Value,
    },
    ToolEnd {
        id: String,
        name: String,
        result: Value,
        is_error: bool,
        duration_ms: Option<u64>,
    },
    CompactionStart {
        reason: String,
    },
    CompactionEnd {
        reason: String,
        aborted: bool,
        will_retry: bool,
        error: Option<String>,
        tokens_before: Option<u64>,
        tokens_after: Option<u64>,
    },
    AutoRetryStart {
        attempt: u64,
        max_attempts: u64,
        delay_ms: u64,
        error: String,
    },
    AutoRetryEnd {
        success: bool,
        attempt: u64,
        final_error: Option<String>,
    },
    SessionInfoChanged {
        name: Option<String>,
    },
    ExtensionError {
        path: String,
        event: String,
        error: String,
    },
    /// A dialog or notification from an extension (already answered "cancelled" if it was a dialog).
    ExtensionUiRequest {
        id: String,
        method: String,
    },
    /// A response nobody was waiting for (for example the answer to unparsable input).
    Response(Response),
    /// A line that is not JSON.
    Garbage(String),
    /// An event this client does not know (new Pi versions add events).
    Unknown {
        kind: String,
        raw: Value,
    },
}

/// What `message_start`/`message_end` carry that Clio uses.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MessageInfo {
    pub role: String,
    /// The text blocks joined.
    pub text: String,
    pub usage: Option<Usage>,
    pub stop_reason: Option<String>,
    pub error_message: Option<String>,
    /// For a `system` message: the tools it declares (`toolsAdded`).
    pub tools_added: Vec<String>,
}

fn message_info(m: &Value) -> MessageInfo {
    let text = match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    MessageInfo {
        role: m["role"].as_str().unwrap_or("").to_owned(),
        text,
        usage: Usage::from_value(&m["usage"]),
        stop_reason: m["stopReason"].as_str().map(str::to_owned),
        error_message: m["errorMessage"].as_str().map(str::to_owned),
        tools_added: m["toolsAdded"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_owned()
}

impl Response {
    pub fn from_value(v: &Value) -> Self {
        Self {
            id: v["id"].as_str().map(str::to_owned),
            command: s(v, "command"),
            success: v["success"].as_bool().unwrap_or(false),
            error: v["error"].as_str().map(str::to_owned),
            data: v["data"].clone(),
        }
    }
}

impl Event {
    pub fn from_value(v: &Value) -> Self {
        let kind = v["type"].as_str().unwrap_or("");
        match kind {
            "agent_start" => Self::AgentStart,
            "agent_end" => Self::AgentEnd {
                will_retry: v["willRetry"].as_bool().unwrap_or(false),
            },
            "agent_settled" => Self::AgentSettled {
                aborted: v["aborted"].as_bool().unwrap_or(false),
            },
            "turn_start" => Self::TurnStart,
            "turn_end" => Self::TurnEnd,
            "message_start" => Self::MessageStart(message_info(&v["message"])),
            "message_end" => Self::MessageEnd(message_info(&v["message"])),
            "message_update" => {
                let u = &v["assistantMessageEvent"];
                Self::MessageUpdate {
                    usage: Usage::from_value(&v["usage"]),
                    kind: s(u, "type"),
                    delta: u["delta"].as_str().map(str::to_owned),
                }
            }
            "tool_execution_start" => Self::ToolStart {
                id: s(v, "toolCallId"),
                name: s(v, "toolName"),
                args: v["args"].clone(),
            },
            "tool_execution_update" => Self::ToolUpdate {
                id: s(v, "toolCallId"),
                name: s(v, "toolName"),
                partial: v["partialResult"].clone(),
            },
            "tool_execution_end" => Self::ToolEnd {
                id: s(v, "toolCallId"),
                name: s(v, "toolName"),
                result: v["result"].clone(),
                is_error: v["isError"].as_bool().unwrap_or(false),
                duration_ms: v["durationMs"].as_u64(),
            },
            "compaction_start" => Self::CompactionStart {
                reason: s(v, "reason"),
            },
            "compaction_end" => Self::CompactionEnd {
                reason: s(v, "reason"),
                aborted: v["aborted"].as_bool().unwrap_or(false),
                will_retry: v["willRetry"].as_bool().unwrap_or(false),
                error: v["errorMessage"].as_str().map(str::to_owned),
                tokens_before: v["result"]["tokensBefore"].as_u64(),
                tokens_after: v["result"]["estimatedTokensAfter"].as_u64(),
            },
            "auto_retry_start" => Self::AutoRetryStart {
                attempt: v["attempt"].as_u64().unwrap_or(0),
                max_attempts: v["maxAttempts"].as_u64().unwrap_or(0),
                delay_ms: v["delayMs"].as_u64().unwrap_or(0),
                error: s(v, "errorMessage"),
            },
            "auto_retry_end" => Self::AutoRetryEnd {
                success: v["success"].as_bool().unwrap_or(false),
                attempt: v["attempt"].as_u64().unwrap_or(0),
                final_error: v["finalError"].as_str().map(str::to_owned),
            },
            "session_info_changed" => Self::SessionInfoChanged {
                name: v["name"].as_str().map(str::to_owned),
            },
            "extension_error" => Self::ExtensionError {
                path: s(v, "extensionPath"),
                event: s(v, "event"),
                error: s(v, "error"),
            },
            "extension_ui_request" => Self::ExtensionUiRequest {
                id: s(v, "id"),
                method: s(v, "method"),
            },
            "response" => Self::Response(Response::from_value(v)),
            _ => Self::Unknown {
                kind: kind.to_owned(),
                raw: v.clone(),
            },
        }
    }
}

/// `session` state from `get_state`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub session_id: String,
    pub session_file: Option<String>,
    pub session_name: Option<String>,
    pub provider: String,
    pub model: String,
    pub is_streaming: bool,
    pub message_count: u64,
    pub raw: Value,
}

impl State {
    pub fn from_value(v: &Value) -> Self {
        Self {
            session_id: s(v, "sessionId"),
            session_file: v["sessionFile"].as_str().map(str::to_owned),
            session_name: v["sessionName"].as_str().map(str::to_owned),
            provider: s(&v["model"], "provider"),
            model: s(&v["model"], "id"),
            is_streaming: v["isStreaming"].as_bool().unwrap_or(false),
            message_count: v["messageCount"].as_u64().unwrap_or(0),
            raw: v.clone(),
        }
    }
}

/// How full the model's context is. `tokens`/`percent` are `None` right after a compaction.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub window: u64,
    pub percent: Option<f64>,
}

/// `get_session_stats`: totals for the whole session plus the context.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SessionStats {
    pub usage: Usage,
    pub context: Option<ContextUsage>,
    pub messages: u64,
    pub tool_calls: u64,
}

impl SessionStats {
    pub fn from_value(v: &Value) -> Self {
        let t = &v["tokens"];
        let n = |k: &str| t[k].as_u64().unwrap_or(0);
        let c = &v["contextUsage"];
        Self {
            usage: Usage {
                input: n("input"),
                output: n("output"),
                cache_read: n("cacheRead"),
                cache_write: n("cacheWrite"),
                total_tokens: n("total"),
                cost: v["cost"].as_f64().unwrap_or(0.0),
            },
            context: c.is_object().then(|| ContextUsage {
                tokens: c["tokens"].as_u64(),
                window: c["contextWindow"].as_u64().unwrap_or(0),
                percent: c["percent"].as_f64(),
            }),
            messages: v["totalMessages"].as_u64().unwrap_or(0),
            tool_calls: v["toolCalls"].as_u64().unwrap_or(0),
        }
    }
}

/// Watches events and keeps the numbers the status bar shows: tokens in and out, cost, and the
/// context. Feed it every event with [`UsageMeter::observe`]; call [`UsageMeter::refresh`] with
/// `get_session_stats` after each settled run (that is the authoritative figure).
#[derive(Debug, Clone, Default)]
pub struct UsageMeter {
    done: Usage,
    live: Option<Usage>,
    pub context: Option<ContextUsage>,
    pub compactions: u32,
    /// `(attempt, max)` while Pi retries a failed turn.
    pub retrying: Option<(u64, u64)>,
}

impl UsageMeter {
    pub fn observe(&mut self, e: &Event) {
        match e {
            Event::MessageUpdate { usage: Some(u), .. } => self.live = Some(*u),
            Event::MessageEnd(m) if m.role == "assistant" => {
                self.live = None;
                if let Some(u) = &m.usage {
                    self.done.add(u);
                }
            }
            Event::CompactionEnd {
                aborted: false,
                error: None,
                ..
            } => {
                self.compactions += 1;
                if let Some(c) = &mut self.context {
                    c.tokens = None;
                    c.percent = None;
                }
            }
            Event::AutoRetryStart {
                attempt,
                max_attempts,
                ..
            } => self.retrying = Some((*attempt, *max_attempts)),
            Event::AutoRetryEnd { .. } => self.retrying = None,
            _ => {}
        }
    }

    pub fn refresh(&mut self, st: &SessionStats) {
        self.done = st.usage;
        self.live = None;
        self.context = st.context;
    }

    /// Everything counted so far, including the message in flight.
    pub fn total(&self) -> Usage {
        let mut t = self.done;
        if let Some(l) = &self.live {
            t.add(l);
        }
        t
    }
}

/// Deadlines on every wait. Nothing in this client waits without one.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// One command and its response.
    pub request: Duration,
    /// Longest silence (no event at all) during a run.
    pub silence: Duration,
    /// Longest whole run.
    pub run: Duration,
    /// After `abort` or when closing: how long to wait before killing.
    pub grace: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(30),
            silence: Duration::from_secs(180),
            run: Duration::from_secs(3600),
            grace: Duration::from_secs(5),
        }
    }
}

/// A flag the caller raises to stop a run; cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What a finished run produced.
#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    /// The last assistant message's text.
    pub text: String,
    /// Summed from every assistant message of the run.
    pub usage: Usage,
    pub aborted: bool,
    /// `handled`: an extension consumed the prompt, no run happened.
    pub handled: bool,
    pub tool_calls: u32,
    /// Provider errors reported inside messages (`stopReason: error`), as sent.
    pub errors: Vec<String>,
}

type Pending = Arc<Mutex<HashMap<String, Sender<Response>>>>;

pub struct RpcClient {
    child: Mutex<Child>,
    pid: u32,
    reaped: AtomicBool,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Pending,
    events: Mutex<Receiver<Event>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    next_id: AtomicU64,
    pub timeouts: Timeouts,
    /// When set: every tool a system message declares or an execution starts must be in this list;
    /// otherwise the process is killed and the call fails.
    allowed_tools: Mutex<Option<(Vec<String>, bool)>>,
}

/// Kills a whole process group (Pi and anything it started). Only called while the leader has
/// not been reaped, so the pid cannot belong to someone else.
fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

impl RpcClient {
    /// Starts `cmd` as the RPC process: pipes on all three streams and its own process group.
    pub fn spawn(cmd: &mut Command) -> Result<Self, PiError> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let child = cmd
            .spawn()
            .map_err(|e| PiError::Spawn(format!("no pude iniciar Pi: {e}")))?;
        Self::from_child(child)
    }

    fn from_child(mut child: Child) -> Result<Self, PiError> {
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(PiError::Spawn("Pi sin pipes".into()));
        };
        let pid = child.id();
        let stdin = Arc::new(Mutex::new(Some(stdin)));
        let pending: Pending = Arc::default();
        let (tx, rx) = mpsc::channel();
        {
            let (stdin, pending) = (stdin.clone(), pending.clone());
            std::thread::spawn(move || reader(stdout, &stdin, &pending, &tx));
        }
        let errbuf: Arc<Mutex<Vec<u8>>> = Arc::default();
        {
            let errbuf = errbuf.clone();
            std::thread::spawn(move || {
                let mut r = stderr;
                let mut chunk = [0u8; 4096];
                while let Ok(n) = r.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut b) = errbuf.lock() {
                        b.extend_from_slice(&chunk[..n]);
                        if b.len() > STDERR_KEEP {
                            let cut = b.len() - STDERR_KEEP;
                            b.drain(..cut);
                        }
                    }
                }
            });
        }
        Ok(Self {
            child: Mutex::new(child),
            pid,
            reaped: AtomicBool::new(false),
            stdin,
            pending,
            events: Mutex::new(rx),
            stderr: errbuf,
            next_id: AtomicU64::new(1),
            timeouts: Timeouts::default(),
            allowed_tools: Mutex::new(None),
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Restrict tools to these names (see the field's doc). With `exact_first`, the first system
    /// message of a new session (it declares every tool) must declare EXACTLY this set: a guard
    /// that failed to load shows up as a missing tool and the process is killed.
    pub fn set_allowed_tools(&self, tools: Vec<String>, exact_first: bool) {
        if let Ok(mut a) = self.allowed_tools.lock() {
            *a = Some((tools, exact_first));
        }
    }

    /// Pi's stderr so far: sanitized, tail only.
    pub fn stderr_text(&self) -> String {
        let raw = self.stderr.lock().map(|b| b.clone()).unwrap_or_default();
        text::block(&String::from_utf8_lossy(&raw), STDERR_SHOWN)
    }

    fn exited(&self) -> PiError {
        // Give the stderr thread a moment to drain what the dying process wrote.
        std::thread::sleep(Duration::from_millis(50));
        PiError::Exited {
            stderr: self.stderr_text(),
        }
    }

    fn send(&self, v: &Value) -> Result<(), PiError> {
        let mut line = serde_json::to_vec(v).map_err(|e| PiError::Protocol(e.to_string()))?;
        line.push(b'\n');
        let mut g = self
            .stdin
            .lock()
            .map_err(|_| PiError::Protocol("stdin envenenado".into()))?;
        let w = g.as_mut().ok_or_else(|| self.exited())?;
        w.write_all(&line)
            .and_then(|()| w.flush())
            .map_err(|_| self.exited())
    }

    /// Sends one command and waits for its response (at most `timeout`). Returns its `data`.
    pub fn request_with(
        &self,
        kind: &str,
        fields: Value,
        timeout: Duration,
    ) -> Result<Value, PiError> {
        let id = format!("clio-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| PiError::Protocol("poisoned".into()))?
            .insert(id.clone(), tx);
        let mut cmd = fields;
        if !cmd.is_object() {
            cmd = json!({});
        }
        cmd["id"] = json!(id);
        cmd["type"] = json!(kind);
        if let Err(e) = self.send(&cmd) {
            self.forget(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(r) if r.success => Ok(r.data),
            Ok(r) => Err(PiError::Command {
                command: kind.to_owned(),
                message: text::line(r.error.as_deref().unwrap_or("sin detalle"), 300),
            }),
            Err(RecvTimeoutError::Timeout) => {
                self.forget(&id);
                Err(PiError::Timeout {
                    what: format!("comando {kind}"),
                    secs: timeout.as_secs(),
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(self.exited()),
        }
    }

    pub fn request(&self, kind: &str, fields: Value) -> Result<Value, PiError> {
        self.request_with(kind, fields, self.timeouts.request)
    }

    fn forget(&self, id: &str) {
        if let Ok(mut p) = self.pending.lock() {
            p.remove(id);
        }
    }

    /// The next event, or `None` if none came within `timeout`. Err when Pi is gone (after the
    /// queue is drained) or when the allowed-tools rule is broken (Pi is killed first).
    pub fn next_event(&self, timeout: Duration) -> Result<Option<Event>, PiError> {
        let r = self
            .events
            .lock()
            .map_err(|_| PiError::Protocol("poisoned".into()))?
            .recv_timeout(timeout);
        match r {
            Ok(e) => {
                self.check_tools(&e)?;
                Ok(Some(e))
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(self.exited()),
        }
    }

    fn check_tools(&self, e: &Event) -> Result<(), PiError> {
        let mut guard = self
            .allowed_tools
            .lock()
            .map_err(|_| PiError::Protocol("poisoned".into()))?;
        let Some((allowed, exact_first)) = guard.as_mut() else {
            return Ok(());
        };
        if let Event::MessageStart(m) = e
            && m.role == "system"
            && std::mem::take(exact_first)
        {
            let mut want = allowed.clone();
            let mut got = m.tools_added.clone();
            want.sort();
            got.sort();
            if want != got {
                drop(guard);
                self.kill();
                return Err(PiError::Verification(format!(
                    "las herramientas activas ({}) no son exactamente las de la guardia ({})",
                    text::line(&got.join(", "), 120),
                    text::line(&want.join(", "), 120)
                )));
            }
        }
        let bad: Vec<&str> = match e {
            Event::MessageStart(m) | Event::MessageEnd(m) if m.role == "system" => m
                .tools_added
                .iter()
                .map(String::as_str)
                .filter(|t| !allowed.iter().any(|a| a == t))
                .collect(),
            Event::ToolStart { name, .. } if !allowed.iter().any(|a| a == name) => {
                vec![name.as_str()]
            }
            _ => vec![],
        };
        if bad.is_empty() {
            return Ok(());
        }
        let bad: Vec<String> = bad.into_iter().map(str::to_owned).collect();
        drop(guard);
        self.kill();
        Err(PiError::Verification(format!(
            "herramientas fuera de la lista permitida: {}",
            text::line(&bad.join(", "), 200)
        )))
    }

    /// Kills the process and its group now.
    pub fn kill(&self) {
        if !self.reaped.load(Ordering::SeqCst) {
            kill_group(self.pid);
        }
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }

    /// True while the process runs.
    pub fn is_alive(&self) -> bool {
        if self.reaped.load(Ordering::SeqCst) {
            return false;
        }
        let Ok(mut c) = self.child.lock() else {
            return false;
        };
        match c.try_wait() {
            Ok(None) => true,
            _ => {
                self.reaped.store(true, Ordering::SeqCst);
                false
            }
        }
    }

    /// Closes stdin (Pi's orderly shutdown), waits up to the grace period, then kills.
    pub fn shutdown(&self) {
        if let Ok(mut g) = self.stdin.lock() {
            *g = None;
        }
        let deadline = Instant::now() + self.timeouts.grace;
        while self.is_alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if self.is_alive() {
            self.kill();
        }
        if let Ok(mut c) = self.child.lock() {
            let _ = c.wait();
            self.reaped.store(true, Ordering::SeqCst);
        }
    }

    // ---- commands -------------------------------------------------------------------------

    pub fn abort(&self) -> Result<(), PiError> {
        self.request_with(
            "abort",
            json!({}),
            self.timeouts.grace.max(self.timeouts.request),
        )
        .map(|_| ())
    }
    pub fn steer(&self, message: &str) -> Result<(), PiError> {
        self.request("steer", json!({"message": message}))
            .map(|_| ())
    }
    pub fn follow_up(&self, message: &str) -> Result<(), PiError> {
        self.request("follow_up", json!({"message": message}))
            .map(|_| ())
    }
    pub fn get_state(&self) -> Result<State, PiError> {
        self.request("get_state", json!({}))
            .map(|v| State::from_value(&v))
    }
    pub fn get_messages(&self) -> Result<Vec<Value>, PiError> {
        let v = self.request_with("get_messages", json!({}), self.timeouts.request * 2)?;
        Ok(v["messages"].as_array().cloned().unwrap_or_default())
    }
    pub fn get_session_stats(&self) -> Result<SessionStats, PiError> {
        self.request("get_session_stats", json!({}))
            .map(|v| SessionStats::from_value(&v))
    }
    pub fn get_commands(&self) -> Result<Vec<Value>, PiError> {
        let v = self.request("get_commands", json!({}))?;
        Ok(v["commands"].as_array().cloned().unwrap_or_default())
    }
    pub fn get_fork_messages(&self) -> Result<Vec<Value>, PiError> {
        let v = self.request("get_fork_messages", json!({}))?;
        Ok(v["messages"].as_array().cloned().unwrap_or_default())
    }
    pub fn set_session_name(&self, name: &str) -> Result<(), PiError> {
        self.request("set_session_name", json!({"name": name}))
            .map(|_| ())
    }
    /// Manual compaction; `Ok` carries Pi's result (`summary`, `tokensBefore`...).
    pub fn compact(&self, instructions: Option<&str>) -> Result<Value, PiError> {
        let f = instructions.map_or_else(|| json!({}), |i| json!({"customInstructions": i}));
        self.request_with(
            "compact",
            f,
            self.timeouts.run.min(Duration::from_secs(600)),
        )
    }
    /// `Ok(false)` when an extension cancelled it.
    pub fn new_session(&self, parent: Option<&str>) -> Result<bool, PiError> {
        let f = parent.map_or_else(|| json!({}), |p| json!({"parentSession": p}));
        self.request("new_session", f)
            .map(|v| !v["cancelled"].as_bool().unwrap_or(false))
    }
    pub fn switch_session(&self, session_path: &str) -> Result<bool, PiError> {
        self.request("switch_session", json!({"sessionPath": session_path}))
            .map(|v| !v["cancelled"].as_bool().unwrap_or(false))
    }
    /// A new session forked from a user message (`entry_id` from [`Self::get_fork_messages`]).
    /// Returns the text of that message, or `None` if an extension cancelled.
    pub fn fork(&self, entry_id: &str) -> Result<Option<String>, PiError> {
        let v = self.request("fork", json!({"entryId": entry_id}))?;
        Ok((!v["cancelled"].as_bool().unwrap_or(false)).then(|| s(&v, "text")))
    }

    /// Sends a prompt and pumps events until Pi settles. `on_event` sees every event (including
    /// the ones that end the run). Ends with `Timeout` on the silence or run limits (after a
    /// best-effort `abort`), and honors `cancel` by sending `abort` and waiting up to the grace
    /// period for the run to settle.
    pub fn run_prompt(
        &self,
        message: &str,
        cancel: &Cancel,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<RunSummary, PiError> {
        // Anything left over belongs to an earlier run.
        while self.next_event(Duration::ZERO)?.is_some() {}
        let ack = self.request("prompt", json!({"message": message}))?;
        let mut sum = RunSummary::default();
        if ack["disposition"] == "handled" {
            sum.handled = true;
            return Ok(sum);
        }
        let start = Instant::now();
        let mut last = start;
        let mut abort_sent: Option<Instant> = None;
        loop {
            if cancel.is_cancelled() && abort_sent.is_none() {
                abort_sent = Some(Instant::now());
                let _ = self.abort();
            }
            if let Some(t) = abort_sent
                && t.elapsed() > self.timeouts.grace
            {
                self.kill();
                return Err(PiError::Cancelled);
            }
            if start.elapsed() > self.timeouts.run || last.elapsed() > self.timeouts.silence {
                let (what, secs) = if start.elapsed() > self.timeouts.run {
                    ("la corrida completa", self.timeouts.run.as_secs())
                } else {
                    ("silencio sin eventos", self.timeouts.silence.as_secs())
                };
                let _ = self.abort();
                return Err(PiError::Timeout {
                    what: what.into(),
                    secs,
                });
            }
            let Some(ev) = self.next_event(Duration::from_millis(50))? else {
                continue;
            };
            last = Instant::now();
            match &ev {
                Event::MessageEnd(m) if m.role == "assistant" => {
                    sum.text.clone_from(&m.text);
                    if let Some(u) = &m.usage {
                        sum.usage.add(u);
                    }
                    if m.stop_reason.as_deref() == Some("error") {
                        sum.errors.push(m.error_message.clone().unwrap_or_default());
                    }
                }
                Event::ToolStart { .. } => sum.tool_calls += 1,
                _ => {}
            }
            on_event(&ev);
            if let Event::AgentSettled { aborted } = ev {
                sum.aborted = aborted;
                return Ok(sum);
            }
        }
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        if let Ok(mut g) = self.stdin.lock() {
            *g = None;
        }
        if !self.reaped.load(Ordering::SeqCst) {
            kill_group(self.pid);
        }
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// The stdout thread: LF-split records, routing, extension dialogs answered "cancelled".
fn reader(
    stdout: std::process::ChildStdout,
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    pending: &Pending,
    events: &Sender<Event>,
) {
    let mut r = std::io::BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // `take` caps one record; read_until splits on b'\n' and nothing else.
        let n = (&mut r)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut buf);
        match n {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.last() != Some(&b'\n') {
            if buf.len() > MAX_LINE {
                let _ = events.send(Event::Garbage(
                    "registro demasiado largo, se cerró la conexión".into(),
                ));
            }
            if buf.len() > MAX_LINE {
                break;
            }
            // A last record without LF at EOF: still a record.
        }
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        if buf.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&buf) else {
            let _ = events.send(Event::Garbage(text::line(
                &String::from_utf8_lossy(&buf),
                200,
            )));
            continue;
        };
        if v["type"] == "response"
            && let Some(id) = v["id"].as_str()
            && let Some(tx) = pending.lock().ok().and_then(|mut p| p.remove(id))
        {
            let _ = tx.send(Response::from_value(&v));
            continue;
        }
        if v["type"] == "extension_ui_request" {
            let dialog = matches!(
                v["method"].as_str(),
                Some("select" | "confirm" | "input" | "editor")
            );
            if dialog && let Some(id) = v["id"].as_str() {
                let reply = json!({"type": "extension_ui_response", "id": id, "cancelled": true});
                if let Ok(mut g) = stdin.lock()
                    && let Some(w) = g.as_mut()
                {
                    let mut line = reply.to_string().into_bytes();
                    line.push(b'\n');
                    let _ = w.write_all(&line).and_then(|()| w.flush());
                }
            }
        }
        if events.send(Event::from_value(&v)).is_err() {
            break;
        }
    }
    // Pi is gone: whoever waits for a response sees the channel close.
    if let Ok(mut p) = pending.lock() {
        p.clear();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn events_parse_and_unknown_ones_do_not_fail() {
        let e = Event::from_value(
            &json!({"type":"tool_execution_end","toolCallId":"c1","toolName":"read","result":{"x":1},"isError":true,"durationMs":7}),
        );
        assert_eq!(
            e,
            Event::ToolEnd {
                id: "c1".into(),
                name: "read".into(),
                result: json!({"x":1}),
                is_error: true,
                duration_ms: Some(7)
            }
        );
        assert!(matches!(
            Event::from_value(&json!({"type":"brand_new","x":1})),
            Event::Unknown { .. }
        ));
        assert!(matches!(
            Event::from_value(&json!([1, 2])),
            Event::Unknown { .. }
        ));
        assert_eq!(
            Event::from_value(&json!({"type":"agent_settled"})),
            Event::AgentSettled { aborted: false }
        );
    }

    #[test]
    fn message_update_carries_usage_and_delta() {
        let e = Event::from_value(
            &json!({"type":"message_update","usage":{"input":100,"output":5,"totalTokens":105,"cost":{"total":0.5}},"assistantMessageEvent":{"type":"text_delta","delta":"hi"}}),
        );
        let Event::MessageUpdate { usage, kind, delta } = e else {
            panic!()
        };
        assert_eq!(
            (kind.as_str(), delta.as_deref()),
            ("text_delta", Some("hi"))
        );
        assert_eq!(usage.unwrap().input, 100);
    }

    #[test]
    fn the_meter_adds_finished_messages_and_the_live_one() {
        let mut m = UsageMeter::default();
        let upd = |i| Event::MessageUpdate {
            usage: Some(Usage {
                input: i,
                output: 1,
                ..Usage::default()
            }),
            kind: "text_delta".into(),
            delta: None,
        };
        m.observe(&upd(10));
        assert_eq!(m.total().input, 10);
        m.observe(&Event::MessageEnd(MessageInfo {
            role: "assistant".into(),
            usage: Some(Usage {
                input: 10,
                output: 2,
                ..Usage::default()
            }),
            ..MessageInfo::default()
        }));
        m.observe(&upd(5));
        assert_eq!((m.total().input, m.total().output), (15, 3));
        m.refresh(&SessionStats::from_value(&json!({"tokens":{"input":99,"output":9,"total":108},"cost":0.25,"contextUsage":{"tokens":null,"contextWindow":1000,"percent":null}})));
        assert_eq!((m.total().input, m.total().cost), (99, 0.25));
        assert_eq!(m.context.unwrap().window, 1000);
    }
}
