#![allow(clippy::unwrap_used)]
//! The RPC client against a fake Pi (`tests/pi_support/fake_pi.js`): framing, ids, tolerance,
//! deadlines, cancel, `Drop`, sanitized stderr. No network, no real Pi; needs Node (skipped with
//! a message without it, a failure with `CLIO_REQUIRE_PI=1`).

mod pi_support;

use std::process::Command;
use std::time::{Duration, Instant};

use clio_brain::pi::PiError;
use clio_brain::pi::rpc::{Cancel, Event, RpcClient, Timeouts, UsageMeter};
use pi_support::{need, pid_alive};

fn fake(pidfile: Option<&std::path::Path>) -> RpcClient {
    let node = pi_support::node().unwrap();
    let js = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pi_support/fake_pi.js");
    let mut cmd = Command::new(&node.path);
    cmd.arg(js)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", node.dir().display()));
    if let Some(p) = pidfile {
        cmd.env("FAKE_PI_PIDFILE", p);
    }
    RpcClient::spawn(&mut cmd).unwrap()
}

fn run(c: &RpcClient, msg: &str) -> (Result<clio_brain::pi::rpc::RunSummary, PiError>, Vec<Event>) {
    let mut evs = Vec::new();
    let r = c.run_prompt(msg, &Cancel::new(), &mut |e| evs.push(e.clone()));
    (r, evs)
}

#[test]
fn a_run_streams_events_and_survives_unknown_ones_and_u2028() {
    if need("a_run_streams_events", false).is_none() {
        return;
    }
    let c = fake(None);
    let (r, evs) = run(&c, "ok");
    let r = r.unwrap();
    // U+2028 is inside the text, not a record boundary.
    assert_eq!(r.text, "hel\u{2028}lo world");
    assert_eq!((r.usage.input, r.usage.output, r.tool_calls), (100, 5, 1));
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::Unknown { kind, .. } if kind == "future_event_v9"))
    );
    assert!(
        evs.iter().any(
            |e| matches!(e, Event::ToolEnd { name, duration_ms: Some(4), .. } if name == "read")
        )
    );
    assert!(evs.iter().any(
        |e| matches!(e, Event::MessageUpdate { delta: Some(d), .. } if d.contains('\u{2028}'))
    ));
    assert!(matches!(
        evs.last(),
        Some(Event::AgentSettled { aborted: false })
    ));
    // The extension dialog was answered "cancelled" by the client.
    let ui = evs.iter().find_map(|e| match e {
        Event::ExtensionUiRequest { id, .. } => Some(id.clone()),
        _ => None,
    });
    assert_eq!(ui.as_deref(), Some("ui-1"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !c.stderr_text().contains("ui-cancelled:ui-1:true") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        c.stderr_text().contains("ui-cancelled:ui-1:true"),
        "{}",
        c.stderr_text()
    );
    // The meter follows the same events.
    let mut m = UsageMeter::default();
    evs.iter().for_each(|e| m.observe(e));
    assert_eq!((m.total().input, m.total().output), (100, 5));
    m.refresh(&c.get_session_stats().unwrap());
    assert_eq!(m.context.unwrap().window, 1000);
}

#[test]
fn split_responses_and_concurrent_ids_correlate() {
    if need("split_responses", false).is_none() {
        return;
    }
    let c = fake(None);
    std::thread::scope(|s| {
        let a = s.spawn(|| c.get_state().unwrap());
        let b = s.spawn(|| c.get_session_stats().unwrap());
        let st = a.join().unwrap();
        assert_eq!((st.session_id.as_str(), st.model.as_str()), ("s1", "m"));
        assert_eq!(b.join().unwrap().usage.total_tokens, 105);
    });
    assert_eq!(c.get_messages().unwrap().len(), 1);
    assert!(c.new_session(None).unwrap());
    assert_eq!(c.fork("e1").unwrap().as_deref(), Some("orig"));
    c.set_session_name("n").unwrap();
    assert!(c.compact(None).unwrap()["summary"] == "s");
}

#[test]
fn a_failed_command_is_a_clean_error() {
    if need("a_failed_command", false).is_none() {
        return;
    }
    let c = fake(None);
    let e = run(&c, "fail").0.unwrap_err();
    let msg = e.to_string();
    assert!(
        matches!(e, PiError::Command { .. }) && msg.contains("rejected") && !msg.contains('\u{1b}'),
        "{msg}"
    );
    // The process is still usable after a rejected command.
    assert_eq!(c.get_state().unwrap().session_id, "s1");
}

#[test]
fn every_wait_has_a_deadline() {
    if need("every_wait_has_a_deadline", false).is_none() {
        return;
    }
    let mut c = fake(None);
    c.timeouts = Timeouts {
        request: Duration::from_millis(300),
        silence: Duration::from_millis(400),
        run: Duration::from_secs(30),
        grace: Duration::from_millis(300),
    };
    let t = Instant::now();
    let e = c
        .request("never_answered", serde_json::json!({}))
        .unwrap_err();
    assert!(matches!(e, PiError::Timeout { .. }), "{e}");
    let e = run(&c, "slow").0.unwrap_err();
    assert!(matches!(e, PiError::Timeout { .. }), "{e}");
    assert!(t.elapsed() < Duration::from_secs(4));
}

#[test]
fn cancel_sends_abort_and_the_run_ends_aborted() {
    if need("cancel", false).is_none() {
        return;
    }
    let c = fake(None);
    let cancel = Cancel::new();
    let r = std::thread::scope(|s| {
        let k = cancel.clone();
        s.spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            k.cancel();
        });
        c.run_prompt("hang", &cancel, &mut |_| {})
    });
    assert!(r.unwrap().aborted);
}

#[test]
fn a_crash_reports_sanitized_stderr() {
    if need("a_crash", false).is_none() {
        return;
    }
    let c = fake(None);
    let e = run(&c, "crash").0.unwrap_err();
    let PiError::Exited { stderr } = e else {
        panic!("{e}")
    };
    assert_eq!(stderr, "boom red  end");
}

#[test]
fn stderr_is_capped() {
    if need("stderr_is_capped", false).is_none() {
        return;
    }
    let c = fake(None);
    run(&c, "flood").0.unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let t = c.stderr_text();
    assert!(!t.is_empty() && t.chars().count() <= 801, "{}", t.len());
}

#[test]
fn a_tool_outside_the_list_kills_the_process() {
    if need("a_tool_outside_the_list", false).is_none() {
        return;
    }
    let c = fake(None);
    c.set_allowed_tools(vec!["read".into()], false);
    let e = run(&c, "evil").0.unwrap_err();
    assert!(matches!(e, PiError::Verification(_)), "{e}");
    let deadline = Instant::now() + Duration::from_secs(3);
    while c.is_alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!c.is_alive());
}

#[test]
fn drop_kills_the_process_and_its_group() {
    if need("drop_kills", false).is_none() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let pf = tmp.path().join("pids");
    let c = fake(Some(&pf));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pf.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    c.get_state().unwrap();
    let pids: Vec<u32> = std::fs::read_to_string(&pf)
        .unwrap()
        .split_whitespace()
        .map(|p| p.parse().unwrap())
        .collect();
    assert_eq!(pids.len(), 2);
    assert!(pids.iter().all(|p| pid_alive(*p)));
    drop(c);
    let deadline = Instant::now() + Duration::from_secs(3);
    while pids.iter().any(|p| pid_alive(*p)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        pids.iter().all(|p| !pid_alive(*p)),
        "something survived the Drop"
    );
}
