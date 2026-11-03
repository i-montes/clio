#![allow(clippy::unwrap_used)]
//! `clio doctor` looks at what Clio itself wrote (S-18): nothing shaped like a key may sit in the
//! run folders. The copies of projects are the person's own files, so they are not looked at.

mod common;

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
