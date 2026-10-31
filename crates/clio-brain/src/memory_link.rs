//! The link between the agent and `clio-memory`: the ONLY place that writes memory on behalf of
//! a run (S-25, S-26, S-27).
//!
//! The file is `<data>/memory.db` (0600, scope `clio`), never anywhere else. Four actors, each
//! with its own quota: `persona` (what the person types or confirms: the only one that may
//! promote, archive and say "origin: user"), `agente`, `sesion` and `importador`.
//!
//! What a run does with it:
//! - At the start, [`MemoryLink::recall_for`] asks for the fixed blocks and what is relevant to
//!   the instruction, each under its own budget. "Nothing relevant" is a valid answer: the
//!   context then has no memory layer at all.
//! - At the close, [`MemoryLink::close_run`] writes ONE batch, after the model loop is over.
//!   The run's own record is episodic: active only if the run never touched outside content
//!   (`tainted == false`), in quarantine otherwise. What the model proposes (lines `RECORDAR:` or
//!   `PROCEDIMIENTO:` in its final message) ALWAYS goes to quarantine, as external origin: a model
//!   that read a hostile file can say anything, and nothing it says is a fact of the person.
//!   Procedural memories wait for a person on top of that. A refusal (secret, quota, size) is
//!   recorded in the report and never breaks the close.
//! - Facts of the person only enter through [`MemoryLink::remember`], which no agent code calls.
//!
//! The embedding model is optional: without it everything works by text and `note()` says so.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use clio_memory::{
    ActorId, ActorInfo, ActorKind, ContextOpts, Engine, Error, MemoryId, MemoryRow, MemoryType,
    OpenMode, Origin, Proposal, Quota, RecallOpts, ScopeId, SearchOpts, Signal, State, SystemClock,
};
use clio_vault::Scrubber;

pub const SCOPE: &str = "clio";
pub const DB_FILE: &str = "memory.db";
/// Where the local embedding model lives, relative to HOME.
pub const MODEL_REL: &str = ".cache/clio-models/potion-multilingual-128M";
/// Most proposals a run may make through its final message; the rest are counted and dropped.
pub const MAX_AGENT_PROPOSALS: usize = 5;
/// Tokens for what is recalled (each fixed block has its own budget).
pub const RECALL_BUDGET: usize = 400;
const BLOCKS: [(&str, usize); 3] = [("identidad", 150), ("reglas", 200), ("preferencias", 150)];

pub type Res<T> = Result<T, String>;

/// A refusal in the words of a person.
pub fn explain(e: &Error) -> String {
    match e {
        Error::SecretDetected => "rechazado: parece una clave o un secreto".into(),
        Error::QuotaExceeded => "rechazado: cuota de propuestas agotada para este actor".into(),
        Error::TooLong => "rechazado: demasiado largo".into(),
        Error::EmptyContent => "rechazado: vacío".into(),
        Error::WriterLocked => "otro proceso está escribiendo la memoria; reintenta".into(),
        Error::NotAllowed => "no permitido para este actor".into(),
        Error::InvalidState => "el recuerdo no está en un estado que lo permita".into(),
        Error::NotFound => "no existe ese recuerdo".into(),
        other => other.to_string(),
    }
}

