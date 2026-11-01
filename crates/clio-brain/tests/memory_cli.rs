#![allow(clippy::unwrap_used)]
//! `clio memory ...` and the memory in `clio run`, end to end with the real binary, a scripted
//! adapter that also keeps every request it was sent, a temporary HOME and no network. Fake
//! secrets are built at run time.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use clio_canaries::Canaries;
use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");

/// Answers from `responses.json` in order and keeps each request it received as `req-N.json`.
const ADAPTER: &str = r#"import json, os, sys
d = os.path.dirname(os.path.abspath(__file__))
req = sys.stdin.read()
n = int(open(d + "/n").read()) if os.path.exists(d + "/n") else 0
r = json.load(open(d + "/responses.json"))
open(d + "/n", "w").write(str(n + 1))
open(d + "/req-%d.json" % n, "w").write(req)
json.dump({"content": r[min(n, len(r) - 1)], "input_tokens": 1000, "output_tokens": 50}, sys.stdout)
"#;

struct Env {
    tmp: tempfile::TempDir,
}

impl Env {
    fn new(init: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["home", "run", "proj", "script"] {
            std::fs::create_dir(tmp.path().join(d)).unwrap();
        }
        std::fs::write(tmp.path().join("script/adapter.py"), ADAPTER).unwrap();
        std::fs::write(tmp.path().join("script/host-check.sh"), "exit 0\n").unwrap();
        std::fs::write(tmp.path().join("proj/app.py"), "x = 1\n").unwrap();
        let e = Self { tmp };
        if init {
            e.clio(&["init"]);
            let cfg_path = e.home().join(".config/clio/config.json");
            let mut cfg: Value =
                serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
            cfg["model"]["command"] = json!(["python3", e.path("script/adapter.py")]);
            std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        }
        e
    }

    fn path(&self, rel: &str) -> String {
        self.tmp.path().join(rel).display().to_string()
    }
    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }
    fn db(&self) -> PathBuf {
        self.home().join(".local/share/clio/memory.db")
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

    fn request(&self, n: usize) -> String {
        std::fs::read_to_string(self.tmp.path().join(format!("script/req-{n}.json"))).unwrap()
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
            .env_remove("CLIO_DEBUG")
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

    /// Same, but typing `input` on the standard input (an answer to a question).
    fn clio_with_stdin(&self, args: &[&str], input: &str) -> String {
        let mut c = self.cmd(args);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }

    fn run(&self, instruction: &str, extra: &[&str]) -> (String, Value) {
        let ws = self.path("proj");
        let mut args = vec!["run", "--workspace", ws.as_str()];
        args.extend_from_slice(extra);
        args.push(instruction);
        let out = self.clio(&args);
        let id = out
            .lines()
            .find_map(|l| l.strip_prefix("run:").map(|s| s.trim().to_owned()))
            .unwrap();
        let dir = self.home().join(".local/share/clio/runs").join(&id);
        let summary: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("summary.json")).unwrap())
                .unwrap();
        (out, summary)
    }

    fn run_dir(&self, summary: &Value) -> PathBuf {
        self.home()
            .join(".local/share/clio/runs")
            .join(summary["id"].as_str().unwrap())
    }
}

