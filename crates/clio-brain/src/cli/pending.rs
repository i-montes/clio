//! Host mode from the terminal: what waits for the person's approval (`clio pending`), saying yes
//! or no to it (`clio approve`, `clio reject`), and the log of what was run (`clio actions`).
//! What is set aside comes from the toolhost (`clio_toolhost::pending`); here it is only shown,
//! and an approval runs in a host-mode `ToolHost` on that session's run folder.

use std::path::PathBuf;

use clio_brain::pi::launch::valid_id;
use clio_toolhost::ask::HostRules;
use clio_toolhost::pending::{self, Pending, State};
use clio_toolhost::{Config, HostCfg, JailMode, ToolHost};
use clio_vault::{Scrubber, SecretValue};
use serde_json::Value;

use super::home::Dirs;
use super::tui::sanitize;
use super::{Res, io_err, mkdir_private, now_ms, stamp};

/// `Dirs.data/host`: the action log and what is set aside.
pub fn state(dirs: &Dirs) -> PathBuf {
    dirs.data.join("host")
}

/// What waits for the person, oldest first: one past its time does not wait any more.
pub fn waiting(dirs: &Dirs) -> Vec<Pending> {
    let now = now_ms();
    pending::list(&state(dirs))
        .into_iter()
        .filter(|p| p.state == State::Waiting && now < p.expires_ms)
        .collect()
}

/// What `p` does, for the person: from its arguments (`pending::shown`), key-looking values struck
/// out. Never the stored `what`. A secret request: its name and where it may go.
pub fn what(p: &Pending) -> String {
    if let Some(b) = pending::block_of(p) {
        let id = b.id.map_or_else(String::new, |n| format!(" m{n}"));
        let text = b.text.map_or_else(String::new, |t| format!(": {t}"));
        // A replace or a remove: the entry as it reads now, so it is read before `clio approve`.
        let old = b
            .old
            .map_or_else(String::new, |o| format!(" (it says now: {o})"));
        return format!("{}{id} in your block «{}»{old}{text}", b.action, b.name);
    }
    match pending::secret_of(p) {
        Some((name, dest, _)) => format!("{name}, it can only be sent to {dest}"),
        None => Scrubber::default().scrub(pending::shown(p)),
    }
}

/// A block change as the approval box shows it, row by row, cleaned: what `clio approve` prints.
/// The session is `suspect` when its tool host left that mark in the run folder.
fn block_text(dirs: &Dirs, p: &Pending) -> Option<String> {
    let suspect = std::fs::read_to_string(dirs.runs.join(&p.run).join("toolhost/tainted.json"))
        .is_ok_and(|t| t.contains("\"suspect\""));
    let b = super::tui::ask::BlockAsk::new(p, suspect)?;
    let rows: Vec<String> = b
        .parts()
        .into_iter()
        .map(|(part, text)| match part {
            super::tui::ask::Part::Text => format!("  {text}"),
            _ => text,
        })
        .collect();
    Some(rows.join("\n"))
}

/// The person's answer to secret request `id`: the value (`Some`) goes to the vault, `None`
/// declines it. What the model is told with the next message.
pub fn answer_secret(dirs: &Dirs, id: &str, value: Option<&SecretValue>) -> Res<String> {
    let st = state(dirs);
    let p = pending::check(&st, id, now_ms())?;
    let Some((name, dest, _)) = pending::secret_of(&p) else {
        return Err(format!("{id} is not a secret request"));
    };
    let note = match value {
        None => {
            pending::reject(&st, id)?;
            format!("the person declined {name}")
        }
        Some(v) => {
            pending::answer_secret(&st, id, now_ms(), |_| {
                super::vault::store_requested(dirs, name, dest, v)
            })?;
            format!("secret {name} saved, bound to {dest}")
        }
    };
    let _ = pending::answered_at_terminal(&st, id);
    Ok(note)
}

/// Secret request `id` while it still waits for the person: (name, destination, why). After a
/// store the vault refused, the box is offered again from this.
pub fn secret_waiting(dirs: &Dirs, id: &str) -> Option<(String, String, String)> {
    let p = pending::check(&state(dirs), id, now_ms()).ok()?;
    let (n, d, w) = pending::secret_of(&p)?;
    Some((n.into(), d.into(), w.into()))
}

