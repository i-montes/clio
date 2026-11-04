//! Jev and the memory in front of each message, wired to this machine (spec
//! `docs/specs/2026-11-03-jev-design.md`): the adapter that holds the HTTP, the key and the scrubber
//! from the vault, the person's memory read-only, and the mark on the Pi session. Everything here
//! degrades to «nothing»: a missing vault, key or Python means no Jev, never no chat.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clio_brain::memory_judge::MemoryJudge;
use clio_brain::memory_link::{EmbedModel, MemoryLink};
use clio_brain::pi::preprompt::{Candidate, Hooks, PrePrompt, Prepared, Recall, Scrub, Settings};
use clio_brain::pi::session::Toolhost;
use clio_core::SystemClock;
use clio_jev::{Config as JevConfig, Jev, Process, ToolHint, parse_catalog};
use clio_memory::Origin;
use clio_vault::VaultClient;
use serde_json::{Value, json};

use super::Res;
use super::home::{Dirs, JevCfg};
use super::vault::gateway_socket;
use super::{io_err, mkdir_private, write_private};

const ADAPTER_SOURCE: &str = include_str!("../../../../adapters/jev/jev_systemone.py");
const SECRET: &str = "jev";
const TOOL: &str = "jev";
const DOMAIN: &str = "api.typesafe.ai";

/// The adapter script: the copy that travels inside this binary, written private and refreshed
/// when it differs. A file of the checkout is used only if it is exactly that copy: the adapter
/// receives the key on every call and runs outside the jail, so nothing found on disk that
/// differs is ever run. Never searched in the working directory.
fn adapter_path(dirs: &Dirs) -> Res<PathBuf> {
    if let Some(c) = super::find_above_exe(Path::new("adapters/jev/jev_systemone.py"))
        && std::fs::read_to_string(&c).ok().as_deref() == Some(ADAPTER_SOURCE)
    {
        return c.canonicalize().map_err(|e| io_err(&c, &e));
    }
    let dir = dirs.data.join("adapters");
    mkdir_private(&dir)?;
    let p = dir.join("jev_systemone.py");
    if std::fs::read_to_string(&p).ok().as_deref() != Some(ADAPTER_SOURCE) {
        write_private(&p, ADAPTER_SOURCE.as_bytes())?;
    }
    Ok(p)
}

/// The client over `script`; the key is asked of the vault at `socket` on every call and kept
/// nowhere.
fn build_client(script: PathBuf, socket: &Path, base_url: Option<String>) -> Arc<Jev> {
    let vault = VaultClient::new(socket).with_timeout(Duration::from_secs(2));
    Arc::new(Jev::new(
        JevConfig::default(),
        Box::new(Process {
            python: "python3".into(),
            script,
            base_url,
        }),
        Box::new(move || {
            vault
                .resolve(SECRET, TOOL, DOMAIN, "")
                .map(|v| v.expose().to_owned())
                .map_err(|e| e.to_string())
        }),
        Arc::new(SystemClock),
    ))
}

/// The client, or `None` when Jev is off (the default) or cannot be set up.
pub fn client(dirs: &Dirs, cfg: &JevCfg) -> Option<Arc<Jev>> {
    if !cfg.enabled {
        return None;
    }
    let script = adapter_path(dirs)
        .inspect_err(|e| eprintln!("clio: Jev apagado: {e}"))
        .ok()?;
    let socket = gateway_socket(dirs).ok()?;
    Some(build_client(script, &socket, cfg.base_url.clone()))
}

const ENV_ADAPTER: &str = "CLIO_JEV_ADAPTER";
const ENV_BASE: &str = "CLIO_JEV_BASE_URL";
const ENV_VAULT: &str = "CLIO_JEV_VAULT_SOCKET";

