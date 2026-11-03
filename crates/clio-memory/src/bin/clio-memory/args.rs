//! Hand-written argument parsing: the grammar is small and fixed, and this crate takes no
//! dependency just for it.

use std::path::PathBuf;

use clio_memory::{JobState, MemoryId, MemoryType, OpId, Origin, Signal, State};

pub const USAGE: &str = "\
usage: clio-memory --db PATH [--actor NAME] [--model DIR] [--json] <command> [args]

inspect (open the file read-only; never write):
  scopes                      list scopes
  stats                       sizes, counts by state, queue
  doctor                      health checks (exit 1 if one fails)
  list SCOPE [--state S]      memories (S: active|quarantine|archived|replaced)
  get ID                      one memory (ID is 12 or m12)
  history ID                  every operation that touched a memory
  review SCOPE                quarantined memories waiting for a person
  jobs [--state S]            background jobs (S: pending|running|done|failed)
  recall SCOPE TEXT...        what the memory would bring up for TEXT
  context SCOPE TEXT...       the prompt context for TEXT (fixed blocks + recall)

change (open the file read-write; fail if another program holds its write lock):
  remember SCOPE TEXT... [--kind K] [--topic T] [--source S]
                              propose one memory through the normal rules (K: working|episodic|
                              semantic|procedural, default semantic); with --actor NAME it is
                              an agent's memory, without it a person's
  promote ID                  accept a quarantined memory
  archive ID                  take a memory out of circulation (nothing is deleted)
  revert OP                   undo one operation (see `history`)
  purge ID [--reason TEXT] [--yes]
                              erase a memory's content for good, from every operation too, and
                              leave a tombstone; asks first in a terminal (--yes skips it, for
                              tests); a person only, and nothing can undo it
  feedback ID useful|wrong    say whether a memory helped; three `wrong` send it to review
  retry-job ID                put a failed job back in the queue
  run-jobs [--heavy]          run queued jobs now (--heavy also runs backup and vacuum)
  backup PATH                 hot copy of the file to a new PATH (mode 0600)
  import PATH                 restore an export into a new, empty file (--db)
  import-memories SCOPE PATH [--origin user|agent|external]
                              load loose memories, one JSON object per line, through the
                              normal rules (default origin external: they wait in quarantine)

export (reads only):
  export PATH                 write the whole memory as JSON Lines to a new PATH (mode 0600)

options:
  --db PATH     the memory file (required)
  --actor NAME  act as this person instead of cli:$USER (write commands)
  --model DIR   local model2vec model for recall and context (needs the model2vec feature)
  --json        machine-readable output where supported
  -h, --help    this text";

#[derive(Debug)]
pub struct UsageError(pub String);

#[derive(Debug)]
pub enum Command {
    Scopes,
    Stats,
    Doctor,
    List {
        scope: String,
        state: Option<State>,
    },
    Get {
        id: MemoryId,
    },
    History {
        id: MemoryId,
    },
    Review {
        scope: String,
    },
    Jobs {
        state: Option<JobState>,
    },
    Recall {
        scope: String,
        text: String,
    },
    Context {
        scope: String,
        text: String,
    },
    Export {
        path: PathBuf,
    },
    Remember {
        scope: String,
        text: String,
        kind: MemoryType,
        topic: Option<String>,
        source: Option<String>,
    },
    Promote {
        id: MemoryId,
    },
    Archive {
        id: MemoryId,
    },
    Revert {
        op: OpId,
    },
    Purge {
        id: MemoryId,
        reason: String,
        yes: bool,
    },
    Feedback {
        id: MemoryId,
        signal: Signal,
    },
    RetryJob {
        id: i64,
    },
    RunJobs {
        heavy: bool,
    },
    Backup {
        path: PathBuf,
    },
    Import {
        path: PathBuf,
    },
    ImportMemories {
        scope: String,
        path: PathBuf,
        origin: Option<Origin>,
    },
}

impl Command {
    /// Whether the command needs the file open read-write (and so the writer lock).
    pub fn writes(&self) -> bool {
        !matches!(
            self,
            Self::Scopes
                | Self::Stats
                | Self::Doctor
                | Self::List { .. }
                | Self::Get { .. }
                | Self::History { .. }
                | Self::Review { .. }
                | Self::Jobs { .. }
                | Self::Recall { .. }
                | Self::Context { .. }
                | Self::Export { .. }
        )
    }
}

#[derive(Debug)]
pub struct Cli {
    pub db: PathBuf,
    /// Who the write commands act as; `cli:$USER` when absent.
    pub actor: Option<String>,
    pub model: Option<PathBuf>,
    pub json: bool,
    pub command: Command,
}