/// Control characters, bidi marks and zero-width characters become spaces: a memory is one
/// short line of plain text.
pub fn clean_text(s: &str) -> String {
    let swapped: String = s
        .chars()
        .map(|c| {
            if c.is_control()
                || matches!(
                    c,
                    '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{feff}'
                )
            {
                ' '
            } else {
                c
            }
        })
        .collect();
    swapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[derive(Debug, Clone, Copy)]
pub struct Actors {
    pub persona: ActorId,
    pub agente: ActorId,
    pub sesion: ActorId,
    pub importador: ActorId,
}

/// What a run learned, as the closing batch needs it.
#[derive(Debug, Clone, Default)]
pub struct RunFacts {
    pub run_id: String,
    pub instruction: String,
    /// The run reached its final message.
    pub finished: bool,
    pub ending: String,
    /// Names of the files the run changed.
    pub files: Vec<String>,
    pub final_message: Option<String>,
    /// The model read outside content (files, test output): nothing it says is trusted.
    pub tainted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub id: i64,
    pub kind: MemoryType,
    pub state: State,
    pub preview: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloseReport {
    pub saved: Vec<Saved>,
    /// Why something was not saved, one line each.
    pub rejected: Vec<String>,
    /// Proposals of the model beyond the per-run cap.
    pub dropped: usize,
}

impl CloseReport {
    pub fn pending(&self) -> usize {
        self.saved
            .iter()
            .filter(|s| s.state == State::Quarantine)
            .count()
    }

    /// One discreet line for the closing panel.
    pub fn line(&self) -> String {
        let active = self
            .saved
            .iter()
            .filter(|s| s.state == State::Active)
            .count();
        let mut s = format!(
            "memoria: {active} guardado(s), {} en cuarentena",
            self.pending()
        );
        if !self.rejected.is_empty() || self.dropped > 0 {
            s += &format!(", {} rechazado(s)", self.rejected.len() + self.dropped);
        }
        s
    }

    /// Debug lines: what was proposed and what came of it.
    pub fn debug_lines(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .saved
            .iter()
            .map(|s| format!("m{} {:?} {:?}: {}", s.id, s.kind, s.state, s.preview))
            .collect();
        v.extend(self.rejected.iter().cloned());
        if self.dropped > 0 {
            v.push(format!(
                "{} propuesta(s) del modelo ignoradas por pasar del tope",
                self.dropped
            ));
        }
        if v.is_empty() {
            v.push("no se propuso guardar nada".into());
        }
        v
    }
}

/// What the start of a run got from memory.
#[derive(Debug, Clone, Default)]
pub struct Recalled {
    /// Ready for the model (already scrubbed); empty when nothing is worth saying.
    pub text: String,
    pub items: u32,
    pub ids: Vec<i64>,
    pub abstained: bool,
    /// For the debug view: the score of each recalled memory and why.
    pub debug: Vec<String>,
}

pub struct MemoryLink {
    engine: Engine,
    scope: ScopeId,
    actors: Actors,
    writable: bool,
    embeddings: bool,
    embed_error: Option<String>,
}

fn private_dir(dir: &Path) -> Res<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("{}: {e}", dir.display()))
}

#[cfg(feature = "model2vec")]
fn load_embedder(dir: &Path) -> Result<Arc<dyn clio_memory::Embedder>, String> {
    clio_memory::Model2VecEmbedder::from_dir(dir)
        .map(|e| Arc::new(e) as Arc<dyn clio_memory::Embedder>)
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "model2vec"))]
fn load_embedder(_: &Path) -> Result<Arc<dyn clio_memory::Embedder>, String> {
    Err("este Clio se compiló sin la característica model2vec".into())
}

fn quota(max: u32) -> Quota {
    Quota {
        max,
        window_secs: 3600,
    }
}

/// The embedding model: loaded at most once per process (reading it takes seconds, and a run
/// opens the memory twice), and only when something asks for it. Share it with an `Arc`.
pub struct EmbedModel {
    dir: Option<PathBuf>,
    cell: OnceLock<Result<Arc<dyn clio_memory::Embedder>, String>>,
}

impl EmbedModel {
    /// A model in `dir`, or none (`None`): memory then works by text only.
    pub fn new(dir: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            dir,
            cell: OnceLock::new(),
        })
    }

    /// No model, never loads anything.
    pub fn none() -> Arc<Self> {
        Self::new(None)
    }

    /// The model Clio would use in `home`: the local one when it is on disk.
    pub fn of_home(home: &Path) -> Arc<Self> {
        Self::new(Some(MemoryLink::model_dir(home)).filter(|m| m.is_dir()))
    }

    pub fn is_present(&self) -> bool {
        self.dir.is_some()
    }

    fn get(&self) -> Result<Option<Arc<dyn clio_memory::Embedder>>, String> {
        let Some(dir) = &self.dir else {
            return Ok(None);
        };
        self.cell
            .get_or_init(|| load_embedder(dir))
            .clone()
            .map(Some)
    }
}