/// What the tool host process needs to judge proposals with Jev, to be set in its environment when
/// `clio` starts it: nothing at all when Jev is off, so the service never judges.
pub fn toolhost_env(dirs: &Dirs, cfg: &JevCfg) -> Vec<(String, String)> {
    if !cfg.enabled {
        return Vec::new();
    }
    let (Ok(script), Ok(socket)) = (adapter_path(dirs), gateway_socket(dirs)) else {
        return Vec::new();
    };
    let mut v = vec![
        (ENV_ADAPTER.to_owned(), script.display().to_string()),
        (ENV_VAULT.to_owned(), socket.display().to_string()),
    ];
    if let Some(b) = &cfg.base_url {
        v.push((ENV_BASE.to_owned(), b.clone()));
    }
    v
}

/// In the tool host process: the judge of proposals, if `clio` said Jev is on. The text it asks
/// about goes through the vault's scrubber, and without the vault there is no question.
pub fn judge_from_env() -> Option<Arc<MemoryJudge>> {
    let script = PathBuf::from(std::env::var_os(ENV_ADAPTER)?);
    let socket = PathBuf::from(std::env::var_os(ENV_VAULT)?);
    let jev = build_client(script, &socket, std::env::var(ENV_BASE).ok());
    let vault = VaultClient::new(&socket).with_timeout(Duration::from_secs(2));
    Some(Arc::new(MemoryJudge::new(
        jev,
        Arc::new(move |t| vault.scrub(t).map_err(|e| e.to_string())),
    )))
}

/// The person's memory, read-only and opened per question (cheap, and the person's own writes
/// are never blocked).
pub struct MemoryRecall {
    data: PathBuf,
    model: Arc<EmbedModel>,
    /// The session's project: its memories are read with the person's and the lessons.
    project: Option<String>,
}

impl MemoryRecall {
    pub fn new(data: &Path, model_dirs: &[PathBuf], project: Option<&str>) -> Self {
        Self {
            data: data.to_path_buf(),
            model: EmbedModel::with_dirs(model_dirs.to_vec()),
            project: project.map(str::to_owned),
        }
    }

    fn with_link<T>(&self, f: impl FnOnce(&MemoryLink) -> Option<T>) -> Option<T> {
        match MemoryLink::open_read_in(&self.data, &self.model, self.project.as_deref()) {
            Ok(Some(link)) => f(&link),
            _ => None,
        }
    }
}

impl Recall for MemoryRecall {
    fn fixed(&self) -> Vec<Candidate> {
        self.with_link(|link| {
            let engine = link.engine();
            let mut out = Vec::new();
            for block in engine.blocks(link.scope()).ok()? {
                // Each entry as it is now (a correction followed, as the context does).
                for row in engine.block_current(block.id).ok()? {
                    out.push(Candidate {
                        id: row.id.0,
                        text: row.content,
                        person: row.origin == Origin::User,
                    });
                }
            }
            Some(out)
        })
        .unwrap_or_default()
    }

    fn candidates(&self, query: &str, limit: usize) -> Vec<Candidate> {
        self.with_link(|link| {
            let (rows, abstained) = link.search_all(query, limit).ok()?;
            if abstained {
                return Some(Vec::new()); // the memory says nothing here is relevant
            }
            Some(
                rows.into_iter()
                    .map(|r| Candidate {
                        id: r.id.0,
                        text: r.content,
                        person: r.origin == Origin::User,
                    })
                    .collect(),
            )
        })
        .unwrap_or_default()
    }
}

/// The tools Pi was started with, from the catalog the guard wrote into the session
/// (`clio-guard.tools`). Empty when it is not there or is not one Jev can be asked about.
pub fn catalog_from_entries(entries: &Value) -> Vec<ToolHint> {
    entries["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["customType"] == "clio-guard.tools")
        .find_map(|e| parse_catalog(&e["data"]["catalog"]).ok())
        .unwrap_or_default()
}

/// One request to the tool host and its answer, or `None` if it could not be had in time.
fn ask_host(socket: &Path, req: &Value) -> Option<Value> {
    let mut s = UnixStream::connect(socket).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(5))).ok()?;
    writeln!(s, "{req}").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// Tells the tool host that this session now holds something the person did not write. Tried a