/// Its class in «», and why it waits when the class alone does not say it.
pub fn label(p: &Pending) -> String {
    match &p.reason {
        Some(r) => format!("«{} · {}»", p.class, sanitize::line(r)),
        None => format!("«{}»", p.class),
    }
}

/// One line for the person: id, class, what, since when, until when, and whether it was asked.
pub fn line(p: &Pending, now: i64) -> String {
    format!(
        "{}  {}  {}  · since {} UTC · {} {} UTC{}",
        p.id,
        label(p),
        sanitize::line(&what(p)),
        stamp(p.created_ms),
        if now >= p.expires_ms {
            "expired"
        } else {
            "expires"
        },
        stamp(p.expires_ms),
        if p.ask {
            ""
        } else {
            " · not asked (goes to the day's summary)"
        }
    )
}

pub fn show(dirs: &Dirs, json: bool) -> Res<()> {
    let list = waiting(dirs);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if list.is_empty() {
        println!("nothing is waiting for your approval");
        return Ok(());
    }
    let now = now_ms();
    for p in &list {
        println!("{}", line(p, now));
    }
    println!("\nclio approve <id> runs it · clio reject <id> drops it");
    Ok(())
}

/// A host-mode `ToolHost` on the run folder of `run`, as its session had it (same folders, same
/// rules), with nothing of the session's own: an approval does not mark the running session.
fn host_for(dirs: &Dirs, run: &str) -> Res<ToolHost> {
    if !valid_id(run) {
        return Err(format!("the session {run:?} is not valid"));
    }
    let dir = dirs.runs.join(run);
    let scratch = dir.join("scratch");
    mkdir_private(&scratch)?;
    let mut rules = HostRules::factory();
    if let Some(f) = dirs.host_rules() {
        let text = std::fs::read_to_string(&f).map_err(|e| io_err(&f, &e))?;
        rules = rules.with_file(&text)?;
    }
    let mut cfg = Config::new(&dirs.home, &scratch, &dir.join("toolhost"));
    // As `Toolhost::start`: `CLIO_JAIL=off` leaves out commands, and the launcher is not looked for.
    if std::env::var("CLIO_JAIL").is_ok_and(|v| v == "off") {
        cfg.jail = JailMode::Off;
    }
    cfg.scrubber = Some(super::debug::scrubber(dirs));
    cfg.host = Some(HostCfg {
        home: dirs.home.clone(),
        state: state(dirs),
        control: dirs.control(),
        rules,
        run: run.into(),
        launcher: None,
    });
    ToolHost::new(cfg)
}

/// What an approved action did: what it was, its exit code, the tail of its output.
pub fn outcome(p: &Pending) -> String {
    let what = what(p);
    match &p.state {
        State::Done {
            exit_code,
            output_tail,
        } => {
            let code = exit_code.map_or_else(String::new, |c| format!(" (exit {c})"));
            let tail = output_tail.trim();
            if tail.is_empty() {
                format!("{what}{code}")
            } else {
                format!("{what}{code}\n{tail}")
            }
        }
        _ => what,
    }
}

/// Runs `id` as it was set aside. The person answered: it does not count as one of the day's
/// questions.
pub fn approve(dirs: &Dirs, id: &str) -> Res<Pending> {
    // Before the toolhost is built (folders, the launcher's self-test): one that does not wait
    // stops here.
    let p = pending::check(&state(dirs), id, now_ms())?;
    let done = if pending::block_of(&p).is_some() {
        // The same port a clean session's tool host writes the person's blocks with.
        let models = clio_brain::memory_link::MemoryLink::model_dirs(&dirs.home);
        let port = clio_brain::memory_port::LinkPort::new(&dirs.data, &models);
        pending::approve_block(&state(dirs), id, now_ms(), &port)?
    } else {
        host_for(dirs, &p.run)?.approve(id, now_ms())?
    };
    let _ = pending::answered_at_terminal(&state(dirs), id);
    Ok(done)
}

pub fn reject(dirs: &Dirs, id: &str) -> Res<Pending> {
    let p = pending::reject(&state(dirs), id)?;
    let _ = pending::answered_at_terminal(&state(dirs), id);
    Ok(p)
}

