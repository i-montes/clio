#![allow(clippy::unwrap_used)]
//! The `clio` command line end to end, with no network: a scripted adapter plays the model (a
//! small Python program that answers from a list), the HOME is a temporary folder, and the
//! workspace is another one. Fake secrets are built at run time.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

mod pi_support;

const CLIO: &str = env!("CARGO_BIN_EXE_clio");

const ADAPTER: &str = r#"import json, os, sys, time
seen = sys.stdin.read()
time.sleep(float(os.environ.get("ADAPTER_SLEEP", "0")))
d = os.path.dirname(os.path.abspath(__file__))
open(d + "/seen", "a").write(seen + "\n")
n = int(open(d + "/n").read()) if os.path.exists(d + "/n") else 0
r = json.load(open(d + "/responses.json"))
open(d + "/n", "w").write(str(n + 1))
json.dump({"content": r[min(n, len(r) - 1)], "input_tokens": 1000 * (n + 1), "output_tokens": 50}, sys.stdout)
"#;

const OLD_APP: &str = "def total(xs):\n    return sum(xs) + 1\n";
const NEW_APP: &str = "def total(xs):\n    return sum(xs)\n";

struct Env {
    tmp: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["home", "run", "proj", "script"] {
            std::fs::create_dir(tmp.path().join(d)).unwrap();
        }
        std::fs::write(tmp.path().join("script/adapter.py"), ADAPTER).unwrap();
        std::fs::write(tmp.path().join("script/host-check.sh"), "exit 0\n").unwrap();
        let p = tmp.path().join("proj");
        std::fs::write(p.join("app.py"), OLD_APP).unwrap();
        std::fs::write(p.join("notes.txt"), "keep me\n").unwrap();
        std::fs::write(p.join(".env"), format!("TOKEN={}\n", "x".repeat(12))).unwrap();
        std::fs::create_dir(p.join(".git")).unwrap();
        std::fs::write(p.join(".git/config"), "[core]\n").unwrap();
        let e = Self { tmp };
        e.clio(&["init"]);
        // Use the scripted adapter instead of the real one.
        let cfg_path = e.home().join(".config/clio/config.json");
        let mut cfg: Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        cfg["model"]["command"] = json!(["python3", e.path("script/adapter.py")]);
        // The builtin loop works on a copy; host is the factory mode.
        cfg["mode"] = json!("copy");
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        e
    }

    fn path(&self, rel: &str) -> String {
        self.tmp.path().join(rel).display().to_string()
    }
    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }
    fn proj(&self) -> PathBuf {
        self.tmp.path().join("proj")
    }

    fn script(&self, replies: &[Value]) {
        let texts: Vec<String> = replies.iter().map(Value::to_string).collect();
        std::fs::write(
            self.tmp.path().join("script/responses.json"),
            serde_json::to_string(&texts).unwrap(),
        )
        .unwrap();
        let _ = std::fs::remove_file(self.tmp.path().join("script/n"));
    }

    /// The model reads app.py, fixes it, creates a new file in a new folder, and finishes.
    fn script_fix(&self) {
        self.script(&[
            json!({"tool": "read_file", "args": {"path": "app.py"}}),
            json!({"tool": "write_file", "args": {"path": "app.py", "content": NEW_APP}}),
            json!({"tool": "write_file", "args": {"path": "docs/new.md", "content": "hello\n"}}),
            json!({"final": "fixed total(); could not run anything"}),
        ]);
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(CLIO);
        c.args(args)
            .env("HOME", self.home())
            .env("XDG_RUNTIME_DIR", self.tmp.path().join("run"))
            .env("CLIO_SKIP_PI_INSTALL", "1")
            .env(
                "CLIO_HOST_CHECK",
                self.tmp.path().join("script/host-check.sh"),
            )
            .env_remove("OPENCODE_API_KEY")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .current_dir(self.tmp.path())
            .stdin(Stdio::null());
        c
    }

    fn try_clio(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn clio(&self, args: &[&str]) -> String {
        let o = self.try_clio(args);
        assert!(
            o.status.success(),
            "clio {args:?}: {}{}",
            text(&o.stdout),
            text(&o.stderr)
        );
        text(&o.stdout)
    }

    fn run_fix(&self) -> String {
        self.script_fix();
        let out = self.clio(&[
            "run",
            "--workspace",
            &self.path("proj"),
            "fix total in app.py",
        ]);
        out.lines()
            .find_map(|l| l.strip_prefix("run:").map(|s| s.trim().to_owned()))
            .unwrap()
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.proj().join(rel)).unwrap()
    }

    /// Runs `clio` inside a real pseudo-terminal (see `pty_driver.py`) and returns its verdict.
    fn pty(&self, scenario: &str, adapter_sleep: &str) -> Value {
        let cfg = json!({
            "clio": CLIO,
            "cwd": self.path("proj"),
            "runs": self.home().join(".local/share/clio/runs").display().to_string(),
            "app_py": self.proj().join("app.py").display().to_string(),
            "old": OLD_APP,
            "new": NEW_APP,
            "env": {
                "HOME": self.home().display().to_string(),
                "XDG_RUNTIME_DIR": self.path("run"),
                "CLIO_HOST_CHECK": self.path("script/host-check.sh"),
                "TERM": "xterm-256color",
                "LANG": "C.UTF-8",
                "PATH": std::env::var("PATH").unwrap_or_default(),
                "ADAPTER_SLEEP": adapter_sleep,
            },
        });
        let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pty_driver.py");
        let o = Command::new("python3")
            .arg("-I")
            .arg(driver)
            .arg(scenario)
            .arg(cfg.to_string())
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{scenario}: {}{}",
            text(&o.stdout),
            text(&o.stderr)
        );
        serde_json::from_str(text(&o.stdout).trim()).unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn full_cycle_run_diff_apply_undo() {
    let e = Env::new();
    let out_run = {
        e.script_fix();
        e.clio(&["run", "--workspace", &e.path("proj"), "fix total in app.py"])
    };
    // run did NOT touch the original, and said what it left out.
    assert_eq!(e.read("app.py"), OLD_APP);
    assert!(!e.proj().join("docs").exists());
    assert!(
        out_run.contains("NOT copied") && out_run.contains(".env"),
        "{out_run}"
    );
    assert!(
        out_run.contains("1 modified, 1 created, 0 deleted"),
        "{out_run}"
    );
    assert!(out_run.contains("clio diff") && out_run.contains("clio apply"));
    let id = out_run
        .lines()
        .find_map(|l| l.strip_prefix("run:").map(|s| s.trim().to_owned()))
        .unwrap();

    // The secret and .git never entered the sandbox; the run folder holds the artifacts.
    let run_dir = e.home().join(".local/share/clio/runs").join(&id);
    assert!(run_dir.join("sandbox/app.py").exists());
    assert!(!run_dir.join("sandbox/.env").exists());
    assert!(!run_dir.join("sandbox/.git").exists());
    for f in [
        "contract.json",
        "transcript.json",
        "report.json",
        "summary.json",
        "manifest.json",
    ] {
        // contract.json only exists from level 2 up; the rest always.
        if f != "contract.json" {
            assert!(run_dir.join(f).exists(), "{f}");
        }
    }
    let summary: Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["modified"], json!(["app.py"]));
    assert_eq!(summary["created"], json!(["docs/new.md"]));

    let diff = e.clio(&["diff", &id]);
    assert!(
        diff.contains("--- a/app.py")
            && diff.contains("-    return sum(xs) + 1")
            && diff.contains("+    return sum(xs)\n"),
        "{diff}"
    );
    assert!(diff.contains("+++ b/docs/new.md"));

    let applied = e.clio(&["apply", "--yes", &id]);
    assert!(applied.contains("2 file(s) applied"), "{applied}");
    assert_eq!(e.read("app.py"), NEW_APP);
    assert_eq!(e.read("docs/new.md"), "hello\n");
    assert_eq!(e.read("notes.txt"), "keep me\n");
    assert!(e.read(".env").starts_with("TOKEN="));
    assert_eq!(e.read(".git/config"), "[core]\n");
    assert_eq!(
        std::fs::read_to_string(run_dir.join("backup/app.py")).unwrap(),
        OLD_APP
    );
    // A second apply is refused until undone.
    assert!(!e.try_clio(&["apply", "--yes", &id]).status.success());

    e.clio(&["undo", &id]);
    assert_eq!(e.read("app.py"), OLD_APP);
    assert!(
        !e.proj().join("docs").exists(),
        "the folder apply created is gone"
    );
    assert_eq!(e.read("notes.txt"), "keep me\n");

    assert!(e.clio(&["runs"]).contains(&id));
    assert!(e.clio(&["show", &id[..12]]).contains("fixed total()"));
}

