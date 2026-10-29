use std::process::ExitCode;
use std::sync::Arc;

use clio_memory::{
    ContextOpts, Engine, Error, Format, MemoryRow, OpenMode, RecallOpts, ScopeId, State,
    SystemClock,
};

use crate::args::{Cli, Command};
use crate::output::{clean, clean_multiline, line};

const JOB_LIMIT: usize = 200;

pub fn run(cli: &Cli) -> Result<ExitCode, String> {
    if cli.command.writes() {
        return crate::write::run(cli);
    }
    let engine = open_read(cli)?;
    let fail = |e: Error| e.to_string();
    match &cli.command {
        Command::Scopes => {
            let scopes = engine.scopes().map_err(fail)?;
            if cli.json {
                return json(&scopes);
            }
            for s in scopes {
                let keep = s.retention_days.map_or("never forgotten".to_string(), |d| {
                    format!("forgets after {d} days")
                });
                line(&format!(
                    "{} (id {}) · language {} · {keep}",
                    clean(&s.name),
                    s.id.0,
                    clean(&s.language)
                ));
            }
        }
        Command::Stats => {
            let s = engine.stats().map_err(fail)?;
            if cli.json {
                return json(&s);
            }
            line(&format!(
                "file: {} bytes (wal {})",
                s.file_bytes, s.wal_bytes
            ));
            line(&format!(
                "operations: {} · decisions: {} · vectors: {} · pending embeddings: {}",
                s.operations, s.decisions, s.vectors, s.pending_embeddings
            ));
            let age = s
                .queue
                .oldest_pending_age_secs
                .map_or(String::new(), |a| format!(" · oldest pending {a}s"));
            line(&format!(
                "queue: pending {} · running {} · failed {}{age}",
                s.queue.pending, s.queue.running, s.queue.failed
            ));
            for sc in s.scopes {
                line(&format!(
                    "scope {}: active {} · quarantine {} · archived {} · replaced {}",
                    clean(&sc.name),
                    sc.active,
                    sc.quarantine,
                    sc.archived,
                    sc.replaced
                ));
            }
        }
        Command::Doctor => {
            let report = engine.doctor().map_err(fail)?;
            if cli.json {
                json(&serde_json::json!({ "ok": report.all_ok(), "checks": report.checks }))?;
            } else {
                for c in &report.checks {
                    let mark = if c.ok { "ok  " } else { "FAIL" };
                    line(&format!("{mark} {}: {}", c.name, clean(&c.detail)));
                }
            }
            return Ok(if report.all_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            });
        }
        Command::List { scope, state } => {
            let rows = engine
                .list(scope_id(&engine, scope)?, *state, None)
                .map_err(fail)?;
            return memories(cli, &rows);
        }
        Command::Review { scope } => {
            let rows = engine
                .list(scope_id(&engine, scope)?, Some(State::Quarantine), None)
                .map_err(fail)?;
            return memories(cli, &rows);
        }
        Command::Get { id } => {
            let m = engine.get(*id).map_err(fail)?;
            if cli.json {
                return json(&m);
            }
            let who = actor_name(&engine, m.actor.0);
            let scope = engine
                .scopes()
                .map_err(fail)?
                .into_iter()
                .find(|s| s.id == m.scope)
                .map_or(m.scope.0.to_string(), |s| clean(&s.name));
            line(&format!("id: m{}", m.id.0));
            line(&format!("scope: {scope}"));
            line(&format!("state: {:?}", m.state));
            line(&format!("kind: {:?}", m.kind));
            line(&format!("origin: {:?}", m.origin));
            line(&format!("actor: {}", clean(&who)));
            if let Some(t) = &m.topic_key {
                line(&format!("topic: {}", clean(t)));
            }
            if let Some(s) = &m.source {
                line(&format!("source: {}", clean(s)));
            }
            let to = m.valid_to.map_or("open".to_string(), |t| t.to_string());
            line(&format!("valid: {} .. {to}", m.valid_from));
            line(&format!("known since: {}", m.known_since));
            line(&format!("content: {}", clean_multiline(&m.content)));
        }
        Command::History { id } => {
            let ops = engine.history(*id).map_err(fail)?;
            if cli.json {
                return json(&ops);
            }
            for op in ops {
                line(&format!(
                    "op {} · {:?} · by {} · at {} · {}",
                    op.id.0,
                    op.kind,
                    clean(&actor_name(&engine, op.actor.0)),
                    op.at,
                    clean(&op.reason)
                ));
            }
        }
        Command::Jobs { state } => {
            let jobs = engine.jobs(*state, JOB_LIMIT).map_err(fail)?;
            if cli.json {
                return json(&jobs);
            }
            for j in jobs {
                let error = j
                    .error
                    .as_deref()
                    .map_or(String::new(), |e| format!(" · error: {}", clean(e)));
                line(&format!(
                    "job {} · {:?} · {:?} · attempts {}/{} · run after {}{error}",
                    j.id, j.kind, j.state, j.attempts, j.max_attempts, j.run_after
                ));
            }
        }
        Command::Recall { scope, text } => {
            let found = engine
                .recall(scope_id(&engine, scope)?, text, &RecallOpts::default())
                .map_err(fail)?;
            if cli.json {
                let hits: Vec<_> = found
                    .hits
                    .iter()
                    .map(|h| {
                        serde_json::json!({
                            "memory": h.memory, "score": h.score, "confidence": h.confidence,
                        })
                    })
                    .collect();
                return json(&hits);
            }
            if found.hits.is_empty() {
                line("nothing relevant");
            }
            for h in &found.hits {
                line(&format!(
                    "{} · score {:.4}",
                    memory_line(&h.memory),
                    h.score
                ));
            }
        }
        Command::Export { path } => {
            refuse_overwrite(path)?;
            let s = engine.export_to_file(path).map_err(|e| match e {
                Error::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists => overwrite(path),
                other => other.to_string(),
            })?;
            line(&format!(
                "exported to {}: {} scopes · {} actors · {} operations · {} decisions · {} blocks · {} usage · {} feedback",
                path.display(),
                s.scopes,
                s.actors,
                s.operations,
                s.decisions,
                s.blocks,
                s.usage,
                s.feedback
            ));
        }
        Command::Context { scope, text } => {
            let opts = ContextOpts {
                query: Some(text.clone()),
                format: if cli.json {
                    Format::Json
                } else {
                    Format::Compact
                },
                ..ContextOpts::default()
            };
            let out = engine
                .context(scope_id(&engine, scope)?, &opts)
                .map_err(fail)?;
            if cli.json {
                line(&out);
            } else {
                for l in out.lines() {
                    line(&clean(l));
                }
            }
        }
        // the write commands never get here (`Command::writes` routes them to `write::run`)
        _ => return Err("not a read command".into()),
    }
    Ok(ExitCode::SUCCESS)
}

