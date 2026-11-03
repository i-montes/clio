#![allow(clippy::unwrap_used)]
//! `scripts/rule-coverage.sh` (S-04): every rule of docs/security.md has a test that runs, unless
//! `scripts/rule-coverage.exempt` says, with a reason, where else it is checked. A rule added
//! without a test must stop the build; it used to join a list of enforced rules only when someone
//! remembered to add it.

use std::path::Path;
use std::process::Command;

/// A repository in miniature: two rules, S-01 with a test and S-02 without, and an exempt file.
fn repo(exempt: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let p = t.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    w(
        "docs/security.md",
        "| S-01 | uno | x |\n| S-02 | dos | x |\n",
    );
    w("crates/c/tests/t.rs", "// S-01\n#[test]\nfn a() {}\n");
    w("testing/.keep", "");
    w("scripts/rule-coverage.exempt", exempt);
    t
}

/// Runs the real script against the miniature repository: (exit code, what it printed).
fn run(root: &Path) -> (i32, String) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/rule-coverage.sh");
    let o = Command::new("bash")
        .arg(script)
        .env("RULE_COVERAGE_ROOT", root)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&[o.stdout, o.stderr].concat()).into_owned();
    (o.status.code().unwrap_or(-1), said)
}

// S-04
#[test]
fn a_rule_without_a_test_and_without_an_exemption_stops_the_build() {
    let (code, said) = run(repo("").path());
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("S-02") && said.contains("not exempt"),
        "{said}"
    );
}

// S-04
#[test]
fn an_exemption_with_a_reason_lets_it_through_and_is_counted() {
    let (code, said) = run(repo("S-02  checked by the audit job of CI\n").path());
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("1/2") || said.contains("exempt: 1"), "{said}");
}

// S-04
#[test]
fn an_exemption_needs_a_reason() {
    let (code, said) = run(repo("S-02\n").path());
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("S-02") && said.contains("reason"), "{said}");
}

// S-04
#[test]
fn an_exemption_that_now_has_a_test_must_be_removed() {
    let (code, said) = run(repo("S-01  it had no test once\nS-02  elsewhere\n").path());
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("S-01") && said.contains("has a test"),
        "{said}"
    );
}

// S-04
#[test]
fn an_exemption_for_a_rule_nothing_defines_is_an_error() {
    let (code, said) = run(repo("S-02  elsewhere\nS-99  a rule that was deleted\n").path());
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("S-99") && said.contains("no document defines"),
        "{said}"
    );
}
