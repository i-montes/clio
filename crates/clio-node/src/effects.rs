use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
// (File is the append handle)
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

#[derive(Serialize, Deserialize)]
struct Line {
    id: String,
    hash: String,
    result: Value,
}

type State = (File, HashMap<String, (String, Value)>);

/// Remembers, per action id, that its effect was applied and what it returned. One JSON line per
/// action, appended and synced. Shortcut: a crash between the effect and its line applies the
/// effect again on retry; close that window with an intent line when an effect is not repeatable.
pub struct EffectLog {
    state: Mutex<State>,
}

impl EffectLog {
    pub fn open(path: &Path) -> Result<Self> {
        let mut seen = HashMap::new();
        let mut torn = false;
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            torn = !text.is_empty() && !text.ends_with('\n');
            for l in text.lines() {
                // A torn last line (crash mid-append) is skipped: its effect is not recorded.
                if let Ok(l) = serde_json::from_str::<Line>(l) {
                    seen.insert(l.id, (l.hash, l.result));
                }
            }
        }
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        if torn {
            f.write_all(b"\n")?; // the next line must not glue onto the torn one
        }
        Ok(Self {
            state: Mutex::new((f, seen)),
        })
    }

    /// Runs `effect` unless `id` was already applied; returns the result and whether it ran now.
    /// The same id with another action hash is refused.
    pub fn apply(
        &self,
        id: &str,
        hash: &str,
        effect: impl FnOnce() -> Result<Value>,
    ) -> Result<(Value, bool)> {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((h, r)) = g.1.get(id) {
            return if h == hash {
                Ok((r.clone(), false))
            } else {
                Err(Error::IdReused(id.into()))
            };
        }
        let result = effect()?;
        let mut line = serde_json::to_string(&Line {
            id: id.into(),
            hash: hash.into(),
            result: result.clone(),
        })
        .map_err(|e| Error::BadArgs(e.to_string()))?;
        line.push('\n');
        g.0.write_all(line.as_bytes())?;
        g.0.sync_data()?;
        g.1.insert(id.into(), (hash.into(), result.clone()));
        Ok((result, true))
    }
}
