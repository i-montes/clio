//! Test harness: the REAL Pi from `adapters/pi/node_modules`, launched with the hardened flags,
//! the `clio-guard` extension and the real `clio-toolhost` binary, talking RPC, against a fake
//! model. Everything lives in private temp folders; every child process has a `Drop` guard.
#![allow(dead_code, clippy::unwrap_used)]

pub mod fake_model;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clio_canaries::Canaries;
use fake_model::FakeModel;
use serde_json::{Value, json};
use tempfile::TempDir;

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn pi_dir() -> PathBuf {
    repo().join("adapters/pi")
}

fn pi_cli() -> PathBuf {
    pi_dir().join("node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js")
}

pub fn extension() -> PathBuf {
    pi_dir().join("extensions/clio-guard")
}

pub const PROMPT: &str = "clio-agent@4";

/// Where node is, if Pi can run here. Otherwise a visible message (and a panic if
/// `CLIO_REQUIRE_PI` is set) and `None`: the test must return.
pub fn pi_available() -> Option<PathBuf> {
    let node = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("node"))
            .find(|n| n.is_file())
    });
    let why = match &node {
        None => Some("node is not in PATH".to_owned()),
        Some(_) if !pi_cli().is_file() => Some(
            "adapters/pi/node_modules is missing (run `npm ci --ignore-scripts` in adapters/pi)"
                .to_owned(),
        ),
        Some(_) => None,
    };
    match why {
        None => node,
        Some(why) => {
            assert!(
                std::env::var_os("CLIO_REQUIRE_PI").is_none(),
                "CLIO_REQUIRE_PI is set and Pi cannot run: {why}"
            );
            eprintln!("SKIPPED (Pi cannot run on this machine, nothing was tested): {why}");
            None
        }
    }
}

pub struct World {
    pub tmp: TempDir,
    pub work: PathBuf,
    pub home: PathBuf,
    pub agent: PathBuf,
    pub sessions: PathBuf,
    pub scratch: PathBuf,
    pub run_dir: PathBuf,
    pub sock_dir: PathBuf,
    pub canaries: Canaries,
}

pub fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().canonicalize().unwrap();
    let w = World {
        work: r.join("work"),
        home: r.join("home"),
        agent: r.join("agent"),
        sessions: r.join("sessions"),
        scratch: r.join("scratch"),
        run_dir: r.join("run"),
        sock_dir: r.join("sock"),
        canaries: Canaries::fresh(),
        tmp,
    };
    for d in [
        &w.work,
        &w.home.join(".ssh"),
        &w.agent,
        &w.sessions,
        &w.scratch,
        &w.run_dir,
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir(&w.sock_dir).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&w.sock_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        w.home.join(".ssh/id_ed25519"),
        w.canaries.value("ssh").unwrap(),
    )
    .unwrap();
    w
}

impl World {
    pub fn put(&self, rel: &str, text: &str) {
        let p = self.work.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    pub fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.work.join(rel)).ok()
    }
    /// Every path under the working copy, relative, sorted: to compare before and after.
    pub fn tree(&self) -> Vec<String> {
        fn go(base: &Path, d: &Path, out: &mut Vec<String>) {
            let Ok(rd) = std::fs::read_dir(d) else { return };
            for e in rd.flatten() {
                let p = e.path();
                out.push(p.strip_prefix(base).unwrap().display().to_string());
                if p.is_dir() && !p.is_symlink() {
                    go(base, &p, out);
                }
            }
        }
        let mut v = Vec::new();
        go(&self.work, &self.work, &mut v);
        v.sort();
        v
    }
}

/// Kills and reaps its child when dropped.
pub struct Reaper(pub Child);
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub struct Toolhost {
    pub proc: Option<Reaper>,
    pub socket: PathBuf,
    pub stderr: Arc<Mutex<String>>,
    pub stdin: Option<ChildStdin>,
}

