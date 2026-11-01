//! Helpers for the Pi tests: Node and the real Pi (skipped with a visible message when missing,
//! a failure with `CLIO_REQUIRE_PI=1`), the fake model server of `testing/fake-provider/`, and a
//! rig that builds a `Launch` inside a temporary folder. Children are killed on `Drop`.
#![allow(dead_code, clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use clio_brain::pi::PiPaths;
use clio_brain::pi::install::{NodeInfo, PiInstall, find_node};
use clio_brain::pi::launch::{Launch, Provider, Secret, SessionTarget};

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub fn node() -> Option<NodeInfo> {
    find_node(&std::env::var("PATH").unwrap_or_default()).ok()
}

pub fn real_pi_bin() -> PathBuf {
    repo().join("adapters/pi/node_modules/.bin/pi")
}

/// Node (and, with `need_pi`, the real Pi from `adapters/pi`). Prints SKIPPED when missing;
/// panics instead when `CLIO_REQUIRE_PI=1`.
pub fn need(test: &str, need_pi: bool) -> Option<NodeInfo> {
    let node = node();
    let pi_ok = !need_pi || real_pi_bin().exists();
    if let (Some(n), true) = (&node, pi_ok) {
        return Some(n.clone());
    }
    let why = if node.is_none() {
        "Node 22.19+ no está instalado"
    } else {
        "falta adapters/pi/node_modules (cd adapters/pi && npm ci --ignore-scripts)"
    };
    assert!(
        std::env::var("CLIO_REQUIRE_PI").as_deref() != Ok("1"),
        "CLIO_REQUIRE_PI=1 y {why}: {test}"
    );
    eprintln!("SKIPPED {test}: {why}");
    None
}

/// Kills the child (and waits) when dropped.
pub struct Guard(pub Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub struct FakeProvider {
    _g: Guard,
    pub base_url: String,
    pub log: PathBuf,
}

impl FakeProvider {
    /// `script` is the JSON list of replies (see testing/fake-provider/server.py).
    pub fn start(dir: &Path, script: &str, expect_key: Option<&str>) -> Self {
        let sp = dir.join("script.json");
        std::fs::write(&sp, script).unwrap();
        let log = dir.join("provider.jsonl");
        let mut cmd = Command::new("python3");
        cmd.arg(repo().join("testing/fake-provider/server.py"))
            .arg("--script")
            .arg(&sp)
            .arg("--log")
            .arg(&log);
        if let Some(k) = expect_key {
            cmd.args(["--expect-key", k]);
        }
        let mut child = cmd.stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        Self {
            _g: Guard(child),
            base_url: v["base_url"].as_str().unwrap().to_owned(),
            log,
        }
    }

    /// The requests the server saw.
    pub fn requests(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

/// A temporary Pi home for Clio and a `Launch` that uses the real Pi of `adapters/pi`.
pub struct Rig {
    pub tmp: tempfile::TempDir,
    pub paths: PiPaths,
    pub proj: PathBuf,
    pub node: NodeInfo,
}

impl Rig {
    pub fn new(node: NodeInfo) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = PiPaths::under_data(&tmp.path().join("data"));
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        Self {
            tmp,
            paths,
            proj,
            node,
        }
    }

    pub fn launch(&self, base_url: &str, key: Option<&str>) -> Launch {
        let mut provider = Provider::opencode_go("fm");
        provider.id = "fakeprov".into();
        provider.base_url = base_url.into();
        Launch {
            install: PiInstall {
                version: "test".into(),
                dir: repo().join("adapters/pi"),
                bin: real_pi_bin(),
            },
            node: self.node.clone(),
            paths: self.paths.clone(),
            provider,
            api_key: key.map(Secret::new),
            cwd: self.proj.clone(),
            session: SessionTarget::New {
                id: Some("sess-test-1".into()),
            },
            name: None,
            guard: None,
            guard_tools: vec![],
            extra_env: vec![],
        }
    }
}

pub fn pid_alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // `pid (comm) S ...`: a zombie is as good as dead.
        Ok(s) => s
            .rsplit(')')
            .next()
            .is_some_and(|r| !r.trim_start().starts_with('Z')),
        Err(_) => false,
    }
}