/// Attaches the embedder when there is a model and it loads.
fn with_model(engine: Engine, model: &EmbedModel) -> (Engine, bool, Option<String>) {
    match model.get() {
        Ok(Some(emb)) => (engine.with_embedder(emb), true, None),
        Ok(None) => (engine, false, None),
        Err(e) => (engine, false, Some(e)),
    }
}

impl MemoryLink {
    pub fn db_path(data: &Path) -> PathBuf {
        data.join(DB_FILE)
    }

    pub fn model_dir(home: &Path) -> PathBuf {
        home.join(MODEL_REL)
    }

    /// Opens (creating on first use) `<data>/memory.db` for writing, with the scope, the four
    /// actors and the fixed blocks in place. `model` is the embedding model directory, if it
    /// should be used. Waits a little for another writer (a run closing while the screen acts).
    pub fn open_write(data: &Path, model: &EmbedModel) -> Res<Self> {
        let path = Self::db_path(data);
        if !data.is_dir() {
            private_dir(data)?;
        }
        let mut engine = None;
        for _ in 0..8 {
            match Engine::open(&path, OpenMode::ReadWrite, Arc::new(SystemClock)) {
                Ok(e) => {
                    engine = Some(e);
                    break;
                }
                Err(Error::WriterLocked) => std::thread::sleep(Duration::from_millis(250)),
                Err(e) => return Err(format!("{}: {}", path.display(), explain(&e))),
            }
        }
        let engine = engine.ok_or_else(|| explain(&Error::WriterLocked))?;
        let (engine, embeddings, embed_error) = with_model(engine, model);
        let m = |e: Error| explain(&e);
        let scope = match engine
            .scopes()
            .map_err(m)?
            .into_iter()
            .find(|s| s.name == SCOPE)
        {
            Some(s) => s.id,
            None => engine.create_scope(SCOPE, "es").map_err(m)?,
        };
        let have = engine.actors().map_err(m)?;
        let get = |name: &str, kind: ActorKind, max: u32| -> Res<ActorId> {
            match have.iter().find(|a: &&ActorInfo| a.name == name) {
                Some(a) => Ok(a.id),
                None => engine.register_actor(name, kind, quota(max)).map_err(m),
            }
        };
        let actors = Actors {
            persona: get("persona", ActorKind::Person, 200)?,
            agente: get("agente", ActorKind::Agent, 20)?,
            sesion: get("sesion", ActorKind::Session, 30)?,
            importador: get("importador", ActorKind::Importer, 100)?,
        };
        let blocks = engine.blocks(scope).map_err(m)?;
        for (name, budget) in BLOCKS {
            if !blocks.iter().any(|b| b.name == name) {
                engine
                    .create_block(scope, name, budget, actors.persona)
                    .map_err(m)?;
            }
        }
        Ok(Self {
            engine,
            scope,
            actors,
            writable: true,
            embeddings,
            embed_error,
        })
    }

