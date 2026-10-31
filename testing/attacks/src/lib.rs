//! The attack battery of docs/security.md: cases that must always fail. A case names the rules
//! it exercises and what a safe system does with it. Until the system under test exists a case
//! is `pending`: it is listed, never silently passed. Once `active` it must pass or the gate
//! blocks.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Active,
}

/// What a safe system does with the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// The payload has no effect: the action is denied or ignored.
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub title: String,
    /// The S-xx rules of docs/security.md this case exercises.
    pub rules: Vec<String>,
    pub status: Status,
    pub expect: Expect,
}

/// What the system under test did with a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    pub refused: bool,
}

pub trait Target {
    fn attack(&self, case: &Case) -> Observed;
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub passed: Vec<String>,
    pub failed: Vec<String>,
    pub pending: Vec<String>,
}

impl Report {
    /// The gate: no active case got through.
    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }

    pub fn line(&self) -> String {
        format!(
            "attacks: {} passed · {} failed · {} pending{}",
            self.passed.len(),
            self.failed.len(),
            self.pending.len(),
            if self.failed.is_empty() {
                String::new()
            } else {
                format!(" · failed: {}", self.failed.join(", "))
            }
        )
    }
}

pub fn run(cases: &[Case], target: &dyn Target) -> Report {
    let mut report = Report::default();
    for case in cases {
        match case.status {
            Status::Pending => report.pending.push(case.id.clone()),
            Status::Active => {
                let seen = target.attack(case);
                let held = match case.expect {
                    Expect::Refused => seen.refused,
                };
                if held {
                    &mut report.passed
                } else {
                    &mut report.failed
                }
                .push(case.id.clone());
            }
        }
    }
    report
}

/// Every `*.json` in `dir`, ordered by id.
pub fn load_cases(dir: &Path) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|e| e == "json") {
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            cases
                .push(serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?);
        }
    }
    cases.sort_by(|a: &Case, b: &Case| a.id.cmp(&b.id));
    Ok(cases)
}
