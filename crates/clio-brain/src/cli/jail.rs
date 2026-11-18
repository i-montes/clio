//! Is the jail there, and has it been PROVEN? The agent is offered `run_tests` only if the answer
//! is yes: `bwrap` and `prlimit` found, namespaces working on this kernel, and the self-test (a
//! program inside the jail tries forbidden things) passed every check. Anything else closes the
//! tool, and the person is told why.

use std::sync::OnceLock;

use clio_node::{Jail, SelfCheck, SelfTest};

pub enum Status {
    /// Proven: `run_tests` may be offered.
    Ready { bwrap: String, test: SelfTest },
    /// The jail cannot run here (missing tool, no namespaces, not Linux, switched off).
    Unavailable(String),
    /// It runs but a check FAILED: a hole. The tool stays closed.
    Failed { bwrap: String, test: SelfTest },
}

impl Status {
    pub fn ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// One line for `clio run`.
    pub fn summary(&self) -> String {
        match self {
            Self::Ready { bwrap, test } => format!(
                "jail active (bwrap {bwrap}, {} escape tests stopped): the agent can run tests",
                test.checks.len()
            ),
            Self::Unavailable(why) => {
                format!("jail not available ({why}): the agent cannot run anything")
            }
            Self::Failed { test, .. } => format!(
                "the jail FAILED its self-test ({}): run_tests is off",
                failed_names(test)
            ),
        }
    }
}

fn failed_names(t: &SelfTest) -> String {
    t.checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Detects, probes and self-tests the jail. `CLIO_JAIL=off` switches the tool off.
pub fn check_fresh() -> Status {
    if std::env::var("CLIO_JAIL").is_ok_and(|v| v == "off") {
        return Status::Unavailable("switched off with CLIO_JAIL=off".into());
    }
    let jail = match Jail::detect().and_then(|j| j.probe().map(|()| j)) {
        Ok(j) => j,
        Err(e) => return Status::Unavailable(e),
    };
    let bwrap = jail.bwrap_version();
    match jail.self_test() {
        Err(e) => Status::Unavailable(e),
        Ok(test) if test.ok() => Status::Ready { bwrap, test },
        Ok(test) => Status::Failed { bwrap, test },
    }
}

/// The same, once per process (a run does not repeat the self-test).
pub fn status() -> &'static Status {
    static S: OnceLock<Status> = OnceLock::new();
    S.get_or_init(check_fresh)
}

/// What each check of the self-test proves, in plain words.
pub fn describe(c: &SelfCheck) -> &'static str {
    match c.name.as_str() {
        "sees_only_work" => "it sees only /work and the read-only system",
        "home_decoy_unreadable" => "it cannot read a decoy file from your HOME",
        "home_folder_invisible" => "your home folder does not exist inside",
        "runtime_dir_invisible" => "Clio's sockets (XDG_RUNTIME_DIR) do not exist inside",
        "network_tcp" => "it cannot connect to a local TCP port of the host",
        "network_abstract_socket" => "it cannot reach an abstract socket of the host",
        "write_usr" => "it cannot write to /usr",
        "write_etc" => "it cannot write to /etc",
        "write_root" => "it cannot write to the root",
        "write_real_home" => "it cannot write to your real HOME",
        "host_processes_hidden" => "it does not see the host's processes",
        "kill_host_parent" => "it cannot kill a process of the host",
        "environment_clean" => {
            "it does not inherit your environment variables (keys, SSH_AUTH_SOCK)"
        }
        "no_nested_user_namespace" => "it cannot create other namespaces",
        _ => "a jail check",
    }
}
