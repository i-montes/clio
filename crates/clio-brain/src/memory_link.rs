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
//!   Everything is active at once (the memory is the agent's): the run's own record is episodic,
//!   with origin `agente`, or `externo` if the run touched outside content (`tainted`). What the
//!   model asks to keep (lines `RECORDAR:` or `PROCEDIMIENTO:` in its final message) is always
//!   `externo`: a model that read a hostile file can say anything, so it is read back marked. A
//!   refusal (secret, quota, size) is recorded in the report and never breaks the close.
//! - During a session the agent saves, corrects and forgets ([`MemoryLink::save_as_agent`],
//!   [`MemoryLink::update_as_agent`], [`MemoryLink::forget_as_agent`]); forgetting sends to
//!   quarantine, where the record stays.
//! - Facts of the person only enter through [`MemoryLink::remember`], which no agent code calls.
//!
//! The embedding model is optional: without it everything works by text and `note()` says so.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use clio_memory::{
    ActorId, ActorInfo, ActorKind, Category, ContextOpts, Engine, Error, MemoryId, MemoryRow,
    MemoryType, OpId, OpenMode, Origin, Outcome, Proposal, Quota, RecallOpts, Relation, ScopeId,
    SearchOpts, Signal, State, SystemClock,
};
use clio_vault::Scrubber;

/// How many relation notes a memory's line carries.
const MAX_NOTES: usize = 4;

/// The name the judge (Jev) signs its relations with, as the engine names its actor.
const JUDGE_NAME: &str = "jev";

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
        Error::QuarantineFull => {
            "rechazado: hay demasiados recuerdos de este actor en cuarentena (`clio memory review`)"
                .into()
        }
        Error::EmptyContent => "rechazado: vacío".into(),
        Error::WriterLocked => "otro proceso está escribiendo la memoria; reintenta".into(),
        Error::NotAllowed => "no permitido para este actor".into(),
        Error::InvalidState => "el recuerdo no está en un estado que lo permita".into(),
        Error::NotFound => "no existe ese recuerdo".into(),
        other => other.to_string(),
    }
}

/// The id of a line that starts `[m12]`, as the engine's context lines do.
fn memory_line_id(line: &str) -> Option<i64> {
    line.strip_prefix("[m")?.split(']').next()?.parse().ok()
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

/// The scope of the lessons: what was learned in one project and is worth knowing in any.
pub const LESSONS: &str = "lecciones";
/// A project's scope is this and the project's name.
pub const PROJECT_PREFIX: &str = "proyecto:";
/// The longest text of an agent's memory (`Qué: … · Por qué: … · Dónde: … · Aprendido: …`).
pub const AGENT_TEXT_CHARS: usize = 1200;
/// The longest title.
pub const TITLE_CHARS: usize = 120;

/// Where a memory lives (spec `2026-11-04-memory-engram-design.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// This project's: decisions, architecture, settings.
    Project,
    /// The person's, in every project: who they are, what they prefer.
    Personal,
    /// Learned somewhere, worth knowing anywhere: errors, root causes, patterns, traps.
    Lessons,
}

impl Place {
    pub fn label(self) -> &'static str {
        match self {
            Place::Project => "proyecto",
            Place::Personal => "personal",
            Place::Lessons => "leccion",
        }
    }

    pub fn parse(s: &str) -> Option<Place> {
        [Place::Project, Place::Personal, Place::Lessons]
            .into_iter()
            .find(|p| p.label() == s.trim())
    }

    /// Where a memory goes when nobody (Jev) says: who the person is and what they prefer are the
    /// person's, an error or a
    /// pattern is a lesson, the rest is the project's (or the person's when there is no project).
    pub fn by_rule(category: Option<Category>, has_project: bool) -> Place {
        match category {
            Some(Category::Profile | Category::Preference) => Place::Personal,
            Some(Category::Bugfix | Category::Pattern | Category::Learning) => Place::Lessons,
            _ if has_project => Place::Project,
            _ => Place::Personal,
        }
    }
}

