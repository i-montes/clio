#![allow(clippy::unwrap_used)]
//! The REAL Pi (from `adapters/pi/node_modules`) against the fake model server of
//! `testing/fake-provider/`: a whole turn over RPC, the hardened launch, the environment of the
//! process, fail-closed checks and the API key staying out of every file. No network. Needs Node
//! and `cd adapters/pi && npm ci --ignore-scripts` (skipped with a message otherwise;
//! `CLIO_REQUIRE_PI=1` turns the skip into a failure).

mod pi_support;

use std::collections::BTreeSet;
use std::path::Path;

use clio_brain::pi::PiError;
use clio_brain::pi::launch::{PiProcess, SessionTarget, start};
use clio_brain::pi::rpc::{Cancel, Event, RunSummary};
use clio_brain::pi::sessions::list_sessions;
use pi_support::{FakeProvider, Rig, need};

/// Low-entropy decoy, built at run time.
fn decoy() -> String {
    format!("decoy-{}-{}", "key", "1111")
}

fn run(p: &PiProcess, msg: &str) -> (Result<RunSummary, PiError>, Vec<Event>) {
    let mut evs = Vec::new();
    let r = p
        .client
        .run_prompt(msg, &Cancel::new(), &mut |e| evs.push(e.clone()));
    (r, evs)
}

fn all_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            all_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

const TWO_TURNS: &str = r#"[{"text":"hello from the fake model","usage":[100,5]},{"text":"second answer","usage":[200,7]}]"#;

#[test]
fn a_whole_turn_over_rpc_saves_a_session_and_it_can_be_reopened() {
    let Some(node) = need("a_whole_turn", true) else {
        return;
    };
    let rig = Rig::new(node);
    let prov = FakeProvider::start(rig.tmp.path(), TWO_TURNS, None);
    let launch = rig.launch(&prov.base_url, Some(&decoy()));
    let p = start(&launch).unwrap();
    assert_eq!(p.state.session_id, "sess-test-1");

    let (r, evs) = run(&p, "say hi");
    let r = r.unwrap();
    assert_eq!(r.text, "hello from the fake model");
    assert_eq!((r.usage.input, r.usage.output), (100, 5));
    assert!(evs.iter().any(|e| matches!(e, Event::AgentStart)));
    assert!(evs.iter().any(|e| matches!(e, Event::TurnEnd)));
    assert!(evs.iter().any(
        |e| matches!(e, Event::MessageUpdate { kind, delta: Some(_), .. } if kind == "text_delta")
    ));
    assert!(matches!(
        evs.last(),
        Some(Event::AgentSettled { aborted: false })
    ));
    let stats = p.client.get_session_stats().unwrap();
    assert_eq!((stats.usage.input, stats.usage.output), (100, 5));
    assert_eq!(stats.context.unwrap().window, 128_000);

    // Pi saved a .jsonl and Clio lists it, with the first thing the person said as the title.
    let list = list_sessions(&rig.paths.sessions_dir());
    assert!(list.warnings.is_empty(), "{:?}", list.warnings);
    assert_eq!(list.sessions.len(), 1);
    let s = &list.sessions[0];
    assert_eq!((s.id.as_str(), s.title.as_str()), ("sess-test-1", "say hi"));
    assert_eq!(
        s.cwd_path.as_deref(),
        Some(rig.proj.canonicalize().unwrap().as_path())
    );
    assert!(s.messages >= 2 && s.created_ms.is_some());

    // Rename through Pi; the listing shows the name.
    p.client.set_session_name("my work\u{1b}[31m").unwrap();
    drop(p);
    let list = list_sessions(&rig.paths.sessions_dir());
    assert_eq!(list.sessions[0].name.as_deref(), Some("my work"));

    // Reopen it: the history is there and the model sees it.
    let mut again = launch.clone();
    again.session = SessionTarget::Open(list.sessions[0].path.display().to_string());
    let p2 = start(&again).unwrap();
    assert_eq!(p2.session_id, "sess-test-1");
    assert!(p2.client.get_messages().unwrap().len() >= 2);
    let r2 = run(&p2, "and again").0.unwrap();
    assert_eq!(r2.text, "second answer");
    let reqs = prov.requests();
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs[1]["roles"].as_array().unwrap().len() > reqs[0]["roles"].as_array().unwrap().len()
    );
    // Fork from the first user message into a new session.
    let forks = p2.client.get_fork_messages().unwrap();
    let entry = forks[0]["entryId"].as_str().unwrap().to_owned();
    assert_eq!(p2.client.fork(&entry).unwrap().as_deref(), Some("say hi"));
    // The fork is a different session (Pi writes its file with the first new message).
    let st = p2.client.get_state().unwrap();
    assert_ne!(st.session_id, "sess-test-1");
    drop(p2);
}

