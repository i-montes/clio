//! Shared by the tests that drive the `clio` binary against a throwaway HOME: a model that
//! answers "done" at once, so only what Clio does around the agent is under test.
#![allow(dead_code, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");

/// A model that answers "done" at once: only the copy is under test.
const ADAPTER: &str = r#"import json, sys
sys.stdin.read()
json.dump({"content": json.dumps({"final": "ok"}), "input_tokens": 10, "output_tokens": 5}, sys.stdout)
"#;

pub struct Env {
    pub tmp: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["home", "run", "proj", "script"] {
            std::fs::create_dir(tmp.path().join(d)).unwrap();
        }
        std::fs::write(tmp.path().join("script/adapter.py"), ADAPTER).unwrap();
        std::fs::write(tmp.path().join("script/host-check.sh"), "exit 0\n").unwrap();
        let e = Self { tmp };
        e.clio(&["init"]);
        let cfg_path = e.tmp.path().join("home/.config/clio/config.json");
        let mut cfg: Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        cfg["model"]["command"] = json!(["python3", e.tmp.path().join("script/adapter.py")]);
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        e
    }

    pub fn proj(&self) -> PathBuf {
        self.tmp.path().join("proj")
    }

    pub fn try_clio(&self, args: &[&str]) -> std::process::Output {
        Command::new(CLIO)
            .args(args)
            .env("HOME", self.tmp.path().join("home"))
            .env("XDG_RUNTIME_DIR", self.tmp.path().join("run"))
            .env("CLIO_SKIP_PI_INSTALL", "1")
            .env(
                "CLIO_HOST_CHECK",
                self.tmp.path().join("script/host-check.sh"),
            )
            .env_remove("OPENCODE_API_KEY")
            .current_dir(self.tmp.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    pub fn clio(&self, args: &[&str]) -> String {
        let o = self.try_clio(args);
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        assert!(
            o.status.success(),
            "clio {args:?}: {out}{}",
            String::from_utf8_lossy(&o.stderr)
        );
        out
    }

    /// Runs the agent on the project; returns the id of the run.
    pub fn run(&self) -> String {
        let out = self.clio(&[
            "run",
            "--workspace",
            &self.proj().display().to_string(),
            "mira",
        ]);
        out.lines()
            .find_map(|l| l.strip_prefix("run:").map(|s| s.trim().to_owned()))
            .unwrap()
    }

    pub fn sandbox(&self, id: &str) -> PathBuf {
        self.tmp
            .path()
            .join("home/.local/share/clio/runs")
            .join(id)
            .join("sandbox")
    }

    /// Runs the agent on the project and returns the files of the copy it was given.
    pub fn copied(&self) -> Vec<String> {
        let sandbox = self.sandbox(&self.run());
        let mut files = Vec::new();
        walk(&sandbox, &sandbox, &mut files);
        files.sort();
        files
    }
}

pub fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(base, &p, out);
        } else {
            out.push(p.strip_prefix(base).unwrap().display().to_string());
        }
    }
}

pub fn put(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}