#[test]
fn apply_declines_without_confirmation() {
    let e = Env::new();
    let id = e.run_fix();
    let mut c = e.cmd(&["apply", &id]);
    c.stdin(Stdio::piped());
    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(b"n\n").unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(text(&o.stdout).contains("not applied"));
    assert_eq!(e.read("app.py"), OLD_APP);
}

#[test]
fn apply_refuses_when_the_original_changed() {
    let e = Env::new();
    let id = e.run_fix();
    std::fs::write(
        e.proj().join("app.py"),
        "# edited by the person meanwhile\n",
    )
    .unwrap();
    let o = e.try_clio(&["apply", "--yes", &id]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("changed in the original"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(e.read("app.py"), "# edited by the person meanwhile\n");
    assert!(
        !e.proj().join("docs").exists(),
        "nothing at all was applied"
    );
    // --force overwrites, with a backup of the person's version, and undo brings it back.
    e.clio(&["apply", "--yes", "--force", &id]);
    assert_eq!(e.read("app.py"), NEW_APP);
    e.clio(&["undo", &id]);
    assert_eq!(e.read("app.py"), "# edited by the person meanwhile\n");
}

#[test]
fn apply_refuses_a_symlink_that_escapes() {
    let e = Env::new();
    let id = e.run_fix();
    // After the copy, `docs` in the original becomes a link to a folder outside the workspace.
    let outside = e.tmp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    symlink(&outside, e.proj().join("docs")).unwrap();
    let o = e.try_clio(&["apply", "--yes", &id]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("symbolic link"),
        "{}",
        text(&o.stderr)
    );
    assert!(
        std::fs::read_dir(&outside).unwrap().next().is_none(),
        "nothing written outside"
    );
    assert_eq!(e.read("app.py"), OLD_APP, "all or nothing");
}

#[test]
fn apply_refuses_protected_and_secret_paths_even_with_force() {
    let e = Env::new();
    let id = e.run_fix();
    let sandbox = e
        .home()
        .join(".local/share/clio/runs")
        .join(&id)
        .join("sandbox");
    std::fs::create_dir_all(sandbox.join(".git/hooks")).unwrap();
    std::fs::write(sandbox.join(".git/hooks/pre-commit"), "#!/bin/sh\n").unwrap();
    let o = e.try_clio(&["apply", "--yes", "--force", &id]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("protected path"),
        "{}",
        text(&o.stderr)
    );
    std::fs::remove_dir_all(sandbox.join(".git")).unwrap();
    std::fs::write(sandbox.join(".env.production"), "A=1\n").unwrap();
    let o = e.try_clio(&["apply", "--yes", "--force", &id]);
    assert!(
        text(&o.stderr).contains("it looks like a secret"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(e.read("app.py"), OLD_APP);
    assert!(!e.proj().join(".git/hooks").exists());
}

#[test]
fn apply_refuses_agent_folders_any_case_and_ignores_generated_ones() {
    let e = Env::new();
    let id = e.run_fix();
    let sandbox = e
        .home()
        .join(".local/share/clio/runs")
        .join(&id)
        .join("sandbox");
    std::fs::create_dir_all(sandbox.join(".AGENTS/skills/x")).unwrap();
    std::fs::write(sandbox.join(".AGENTS/skills/x/SKILL.md"), "obey me\n").unwrap();
    let o = e.try_clio(&["apply", "--yes", "--force", &id]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("protected path"),
        "{}",
        text(&o.stderr)
    );
    std::fs::remove_dir_all(sandbox.join(".AGENTS")).unwrap();
    // What the copy leaves out never goes back: node_modules is ignored, not written.
    std::fs::create_dir_all(sandbox.join("node_modules/x")).unwrap();
    std::fs::write(sandbox.join("node_modules/x/index.js"), "evil()\n").unwrap();
    e.clio(&["apply", "--yes", &id]);
    assert_eq!(e.read("app.py"), NEW_APP);
    assert!(!e.proj().join("node_modules").exists());
}

#[test]
fn apply_refuses_what_the_diff_cannot_show_unless_forced() {
    let e = Env::new();
    let id = e.run_fix();
    let sandbox = e
        .home()
        .join(".local/share/clio/runs")
        .join(&id)
        .join("sandbox");
    std::fs::write(sandbox.join("blob.bin"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
    let o = e.try_clio(&["apply", "--yes", &id]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("cannot be shown in full"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(e.read("app.py"), OLD_APP, "nothing was applied");
    assert!(!e.proj().join("blob.bin").exists());
    e.clio(&["apply", "--yes", "--force", &id]);
    assert!(e.proj().join("blob.bin").exists());
}

#[test]
fn symlinks_in_the_sandbox_are_ignored() {
    let e = Env::new();
    let id = e.run_fix();
    let sandbox = e
        .home()
        .join(".local/share/clio/runs")
        .join(&id)
        .join("sandbox");
    symlink("/etc/passwd", sandbox.join("passwd-link")).unwrap();
    let diff = e.clio(&["diff", &id]);
    assert!(!diff.contains("passwd"));
}

#[test]
fn init_is_idempotent_and_private() {
    let e = Env::new();
    let cfg = e.home().join(".config/clio/config.json");
    let before = std::fs::read_to_string(&cfg).unwrap();
    std::fs::set_permissions(
        e.home().join(".local/share/clio"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let out = e.clio(&["init"]);
    assert!(out.contains("left as it is"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        before,
        "config untouched"
    );
    for d in [
        ".config/clio",
        ".config/clio/commands",
        ".local/share/clio",
        ".local/share/clio/runs",
    ] {
        assert_eq!(mode(&e.home().join(d)), 0o700, "{d}");
    }
    assert_eq!(mode(&e.tmp.path().join("run/clio")), 0o700);
    assert_eq!(mode(&cfg), 0o600);
    let c: Value = serde_json::from_str(&before).unwrap();
    assert_eq!(c["model"]["provider"], "opencode-go");
    assert_eq!(c["model"]["model"], "beam-free");
    assert_eq!(c["model"]["prompt"], "clio-agent@2");
}

#[test]
fn init_without_a_key_says_to_connect_and_never_names_the_key_file() {
    let e = Env::new();
    let out = e.clio(&["init"]);
    assert!(out.contains("clio connect opencode"), "{out}");
    assert!(!out.contains("opencode-api-key"), "{out}");
    // The variable is a key too: said, and never shown.
    let secret = format!("sk-{}", "q".repeat(20));
    let o = e
        .cmd(&["init"])
        .env("OPENCODE_API_KEY", &secret)
        .output()
        .unwrap();
    let out = text(&o.stdout);
    assert!(
        o.status.success() && out.contains("OPENCODE_API_KEY") && !out.contains(&secret),
        "{out}"
    );
    assert!(!out.contains("clio connect opencode"), "{out}");
}

// The model adapter is the one inside the binary: `init` writes no path into the config (so it
// keeps working when Clio is updated, moved or installed on another machine), and the path an
// older `init` wrote, of a checkout that has since moved, is not run nor a failure.
#[test]
fn the_model_adapter_is_the_one_inside_the_binary() {
    let e = Env::new();
    let cfg_path = e.home().join(".config/clio/config.json");
    std::fs::remove_file(&cfg_path).unwrap();
    e.clio(&["init"]);
    let text_cfg = std::fs::read_to_string(&cfg_path).unwrap();
    let mut cfg: Value = serde_json::from_str(&text_cfg).unwrap();
    assert_eq!(cfg["model"]["command"], json!([]), "{text_cfg}");
    assert!(!text_cfg.contains("openai_compat"), "{text_cfg}");
    let o = e.try_clio(&["doctor"]);
    let out = text(&o.stdout);
    assert!(o.status.success(), "{out}{}", text(&o.stderr));
    assert!(
        out.contains("the model adapter is the one this binary brings"),
        "{out}"
    );

    cfg["model"]["command"] = json!([
        "python3",
        "/ya/no/esta/clio/adapters/models/openai_compat.py"
    ]);
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    let o = e.try_clio(&["doctor"]);
    let out = text(&o.stdout);
    assert!(o.status.success(), "{out}{}", text(&o.stderr));
    assert!(
        out.contains("the model adapter is the one this binary brings"),
        "{out}"
    );
    assert!(out.contains("is no longer used"), "{out}");
    assert!(!out.contains("openai_compat.py does not exist"), "{out}");

    // Another adapter, written by the person, is theirs: missing, it is a failure.
    cfg["model"]["command"] = json!(["python3", "/ya/no/esta/mi_adaptador.py"]);
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    let o = e.try_clio(&["doctor"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stdout).contains("mi_adaptador.py does not exist"));
}

#[test]
fn a_config_with_an_unknown_provider_says_to_connect() {
    let e = Env::new();
    let cfg_path = e.home().join(".config/clio/config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["model"]["provider"] = json!("fakeprov");
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    let o = e.try_clio(&["doctor"]);
    let out = format!("{}{}", text(&o.stdout), text(&o.stderr));
    assert!(out.contains("clio connect"), "{out}");

    cfg["model"]["provider"] = json!("opencode-go");
    cfg["model"]["base_url"] = json!("https://evil.test");
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    let o = e.try_clio(&["doctor"]);
    let out = format!("{}{}", text(&o.stdout), text(&o.stderr));
    assert!(out.contains("this machine"), "{out}");
}

#[test]
fn doctor_passes_when_clean_and_fails_on_open_permissions() {
    let e = Env::new();
    let o = e.try_clio(&["doctor"]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let cfg_dir = e.home().join(".config/clio");
    std::fs::set_permissions(&cfg_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = e.try_clio(&["doctor"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stdout).contains("FAIL"));
    std::fs::set_permissions(&cfg_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Makes `claude` the active connector of this Env.
fn use_claude(e: &Env) {
    let cfg_path = e.home().join(".config/clio/config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["model"]["provider"] = json!("claude");
    cfg["model"]["model"] = json!("claude-sonnet-5-5");
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
}

fn connector_line(e: &Env, extra_env: &[(&str, &str)]) -> String {
    let o = e
        .cmd(&["doctor"])
        .envs(extra_env.iter().copied())
        .output()
        .unwrap();
    text(&o.stdout)
        .lines()
        .find(|l| l.contains("connector "))
        .unwrap_or_default()
        .to_owned()
}

// ADR 0009: with the claude connector Pi does not start without its jail; `CLIO_PI_JAIL=off`
// is a FAIL that says so. Another connector: the same FAIL, without that.
#[test]
fn doctor_says_the_claude_connector_will_not_start_with_the_jail_off() {
    let e = Env::new();
    let jail_line = |e: &Env| {
        let o = e
            .cmd(&["doctor"])
            .env("CLIO_PI_JAIL", "off")
            .output()
            .unwrap();
        text(&o.stdout)
            .lines()
            .find(|l| l.contains("Pi jail"))
            .unwrap_or_default()
            .to_owned()
    };
    let other = jail_line(&e);
    assert!(
        other.starts_with("FAIL") && other.contains("CLIO_PI_JAIL=off"),
        "{other}"
    );
    assert!(!other.contains("connector"), "{other}");
    use_claude(&e);
    let claude = jail_line(&e);
    assert!(
        claude.starts_with("FAIL")
            && claude.contains("the connector claude will NOT start without it"),
        "{claude}"
    );
}

#[test]
fn doctor_says_a_connector_key_is_in_the_vault_and_where_it_may_go() {
    let e = Env::new();
    use_claude(&e);
    let _v = e.vault();
    let line = connector_line(&e, &[]);
    assert!(
        line.starts_with("WARN") && line.contains("clio connect claude"),
        "{line}"
    );
    let tok = "sk-ant-oat01-decoy-token";
    let o = e.with_stdin(
        &[
            "vault",
            "put",
            "claude",
            "--dest",
            "model:api.anthropic.com",
        ],
        &format!("{tok}\n"),
        &[],
    );
    assert!(o.status.success(), "{}", all_text(&o));
    let line = connector_line(&e, &[]);
    assert!(
        line.starts_with("PASS")
            && line.contains(
                "connector claude: the key is in the vault and can only go to api.anthropic.com"
            ),
        "{line}"
    );
    assert!(!line.contains(tok));
    // The variable wins over the vault, and the doctor says so.
    let line = connector_line(&e, &[("CLAUDE_CODE_OAUTH_TOKEN", "from-the-variable")]);
    assert!(
        line.starts_with("PASS")
            && line.contains("connector claude: the key comes from CLAUDE_CODE_OAUTH_TOKEN")
            && !line.contains("from-the-variable"),
        "{line}"
    );
}

#[test]
fn doctor_says_whether_the_claude_bridge_is_installed() {
    let e = Env::new();
    use_claude(&e);
    let bridge = |e: &Env| {
        text(&e.try_clio(&["doctor"]).stdout)
            .lines()
            .find(|l| l.contains("Claude Code"))
            .unwrap_or_default()
            .to_owned()
    };
    let line = bridge(&e);
    assert!(
        line.starts_with("WARN") && line.contains("clio pi install"),
        "{line}"
    );
    e.fake_bridge();
    let line = bridge(&e);
    assert!(line.starts_with("PASS"), "{line}");
}

#[test]
fn doctor_warns_about_a_key_file_left_behind() {
    let e = Env::new();
    let key = e.home().join(".config/clio/opencode-api-key");
    std::fs::write(&key, "not-a-real-key").unwrap();
    let o = e.try_clio(&["doctor"]);
    let out = text(&o.stdout);
    let line = out
        .lines()
        .find(|l| l.contains("a key is left"))
        .unwrap_or_default();
    assert!(
        line.starts_with("WARN")
            && line.contains(&format!(
                "a key is left in {}: clio connect opencode moves it into the vault",
                key.display()
            )),
        "{out}"
    );
    assert!(!out.contains("not-a-real-key"));
}

// The built-in adapter needs the model key before anything is copied: without a vault and without
// the variable the run says how to fix it and leaves nothing behind.
#[test]
fn run_with_the_builtin_adapter_without_a_key_fails_before_copying() {
    let e = Env::new();
    let cfg_path = e.home().join(".config/clio/config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["model"]["command"] = json!([]);
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    let o = e.try_clio(&[
        "run",
        "--harness",
        "builtin",
        "--workspace",
        &e.path("proj"),
        "hazlo",
    ]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("clio vault start"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(
        std::fs::read_dir(e.home().join(".local/share/clio/runs"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn run_refuses_broad_or_huge_workspaces() {
    let e = Env::new();
    e.script_fix();
    for ws in ["/", &e.home().display().to_string()] {
        let o = e.try_clio(&["run", "--workspace", ws, "hazlo"]);
        assert!(!o.status.success(), "{ws}");
        assert!(text(&o.stderr).contains("refusing"), "{}", text(&o.stderr));
    }
    // Lower the limit to 2 files: the project (3 files) is too big until --allow-broad.
    let cfg_path = e.home().join(".config/clio/config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["workspace"]["max_files"] = json!(1);
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    let o = e.try_clio(&["run", "--workspace", &e.path("proj"), "hazlo"]);
    assert!(
        text(&o.stderr).contains("--allow-broad"),
        "{}",
        text(&o.stderr)
    );
    assert!(
        e.try_clio(&[
            "run",
            "--allow-broad",
            "--workspace",
            &e.path("proj"),
            "hazlo"
        ])
        .status
        .success()
    );
}

#[test]
fn no_arguments_without_a_terminal_prints_help_and_exits_0() {
    let e = Env::new();
    for args in [&[][..], &["--help"], &["help"]] {
        let o = e.try_clio(args);
        assert_eq!(o.status.code(), Some(0), "{args:?}");
        assert!(text(&o.stdout).contains("clio run"), "{args:?}");
    }
    let o = e.try_clio(&["frobnicate"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o.stderr).contains("unknown command"));
}

#[test]
fn run_prints_a_line_per_step_and_a_plain_panel_without_escape_codes() {
    let e = Env::new();
    e.script_fix();
    let o = e.clio(&["run", "--workspace", &e.path("proj"), "fix total in app.py"]);
    assert!(!o.contains('\u{1b}'), "a pipe gets no color codes: {o:?}");
    assert!(o.contains("[1/40] read_file app.py"), "{o}");
    assert!(o.contains("allowed"), "{o}");
    assert!(o.contains("[4/40] the model says it is done"), "{o}");
    assert!(
        o.contains("agent:") && o.contains("The original was NOT touched"),
        "{o}"
    );
    assert!(
        o.contains("  M app.py") && o.contains("  A docs/new.md"),
        "{o}"
    );
}

// Second audit, batch D: `clio run` keeps escape sequences and hidden characters out of what the
// model is shown of a file and of what is saved, and leaves tabs alone.
#[test]
fn run_strips_escape_sequences_and_hidden_text_from_what_it_saves() {
    let e = Env::new();
    std::fs::write(
        e.proj().join("notes.txt"),
        "all:\n\tmake\n\u{1b}[31mred\u{1b}[0m\u{e0041}\u{202e}x\n",
    )
    .unwrap();
    e.script(&[
        json!({"tool": "read_file", "args": {"path": "notes.txt"}}),
        json!({"final": "done\u{1b}[2J\u{e0042}"}),
    ]);
    e.clio(&["run", "--workspace", &e.path("proj"), "lee notes.txt"]);
    // What the model was shown of the file: the text, the tab, none of the rest.
    let seen = std::fs::read_to_string(e.tmp.path().join("script/seen")).unwrap();
    assert!(seen.contains("make"), "{seen}");
    assert!(
        !seen.contains(['\u{e0041}', '\u{202e}']) && !seen.contains("\\u001b"),
        "the model was shown hidden text: {seen}"
    );
    let runs = e.home().join(".local/share/clio/runs");
    let run = std::fs::read_dir(runs)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let transcript: Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("transcript.json")).unwrap())
            .unwrap();
    let all: String = transcript
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        all.contains("all:\n\tmake\nred"),
        "text and tab kept: {all:?}"
    );
    assert!(
        !all.contains(['\u{1b}', '\u{e0041}', '\u{e0042}', '\u{202e}']),
        "{all:?}"
    );
}

// Second audit, batch D: the default harness is Pi; when Pi cannot run, falling back to the builtin
// loop (not jailed the same way) is said, and asking for the builtin loop is not a fallback.
#[test]
fn the_default_harness_says_when_it_falls_back_to_the_builtin_loop() {
    let e = Env::new();
    e.script_fix();
    let proj = e.path("proj");
    let o = e.try_clio(&["run", "--workspace", &proj, "fix total in app.py"]);
    assert!(o.status.success());
    assert!(
        text(&o.stderr).contains("Pi is not available"),
        "{}",
        text(&o.stderr)
    );
    e.script_fix();
    let o = e.try_clio(&[
        "run",
        "--harness",
        "builtin",
        "--workspace",
        &proj,
        "fix total in app.py",
    ]);
    assert!(o.status.success());
    assert!(!text(&o.stderr).contains("Pi is not available"));
}

#[test]
fn runs_show_and_doctor_speak_json() {
    let e = Env::new();
    let id = e.run_fix();
    let runs: Value = serde_json::from_str(&e.clio(&["runs", "--json"])).unwrap();
    assert_eq!(runs[0]["id"], id.as_str());
    assert_eq!(runs[0]["status"], "pending_review");
    assert_eq!(runs[0]["files"], 2);
    let show: Value = serde_json::from_str(&e.clio(&["show", "--json", &id[..12]])).unwrap();
    assert_eq!(show["summary"]["modified"], json!(["app.py"]));
    assert_eq!(show["summary"]["ending"], "Final");
    assert!(show["folder"].as_str().unwrap().ends_with(&id));
    let doc: Value = serde_json::from_str(&e.clio(&["doctor", "--json"])).unwrap();
    assert_eq!(doc["ok"], true);
    assert!(doc["checks"].as_array().unwrap().len() > 3);
    // The table is aligned and has a header.
    let table = e.clio(&["runs"]);
    assert!(
        table.starts_with("RUN") && table.contains("pending review"),
        "{table}"
    );
    e.clio(&["apply", "--yes", &id]);
    let runs: Value = serde_json::from_str(&e.clio(&["runs", "--json"])).unwrap();
    assert_eq!(runs[0]["status"], "applied");
}

#[test]
fn pty_the_tui_opens_in_the_alternate_screen_and_q_restores_the_terminal() {
    let e = Env::new();
    for scenario in ["quit", "ctrl_c"] {
        let v = e.pty(scenario, "0");
        assert_eq!(v["alt_on"], true, "{scenario}: {v}");
        assert_eq!(v["title_drawn"], true, "{scenario}: {v}");
        assert_eq!(v["code"], 0, "{scenario}: {v}");
        assert_eq!(v["alt_off_after_on"], true, "{scenario}: {v}");
        assert_eq!(v["during"]["canonical"], false, "raw mode while open: {v}");
        assert_eq!(v["after"]["canonical"], true, "cooked mode back: {v}");
        assert_eq!(v["after"]["echo"], true, "echo back: {v}");
    }
}

// Second audit, batch D: a signal that ends the process must not leave the terminal in the
// alternate screen in raw mode.
#[test]
fn pty_sigterm_and_sighup_give_the_terminal_back() {
    let e = Env::new();
    for (scenario, code) in [("sigterm", 143), ("sighup", 129)] {
        let v = e.pty(scenario, "0");
        assert_eq!(v["alt_on"], true, "{scenario}: {v}");
        assert_eq!(v["code"], code, "{scenario}: {v}");
        assert_eq!(v["alt_off_after_on"], true, "{scenario}: {v}");
        assert_eq!(v["after"]["canonical"], true, "{scenario}: {v}");
        assert_eq!(v["after"]["echo"], true, "{scenario}: {v}");
    }
}

// Second audit, batch D: text typed or pasted before the question is not the answer to it.
#[test]
fn pty_what_was_typed_before_apply_asks_does_not_answer() {
    let e = Env::new();
    e.run_fix();
    let v = e.pty("typeahead", "0");
    assert_eq!(v["asked"], true, "{v}");
    assert_eq!(v["untouched"], true, "typeahead applied the run: {v}");
}

// On macOS CI `q` did not leave after the key sequence (timeout); it needs a Mac to debug, so Linux only.
#[cfg(target_os = "linux")]
#[test]
fn pty_debug_context_usage_help_and_redraw_keys_work_and_the_terminal_is_restored() {
    let e = Env::new();
    e.run_fix();
    let v = e.pty("views", "0");
    assert_eq!(v["opened"], true, "{v}");
    assert_eq!(v["debug_before"], false, "debug starts off: {v}");
    assert_eq!(v["debug_on"], true, "Ctrl-D turns it on: {v}");
    assert_eq!(v["context"], true, "c opens the context layers: {v}");
    assert_eq!(v["usage"], true, "U opens the usage table: {v}");
    assert_eq!(v["help"], true, "? opens the help: {v}");
    assert_eq!(v["code"], 0, "q leaves: {v}");
    assert_eq!(v["alt_off_after_on"], true, "{v}");
    assert_eq!(v["after"]["canonical"], true, "{v}");
    assert_eq!(v["after"]["echo"], true, "{v}");
    // Only the TUI toggled debug on a screen; it wrote no log (no run was launched with it).
    let runs = e.home().join(".local/share/clio/runs");
    for r in std::fs::read_dir(runs).unwrap() {
        assert!(!r.unwrap().path().join("debug.log").exists());
    }
}

#[test]
fn pty_clio_debug_env_opens_the_tui_with_debug_on() {
    let e = Env::new();
    let v = e.pty("debug_env", "0");
    assert_eq!(v["opened"], true, "{v}");
    assert_eq!(v["debug_from_env"], true, "{v}");
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["after"]["canonical"], true, "{v}");
}

#[test]
fn pty_a_panic_still_gives_the_terminal_back() {
    let e = Env::new();
    let v = e.pty("panic", "0");
    assert_eq!(v["alt_on"], true, "{v}");
    assert_eq!(v["code"], 101, "{v}");
    assert_eq!(v["alt_off_after_on"], true, "{v}");
    assert_eq!(
        v["message_after_restore"], true,
        "the panic message prints on the normal screen: {v}"
    );
    assert_eq!(v["after"]["canonical"], true, "{v}");
    assert_eq!(v["after"]["echo"], true, "{v}");
}

#[test]
fn pty_with_stdout_piped_it_prints_help_and_exits_0() {
    let e = Env::new();
    let v = e.pty("help_when_piped", "0");
    assert_eq!(v["code"], 0, "{v}");
    assert!(v["stdout"].as_str().unwrap().contains("clio run"), "{v}");
    assert_eq!(v["escapes"], false, "{v}");
}

// The interactive job flow stalls on macOS CI (a scenario sat past 110 s); it needs a Mac to debug,
// so it is verified on Linux only. The open/quit/panic/piped-help scenarios do run on macOS.
#[cfg(target_os = "linux")]
#[test]
fn pty_a_job_runs_from_the_tui_and_apply_and_undo_ask_first() {
    let e = Env::new();
    e.script_fix();
    let v = e.pty("live", "0");
    assert_eq!(v["summary"], true, "{v}");
    assert_eq!(
        v["asked_before_apply"], true,
        "the dialog asks, nothing written yet: {v}"
    );
    assert_eq!(v["applied"], true, "{v}");
    assert_eq!(v["asked_before_undo"], true, "{v}");
    assert_eq!(v["undone"], true, "{v}");
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["after"]["canonical"], true, "{v}");
    assert_eq!(e.read("app.py"), OLD_APP);
    assert!(
        !e.proj().join("docs").exists(),
        "undo removed the folder apply created"
    );
}

// The interactive job flow stalls on macOS CI (a scenario sat past 110 s); it needs a Mac to debug,
// so it is verified on Linux only. The open/quit/panic/piped-help scenarios do run on macOS.
#[cfg(target_os = "linux")]
#[test]
fn pty_cancelling_a_live_run_leaves_it_cancelled_and_the_original_untouched() {
    let e = Env::new();
    e.script_fix();
    let v = e.pty("cancel", "2");
    assert_eq!(v["started"], true, "{v}");
    assert_eq!(v["ending"], "Cancelled", "{v}");
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(e.read("app.py"), OLD_APP);
    let runs: Value = serde_json::from_str(&e.clio(&["runs", "--json"])).unwrap();
    assert_eq!(runs[0]["status"], "cancelled");
    assert!(e.clio(&["runs"]).contains("cancelled"));
}

/// Prints the plain outputs (for the docs): `cargo test -p clio-brain --test cli print_plain -- --ignored --nocapture`.
#[test]
#[ignore = "prints outputs; not a check"]
fn print_plain_outputs() {
    let e = Env::new();
    e.script(&[
        json!({"tool": "read_file", "args": {"path": "app.py"}}),
        json!({"tool": "write_file", "args": {"path": "app.py", "content": NEW_APP}}),
        json!({"tool": "write_file", "args": {"path": ".git/x", "content": "no"}}),
        json!({"final": "Fixed total() in app.py: it no longer adds 1 too many. I could not run the tests; run them before applying."}),
    ]);
    println!("--- init\n{}", e.clio(&["init"]));
    let o = e.try_clio(&["run", "--workspace", &e.path("proj"), "fix total in app.py"]);
    println!("--- run\n{}", text(&o.stdout));
    println!("--- runs\n{}", e.clio(&["runs"]));
    println!("--- doctor\n{}", text(&e.try_clio(&["doctor"]).stdout));
}

#[test]
fn hostile_text_is_cleaned_before_it_reaches_a_terminal_view() {
    let e = Env::new();
    e.script(&[json!({"final": "listo\u{1b}]0;pwned\u{7} \u{1b}[2J"})]);
    let id = {
        let out = e.clio(&["run", "--workspace", &e.path("proj"), "haz algo\u{1b}[31m"]);
        out.lines()
            .find_map(|l| l.strip_prefix("run:").map(|s| s.trim().to_owned()))
            .unwrap()
    };
    // The text is stored as it came (the record is the truth) but printing removes the escapes.
    for out in [e.clio(&["runs"]), e.clio(&["show", &id])] {
        assert!(!out.contains('\u{1b}') && !out.contains('\u{7}'), "{out:?}");
    }
}

// ---------------------------------------------------------------- debug, usage and doctor

fn b64(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in s.as_bytes().chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn run_dirs(e: &Env) -> std::collections::BTreeSet<PathBuf> {
    std::fs::read_dir(e.home().join(".local/share/clio/runs"))
        .map(|rd| rd.map(|d| d.unwrap().path()).collect())
        .unwrap_or_default()
}

/// Runs `clio` and returns what it printed and the folder of the run it made (two runs in one
/// second share a prefix, so "the newest by name" would be a guess).
fn run_and_find(e: &Env, args: &[&str], env: &[(&str, &str)]) -> (Output, PathBuf) {
    let before = run_dirs(e);
    let o = run_with(e, args, env);
    let made: Vec<PathBuf> = run_dirs(e).difference(&before).cloned().collect();
    assert_eq!(made.len(), 1, "{made:?}");
    (o, made[0].clone())
}

/// A decoy secret in all its forms, planted in a file the model reads and in what it answers.
struct Planted {
    canaries: clio_canaries::Canaries,
    decoy: String,
}

fn plant(e: &Env) -> Planted {
    let canaries = clio_canaries::Canaries::from_seed(21);
    let decoy = canaries.value("llm-provider").unwrap().to_owned();
    let url: String = decoy.bytes().map(|b| format!("%{b:02X}")).collect();
    let forms = format!("{decoy} {} {url}", b64(&decoy));
    std::fs::write(
        e.proj().join("notes.txt"),
        format!("keep me\nkey: {forms}\n"),
    )
    .unwrap();
    e.script(&[
        json!({"tool": "read_file", "args": {"path": "notes.txt"}, "note": forms}),
        json!({"final": format!("done {forms}")}),
    ]);
    Planted { canaries, decoy }
}

fn run_with(e: &Env, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = e.cmd(args);
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{args:?}: {}{}",
        text(&o.stdout),
        text(&o.stderr)
    );
    o
}

#[test]
fn debug_writes_a_private_log_and_stderr_lines_without_the_secret() {
    let e = Env::new();
    let p = plant(&e);
    let ws = e.path("proj");
    let (o, run) = run_and_find(
        &e,
        &["run", "--debug", "--workspace", &ws, "mira notes.txt"],
        &[("OPENCODE_API_KEY", &p.decoy)],
    );
    let (out, err) = (text(&o.stdout), text(&o.stderr));
    p.canaries.assert_clean("stdout", &out);
    p.canaries.assert_clean("stderr", &err);
    assert!(!err.contains('\u{1b}'), "stderr is sanitized: {err:?}");
    for want in [
        "[debug] debug on",
        "[debug] context #1",
        "[debug] call #1",
        "[debug] request #1",
        "[debug] gateway #1",
        "read_file",
    ] {
        assert!(err.contains(want), "{want}\n{err}");
    }
    let log = run.join("debug.log");
    assert_eq!(mode(&log), 0o600, "debug.log is private");
    let log_text = std::fs::read_to_string(&log).unwrap();
    let kinds: Vec<String> = log_text
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    for k in [
        "started", "context", "usage", "exchange", "step", "decision", "tool", "final",
    ] {
        assert!(kinds.iter().any(|x| x == k), "{k} in {kinds:?}");
    }
    // The decoy is in none of what Clio wrote or showed: plain, base64 or URL-encoded.
    for f in [
        "debug.log",
        "usage.json",
        "summary.json",
        "transcript.json",
        "report.json",
        "effects.jsonl",
    ] {
        let t = std::fs::read_to_string(run.join(f)).unwrap_or_default();
        p.canaries.assert_clean(f, &t);
    }
    // The decoy WAS in the project file the model read: the scrubbing is what hid it.
    assert!(
        std::fs::read_to_string(run.join("sandbox/notes.txt"))
            .unwrap()
            .contains(&p.decoy)
    );
    let shown = e.clio(&["show", &run.file_name().unwrap().to_string_lossy()]);
    p.canaries.assert_clean("clio show", &shown);
}

#[test]
fn without_debug_nothing_extra_is_written_or_printed() {
    let e = Env::new();
    let p = plant(&e);
    let ws = e.path("proj");
    let (o, run) = run_and_find(
        &e,
        &["run", "--workspace", &ws, "mira notes.txt"],
        &[("OPENCODE_API_KEY", &p.decoy)],
    );
    let err = text(&o.stderr);
    assert!(!err.contains("[debug]"), "{err}");
    p.canaries.assert_clean("stdout", &text(&o.stdout));
    assert!(
        !run.join("debug.log").exists(),
        "no debug.log without --debug"
    );
    // What a run always keeps: its numbers, private, and still clean.
    assert_eq!(mode(&run.join("usage.json")), 0o600);
    p.canaries.assert_clean(
        "transcript",
        &std::fs::read_to_string(run.join("transcript.json")).unwrap(),
    );
    // CLIO_DEBUG=1 is the same as --debug; 0 and empty are off.
    for (v, on) in [("1", true), ("true", true), ("0", false), ("", false)] {
        e.script(&[json!({"final": "ok"})]);
        let (_, r) = run_and_find(
            &e,
            &["run", "--workspace", &ws, "algo"],
            &[("CLIO_DEBUG", v)],
        );
        assert_eq!(r.join("debug.log").exists(), on, "CLIO_DEBUG={v:?}");
    }
}

#[test]
fn debug_flag_placement_is_checked() {
    let e = Env::new();
    let o = e.try_clio(&["--debug", "doctor"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("--debug"), "{}", text(&o.stderr));
    // Alone, without a terminal, it prints the help like `clio` does.
    let o = e.try_clio(&["--debug"]);
    assert!(o.status.success() && text(&o.stdout).contains("clio run"));
    assert!(text(&o.stdout).contains("--debug"));
    // `--new` (a new session instead of the last conversation) is one of the interface's flags.
    let o = e.try_clio(&["--new"]);
    assert!(o.status.success() && text(&o.stdout).contains("--new"));
    let o = e.try_clio(&["--new", "doctor"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("--new"), "{}", text(&o.stderr));
}

#[test]
fn every_run_keeps_its_usage_and_context_in_usage_json() {
    let e = Env::new();
    let before = run_dirs(&e);
    e.run_fix();
    let run = run_dirs(&e).difference(&before).next().unwrap().clone();
    assert_eq!(mode(&run.join("usage.json")), 0o600);
    let u: Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("usage.json")).unwrap()).unwrap();
    assert_eq!(u["window"], 128_000);
    assert_eq!(u["window_known"], true);
    assert_eq!(u["limits"]["max_steps"], 40);
    let calls = u["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 4);
    // The scripted adapter reports 1000, 2000, ... input tokens and 50 out per call.
    let tin: u64 = calls
        .iter()
        .map(|c| c["input_tokens"].as_u64().unwrap())
        .sum();
    assert_eq!(tin, 1_000 + 2_000 + 3_000 + 4_000);
    assert!(
        calls
            .iter()
            .all(|c| c["output_tokens"] == 50 && c["cost_micro"] == 0)
    );
    let ctx = &u["context"];
    assert_eq!(ctx["external"][0]["name"], "app.py");
    assert!(ctx["system"].as_u64().unwrap() > 0 && ctx["tools"].as_u64().unwrap() > 0);
    // The summary's totals are the same numbers.
    let s: Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("summary.json")).unwrap()).unwrap();
    assert_eq!(s["input_tokens"], 10_000);
    assert_eq!(s["output_tokens"], 200);
}

// The real host script is Linux-only (ss, findmnt, systemctl): this test does not apply on macOS.
#[cfg(target_os = "linux")]
#[test]
fn doctor_shows_each_check_once_with_the_real_host_script() {
    let e = Env::new();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../infra/local/check-host.sh");
    let o = e
        .cmd(&["doctor", "--json"])
        .env("CLIO_HOST_CHECK", &script)
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(&text(&o.stdout)).unwrap();
    // The JSON keeps its shape.
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "checks",
            "fail",
            "host_script",
            "jail",
            "ok",
            "pass",
            "warn"
        ]
    );
    let checks = v["checks"].as_array().unwrap();
    for c in checks {
        let mut k: Vec<&str> = c.as_object().unwrap().keys().map(String::as_str).collect();
        k.sort_unstable();
        assert_eq!(k, ["hint", "level", "message"]);
    }
    let msgs: Vec<&str> = checks
        .iter()
        .map(|c| c["message"].as_str().unwrap())
        .collect();
    let count = |needle: &str| msgs.iter().filter(|m| m.contains(needle)).count();
    // Permissions: one line per Clio folder, not two.
    assert_eq!(count("is private (700)"), 3, "{msgs:#?}");
    assert_eq!(count("(S-23)"), 0, "{msgs:#?}");
    // Ports: one line, Clio's own.
    assert_eq!(count("listens outside loopback"), 1, "{msgs:#?}");
    assert_eq!(
        count("no Clio process listens on a public interface"),
        0,
        "{msgs:#?}"
    );
    // The host script's own lines (infra/local/check-host.sh) come through.
    assert!(
        msgs.iter()
            .any(|m| m.contains("firewall") || m.contains("OpenSSH") || m.contains("bwrap")),
        "{msgs:#?}"
    );
    // The script on its own still does every check and keeps its exit code.
    let alone = Command::new("bash")
        .arg(&script)
        .env("HOME", e.home())
        .env("XDG_RUNTIME_DIR", e.path("run"))
        .output()
        .unwrap();
    let alone_out = text(&alone.stdout);
    assert!(
        alone_out.contains("is private (700)") && alone_out.contains("S-38"),
        "{alone_out}"
    );
    assert!(
        alone_out.contains("host: no FAIL") || alone_out.contains("host: there is a FAIL"),
        "{alone_out}"
    );
    assert_eq!(
        alone.status.code() == Some(1),
        alone_out.contains("host: there is a FAIL")
    );
}

/// ONNX Runtime turns its telemetry on by itself; clio-memory refuses to start it (Granite) unless
/// `ORT_DISABLE_TELEMETRY=1`. `clio` started without it runs again with it set: a program it
/// starts (here the `node` that `clio pi doctor` asks for its version) sees the variable.
#[test]
fn clio_runs_with_the_runtime_telemetry_off() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let seen = tmp.path().join("seen");
    let node = bin.join("node");
    std::fs::write(
        &node,
        format!(
            "#!/bin/sh\nprintf '%s' \"${{ORT_DISABLE_TELEMETRY-unset}}\" > '{}'\necho v22.20.0\n",
            seen.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = Command::new(CLIO)
        .args(["pi", "doctor"])
        .env("HOME", tmp.path())
        .env("XDG_RUNTIME_DIR", tmp.path())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env_remove("ORT_DISABLE_TELEMETRY")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let said = std::fs::read_to_string(&seen)
        .unwrap_or_else(|_| panic!("node was not run: {}{}", text(&o.stdout), text(&o.stderr)));
    assert_eq!(said, "1");
}

// ---------------------------------------------------------------- clio connect

/// The `clio-vault` binary (another package): next to this test's executable, built if missing.
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

const PASS: &str = "correct horse battery staple";

impl Env {
    /// Runs `clio` with `stdin` and extra environment variables.
    fn with_stdin(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
        use std::io::Write;
        let mut c = self.cmd(args);
        c.env("CLIO_VAULT_BIN", vault_bin())
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        // `clio` may exit before reading (EPIPE): that is an answer, not a test failure.
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        child.wait_with_output().unwrap()
    }

    /// A real vault (created and unlocked by `clio vault start`), stopped when the guard drops.
    fn vault(&self) -> VaultGuard<'_> {
        let o = self.with_stdin(&["vault", "start"], &format!("{PASS}\n"), &[]);
        assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
        VaultGuard(self)
    }

    fn config_bytes(&self) -> Vec<u8> {
        std::fs::read(self.home().join(".config/clio/config.json")).unwrap()
    }

    fn vault_list(&self) -> String {
        let o = self.with_stdin(&["vault", "list"], "", &[]);
        assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
}

struct VaultGuard<'a>(&'a Env);

impl Drop for VaultGuard<'_> {
    fn drop(&mut self) {
        self.0.with_stdin(&["vault", "stop"], "", &[]);
    }
}

fn all_text(o: &Output) -> String {
    format!("{}{}", text(&o.stdout), text(&o.stderr))
}

impl Env {
    /// A Pi install in this Env's data folder with the Claude bridge and a fake Claude Code that
    /// writes its environment and stdin to `script/claude-seen.json` and answers `ok`. Nothing
    /// in it runs Pi.
    fn fake_bridge(&self) {
        let pkg: Value = serde_json::from_str(
            &std::fs::read_to_string(pi_support::repo().join("adapters/pi/package.json")).unwrap(),
        )
        .unwrap();
        let version = pkg["dependencies"]["@earendil-works/pi-coding-agent"]
            .as_str()
            .unwrap();
        let nm = self
            .home()
            .join(".local/share/clio/pi")
            .join(version)
            .join("node_modules");
        let file = |rel: &str, body: &str, exec: bool| {
            let p = nm.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            if exec {
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        };
        file(".bin/pi", "#!/bin/sh\nexit 1\n", true);
        file(
            "@earendil-works/pi-coding-agent/package.json",
            &json!({ "version": version }).to_string(),
            false,
        );
        file("pi-claude-agent-sdk/src/index.ts", "", false);
        file(
            "@anthropic-ai/claude-agent-sdk-linux-x64/claude",
            &format!(
                "#!/usr/bin/env python3\nimport json, os, sys\njson.dump({{'env': dict(os.environ), 'stdin': sys.stdin.read(), 'argv': sys.argv[1:]}}, open({:?}, 'w'))\nprint(json.dumps({{'type': 'result', 'is_error': False, 'result': 'ok', 'usage': {{'input_tokens': 5, 'output_tokens': 1}}}}))\n",
                self.path("script/claude-seen.json")
            ),
            true,
        );
    }

    /// What the fake Claude Code of `fake_bridge` was given, if it ran.
    fn claude_seen(&self) -> Option<Value> {
        std::fs::read_to_string(self.tmp.path().join("script/claude-seen.json"))
            .ok()
            .map(|t| serde_json::from_str(&t).unwrap())
    }
}

#[test]
fn connect_without_the_vault_changes_nothing() {
    let e = Env::new();
    let before = e.config_bytes();
    let o = e.with_stdin(&["connect", "claude"], "sk-ant-oat01-decoy\n", &[]);
    assert!(!o.status.success());
    assert!(
        all_text(&o).contains("the vault is not running"),
        "{}",
        all_text(&o)
    );
    assert_eq!(e.config_bytes(), before);
}

#[test]
fn connect_claude_refuses_a_token_that_is_not_oauth() {
    let e = Env::new();
    e.fake_bridge();
    let _v = e.vault();
    let before = e.config_bytes();
    let o = e.with_stdin(&["connect", "claude"], "sk-ant-api03-x-not-oauth\n", &[]);
    assert!(!o.status.success());
    assert!(!all_text(&o).contains("sk-ant-api03"), "{}", all_text(&o));
    assert!(all_text(&o).contains("sk-ant-oat"), "{}", all_text(&o));
    // It must start with the prefix, not merely contain it.
    let o = e.with_stdin(
        &["connect", "claude"],
        "x-sk-ant-oat01-decoy
",
        &[],
    );
    assert!(!o.status.success(), "{}", all_text(&o));
    assert!(!e.vault_list().contains("claude"));
    assert_eq!(e.config_bytes(), before);
}

#[test]
fn connect_claude_trims_stores_and_switches() {
    let e = Env::new();
    e.fake_bridge();
    let _v = e.vault();
    let before: Value = serde_json::from_slice(&e.config_bytes()).unwrap();
    let o = e.with_stdin(
        &["connect", "claude"],
        "  sk-ant-oat01-decoy-token  \n",
        &[],
    );
    let out = all_text(&o);
    assert!(o.status.success(), "{out}");
    assert!(!out.contains("sk-ant-oat01-decoy"), "{out}");
    assert!(
        out.contains("Clio uses your subscription through Claude Code (official). The token comes from: claude setup-token"),
        "{out}"
    );
    assert!(
        !out.contains("presentándose") && !out.contains("suspender"),
        "{out}"
    );
    assert!(
        out.contains("connected: claude / claude-sonnet-5-5 answered"),
        "{out}"
    );
    let list = e.vault_list();
    assert!(
        list.contains("claude") && list.contains("api.anthropic.com"),
        "{list}"
    );
    assert!(list.contains("model"), "{list}");
    let after: Value = serde_json::from_slice(&e.config_bytes()).unwrap();
    assert_eq!(after["model"]["provider"], "claude");
    assert_eq!(after["model"]["model"], "claude-sonnet-5-5");
    let mut want = before.clone();
    want["model"]["provider"] = json!("claude");
    want["model"]["model"] = json!("claude-sonnet-5-5");
    // Remembered as this connector's model, for `/connector` in the chat.
    want["model"]["last"] = json!({"claude": "claude-sonnet-5-5"});
    assert_eq!(after, want);
    assert_eq!(mode(&e.home().join(".config/clio/config.json")), 0o600);
    // model.command (the scripted adapter of this Env) is the person's own program: warned about,
    // and the probe did not use it (it would have left its `seen` file).
    assert!(out.contains("model.command is a program of yours"), "{out}");
    assert!(!e.tmp.path().join("script/seen").exists());
    // The probe went through Claude Code, with the trimmed token only in its environment.
    let seen = e.claude_seen().expect("Claude Code was not run");
    assert_eq!(
        seen["env"]["CLAUDE_CODE_OAUTH_TOKEN"],
        "sk-ant-oat01-decoy-token"
    );
    assert!(seen["stdin"].as_str().unwrap().contains("Reply only: ok"));
    assert!(!seen["argv"].to_string().contains("sk-ant-oat01"));
}

#[test]
fn connect_claude_without_the_bridge_saves_nothing() {
    let e = Env::new();
    let _v = e.vault();
    let before = e.config_bytes();
    let o = e.with_stdin(&["connect", "claude"], "sk-ant-oat01-decoy-token\n", &[]);
    let out = all_text(&o);
    assert!(!o.status.success(), "{out}");
    assert!(
        out.contains("the Claude bridge (pi-claude-agent-sdk) is missing: clio pi install"),
        "{out}"
    );
    assert!(!out.contains("sk-ant-oat01-decoy"), "{out}");
    assert!(!e.vault_list().contains("claude"));
    assert_eq!(e.config_bytes(), before);
}

#[test]
fn connect_anthropic_api_stores_and_switches() {
    let e = Env::new();
    let _v = e.vault();
    let key = "sk-ant-api03-decoy-key-0123456789";
    let fake = pi_support::FakeProvider::start(e.tmp.path(), r#"[{"text": "ok"}]"#, Some(key));
    let env = [("LLM_BASE_URL", fake.base_url.as_str())];
    let before = e.config_bytes();
    // A subscription token is not an API key: refused, nothing stored.
    let o = e.with_stdin(&["connect", "anthropic-api"], "sk-ant-oat01-decoy\n", &env);
    assert!(!o.status.success(), "{}", all_text(&o));
    assert!(all_text(&o).contains("sk-ant-api"), "{}", all_text(&o));
    assert!(
        !all_text(&o).contains("sk-ant-oat01-decoy"),
        "{}",
        all_text(&o)
    );
    assert_eq!(e.config_bytes(), before);

    let o = e.with_stdin(&["connect", "anthropic-api"], &format!(" {key} \n"), &env);
    let out = all_text(&o);
    assert!(o.status.success(), "{out}");
    assert!(!out.contains(key), "{out}");
    assert!(
        out.contains("connected: anthropic-api / claude-sonnet-5-5 answered"),
        "{out}"
    );
    let list = e.vault_list();
    assert!(
        list.contains("anthropic-api") && list.contains("api.anthropic.com"),
        "{list}"
    );
    let cfg: Value = serde_json::from_slice(&e.config_bytes()).unwrap();
    assert_eq!(cfg["model"]["provider"], "anthropic");
    assert_eq!(cfg["model"]["model"], "claude-sonnet-5-5");
    let seen = fake.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["headers"]["x_api_key_matches"], json!(true));
    assert_eq!(seen[0]["headers"]["authorization_present"], json!(false));
    assert_eq!(seen[0]["last_user"], "Reply only: ok");
}

#[test]
fn connect_opencode_takes_the_old_file_and_deletes_it_only_on_yes() {
    let e = Env::new();
    let _v = e.vault();
    let file = e.home().join(".config/clio/opencode-api-key");
    let fake = pi_support::FakeProvider::start(e.tmp.path(), r#"[{"text": "ok"}]"#, None);
    let env = [("LLM_BASE_URL", fake.base_url.as_str())];
    std::fs::write(&file, "oc-decoy-key-0123456789\n").unwrap();
    let o = e.with_stdin(&["connect", "opencode"], "s\nN\n", &env);
    assert!(o.status.success(), "{}", all_text(&o));
    assert!(!all_text(&o).contains("oc-decoy-key"), "{}", all_text(&o));
    assert!(e.vault_list().contains("opencode.ai"));
    assert!(file.exists(), "kept on N");
    let o = e.with_stdin(&["connect", "opencode", "--new-key"], "s\ns\n", &env);
    assert!(o.status.success(), "{}", all_text(&o));
    assert!(!file.exists(), "deleted on s");
}

#[test]
fn connect_with_an_unknown_model_warns() {
    let e = Env::new();
    let _v = e.vault();
    let fake = pi_support::FakeProvider::start(e.tmp.path(), r#"[{"text": "ok"}]"#, None);
    let o = e.with_stdin(
        &["connect", "anthropic-api", "--model", "claude-nada"],
        "sk-ant-api03-decoy-key\n",
        &[("LLM_BASE_URL", &fake.base_url)],
    );
    assert!(o.status.success(), "{}", all_text(&o));
    assert!(
        all_text(&o).contains("unknown context window"),
        "{}",
        all_text(&o)
    );
}

#[test]
fn connect_reuses_a_key_already_in_the_vault_and_still_probes() {
    let e = Env::new();
    let _v = e.vault();
    let tok = "sk-ant-api03-decoy-key";
    let fake = pi_support::FakeProvider::start(e.tmp.path(), r#"[{"text": "ok"}]"#, Some(tok));
    let env = [("LLM_BASE_URL", fake.base_url.as_str())];
    assert!(
        e.with_stdin(&["connect", "anthropic-api"], &format!("{tok}\n"), &env)
            .status
            .success()
    );
    // Second time: nothing on stdin, so any prompt would fail; the stored key is used.
    let o = e.with_stdin(
        &["connect", "anthropic-api", "--model", "claude-opus-5-5"],
        "",
        &env,
    );
    let out = all_text(&o);
    assert!(o.status.success(), "{out}");
    assert!(!out.contains("guardada"), "{out}");
    assert!(
        out.contains("connected: anthropic-api / claude-opus-5-5 answered"),
        "{out}"
    );
    let seen = fake.requests();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[1]["model"], "claude-opus-5-5");
    assert_eq!(seen[1]["headers"]["x_api_key_matches"], json!(true));
}

#[test]
fn connect_keeps_the_key_when_the_probe_fails() {
    let e = Env::new();
    let _v = e.vault();
    // Nothing listens on port 1.
    let o = e.with_stdin(
        &["connect", "anthropic-api"],
        "sk-ant-api03-decoy-key\n",
        &[("LLM_BASE_URL", "http://127.0.0.1:1")],
    );
    let out = all_text(&o);
    assert!(o.status.success(), "{out}");
    assert!(
        out.contains("stored, but anthropic-api did not answer"),
        "{out}"
    );
    assert!(!out.contains("sk-ant-api03-decoy"), "{out}");
    assert!(e.vault_list().contains("api.anthropic.com"));
    let cfg: Value = serde_json::from_slice(&e.config_bytes()).unwrap();
    assert_eq!(cfg["model"]["provider"], "anthropic");
}

// An upgrading person may have any provider id in config.json; `clio connect` is what the error
// tells them to run, so it must repair exactly that.
#[test]
fn connect_repairs_a_provider_that_is_not_a_connector() {
    let e = Env::new();
    let _v = e.vault();
    let cfg_path = e.home().join(".config/clio/config.json");
    let mut cfg: Value = serde_json::from_slice(&e.config_bytes()).unwrap();
    cfg["model"]["provider"] = json!("fakeprov");
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    // Every other command still refuses it, and points at connect.
    let o = e.try_clio(&["run", "--workspace", &e.path("proj"), "hazlo"]);
    assert!(
        !o.status.success() && all_text(&o).contains("clio connect opencode|claude|anthropic-api"),
        "{}",
        all_text(&o)
    );
    let fake = pi_support::FakeProvider::start(e.tmp.path(), r#"[{"text": "ok"}]"#, None);
    let o = e.with_stdin(
        &["connect", "opencode"],
        "oc-decoy-key-0123456789\n",
        &[("LLM_BASE_URL", &fake.base_url)],
    );
    assert!(o.status.success(), "{}", all_text(&o));
    let after: Value = serde_json::from_slice(&e.config_bytes()).unwrap();
    assert_eq!(after["model"]["provider"], "opencode-go");
    e.run_fix(); // the config loads again
}

/// `clio-vault`'s own `lock`, on the person's socket: the vault keeps running, locked.
fn lock_vault(e: &Env) {
    let sock = e.tmp.path().join("run/clio/vault-person.sock");
    let st = Command::new(vault_bin())
        .args(["--sock", &sock.display().to_string(), "lock"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success());
}

// A running but locked vault: every message says `clio vault start`, and that opens it.
#[test]
fn a_locked_vault_is_opened_by_vault_start() {
    let e = Env::new();
    let _v = e.vault();
    lock_vault(&e);
    // `unlocked` (connect) and `model_key` (a run with the built-in adapter) say it is locked.
    let o = e.with_stdin(&["connect", "claude"], "sk-ant-oat01-decoy\n", &[]);
    assert!(!o.status.success());
    assert!(
        all_text(&o).contains("the vault is locked: clio vault start"),
        "{}",
        all_text(&o)
    );
    let cfg_path = e.home().join(".config/clio/config.json");
    let mut cfg: Value = serde_json::from_slice(&e.config_bytes()).unwrap();
    cfg["model"]["command"] = json!([]);
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    let o = e.try_clio(&[
        "run",
        "--harness",
        "builtin",
        "--workspace",
        &e.path("proj"),
        "hazlo",
    ]);
    assert!(
        !o.status.success() && text(&o.stderr).contains("the vault is locked: clio vault start"),
        "{}",
        all_text(&o)
    );
    // A wrong passphrase leaves it locked and running.
    let o = e.with_stdin(&["vault", "start"], "wrong phrase\n", &[]);
    assert!(!o.status.success(), "{}", all_text(&o));
    assert!(
        all_text(&o).contains("clio vault start"),
        "{}",
        all_text(&o)
    );
    // The right one opens it.
    let o = e.with_stdin(&["vault", "start"], &format!("{PASS}\n"), &[]);
    assert!(o.status.success(), "{}", all_text(&o));
    let o = e.with_stdin(&["connect", "claude"], "", &[]);
    assert!(
        !all_text(&o).contains("the vault is locked"),
        "{}",
        all_text(&o)
    );
    // Running and open: nothing to do, and it says so.
    let o = e.with_stdin(&["vault", "start"], &format!("{PASS}\n"), &[]);
    assert!(
        !o.status.success() && all_text(&o).contains("is already running"),
        "{}",
        all_text(&o)
    );
}

/// The `clio-vault` processes serving this test's vault file.
fn vault_servers(e: &Env) -> Vec<String> {
    let mark = e.home().join(".local/share/clio/vault.bin");
    let mark = mark.to_string_lossy().into_owned();
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|p| std::fs::read(p.path().join("cmdline")).ok())
        .map(|c| String::from_utf8_lossy(&c).replace('\0', " "))
        .filter(|c| c.contains("serve") && c.contains(&mark))
        .collect()
}

#[test]
fn a_wrong_passphrase_at_start_leaves_no_server_behind() {
    let e = Env::new();
    drop(e.vault()); // created, then stopped
    assert!(vault_servers(&e).is_empty());
    let o = e.with_stdin(&["vault", "start"], "wrong phrase\n", &[]);
    assert!(!o.status.success(), "{}", all_text(&o));
    assert!(
        all_text(&o).contains("could not unlock the vault"),
        "{}",
        all_text(&o)
    );
    assert!(vault_servers(&e).is_empty(), "{:?}", vault_servers(&e));
    // And a normal start works afterwards.
    let _v = e.vault();
}

#[test]
fn doctor_says_a_key_file_already_in_the_vault_can_go() {
    let e = Env::new();
    let _v = e.vault();
    let key = e.home().join(".config/clio/opencode-api-key");
    std::fs::write(&key, "not-a-real-key").unwrap();
    let o = e.with_stdin(
        &["vault", "put", "opencode", "--dest", "model:opencode.ai"],
        "oc-decoy-key-0123456789\n",
        &[],
    );
    assert!(o.status.success(), "{}", all_text(&o));
    let out = text(&e.try_clio(&["doctor"]).stdout);
    let line = out
        .lines()
        .find(|l| l.contains("a key is left"))
        .unwrap_or_default();
    assert!(
        line.contains("it is already in the vault")
            && line.contains(&format!("rm {}", key.display())),
        "{out}"
    );
    // connect, which reuses the stored key, says the same.
    let fake = pi_support::FakeProvider::start(e.tmp.path(), r#"[{"text": "ok"}]"#, None);
    let o = e.with_stdin(
        &["connect", "opencode"],
        "",
        &[("LLM_BASE_URL", &fake.base_url)],
    );
    assert!(o.status.success(), "{}", all_text(&o));
    assert!(
        all_text(&o).contains(&format!("rm {}", key.display())),
        "{}",
        all_text(&o)
    );
    assert!(!all_text(&o).contains("oc-decoy-key"));
}

// Host is the factory mode (ADR 0012), and `clio run --workspace DIR` still asks for a copy of
// that folder; without a folder the builtin loop, which has no host mode, says so.
#[test]
fn a_named_workspace_is_a_copy_under_the_host_default() {
    let e = Env::new();
    let cfg_path = e.home().join(".config/clio/config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg.as_object_mut().unwrap().remove("mode");
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    e.run_fix();
    assert_eq!(
        e.read("app.py"),
        OLD_APP,
        "a copy: the original is untouched"
    );
    let proj = e.path("proj");
    let o = e.try_clio(&["run", "--host", "--workspace", &proj, "x"]);
    assert!(
        !o.status.success() && text(&o.stderr).contains("does not go with --host"),
        "{o:?}"
    );
    let o = e.try_clio(&["run", "x"]);
    assert!(
        !o.status.success() && text(&o.stderr).contains("host mode needs Pi"),
        "{o:?}"
    );
}