#[test]
fn the_hardened_launch_has_no_tools_no_extensions_and_a_clean_environment() {
    let Some(node) = need("the_hardened_launch", true) else {
        return;
    };
    let rig = Rig::new(node);
    let key = decoy();
    let prov = FakeProvider::start(rig.tmp.path(), TWO_TURNS, Some(&key));
    let launch = rig.launch(&prov.base_url, Some(&key));
    let p = start(&launch).unwrap();
    run(&p, "hi").0.unwrap();

    // The process: environment, command line, working folder.
    let pid = p.client.pid();
    let env_raw = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    let env: Vec<(String, String)> = env_raw
        .split(|b| *b == 0)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let s = String::from_utf8_lossy(l).into_owned();
            let (k, v) = s.split_once('=').unwrap();
            (k.to_owned(), v.to_owned())
        })
        .collect();
    let names: BTreeSet<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    // Pi adds its own markers (AI_AGENT, PI_CODING_AGENT) to the environment it runs with.
    let expected: BTreeSet<&str> = [
        "PATH",
        "HOME",
        "PI_CODING_AGENT_DIR",
        "PI_CODING_AGENT_SESSION_DIR",
        "PI_OFFLINE",
        "PI_TELEMETRY",
        "PI_SKIP_VERSION_CHECK",
        "NO_COLOR",
        "CLIO_SESSION_ID",
        "OPENCODE_API_KEY",
        "AI_AGENT",
        "PI_CODING_AGENT",
    ]
    .into();
    assert!(
        names.is_subset(&expected),
        "unexpected variables: {:?}",
        names.difference(&expected).collect::<Vec<_>>()
    );
    for bad in [
        "SSH_AUTH_SOCK",
        "XDG_RUNTIME_DIR",
        "DISPLAY",
        "AWS_ACCESS_KEY_ID",
        "ANTHROPIC_API_KEY",
        "USER",
    ] {
        assert!(!names.contains(bad), "{bad} leaked");
    }
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(
        get("HOME").unwrap(),
        rig.paths.home_dir().display().to_string()
    );
    assert_ne!(Some(get("HOME").unwrap()), std::env::var("HOME").ok());
    assert_eq!(
        get("PI_CODING_AGENT_DIR").unwrap(),
        rig.paths.agent_dir().display().to_string()
    );
    assert_eq!(get("CLIO_SESSION_ID").unwrap(), "sess-test-1");
    let cmdline = String::from_utf8_lossy(&std::fs::read(format!("/proc/{pid}/cmdline")).unwrap())
        .replace('\0', " ");
    assert!(!cmdline.contains(&key), "the key is on the command line");
    // (Pi rewrites its process title, so the flags are not visible here: `hardened_args` is pinned
    // by a unit test, and the behavior below proves they took effect.)
    assert_eq!(
        std::fs::read_link(format!("/proc/{pid}/cwd")).unwrap(),
        rig.proj.canonicalize().unwrap()
    );

    // The model was offered no tools; the key and the session header arrived through the env.
    let reqs = prov.requests();
    assert_eq!(
        reqs[0]["tools"].as_array().unwrap().len(),
        0,
        "{:?}",
        reqs[0]["tools"]
    );
    assert_eq!(reqs[0]["headers"]["authorization_matches"], true);
    assert_eq!(reqs[0]["headers"]["x-opencode-session"], "sess-test-1");
    assert_eq!(reqs[0]["headers"]["user-agent"], "clio/0.1");
    assert!(p.client.get_commands().unwrap().is_empty());
    assert!(
        p.client
            .get_messages()
            .unwrap()
            .iter()
            .all(|m| m["toolsAdded"].as_array().is_none_or(Vec::is_empty))
    );

    // Everything Pi's tree holds is 0700 / 0600.
    let mut files = Vec::new();
    all_files(&rig.paths.root, &mut files);
    for f in &files {
        let m = std::fs::metadata(f).unwrap();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o077,
            0,
            "{}",
            f.display()
        );
    }
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&rig.paths.root).unwrap().permissions()
        ) & 0o777,
        0o700
    );
}

