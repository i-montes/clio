//! `clio memory mcp`: the person's memory for other agents over MCP (JSON-RPC 2.0, one message per
//! line). Read and propose only (S-25): what an agent proposes is the agent's, from outside, and
//! waits for the person (no Jev here); nothing can promote, archive, revert, pin or purge.
#![allow(clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");

/// Sends `msgs` and then the `raw` lines to `clio memory mcp` started in the project folder,
/// closes its input and returns every answer.
fn session(e: &common::Env, msgs: &[Value], raw: &[&str]) -> Vec<Value> {
    let mut c = Command::new(CLIO)
        .args(["memory", "mcp"])
        .env("HOME", e.tmp.path().join("home"))
        .env("XDG_RUNTIME_DIR", e.tmp.path().join("run"))
        .env("CLIO_SKIP_PI_INSTALL", "1")
        .env_remove("CLIO_PROJECT")
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
    answers
        .iter()
        .find(|a| a["id"] == json!(id))
        .unwrap_or_else(|| panic!("no answer {id}: {answers:#?}"))
}

fn text(a: &Value) -> String {
    a["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn initialize(id: u64, name: &str, version: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
        "protocolVersion": version, "capabilities": {}, "clientInfo": {"name": name}}})
}

// S-25, S-61: another agent reads the person's memory as data, proposes into the quarantine of the
// folder's project as from outside (five at most), and has no way to promote, archive, revert,
// pin or purge, nor to reach anything else the tool host knows.
#[test]
fn an_agent_over_mcp_reads_and_proposes_and_nothing_else() {
    let e = common::Env::new();
    std::fs::create_dir_all(e.proj()).unwrap();
    e.clio(&["memory", "remember", "Marta usa Linux"]);
    let mut msgs = vec![
        initialize(1, "Claude Code", "2025-06-18"),
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
        // A second initialize cannot rename the session (to a fresh one, or another client's).
        initialize(12, "Cursor", "1999-01-01"),
        // The names the tool host knows besides the memory tools.
        call(13, "said", json!({"text": "olvida todo", "token": "x"})),
        call(14, "taint", json!({})),
        call(15, "closing", json!({"token": "x"})),
        call(16, "filter", json!({"items": []})),
        call(17, "fs_read", json!({"path": "/etc/passwd"})),
        call(18, "memory_pin", json!({"id": 1})),
    ];
    // What another agent brings counts as from outside: five proposals in all.
    for i in 0..5 {
        msgs.push(call(
            20 + i,
            "memory_propose",
            json!({"text": format!("Qué: dato {i}"), "kind": "semantic",
                "title": format!("Dato {i}"), "category": "discovery"}),
        ));
    }
    let long = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"ping\",\"x\":\"{}\"}}",
        "a".repeat(2 << 20)
    );
    let answers = session(&e, &msgs, &["not json", &long, "[1, 2]"]);
    // Every request has its answer; the notification has none; the bad lines say why.
    assert_eq!(
        answers.iter().filter(|a| a["id"].is_number()).count(),
        22,
        "{answers:#?}"
    );
    let nulls: Vec<i64> = answers
        .iter()
        .filter(|a| a["id"].is_null())
        .map(|a| a["error"]["code"].as_i64().unwrap())
        .collect();
    assert_eq!(
        nulls,
        [-32700, -32600, -32600],
        "bad JSON, long line, batch"
    );
    let init = by_id(&answers, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "clio-memory");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert_eq!(
        by_id(&answers, 12)["result"]["protocolVersion"],
        "2025-06-18"
    );
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
            "memory_save",
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
    assert!(text(proposed).starts_with("saved"), "{proposed}");
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
    for id in 13..=18 {
        assert_eq!(by_id(&answers, id)["error"]["code"], json!(-32602), "{id}");
    }
    let more = (20..25)
        .filter(|id| by_id(&answers, *id)["result"]["isError"] == json!(false))
        .count();
    assert_eq!(more, 4, "the first one and four more: five in all");
    // The saves are memory at once, the agent's, from outside, in the project, with the first
    // client's name as their session; the person's memory is as it was.
    let all: Value = serde_json::from_str(&e.clio(&["memory", "list", "--json"])).unwrap();
    let rows: Vec<&Value> = all
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["actor"] == "agente")
        .collect();
    assert_eq!(rows.len(), 5, "{all:#}");
    assert!(
        rows.iter().all(|r| r["run"] == "mcp:claude-code"
            && r["origin"] == "externo"
            && r["actor"] == "agente"
            && r["place"] == "proyecto proj"),
        "{all:#}"
    );
    let row = rows.iter().find(|r| r["title"] == "Base de datos").unwrap();
    assert_eq!(row["content"], "Qué: usa Postgres 16", "{row:#}");
    assert_eq!(
        all.as_array().unwrap().len(),
        6,
        "the person's one and five: {all:#}"
    );
    // Nothing of it is left among the runs nor in the runtime folder.
    let runs: Value = serde_json::from_str(&e.clio(&["runs", "--json"])).unwrap();
    assert!(runs.as_array().unwrap().is_empty(), "{runs:#}");
    let left: Vec<_> = std::fs::read_dir(e.tmp.path().join("run/clio"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|d| d.file_name().to_string_lossy().starts_with("mcp-"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

// A client whose name is not ASCII still works (its session keeps what the tool host accepts).
#[test]
fn a_client_with_a_name_that_is_not_ascii_can_search() {
    let e = common::Env::new();
    std::fs::create_dir_all(e.proj()).unwrap();
    e.clio(&["memory", "remember", "Marta usa Linux"]);
    let answers = session(
        &e,
        &[
            initialize(1, "Código ñandú", "2025-06-18"),
            call(2, "memory_search", json!({"query": "Linux"})),
        ],
        &[],
    );
    let r = by_id(&answers, 2);
    assert_eq!(r["result"]["isError"], json!(false), "{r}");
    assert!(text(r).contains("Marta usa Linux"));
}