    /// Opens for reading when the file exists and was set up; `None` otherwise. Never creates or
    /// changes anything, and does not wait for the writer.
    pub fn open_read(data: &Path, model: &EmbedModel) -> Res<Option<Self>> {
        let path = Self::db_path(data);
        if !path.is_file() {
            return Ok(None);
        }
        let engine = Engine::open(&path, OpenMode::ReadOnly, Arc::new(SystemClock))
            .map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
        let (engine, embeddings, embed_error) = with_model(engine, model);
        let m = |e: Error| explain(&e);
        let Some(scope) = engine
            .scopes()
            .map_err(m)?
            .into_iter()
            .find(|s| s.name == SCOPE)
            .map(|s| s.id)
        else {
            return Ok(None);
        };
        let have = engine.actors().map_err(m)?;
        let id = |name: &str| have.iter().find(|a| a.name == name).map(|a| a.id);
        let (Some(persona), Some(agente), Some(sesion), Some(importador)) =
            (id("persona"), id("agente"), id("sesion"), id("importador"))
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            engine,
            scope,
            actors: Actors {
                persona,
                agente,
                sesion,
                importador,
            },
            writable: false,
            embeddings,
            embed_error,
        }))
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn scope(&self) -> ScopeId {
        self.scope
    }

    pub fn actors(&self) -> Actors {
        self.actors
    }

    pub fn has_embeddings(&self) -> bool {
        self.embeddings
    }

    /// A discreet line about the embedding model, `None` when it is in use.
    pub fn note(&self) -> Option<String> {
        if self.embeddings {
            return None;
        }
        Some(if self.embed_error.is_some() {
            "sin modelo de embeddings (no se pudo cargar): la memoria busca solo por texto".into()
        } else {
            "sin modelo de embeddings: la memoria busca solo por texto".into()
        })
    }

    fn need_write(&self) -> Res<()> {
        if self.writable {
            Ok(())
        } else {
            Err("la memoria está abierta solo para leer".into())
        }
    }

    // ---------------------------------------------------------------- the start of a run

    /// The fixed blocks and what is relevant to `instruction`, ready for the model.
    /// Never fails a run: an error becomes an empty answer with a debug line.
    pub fn recall_for(&self, instruction: &str, scrubber: &Scrubber) -> Recalled {
        let query = clip(&clean_text(instruction), 500);
        let mut out = Recalled::default();
        let recall_opts = RecallOpts {
            search: SearchOpts {
                limit: 6,
                ..SearchOpts::default()
            },
            ..RecallOpts::default()
        };
        match self.engine.recall(self.scope, &query, &recall_opts) {
            Ok(r) => {
                out.abstained = r.abstained;
                if r.abstained {
                    out.debug
                        .push("nada relevante para este pedido (se abstiene)".into());
                }
                for h in &r.hits {
                    out.debug.push(format!(
                        "m{} puntaje {:.4} (texto {}, vector {}, confianza {:.2}): {}",
                        h.memory.id.0,
                        h.score,
                        h.text.map_or("-".into(), |t| format!("{t:.2}")),
                        h.vector.map_or("-".into(), |v| format!("{v:.2}")),
                        h.confidence,
                        clip(&clean_text(&scrubber.scrub(&h.memory.content)), 80)
                    ));
                }
            }
            Err(e) => out
                .debug
                .push(format!("no se pudo recordar: {}", explain(&e))),
        }
        let opts = ContextOpts {
            query: Some(query),
            recall: recall_opts,
            recall_budget: RECALL_BUDGET,
            ..ContextOpts::default()
        };
        match self.engine.context(self.scope, &opts) {
            Ok(text) if !text.trim().is_empty() => {
                // One clean line per memory: the model never sees a control character or a
                // secret, whatever the file holds.
                let text = text
                    .lines()
                    .map(|l| clean_text(&scrubber.scrub(l)))
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                for l in text.lines() {
                    if let Some(id) = l
                        .strip_prefix("[m")
                        .and_then(|r| r.split(']').next())
                        .and_then(|n| n.parse::<i64>().ok())
                    {
                        out.ids.push(id);
                    }
                }
                out.items = u32::try_from(out.ids.len()).unwrap_or(u32::MAX);
                out.text = text;
            }
            Ok(_) => {}
            Err(e) => out
                .debug
                .push(format!("no se pudo armar el contexto: {}", explain(&e))),
        }
        out
    }

    // ---------------------------------------------------------------- the close of a run

    /// The one batch at the end of a run. Never fails: every refusal is a line in the report.
    pub fn close_run(&self, f: &RunFacts, scrubber: &Scrubber) -> CloseReport {
        let mut rep = CloseReport::default();
        if let Err(e) = self.need_write() {
            rep.rejected.push(e);
            return rep;
        }
        let text = |t: &str, max: usize| clip(&clean_text(&scrubber.scrub(t)), max);
        let mut episode = format!("Pedido: {}.", text(&f.instruction, 220));
        if f.files.is_empty() {
            episode += " No cambió archivos.";
        } else {
            let names: Vec<String> = f.files.iter().take(5).map(|n| text(n, 60)).collect();
            episode += &format!(
                " Cambió {} archivo(s): {}{}.",
                f.files.len(),
                names.join(", "),
                if f.files.len() > 5 { ", …" } else { "" }
            );
        }
        episode += &format!(
            " Resultado: {}.",
            if f.finished {
                "terminó".to_owned()
            } else {
                text(&f.ending, 40)
            }
        );
        if let Some(m) = f.final_message.as_deref().filter(|m| !m.trim().is_empty()) {
            episode += &format!(" Dijo: {}", text(m, 240));
        }
        // Active only for a run that never read outside content; external origin = quarantine.
        let origin = if f.tainted {
            Origin::External
        } else {
            Origin::Agent
        };
        self.propose_into(
            &mut rep,
            (self.actors.sesion, MemoryType::Episodic, origin),
            episode,
            f,
            "run",
        );

        // What the model asks to keep: always quarantine, as external; procedures wait on top.
        let mut asked = 0usize;
        for line in f.final_message.as_deref().unwrap_or_default().lines() {
            let t = line.trim();
            let (kind, body) = if let Some(b) = strip_tag(t, "PROCEDIMIENTO:") {
                (MemoryType::Procedural, b)
            } else if let Some(b) = strip_tag(t, "RECORDAR:") {
                (MemoryType::Semantic, b)
            } else {
                continue;
            };
            asked += 1;
            if asked > MAX_AGENT_PROPOSALS {
                rep.dropped += 1;
                continue;
            }
            self.propose_into(
                &mut rep,
                (self.actors.agente, kind, Origin::External),
                text(body, 400),
                f,
                "agente",
            );
        }
        if self.embeddings
            && let Err(e) = self.engine.embed_pending(64)
        {
            rep.rejected
                .push(format!("no se calcularon los vectores: {}", explain(&e)));
        }
        rep
    }

    /// One proposal of the batch; `(actor, kind, origin)` says who writes what and how far to
    /// trust it.
    fn propose_into(
        &self,
        rep: &mut CloseReport,
        (actor, kind, origin): (ActorId, MemoryType, Origin),
        content: String,
        f: &RunFacts,
        who: &str,
    ) {
        let preview = clip(&content, 80);
        let run = clip(&clean_text(&f.run_id), 200);
        let r = self.engine.propose(Proposal {
            scope: self.scope,
            actor,
            kind,
            content,
            topic_key: None,
            origin,
            session: Some(run.clone()),
            episode: Some(run.clone()),
            source: Some(format!("{who}:{run}")),
            valid_from: None,
            valid_to: None,
        });
        match r {
            Ok(o) => rep.saved.push(Saved {
                id: o.memory.0,
                kind,
                state: o.state,
                preview,
            }),
            Err(e) => rep.rejected.push(format!("{kind:?}: {}", explain(&e))),
        }
    }

    // ---------------------------------------------------------------- the person

    /// A fact the person typed: origin `user`, active at once. Optionally in a fixed block.
    pub fn remember(&self, text: &str, block: Option<&str>) -> Res<MemoryId> {
        self.need_write()?;
        // The block is looked up first: a wrong name must not leave a memory behind.
        let block = match block {
            Some(name) => Some(
                self.engine
                    .blocks(self.scope)
                    .map_err(|e| explain(&e))?
                    .into_iter()
                    .find(|b| b.name == name)
                    .ok_or_else(|| {
                        format!("no hay un bloque {name:?} (identidad, reglas, preferencias)")
                    })?,
            ),
            None => None,
        };
        let out = self
            .engine
            .propose(Proposal {
                scope: self.scope,
                actor: self.actors.persona,
                kind: MemoryType::Semantic,
                content: clean_text(text),
                topic_key: None,
                origin: Origin::User,
                session: None,
                episode: None,
                source: Some("persona".into()),
                valid_from: None,
                valid_to: None,
            })
            .map_err(|e| explain(&e))?;
        if let Some(b) = block {
            let mut ids = self.engine.block_get(b.id).map_err(|e| explain(&e))?;
            if !ids.contains(&out.memory) {
                ids.push(out.memory);
            }
            self.engine
                .block_set(b.id, &ids, self.actors.persona)
                .map_err(|e| explain(&e))?;
        }
        Ok(out.memory)
    }

    /// Only the person promotes; the agent has no way to reach this.
    pub fn promote(&self, id: MemoryId) -> Res<()> {
        self.need_write()?;
        self.engine
            .promote(id, self.actors.persona)
            .map(|_| ())
            .map_err(|e| explain(&e))
    }

    /// Deletes the content of a memory for good (PURGE, S-24). Only the person; the agent has no way to
    /// reach this. The reason is kept in the tombstone in the clear: it must not hold the deleted text.
    pub fn purge(&self, id: MemoryId, reason: &str) -> Res<clio_memory::PurgeOutcome> {
        self.need_write()?;
        self.engine
            .purge(id, self.actors.persona, reason)
            .map_err(|e| explain(&e))
    }

    /// What is left of a purged memory: who, when and why, never the content.
    pub fn tombstone(&self, id: MemoryId) -> Res<Option<clio_memory::Tombstone>> {
        self.engine.tombstone(id).map_err(|e| explain(&e))
    }

    pub fn archive(&self, id: MemoryId) -> Res<()> {
        self.need_write()?;
        self.engine
            .archive(id, self.actors.persona)
            .map(|_| ())
            .map_err(|e| explain(&e))
    }

    /// Useful or wrong: a "wrong" can only send a memory to review, never delete or replace it.
    pub fn feedback(&self, id: MemoryId, signal: Signal) -> Res<()> {
        self.need_write()?;
        self.engine
            .feedback(id, signal, self.actors.persona)
            .map_err(|e| explain(&e))
    }

    // ---------------------------------------------------------------- reading

    pub fn get(&self, id: MemoryId) -> Res<MemoryRow> {
        let row = self.engine.get(id).map_err(|e| explain(&e))?;
        if row.scope == self.scope {
            Ok(row)
        } else {
            Err("no existe ese recuerdo".into())
        }
    }

    /// Newest first.
    pub fn list(&self, state: State) -> Res<Vec<MemoryRow>> {
        let mut v = self
            .engine
            .list(self.scope, Some(state), None)
            .map_err(|e| explain(&e))?;
        v.sort_by_key(|r| std::cmp::Reverse(r.id));
        Ok(v)
    }

    /// Waiting for a person: quarantine (which holds the pending procedures too).
    pub fn pending(&self) -> Res<Vec<MemoryRow>> {
        self.list(State::Quarantine)
    }

    /// What `recall` finds, active memories only (quarantine never comes up here).
    pub fn search(&self, query: &str, limit: usize) -> Res<(Vec<MemoryRow>, bool)> {
        let opts = RecallOpts {
            search: SearchOpts {
                limit,
                ..SearchOpts::default()
            },
            ..RecallOpts::default()
        };
        let r = self
            .engine
            .recall(self.scope, &clean_text(query), &opts)
            .map_err(|e| explain(&e))?;
        Ok((r.hits.into_iter().map(|h| h.memory).collect(), r.abstained))
    }

    /// (active, quarantine, archived) of the scope.
    pub fn counts(&self) -> Res<(u64, u64, u64)> {
        let st = self.engine.stats().map_err(|e| explain(&e))?;
        Ok(st
            .scopes
            .iter()
            .find(|s| s.scope == self.scope)
            .map_or((0, 0, 0), |s| (s.active, s.quarantine, s.archived)))
    }

    pub fn actor_name(&self, id: ActorId) -> String {
        self.engine
            .actors()
            .ok()
            .and_then(|a| a.into_iter().find(|x| x.id == id).map(|x| x.name))
            .unwrap_or_else(|| format!("#{}", id.0))
    }
}

fn strip_tag<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let head = line.get(..tag.len())?;
    head.eq_ignore_ascii_case(tag)
        .then(|| line[tag.len()..].trim())
        .filter(|b| !b.is_empty())
}

/// The Spanish word for a kind.
pub fn kind_label(k: MemoryType) -> &'static str {
    match k {
        MemoryType::Working => "trabajo",
        MemoryType::Episodic => "episódico",
        MemoryType::Semantic => "semántico",
        MemoryType::Procedural => "procedimental",
    }
}

pub fn state_label(s: State) -> &'static str {
    match s {
        State::Active => "activo",
        State::Quarantine => "cuarentena",
        State::Archived => "archivado",
        State::Replaced => "reemplazado",
    }
}

pub fn origin_label(o: Origin) -> &'static str {
    match o {
        Origin::User => "persona",
        Origin::Agent => "agente",
        Origin::External => "externo",
    }
}