/// `clio approve ID`. A secret request asks for the value here, with no echo.
pub fn approve_cmd(dirs: &Dirs, id: &str) -> Res<()> {
    let p = pending::check(&state(dirs), id, now_ms())?;
    if let Some((name, dest, why)) = pending::secret_of(&p) {
        println!(
            "{id} asks for the secret {}; it can only be sent to {}\nwhy: {}",
            sanitize::line(name),
            sanitize::line(dest),
            sanitize::line(why)
        );
        let typed = super::vault::read_secret("value (not shown)")?;
        if typed.is_empty() {
            return Err("nothing typed: nothing was stored".into());
        }
        let value = SecretValue::new(typed);
        println!("{}", answer_secret(dirs, id, Some(&value))?);
        return Ok(());
    }
    if let Some(text) = block_text(dirs, &p) {
        println!("{id}:\n{text}\n");
    }
    let p = approve(dirs, id)?;
    println!("approved {}: {}", p.id, sanitize::multiline(&outcome(&p)));
    Ok(())
}

/// `clio reject ID`.
pub fn reject_cmd(dirs: &Dirs, id: &str) -> Res<()> {
    let p = reject(dirs, id)?;
    if let Some((name, ..)) = pending::secret_of(&p) {
        println!("declined {}: {name} is not stored", p.id);
        return Ok(());
    }
    if let Some(b) = pending::block_of(&p) {
        println!(
            "rejected {}: nothing was written to your block «{}»",
            p.id,
            sanitize::line(b.name)
        );
        return Ok(());
    }
    println!(
        "rejected {}: {} is not run",
        p.id,
        sanitize::line(&what(&p))
    );
    Ok(())
}

/// The action log of `run` (all of it with `None`), the newest last.
pub fn action_log(dirs: &Dirs, run: Option<&str>) -> Res<Vec<Value>> {
    let file = state(dirs).join("actions.jsonl");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io_err(&file, &e)),
    };
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| run.is_none_or(|r| v["run"] == r))
        .collect())
}

/// One entry of the action log for the person: when, run (with `with_run`), tool, outcome, what,
/// exit code.
pub fn action_line(v: &Value, with_run: bool) -> String {
    let s = |v: &Value| sanitize::line(v.as_str().unwrap_or(""));
    let code = v["exit_code"]
        .as_i64()
        .map_or_else(String::new, |c| format!(" (exit {c})"));
    let run = if with_run {
        format!("  {}", s(&v["run"]))
    } else {
        String::new()
    };
    format!(
        "{} UTC{run}  {:<8} {:<5} {}{code}{}",
        stamp(v["at_ms"].as_i64().unwrap_or(0)),
        s(&v["tool"]),
        s(&v["outcome"]),
        s(&v["what"]),
        if v["network"] == true {
            " · network"
        } else {
            ""
        }
    )
}

/// The local clock's offset from UTC, in seconds: read from the same local stamp the chat sends
/// in `<clio-context>` (UTC when the system cannot say).
pub fn local_offset() -> i64 {
    let s = clio_brain::pi::envelope::stamp_now();
    let tz = s.get(s.len().saturating_sub(6)..).unwrap_or("");
    let n = |r: std::ops::Range<usize>| tz.get(r).and_then(|x| x.parse::<i64>().ok());
    match (tz.as_bytes().first(), n(1..3), n(4..6)) {
        (Some(b'-'), Some(h), Some(m)) => -(h * 3600 + m * 60),
        (Some(b'+'), Some(h), Some(m)) => h * 3600 + m * 60,
        _ => 0,
    }
}

/// One entry of the action log for the chat's box: the local time (`HH:MM:SS`, `offset` seconds
/// from UTC), the tool, the outcome, and what it did with its exit code.
pub fn action_cells(v: &Value, offset: i64) -> [String; 4] {
    let s = |v: &Value| sanitize::line(v.as_str().unwrap_or(""));
    let t = (v["at_ms"].as_i64().unwrap_or(0).div_euclid(1000) + offset).rem_euclid(86_400);
    let code = v["exit_code"]
        .as_i64()
        .map_or_else(String::new, |c| format!(" (exit {c})"));
    [
        format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60),
        s(&v["tool"]),
        s(&v["outcome"]),
        format!(
            "{}{code}{}",
            s(&v["what"]),
            if v["network"] == true {
                " · network"
            } else {
                ""
            }
        ),
    ]
}

