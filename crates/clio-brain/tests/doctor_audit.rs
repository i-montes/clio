#![allow(clippy::unwrap_used)]
//! `clio doctor` looks at what Clio itself wrote (S-18): nothing shaped like a key may sit in the
//! run folders. The copies of projects are the person's own files, so they are not looked at.

mod common;

use std::sync::Arc;

use std::os::unix::fs::PermissionsExt;

use common::{Env, put};

// Built from two pieces: a scanner reads the source, not the program.
const TOKEN: &str = concat!("ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");

fn doctor(e: &Env) -> (bool, String) {
    let o = e.try_clio(&["doctor"]);
    (
        o.status.success(),
        String::from_utf8_lossy(&[o.stdout, o.stderr].concat()).into_owned(),
    )
}

// S-58
#[test]
fn doctor_flags_a_key_shaped_value_in_what_clio_wrote_and_never_prints_it() {
    let e = Env::new();
    let id = e.run();
    let run = e.sandbox(&id).parent().unwrap().to_path_buf();
    // What an older version left behind: the raw output of a command, in the effect log.
    put(
        &run,
        "toolhost/effects.jsonl",
        &format!("{{\"result\":{{\"output\":\"TOKEN={TOKEN}\"}}}}\n"),
    );
    let (ok, out) = doctor(&e);
    assert!(!ok, "{out}");
    assert!(
        out.contains("effects.jsonl") && out.contains("shaped like a key"),
        "{out}"
    );
    assert!(!out.contains(TOKEN) && !out.contains("ghp_"), "{out}");
}

