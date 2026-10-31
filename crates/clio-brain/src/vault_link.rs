//! The link between the gateway and the vault process (tasks 206, 207). It lives here because
//! the vault must not depend on the gateway and the gateway does not know the vault.
//!
//! - [`VaultPolicy`]: the gateway's `SecretPolicy`, answered by the vault's `check` (no value
//!   moves; a refusal is audited and alerted by the vault).
//! - [`SecretExecutor`]: the gateway's `Executor`. It resolves `{{secreto:name}}` last, inside
//!   the executing process, only for the action's tool and destination domains, runs the tool
//!   with the real values, and sends every output through the vault's filter before returning.
//!   The model, the decision and the gateway's log only ever see the markers.
//!
//! Limits: the resolved values live in this process's memory while the tool runs and are not
//! wiped afterwards (no `zeroize` dependency); a secret needs a destination domain.

use clio_gateway::{Executor, NormalizedAction, SecretPolicy};
use clio_vault::{SecretValue, VaultClient};
use serde_json::{Map, Value};

pub struct VaultPolicy(pub VaultClient);

impl SecretPolicy for VaultPolicy {
    /// Any failure to reach the vault is "not allowed".
    fn allowed(&self, secret: &str, tool: &str, domain: Option<&str>) -> bool {
        self.0.check(secret, tool, domain, "gateway-policy").is_ok()
    }
}

/// What actually runs a tool, with secrets already in its arguments.
pub trait ToolRunner: Send + Sync {
    fn run(&self, tool: &str, args: &Map<String, Value>) -> Result<String, String>;
}

pub struct SecretExecutor<R> {
    vault: VaultClient,
    runner: R,
}

impl<R: ToolRunner> SecretExecutor<R> {
    /// `vault` must talk to the vault's GATEWAY socket.
    pub fn new(vault: VaultClient, runner: R) -> Self {
        Self { vault, runner }
    }

    fn resolve_all(
        &self,
        a: &NormalizedAction,
        trace: &str,
    ) -> Result<Vec<(String, SecretValue)>, String> {
        let mut values = Vec::new();
        for name in &a.secrets {
            if a.domains.is_empty() {
                return Err(format!("secret {name} needs a destination domain"));
            }
            // The vault checks every destination; the value is the same for all of them.
            let mut value = None;
            for d in &a.domains {
                let v = self
                    .vault
                    .resolve(name, &a.tool, d, trace)
                    .map_err(|e| format!("secret {name}: {e}"))?;
                value = Some(v);
            }
            values.extend(value.map(|v| (name.clone(), v)));
        }
        Ok(values)
    }
}

impl<R: ToolRunner> Executor for SecretExecutor<R> {
    fn run(&self, a: &NormalizedAction, hash: &str) -> Result<String, String> {
        let values = self.resolve_all(a, hash)?;
        let args = fill(&Value::Object(a.args.clone()), &values);
        let Value::Object(args) = args else {
            return Err("arguments are not an object".into());
        };
        let result = self.runner.run(&a.tool, &args);
        drop(values);
        // Success or failure, the text goes through the vault's filter; if the vault cannot
        // filter, the text is dropped rather than returned raw.
        match result {
            Ok(out) => self
                .vault
                .scrub(&out)
                .map_err(|e| format!("output dropped, the vault could not filter it: {e}")),
            Err(e) => Err(self
                .vault
                .scrub(&e)
                .unwrap_or_else(|_| "the tool failed (message dropped: no filter)".into())),
        }
    }
}

/// Replaces each marker in the string values of `v` with its value.
fn fill(v: &Value, values: &[(String, SecretValue)]) -> Value {
    match v {
        Value::String(s) => Value::String(values.iter().fold(s.clone(), |s, (name, val)| {
            s.replace(&clio_vault::marker(name), val.expose())
        })),
        Value::Array(a) => Value::Array(a.iter().map(|x| fill(x, values)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), fill(x, values)))
                .collect(),
        ),
        other => other.clone(),
    }
}
