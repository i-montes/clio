//! The bench with Pi as the harness (`clio-bench agent --harness pi`): the same lifecycle as
//! `clio run --harness pi` (`clio toolhost` on the working copy, hardened Pi with the guard, the
//! six tools, `--thinking low`), driven to `agent_settled` under the task's limits. The task's
//! prompt is sent as is; the system prompt is the one the guard checks (`clio-agent@4`).
//! The API key comes only from `OPENCODE_API_KEY` or `~/.config/clio/opencode-api-key`; it is
//! handed to Pi and the toolhost through their environment and never printed or written.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clio_brain::pi::PiPaths;
use clio_brain::pi::install::{self, NodeInfo, PiInstall};
use clio_brain::pi::launch::{Launch, Provider, Secret, SessionTarget, read_api_key, start};
use clio_brain::pi::rpc::{Cancel, Event};
use clio_brain::pi::session::{GUARD_TOOLS, Toolhost, ensure_guard, guard_env};
use serde_json::{Value, json};

/// Everything a Pi run needs that does not change between runs.
pub struct PiBench {
    /// A `clio` binary (the toolhost is its `toolhost` subcommand).
    pub clio: PathBuf,
    pub paths: PiPaths,
    pub node: NodeInfo,
    pub install: PiInstall,
    pub provider: Provider,
    pub key: Secret,
    /// Where the 0700 socket folders go (a short path).
    pub runtime: PathBuf,
}

/// What one Pi run left, in the same terms as the builtin loop's outcome.
pub struct PiDone {
    pub steps: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micro: u64,
    /// Toolhost and Pi start, before the prompt is sent.
    pub setup_ms: i64,
    pub ending: String,
    pub tool_calls: u32,
    pub denied: u32,
    pub tainted: bool,
    pub transcript: Value,
}

/// A `clio` binary: `CLIO_BIN`, else next to this executable (or one folder up, for tests).
pub fn locate_clio() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("CLIO_BIN") {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    exe.ancestors()
        .skip(1)
        .take(2)
        .map(|d| d.join("clio"))
        .find(|p| p.is_file())
        .ok_or_else(|| "no encuentro el binario `clio`: compílalo o pon CLIO_BIN".into())
}

impl PiBench {
    /// The real setup: Clio's data folder under `$HOME`, the pinned Pi install, the key from the
    /// environment or the key file. `price` is micro-dollars per million tokens, (in, out).
    pub fn from_env(provider: &str, model: &str, price: (u64, u64)) -> Result<Self, String> {
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME no está definida")?);
        let key = read_api_key(
            "OPENCODE_API_KEY",
            &home.join(".config/clio/opencode-api-key"),
        )
        .ok_or("falta la clave del modelo: OPENCODE_API_KEY o ~/.config/clio/opencode-api-key")?;
        let mut p = Provider::opencode_go(model);
        p.id = provider.into();
        p.dollars_per_mtok = (price.0 as f64 / 1e6, price.1 as f64 / 1e6);
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join("clio");
        Self::new(
            PiPaths::under_data(&home.join(".local/share/clio")),
            p,
            key,
            runtime,
        )
    }

    pub fn new(
        paths: PiPaths,
        provider: Provider,
        key: Secret,
        runtime: PathBuf,
    ) -> Result<Self, String> {
        let node = install::find_node(&std::env::var("PATH").unwrap_or_default())
            .map_err(|e| e.to_string())?;
        let install = install::locate(&paths).map_err(|e| e.to_string())?;
        Ok(Self {
            clio: locate_clio()?,
            paths,
            node,
            install,
            provider,
            key,
            runtime,
        })
    }

