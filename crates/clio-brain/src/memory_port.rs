//! The tool host's window into the memory, which is the agent's: it reads the active memories of
//! the places the session reads, and saves, corrects, forgets (to quarantine), pins, merges and
//! undoes its own changes, all at once (S-60, S-61). With Jev, what is written is guarded first:
//! a clear «no» refuses it, anything else saves (S-64). The tool host process runs this. Reading opens the database with [`MemoryLink::open_read`], which never
//! creates or changes a file and never takes the writer lock, so the person's `clio memory
//! remember` is not blocked while the agent reads. A proposal opens it for writing for that one
//! call and closes it: the writer lock is held for as long as the engine lives, and holding it
//! between calls would lock the person out.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use clio_memory::{
    Category, MemoryId, MemoryRow, MemoryType, OpId, OpKind, Origin as Written, Relation, State,
};
use clio_toolhost::{
    Found, Judging, MemoryItem, MemoryPort, Near, Origin, ProposalDraft, ProposalKind, Proposed,
    Saved,
};

use crate::memory_judge::{Guard, MemoryJudge};
use crate::memory_link::{Draft, EmbedModel, MemoryLink, Place, Res, kind_label};

/// One text for every reason a memory cannot be read: missing, in quarantine, archived or of
/// another scope. Which one it is must not leak.
const NOT_READABLE: &str = "ese recuerdo no existe o no se puede leer";
const NO_VECTORS: &str = "solo texto: el modelo de vectores no está disponible";

pub struct LinkPort {
    data: PathBuf,
    model: Arc<EmbedModel>,
    /// Opened on the first call that finds a database, and kept. A failure is never kept: the
    /// person may create the memory (or fix it) while the session runs.
    link: Mutex<Option<MemoryLink>>,
    /// Jev, who guards what is written and chooses its place. Optional: without it everything saves.
    judge: Option<Arc<MemoryJudge>>,
    /// The project of the session, decided by the brain: its scope is read with the person's and
    /// the lessons', and what belongs to it is written there.
    project: Option<String>,
}

impl LinkPort {
    /// `model_dirs` are the vector models, best first (none: text); one is only loaded when a
    /// search needs it.
    pub fn new(data: &Path, model_dirs: &[PathBuf]) -> Self {
        Self {
            data: data.to_path_buf(),
            model: EmbedModel::with_dirs(model_dirs.to_vec()),
            link: Mutex::new(None),
            judge: None,
            project: None,
        }
    }

    /// In a project (the brain's, never the model's).
    #[must_use]
    pub fn in_project(mut self, project: Option<String>) -> Self {
        self.project = project;
        self
    }

    /// With this vector model (the one `new` was given, by default).
    #[must_use]
    pub fn with_model(mut self, model: Arc<EmbedModel>) -> Self {
        self.model = model;
        self
    }

    /// With Jev guarding what is written.
    #[must_use]
    pub fn with_judge(mut self, judge: Arc<MemoryJudge>) -> Self {
        self.judge = Some(judge);
        self
    }

    fn with_link<T>(&self, f: impl FnOnce(&MemoryLink) -> Res<T>) -> Res<T> {
        // A panic in an earlier call leaves nothing half-done in a read-only link.
        let mut slot = self.link.lock().unwrap_or_else(PoisonError::into_inner);
        // A place made after the link was opened (by a proposal here or anywhere else) is only
        // seen by a new link: one that lacks a place is opened again.
        let stale = slot.as_ref().is_some_and(|l| {
            l.scope_of(Place::Lessons).is_none()
                || (self.project.is_some() && l.scope_of(Place::Project).is_none())
        });
        if slot.is_none() || stale {
            match MemoryLink::open_read_in(&self.data, &self.model, self.project.as_deref()) {
                Ok(Some(link)) => *slot = Some(link),
                Ok(None) => return Err("la memoria no existe todavía".into()),
                Err(why) => {
                    // The detail names a path of the person's: it goes to the service's log, not
                    // to the model.
                    eprintln!("clio-toolhost: memory: {why}");
                    return Err("la memoria no se puede abrir ahora".into());
                }
            }
        }
        slot.as_ref().map_or_else(|| Err(NOT_READABLE.into()), f)
    }
}

