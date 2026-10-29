//! The commands that change the file. They run as a person (`cli:$USER` unless `--actor`).

use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;
use std::sync::Arc;

use clio_memory::{
    ActorId, ActorKind, Engine, Error, ImportOpts, JobKind, JobState, OpenMode, Origin, Proposal,
    Quota, RunOpts, Signal, SystemClock,
};

use crate::args::{Cli, Command};
use crate::commands::{refuse_overwrite, scope_id, with_model};
use crate::output::{clean, line};

/// The CLI's own actors are people at a keyboard, not agents under a rate limit.
const CLI_QUOTA: Quota = Quota {
    max: 1_000_000,
    window_secs: 3600,
};

/// An agent that remembers through the CLI still answers to a quota.
const AGENT_QUOTA: Quota = Quota {
    max: 200,
    window_secs: 3600,
};

pub fn run(cli: &Cli) -> Result<ExitCode, String> {
    let created_new = matches!(cli.command, Command::Import { .. }) && !cli.db.exists();
    // the input is opened before the database so a bad path cannot leave a new empty file behind
    let import_input = match &cli.command {
        Command::Import { path } => {
            Some(File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?)
        }
        _ => None,
    };
    let engine = open_write(cli)?;
    let fail = |e: Error| e.to_string();
    match &cli.command {
        Command::Remember {
            scope,
            text,
            kind,
            topic,
            source,
        } => {
            let scope = scope_id(&engine, scope)?;
            let (actor, origin) = match &cli.actor {
                Some(name) => (
                    ensure_actor(&engine, name, ActorKind::Agent, &[ActorKind::Agent])?,
                    Origin::Agent,
                ),
                None => (person(&engine, cli)?, Origin::User),
            };
            let o = engine
                .propose(Proposal {
                    scope,
                    actor,
                    kind: *kind,
                    content: text.clone(),
                    topic_key: topic.clone(),
                    origin,
                    session: None,
                    episode: None,
                    source: source.clone(),
                    valid_from: None,
                    valid_to: None,
                })
                .map_err(fail)?;
            line(&format!(
                "{:?} m{} (op {}) · {:?}",
                o.kind, o.memory.0, o.op.0, o.state
            ));
        }
        Command::Promote { id } => {
            let actor = person(&engine, cli)?;
            let op = engine.promote(*id, actor).map_err(fail)?;
            line(&format!("promoted m{} (op {})", id.0, op.0));
        }
        Command::Archive { id } => {
            let actor = person(&engine, cli)?;
            let op = engine.archive(*id, actor).map_err(fail)?;
            line(&format!("archived m{} (op {})", id.0, op.0));
        }
        Command::Revert { op } => {
            let actor = person(&engine, cli)?;
            let new = engine.revert(*op, actor).map_err(fail)?;
            line(&format!("reverted op {} (op {})", op.0, new.0));
        }
        Command::Feedback { id, signal } => {
            let actor = person(&engine, cli)?;
            engine.feedback(*id, *signal, actor).map_err(fail)?;
            let summary = engine.feedback_summary(*id).map_err(fail)?;
            let word = if *signal == Signal::Useful {
                "useful"
            } else {
                "wrong"
            };
            line(&format!(
                "recorded {word} for m{} · confidence {:.2} · useful {} · wrong {}",
                id.0, summary.confidence, summary.useful, summary.wrong
            ));
        }
        Command::RetryJob { id } => {
            let actor = person(&engine, cli)?;
            engine.retry_job(*id, actor).map_err(fail)?;
            line(&format!("job {id} is back in the queue"));
        }
        Command::RunJobs { heavy } => {
            refuse_embed_without_model(&engine, cli)?;
            let report = engine
                .run_jobs(&RunOpts {
                    allow_heavy: *heavy,
                    ..RunOpts::default()
                })
                .map_err(fail)?;
            line(&format!(
                "done {} · retried {} · failed {} · skipped heavy {} · recovered {}",
                report.done, report.retried, report.failed, report.skipped_heavy, report.recovered
            ));
            if report.failed > 0 {
                return Ok(ExitCode::from(1)); // see `jobs --state failed`
            }
        }
        Command::Backup { path } => {
            refuse_overwrite(path)?;
            engine.enqueue_backup(path).map_err(fail)?;
            // only the backup: other queued work (an Embed job this run cannot do) stays queued
            let report = engine
                .run_jobs(&RunOpts {
                    allow_heavy: true,
                    only: Some(JobKind::Backup),
                    ..RunOpts::default()
                })
                .map_err(fail)?;
            // "done" is the only success: a failure that retrying might fix is put back in the
            // queue, and that is not a backup
            if report.done == 0 || std::fs::symlink_metadata(path).is_err() {
                let mut backups = Vec::new();
                for state in [JobState::Failed, JobState::Pending] {
                    backups.extend(
                        engine
                            .jobs(Some(state), 20)
                            .map_err(fail)?
                            .into_iter()
                            .filter(|j| j.kind == JobKind::Backup),
                    );
                }
                let newest = backups.into_iter().max_by_key(|j| j.id);
                let why = newest
                    .and_then(|j| j.error)
                    .map_or("no reason recorded".to_string(), |e| clean(&e));
                return Err(format!(
                    "the backup was not made ({why}); it stays queued unless it failed for good: see `jobs`"
                ));
            }
            line(&format!("backup written to {}", path.display()));
        }
        Command::Import { .. } => {
            let Some(file) = import_input else {
                return Err("no input".into());
            };
            let result = engine.import(&mut BufReader::new(file));
            if result.is_err() && created_new {
                drop(engine);
                remove_new_database(&cli.db);
            }
            let s = result.map_err(|e| match e {
                Error::InvalidState => {
                    "the database already has data: import restores into a new, empty file"
                        .to_string()
                }
                other => other.to_string(),
            })?;
            line(&format!(
                "restored: {} scopes · {} actors · {} operations · {} decisions · {} blocks · {} usage · {} feedback",
                s.scopes, s.actors, s.operations, s.decisions, s.blocks, s.usage, s.feedback
            ));
        }
        Command::ImportMemories {
            scope,
            path,
            origin,
        } => {
            let scope = scope_id(&engine, scope)?;
            let file =
                File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let actor = importer(&engine, cli)?;
            let opts = origin.map_or_else(ImportOpts::default, |origin| ImportOpts { origin });
            let s = engine
                .import_memories(&mut BufReader::new(file), scope, actor, &opts)
                .map_err(fail)?;
            line(&format!(
                "read {} · added {} · noop {} · quarantined {} · rejected {}",
                s.read, s.added, s.noop, s.quarantined, s.rejected_total
            ));
            for (n, why) in &s.rejected {
                line(&format!("line {n}: {}", clean(why)));
            }
            if s.rejected_total > s.rejected.len() {
                line(&format!(
                    "… and {} more",
                    s.rejected_total - s.rejected.len()
                ));
            }
            if let Some((n, why)) = &s.aborted {
                line(&format!("stopped at line {n}: {}", clean(why)));
            }
            if s.rejected_total > 0 || s.aborted.is_some() {
                return Ok(ExitCode::from(1));
            }
        }
        // the read commands never get here (`Command::writes` routes them elsewhere)
        _ => return Err("not a write command".into()),
    }
    Ok(ExitCode::SUCCESS)
}

