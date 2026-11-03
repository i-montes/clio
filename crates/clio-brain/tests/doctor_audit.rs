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
        out.contains("effects.jsonl") && out.contains("forma de llave"),
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
    assert!(out.contains("revisados"), "the check says it ran: {out}");
}

#[test]
fn doctor_says_what_pis_jail_is_on_this_machine() {
    let e = Env::new();
    let (_, out) = doctor(&e);
    let line = out
        .lines()
        .find(|l| l.contains("jaula de Pi"))
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
        .find(|l| l.contains("baúl:"))
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
        vault_line(&out).starts_with("WARN") && vault_line(&out).contains("no está corriendo"),
        "{out}"
    );
    // Running and locked.
    serve_vault(&e, false);
    let (_, out) = doctor(&e);
    assert!(
        vault_line(&out).starts_with("WARN") && vault_line(&out).contains("bloqueado"),
        "{out}"
    );
}

// No vault file at all: nothing to check, and it is optional.
#[test]
fn doctor_does_not_complain_when_there_is_no_vault() {
    let e = Env::new();
    let (_, out) = doctor(&e);
    assert!(
        vault_line(&out).starts_with("PASS") && vault_line(&out).contains("no hay"),
        "{out}"
    );
}