impl LinkPort {
    /// Opened for writing for this one call (it waits a few seconds for another writer).
    fn writer(&self) -> Result<MemoryLink, String> {
        MemoryLink::open_write_in(&self.data, &self.model, self.project.as_deref()).map_err(|why| {
            eprintln!("clio-toolhost: memory: {why}");
            "la memoria no se puede abrir para escribir ahora".to_owned()
        })
    }

    /// Reads without holding the writer: what Jev is asked about must not keep the person's own
    /// writes waiting.
    fn reader(&self) -> Result<MemoryLink, String> {
        match MemoryLink::open_read_in(&self.data, &EmbedModel::none(), self.project.as_deref()) {
            Ok(Some(link)) => Ok(link),
            _ => Err(NOT_READABLE.to_owned()),
        }
    }

    /// Asks Jev (when there is one) about a text before it is written: a clear «no» is the reason
    /// the agent hears; otherwise the category to keep (Jev's when it was sure).
    fn guard(&self, draft: &Draft, judging: &Judging) -> Result<Option<Category>, String> {
        let Some(judge) = &self.judge else {
            return Ok(draft.category);
        };
        match judge.guard(
            &MemoryLink::agent_text(&draft.text),
            draft.title.as_deref(),
            draft.category.map(Category::label),
            judging.tainted,
        ) {
            Guard::Refuse { why } => Err(why.to_owned()),
            Guard::Allow { category } => Ok(category),
            Guard::Unsure => Ok(draft.category),
        }
    }
}

impl LinkPort {
    /// A save, or (with `target`) a new text for that memory.
    fn propose_to(
        &self,
        draft: &ProposalDraft,
        session: &str,
        judging: &Judging,
        target: Option<MemoryRow>,
    ) -> Result<Proposed, String> {
        let summary = draft.kind == ProposalKind::Session;
        let mut draft = if summary {
            // A session's summary is named here, has no category and is never a lesson.
            Draft {
                text: draft.text.clone(),
                title: Some(format!("Sesión {}", civil_date(now_secs()))),
                category: None,
                kind: MemoryType::Episodic,
            }
        } else {
            Draft {
                text: draft.text.clone(),
                title: draft.title.clone(),
                category: draft.category.as_deref().and_then(Category::parse),
                kind: match draft.kind {
                    ProposalKind::Procedural => MemoryType::Procedural,
                    _ => MemoryType::Semantic,
                },
            }
        };
        let has_project = self.project.is_some();
        // Jev is asked BEFORE the writer is taken: a slow answer never keeps the person's own
        // writes waiting.
        let category = self.guard(&draft, judging)?;
        // A summary has no category, whatever Jev would give it.
        if !summary {
            draft.category = category;
        }
        let stored = MemoryLink::agent_text(&draft.text);
        let place = self
            .judge
            .as_ref()
            .filter(|_| has_project && !summary && target.is_none())
            .and_then(|j| {
                j.place(
                    &stored,
                    draft.title.as_deref(),
                    draft.category.map(Category::label),
                )
            })
            .and_then(Place::parse)
            .unwrap_or_else(|| match (summary, has_project) {
                (true, true) => Place::Project,
                (true, false) => Place::Personal,
                _ => Place::by_rule(draft.category, has_project),
            });
        let link = self.writer()?;
        let o = match &target {
            Some(t) => link.update_as_agent(t.id, &draft, session, judging.tainted)?,
            None => link.save_as_agent(&draft, session, place, judging.tainted)?,
        };
        embed_now(&link);
        let similar = match o.kind {
            OpKind::Noop => None,
            _ => link.near_duplicate(o.memory).as_ref().map(near_of),
        };
        drop(link);
        if let (OpKind::Add, Some(judge), false) = (o.kind, &self.judge, summary) {
            self.relate_nearest(judge, o.memory);
        }
        Ok(Proposed {
            similar,
            id: o.memory.0,
            saved: match o.kind {
                OpKind::Noop => Saved::Known,
                OpKind::Update => Saved::Replaced,
                _ => Saved::Active,
            },
            change: (o.kind != OpKind::Noop).then_some(o.op.0),
        })
    }
}

