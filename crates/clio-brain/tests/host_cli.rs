#![allow(clippy::unwrap_used)]
//! The person's commands of the host mode on a temporary HOME: `clio pending`, `clio approve`,
//! `clio reject`, `clio actions` and `clio undo` on a host session. What is set aside is made by
//! a real host-mode `ToolHost` (commands off: no launcher is looked for, so no `sudo`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use clio_toolhost::ask::HostRules;
use clio_toolhost::{Config, HostCfg, JailMode, Request, ToolHost};
use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");

struct World {
    _t: tempfile::TempDir,
    home: PathBuf,
    data: PathBuf,
    host: ToolHost,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().canonicalize().unwrap().join("home");
    let (config, data) = (home.join(".config/clio"), home.join(".local/share/clio"));
    let run = data.join("runs/20261117-120000-abcd");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join("meta.json"),
        json!({"id": "20261117-120000-abcd", "workspace": home, "instruction": "x",
               "created_ms": 0, "host": true})
        .to_string(),
    )
    .unwrap();
    let mut c = Config::new(&home, &run.join("scratch"), &run.join("toolhost"));
    c.jail = JailMode::Off;
    c.host = Some(HostCfg {
        home: home.clone(),
        state: data.join("host"),
        control: vec![config, data.clone()],
        rules: HostRules::factory(),
        run: "20261117-120000-abcd".into(),
        launcher: None,
    });
    World {
        host: ToolHost::new(c).unwrap(),
        _t: t,
        home,
        data,
    }
}

impl World {
    fn clio(&self, args: &[&str]) -> Output {
        Command::new(CLIO)
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("CLIO_JAIL", "off")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// Writes `path` (under the home) through the session's toolhost: the id it was set aside as.
    fn set_aside(&self, path: &str) -> String {
        let r = self.host.handle(Request {
            id: "i".into(),
            tool: "fs_write".into(),
            args: json!({"path": path, "content": "new"}),
            session: "20261117-120000-abcd".into(),
            call_id: "c".into(),
            tainted: false,
        });
        assert!(!r.ok && r.decision.rule == "host/aside", "{r:?}");
        r.content
            .strip_prefix("set aside ")
            .and_then(|c| c.split(':').next())
            .unwrap()
            .to_owned()
    }
}

fn out(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn pending_approve_actions_and_undo_on_a_host_session() {
    let w = world();
    let backup = w.home.join("backup.sql");
    std::fs::write(&backup, "old").unwrap();
    let id = w.set_aside("backup.sql");
    assert_eq!(read(&backup), "old", "a set-aside write ran");

    let o = w.clio(&["pending"]);
    assert!(o.status.success() && out(&o).contains(&id), "{}", out(&o));
    assert!(out(&o).contains("«delete»") && out(&o).contains("backup.sql"));
    let o = w.clio(&["pending", "--json"]);
    let list: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], json!(id));

    let o = w.clio(&["approve", &id]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains(&format!("approved {id}")), "{}", out(&o));
    assert_eq!(read(&backup), "new");
    // Answered at the terminal: it is not one of the day's questions any more.
    let q = read(&w.data.join("host/questions.json"));
    assert!(!q.contains(&id), "{q}");
    let o = w.clio(&["approve", &id]);
    assert!(
        !o.status.success() && out(&o).contains("already ran"),
        "{}",
        out(&o)
    );
    let o = w.clio(&["pending"]);
    assert!(
        out(&o).contains("nothing is waiting for your approval"),
        "{}",
        out(&o)
    );

    let o = w.clio(&["actions", "--run", "20261117-120000-abcd"]);
    let text = out(&o);
    assert!(o.status.success(), "{text}");
    for word in ["aside", "run", "done", "fs_write"] {
        assert!(text.contains(word), "{word}: {text}");
    }
    let o = w.clio(&["actions", "--json", "--run", "other"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stdout).unwrap(),
        json!([])
    );

    // The session's changes go back; a second undo has nothing left.
    let o = w.clio(&["undo", "20261117-120000-abcd"]);
    assert!(
        o.status.success() && out(&o).contains("restored"),
        "{}",
        out(&o)
    );
    assert_eq!(read(&backup), "old");
    let o = w.clio(&["undo", "20261117-120000-abcd"]);
    assert!(
        o.status.success() && out(&o).contains("nothing is left to undo"),
        "{}",
        out(&o)
    );
}

#[test]
fn a_rejected_action_never_runs() {
    let w = world();
    let f = w.home.join("data.bak");
    std::fs::write(&f, "old").unwrap();
    let id = w.set_aside("data.bak");
    let o = w.clio(&["reject", &id]);
    assert!(
        o.status.success() && out(&o).contains(&format!("rejected {id}")),
        "{}",
        out(&o)
    );
    let o = w.clio(&["approve", &id]);
    assert!(
        !o.status.success() && out(&o).contains("was rejected"),
        "{}",
        out(&o)
    );
    assert_eq!(read(&f), "old");
    let o = w.clio(&["approve", "nothing"]);
    assert!(
        !o.status.success() && out(&o).contains("no pending item"),
        "{}",
        out(&o)
    );
    let o = w.clio(&["approve"]);
    assert!(
        !o.status.success() && out(&o).contains("clio pending"),
        "{}",
        out(&o)
    );
}

/// Rewrites the saved pending `id` with `f`.
fn edit(w: &World, id: &str, f: impl FnOnce(&mut Value)) {
    let file = w.data.join(format!("host/pending/{id}.json"));
    let mut v: Value = serde_json::from_str(&read(&file)).unwrap();
    f(&mut v);
    std::fs::write(&file, v.to_string()).unwrap();
}

#[test]
fn what_is_shown_comes_from_the_arguments_and_an_expired_one_neither_waits_nor_builds_anything() {
    let w = world();
    std::fs::write(w.home.join("backup.sql"), "old").unwrap();
    let id = w.set_aside("backup.sql");
    edit(&w, &id, |v| v["what"] = json!("harmless.txt"));
    let o = w.clio(&["pending"]);
    assert!(
        out(&o).contains("backup.sql") && !out(&o).contains("harmless"),
        "{}",
        out(&o)
    );

    // Past its time, in a session whose folder is gone: not listed, and approving it says so
    // before any folder is made or launcher looked for.
    edit(&w, &id, |v| {
        v["expires_ms"] = json!(1);
        v["run"] = json!("20261117-130000-ffff");
    });
    let o = w.clio(&["pending"]);
    assert!(
        out(&o).contains("nothing is waiting for your approval"),
        "{}",
        out(&o)
    );
    let o = w.clio(&["approve", &id]);
    assert!(
        !o.status.success() && out(&o).contains("expired on"),
        "{}",
        out(&o)
    );
    assert!(!w.data.join("runs/20261117-130000-ffff").exists());
    assert_eq!(read(&w.home.join("backup.sql")), "old");
}