/// An `import` that created the file and then failed must not leave an empty database behind.
fn remove_new_database(db: &std::path::Path) {
    for suffix in ["", "-wal", "-shm", ".lock"] {
        let mut name = db.as_os_str().to_owned();
        name.push(suffix);
        let _ = std::fs::remove_file(name);
    }
}

fn open_write(cli: &Cli) -> Result<Engine, String> {
    // opening read-write creates a missing file; only `import` is meant to
    let creates = matches!(cli.command, Command::Import { .. });
    if !creates && !cli.db.exists() {
        return Err(format!("database not found: {}", cli.db.display()));
    }
    let engine = Engine::open(&cli.db, OpenMode::ReadWrite, Arc::new(SystemClock))
        .map_err(|e| e.to_string())?;
    with_model(engine, cli)
}

/// Embed jobs fail for good when the engine has no embedder; this run has none unless
/// `--model` was given, so it must not start them.
fn refuse_embed_without_model(engine: &Engine, cli: &Cli) -> Result<(), String> {
    if cli.model.is_some() {
        return Ok(());
    }
    let waiting = engine
        .ready_jobs(JobKind::Embed)
        .map_err(|e| e.to_string())?;
    if waiting == 0 {
        return Ok(());
    }
    Err(format!(
        "{waiting} Embed job(s) are waiting and this run has no model, so they would fail for good; \
         pass --model DIR (needs a build with the model2vec feature)"
    ))
}

fn user() -> String {
    let raw = std::env::var("USER").unwrap_or_default();
    let name: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if name.is_empty() {
        "unknown".into()
    } else {
        name
    }
}

fn person(engine: &Engine, cli: &Cli) -> Result<ActorId, String> {
    let name = cli
        .actor
        .clone()
        .unwrap_or_else(|| format!("cli:{}", user()));
    ensure_actor(engine, &name, ActorKind::Person, &[ActorKind::Person])
}

fn importer(engine: &Engine, cli: &Cli) -> Result<ActorId, String> {
    match &cli.actor {
        Some(name) => ensure_actor(
            engine,
            name,
            ActorKind::Importer,
            &[ActorKind::Importer, ActorKind::Person],
        ),
        None => ensure_actor(
            engine,
            &format!("cli-import:{}", user()),
            ActorKind::Importer,
            &[ActorKind::Importer],
        ),
    }
}

/// The actor called `name`, created as `kind` if it does not exist yet; an existing one must be
/// one of `allowed`.
fn ensure_actor(
    engine: &Engine,
    name: &str,
    kind: ActorKind,
    allowed: &[ActorKind],
) -> Result<ActorId, String> {
    let known = engine.actors().map_err(|e| e.to_string())?;
    match known.into_iter().find(|a| a.name == name) {
        Some(a) if allowed.contains(&a.kind) => Ok(a.id),
        Some(a) => Err(format!(
            "actor {:?} is {}, not {}",
            clean(name),
            describe(a.kind),
            allowed
                .iter()
                .map(|k| describe(*k))
                .collect::<Vec<_>>()
                .join(" or ")
        )),
        None => {
            let quota = if kind == ActorKind::Agent {
                AGENT_QUOTA
            } else {
                CLI_QUOTA
            };
            engine
                .register_actor(name, kind, quota)
                .map_err(|e| e.to_string())
        }
    }
}

fn describe(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Person => "a person",
        ActorKind::Agent => "an agent",
        ActorKind::Session => "a session",
        ActorKind::Importer => "an importer",
        ActorKind::System => "the system",
    }
}