/// few times (the host answers one request at a time and may be busy with a tool); `true` only if
/// the host said it was done. A mark that could not be made means what is not the person's is not
/// shown.
pub fn taint_session(socket: &Path, session: &str) -> bool {
    let req =
        json!({"id": "taint", "tool": "taint", "args": {}, "session": session, "call_id": "taint"});
    for _ in 0..2 {
        if ask_host(socket, &req).is_some_and(|a| a["ok"] == true && a["tainted"] == true) {
            return true;
        }
    }
    eprintln!("clio: no se pudo marcar la sesión como contaminada: no se muestra memoria ajena");
    false
}

/// Tells the tool host the person closed the session: its last turn may only save the summary.
/// `false` if the host did not take it.
pub fn closing_to_host(socket: &Path, session: &str, token: &str) -> bool {
    let req = json!({"id": "closing", "tool": "closing", "args": {"token": token}, "session": session, "call_id": "closing"});
    ask_host(socket, &req).is_some_and(|r| r["ok"] == json!(true))
}

/// Everything that goes in front of a message in this chat.
pub fn preprompt(
    dirs: &Dirs,
    cfg: &JevCfg,
    entries: &Value,
    toolhost: &Toolhost,
    session: &str,
    run_dir: &Path,
    project: Option<&str>,
) -> PrePrompt {
    let models = MemoryLink::model_dirs(&dirs.home);
    let recall: Arc<dyn Recall> = Arc::new(MemoryRecall::new(&dirs.data, &models, project));
    let vault = gateway_socket(dirs)
        .ok()
        .map(|s| VaultClient::new(&s).with_timeout(Duration::from_secs(2)));
    // The vault's scrubber or nothing, for what goes to Jev (S-44) and for what the model is shown:
    // a value only the vault knows cannot be struck out without it.
    let scrub: Scrub = Arc::new(move |t| match &vault {
        Some(v) => v.scrub(t).map_err(|e| e.to_string()),
        None => Err("sin baúl".into()),
    });
    let (socket, id) = (toolhost.socket.clone(), session.to_owned());
    let log_file = run_dir.join("memory-inject.jsonl");
    PrePrompt::new(
        client(dirs, cfg),
        catalog_from_entries(entries),
        Some(recall),
        scrub,
        Hooks {
            taint: Arc::new(move || taint_session(&socket, &id)),
            log: Arc::new(move |p| log_shown(&log_file, p)),
        },
        Settings {
            memory: cfg.memory,
            tool_hints: cfg.tool_hints,
        },
    )
}

/// One line of facts per message that had anything to show: which memories (ids), how many tools
/// were suggested and whether Jev took part. Never a text.
fn log_shown(path: &Path, p: &Prepared) {
    if p.memory_ids.is_empty() && p.tools.is_empty() {
        return;
    }
    use std::os::unix::fs::OpenOptionsExt;
    let line = json!({"ids": p.memory_ids, "tools": p.tools.len(), "jev": p.used_jev, "tainted": p.tainted});
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_is_read_from_the_entry_the_guard_wrote() {
        let entries = json!({"entries": [
            {"type": "message"},
            {"customType": "clio-guard.tools", "data": {"active": ["read"], "catalog": [
                {"name": "read", "use_when": "leer archivos del equipo"},
                {"name": "bash", "use_when": "ejecutar comandos en una terminal"}]}},
        ]});
        let c = catalog_from_entries(&entries);
        assert_eq!(
            c.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["read", "bash"]
        );
        // No entry, or one Jev cannot be asked about: no tools are suggested, nothing breaks.
        assert!(catalog_from_entries(&json!({"entries": []})).is_empty());
        assert!(catalog_from_entries(&json!({})).is_empty());
        let bad = json!({"entries": [{"customType": "clio-guard.tools", "data": {"catalog": [{"name": "x"}]}}]});
        assert!(catalog_from_entries(&bad).is_empty());
    }
}
