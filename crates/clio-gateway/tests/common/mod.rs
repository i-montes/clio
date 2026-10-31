#![allow(clippy::unwrap_used, dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use clio_core::Level;
use clio_gateway::{
    ActionRequest, Budget, Executor, Gateway, NormalizedAction, Policy, Rules, SessionInfo,
};
use serde_json::{Value, json};

static N: AtomicU32 = AtomicU32::new(0);

/// A real temporary tree: `work/` is the sandbox, `outside/` is not.
pub struct Env {
    pub root: PathBuf,
}

impl Env {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "clio-gw-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(root.join("work/extensions")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(root.join("outside/secret.txt"), "x").unwrap();
        std::fs::write(root.join("work/notes.txt"), "x").unwrap();
        Self { root }
    }

    pub fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    pub fn rules_json(&self, day_zero: bool) -> Value {
        let tool = |name: &str, effect: &str, egress: Value, args: Value| {
            json!({"name": name, "version": "1.0", "effect": effect, "egress": egress,
                   "description_hash": format!("h-{name}"), "args": args})
        };
        json!({
            "sandbox_root": self.work(),
            "allowed_binaries": ["ls", "echo", "git"],
            "trusted_domains": ["api.notion.com", "docs.rs"],
            "protected_paths": ["extensions"],
            "day_zero_complete": day_zero,
            "breaker_threshold": 1000,
            "tools": [
                tool("read_file", "Read", json!("none"), json!([{"name":"path","kind":"path","required":true}])),
                tool("write_file", "LocalReversible", json!("none"),
                     json!([{"name":"path","kind":"path","required":true},{"name":"text","kind":"text"}])),
                tool("run", "LocalReversible", json!("none"), json!([{"name":"cmd","kind":"argv","required":true}])),
                tool("http_get", "Read", json!("any"), json!([{"name":"url","kind":"url","required":true}])),
                tool("http_post", "ExternalWrite", json!("any"),
                     json!([{"name":"url","kind":"url","required":true},{"name":"body","kind":"text"}])),
                tool("notion_post", "ExternalWrite", json!({"domains": ["api.notion.com"]}),
                     json!([{"name":"body","kind":"text"},{"name":"url","kind":"url"}])),
                tool("delete_all", "Irreversible", json!("none"), json!([{"name":"path","kind":"path","required":true}])),
            ],
            "secret_bindings": [{"secret":"notion","tool":"notion_post","domains":["api.notion.com"]}]
        })
    }

    pub fn rules(&self) -> Rules {
        serde_json::from_value(self.rules_json(true)).unwrap()
    }

    pub fn gateway(&self) -> Gateway {
        Gateway::new(self.rules()).unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn req(tool: &str, args: Value) -> ActionRequest {
    ActionRequest {
        tool: tool.into(),
        version: "1.0".into(),
        description_hash: format!("h-{tool}"),
        args: args.as_object().unwrap().clone(),
        node: "laptop".into(),
        session: SessionInfo {
            id: "s1".into(),
            parent: None,
            level: Level::Simple,
            tainted: false,
            subagent: false,
        },
        actor: "user".into(),
        budget: Budget::default(),
        plan: vec![],
    }
}

pub struct Counter(pub AtomicU32);

impl Counter {
    pub fn new() -> Self {
        Self(AtomicU32::new(0))
    }
    pub fn runs(&self) -> u32 {
        self.0.load(Ordering::SeqCst)
    }
}

impl Executor for Counter {
    fn run(&self, _: &NormalizedAction, _: &str) -> Result<String, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("ok".into())
    }
}

pub struct Covers;

impl Policy for Covers {
    fn covers(&self, _: &NormalizedAction) -> bool {
        true
    }
}

pub struct Yes;

impl clio_gateway::Approver for Yes {
    fn approve(&self, _: &clio_gateway::Decision, _: &NormalizedAction) -> bool {
        true
    }
}