// On macOS CI the long pty sequences did not finish; the interactive tests run on Linux only.
#[cfg(target_os = "linux")]
#[test]
fn pty_the_memory_screen_opens_searches_goes_back_and_restores_the_terminal() {
    let e = Env::new(true);
    e.clio(&["memory", "remember", "El proyecto se llama ZAFIRO"]);
    e.clio(&["memory", "remember", "Prefiero respuestas cortas"]);
    // A pending one, from a run that read a file.
    e.script(&[
        json!({"tool": "read_file", "args": {"path": "app.py"}}),
        json!({"final": "leí"}),
    ]);
    e.run("lee app.py", &[]);
    let cfg = json!({
        "clio": CLIO,
        "cwd": e.path("proj"),
        "runs": e.home().join(".local/share/clio/runs").display().to_string(),
        "env": {
            "HOME": e.home().display().to_string(),
            "XDG_RUNTIME_DIR": e.path("run"),
            "CLIO_HOST_CHECK": e.path("script/host-check.sh"),
            "TERM": "xterm-256color",
            "LANG": "C.UTF-8",
            "PATH": std::env::var("PATH").unwrap_or_default(),
        },
    });
    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pty_driver.py");
    let o = Command::new("python3")
        .arg("-I")
        .arg(driver)
        .arg("memory")
        .arg(cfg.to_string())
        .output()
        .unwrap();
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let v: Value = serde_json::from_str(text(&o.stdout).trim()).unwrap();
    assert_eq!(v["opened"], true, "{v}");
    assert_eq!(v["memory_screen"], true, "m opens the memory screen: {v}");
    assert_eq!(v["pending_section"], true, "{v}");
    assert_eq!(v["search_counts"], true, "/ filters the list: {v}");
    assert_eq!(v["back_home"], true, "Esc goes back: {v}");
    assert_eq!(v["code"], 0, "q leaves: {v}");
    assert_eq!(v["alt_off_after_on"], true, "{v}");
    assert_eq!(v["after"]["canonical"], true, "{v}");
    assert_eq!(v["after"]["echo"], true, "{v}");
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn init_creates_the_memory_file_private_and_inside_the_data_folder_only() {
    let e = Env::new(true);
    assert_eq!(
        std::fs::metadata(e.db()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let mut all = Vec::new();
    files_under(e.tmp.path(), &mut all);
    let dbs: Vec<_> = all
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "memory.db"))
        .collect();
    assert_eq!(dbs, vec![&e.db()], "one memory file, and only there");
    // Idempotent: a second init changes nothing.
    e.clio(&["init"]);
}

#[test]
fn reading_commands_never_create_the_file() {
    let e = Env::new(false);
    for args in [
        vec!["memory", "list"],
        vec!["memory", "list", "--pending"],
        vec!["memory", "search", "algo"],
        vec!["memory", "stats"],
        vec!["memory", "show", "1"],
        vec!["memory", "export"],
    ] {
        let out = e.clio(&args);
        assert!(out.contains("todavía no hay memoria"), "{args:?}: {out}");
    }
    assert!(!e.home().join(".local").exists(), "nothing was created");
}

#[test]
fn every_subcommand_works_and_speaks_json_where_it_should() {
    let e = Env::new(true);
    assert!(e.clio(&["memory"]).contains("clio memory search"));
    assert!(
        e.clio(&["memory", "list"])
            .contains("todavía no hay recuerdos activos")
    );
    assert!(
        e.clio(&["memory", "list", "--pending"])
            .contains("nada pendiente")
    );

    // remember: origin persona, active at once; optionally in a fixed block.
    let out = e.clio(&[
        "memory", "remember", "Mi", "proyecto", "usa", "Rust", "y", "SQLite",
    ]);
    assert!(
        out.contains("m1") && out.contains("origen: persona"),
        "{out}"
    );
    e.clio(&[
        "memory",
        "remember",
        "Responde siempre en español",
        "--block",
        "reglas",
    ]);
    assert!(
        e.try_clio(&["memory", "remember", "x", "--block", "nada"])
            .status
            .code()
            == Some(1)
    );

    let list = e.clio(&["memory", "list"]);
    assert!(
        list.contains("m1") && list.contains("persona") && list.contains("semántico"),
        "{list}"
    );
    let j: Value = serde_json::from_str(&e.clio(&["memory", "list", "--json"])).unwrap();
    assert_eq!(j.as_array().unwrap().len(), 2);
    assert_eq!(j[1]["origin"], "persona");
    assert_eq!(j[1]["state"], "activo");
    assert_eq!(j[1]["actor"], "persona");

    // search: what is relevant, and "nothing relevant" is a clean answer, not an error.
    assert!(
        e.clio(&[
            "memory", "search", "qué", "lenguaje", "usa", "mi", "proyecto"
        ])
        .contains("Rust")
    );
    let s: Value =
        serde_json::from_str(&e.clio(&["memory", "search", "--json", "lenguaje de mi proyecto"]))
            .unwrap();
    assert_eq!(s["abstained"], false);
    assert_eq!(s["results"][0]["id"], 1);
    let none = e.clio(&["memory", "search", "receta de arepas con queso"]);
    assert!(none.contains("nada relevante"), "{none}");
    let none: Value =
        serde_json::from_str(&e.clio(&["memory", "search", "--json", "receta de arepas"])).unwrap();
    assert_eq!(none["abstained"], true);

    // show, with and without json, with and without the `m` prefix.
    let show = e.clio(&["memory", "show", "m1"]);
    assert!(show.contains("persona") && show.contains("Rust"), "{show}");
    let j: Value = serde_json::from_str(&e.clio(&["memory", "show", "1", "--json"])).unwrap();
    assert_eq!(j["content"], "Mi proyecto usa Rust y SQLite");
    assert_eq!(
        e.try_clio(&["memory", "show", "999"]).status.code(),
        Some(1)
    );
    assert_eq!(
        e.try_clio(&["memory", "show", "abc"]).status.code(),
        Some(1)
    );

    // feedback: useful and wrong are noted; nothing is deleted or changed.
    assert!(
        e.clio(&["memory", "feedback", "1", "useful"])
            .contains("anotado")
    );
    assert!(
        e.clio(&["memory", "feedback", "m1", "wrong"])
            .contains("nunca borra")
    );
    assert_eq!(
        e.try_clio(&["memory", "feedback", "1", "meh"])
            .status
            .code(),
        Some(1)
    );
    let j: Value = serde_json::from_str(&e.clio(&["memory", "show", "1", "--json"])).unwrap();
    assert_eq!(j["content"], "Mi proyecto usa Rust y SQLite");
    assert_eq!(
        j["state"], "activo",
        "one wrong signal does not even flag it"
    );

    // stats and export.
    let st = e.clio(&["memory", "stats"]);
    assert!(
        st.contains("activos 2") && st.contains("sin modelo de embeddings"),
        "{st}"
    );
    let sj: Value = serde_json::from_str(&e.clio(&["memory", "stats", "--json"])).unwrap();
    assert_eq!(
        (sj["active"].as_u64(), sj["quarantine"].as_u64()),
        (Some(2), Some(0))
    );
    let export = e.clio(&["memory", "export"]);
    let first: Value = serde_json::from_str(export.lines().next().unwrap()).unwrap();
    assert!(first["format"].is_string(), "{first}");
    for l in export.lines() {
        serde_json::from_str::<Value>(l).unwrap();
    }
    let file = e.path("out.jsonl");
    assert!(e.clio(&["memory", "export", &file]).contains("0600"));
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        e.try_clio(&["memory", "export", &file]).status.code(),
        Some(1),
        "an export never overwrites"
    );

    // purge needs an id; an unknown subcommand is a usage error.
    let p = e.try_clio(&["memory", "purge"]);
    assert_eq!(p.status.code(), Some(1));
    assert!(
        text(&p.stderr).contains("falta el id"),
        "{}",
        text(&p.stderr)
    );
    assert_eq!(e.try_clio(&["memory", "zzz"]).status.code(), Some(2));
}

