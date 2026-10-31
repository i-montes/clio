#![allow(clippy::unwrap_used)]
//! The `clio` command line end to end, with no network: a scripted adapter plays the model (a
//! small Python program that answers from a list), the HOME is a temporary folder, and the
//! workspace is another one. Fake secrets are built at run time.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");

const ADAPTER: &str = r#"import json, os, sys, time
sys.stdin.read()
time.sleep(float(os.environ.get("ADAPTER_SLEEP", "0")))
d = os.path.dirname(os.path.abspath(__file__))
n = int(open(d + "/n").read()) if os.path.exists(d + "/n") else 0
r = json.load(open(d + "/responses.json"))
open(d + "/n", "w").write(str(n + 1))
json.dump({"content": r[min(n, len(r) - 1)]}, sys.stdout)
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
            .env(
                "CLIO_HOST_CHECK",
                self.tmp.path().join("script/host-check.sh"),
            )
            .env_remove("OPENCODE_API_KEY")
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
            "arregla total en app.py",
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
        e.clio(&[
            "run",
            "--workspace",
            &e.path("proj"),
            "arregla total en app.py",
        ])
    };
    // run did NOT touch the original, and said what it left out.
    assert_eq!(e.read("app.py"), OLD_APP);
    assert!(!e.proj().join("docs").exists());
    assert!(
        out_run.contains("NO se copió") && out_run.contains(".env"),
        "{out_run}"
    );
    assert!(
        out_run.contains("1 modificados, 1 creados, 0 borrados"),
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
    assert!(applied.contains("2 archivo(s) aplicados"), "{applied}");
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
    assert!(text(&o.stdout).contains("no se aplicó"));
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
        text(&o.stderr).contains("cambió en el original"),
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
        text(&o.stderr).contains("enlace simbólico"),
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
        text(&o.stderr).contains("ruta protegida"),
        "{}",
        text(&o.stderr)
    );
    std::fs::remove_dir_all(sandbox.join(".git")).unwrap();
    std::fs::write(sandbox.join(".env.production"), "A=1\n").unwrap();
    let o = e.try_clio(&["apply", "--yes", "--force", &id]);
    assert!(
        text(&o.stderr).contains("parece un secreto"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(e.read("app.py"), OLD_APP);
    assert!(!e.proj().join(".git/hooks").exists());
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
    assert!(out.contains("se deja como está"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        before,
        "config untouched"
    );
    for d in [
        ".config/clio",
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
fn init_checks_the_key_without_printing_it() {
    let e = Env::new();
    let secret = format!("sk-{}", "q".repeat(20));
    let key = e.home().join(".config/clio/opencode-api-key");
    std::fs::write(&key, &secret).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = e.clio(&["init"]);
    assert!(
        out.contains("ADVERTENCIA") && out.contains("chmod 600"),
        "{out}"
    );
    assert!(!out.contains(&secret));
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let out = e.clio(&["init"]);
    assert!(
        out.contains("clave del modelo encontrada") && !out.contains(&secret),
        "{out}"
    );
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
    // A key file that others can read is a FAIL too.
    let key = cfg_dir.join("opencode-api-key");
    std::fs::write(&key, "not-a-real-key").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let o = e.try_clio(&["doctor"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(!text(&o.stdout).contains("not-a-real-key"));
}

#[test]
fn run_refuses_broad_or_huge_workspaces() {
    let e = Env::new();
    e.script_fix();
    for ws in ["/", &e.home().display().to_string()] {
        let o = e.try_clio(&["run", "--workspace", ws, "hazlo"]);
        assert!(!o.status.success(), "{ws}");
        assert!(text(&o.stderr).contains("rechazo"), "{}", text(&o.stderr));
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
    assert!(text(&o.stderr).contains("comando desconocido"));
}

#[test]
fn run_prints_a_line_per_step_and_a_plain_panel_without_escape_codes() {
    let e = Env::new();
    e.script_fix();
    let o = e.clio(&[
        "run",
        "--workspace",
        &e.path("proj"),
        "arregla total en app.py",
    ]);
    assert!(!o.contains('\u{1b}'), "a pipe gets no color codes: {o:?}");
    assert!(o.contains("[1/40] read_file app.py"), "{o}");
    assert!(o.contains("permitido"), "{o}");
    assert!(o.contains("[4/40] el modelo dice que terminó"), "{o}");
    assert!(
        o.contains("agente:") && o.contains("El original NO se tocó"),
        "{o}"
    );
    assert!(
        o.contains("  M app.py") && o.contains("  A docs/new.md"),
        "{o}"
    );
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
        table.starts_with("RUN") && table.contains("pendiente de revisar"),
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
    assert!(e.clio(&["runs"]).contains("cancelado"));
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
        json!({"final": "Corregí total() en app.py: ya no suma 1 de más. No pude correr las pruebas; ejecútalas antes de aplicar."}),
    ]);
    println!("--- init\n{}", e.clio(&["init"]));
    let o = e.try_clio(&[
        "run",
        "--workspace",
        &e.path("proj"),
        "arregla total en app.py",
    ]);
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
