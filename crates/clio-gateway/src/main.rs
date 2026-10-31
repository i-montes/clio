//! `clio-gateway <rules.json>`: reads one JSON request per line on stdin, writes one decision
//! per line on stdout. It runs as its own process so the loop cannot skip it (S-11). A line it
//! cannot read, or any failure, is a deny.

use std::io::{BufRead, Write};

use clio_gateway::{ActionRequest, Decision, Gateway};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: clio-gateway <rules.json>");
        std::process::exit(2);
    };
    let gateway = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| Gateway::from_json(&t))
    {
        Ok(g) => g,
        Err(e) => {
            eprintln!("clio-gateway: {e}");
            std::process::exit(2);
        }
    };
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let decision = match serde_json::from_str::<ActionRequest>(&line) {
            Ok(req) => gateway.decide_or_deny(&req),
            Err(e) => Decision::fail_closed(format!("unreadable request: {e}")),
        };
        let text = serde_json::to_string(&decision)
            .unwrap_or_else(|_| r#"{"outcome":"deny","rule":"fail-closed"}"#.into());
        if writeln!(out, "{text}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
}
