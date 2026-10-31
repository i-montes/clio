use crate::{EffectLog, Error, Registry, Result, Sandbox};
use clio_core::Effect;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// What the gateway hands over after approving an action. Defined here so this crate does not
/// depend on the gateway.
pub trait Approval {
    fn is_valid_for(&self, action_hash: &str) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub tool: String,
    pub args: Value,
}

fn canon(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            out.push('{');
            for k in keys {
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canon(&m[k], out);
                out.push(',');
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for x in a {
                canon(x, out);
                out.push(',');
            }
            out.push(']');
        }
        x => out.push_str(&x.to_string()),
    }
}

impl Action {
    /// SHA-256 (hex) of the normalized action: object keys sorted, so key order never matters.
    pub fn hash(&self) -> String {
        let mut s = String::new();
        let v = serde_json::json!({"id": self.id, "tool": self.tool, "args": self.args});
        canon(&v, &mut s);
        Sha256::digest(s.as_bytes())
            .iter()
            .fold(String::new(), |mut h, b| {
                let _ = write!(h, "{b:02x}");
                h
            })
    }
}

#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub result: Value,
    /// False when the same action id had already been applied and this is its recorded result.
    pub applied: bool,
    /// False when the tool was not on the model's recommended list.
    pub recommended: bool,
}

pub struct Executor {
    pub registry: Registry,
    pub sandbox: Sandbox,
    pub log: EffectLog,
}

impl Executor {
    /// Fails closed: without an approval valid for this exact action, nothing runs or is recorded.
    pub fn execute(&self, action: &Action, approval: &dyn Approval) -> Result<Outcome> {
        let hash = action.hash();
        if !approval.is_valid_for(&hash) {
            return Err(Error::NotApproved);
        }
        let found = self
            .registry
            .get(&action.tool)
            .ok_or_else(|| Error::UnknownTool(action.tool.clone()))?;
        let run = || (found.tool.run)(&self.sandbox, &action.args);
        let (result, applied) = if found.tool.effect == Some(Effect::Read) {
            (run()?, true)
        } else {
            self.log.apply(&action.id, &hash, run)?
        };
        Ok(Outcome {
            result,
            applied,
            recommended: found.recommended,
        })
    }
}