#[test]
fn the_api_key_is_in_no_file_under_the_clio_tree() {
    let Some(node) = need("the_api_key_is_in_no_file", true) else {
        return;
    };
    let rig = Rig::new(node);
    let key = decoy();
    let prov = FakeProvider::start(rig.tmp.path(), TWO_TURNS, Some(&key));
    let p = start(&rig.launch(&prov.base_url, Some(&key))).unwrap();
    run(&p, "hi").0.unwrap();
    p.client.set_session_name("n").unwrap();
    drop(p);
    let mut files = Vec::new();
    all_files(&rig.paths.root, &mut files);
    assert!(
        files.iter().any(|f| f.ends_with("models.json"))
            && files
                .iter()
                .any(|f| f.extension().is_some_and(|e| e == "jsonl"))
    );
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        assert!(
            !bytes.windows(key.len()).any(|w| w == key.as_bytes()),
            "the key is in {}",
            f.display()
        );
    }
    let models = std::fs::read_to_string(rig.paths.agent_dir().join("models.json")).unwrap();
    assert!(models.contains("${OPENCODE_API_KEY}"));
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(rig.paths.agent_dir().join("models.json"))
                .unwrap()
                .permissions()
        ) & 0o777,
        0o600
    );
}

/// A guard stand-in: registers tools named like Pi's built-ins and a command.
const GUARD: &str = r#"export default function (pi) {
  for (const name of ["read", "probe"]) {
    pi.registerTool({ name, label: name, description: "test tool " + name,
      parameters: { type: "object", properties: {} },
      async execute() { return { content: [{ type: "text", text: "ok" }], details: {} }; } });
  }
  pi.registerCommand("guard-test", { description: "t", handler: async () => {} });
}
"#;

#[test]
fn only_the_guards_tools_are_active_and_a_guard_that_does_not_load_fails_closed() {
    let Some(node) = need("only_the_guards_tools", true) else {
        return;
    };
    let rig = Rig::new(node);
    let prov = FakeProvider::start(rig.tmp.path(), r#"[{"text":"ok"}]"#, None);
    let guard = rig.tmp.path().join("guard.ts");
    std::fs::write(&guard, GUARD).unwrap();

    // 1. The guard loads: with --no-builtin-tools its tools (even one named `read`) are the only ones.
    let mut l = rig.launch(&prov.base_url, Some(&decoy()));
    l.guard = Some(guard.clone());
    l.guard_tools = vec!["read".into(), "probe".into()];
    let p = start(&l).unwrap();
    run(&p, "hi").0.unwrap();
    let declared: BTreeSet<String> = prov.requests()[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        declared,
        BTreeSet::from(["read".to_owned(), "probe".to_owned()])
    );
    drop(p);

    // 2. The same expectation, but the extension is missing a tool: the first system message
    // does not match and the process is killed.
    let mut l = rig.launch(&prov.base_url, Some(&decoy()));
    l.session = SessionTarget::New {
        id: Some("sess-test-2".into()),
    };
    l.guard = Some(guard.clone());
    l.guard_tools = vec!["read".into(), "probe".into(), "bash".into()];
    let p = start(&l).unwrap();
    let e = run(&p, "hi").0.unwrap_err();
    assert!(matches!(e, PiError::Verification(_)), "{e}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while p.client.is_alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!p.client.is_alive());
    drop(p);

    // 3. A guard path that does not exist never starts Pi.
    let mut l = rig.launch(&prov.base_url, Some(&decoy()));
    l.guard = Some(rig.tmp.path().join("missing.ts"));
    assert!(matches!(start(&l), Err(PiError::NotInstalled(_))));

    // 4. A session that declares tools outside the list is refused when reopened.
    let mut l = rig.launch(&prov.base_url, Some(&decoy()));
    l.guard = Some(guard);
    l.guard_tools = vec!["read".into(), "probe".into()];
    l.session = SessionTarget::Open(
        list_sessions(&rig.paths.sessions_dir())
            .sessions
            .iter()
            .find(|s| s.id == "sess-test-1")
            .unwrap()
            .path
            .display()
            .to_string(),
    );
    assert!(start(&l).is_ok());
    l.guard_tools = vec!["read".into()];
    assert!(matches!(start(&l), Err(PiError::Verification(_))));
}

#[test]
fn a_model_that_asks_for_a_tool_pi_does_not_have_gets_an_error_and_nothing_runs() {
    let Some(node) = need("a_model_that_asks_for_a_tool", true) else {
        return;
    };
    let rig = Rig::new(node);
    let script = r#"[{"tool_calls":[{"name":"bash","arguments":{"command":"touch /tmp/clio-pi-should-not-exist"}}]},{"text":"gave up"}]"#;
    let prov = FakeProvider::start(rig.tmp.path(), script, None);
    let p = start(&rig.launch(&prov.base_url, Some(&decoy()))).unwrap();
    let (r, evs) = run(&p, "do it");
    // Either Pi answers the unknown tool with an error and the run finishes, or the client's
    // allowed-list killed it: both are safe. What must hold is that nothing ran.
    match r {
        Ok(s) => assert!(s.tool_calls <= 1, "{s:?}"),
        Err(PiError::Verification(_)) => {}
        Err(e) => panic!("{e}; events: {evs:?}"),
    }
    assert!(!Path::new("/tmp/clio-pi-should-not-exist").exists());
}
