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
                "jaula activa (bwrap {bwrap}, {} pruebas de escape detenidas): el agente puede correr pruebas",
                test.checks.len()
            ),
            Self::Unavailable(why) => {
                format!("jaula no disponible ({why}): el agente no puede ejecutar nada")
            }
            Self::Failed { test, .. } => format!(
                "la jaula FALLÓ su autoprueba ({}): run_tests desactivada",
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
        return Status::Unavailable("apagada con CLIO_JAIL=off".into());
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

/// What each check of the self-test proves, in Spanish.
pub fn describe(c: &SelfCheck) -> &'static str {
    match c.name.as_str() {
        "sees_only_work" => "solo ve /work y el sistema de solo lectura",
        "home_decoy_unreadable" => "no puede leer un archivo señuelo de tu HOME",
        "home_folder_invisible" => "tu carpeta personal no existe dentro",
        "runtime_dir_invisible" => "los sockets de Clio (XDG_RUNTIME_DIR) no existen dentro",
        "network_tcp" => "no puede conectar a un puerto TCP local del anfitrión",
        "network_abstract_socket" => "no alcanza un socket abstracto del anfitrión",
        "write_usr" => "no puede escribir en /usr",
        "write_etc" => "no puede escribir en /etc",
        "write_root" => "no puede escribir en la raíz",
        "write_real_home" => "no puede escribir en tu HOME real",
        "host_processes_hidden" => "no ve los procesos del anfitrión",
        "kill_host_parent" => "no puede matar a un proceso del anfitrión",
        "environment_clean" => "no hereda tus variables de entorno (claves, SSH_AUTH_SOCK)",
        "no_nested_user_namespace" => "no puede crear otros espacios de nombres",
        _ => "comprobación de la jaula",
    }
}
