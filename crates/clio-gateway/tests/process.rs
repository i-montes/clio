#![allow(clippy::unwrap_used)]

mod common;

use clio_gateway::{GatewayClient, Outcome};
use common::{Env, req};
use serde_json::json;

// S-11: the gateway runs as its own process; with it down, nothing is allowed.
#[test]
fn t05_with_the_gateway_process_down_no_action_is_allowed() {
    let env = Env::new();
    let rules = env.root.join("rules.json");
    std::fs::write(&rules, env.rules_json(true).to_string()).unwrap();
    let mut c =
        GatewayClient::spawn(env!("CARGO_BIN_EXE_clio-gateway"), rules.to_str().unwrap()).unwrap();
    let notes = env.work().join("notes.txt").to_string_lossy().into_owned();
    let ok = req("read_file", json!({"path": notes}));
    assert_eq!(c.decide(&ok).outcome, Outcome::Allow);
    assert_eq!(c.decide(&req("shell", json!({}))).rule, "S-02");
    c.kill();
    let d = c.decide(&ok);
    assert_eq!((d.outcome, d.rule.as_str()), (Outcome::Deny, "fail-closed"));
}

#[test]
fn a_gateway_that_cannot_read_its_rules_does_not_start() {
    let c = std::process::Command::new(env!("CARGO_BIN_EXE_clio-gateway"))
        .arg("/nonexistent/rules.json")
        .output()
        .unwrap();
    assert!(!c.status.success());
}
