#![allow(clippy::unwrap_used)]
//! The Pi harness in the terminal interface, end to end and with no network: the REAL `clio`
//! binary in a pseudo-terminal (`pty_chat_driver.py`, Linux only), the REAL Pi with the
//! `clio-guard` extension and the real tool host (`clio toolhost`), against the fake model of
//! `testing/fake-provider/`. The HOME and the runtime folder are temporary. Also `clio run
//! --harness pi` without a terminal. Needs Node, `npm ci` in `adapters/pi` and `python3`
//! (skipped with a message otherwise; `CLIO_REQUIRE_PI=1` turns the skip into a failure).
//!
//! The interactive ones read the screen through `CLIO_TUI_SCREEN_DUMP`, which exists in debug
//! builds only: with `--release` they say so and skip.

mod pi_support;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use pi_support::{FakeProvider, need, repo};
use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");

/// Low-entropy decoy, built at run time.
fn decoy() -> String {
    format!("decoy-{}-{}", "key", "2222")
}

struct World {
    tmp: tempfile::TempDir,
    _provider: FakeProvider,
}

impl World {
    fn new(script: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        for d in ["home", "run", "proj"] {
            std::fs::create_dir(r.join(d)).unwrap();
        }
        std::fs::set_permissions(
            r.join("run"),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(r.join("proj/app.py"), "print('hola')\n").unwrap();
        let provider = FakeProvider::start(r, script, Some(&decoy()));
        let w = Self {
            tmp,
            _provider: provider,
        };
        let o = w.clio(&["init"]).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let cfg_path = w.home().join(".config/clio/config.json");
        let mut cfg: Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        cfg["model"]["provider"] = json!("fakeprov");
        cfg["model"]["model"] = json!("fm");
        cfg["model"]["base_url"] = json!(w._provider.base_url);
        cfg["model"]["price_in_micro_per_mtok"] = json!(1_000_000);
        cfg["model"]["price_out_micro_per_mtok"] = json!(2_000_000);
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        // "Install" Pi: the pinned version's folder is the checkout's adapters/pi (it has
        // node_modules and the guard).
        let pi = w.home().join(".local/share/clio/pi");
        std::fs::create_dir_all(&pi).unwrap();
        let version = clio_brain::pi::install::pinned_version().unwrap();
        std::os::unix::fs::symlink(repo().join("adapters/pi"), pi.join(version)).unwrap();
        w
    }

    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn envs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("HOME", self.home().display().to_string()),
            (
                "XDG_RUNTIME_DIR",
                self.root().join("run").display().to_string(),
            ),
            ("CLIO_SKIP_PI_INSTALL", "1".into()),
            ("OPENCODE_API_KEY", decoy()),
            ("PATH", std::env::var("PATH").unwrap_or_default()),
            ("TERM", "xterm-256color".into()),
            ("LANG", "C.UTF-8".into()),
        ]
    }

    fn clio(&self, args: &[&str]) -> Command {
        let mut c = Command::new(CLIO);
        c.args(args)
            .env_clear()
            .envs(self.envs())
            .current_dir(self.root().join("proj"))
            .stdin(Stdio::null());
        c
    }

    fn pty(&self, scenario: &str) -> Value {
        let cfg = json!({
            "clio": CLIO,
            "cwd": self.root().join("proj").display().to_string(),
            "root": self.root().display().to_string(),
            "dump": self.root().join("screen.txt").display().to_string(),
            "key": decoy(),
            "env": self.envs().into_iter().collect::<std::collections::BTreeMap<_, _>>(),
        });
        let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pty_chat_driver.py");
        let o = Command::new("python3")
            .arg("-I")
            .arg(driver)
            .arg(scenario)
            .arg(cfg.to_string())
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        assert!(
            o.status.success(),
            "{scenario}: {out}{}",
            String::from_utf8_lossy(&o.stderr)
        );
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert!(v["timeout"].is_null(), "{scenario} timed out: {v:#}");
        v
    }

    /// Every file under the temporary folder, to look for the key in.
    fn files(&self) -> Vec<PathBuf> {
        fn go(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                let t = e.file_type().unwrap();
                if t.is_dir() {
                    go(&p, out);
                } else if t.is_file() {
                    out.push(p);
                }
            }
        }
        let mut v = Vec::new();
        go(&self.home().join(".local"), &mut v);
        go(&self.home().join(".config"), &mut v);
        v
    }

    fn assert_key_nowhere(&self) {
        for f in self.files() {
            let b = std::fs::read(&f).unwrap();
            assert!(
                !String::from_utf8_lossy(&b).contains(&decoy()),
                "the API key is in {}",
                f.display()
            );
        }
    }
}

