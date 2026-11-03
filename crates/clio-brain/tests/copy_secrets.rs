#![allow(clippy::unwrap_used)]
//! Security audit of 2026-11-02 (finding 1, attack-52): what `clio run` copies for the agent, and
//! what happens to a file that carries a value the filter masked.

mod common;

use clio_attacks::{Case, Observed, Status, Target, load_cases, run};
use common::{Env, put};

// S-18, S-44: files that exist to hold a credential never enter the copy, whatever tool wrote
// them. The list today stops at `.env*`, `*.pem`, `*.key`, `id_rsa*`, `id_ed25519*`, `.ssh`,
// `.aws` and `credentials*`.
#[test]
fn credential_files_are_not_copied_for_the_agent() {
    let e = Env::new();
    let p = e.proj();
    put(&p, "app.py", "print(1)\n");
    for rel in [
        ".npmrc",
        ".netrc",
        ".pgpass",
        ".git-credentials",
        "id_ecdsa",
        "id_dsa",
        "deploy.p12",
        "deploy.pfx",
        "vault.kdbx",
        "infra/terraform.tfstate",
        ".kube/config",
        ".docker/config.json",
        ".gnupg/pubring.kbx",
    ] {
        put(&p, rel, "not a real credential\n");
    }
    assert_eq!(e.copied(), ["app.py"], "only the plain file may be copied");
}

// S-18, S-36: a value the filter masked is not the file's content. A copy of the masked text
// must never travel back into the person's project through `apply` and replace the real value.
#[test]
fn apply_refuses_a_file_that_carries_a_masked_value() {
    let e = Env::new();
    put(&e.proj(), "notes.md", "hello\n");
    let id = e.run();
    let masked = "[secreto:patrón tachado: parece una llave y no está en el baúl; no la reescribas, pídesela a la persona]";
    put(&e.sandbox(&id), "notes.md", &format!("key = {masked}\n"));
    let o = e.try_clio(&["apply", "--yes", "--force", &id]);
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(!o.status.success() && err.contains("tachado"), "{err}");
    assert_eq!(
        std::fs::read_to_string(e.proj().join("notes.md")).unwrap(),
        "hello\n"
    );
}

// ---------------------------------------------------------------------------------------------
// The attack cases that live in this file, through clio-attacks (the gate of the battery)
// ---------------------------------------------------------------------------------------------

/// A scenario holds when it does not panic: the plain tests above are the scenarios, with their
/// own messages.
fn held(f: impl FnOnce() + std::panic::UnwindSafe) -> bool {
    std::panic::catch_unwind(f).is_ok()
}

const OURS: [&str; 1] = ["attack-52"];

struct CopyTarget;

impl Target for CopyTarget {
    fn attack(&self, case: &Case) -> Observed {
        let refused = match case.id.as_str() {
            "attack-52" => held(|| {
                credential_files_are_not_copied_for_the_agent();
                apply_refuses_a_file_that_carries_a_masked_value();
            }),
            other => panic!("{other} is active but this target has no scenario for it"),
        };
        Observed { refused }
    }
}

// S-18 S-44: the copy half of attack 52 (the host half is in clio-toolhost/tests/hardening.rs)
#[test]
fn the_copy_case_is_active_and_refused() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testing/attacks/cases");
    let mine: Vec<Case> = load_cases(&dir)
        .unwrap()
        .into_iter()
        .filter(|c| OURS.contains(&c.id.as_str()))
        .collect();
    assert_eq!(
        mine.len(),
        OURS.len(),
        "a case is missing in testing/attacks/cases"
    );
    assert!(mine.iter().all(|c| c.status == Status::Active));
    let report = run(&mine, &CopyTarget);
    assert!(report.ok(), "{}", report.line());
    assert_eq!(report.passed.len(), OURS.len());
    eprintln!("{}", report.line());
}