impl Toolhost {
    pub fn start(w: &World, jail: bool, scrub: Option<(&str, &str)>) -> Self {
        let _ = std::fs::remove_file(w.sock_dir.join("toolhost.sock")); // a killed one leaves it
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio-toolhost"));
        cmd.args(["--socket-dir"]).arg(&w.sock_dir);
        cmd.args(["--work"]).arg(&w.work);
        cmd.args(["--scratch"]).arg(&w.scratch);
        cmd.args(["--run-dir"]).arg(&w.run_dir);
        cmd.args(["--jail", if jail { "auto" } else { "off" }]);
        // HOME matters: the jail's self-test plants its decoy there and checks it is invisible.
        cmd.env_clear().env("HOME", &w.home);
        if let Some((n, v)) = scrub {
            cmd.env("CLIO_TOOLHOST_SCRUB_JSON", json!({ n: v }).to_string());
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stderr = collect(child.stderr.take().unwrap());
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let proc = Reaper(child);
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let socket = PathBuf::from(
            line.strip_prefix("READY ")
                .unwrap_or_else(|| {
                    panic!(
                        "the toolhost did not say READY: {line:?} / {}",
                        stderr.lock().unwrap()
                    )
                })
                .trim(),
        );
        Self {
            proc: Some(proc),
            socket,
            stderr,
            stdin,
        }
    }

    /// The service dies (a crash, not an orderly stop).
    pub fn kill(&mut self) {
        self.proc = None; // Reaper::drop kills and waits
    }
}

fn collect(r: impl std::io::Read + Send + 'static) -> Arc<Mutex<String>> {
    let buf = Arc::new(Mutex::new(String::new()));
    let b = buf.clone();
    std::thread::spawn(move || {
        for l in BufReader::new(r).lines().map_while(Result::ok) {
            let mut g = b.lock().unwrap_or_else(|e| e.into_inner());
            g.push_str(&l);
            g.push('\n');
        }
    });
    buf
}

pub struct Pi {
    child: Option<Reaper>,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    pub stderr: Arc<Mutex<String>>,
    next: u32,
    pub started_in: Duration,
}

pub struct Opts {
    pub extension: bool,
    pub session: Option<PathBuf>,
    pub prompt_file: Option<PathBuf>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            extension: true,
            session: None,
            prompt_file: None,
        }
    }
}

impl Pi {
    pub fn start(
        node: &Path,
        w: &World,
        model: &FakeModel,
        socket: Option<&Path>,
        o: &Opts,
    ) -> Self {
        Self::try_start(node, w, model, socket, o).unwrap_or_else(|e| panic!("{e}"))
    }