    /// One task prompt on a working copy. `run_dir` takes the toolhost's folders and log.
    /// `max_steps` (model calls) and `max_ms` are the task's limits: crossing one aborts the run.
    pub fn run(
        &self,
        id: &str,
        work: &Path,
        run_dir: &Path,
        prompt: &str,
        max_steps: u32,
        max_ms: i64,
    ) -> Result<PiDone, String> {
        let t0 = std::time::Instant::now();
        std::fs::create_dir_all(run_dir).map_err(|e| e.to_string())?;
        let toolhost = Toolhost::start(
            &self.clio,
            &self.runtime,
            &self.paths,
            run_dir,
            work,
            &self.key,
        )?;
        let launch = Launch {
            guard: Some(ensure_guard(&self.install)?),
            guard_tools: GUARD_TOOLS.map(String::from).to_vec(),
            extra_env: guard_env(&self.paths, &toolhost)?,
            provider: self.provider.clone(),
            api_key: Some(self.key.clone()),
            cwd: work.to_path_buf(),
            name: None,
            install: self.install.clone(),
            node: self.node.clone(),
            paths: self.paths.clone(),
            session: SessionTarget::New {
                id: Some(id.to_owned()),
            },
        };
        let mut pi = start(&launch).map_err(|e| e.to_string())?;
        pi.client.timeouts.run = Duration::from_millis(u64::try_from(max_ms).unwrap_or(0));
        let setup_ms = i64::try_from(t0.elapsed().as_millis()).unwrap_or(i64::MAX);
        let (cancel, mut steps, mut over, mut denied) = (Cancel::new(), 0u32, false, 0u32);
        let r = pi.client.run_prompt(prompt, &cancel, &mut |e| match e {
            Event::MessageEnd(m) if m.role == "assistant" => {
                steps += 1;
                if steps > max_steps && !over {
                    over = true;
                    cancel.cancel();
                }
            }
            Event::ToolEnd {
                is_error: true,
                result,
                ..
            } if is_denial(&result.to_string()) => denied += 1,
            _ => {}
        });
        let stats = pi.client.get_session_stats().ok();
        let entries = pi.client.request("get_entries", json!({})).ok();
        let transcript = pi
            .client
            .get_messages()
            .map(Value::Array)
            .unwrap_or(Value::Null);
        pi.client.kill();
        drop(pi);
        toolhost.stop();
        let list = |t: &str| {
            entries.as_ref().map_or(0, |e| {
                e["entries"]
                    .as_array()
                    .map_or(0, |a| a.iter().filter(|x| x["customType"] == t).count())
            })
        };
        let tainted = entries.as_ref().is_some_and(|e| {
            e["entries"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|x| x["customType"] == "clio-guard.taint" && x["data"]["tainted"] == true)
            })
        });
        let u = stats.map(|s| s.usage).unwrap_or_default();
        let (ending, tool_calls) = match &r {
            Ok(s) if over => (format!("Stopped(Steps) after {steps}"), s.tool_calls),
            Ok(s) if s.aborted => ("Aborted".to_owned(), s.tool_calls),
            Ok(s) if !s.errors.is_empty() => (
                format!(
                    "ModelError({})",
                    s.errors[0].chars().take(160).collect::<String>()
                ),
                s.tool_calls,
            ),
            Ok(s) => ("Settled".to_owned(), s.tool_calls),
            Err(e) => (format!("PiError({e})"), 0),
        };
        Ok(PiDone {
            steps,
            input_tokens: u.input + u.cache_read,
            output_tokens: u.output,
            cost_micro: (u.cost * 1e6).round() as u64,
            setup_ms,
            ending,
            tool_calls,
            denied: denied + u32::try_from(list("clio-guard.block")).unwrap_or(0),
            tainted,
            transcript,
        })
    }
}

/// The toolhost and the guard say so in their own words when they refuse a call.
fn is_denial(result: &str) -> bool {
    result.contains("[gateway]") || result.contains("[toolhost]") || result.contains("clio-guard:")
}

#[cfg(test)]
mod tests {
    use super::is_denial;

    #[test]
    fn denials_are_recognised_by_their_markers() {
        assert!(is_denial("[gateway] exec was not run (rule S-12)"));
        assert!(is_denial("clio-guard: not allowed"));
        assert!(!is_denial("ENOENT: no such file"));
    }
}