#[derive(Debug)]
pub enum Parsed {
    Help,
    Run(Cli),
}

fn usage<T>(msg: impl Into<String>) -> Result<T, UsageError> {
    Err(UsageError(msg.into()))
}

/// The arguments after the command name: positionals and `--flags`.
struct Rest(Vec<String>);

impl Rest {
    /// Removes `--name VALUE` if present.
    fn flag(&mut self, name: &str) -> Result<Option<String>, UsageError> {
        let Some(i) = self.0.iter().position(|a| a == name) else {
            return Ok(None);
        };
        if i + 1 >= self.0.len() {
            return usage(format!("{name} needs a value"));
        }
        let value = self.0.remove(i + 1);
        self.0.remove(i);
        Ok(Some(value))
    }

    /// Removes a bare `--name` switch if present.
    fn switch(&mut self, name: &str) -> bool {
        match self.0.iter().position(|a| a == name) {
            Some(i) => {
                self.0.remove(i);
                true
            }
            None => false,
        }
    }

    /// Everything left must be positional.
    fn positionals(self) -> Result<Vec<String>, UsageError> {
        match self.0.iter().find(|a| a.starts_with("--")) {
            Some(bad) => usage(format!("unknown option {bad}")),
            None => Ok(self.0),
        }
    }

    /// No positionals at all.
    fn none(self) -> Result<(), UsageError> {
        match self.positionals()?.first() {
            Some(extra) => usage(format!("unexpected argument {extra:?}")),
            None => Ok(()),
        }
    }

    /// Exactly one positional, called `name` in the error message.
    fn one(self, name: &str) -> Result<String, UsageError> {
        let mut args = self.positionals()?;
        if args.len() != 1 {
            return usage(format!("expected {name}"));
        }
        Ok(args.remove(0))
    }

    /// Exactly two positionals.
    fn two(self, names: &str) -> Result<(String, String), UsageError> {
        let mut args = self.positionals()?;
        if args.len() != 2 {
            return usage(format!("expected {names}"));
        }
        let second = args.remove(1);
        Ok((args.remove(0), second))
    }

    /// One positional followed by free text (joined with spaces, so quoting is optional).
    fn with_text(self, scope: &str) -> Result<(String, String), UsageError> {
        let mut args = self.positionals()?;
        if args.len() < 2 {
            return usage(format!("expected {scope} TEXT"));
        }
        let first = args.remove(0);
        Ok((first, args.join(" ")))
    }
}

pub fn parse_id(s: &str) -> Result<MemoryId, UsageError> {
    let digits = s.strip_prefix(['m', 'M']).unwrap_or(s);
    match digits.parse::<i64>() {
        Ok(n) if n > 0 => Ok(MemoryId(n)),
        _ => usage(format!("{s:?} is not an id (use 12 or m12)")),
    }
}

fn parse_op(s: &str) -> Result<OpId, UsageError> {
    match s.parse::<i64>() {
        Ok(n) if n > 0 => Ok(OpId(n)),
        _ => usage(format!("{s:?} is not an operation id")),
    }
}

fn parse_origin(s: &str) -> Result<Origin, UsageError> {
    match s {
        "user" => Ok(Origin::User),
        "agent" => Ok(Origin::Agent),
        "external" => Ok(Origin::External),
        _ => usage(format!("unknown origin {s:?} (user|agent|external)")),
    }
}

fn parse_kind(s: &str) -> Result<MemoryType, UsageError> {
    match s {
        "working" => Ok(MemoryType::Working),
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        _ => usage(format!(
            "unknown kind {s:?} (working|episodic|semantic|procedural)"
        )),
    }
}

fn parse_signal(s: &str) -> Result<Signal, UsageError> {
    match s {
        "useful" => Ok(Signal::Useful),
        "wrong" => Ok(Signal::Wrong),
        _ => usage(format!("unknown signal {s:?} (useful|wrong)")),
    }
}

fn parse_state(s: &str) -> Result<State, UsageError> {
    match s {
        "active" => Ok(State::Active),
        "quarantine" => Ok(State::Quarantine),
        "archived" => Ok(State::Archived),
        "replaced" => Ok(State::Replaced),
        _ => usage(format!(
            "unknown state {s:?} (active|quarantine|archived|replaced)"
        )),
    }
}

