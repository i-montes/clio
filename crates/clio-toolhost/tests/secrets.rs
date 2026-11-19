//! `request_secret`: the toolhost checks a request against the vault and stores it for the person;
//! no value ever goes through it. Answering it is the chat's (or `clio approve`'s) job.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clio_core::SystemClock;
use clio_toolhost::ask::Class;
use clio_toolhost::pending::{self, SecretCfg, State};
use clio_toolhost::{Config, JailMode, Response, ToolHost};
use clio_vault::{Dest, Kind, Scope, SecretSpec, Vault};
use serde_json::{Value, json};
use tempfile::TempDir;

struct Env {
    t: TempDir,
    host: ToolHost,
    state: PathBuf,
    person: PathBuf,
}

/// A toolhost that can ask for secrets; the vault is not started.
fn env() -> Env {
    let t = tempfile::tempdir().unwrap();
    let r = t.path().canonicalize().unwrap();
    let work = r.join("work");
    std::fs::create_dir(&work).unwrap();
    let mut cfg = Config::new(&work, &r.join("scratch"), &r.join("run"));
    cfg.jail = JailMode::Off;
    let (state, person) = (r.join("state"), r.join("p.sock"));
    cfg.secrets = Some(SecretCfg {
        state: state.clone(),
        run: "run1".into(),
        person: person.clone(),
    });
    let host = ToolHost::new(cfg).unwrap();
    Env {
        t,
        host,
        state,
        person,
    }
}

