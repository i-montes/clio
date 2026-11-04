//! `clio memory mcp`: the person's memory for other agents over MCP (JSON-RPC 2.0, one message per
//! line). Read and propose only (S-25): what an agent proposes is the agent's, from outside, and
//! waits for the person (no Jev here); nothing can promote, archive, revert or purge.
#![allow(clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");

/// Sends `msgs` (one per line) to `clio memory mcp` started in the project folder, closes its
/// input and returns every answer, by id.
fn session(e: &common::Env, msgs: &[Value], raw: &[&str]) -> Vec<Value> {
    let mut c = Command::new(CLIO)
        .args(["memory", "mcp"])
        .env("HOME", e.tmp.path().join("home"))
        .env("XDG_RUNTIME_DIR", e.tmp.path().join("run"))
        .env("CLIO_SKIP_PI_INSTALL", "1")
        .current_dir(e.proj())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = c.stdin.take().unwrap();
        for m in msgs {
            writeln!(stdin, "{m}").unwrap();
        }
        for r in raw {
            writeln!(stdin, "{r}").unwrap();
        }
    }
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
}

fn by_id(answers: &[Value], id: u64) -> &Value {
    answers.iter().find(|a| a["id"] == json!(id)).unwrap()
}

fn text(a: &Value) -> String {
    a["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

// S-25, S-61: another agent reads the person's memory as data, proposes into the quarantine of the
// folder's project, and has no way to promote, archive, revert, pin or purge.
#[test]
fn an_agent_over_mcp_reads_and_proposes_and_nothing_else() {
    let e = common::Env::new();
    std::fs::create_dir_all(e.proj()).unwrap();
    e.clio(&["memory", "remember", "Marta usa Linux"]);
    let msgs = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "Claude Code", "version": "9"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        call(3, "memory_search", json!({"query": "Linux"})),
        call(
            4,
            "memory_propose",
            json!({"title": "Base de datos", "text": "Qué: usa Postgres 16",
            "category": "decision", "kind": "semantic"}),
        ),
        call(5, "memory_current_project", json!({})),
        call(6, "memory_forget", json!({"id": 1})),
        call(7, "memory_purge", json!({"id": 1})),
        json!({"jsonrpc": "2.0", "id": 8, "method": "resources/list"}),
        call(
            9,
            "memory_search",
            json!({"query": "Linux", "origin": "user"}),
        ),
        json!({"jsonrpc": "2.0", "id": 10, "method": "ping"}),
    ];
    let long = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"ping\",\"x\":\"{}\"}}",
        "a".repeat(2 << 20)
    );
    let answers = session(&e, &msgs, &["not json", &long]);
    // Every request has its answer; the notification has none.
    assert_eq!(
        answers.iter().filter(|a| a["id"].is_number()).count(),
        10,
        "{answers:#?}"
    );
    let init = by_id(&answers, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "clio-memory");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let mut names: Vec<String> = by_id(&answers, 2)["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "memory_context",
            "memory_current_project",
            "memory_get",
            "memory_propose",
            "memory_search",
            "memory_session_summary",
            "memory_timeline"
        ]
    );
    let found = text(by_id(&answers, 3));
    assert!(
        found.contains("Marta usa Linux") && found.contains("UNTRUSTED-DATA"),
        "{found}"
    );
    let proposed = by_id(&answers, 4);
    assert_eq!(proposed["result"]["isError"], json!(false), "{proposed}");
    assert!(text(proposed).starts_with("proposed"), "{proposed}");
    assert_eq!(text(by_id(&answers, 5)), "proj");
    for id in [6, 7, 9] {
        let a = by_id(&answers, id);
        assert!(
            a["error"].is_object() || a["result"]["isError"] == json!(true),
            "{id}: {a}"
        );
    }
    assert_eq!(by_id(&answers, 8)["error"]["code"], json!(-32601));
    assert!(by_id(&answers, 10)["result"].is_object());
    assert!(
        answers.iter().any(|a| a["error"]["code"] == json!(-32700)),
        "the bad line"
    );
    // The proposal waits for the person, as the agent's, from outside, in the project, with the
    // client's name as its session; the person's memory is as it was.
    let pending: Value =
        serde_json::from_str(&e.clio(&["memory", "list", "--pending", "--json"])).unwrap();
    let rows = pending.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{pending:#}");
    let row = &rows[0];
    assert_eq!(row["content"], "Qué: usa Postgres 16", "{row:#}");
    assert_eq!(row["origin"], "externo", "{row:#}");
    assert_eq!(row["actor"], "agente", "{row:#}");
    assert_eq!(row["run"], "mcp:claude-code", "{row:#}");
    assert_eq!(row["place"], "proyecto proj", "{row:#}");
    assert_eq!(row["title"], "Base de datos", "{row:#}");
    let active: Value = serde_json::from_str(&e.clio(&["memory", "list", "--json"])).unwrap();
    assert_eq!(active.as_array().unwrap().len(), 1, "{active:#}");
}