fn parse_job_state(s: &str) -> Result<JobState, UsageError> {
    match s {
        "pending" => Ok(JobState::Pending),
        "running" => Ok(JobState::Running),
        "done" => Ok(JobState::Done),
        "failed" => Ok(JobState::Failed),
        _ => usage(format!(
            "unknown job state {s:?} (pending|running|done|failed)"
        )),
    }
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, UsageError> {
    let mut args = args.into_iter();
    let (mut db, mut actor, mut model, mut json) = (None, None, None, false);
    let value = |args: &mut dyn Iterator<Item = String>, name: &str| {
        args.next()
            .ok_or_else(|| UsageError(format!("{name} needs a value")))
    };
    let name = loop {
        match args.next() {
            None => return usage("no command given"),
            Some(a) => match a.as_str() {
                "-h" | "--help" => return Ok(Parsed::Help),
                "--db" => db = Some(PathBuf::from(value(&mut args, "--db")?)),
                "--actor" => actor = Some(value(&mut args, "--actor")?),
                "--model" => model = Some(PathBuf::from(value(&mut args, "--model")?)),
                "--json" => json = true,
                flag if flag.starts_with('-') => return usage(format!("unknown option {flag}")),
                _ => break a,
            },
        }
    };
    let mut rest = Rest(args.collect());
    let command = match name.as_str() {
        "scopes" => {
            rest.none()?;
            Command::Scopes
        }
        "stats" => {
            rest.none()?;
            Command::Stats
        }
        "doctor" => {
            rest.none()?;
            Command::Doctor
        }
        "list" => {
            let state = rest.flag("--state")?.map(|s| parse_state(&s)).transpose()?;
            let scope = rest.one("SCOPE")?;
            Command::List { scope, state }
        }
        "get" | "history" => {
            let id = parse_id(&rest.one("ID")?)?;
            if name == "get" {
                Command::Get { id }
            } else {
                Command::History { id }
            }
        }
        "review" => {
            let scope = rest.one("SCOPE")?;
            Command::Review { scope }
        }
        "jobs" => {
            let state = rest
                .flag("--state")?
                .map(|s| parse_job_state(&s))
                .transpose()?;
            rest.none()?;
            Command::Jobs { state }
        }
        "recall" | "context" => {
            let (scope, text) = rest.with_text("SCOPE")?;
            if name == "recall" {
                Command::Recall { scope, text }
            } else {
                Command::Context { scope, text }
            }
        }
        "export" => Command::Export {
            path: PathBuf::from(rest.one("PATH")?),
        },
        "remember" => {
            let kind = rest
                .flag("--kind")?
                .map_or(Ok(MemoryType::Semantic), |k| parse_kind(&k))?;
            let topic = rest.flag("--topic")?;
            let source = rest.flag("--source")?;
            let (scope, text) = rest.with_text("SCOPE")?;
            Command::Remember {
                scope,
                text,
                kind,
                topic,
                source,
            }
        }
        "promote" => Command::Promote {
            id: parse_id(&rest.one("ID")?)?,
        },
        "archive" => Command::Archive {
            id: parse_id(&rest.one("ID")?)?,
        },
        "revert" => Command::Revert {
            op: parse_op(&rest.one("OP")?)?,
        },
        "purge" => {
            let yes = rest.switch("--yes");
            let reason = rest
                .flag("--reason")?
                .unwrap_or_else(|| "purged by a person".into());
            Command::Purge {
                id: parse_id(&rest.one("ID")?)?,
                reason,
                yes,
            }
        }
        "feedback" => {
            let (id, signal) = rest.two("ID useful|wrong")?;
            Command::Feedback {
                id: parse_id(&id)?,
                signal: parse_signal(&signal)?,
            }
        }
        "retry-job" => {
            let id = rest.one("ID")?;
            match id.parse::<i64>() {
                Ok(id) if id > 0 => Command::RetryJob { id },
                _ => return usage(format!("{id:?} is not a job id")),
            }
        }
        "run-jobs" => {
            let heavy = rest.switch("--heavy");
            rest.none()?;
            Command::RunJobs { heavy }
        }
        "backup" => Command::Backup {
            path: PathBuf::from(rest.one("PATH")?),
        },
        "import" => Command::Import {
            path: PathBuf::from(rest.one("PATH")?),
        },
        "import-memories" => {
            let origin = rest
                .flag("--origin")?
                .map(|o| parse_origin(&o))
                .transpose()?;
            let (scope, path) = rest.two("SCOPE PATH")?;
            Command::ImportMemories {
                scope,
                path: PathBuf::from(path),
                origin,
            }
        }
        other => return usage(format!("unknown command {other:?}")),
    };
    let Some(db) = db else {
        return usage("--db PATH is required");
    };
    #[cfg(not(feature = "model2vec"))]
    if model.is_some() {
        return usage("--model needs a build with the model2vec feature");
    }
    Ok(Parsed::Run(Cli {
        db,
        actor,
        model,
        json,
        command,
    }))
}