/// The vault, on the env's person socket, with `github` in it.
fn serve(e: &Env, unlocked: bool) {
    let (v, cap) =
        Vault::init(None, "correct horse battery staple", Arc::new(SystemClock)).unwrap();
    v.put(
        &cap,
        SecretSpec {
            name: "github".into(),
            value: "ghp_a-value-long-enough-0123456789".into(),
            kind: Kind::ApiKey,
            destinations: vec![Dest::new("http_fetch", "api.github.com")],
            scope: Scope::Personal,
            rotate_every_ms: None,
            expires_at_ms: None,
        },
    )
    .unwrap();
    if !unlocked {
        v.lock();
    }
    let gw = e.t.path().join("g.sock");
    let p = e.person.clone();
    std::thread::spawn(move || clio_vault::server::serve(Arc::new(v), &gw, &p));
    let end = Instant::now() + Duration::from_secs(5);
    while !e.person.exists() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn ask(e: &Env, session: &str, args: Value) -> Response {
    let line = json!({"id":"i1","tool":"request_secret","args":args,"session":session,"call_id":"c1","tainted":false}).to_string();
    serde_json::from_str(&e.host.handle_line(&line)).unwrap()
}

fn req(name: &str) -> Value {
    json!({"name": name, "destination": "http_fetch:API.example.com", "why": "to list the issues\nof the repo", "reason": "r"})
}

fn id_of(r: &Response) -> String {
    r.content
        .strip_prefix("requested ")
        .and_then(|s| s.split(':').next())
        .unwrap()
        .to_owned()
}

#[test]
fn without_a_place_for_requests_nobody_is_asked() {
    let t = tempfile::tempdir().unwrap();
    let work = t.path().join("w");
    std::fs::create_dir(&work).unwrap();
    let mut cfg = Config::new(&work, &t.path().join("s"), &t.path().join("r"));
    cfg.jail = JailMode::Off;
    let host = ToolHost::new(cfg).unwrap();
    let line = json!({"id":"i","tool":"request_secret","args":req("x"),"session":"s1","call_id":"c","tainted":false}).to_string();
    let r: Response = serde_json::from_str(&host.handle_line(&line)).unwrap();
    assert!(
        !r.ok && r.content.contains("nobody can be asked"),
        "{}",
        r.content
    );
}

#[test]
fn bad_names_destinations_and_reasons_are_refused_before_the_vault_is_asked() {
    let e = env();
    for (args, why) in [
        (
            json!({"name":"Bad Name","destination":"a:b","why":"w"}),
            "`name`",
        ),
        (
            json!({"name":"../x","destination":"a:b","why":"w"}),
            "`name`",
        ),
        (
            json!({"name":"ok","destination":"nocolon","why":"w"}),
            "`destination`",
        ),
        (
            json!({"name":"ok","destination":":b","why":"w"}),
            "`destination`",
        ),
        (
            json!({"name":"ok","destination":"http_fetch:api.example.com","why":" "}),
            "`why`",
        ),
        (
            json!({"name":"ok\u{1b}[2J","destination":"http_fetch:a.example","why":"w"}),
            "`name`",
        ),
    ] {
        let r = ask(&e, "s1", args);
        assert!(!r.ok && r.content.contains(why), "{}", r.content);
    }
    // A secret is bound to the host and everything under it: never a bare suffix, never a
    // host that is not a plain name.
    for dest in [
        "http_fetch:com",
        "http_fetch:.com",
        "http_fetch:example.com.",
        "http_fetch:.example.com",
        "http_fetch:a..example.com",
        "http_fetch:api example.com",
        "http_fetch:api.example.com\u{1b}[1A",
        "http_fetch:api.example.com/path",
        "http_fetch:api.example.com:443",
        "http_fetch:*.example.com",
        "Http_Fetch:api.example.com",
        "http-fetch:api.example.com",
        "http fetch:api.example.com",
        ":api.example.com",
        "http_fetch:",
    ] {
        let r = ask(&e, "s1", json!({"name":"ok","destination":dest,"why":"w"}));
        assert!(
            !r.ok && r.content.contains("`destination`"),
            "{dest:?}: {}",
            r.content
        );
    }
}

#[test]
fn a_stopped_or_locked_vault_and_a_name_it_has_are_refused_at_request_time() {
    let e = env();
    let r = ask(&e, "s1", req("new-one"));
    assert!(!r.ok && r.content.contains("not running"), "{}", r.content);
    serve(&e, false);
    let r = ask(&e, "s1", req("new-one"));
    assert!(!r.ok && r.content.contains("locked"), "{}", r.content);
    let e = env();
    serve(&e, true);
    let r = ask(&e, "s1", req("github"));
    assert!(
        !r.ok
            && r.content.contains("already exists")
            && r.content.contains("clio vault put github"),
        "{}",
        r.content
    );
    assert!(pending::list(&e.state).is_empty());
}

#[test]
fn a_request_waits_for_the_person_once_and_is_told_it_is_answered_later() {
    let e = env();
    serve(&e, true);
    let r = ask(&e, "s1", req("linear"));
    assert!(r.ok, "{}", r.content);
    assert!(
        r.content.ends_with(
            ": the person is asked for it in the chat when this turn ends (a permission window never answers a secret), and their answer reaches you in a later turn; go on with what does not need it"
        ),
        "{}",
        r.content
    );
    let id = id_of(&r);
    // The same request again is the same one.
    assert_eq!(id_of(&ask(&e, "s1", req("linear"))), id);
    let list = pending::list(&e.state);
    assert_eq!(list.len(), 1);
    let p = &list[0];
    assert_eq!(
        (p.class, p.tool.as_str()),
        (Class::Secret, "request_secret")
    );
    assert_eq!(
        pending::secret_of(p),
        Some((
            "linear",
            "http_fetch:api.example.com",
            "to list the issues of the repo"
        ))
    );
    assert_eq!(pending::shown(p), "linear");
    // No daily cap: the fourth request is told the same as the first.
    ask(&e, "s1", req("b"));
    ask(&e, "s1", req("c"));
    let r = ask(&e, "s1", req("d"));
    assert!(
        r.ok && r.content.contains("the person is asked for it"),
        "{}",
        r.content
    );
    assert!(!e.state.join("questions.json").exists());
}

#[test]
fn a_secret_request_is_answered_once_and_never_run_as_an_action() {
    let e = env();
    serve(&e, true);
    let id = id_of(&ask(&e, "s1", req("linear")));
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    // A failed store leaves it waiting.
    let err = pending::answer_secret(&e.state, &id, now, |_| Err("the vault said no".into()));
    assert_eq!(err.unwrap_err(), "the vault said no");
    let mut seen = None;
    let p = pending::answer_secret(&e.state, &id, now, |p| {
        seen = pending::secret_of(p).map(|(n, d, _)| (n.to_owned(), d.to_owned()));
        Ok(())
    })
    .unwrap();
    assert_eq!(p.state, State::Stored);
    assert_eq!(
        seen,
        Some(("linear".to_owned(), "http_fetch:api.example.com".to_owned()))
    );
    assert!(pending::answer_secret(&e.state, &id, now, |_| Ok(())).is_err());
    assert!(pending::reject(&e.state, &id).is_err());
    // Declined: rejected like an action, and nothing is stored.
    let id = id_of(&ask(&e, "s1", req("other")));
    assert_eq!(
        pending::reject(&e.state, &id).unwrap().state,
        State::Rejected
    );
    // Never through `approve`: it is not a command.
    let id = id_of(&ask(&e, "s1", req("third")));
    let t = tempfile::tempdir().unwrap();
    let home = t.path().canonicalize().unwrap().join("home");
    std::fs::create_dir(&home).unwrap();
    let mut cfg = Config::new(&home, &t.path().join("s"), &t.path().join("r"));
    cfg.jail = JailMode::Off;
    cfg.host = Some(clio_toolhost::HostCfg {
        home: home.clone(),
        state: e.state.clone(),
        control: vec![],
        rules: clio_toolhost::ask::HostRules::factory(),
        run: "run1".into(),
        launcher: None,
    });
    let host = ToolHost::new(cfg).unwrap();
    let err = host.approve(&id, now).unwrap_err();
    assert!(err.contains("asks for a secret"), "{err}");
}

#[test]
fn host_json_cannot_make_a_command_a_secret() {
    let err = clio_toolhost::ask::HostRules::factory()
        .with_file(r#"{"ask":[{"class":"secret","pattern":"^curl"}]}"#)
        .err()
        .unwrap();
    assert!(err.contains("secret"), "{err}");
}

#[test]
fn background_commands_exist_only_in_host_mode() {
    let e = env();
    for (tool, args) in [
        ("job_output", json!({"id": "j1"})),
        ("job_stop", json!({"id": "j1"})),
        ("exec", json!({"command": "true", "background": true})),
    ] {
        let line =
            json!({"id":"i","tool":tool,"args":args,"session":"s1","call_id":"c","tainted":false})
                .to_string();
        let r: Response = serde_json::from_str(&e.host.handle_line(&line)).unwrap();
        assert!(!r.ok, "{tool}: {}", r.content);
    }
}