fn open_read(cli: &Cli) -> Result<Engine, String> {
    let engine =
        Engine::open(&cli.db, OpenMode::ReadOnly, Arc::new(SystemClock)).map_err(|e| match e {
            Error::NotFound => format!("database not found: {}", cli.db.display()),
            other => other.to_string(),
        })?;
    with_model(engine, cli)
}

#[cfg(feature = "model2vec")]
pub fn with_model(engine: Engine, cli: &Cli) -> Result<Engine, String> {
    match &cli.model {
        Some(dir) => {
            let model = clio_memory::Model2VecEmbedder::from_dir(dir).map_err(|e| e.to_string())?;
            Ok(engine.with_embedder(Arc::new(model)))
        }
        None => Ok(engine),
    }
}

#[cfg(not(feature = "model2vec"))]
pub fn with_model(engine: Engine, cli: &Cli) -> Result<Engine, String> {
    debug_assert!(
        cli.model.is_none(),
        "`args::parse` refuses --model in this build"
    );
    Ok(engine)
}

fn json<T: serde::Serialize>(value: &T) -> Result<ExitCode, String> {
    line(&serde_json::to_string(value).map_err(|e| e.to_string())?);
    Ok(ExitCode::SUCCESS)
}

pub fn scope_id(engine: &Engine, name: &str) -> Result<ScopeId, String> {
    let scopes = engine.scopes().map_err(|e| e.to_string())?;
    match scopes.iter().find(|s| s.name == name) {
        Some(s) => Ok(s.id),
        None => {
            let known: Vec<String> = scopes.iter().map(|s| clean(&s.name)).collect();
            Err(format!(
                "unknown scope {:?} (known: {})",
                clean(name),
                known.join(", ")
            ))
        }
    }
}

fn actor_name(engine: &Engine, id: i64) -> String {
    engine
        .actors()
        .ok()
        .and_then(|a| a.into_iter().find(|a| a.id.0 == id))
        .map_or(format!("actor {id}"), |a| a.name)
}

fn memory_line(m: &MemoryRow) -> String {
    let topic = m
        .topic_key
        .as_deref()
        .map_or(String::new(), |t| format!(" · topic {}", clean(t)));
    format!(
        "m{} · {:?} · {:?} · {}{topic}",
        m.id.0,
        m.state,
        m.kind,
        clean(&m.content)
    )
}

fn memories(cli: &Cli, rows: &[MemoryRow]) -> Result<ExitCode, String> {
    if cli.json {
        return json(&rows);
    }
    for m in rows {
        line(&memory_line(m));
    }
    Ok(ExitCode::SUCCESS)
}

pub fn overwrite(path: &std::path::Path) -> String {
    format!("refusing to overwrite {}", path.display())
}

/// Checked up front so a refused write leaves nothing behind (no failed job, no empty file).
pub fn refuse_overwrite(path: &std::path::Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(overwrite(path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot check {}: {e}", path.display())),
    }
}