/// What the agent proposes: the text, and optionally a title and a category.
#[derive(Debug, Clone)]
pub struct Draft {
    pub text: String,
    pub title: Option<String>,
    pub category: Option<Category>,
    pub kind: MemoryType,
}

impl Draft {
    pub fn plain(text: &str, kind: MemoryType) -> Self {
        Self {
            text: text.to_owned(),
            title: None,
            category: None,
            kind,
        }
    }
}

pub struct MemoryLink {
    engine: Engine,
    /// The person's scope (`clio`).
    scope: ScopeId,
    /// The project of the session and its scope, when there is one (and, reading, when it exists).
    project: Option<(String, Option<ScopeId>)>,
    lessons: Option<ScopeId>,
    /// Every other project's scope and its name: the memory is global, a session reads them too.
    others: Vec<(ScopeId, String)>,
    person_view: bool,
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

/// The agent's writes an hour: a brake on a loop, not a review (the memory is the agent's).
const AGENT_QUOTA: u32 = 500;

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
        Self::open_write_in(data, model, None)
    }

    /// For writing, in a project: its scope and the lessons' are made if they do not exist yet.
    pub fn open_write_in(data: &Path, model: &EmbedModel, project: Option<&str>) -> Res<Self> {
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
        let scopes = engine.scopes().map_err(m)?;
        let find_or_make = |name: &str| -> Res<ScopeId> {
            match scopes.iter().find(|s| s.name == name) {
                Some(s) => Ok(s.id),
                None => engine.create_scope(name, "es").map_err(m),
            }
        };
        let scope = find_or_make(SCOPE)?;
        let lessons = Some(find_or_make(LESSONS)?);
        let project = match project {
            Some(p) => Some((
                p.to_owned(),
                Some(find_or_make(&format!("{PROJECT_PREFIX}{p}"))?),
            )),
            None => None,
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
            agente: get("agente", ActorKind::Agent, AGENT_QUOTA)?,
            sesion: get("sesion", ActorKind::Session, 30)?,
            importador: get("importador", ActorKind::Importer, 100)?,
        };
        // A file made when the agent had 20 an hour gets the quota of now.
        engine
            .set_quota(actors.agente, quota(AGENT_QUOTA))
            .map_err(m)?;
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
            project,
            lessons,
            others: Vec::new(),
            person_view: false,
            actors,
            writable: true,
            embeddings,
            embed_error,
        }
        .with_others())
    }

    /// Opens for reading when the file exists and was set up; `None` otherwise. Never creates or
    /// changes anything, and does not wait for the writer.
    pub fn open_read(data: &Path, model: &EmbedModel) -> Res<Option<Self>> {
        Self::open_read_in(data, model, None)
    }

    /// For reading, in a project (whose scope may not exist yet: then nothing of it is found).
    pub fn open_read_in(
        data: &Path,
        model: &EmbedModel,
        project: Option<&str>,
    ) -> Res<Option<Self>> {
        let path = Self::db_path(data);
        if !path.is_file() {
            return Ok(None);
        }
        let open = || Engine::open(&path, OpenMode::ReadOnly, Arc::new(SystemClock));
        let engine = match open() {
            // A file of an older schema: a reader never changes it, so it is opened once for
            // writing (which upgrades it, in one transaction) and then read. If another writer
            // holds it, that writer has already upgraded it or will: the read is tried again.
            Err(Error::SchemaTooOld(_)) => {
                drop(Self::open_write(data, &EmbedModel::none())?);
                open()
            }
            other => other,
        }
        .map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
        let (engine, embeddings, embed_error) = with_model(engine, model);
        let m = |e: Error| explain(&e);
        let scopes = engine.scopes().map_err(m)?;
        let find = |name: &str| scopes.iter().find(|s| s.name == name).map(|s| s.id);
        let Some(scope) = find(SCOPE) else {
            return Ok(None);
        };
        let lessons = find(LESSONS);
        let project = project.map(|p| (p.to_owned(), find(&format!("{PROJECT_PREFIX}{p}"))));
        let have = engine.actors().map_err(m)?;
        let id = |name: &str| have.iter().find(|a| a.name == name).map(|a| a.id);
        let (Some(persona), Some(agente), Some(sesion), Some(importador)) =
            (id("persona"), id("agente"), id("sesion"), id("importador"))
        else {
            return Ok(None);
        };
        Ok(Some(
            Self {
                engine,
                scope,
                project,
                lessons,
                others: Vec::new(),
                person_view: false,
                actors: Actors {
                    persona,
                    agente,
                    sesion,
                    importador,
                },
                writable: false,
                embeddings,
                embed_error,
            }
            .with_others(),
        ))
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
                // This loop does not mark the run as tainted when it reads memory, so what came from
                // outside is shown only if the person holds it (accepted it). What the judge made
                // active from the agent's own proposals is for the Pi chat, which shows it as data
                // and marks the session (S-63). The loop's own episodes are not from outside.
                let text = text
                    .lines()
                    .map(|l| clean_text(&scrubber.scrub(l)))
                    .filter(|l| !l.is_empty())
                    .filter(|l| {
                        memory_line_id(l).is_none_or(|id| {
                            self.engine.get(MemoryId(id)).ok().is_some_and(|row| {
                                row.origin != Origin::External
                                    || self.engine.held_by_person(&row).unwrap_or(false)
                            })
                        })
                    })
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
        // Active either way; the origin says whether the run read outside content.
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

        // What the model asks to keep: active, always marked as from outside.
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
            title: None,
            category: None,
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

    /// What the agent saves during a session (the `memory_save` tool): active at once, the agent's,
    /// with origin `externo` when the session read outside content (`tainted`) and `agente`
    /// otherwise. There is no argument for the actor or the scope beyond the place: they are not
    /// the caller's to say (S-61). The engine's secret check and the agent's quota still apply. A
    /// place this link has no scope for (a project when there is none) is the person's.
    pub fn save_as_agent(
        &self,
        draft: &Draft,
        session: &str,
        place: Place,
        tainted: bool,
    ) -> Res<Outcome> {
        self.need_write()?;
        self.engine
            .propose(self.agent_proposal(draft, session, place, tainted))
            .map_err(|e| explain(&e))
    }

    /// The same, in the scope of `beside` (another project's too): what a merge of that place's
    /// memories saves.
    pub fn save_beside(
        &self,
        draft: &Draft,
        session: &str,
        beside: &MemoryRow,
        tainted: bool,
    ) -> Res<Outcome> {
        self.need_write()?;
        let place = self
            .place_of(beside.scope)
            .ok_or("esa memoria no es de este lugar")?;
        let mut p = self.agent_proposal(draft, session, place, tainted);
        p.scope = beside.scope;
        self.engine.propose(p).map_err(|e| explain(&e))
    }

    /// The agent corrects `old` (the `memory_update` tool): the new text replaces exactly that
    /// memory, whoever wrote it, in its place and of its kind. The old one stays in the log as
    /// replaced and the person's revert brings it back.
    pub fn update_as_agent(
        &self,
        old: MemoryId,
        draft: &Draft,
        session: &str,
        tainted: bool,
    ) -> Res<Outcome> {
        self.need_write()?;
        let row = self.get(old)?;
        let place = self
            .place_of(row.scope)
            .ok_or("esa memoria no es de este lugar")?;
        let mut p = self.agent_proposal(draft, session, place, tainted);
        // In the old one's own scope (another project's, too) and of its kind.
        p.scope = row.scope;
        p.kind = row.kind;
        self.engine.replace(old, p).map_err(|e| explain(&e))
    }

    /// The agent pins `id` (the `memory_pin` tool): it comes first in the context.
    pub fn pin_as_agent(&self, id: MemoryId) -> Res<()> {
        self.need_write()?;
        self.get(id)?; // only a memory of this link's places
        self.engine
            .pin(id, self.actors.agente)
            .map_err(|e| explain(&e))
    }

    /// The agent undoes `op` (the `memory_revert` tool): only a change it signed itself, in a
    /// place of this link.
    pub fn revert_as_agent(&self, op: OpId) -> Res<OpId> {
        self.need_write()?;
        let done = self.engine.operation(op).map_err(|e| explain(&e))?;
        if done.actor != self.actors.agente || self.place_of(done.scope).is_none() {
            return Err("ese cambio no lo hizo el agente: no se puede deshacer por aquí".into());
        }
        self.engine
            .revert(op, self.actors.agente)
            .map_err(|e| explain(&e))
    }

    /// The agent forgets `id` (the `memory_forget` tool): to quarantine, out of every search, its
    /// record kept; the person's revert brings it back.
    pub fn forget_as_agent(&self, id: MemoryId) -> Res<OpId> {
        self.need_write()?;
        self.get(id)?; // only a memory of this link's places
        self.engine
            .forget(id, self.actors.agente, "olvidado por el agente")
            .map_err(|e| explain(&e))
    }

    /// The session of an agent's proposal as it is stored.
    pub fn agent_session(session: &str) -> String {
        clip(&clean_text(session), 200)
    }

    /// The text of an agent's proposal as it is stored: clean, one line.
    pub fn agent_text(text: &str) -> String {
        clip(&clean_text(text), AGENT_TEXT_CHARS)
    }

    fn agent_proposal(
        &self,
        draft: &Draft,
        session: &str,
        place: Place,
        tainted: bool,
    ) -> Proposal {
        let session = Self::agent_session(session);
        // Where it was learned: the project, when there is one (what a lesson shows elsewhere).
        let source = match self.project_name() {
            Some(p) => format!("{PROJECT_PREFIX}{p}"),
            None => format!("pi:{session}"),
        };
        Proposal {
            scope: self.scope_of(place).unwrap_or(self.scope),
            actor: self.actors.agente,
            kind: draft.kind,
            content: Self::agent_text(&draft.text),
            topic_key: None,
            origin: if tainted {
                Origin::External
            } else {
                Origin::Agent
            },
            session: Some(session.clone()),
            // The session is the episode: the timeline groups by it.
            episode: Some(session),
            source: Some(source),
            valid_from: None,
            valid_to: None,
            title: draft
                .title
                .as_deref()
                .map(|t| clip(&clean_text(t), TITLE_CHARS))
                .filter(|t| !t.is_empty()),
            category: draft.category,
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
                title: None,
                category: None,
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

    /// First in the context of its place: the person's pin (the agent's is `pin_as_agent`).
    pub fn pin(&self, id: MemoryId) -> Res<()> {
        self.need_write()?;
        self.engine
            .pin(id, self.actors.persona)
            .map_err(|e| explain(&e))
    }

    pub fn unpin(&self, id: MemoryId) -> Res<()> {
        self.need_write()?;
        self.engine
            .unpin(id, self.actors.persona)
            .map_err(|e| explain(&e))
    }

    /// How `source` stands to `target`, as the judge (Jev) said.
    pub fn relate_judged(
        &self,
        source: MemoryId,
        target: MemoryId,
        relation: Relation,
        confidence: f32,
    ) -> Res<()> {
        self.need_write()?;
        self.engine
            .relate(source, target, relation, JUDGE_NAME, confidence)
            .map_err(|e| explain(&e))
    }

    /// What the relations of `row` say, for the model and the person: only those with a memory
    /// this link can read and that is active (a note pointing at nothing readable says nothing),
    /// the newest four.
    pub fn relation_notes(&self, row: &MemoryRow) -> Vec<String> {
        let Ok(rels) = self.engine.relations(row.id) else {
            return Vec::new();
        };
        rels.into_iter()
            .filter_map(|r| {
                let mine = r.source == row.id;
                let other = if mine { r.target } else { r.source };
                let readable = self.get(other).is_ok_and(|o| o.state == State::Active);
                if !readable {
                    return None;
                }
                let n = other.0;
                Some(match (r.relation, mine) {
                    (Relation::ConflictsWith, _) => format!("contradice a m{n}"),
                    (Relation::Compatible, _) => format!("compatible con m{n}"),
                    (Relation::Related, _) => format!("relacionada con m{n}"),
                    (Relation::Scoped, true) => format!("caso concreto de m{n}"),
                    (Relation::Scoped, false) => format!("m{n} es un caso concreto"),
                    (Relation::NotConflict, _) => return None,
                })
            })
            .take(MAX_NOTES)
            .collect()
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
        if self.place_of(row.scope).is_some() {
            Ok(row)
        } else {
            Err("no existe ese recuerdo".into())
        }
    }

    /// The person's own view (the CLI and the TUI): their commands act on every place too.
    #[must_use]
    pub fn for_person(mut self) -> Self {
        self.person_view = true;
        self
    }

    /// Every other project, read after the session's own places (the memory is global; the
    /// project is a mark).
    fn with_others(mut self) -> Self {
        let own = self.scope_of(Place::Project);
        self.others = self
            .engine
            .scopes()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| Some(s.id) != own)
            .filter_map(|s| {
                let name = s.name.strip_prefix(PROJECT_PREFIX)?.to_owned();
                Some((s.id, name))
            })
            .collect();
        self
    }

    /// The scopes the person's commands see: all of them in the person's view, else the person's.
    fn person_scopes(&self) -> Vec<ScopeId> {
        if !self.person_view {
            return vec![self.scope];
        }
        self.reading_scopes()
    }

    /// Each place this view knows, with what it holds: `(label, active, quarantine, archived)`,
    /// the person's first, then the lessons, then every project.
    pub fn places(&self) -> Res<Vec<(String, u64, u64, u64)>> {
        let st = self.engine.stats().map_err(|e| explain(&e))?;
        let mut out: Vec<(u8, String, u64, u64, u64)> = self
            .person_scopes()
            .into_iter()
            .map(|scope| {
                let (order, label) = match self.place_of(scope) {
                    Some(Place::Personal) => (0, "personal".to_owned()),
                    Some(Place::Lessons) => (1, "lecciones".to_owned()),
                    _ => (2, self.project_label(scope)),
                };
                let s = st.scopes.iter().find(|s| s.scope == scope);
                let n = |f: fn(&clio_memory::ScopeStats) -> u64| s.map_or(0, f);
                (
                    order,
                    label,
                    n(|s| s.active),
                    n(|s| s.quarantine),
                    n(|s| s.archived),
                )
            })
            .collect();
        out.sort();
        Ok(out
            .into_iter()
            .map(|(_, l, a, q, r)| (l, a, q, r))
            .collect())
    }

    /// `proyecto X` for a project's scope (the session's own, or another in the person's view).
    fn project_label(&self, scope: ScopeId) -> String {
        match self.others.iter().find(|(id, _)| *id == scope) {
            Some((_, name)) => format!("proyecto {name}"),
            None => match self.project_name() {
                Some(p) => format!("proyecto {p}"),
                None => "proyecto".into(),
            },
        }
    }

    /// What is past its review cycle, in every place this view knows, the oldest first: still
    /// active, only due for a look.
    pub fn review_due(&self) -> Res<Vec<MemoryRow>> {
        let mut due = Vec::new();
        for scope in self.person_scopes() {
            due.extend(self.engine.review_due(scope).map_err(|e| explain(&e))?);
        }
        due.sort_by_key(|r| (r.known_since, r.id));
        Ok(due)
    }

    /// The person looked at it again: its review cycle starts over.
    pub fn reviewed(&self, id: MemoryId) -> Res<()> {
        self.need_write()?;
        self.get(id)?;
        self.engine
            .mark_reviewed(id, self.actors.persona)
            .map_err(|e| explain(&e))
    }

    /// Every memory of project `from` (any state but purged) moves to project `into`, created if
    /// it does not exist: projects merged, or renamed. One revertible operation per memory, signed
    /// by the person. Returns how many moved.
    pub fn merge_projects(&self, from: &str, into: &str) -> Res<usize> {
        self.need_write()?;
        let (from, into) = (crate::project::normal(from), crate::project::normal(into));
        if from.is_empty() || into.is_empty() || from == into {
            return Err("hacen falta dos proyectos distintos".into());
        }
        let scopes = self.engine.scopes().map_err(|e| explain(&e))?;
        let find = |n: &str| {
            scopes
                .iter()
                .find(|s| s.name == format!("{PROJECT_PREFIX}{n}"))
                .map(|s| s.id)
        };
        let src = find(&from).ok_or_else(|| format!("no hay un proyecto {from}"))?;
        let dst = match find(&into) {
            Some(id) => id,
            None => self
                .engine
                .create_scope(&format!("{PROJECT_PREFIX}{into}"), "es")
                .map_err(|e| explain(&e))?,
        };
        let rows = self.engine.list(src, None, None).map_err(|e| explain(&e))?;
        for r in &rows {
            self.engine
                .move_to(r.id, dst, self.actors.persona)
                .map_err(|e| explain(&e))?;
        }
        Ok(rows.len())
    }

    /// Archives every active or pending memory of project `name` (reversible, not an erasure).
    pub fn archive_project(&self, name: &str) -> Res<usize> {
        self.need_write()?;
        let name = crate::project::normal(name);
        let scope = self
            .engine
            .scopes()
            .map_err(|e| explain(&e))?
            .into_iter()
            .find(|s| s.name == format!("{PROJECT_PREFIX}{name}"))
            .ok_or_else(|| format!("no hay un proyecto {name}"))?
            .id;
        let mut n = 0;
        for state in [State::Active, State::Quarantine] {
            for r in self
                .engine
                .list(scope, Some(state), None)
                .map_err(|e| explain(&e))?
            {
                self.engine
                    .archive(r.id, self.actors.persona)
                    .map_err(|e| explain(&e))?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// The operations that touched `id`, oldest first (its versions), if this view can read it.
    pub fn history(&self, id: MemoryId) -> Res<Vec<clio_memory::Operation>> {
        self.get(id)?;
        self.engine.history(id).map_err(|e| explain(&e))
    }

    /// Undoes operation `op` as the person (the engine refuses what cannot be undone).
    pub fn revert(&self, op: OpId) -> Res<OpId> {
        self.need_write()?;
        let done = self.engine.operation(op).map_err(|e| explain(&e))?;
        if self.place_of(done.scope).is_none() {
            return Err("ese cambio no es de esta memoria".into());
        }
        self.engine
            .revert(op, self.actors.persona)
            .map_err(|e| explain(&e))
    }

    /// Pairs of active memories judged to contradict each other, in the places this view knows,
    /// each pair once (the newest relations first).
    pub fn conflicts(&self) -> Res<Vec<(MemoryRow, MemoryRow)>> {
        let mut out: Vec<(MemoryRow, MemoryRow)> = Vec::new();
        for row in self.list(State::Active)? {
            for r in self.engine.relations(row.id).map_err(|e| explain(&e))? {
                if r.relation != Relation::ConflictsWith || r.source != row.id {
                    continue;
                }
                if let Ok(other) = self.get(r.target)
                    && other.state == State::Active
                {
                    out.push((row.clone(), other));
                }
            }
        }
        Ok(out)
    }

    /// Is this active memory past its review cycle? (Only its place is looked at.)
    pub fn is_due(&self, row: &MemoryRow) -> bool {
        self.engine.due(row).unwrap_or(false)
    }

    /// Is it pinned (first in its place's context)?
    pub fn is_pinned(&self, row: &MemoryRow) -> bool {
        self.engine
            .pinned(row.scope)
            .is_ok_and(|v| v.iter().any(|r| r.id == row.id))
    }

    /// The memories around `id` in its session (or its place), as this view can read them.
    pub fn timeline(&self, id: MemoryId, before: usize, after: usize) -> Res<Vec<MemoryRow>> {
        if self.get(id)?.state != State::Active {
            return Err("solo los recuerdos activos tienen línea de tiempo".into());
        }
        Ok(self
            .engine
            .timeline(id, before, after)
            .map_err(|e| explain(&e))?
            .into_iter()
            .filter(|r| self.place_of(r.scope).is_some())
            .collect())
    }

    /// The project of this link, if it has one.
    pub fn project_name(&self) -> Option<&str> {
        self.project.as_ref().map(|(p, _)| p.as_str())
    }

    /// Which of this link's places a scope is (`None`: another project's, or unknown).
    pub fn place_of(&self, scope: ScopeId) -> Option<Place> {
        if scope == self.scope {
            Some(Place::Personal)
        } else if Some(scope) == self.lessons {
            Some(Place::Lessons)
        } else if self.project.as_ref().and_then(|(_, s)| *s) == Some(scope)
            || self.others.iter().any(|(id, _)| *id == scope)
        {
            Some(Place::Project)
        } else {
            None
        }
    }

    /// The scope of a place, if it exists.
    pub fn scope_of(&self, place: Place) -> Option<ScopeId> {
        match place {
            Place::Personal => Some(self.scope),
            Place::Lessons => self.lessons,
            Place::Project => self.project.as_ref().and_then(|(_, s)| *s),
        }
    }

    /// What a session reads: its project first, then the person's, the lessons and every other
    /// project (the memory is global).
    pub fn reading_scopes(&self) -> Vec<ScopeId> {
        [Place::Project, Place::Personal, Place::Lessons]
            .into_iter()
            .filter_map(|p| self.scope_of(p))
            .chain(self.others.iter().map(|(id, _)| *id))
            .collect()
    }

    /// `search` over every place the session reads.
    pub fn search_all(&self, query: &str, limit: usize) -> Res<(Vec<MemoryRow>, bool)> {
        let opts = RecallOpts {
            search: SearchOpts {
                limit,
                ..SearchOpts::default()
            },
            ..RecallOpts::default()
        };
        let r = self
            .engine
            .recall_many(&self.reading_scopes(), &clean_text(query), &opts)
            .map_err(|e| explain(&e))?;
        Ok((r.hits.into_iter().map(|h| h.memory).collect(), r.abstained))
    }

    /// `search` in one place (where a proposal would go: its neighbors).
    pub fn search_in(&self, place: Place, query: &str, limit: usize) -> Res<Vec<MemoryRow>> {
        let Some(scope) = self.scope_of(place) else {
            return Ok(Vec::new());
        };
        let opts = RecallOpts {
            search: SearchOpts {
                limit,
                ..SearchOpts::default()
            },
            ..RecallOpts::default()
        };
        self.engine
            .recall(scope, &clean_text(query), &opts)
            .map(|r| r.hits.into_iter().map(|h| h.memory).collect())
            .map_err(|e| explain(&e))
    }

    /// A label for where a memory lives, for the model: `proyecto`, `personal`, `lección de X`.
    pub fn place_label(&self, row: &MemoryRow) -> String {
        match self.place_of(row.scope) {
            Some(Place::Project) => match self.others.iter().find(|(id, _)| *id == row.scope) {
                Some((_, name)) => format!("proyecto {name}"),
                None => "proyecto".into(),
            },
            Some(Place::Personal) => "personal".into(),
            Some(Place::Lessons) => match Self::learned_in(row) {
                Some(p) => format!("lección de {p}"),
                None => "lección".into(),
            },
            None => String::new(),
        }
    }

    /// The project a lesson was learned in (its `source`), if it says.
    pub fn learned_in(row: &MemoryRow) -> Option<&str> {
        row.source.as_deref()?.strip_prefix(PROJECT_PREFIX)
    }

    /// Newest first.
    pub fn list(&self, state: State) -> Res<Vec<MemoryRow>> {
        let mut v = Vec::new();
        for scope in self.person_scopes() {
            v.extend(
                self.engine
                    .list(scope, Some(state), None)
                    .map_err(|e| explain(&e))?,
            );
        }
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
            .recall_many(&self.person_scopes(), &clean_text(query), &opts)
            .map_err(|e| explain(&e))?;
        Ok((r.hits.into_iter().map(|h| h.memory).collect(), r.abstained))
    }

    /// (active, quarantine, archived) of the scope (of every place, in the person's view).
    pub fn counts(&self) -> Res<(u64, u64, u64)> {
        let st = self.engine.stats().map_err(|e| explain(&e))?;
        let scopes = self.person_scopes();
        Ok(st
            .scopes
            .iter()
            .filter(|s| scopes.contains(&s.scope))
            .fold((0, 0, 0), |(a, q, r), s| {
                (a + s.active, q + s.quarantine, r + s.archived)
            }))
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