#[test]
fn doctor_does_not_look_inside_the_copy_of_the_project_nor_the_saved_originals() {
    let e = Env::new();
    put(
        &e.proj(),
        "deploy.sh",
        &format!("export GITHUB_TOKEN={TOKEN}\n"),
    );
    let id = e.run();
    let run = e.sandbox(&id).parent().unwrap().to_path_buf();
    assert!(
        std::fs::read_to_string(e.sandbox(&id).join("deploy.sh"))
            .unwrap()
            .contains(TOKEN)
    );
    put(
        &run,
        "backup/deploy.sh",
        &format!("export GITHUB_TOKEN={TOKEN}\n"),
    );
    // As `apply` leaves them: private, so the permission check has nothing to say.
    for (rel, mode) in [("backup", 0o700), ("backup/deploy.sh", 0o600)] {
        std::fs::set_permissions(run.join(rel), std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let (ok, out) = doctor(&e);
    assert!(ok, "{out}");
    assert!(
        out.contains("files checked"),
        "the check says it ran: {out}"
    );
}

#[test]
fn doctor_says_what_pis_jail_is_on_this_machine() {
    let e = Env::new();
    let (_, out) = doctor(&e);
    let line = out
        .lines()
        .find(|l| l.contains("Pi jail"))
        .unwrap_or_default();
    // Linux with bwrap: ready. Linux without it: Pi will not start, and doctor says so (FAIL).
    // Elsewhere: a warning. Never silent.
    assert!(
        line.starts_with("PASS") || line.starts_with("FAIL") || line.starts_with("WARN"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------------------------
// The vault's own values (S-18, S-58): `doctor` asks the running vault whether any value it keeps
// is in what Clio wrote. The key patterns cannot know a value the vault holds that looks like
// nothing in particular.
// ---------------------------------------------------------------------------------------------

use clio_core::SystemClock;
use clio_vault::{Dest, Kind, Scope, SecretSpec, Vault};

/// A value no key pattern would recognise: only the vault knows it.
const VAULT_VALUE: &str = "violeta-azul-7731-clio";

/// The vault, serving the person's and the gateway's sockets where `clio doctor` looks for them
/// (`$XDG_RUNTIME_DIR/clio/`), in this process. `unlocked` says whether it knows its values.
fn serve_vault(e: &Env, unlocked: bool) {
    let dir = e.tmp.path().join("run/clio");
    let (v, cap) =
        Vault::init(None, "correct horse battery staple", Arc::new(SystemClock)).unwrap();
    v.put(
        &cap,
        SecretSpec {
            name: "notion".into(),
            value: VAULT_VALUE.into(),
            kind: Kind::ApiKey,
            destinations: vec![Dest::new("notion-api", "api.notion.com")],
            scope: Scope::Personal,
            rotate_every_ms: None,
            expires_at_ms: None,
        },
    )
    .unwrap();
    if !unlocked {
        v.lock();
    }
    let (gw, person) = (
        dir.join("vault-gateway.sock"),
        dir.join("vault-person.sock"),
    );
    let (g, p) = (gw.clone(), person.clone());
    std::thread::spawn(move || clio_vault::server::serve(Arc::new(v), &g, &p));
    let t = std::time::Instant::now();
    while !(gw.exists() && person.exists()) {
        assert!(t.elapsed().as_secs() < 10, "the vault never came up");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn vault_line(out: &str) -> String {
    out.lines()
        .find(|l| l.contains("vault:"))
        .unwrap_or_default()
        .to_owned()
}

// S-18, S-58
#[test]
fn doctor_finds_a_value_of_the_vault_in_what_clio_wrote_and_never_prints_it() {
    let e = Env::new();
    let id = e.run();
    let run = e.sandbox(&id).parent().unwrap().to_path_buf();
    put(
        &run,
        "transcript.json",
        &format!("{{\"said\":\"the key is {VAULT_VALUE}\"}}\n"),
    );
    // The copy of a project may hold the person's own values: not Clio's output, not looked at.
    put(
        &e.sandbox(&id),
        "notes.txt",
        &format!("mine: {VAULT_VALUE}\n"),
    );
    serve_vault(&e, true);
    let (ok, out) = doctor(&e);
    let line = vault_line(&out);
    assert!(!ok && line.starts_with("FAIL"), "{out}");
    assert!(
        line.contains("transcript.json") && line.contains("notion"),
        "{line}"
    );
    assert!(
        !line.contains("notes.txt"),
        "the copy of the project is not scanned: {line}"
    );
    assert!(
        !out.contains(VAULT_VALUE),
        "the value is never printed: {out}"
    );
}

// S-18: the vault scans what Clio wrote, leaving out only Chromium and npm's store, a server's
// or a service's own data folder and the copies, each at its place from the data root: the same
// names deeper (a skill's files) are read.
#[test]
fn the_vault_scan_leaves_out_only_the_places_at_the_data_root() {
    let e = Env::new();
    let data = e.tmp.path().join("home/.local/share/clio");
    let read = [
        "skills/s/browsers/x",
        "skills/s/node/x",
        "mcp/servers/n/x",
        "mcp/npm-cache/_cacache/index-v5/x",
    ];
    let left = [
        "browsers/x",
        "mcp/data/n/x",
        "mcp/npm-cache/_cacache/content-v2/x",
        "services/state/s/x",
        "node/x",
        "runs/r1/sandbox/x",
    ];
    for f in read.iter().chain(&left) {
        put(&data, f, &format!("v={VAULT_VALUE}\n"));
    }
    serve_vault(&e, true);
    let (_, out) = doctor(&e);
    let line = vault_line(&out);
    assert!(line.starts_with("FAIL"), "{out}");
    let said = |f: &str| line.contains(&format!(" {f} (")) || line.contains(&format!(": {f} ("));
    for f in read {
        assert!(said(f), "{f} was not scanned: {line}");
    }
    for f in left {
        assert!(!said(f), "{f} was scanned: {line}");
    }
}

// S-18
#[test]
fn doctor_says_the_vault_is_clean_when_none_of_its_values_was_written() {
    let e = Env::new();
    let _ = e.run();
    serve_vault(&e, true);
    let (ok, out) = doctor(&e);
    assert!(ok, "{out}");
    assert!(vault_line(&out).starts_with("PASS"), "{out}");
}

// S-18: a locked vault does not know its values, so it cannot say they are absent.
#[test]
fn doctor_warns_when_the_vault_is_locked_and_when_it_is_not_running() {
    let e = Env::new();
    put(
        &e.tmp.path().join("home/.local/share/clio"),
        "vault.bin",
        "x",
    );
    // Not running, but there is a vault file: its values were not checked.
    let (_, out) = doctor(&e);
    assert!(
        vault_line(&out).starts_with("WARN") && vault_line(&out).contains("not running"),
        "{out}"
    );
    // Running and locked.
    serve_vault(&e, false);
    let (_, out) = doctor(&e);
    assert!(
        vault_line(&out).starts_with("WARN") && vault_line(&out).contains("locked"),
        "{out}"
    );
}

// No vault file at all: nothing to check, and it is optional.
#[test]
fn doctor_does_not_complain_when_there_is_no_vault() {
    let e = Env::new();
    let (_, out) = doctor(&e);
    assert!(
        vault_line(&out).starts_with("PASS") && vault_line(&out).contains("there is none"),
        "{out}"
    );
}

// Each check's line comes out when that check is made, not all at the end: read through a pipe
// while the machine check (the last one) is held, the lines before it are already there.
#[test]
fn doctor_prints_each_check_as_it_is_made() {
    use std::io::BufRead;
    let e = Env::new();
    let go = e.tmp.path().join("go");
    let script = e.tmp.path().join("script/held.sh");
    // Held until the test says go (or a minute, so a broken doctor fails instead of hanging).
    std::fs::write(
        &script,
        format!(
            "i=0; while [ ! -e '{}' ] && [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done\necho 'PASS the machine was held'\n",
            go.display()
        ),
    )
    .unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_clio"))
        .arg("doctor")
        .env("HOME", e.tmp.path().join("home"))
        .env("XDG_RUNTIME_DIR", e.tmp.path().join("run"))
        .env("CLIO_SKIP_PI_INSTALL", "1")
        .env("CLIO_HOST_CHECK", &script)
        .env_remove("OPENCODE_API_KEY")
        .current_dir(e.tmp.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let mut before = Vec::new();
    for l in lines.by_ref() {
        let l = l.unwrap();
        let last = l.contains("Pi jail");
        before.push(l);
        if last {
            break;
        }
    }
    // The machine check has not ended, and the lines before it were printed.
    assert!(child.try_wait().unwrap().is_none(), "{before:?}");
    assert!(
        before.iter().any(|l| l.contains("files checked")),
        "{before:?}"
    );
    // A pipe gets plain lines only: nothing that says what is being checked.
    assert!(before.iter().all(|l| !l.contains("checking")), "{before:?}");
    std::fs::write(&go, "").unwrap();
    let after: Vec<String> = lines.map(Result::unwrap).collect();
    assert!(child.wait().unwrap().code().is_some());
    assert!(
        after.iter().any(|l| l.contains("the machine was held"))
            && after.last().unwrap().starts_with("doctor:"),
        "{after:?}"
    );
}