    /// `Err(stderr)` when Pi exits before it answers (an extension that fails to load stops it).
    pub fn try_start(
        node: &Path,
        w: &World,
        model: &FakeModel,
        socket: Option<&Path>,
        o: &Opts,
    ) -> Result<Self, String> {
        let t0 = Instant::now();
        let models = json!({"providers":{"fake":{
            "baseUrl": format!("http://{}/v1", model.addr), "api":"openai-completions",
            "apiKey":"not-a-real-key", "models":[{"id":"fake-model","name":"Fake","reasoning":false,
            "input":["text"],"contextWindow":32000,"maxTokens":2048,
            "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}}]}}});
        std::fs::write(w.agent.join("models.json"), models.to_string()).unwrap();
        let mut cmd = Command::new(node);
        cmd.arg(pi_cli());
        cmd.args(["--mode", "rpc", "--no-builtin-tools", "--no-extensions"]);
        if o.extension {
            cmd.arg("-e").arg(extension());
        }
        cmd.args([
            "--no-mcp",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
            "--no-context-files",
            "--no-approve",
            "--offline",
            "--provider",
            "fake",
            "--model",
            "fake-model",
        ]);
        if let Some(s) = &o.session {
            cmd.arg("--session").arg(s);
        }
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &w.home)
            .env("PI_CODING_AGENT_DIR", &w.agent)
            .env("PI_CODING_AGENT_SESSION_DIR", &w.sessions)
            .env("PI_OFFLINE", "1")
            .env("PI_TELEMETRY", "0")
            .env("PI_SKIP_VERSION_CHECK", "1")
            .env("CLIO_GUARD_PROMPT_VERSION", PROMPT)
            .env(
                "CLIO_GUARD_PROMPT_FILE",
                o.prompt_file
                    .clone()
                    .unwrap_or_else(|| repo().join(format!("prompts/system/{PROMPT}.md"))),
            );
        if let Some(s) = socket {
            cmd.env("CLIO_TOOLHOST_SOCKET", s);
        }
        let mut child = cmd
            .current_dir(&w.work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stderr = collect(child.stderr.take().unwrap());
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&l) {
                    let _ = tx.send(v);
                }
            }
        });
        let mut pi = Self {
            child: Some(Reaper(child)),
            stdin,
            rx,
            stderr,
            next: 0,
            started_in: Duration::ZERO,
        };
        // Ready when it answers a command.
        let id = pi.send(json!({"type":"get_state"}));
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            match pi
                .rx
                .recv_timeout(end.saturating_duration_since(Instant::now()))
            {
                Ok(r) if r["type"] == "response" && r["id"] == id => break,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Err("Pi did not answer in 60 s".into()),
                Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(200)); // let stderr drain
                    return Err(pi.stderr_text());
                }
            }
        }
        pi.started_in = t0.elapsed();
        Ok(pi)
    }

    pub fn stderr_text(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn send(&mut self, mut v: Value) -> String {
        self.next += 1;
        let id = format!("c{}", self.next);
        v["id"] = json!(id);
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
        id
    }

    /// Sends a command and waits for its response (events seen meanwhile are dropped).
    pub fn command(&mut self, v: Value, timeout: Duration) -> Value {
        let id = self.send(v);
        let end = Instant::now() + timeout;
        loop {
            match self
                .rx
                .recv_timeout(end.saturating_duration_since(Instant::now()))
            {
                Ok(r) if r["type"] == "response" && r["id"] == id => return r,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => panic!(
                    "no response to {id} in {timeout:?}; stderr:\n{}",
                    self.stderr_text()
                ),
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("Pi exited; stderr:\n{}", self.stderr_text())
                }
            }
        }
    }

    /// Sends a prompt and returns every event until Pi settles.
    pub fn prompt(&mut self, text: &str) -> Vec<Value> {
        let id = self.send(json!({"type":"prompt","message":text}));
        let end = Instant::now() + Duration::from_secs(120);
        let mut events = Vec::new();
        loop {
            match self
                .rx
                .recv_timeout(end.saturating_duration_since(Instant::now()))
            {
                Ok(e) => {
                    let done = e["type"] == "agent_settled";
                    let refused = e["type"] == "response" && e["id"] == id && e["success"] == false;
                    events.push(e);
                    if done || refused {
                        return events;
                    }
                }
                Err(_) => panic!(
                    "Pi did not settle; events so far: {events:#?}\nstderr:\n{}",
                    self.stderr_text()
                ),
            }
        }
    }

    pub fn entries(&mut self) -> Vec<Value> {
        let r = self.command(json!({"type":"get_entries"}), Duration::from_secs(20));
        r["data"]["entries"].as_array().cloned().unwrap_or_default()
    }

    pub fn state(&mut self) -> Value {
        self.command(json!({"type":"get_state"}), Duration::from_secs(20))["data"].clone()
    }

    /// Asks Pi to stop (closes stdin) and waits for it to exit by itself; kills it if it will not.
    pub fn close(&mut self) {
        self.stdin = None;
        let Some(r) = self.child.as_mut() else { return };
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            if matches!(r.0.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.child = None; // the Reaper kills a process that is still there
    }

    /// The startup check: the tools Pi has active are EXACTLY registered by the Clio extension
    /// (names in `allowed`, source path the extension's) and none comes from anywhere else.
    /// Pi's RPC has no command that lists tools, so the extension reports what
    /// `pi.getActiveTools()` / `pi.getAllTools()` say in a `clio-guard.tools` entry. This reads
    /// it back. (A tool the extension did not see is not covered here: `declared_tools` checks
    /// the list the model is really sent.)
    pub fn verify_tools(&mut self, allowed: &[&str]) -> Result<Vec<String>, String> {
        let entries = self.entries();
        let Some(rep) = entries
            .iter()
            .rev()
            .find(|e| e["type"] == "custom" && e["customType"] == "clio-guard.tools")
        else {
            return Err("the extension did not report its tools: it did not load".into());
        };
        let data = &rep["data"];
        let (ext, ext_index) = (extension(), extension().join("index.ts"));
        let mut active: Vec<String> = Vec::new();
        for n in data["active"].as_array().unwrap_or(&vec![]) {
            let name = n.as_str().unwrap_or_default().to_owned();
            if !allowed.contains(&name.as_str()) {
                return Err(format!("active tool {name} is not in the allowed list"));
            }
            let src = data["all"]
                .as_array()
                .and_then(|a| a.iter().find(|t| t["name"] == name.as_str()))
                .and_then(|t| t["source"].as_str())
                .unwrap_or_default();
            if Path::new(src) != ext && Path::new(src) != ext_index {
                return Err(format!(
                    "active tool {name} comes from {src:?}, not from the Clio extension"
                ));
            }
            active.push(name);
        }
        Ok(active)
    }
}

/// The names of the tools the fake model was told about in its first request.
pub fn declared_tools(model: &FakeModel) -> Vec<String> {
    let reqs = model.requests();
    let first = reqs.first().expect("the model got no request");
    first["tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `(tool name, is error, text)` of every finished tool execution in `events`.
pub fn tool_ends(events: &[Value]) -> Vec<(String, bool, String)> {
    events
        .iter()
        .filter(|e| e["type"] == "tool_execution_end")
        .map(|e| {
            let text = e["result"]["content"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            (
                e["toolName"].as_str().unwrap_or_default().to_owned(),
                e["isError"].as_bool().unwrap_or(false),
                text,
            )
        })
        .collect()
}

pub struct Stack {
    pub pi: Pi,
    pub host: Toolhost,
}

pub fn stack(node: &Path, w: &World, model: &FakeModel, jail: bool, o: &Opts) -> Stack {
    let host = Toolhost::start(w, jail, None);
    let pi = Pi::start(node, w, model, Some(&host.socket), o);
    Stack { pi, host }
}