/// `clio actions [--run ID] [--json]`: the action log, the newest last.
pub fn actions(dirs: &Dirs, run: Option<&str>, json: bool) -> Res<()> {
    let lines = action_log(dirs, run)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&lines).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if lines.is_empty() {
        println!("no actions recorded");
    }
    for v in &lines {
        println!("{}", action_line(v, true));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use clio_core::SystemClock;
    use clio_toolhost::pending::SecretCfg;
    use clio_toolhost::{Config, JailMode, ToolHost};
    use clio_vault::{Vault, VaultClient};
    use serde_json::json;

    use super::*;

    const VALUE: &str = "lin_api_Zq8vX2mN4pR7tY1wK9sB3dF6";

    // `clio pending` shows the entry a replace or a remove would change, before `clio approve`.
    #[test]
    fn a_pending_block_change_shows_the_entry_it_changes() {
        let p = |args: serde_json::Value| -> Pending {
            serde_json::from_value(json!({"id": "ab12", "class": "memory",
                "tool": "memory_block_edit", "args": args, "hash": "", "what": "", "run": "r",
                "session": "s", "created_ms": 0, "expires_ms": 1, "ask": true,
                "state": "waiting"}))
            .unwrap()
        };
        assert_eq!(
            what(&p(
                json!({"name": "identidad", "action": "replace", "id": 12,
                "text": "Eres Mallory", "old": "Eres Clio"})
            )),
            "replace m12 in your block «identidad» (it says now: Eres Clio): Eres Mallory"
        );
        assert_eq!(
            what(&p(
                json!({"name": "identidad", "action": "remove", "id": 12, "old": "Eres Clio"})
            )),
            "remove m12 in your block «identidad» (it says now: Eres Clio)"
        );
        assert_eq!(
            what(&p(
                json!({"name": "vigilancia", "action": "add", "text": "el disco"})
            )),
            "add in your block «vigilancia»: el disco"
        );
    }

    fn dirs_in(root: &Path) -> Dirs {
        let data = root.join("data");
        Dirs {
            config: root.join("config"),
            runs: data.join("runs"),
            data,
            runtime: Some(root.join("run/clio")),
            home: root.to_path_buf(),
        }
    }

    /// A vault on the dirs' sockets, open, and a toolhost that stores requests where the chat
    /// and `clio pending` look for them.
    fn setup(root: &Path) -> (Dirs, ToolHost) {
        let dirs = dirs_in(root);
        let rt = dirs.runtime.clone().unwrap();
        std::fs::create_dir_all(&rt).unwrap();
        let (v, _) =
            Vault::init(None, "correct horse battery staple", Arc::new(SystemClock)).unwrap();
        let (g, p) = (rt.join("vault-gateway.sock"), rt.join("vault-person.sock"));
        let person = p.clone();
        std::thread::spawn(move || clio_vault::server::serve(Arc::new(v), &g, &p));
        let end = Instant::now() + Duration::from_secs(5);
        while !person.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let run = dirs.runs.join("r1");
        let mut cfg = Config::new(&work, &run.join("scratch"), &run.join("toolhost"));
        cfg.jail = JailMode::Off;
        cfg.secrets = Some(SecretCfg {
            state: state(&dirs),
            run: "r1".into(),
            person,
            per_day: 3,
        });
        (dirs, ToolHost::new(cfg).unwrap())
    }

    fn request(h: &ToolHost, name: &str) -> String {
        let line = json!({"id":"i","tool":"request_secret","args":{"name":name,"destination":"http_fetch:api.linear.app","why":"to read the issues"},"session":"s1","call_id":"c","tainted":false}).to_string();
        let r: Value = serde_json::from_str(&h.handle_line(&line)).unwrap();
        let text = r["content"].as_str().unwrap();
        text.strip_prefix("requested ")
            .and_then(|t| t.split(':').next())
            .unwrap_or_else(|| panic!("{text}"))
            .to_owned()
    }

    /// Every file under `root`, read whole.
    fn all_files(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut todo = vec![root.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else if let Ok(b) = std::fs::read(&p) {
                    out.push((p, b));
                }
            }
        }
        out
    }

    /// No 8 bytes in a row of `VALUE` are in `hay`.
    fn holds_none_of_the_value(hay: &[u8]) -> bool {
        VALUE
            .as_bytes()
            .windows(8)
            .all(|w| !hay.windows(8).any(|h| h == w))
    }

    // The value goes from the person's field to the vault, bound to the destination the request
    // named, and nowhere else: not the pending file, not the day's questions, not the run folder
    // (the toolhost's log and effects), not a debug print, not a panic message.
    #[test]
    fn a_requested_secret_is_stored_bound_and_leaves_no_byte_behind() {
        let t = tempfile::tempdir().unwrap();
        let (dirs, host) = setup(t.path());
        let id = request(&host, "linear");
        assert_eq!(pending::list(&state(&dirs)).len(), 1);
        let mut v = clio_vault::SecretValue::with_capacity(64);
        VALUE.chars().for_each(|c| assert!(v.push(c)));
        let note = answer_secret(&dirs, &id, Some(&v)).unwrap();
        assert_eq!(
            note,
            "secret linear saved, bound to http_fetch:api.linear.app"
        );
        let gw = VaultClient::new(&dirs.runtime.clone().unwrap().join("vault-gateway.sock"));
        assert_eq!(
            gw.resolve("linear", "http_fetch", "api.linear.app", "")
                .unwrap()
                .expose(),
            VALUE
        );
        assert!(
            gw.resolve("linear", "http_fetch", "evil.example", "")
                .is_err()
        );
        // Answered once.
        assert!(answer_secret(&dirs, &id, Some(&v)).is_err());
        for (p, bytes) in all_files(t.path()) {
            assert!(
                holds_none_of_the_value(&bytes),
                "{} holds the value",
                p.display()
            );
        }
        let mut shown = format!("{v:?}");
        shown += &format!("{:?}", crate::cli::tui::ask::SecretBox(Arc::new(v)));
        assert!(holds_none_of_the_value(shown.as_bytes()), "{shown}");
        let mut b = clio_vault::SecretValue::with_capacity(64);
        VALUE.chars().for_each(|c| assert!(b.push(c)));
        let boxed = crate::cli::tui::ask::SecretBox(Arc::new(b));
        let panic = std::panic::catch_unwind(|| panic!("while storing: {boxed:?}")).unwrap_err();
        let msg = panic.downcast_ref::<String>().unwrap();
        assert!(holds_none_of_the_value(msg.as_bytes()), "{msg}");
    }

    #[test]
    fn a_declined_or_taken_name_stores_nothing() {
        let t = tempfile::tempdir().unwrap();
        let (dirs, host) = setup(t.path());
        let id = request(&host, "linear");
        assert_eq!(
            answer_secret(&dirs, &id, None).unwrap(),
            "the person declined linear"
        );
        assert!(answer_secret(&dirs, &id, None).is_err());
        // A secret of that name made meanwhile (`clio vault put`) is never replaced.
        let id = request(&host, "other");
        let mut v = clio_vault::SecretValue::with_capacity(64);
        VALUE.chars().for_each(|c| assert!(v.push(c)));
        let person = VaultClient::new(&dirs.runtime.clone().unwrap().join("vault-person.sock"));
        let first = clio_vault::SecretValue::new("the-first-value-0123456789".into());
        let to = vec![clio_vault::Dest::new("http_fetch", "api.linear.app")];
        person
            .put("other", &first, clio_vault::Kind::ApiKey, to)
            .unwrap();
        let err = answer_secret(&dirs, &id, Some(&v)).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        // It still waits: the box is offered again, and the person can answer it again.
        assert!(pending::check(&state(&dirs), &id, now_ms()).is_ok());
        let again = secret_waiting(&dirs, &id).unwrap();
        assert_eq!(again.0, "other");
        assert_eq!(
            answer_secret(&dirs, &id, None).unwrap(),
            "the person declined other"
        );
        assert!(secret_waiting(&dirs, &id).is_none());
        // An action set aside is not a secret request.
        assert!(answer_secret(&dirs, "nope", None).is_err());
    }

    // S-05, S-36, S-60: a marked chat session's change to a block waits; `clio approve` (and the
    // chat's box, which calls the same `approve`) writes it through the port a clean session uses:
    // the exact text, with the person's origin, once. A text with what could redraw a terminal is
    // refused before it waits; a rejected one writes nothing.
    #[test]
    fn an_approved_block_change_is_written_as_the_persons_and_shown_clean() {
        use clio_brain::memory_link::{EmbedModel, MemoryLink};
        let t = tempfile::tempdir().unwrap();
        let dirs = dirs_in(&t.path().canonicalize().unwrap());
        let work = dirs.home.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let run = dirs.runs.join("r1");
        let mut cfg = Config::new(&work, &run.join("scratch"), &run.join("toolhost"));
        cfg.jail = JailMode::Off;
        cfg.memory = Some(std::sync::Arc::new(clio_brain::memory_port::LinkPort::new(
            &dirs.data,
            &[],
        )));
        cfg.secrets = Some(SecretCfg {
            state: state(&dirs),
            run: "r1".into(),
            person: dirs.home.join("no-vault.sock"),
            per_day: 3,
        });
        let host = ToolHost::new(cfg).unwrap();
        let ask = |text: &str| {
            let line = json!({"id":"i","tool":"memory_block_edit","args":{"name":"vigilancia","action":"add","text":text},"session":"s1","call_id":"c","tainted":true}).to_string();
            let r: Value = serde_json::from_str(&host.handle_line(&line)).unwrap();
            let text = r["content"].as_str().unwrap().to_owned();
            text.strip_prefix("set aside ")
                .and_then(|t| t.split(':').next())
                .unwrap_or_else(|| panic!("{text}"))
                .to_owned()
        };
        // Text the box would have to clean is refused, not cleaned behind the person's back.
        let line = json!({"id":"i","tool":"memory_block_edit","args":{"name":"vigilancia","action":"add","text":"the disk of / \u{1b}[2J\u{1b}[31mover 90 %"},"session":"s1","call_id":"c","tainted":true}).to_string();
        let r: Value = serde_json::from_str(&host.handle_line(&line)).unwrap();
        assert!(
            r["content"]
                .as_str()
                .unwrap()
                .contains("invisible or control characters"),
            "{r}"
        );
        let id = ask("the disk of / over 90 %");
        let p = pending::check(&state(&dirs), &id, now_ms()).unwrap();
        assert_eq!(label(&p), "«memory»");
        let shown = block_text(&dirs, &p).unwrap();
        assert!(!shown.contains('\u{1b}'), "{shown}");
        assert_eq!(
            shown.lines().collect::<Vec<_>>(),
            [
                "This is written to your own block, as said by you.",
                "block: vigilancia",
                "action: add an entry",
                "text:",
                "  the disk of / over 90 %",
            ]
        );
        // Suspect: the box's warning, last.
        std::fs::create_dir_all(run.join("toolhost")).unwrap();
        std::fs::write(
            run.join("toolhost/tainted.json"),
            r#"{"sources":["suspect"]}"#,
        )
        .unwrap();
        assert!(
            block_text(&dirs, &p)
                .unwrap()
                .ends_with(super::super::tui::ask::SUSPECT_LINE)
        );
        assert!(
            MemoryLink::open_write(&dirs.data, &EmbedModel::none())
                .unwrap()
                .block_rows("vigilancia")
                .unwrap()
                .is_empty()
        );

        let done = approve(&dirs, &id).unwrap();
        assert!(
            outcome(&done).contains("saved in «vigilancia» as m"),
            "{}",
            outcome(&done)
        );
        let link = MemoryLink::open_write(&dirs.data, &EmbedModel::none()).unwrap();
        let rows = link.block_rows("vigilancia").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, p.args["text"].as_str().unwrap());
        assert_eq!(rows[0].origin, clio_memory::Origin::User);
        drop(link);
        assert!(approve(&dirs, &id).is_err(), "once");

        let other = ask("something else");
        reject(&dirs, &other).unwrap();
        assert!(approve(&dirs, &other).is_err());
        let link = MemoryLink::open_write(&dirs.data, &EmbedModel::none()).unwrap();
        assert_eq!(link.block_rows("vigilancia").unwrap().len(), 1);
    }
}