impl LinkPort {
    /// Something new next to its nearest memories: Jev says how each pair stands, and only what
    /// `JEV_TYPES` allows is noted (best effort). Jev is asked with no writer held.
    fn relate_nearest(&self, judge: &MemoryJudge, id: MemoryId) {
        let Ok((text, near)) = self.with_link(|l| {
            let row = l.get(id)?;
            let near = l.neighbors(&row, JEV_NEIGHBORS);
            Ok((row.content, near))
        }) else {
            return;
        };
        let found: Vec<_> = near
            .into_iter()
            .filter_map(|n| {
                judge
                    .relation(&text, &n.content)
                    .filter(|(r, sure)| JEV_TYPES.contains(r) && *sure >= JEV_SURE)
                    .map(|(r, sure)| (n.id, r, sure))
            })
            .collect();
        if found.is_empty() {
            return;
        }
        if let Ok(link) = self.writer() {
            for (other, r, sure) in found {
                let _ = link.relate_judged(id, other, r, sure);
            }
        }
    }
}

impl MemoryPort for LinkPort {
    fn propose(
        &self,
        draft: &ProposalDraft,
        session: &str,
        judging: &Judging,
    ) -> Result<Proposed, String> {
        self.propose_to(draft, session, judging, None)
    }

    /// A new text for memory `id`, whoever wrote it: it replaces it at once, in its place and of
    /// its kind; the old one stays in the log. What the model leaves out stays as it was.
    fn update(
        &self,
        id: i64,
        draft: &ProposalDraft,
        session: &str,
        judging: &Judging,
    ) -> Result<Proposed, String> {
        let target = self
            .reader()?
            .get(MemoryId(id))
            .ok()
            .filter(|r| r.state == State::Active)
            .ok_or_else(|| NOT_READABLE.to_owned())?;
        let draft = ProposalDraft {
            title: draft.title.clone().or_else(|| target.title.clone()),
            category: draft
                .category
                .clone()
                .or_else(|| target.category.map(|c| c.label().to_owned())),
            ..draft.clone()
        };
        self.propose_to(&draft, session, judging, Some(target))
    }

    /// One memory instead of several: all of them active facts in one place this session reads.
    /// Jev guards the merged text; it is saved there and the originals are forgotten (quarantine,
    /// restorable). Read-only checks come first.
    fn consolidate(
        &self,
        ids: &[i64],
        draft: &ProposalDraft,
        session: &str,
        judging: &Judging,
    ) -> Result<Proposed, String> {
        // All of one scope (one project, not just «a project»), and the merge is saved there.
        let first = {
            let link = self.reader()?;
            let mut first: Option<MemoryRow> = None;
            for id in ids {
                let r = link
                    .get(MemoryId(*id))
                    .ok()
                    .filter(|r| r.state == State::Active && r.kind == MemoryType::Semantic)
                    .ok_or_else(|| NOT_READABLE.to_owned())?;
                if first.as_ref().is_some_and(|f| f.scope != r.scope) {
                    return Err("solo se unen memorias del mismo sitio".into());
                }
                first.get_or_insert(r);
            }
            first.ok_or_else(|| NOT_READABLE.to_owned())?
        };
        let mut merged = Draft {
            text: draft.text.clone(),
            title: draft.title.clone(),
            category: draft.category.as_deref().and_then(Category::parse),
            kind: MemoryType::Semantic,
        };
        merged.category = self.guard(&merged, judging)?;
        let link = self.writer()?;
        for id in ids {
            // Checked again under the writer: still active, still in that scope.
            link.get(MemoryId(*id))
                .ok()
                .filter(|r| r.state == State::Active && r.scope == first.scope)
                .ok_or_else(|| NOT_READABLE.to_owned())?;
        }
        let o = link.save_beside(&merged, session, &first, judging.tainted)?;
        // A new memory, active, not one of them (the engine answers a repeat with the existing
        // one): otherwise nothing is forgotten.
        if o.kind != OpKind::Add || o.state != State::Active || ids.contains(&o.memory.0) {
            return Err("la unión no dio una memoria nueva y activa: no se olvidó nada".into());
        }
        for id in ids {
            // Best effort: the merge is made; a relation that does not follow is a note lost.
            let _ = link.copy_relations_as_agent(MemoryId(*id), o.memory);
        }
        for (forgotten, id) in ids.iter().enumerate() {
            if let Err(why) = link.forget_as_agent(MemoryId(*id)) {
                return Err(format!(
                    "se guardó m{} y se olvidaron {forgotten} de {}; falló una: {why}",
                    o.memory.0,
                    ids.len()
                ));
            }
        }
        embed_now(&link);
        // No change number: undoing only the new one would leave the originals forgotten.
        Ok(Proposed {
            id: o.memory.0,
            saved: Saved::Active,
            change: None,
            similar: None,
        })
    }