/// `Some(())` when this test can run here.
fn can_run(test: &str, interactive: bool) -> Option<()> {
    need(test, true)?;
    if Command::new("python3").arg("--version").output().is_err() {
        assert!(
            std::env::var("CLIO_REQUIRE_PI").as_deref() != Ok("1"),
            "CLIO_REQUIRE_PI=1 y falta python3: {test}"
        );
        eprintln!("SKIPPED {test}: falta python3");
        return None;
    }
    if interactive && !cfg!(debug_assertions) {
        eprintln!("SKIPPED {test}: la captura de pantalla solo existe en builds debug");
        return None;
    }
    Some(())
}

fn ok(v: &Value, keys: &[&str]) {
    for k in keys {
        assert_eq!(v[*k], json!(true), "{k}: {v:#}");
    }
    assert_eq!(
        v["raw_has_key"],
        json!(false),
        "the key reached the terminal: {v:#}"
    );
    assert_eq!(v["screen_has_key"], json!(false));
}

// S-18: the API key (a decoy here) reaches no file under Clio's folders, no screen and no terminal stream
#[test]
fn selector_new_session_chat_answer_debug_leave_and_quit_leave_nothing_behind() {
    if can_run("selector_new_chat", true).is_none() {
        return;
    }
    let w = World::new(r#"[{"text":"hello from the fake model","usage":[100,5]}]"#);
    let v = w.pty("new_chat");
    ok(
        &v,
        &[
            "selector",
            "remote_row",
            "opened",
            "answered",
            "user_line_shown",
            "usage_bar",
            "debug_raw_events",
            "back_in_selector",
            "listed_again",
            "procs_after_leaving",
            "remote_notice",
        ],
    );
    assert!(
        v["procs_while_open"].as_u64().unwrap() >= 2,
        "Pi and the toolhost: {v:#}"
    );
    assert_eq!(v["code"], json!(0), "{v:#}");
    assert_eq!(v["procs_after_quit"], json!(0), "{v:#}");
    // The session is Pi's own file, in Clio's sessions folder, and the copy is a run folder.
    let sessions = w.home().join(".local/share/clio/pi/sessions");
    let list = clio_brain::pi::sessions::list_sessions(&sessions);
    assert_eq!(list.sessions.len(), 1, "{:?}", list.warnings);
    assert_eq!(list.sessions[0].title, "hola");
    let id = &list.sessions[0].id;
    assert!(
        w.home()
            .join(".local/share/clio/runs")
            .join(id)
            .join("sandbox/app.py")
            .is_file()
    );
    w.assert_key_nowhere();
}

#[test]
fn aborting_a_turn_with_esc_leaves_the_session_usable() {
    if can_run("abort", true).is_none() {
        return;
    }
    let w = World::new(
        r#"[{"hang":true,"text":"never shown"},{"text":"answer after the abort","usage":[50,4]}]"#,
    );
    let v = w.pty("abort");
    ok(
        &v,
        &[
            "opened",
            "working",
            "aborting_shown",
            "idle_again",
            "slow_text_absent",
            "next_turn_answered",
        ],
    );
    assert_eq!(v["code"], json!(0), "{v:#}");
    assert_eq!(v["procs_after_quit"], json!(0), "{v:#}");
}

// S-18 S-36: the person supervises from the chat. When the host strikes keys out of what Pi read,
// the chat says how many, once, as a quiet note, and the keys are not on the screen.
#[test]
fn the_chat_says_when_values_were_struck_out_of_what_pi_read() {
    if can_run("masked", true).is_none() {
        return;
    }
    let w = World::new(
        r#"[{"tool_calls":[{"name":"read","arguments":{"reason":"leer","path":"keys.txt"}}]},{"text":"leído sin ver los valores","usage":[80,6]}]"#,
    );
    std::fs::write(
        w.root().join("proj/keys.txt"),
        concat!(
            "a=ghp_",
            "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8",
            "\nb=AKIA",
            "ABCDEFGHIJKLMNOP",
            "\n"
        ),
    )
    .unwrap();
    let v = w.pty("masked");
    ok(&v, &["opened", "final", "note", "keys_not_shown"]);
    assert_eq!(v["code"], json!(0), "{v:#}");
    assert_eq!(v["procs_after_quit"], json!(0), "{v:#}");
}

// S-05 S-16: the interface shows the session as having read outside content, and a write into .git is refused
#[test]
fn steering_a_running_turn_and_a_tool_line_with_its_duration() {
    if can_run("steer", true).is_none() {
        return;
    }
    let w = World::new(
        r#"[{"delay_ms":2500,"tool_calls":[{"name":"write","arguments":{"reason":"probar","path":"out.txt","content":"x\n"}},{"name":"write","arguments":{"reason":"probar","path":".git/hook","content":"x\n"}},{"name":"read","arguments":{"reason":"probar","path":"app.py"}}]},{"text":"done after the steer","usage":[80,6]}]"#,
    );
    let v = w.pty("steer");
    ok(
        &v,
        &[
            "opened",
            "working",
            "steer_sent",
            "tool_line",
            "final",
            "tool_ok_with_duration",
            "taint_hidden",
        ],
    );
    assert_eq!(v["code"], json!(0), "{v:#}");
    assert_eq!(v["procs_after_quit"], json!(0), "{v:#}");
    // The write went to the copy, never to the original project.
    assert!(!w.root().join("proj/out.txt").exists());
    // S-16: the protected folder was refused (nothing was written there).
    let sandbox = std::fs::read_dir(w.home().join(".local/share/clio/runs"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path()
        .join("sandbox");
    assert!(!sandbox.join(".git/hook").exists());
    let runs = w.home().join(".local/share/clio/runs");
    let copy = std::fs::read_dir(&runs)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path()
        .join("sandbox/out.txt");
    assert_eq!(std::fs::read_to_string(copy).unwrap(), "x\n");
}

// S-18: reopening a session writes the API key (a decoy here) nowhere
#[test]
fn the_selector_lists_a_session_and_reopens_it_with_its_history() {
    if can_run("reopen", true).is_none() {
        return;
    }
    let w = World::new(
        r#"[{"text":"first answer","usage":[100,5]},{"text":"second answer","usage":[200,7]}]"#,
    );
    let v = w.pty("reopen");
    ok(
        &v,
        &[
            "opened",
            "answered",
            "selector",
            "row_listed",
            "reopened",
            "history_user",
            "history_answer",
            "answered_again",
            "history_still_there",
        ],
    );
    assert_eq!(
        (v["code1"].as_i64(), v["code2"].as_i64()),
        (Some(0), Some(0)),
        "{v:#}"
    );
    assert_eq!(v["procs_after_first"], json!(0), "{v:#}");
    assert_eq!(v["procs_after_quit"], json!(0), "{v:#}");
    // Still ONE session: it was reopened, not copied.
    let list =
        clio_brain::pi::sessions::list_sessions(&w.home().join(".local/share/clio/pi/sessions"));
    assert_eq!(list.sessions.len(), 1);
    w.assert_key_nowhere();
}

#[test]
fn killing_clio_with_sigkill_leaves_no_pi_and_no_toolhost() {
    if can_run("sigkill", true).is_none() {
        return;
    }
    let w = World::new(r#"[{"text":"hello from the fake model"}]"#);
    let v = w.pty("sigkill");
    ok(&v, &["opened", "orphans_gone"]);
    assert!(v["procs_while_open"].as_u64().unwrap() >= 2, "{v:#}");
    assert_eq!(v["orphans_left"], json!([]));
}

// S-18 S-14: the run is a copy (the original is untouched) and the API key stays out of every file
#[test]
fn clio_run_with_pi_prints_the_answer_and_the_change_shows_in_diff() {
    if can_run("run_pi", false).is_none() {
        return;
    }
    let w = World::new(
        r#"[{"tool_calls":[{"name":"write","arguments":{"reason":"probar","path":"new.txt","content":"hi\n"}}]},{"text":"hello from the fake model","usage":[100,5]}]"#,
    );
    let o = w
        .clio(&["run", "--harness", "pi", "haz algo"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        o.status.success(),
        "{out}{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        out.contains("hello from the fake model") && out.contains("[write] ok"),
        "{out}"
    );
    // 100 tokens in at $1/Mtok and 5 out at $2/Mtok, plus the first call's.
    assert!(out.contains("uso:") && out.contains('$'), "{out}");
    let id = out
        .lines()
        .find_map(|l| l.strip_prefix("run: "))
        .unwrap()
        .trim()
        .to_owned();
    assert!(
        !w.root().join("proj/new.txt").exists(),
        "the original was touched"
    );
    let d = w.clio(&["diff", &id]).output().unwrap();
    let diff = String::from_utf8_lossy(&d.stdout).into_owned();
    assert!(
        d.status.success() && diff.contains("new.txt"),
        "{diff}{}",
        String::from_utf8_lossy(&d.stderr)
    );
    w.assert_key_nowhere();
}

#[test]
fn harness_flag_is_validated_and_pi_without_an_install_is_an_error_that_says_why() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let run = |args: &[&str]| {
        Command::new(CLIO)
            .args(args)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let o = run(&["run", "--harness", "nope", "x"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("no es válido"));
    let o = run(&["run", "--harness", "pi", "x"]);
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("Pi") || e.contains("Node"), "{e}");
}

// ---- Jev: the time, the memory and the tools in front of the message (spec: Jev, entrega 2) ----

const JEV_PASS: &str = "correct horse battery staple";

/// The binary of another package (`CARGO_BIN_EXE_` does not cover it): next to this test's own
/// executable, built if it is not there yet.
fn vault_bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.args(["build", "-q", "-p", "clio-vault", "--bin", "clio-vault"]);
    if profile_dir.file_name().is_some_and(|n| n == "release") {
        cmd.arg("--release");
    }
    assert!(cmd.status().unwrap().success(), "cannot build clio-vault");
    profile_dir.join("clio-vault")
}

/// A real vault process on the sockets `clio` looks at, holding the secret `jev`.
struct Vault(std::process::Child);

impl Drop for Vault {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_vault(w: &World, key: &str) -> Vault {
    use clio_vault::proto::Request;
    use clio_vault::{Dest, Kind, Scope, SecretSpec, VaultClient};
    let file = w.home().join(".local/share/clio/vault.bin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let (v, cap) = clio_vault::Vault::init(
        Some(&file),
        JEV_PASS,
        std::sync::Arc::new(clio_core::SystemClock),
    )
    .unwrap();
    v.put(
        &cap,
        SecretSpec {
            name: "jev".into(),
            value: key.into(),
            kind: Kind::ApiKey,
            destinations: vec![Dest::new("jev", "api.typesafe.ai")],
            scope: Scope::Personal,
            rotate_every_ms: None,
            expires_at_ms: None,
        },
    )
    .unwrap();
    drop(v);
    // `clio` looks in `$XDG_RUNTIME_DIR/clio`, a private folder.
    let run = w.root().join("run/clio");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::set_permissions(&run, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let child = Command::new(vault_bin())
        .args(["serve", "--vault"])
        .arg(&file)
        .arg("--gateway-sock")
        .arg(run.join("vault-gateway.sock"))
        .arg("--person-sock")
        .arg(run.join("vault-person.sock"))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let person = VaultClient::new(&run.join("vault-person.sock"));
    let t = std::time::Instant::now();
    while person.call(&Request::Status).is_err() {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "vault never came up"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let unlock = Request::Unlock {
        passphrase: JEV_PASS.into(),
        eager: vec![],
    };
    assert!(person.call(&unlock).unwrap().ok);
    Vault(child)
}

/// A stand-in for api.typesafe.ai on loopback: says yes (0.9) to the questions about memory and
/// to `read`, no (0.1) to the rest, and remembers what it was sent.
/// The `Authorization` header and the body of each request.
type Seen = std::sync::Arc<std::sync::Mutex<Vec<(Option<String>, Value)>>>;

struct FakeJev {
    base: String,
    seen: Seen,
}

fn fake_jev() -> FakeJev {
    use std::io::{BufRead, BufReader, Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut c) = conn else { return };
            let mut r = BufReader::new(c.try_clone().unwrap());
            let (mut auth, mut len) = (None, 0usize);
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("authorization:") {
                    auth = Some(line.trim().to_owned());
                }
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            let _ = r.read_exact(&mut body);
            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let mut answers = serde_json::Map::new();
            for q in req["questions"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| k)
            {
                let about_a_memory = q
                    .strip_prefix('m')
                    .is_some_and(|n| n.parse::<u32>().is_ok());
                let yes = q == "necesita_memoria" || q == "read" || about_a_memory;
                answers.insert(
                    q.clone(),
                    json!({"type": "noul", "noul": if yes { 0.9 } else { 0.1 }}),
                );
            }
            let reply =
                json!({"model": "jev-fake-1", "answers": answers, "usage": {"input_tokens": 20}})
                    .to_string();
            s2.lock().unwrap().push((auth, req));
            let _ = write!(
                c,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    FakeJev { base, seen }
}

// S-62, S-63
#[test]
fn with_jev_on_the_model_gets_the_time_the_memory_and_the_tool_suggestions_in_front_of_the_message()
{
    if can_run("jev_envelope", false).is_none() {
        return;
    }
    let w = World::new(r#"[{"text":"listo","usage":[10,2]}]"#);
    let key = format!("jev-test-{}-{}", "key", "7777");
    let _vault = start_vault(&w, &key);
    let jev = fake_jev();
    let cfg_path = w.home().join(".config/clio/config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["jev"] = json!({"enabled": true, "base_url": jev.base});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    let r = w
        .clio(&["memory", "remember", "Marta vive en Medellín"])
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let o = w
        .clio(&["run", "--harness", "pi", "¿dónde vivo?"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        o.status.success(),
        "{out}{}",
        String::from_utf8_lossy(&o.stderr)
    );
    // What the model was asked: the envelope, then what the person typed.
    let log = std::fs::read_to_string(&w._provider.log).unwrap();
    let asked: Value = log
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|e| e["last_user"].is_string())
        .unwrap();
    let said = asked["last_user"].as_str().unwrap();
    assert!(said.starts_with("<clio-contexto>\nhora: 20"), "{said}");
    assert!(
        said.contains("[m1] persona: Marta vive en Medellín"),
        "{said}"
    );
    assert!(said.contains("solo una sugerencia): read\n"), "{said}");
    assert!(said.ends_with("</clio-contexto>\n\n¿dónde vivo?"), "{said}");
    // What Jev was sent: the key, as a bearer, and only closed questions about the message.
    let seen = jev.seen.lock().unwrap();
    assert!(seen.len() >= 2, "{} calls", seen.len());
    for (auth, body) in seen.iter() {
        assert_eq!(
            auth.as_deref(),
            Some(format!("Authorization: Bearer {key}").as_str())
        );
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"]["mensaje_de_la_persona"], "¿dónde vivo?");
        assert!(!body.to_string().contains(&key));
    }
    assert!(
        !out.contains("<clio-contexto"),
        "the person never sees it: {out}"
    );
    w.assert_key_nowhere();
}

// S-63: a memory that is not the person's goes in front of the message marked as such, and the
// session is tainted by the tool host (not by Pi): what the model is shown counts as read.
#[test]
fn a_memory_of_the_agent_in_the_envelope_taints_the_session_in_the_tool_host() {
    if can_run("jev_taint", false).is_none() {
        return;
    }
    let w = World::new(r#"[{"text":"listo","usage":[10,2]}]"#);
    let data = w.home().join(".local/share/clio");
    {
        use clio_brain::memory_link::{EmbedModel, MemoryLink};
        use clio_memory::MemoryType;
        std::fs::create_dir_all(&data).unwrap();
        let link = MemoryLink::open_write(&data, &EmbedModel::none()).unwrap();
        let id = link
            .propose_as_agent("El proyecto usa PostgreSQL 16", MemoryType::Semantic, "s1")
            .unwrap();
        link.promote(id).unwrap();
    }
    let o = w
        .clio(&[
            "run",
            "--harness",
            "pi",
            "¿qué base de datos usa el proyecto?",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let log = std::fs::read_to_string(&w._provider.log).unwrap();
    let said = log
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|e| e["last_user"].as_str().map(str::to_owned))
        .unwrap();
    assert!(
        said.contains("[m1] agente o externo: El proyecto usa PostgreSQL 16"),
        "{said}"
    );
    let tainted: Vec<PathBuf> = {
        fn find(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    find(&p, out);
                } else if p.file_name().is_some_and(|n| n == "tainted.json") {
                    out.push(p);
                }
            }
        }
        let mut v = Vec::new();
        find(&data, &mut v);
        v
    };
    assert_eq!(
        tainted.len(),
        1,
        "the tool host recorded the taint: {tainted:?} / {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        std::fs::read_to_string(&tainted[0])
            .unwrap()
            .contains("memory")
    );
}