#[test]
fn a_run_that_read_files_leaves_its_record_in_quarantine_and_promote_and_archive_ask() {
    let e = Env::new(true);
    e.script(&[
        json!({"tool": "read_file", "args": {"path": "app.py"}}),
        json!({"final": "leí app.py"}),
    ]);
    let (panel, summary) = e.run("lee app.py", &[]);
    assert_eq!(summary["tainted"], true);
    assert_eq!(summary["memory"]["pending"], 1, "{summary}");
    assert!(
        panel.contains("memoria: 0 guardado(s), 1 en cuarentena"),
        "{panel}"
    );
    let pending = e.clio(&["memory", "list", "--pending"]);
    assert!(
        pending.contains("cuarentena")
            && pending.contains("episódico")
            && pending.contains("externo"),
        "{pending}"
    );
    assert!(
        e.clio(&["memory", "list"])
            .contains("todavía no hay recuerdos activos")
    );
    assert!(
        e.clio(&["memory", "search", "lee app.py"])
            .contains("nada relevante")
    );
    let shown = e.clio(&["memory", "show", "1"]);
    assert!(shown.contains("en cuarentena"), "{shown}");

    // The question is asked; no answer, "n" or an empty stdin changes nothing.
    assert!(
        e.clio(&["memory", "promote", "1"])
            .contains("no se cambió nada")
    );
    assert!(
        e.clio_with_stdin(&["memory", "promote", "1"], "n\n")
            .contains("no se cambió nada")
    );
    assert!(e.clio(&["memory", "list", "--pending"]).contains("m1"));
    assert!(
        e.clio_with_stdin(&["memory", "promote", "1"], "s\n")
            .contains("aprobado: m1")
    );
    assert!(e.clio(&["memory", "list"]).contains("m1"));
    assert!(
        e.clio(&["memory", "archive", "1"])
            .contains("no se cambió nada")
    );
    assert!(
        e.clio(&["memory", "archive", "1", "--yes"])
            .contains("archivado: m1")
    );
    assert!(
        e.clio(&["memory", "list"])
            .contains("todavía no hay recuerdos activos")
    );
    assert_eq!(
        e.try_clio(&["memory", "promote", "77", "--yes"])
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn the_second_run_gets_what_the_first_learned_as_data_with_its_layer() {
    let e = Env::new(true);
    e.script(&[
        json!({"tool": "write_file", "args": {"path": "codigo.txt", "content": "ZAFIRO-7\n"}}),
        json!({"final": "Anoté el código ZAFIRO-7 en codigo.txt"}),
    ]);
    let (panel1, s1) = e.run(
        "El código del proyecto es ZAFIRO-7, anótalo en codigo.txt",
        &[],
    );
    assert_eq!(s1["tainted"], false);
    assert_eq!(s1["memory"]["saved"].as_array().unwrap().len(), 1);
    assert_eq!(s1["memory"]["pending"], 0);
    assert!(
        panel1.contains("memoria: 1 guardado(s), 0 en cuarentena"),
        "{panel1}"
    );
    assert!(
        !e.request(0).contains("UNTRUSTED-DATA memory"),
        "nothing to remember yet"
    );

    e.script(&[json!({"final": "El código es ZAFIRO-7"})]);
    let (panel2, s2) = e.run("¿cuál era el código del proyecto?", &["--debug"]);
    let id = s1["memory"]["saved"][0].as_i64().unwrap();
    assert_eq!(s2["memory"]["recalled"], json!([id]));
    // The model was handed it as DATA with the id, after the person's words.
    let req = e.request(0);
    assert!(
        req.contains("UNTRUSTED-DATA memory") && req.contains("ZAFIRO-7"),
        "{req}"
    );
    assert!(req.contains(&format!("[m{id}]")));
    assert!(panel2.contains("memoria:"), "{panel2}");
    // The layer is in the run's usage, with a size.
    let usage: Value =
        serde_json::from_str(&std::fs::read_to_string(e.run_dir(&s2).join("usage.json")).unwrap())
            .unwrap();
    assert_eq!(usage["context"]["memory_items"], 1);
    assert!(usage["context"]["memory"].as_u64().unwrap() > 20, "{usage}");
    // Debug shows what was recalled and what it proposed to save at the close.
    let log = std::fs::read_to_string(e.run_dir(&s2).join("debug.log")).unwrap();
    let kinds: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mem: Vec<&Value> = kinds.iter().filter(|v| v["kind"] == "memory").collect();
    assert_eq!(mem.len(), 2, "recalled + proposed at the close: {log}");
    assert!(mem[0]["title"].as_str().unwrap().contains("recordó 1"));
    assert!(mem[0]["lines"][0].as_str().unwrap().contains("puntaje"));
    assert!(
        mem[1]["title"]
            .as_str()
            .unwrap()
            .contains("al cerrar propuso guardar")
    );
    assert!(
        kinds
            .iter()
            .any(|v| v["kind"] == "context" && v["memory_items"] == 1)
    );
}

#[test]
fn nothing_relevant_means_no_memory_layer_in_the_run() {
    let e = Env::new(true);
    e.clio(&["memory", "remember", "El código del proyecto es ZAFIRO-7"]);
    e.script(&[json!({"final": "pomme"})]);
    let (_, s) = e.run("traduce al francés la palabra manzana", &[]);
    assert_eq!(s["memory"]["recalled"], json!([]));
    assert_eq!(s["memory"]["abstained"], true);
    assert!(!e.request(0).contains("UNTRUSTED-DATA memory"));
}

#[test]
fn a_decoy_in_what_a_run_wants_to_remember_is_nowhere_in_the_home() {
    let e = Env::new(true);
    let c = Canaries::fresh();
    let decoy = c.value("github").unwrap().to_owned();
    e.script(&[json!({
        "final": format!("ok\nRECORDAR: mi clave es {decoy}\nPROCEDIMIENTO: publica con {decoy}")
    })]);
    let ws = e.path("proj");
    let mut cmd = e.cmd(&["run", "--workspace", &ws, "--debug", "guarda mi clave"]);
    cmd.env("OPENCODE_API_KEY", &decoy);
    let o = cmd.output().unwrap();
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    c.assert_clean("stdout", &text(&o.stdout));
    c.assert_clean("stderr", &text(&o.stderr));
    // The memory file, the runs, the debug log: every byte under HOME.
    let mut all = Vec::new();
    files_under(&e.home(), &mut all);
    assert!(all.iter().any(|p| p.ends_with("debug.log")));
    for p in &all {
        let bytes = std::fs::read(p).unwrap();
        c.assert_clean(&p.display().to_string(), &String::from_utf8_lossy(&bytes));
    }
    // And the proposals did happen: crossed out, in quarantine.
    let pending = e.clio(&["memory", "list", "--pending"]);
    assert!(pending.contains("procedimental"), "{pending}");
    for l in e.clio(&["memory", "list", "--json", "--pending"]).lines() {
        c.assert_clean("list", l);
    }
}

#[test]
fn text_from_the_memory_file_never_reaches_the_terminal_raw() {
    use clio_memory::{
        ActorKind, Engine, MemoryType, OpenMode, Origin, Proposal, Quota, SystemClock,
    };
    use std::sync::Arc;
    let e = Env::new(true);
    // The person types an escape: it is cleaned when stored.
    e.clio(&["memory", "remember", "rojo\u{1b}[31m peligro\u{7}"]);
    // Another tool writes raw escapes straight into the file.
    {
        let engine = Engine::open(&e.db(), OpenMode::ReadWrite, Arc::new(SystemClock)).unwrap();
        let scope = engine.scopes().unwrap().remove(0).id;
        let actor = engine
            .register_actor("otra", ActorKind::Importer, Quota::default())
            .unwrap();
        engine
            .propose(Proposal {
                scope,
                actor,
                kind: MemoryType::Semantic,
                content: "evil\u{1b}[2Jtext\u{1b}]0;pwned\u{7}\u{202e}".into(),
                topic_key: None,
                origin: Origin::External,
                session: None,
                episode: None,
                source: Some("src\u{1b}[31m".into()),
                valid_from: None,
                valid_to: None,
            })
            .unwrap();
    }
    for args in [
        vec!["memory", "list"],
        vec!["memory", "list", "--pending"],
        vec!["memory", "show", "2"],
        vec!["memory", "search", "rojo peligro"],
    ] {
        let out = e.clio(&args);
        assert!(
            !out.chars()
                .any(|c| (c.is_control() && c != '\n') || c == '\u{202e}'),
            "{args:?}: {out:?}"
        );
    }
    assert!(e.clio(&["memory", "show", "2"]).contains("evil"));
}

// S-24, S-25: `clio memory purge` asks first, deletes the content for good (the file and the
// vector), leaves only a tombstone, and `show` says so instead of printing anything.
#[test]
fn memory_purge_asks_first_deletes_for_good_and_leaves_only_a_tombstone() {
    let e = Env::new(true);
    let secret_text = format!("la clave del almacén es {}", "gamma-ultra-7731");
    e.clio(&["memory", "remember", &secret_text]);
    assert!(
        e.clio(&["memory", "show", "1"])
            .contains("gamma-ultra-7731")
    );

    // Without an answer, or with "n", nothing is deleted.
    assert!(
        e.clio_with_stdin(&["memory", "purge", "1"], "n\n")
            .contains("no se cambió nada")
    );
    assert!(
        e.clio(&["memory", "show", "1"])
            .contains("gamma-ultra-7731")
    );

    let out = e.clio_with_stdin(&["memory", "purge", "1", "--reason", "ya no vale"], "s\n");
    assert!(out.contains("purgado: m1"), "{out}");

    // The tombstone is all that is left.
    let shown = e.clio(&["memory", "show", "1"]);
    assert!(shown.contains("fue purgado"), "{shown}");
    assert!(shown.contains("ya no vale"), "{shown}");
    assert!(!shown.contains("gamma-ultra-7731"), "{shown}");
    assert!(
        !e.clio(&["memory", "search", "almacén"])
            .contains("gamma-ultra-7731")
    );

    // Not in the file nor in its write-ahead log, nor in an export.
    let db = std::fs::read(e.db()).unwrap();
    let wal = std::fs::read(format!("{}-wal", e.db().display())).unwrap_or_default();
    for (name, bytes) in [("db", db), ("wal", wal)] {
        assert!(
            !bytes
                .windows("gamma-ultra-7731".len())
                .any(|w| w == b"gamma-ultra-7731"),
            "the purged text is still in the {name}"
        );
    }
    assert!(!e.clio(&["memory", "export"]).contains("gamma-ultra-7731"));
    // A second purge of the same memory is an error, not a silent success.
    assert!(
        !e.try_clio(&["memory", "purge", "1", "--yes"])
            .status
            .success()
    );
}