    fn pin(&self, id: i64, _judging: &Judging) -> Result<(), String> {
        self.writer()?
            .pin_as_agent(MemoryId(id))
            .map_err(|_| NOT_READABLE.to_owned())
    }

    fn forget(&self, id: i64, _judging: &Judging) -> Result<(), String> {
        self.writer()?
            .forget_as_agent(MemoryId(id))
            .map(|_| ())
            .map_err(|_| NOT_READABLE.to_owned())
    }

    /// Only a change the agent signed itself; anything else is the same answer as a change that
    /// does not exist.
    fn undo(&self, change: i64, _judging: &Judging) -> Result<(), String> {
        self.writer()?
            .revert_as_agent(OpId(change))
            .map(|_| ())
            .map_err(|_| NOT_READABLE.to_owned())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Found, String> {
        self.with_link(|link| {
            // The engine's own message may repeat the query; the model gets a fixed one.
            let (rows, abstained) = link
                .search_all(query, limit)
                .map_err(|_| "la búsqueda en la memoria falló".to_owned())?;
            Ok(Found {
                items: rows.iter().map(|r| item(link, r, NEAR_SEARCH)).collect(),
                abstained,
                note: (!link.has_embeddings()).then(|| NO_VECTORS.to_owned()),
            })
        })
    }

    fn get(&self, id: i64) -> Result<MemoryItem, String> {
        self.with_link(|link| {
            // `MemoryLink::get` already refuses another scope; its text is dropped with the rest.
            match link.get(MemoryId(id)) {
                Ok(row) if row.state == State::Active && link.place_of(row.scope).is_some() => {
                    Ok(item(link, &row, NEAR_GET))
                }
                _ => Err(NOT_READABLE.into()),
            }
        })
    }

    fn timeline(&self, id: i64, before: usize, after: usize) -> Result<Vec<MemoryItem>, String> {
        self.with_link(|link| {
            match link.get(MemoryId(id)) {
                Ok(row) if row.state == State::Active && link.place_of(row.scope).is_some() => {}
                _ => return Err(NOT_READABLE.into()),
            }
            let rows = link
                .engine()
                .timeline(MemoryId(id), before, after)
                .map_err(|_| NOT_READABLE.to_owned())?;
            Ok(rows
                .iter()
                .filter(|r| link.place_of(r.scope).is_some())
                .map(|r| item(link, r, 0))
                .collect())
        })
    }

    fn morning(&self) -> Option<String> {
        let report = clio_memory::sleep::last_report(&self.data)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        clio_memory::sleep::morning_line(&report, i64::try_from(now).ok()?)
    }

    /// The last session's summary of the project (or the person's place), what is pinned in any
    /// place, and the most recent of the project: what a model needs after a compaction.
    fn context(&self) -> Result<Vec<MemoryItem>, String> {
        self.with_link(|link| {
            let engine = link.engine();
            let home = link
                .scope_of(Place::Project)
                .unwrap_or_else(|| link.scope());
            let recent = engine
                .recent(home, 30)
                .map_err(|_| "la memoria no se pudo leer".to_owned())?;
            let mut out: Vec<MemoryRow> = Vec::new();
            if let Some(summary) = recent.iter().find(|r| r.kind == MemoryType::Episodic) {
                out.push(summary.clone());
            }
            for scope in link.reading_scopes() {
                for r in engine.pinned(scope).unwrap_or_default() {
                    if !out.iter().any(|o| o.id == r.id) {
                        out.push(r);
                    }
                }
            }
            for r in recent.into_iter().take(8) {
                if !out.iter().any(|o| o.id == r.id) {
                    out.push(r);
                }
            }
            // What is past its review cycle in the places the session reads: a few, so the model
            // can say so to the person (they review it with `clio memory review-due`).
            let mut due: Vec<MemoryRow> = Vec::new();
            for scope in link.reading_scopes() {
                due.extend(engine.review_due(scope).unwrap_or_default());
            }
            for r in due.into_iter().take(3) {
                if !out.iter().any(|o| o.id == r.id) {
                    out.push(r);
                }
            }
            Ok(out.iter().map(|r| item(link, r, 0)).collect())
        })
    }
}

/// Right after a write, the vectors of the whole queue (best effort: what fails waits for the next
/// write, the close of a run, or `clio memory`).
fn embed_now(link: &MemoryLink) {
    if let Err(why) = link.embed_queue() {
        eprintln!("clio-toolhost: memory: vectores pendientes: {why}");
    }
}

/// What Jev may note between a new memory and its nearest, and how sure it must be: on clio-memory's
/// bench/relations it found the 3 contradictions at 0.95 or more and its one false one at 0.77, and
/// its other kinds were right 10 % of the time or less (bench/relations/results.md).
const JEV_TYPES: &[Relation] = &[Relation::ConflictsWith];
const JEV_SURE: f32 = 0.9;
const JEV_NEIGHBORS: usize = 3;

/// Neighbors given with a memory read in full, and with each search result (only their ids show).
const NEAR_GET: usize = 5;
const NEAR_SEARCH: usize = 3;

fn origin_of(row: &MemoryRow) -> Origin {
    match row.origin {
        Written::User => Origin::Person,
        Written::Agent => Origin::Agent,
        Written::External => Origin::External,
    }
}

/// Its title, else its text, whole: the tool host scrubs vault values before it cuts, and a cut
/// here first could leave half of one that the scrubber no longer recognizes.
fn near_of(row: &MemoryRow) -> Near {
    Near {
        id: row.id.0,
        origin: origin_of(row),
        preview: row.title.clone().unwrap_or_else(|| row.content.clone()),
    }
}

/// `near`: how many of its neighbors go with it (0: none).
fn item(link: &MemoryLink, row: &MemoryRow, near: usize) -> MemoryItem {
    MemoryItem {
        id: row.id.0,
        kind: kind_label(row.kind).to_owned(),
        origin: origin_of(row),
        date: civil_date(row.valid_from),
        text: row.content.clone(),
        title: row.title.clone(),
        category: row.category.map(|c| c.label().to_owned()),
        place: link.place_label(row),
        notes: {
            let mut n = link.relation_notes(row);
            // Still active, but old enough that it may no longer hold: the model checks it.
            if link.is_due(row) {
                n.insert(0, "revisar: pasó su plazo, puede estar desfasado".into());
            }
            n
        },
        near: if near > 0 {
            link.neighbors(row, near).iter().map(near_of).collect()
        } else {
            Vec::new()
        },
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// `YYYY-MM-DD` (UTC) of seconds since the epoch. Days to a civil date as in H. Hinnant's
/// `civil_from_days`; no calendar crate for one line of text.
fn civil_date(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::civil_date;

    #[test]
    fn dates_come_from_seconds_since_the_epoch() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(-1), "1969-12-31");
        // 2024-02-29 (a leap day) at 23:59:59, and the next second.
        assert_eq!(civil_date(1_709_251_199), "2024-02-29");
        assert_eq!(civil_date(1_709_251_200), "2024-03-01");
        // 2026-11-03, a known day.
        assert_eq!(civil_date(1_793_664_000), "2026-11-03");
    }
}
