use std::path::PathBuf;

use clio_attacks::{Case, Expect, Observed, Status, Target, load_cases, run};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn security_md() -> String {
    std::fs::read_to_string(root().join("../../docs/security.md")).expect("security.md")
}

/// A system that does whatever the payload says.
struct Gullible;
impl Target for Gullible {
    fn attack(&self, _: &Case) -> Observed {
        Observed { refused: false }
    }
}

struct Careful;
impl Target for Careful {
    fn attack(&self, _: &Case) -> Observed {
        Observed { refused: true }
    }
}

fn case(id: &str, status: Status) -> Case {
    Case {
        id: id.into(),
        title: id.into(),
        rules: vec!["S-01".into()],
        status,
        expect: Expect::Refused,
    }
}

#[test]
fn an_active_case_that_gets_through_blocks_the_gate_and_pending_ones_are_listed_not_passed() {
    let cases = [case("a", Status::Active), case("b", Status::Pending)];
    let bad = run(&cases, &Gullible);
    assert!(!bad.ok());
    assert_eq!(
        (bad.failed.as_slice(), bad.pending.as_slice()),
        (["a".to_string()].as_slice(), ["b".to_string()].as_slice())
    );
    let good = run(&cases, &Careful);
    assert!(good.ok());
    assert_eq!(good.passed, ["a"]);
    assert_eq!(good.pending, ["b"]);
}

/// S-04: the battery of docs/security.md and the cases here are the same list. A new incident
/// adds a line there and a file in cases/ (rule of process of the project).
#[test]
fn the_cases_match_the_battery_in_security_md() {
    let doc = security_md();
    let battery = doc
        .split("### Batería de ataques")
        .nth(1)
        .expect("section")
        .split("### Si algo sale mal")
        .next()
        .expect("end");
    let items = battery
        .lines()
        .filter(|l| {
            l.split('.')
                .next()
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .count();
    let cases = load_cases(&root().join("cases")).expect("cases");
    assert_eq!(
        cases.len(),
        items,
        "{items} attacks in security.md, {} cases",
        cases.len()
    );
}

#[test]
fn every_case_names_rules_that_exist_and_ids_are_unique() {
    let doc = security_md();
    let cases = load_cases(&root().join("cases")).expect("cases");
    let mut ids: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
    ids.dedup();
    assert_eq!(ids.len(), cases.len());
    for c in &cases {
        assert!(!c.rules.is_empty(), "{} names no rule", c.id);
        for r in &c.rules {
            assert!(
                doc.contains(&format!("| {r} |")),
                "{} names {r}, which security.md does not define",
                c.id
            );
        }
    }
}
