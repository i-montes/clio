//! A fake OpenAI-compatible model (`/v1/chat/completions`, streaming), on 127.0.0.1 only. A
//! handler closure decides what the "model" says given the request it just received, so a test
//! can write a model that does exactly what an attacker's text tells it to. Every request is
//! kept, so a test can see what the model was really sent (system prompt, tools, tool results).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};

pub enum Turn {
    Text(String),
    Calls(Vec<(String, Value)>),
}

pub type Handler = Box<dyn FnMut(&Value) -> Turn + Send>;

pub struct FakeModel {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeModel {
    pub fn start(handler: Handler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake model");
        let addr = listener.local_addr().expect("addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(Mutex::new(handler));
        let (reqs, flag) = (requests.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let (handler, reqs) = (handler.clone(), reqs.clone());
                std::thread::spawn(move || serve(conn, &handler, &reqs));
            }
        });
        Self {
            addr,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    /// The model says each turn in order, and "done" once the script is over.
    pub fn scripted(turns: Vec<Turn>) -> Self {
        let mut turns = std::collections::VecDeque::from(turns);
        Self::start(Box::new(move |_| {
            turns
                .pop_front()
                .unwrap_or_else(|| Turn::Text("done".into()))
        }))
    }

    pub fn requests(&self) -> Vec<Value> {
        lock(&self.requests).clone()
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The text of the last tool message in an OpenAI-style request, if the last message is one.
pub fn last_tool_text(req: &Value) -> Option<String> {
    let m = req["messages"].as_array()?.last()?;
    (m["role"] == "tool").then(|| match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    })
}

/// A model that obeys the external text it reads: a line `ATTACK-CALL {"tool":..,"args":{..}}`
/// inside a tool result becomes the next tool call. Otherwise it says "done".
pub fn gullible_from_tool_text(req: &Value) -> Turn {
    if let Some(text) = last_tool_text(req) {
        for line in text.lines() {
            if let Some(json) = line.trim().strip_prefix("ATTACK-CALL ")
                && let Ok(v) = serde_json::from_str::<Value>(json)
                && let Some(tool) = v["tool"].as_str()
            {
                return Turn::Calls(vec![(tool.to_owned(), v["args"].clone())]);
            }
        }
    }
    Turn::Text("done".into())
}

fn serve(mut conn: TcpStream, handler: &Mutex<Handler>, reqs: &Mutex<Vec<Value>>) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(30)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let first = head.lines().next().unwrap_or_default().to_owned();
    let len = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + len {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = &buf[head_end..head_end + len];
    if first.starts_with("GET") && first.contains("/models") {
        let b = json!({"object":"list","data":[{"id":"fake-model","object":"model"}]}).to_string();
        let _ = write!(
            conn,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
            b.len()
        );
        return;
    }
    if !(first.starts_with("POST") && first.contains("/chat/completions")) {
        let _ = write!(
            conn,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        return;
    }
    let req: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    lock(reqs).push(req.clone());
    let turn = (lock(handler))(&req);
    let chunk_of = |delta: Value, finish: Value| {
        json!({"id":"chatcmpl-fake","object":"chat.completion.chunk","created":0,"model":"fake-model",
               "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    };
    let mut events = Vec::new();
    match turn {
        Turn::Text(t) => {
            events.push(chunk_of(
                json!({"role":"assistant","content":t}),
                Value::Null,
            ));
            events.push(chunk_of(json!({}), json!("stop")));
        }
        Turn::Calls(calls) => {
            let tcs: Vec<Value> = calls
                .iter()
                .enumerate()
                .map(|(i, (name, args))| {
                    json!({"index":i,"id":format!("call_{}_{i}", lock(reqs).len()),"type":"function",
                           "function":{"name":name,"arguments":args.to_string()}})
                })
                .collect();
            events.push(chunk_of(
                json!({"role":"assistant","content":null,"tool_calls":tcs}),
                Value::Null,
            ));
            events.push(chunk_of(json!({}), json!("tool_calls")));
        }
    }
    events.push(json!({"id":"chatcmpl-fake","object":"chat.completion.chunk","created":0,"model":"fake-model","choices":[],
                       "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}));
    let mut out = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    );
    for e in events {
        out.push_str(&format!("data: {e}\n\n"));
    }
    out.push_str("data: [DONE]\n\n");
    let _ = conn.write_all(out.as_bytes());
}
