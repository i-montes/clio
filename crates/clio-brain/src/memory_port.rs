//! The tool host's window into the person's memory: it reads the active memories of the scope
//! `clio` and proposes candidates into quarantine, nothing else (S-60, S-61). The tool host
//! process runs this. Reading opens the database with [`MemoryLink::open_read`], which never
//! creates or changes a file and never takes the writer lock, so the person's `clio memory
//! remember` is not blocked while the agent reads. A proposal opens it for writing for that one
//! call and closes it: the writer lock is held for as long as the engine lives, and holding it
//! between calls would lock the person out.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use clio_memory::{
    Category, Error, Judged, MemoryId, MemoryRow, MemoryType, OpId, OpKind, Origin as Written,
    State,
};
use clio_toolhost::{
    Found, Judging, MemoryItem, MemoryPort, Origin, ProposalDraft, ProposalKind, Proposed, Saved,
};

use crate::memory_judge::{Call, MemoryJudge, Neighbor, Request};
use crate::memory_link::{Draft, EmbedModel, MemoryLink, Place, Res, explain, kind_label};

/// One text for every reason a memory cannot be read: missing, in quarantine, archived or of
/// another scope. Which one it is must not leak.
const NOT_READABLE: &str = "ese recuerdo no existe o no se puede leer";
/// Said when the person's own words do not show that they asked for the change.
const NOT_CONFIRMED: &str =
    "no consta, por lo que la persona dijo, que lo haya pedido: que lo haga con `clio memory`";
const NO_JUDGE: &str =
    "esta memoria no puede hacer ese cambio sin Jev: que lo haga la persona con `clio memory`";
const NO_VECTORS: &str = "solo texto: el modelo de vectores no está disponible";

pub struct LinkPort {
    data: PathBuf,
    model: Arc<EmbedModel>,
    /// Opened on the first call that finds a database, and kept. A failure is never kept: the
    /// person may create the memory (or fix it) while the session runs.
    link: Mutex<Option<MemoryLink>>,
    /// Jev, who decides what becomes of a proposal. Without it every proposal waits for the person.
    judge: Option<Arc<MemoryJudge>>,
    /// The project of the session, decided by the brain: its scope is read with the person's and
    /// the lessons', and what belongs to it is written there.
    project: Option<String>,
}

impl LinkPort {
    /// `model_dir` is the vector model, if there is one; it is only loaded when a search needs it.
    pub fn new(data: &Path, model_dir: Option<&Path>) -> Self {
        Self {
            data: data.to_path_buf(),
            model: EmbedModel::new(model_dir.map(Path::to_path_buf)),
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

    /// With a judge, a proposal is decided by it (and by the rules of the engine behind it).
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
        MemoryLink::open_write_in(&self.data, &EmbedModel::none(), self.project.as_deref()).map_err(
            |why| {
                eprintln!("clio-toolhost: memory: {why}");
                "la memoria no se puede abrir para escribir ahora".to_owned()
            },
        )
    }

    /// Reads without holding the writer: what Jev is asked about must not keep the person's own
    /// writes waiting.
    fn reader(&self) -> Result<MemoryLink, String> {
        match MemoryLink::open_read_in(&self.data, &EmbedModel::none(), self.project.as_deref()) {
            Ok(Some(link)) => Ok(link),
            _ => Err(NOT_READABLE.to_owned()),
        }
    }

    /// A change the person asked for through the model: the memory must be there, the judge must
    /// confirm from the person's own words that they asked for exactly this about exactly it, and
    /// only then is it done (by the person's actor, like `clio memory` would). The judge is asked
    /// first, with a read-only view; the writer is taken only to do it, and the memory is read
    /// again under it.
    fn change(
        &self,
        id: i64,
        judging: &Judging,
        what: Request,
        allowed: impl Fn(&MemoryRow) -> bool,
        act: impl FnOnce(&MemoryLink, &MemoryRow) -> Res<()>,
    ) -> Result<(), String> {
        let judge = self.judge.as_ref().ok_or(NO_JUDGE)?;
        // Checked on the read-only view BEFORE Jev is asked: a memory this change cannot reach is
        // the same answer as one that does not exist, and its text never leaves.
        let seen = self
            .reader()?
            .get(MemoryId(id))
            .ok()
            .filter(|r| allowed(r))
            .ok_or_else(|| NOT_READABLE.to_owned())?;
        if !judge.confirm(what, &seen.content, &judging.person_said, judging.tainted) {
            return Err(NOT_CONFIRMED.to_owned());
        }
        let link = self.writer()?;
        let row = link
            .get(MemoryId(id))
            .ok()
            .filter(|r| allowed(r))
            .ok_or_else(|| NOT_READABLE.to_owned())?;
        act(&link, &row)
    }
}

impl MemoryPort for LinkPort {
    fn propose(
        &self,
        draft: &ProposalDraft,
        session: &str,
        judging: &Judging,
    ) -> Result<Proposed, String> {
        let summary = draft.kind == ProposalKind::Session;
        let draft = if summary {
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
        let (text, kind) = (draft.text.as_str(), draft.kind);
        let has_project = self.project.is_some();
        // Where it goes and what Jev makes of it are asked BEFORE the writer is taken: what Jev is
        // asked about is read without holding the lock, so a slow answer never keeps the person's
        // own writes waiting. The engine re-checks every memory it is told to replace once it
        // holds the lock.
        let stored = MemoryLink::agent_text(text);
        let place = self
            .judge
            .as_ref()
            .filter(|_| has_project && !summary)
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
        // A summary with no project would be the person's and show up everywhere: the person
        // decides on it, never Jev.
        let verdict = self
            .judge
            .as_ref()
            .filter(|_| !summary || has_project)
            .map(|judge| {
                // A summary is about its own session: nothing to replace or to have known already.
                let neighbors: Vec<Neighbor> = match MemoryLink::open_read_in(
                    &self.data,
                    &EmbedModel::none(),
                    self.project.as_deref(),
                ) {
                    Ok(Some(link)) if !summary => link
                        .search_in(place, &stored, 3)
                        .map(|rows| {
                            rows.into_iter()
                                .map(|r| Neighbor {
                                    id: r.id.0,
                                    // A lesson learned in another project is not this session's to
                                    // replace: only the person's words would (as with their own).
                                    protected: link.is_protected(&r)
                                        || (place == Place::Lessons
                                            && MemoryLink::learned_in(&r)
                                                != self.project.as_deref()),
                                    text: r.content,
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let v = judge.decide(
                    &stored,
                    kind == MemoryType::Procedural,
                    &neighbors,
                    judging.tainted,
                    &judging.person_said,
                );
                // Something new next to a memory it does not replace: how the two stand (asked
                // now, before the writer, like everything Jev is asked).
                let relation = match (&v.call, neighbors.first()) {
                    // Never on what the person holds: the model chose the text, so it could choose
                    // which of their memories gets a «contradice».
                    (Call::Add, Some(n)) if !n.protected => {
                        judge.relation(&stored, &n.text).map(|r| (n.id, r))
                    }
                    _ => None,
                };
                (v, relation)
            });
        // Opened for this call only (it waits a few seconds for another writer), dropped at the end.
        let link = self.writer()?;
        if let Some((v, relation)) = verdict {
            let how = match v.call {
                Call::Add => Some(Judged::Add),
                Call::Update {
                    old,
                    person_confirmed,
                } => Some(Judged::Update {
                    old: MemoryId(old),
                    person_confirmed,
                }),
                Call::Noop { existing } => Some(Judged::Noop {
                    existing: MemoryId(existing),
                }),
                Call::Review => None,
            };
            if let Some(how) = how {
                match link.propose_judged_draft(&draft, session, place, how, v.confidence) {
                    Ok(o) => {
                        if let (OpKind::Add, State::Active, Some((other, (rel, sure)))) =
                            (o.kind, o.state, relation)
                        {
                            // Best effort: a relation is a note on the memory, not the memory.
                            let _ = link.relate_judged(o.memory, MemoryId(other), rel, sure);
                        }
                        let saved = match (o.kind, o.state) {
                            (OpKind::Noop, _) => Saved::Known,
                            (OpKind::Update, State::Active) => Saved::Replaced,
                            (_, State::Active) => Saved::Active,
                            _ => Saved::Waiting,
                        };
                        return Ok(Proposed {
                            id: o.memory.0,
                            saved,
                            change: matches!(saved, Saved::Active | Saved::Replaced)
                                .then_some(o.op.0),
                        });
                    }
                    // A refusal that is the proposal's own (a secret, a quota, too long, empty,
                    // another writer) is said as it is; anything else the judge asked for and the
                    // engine could not do (a neighbor that stopped being active, one the person
                    // owns, a date, a judge that cannot be registered) leaves the proposal to the
                    // person, as it was before the judge.
                    Err(
                        e @ (Error::SecretDetected
                        | Error::QuotaExceeded
                        | Error::TooLong
                        | Error::EmptyContent
                        | Error::WriterLocked),
                    ) => return Err(explain(&e)),
                    Err(_) => {}
                }
            }
        }
        link.propose_draft_as_agent(&draft, session, place)
            .map(|id| Proposed {
                id: id.0,
                saved: Saved::Waiting,
                change: None,
            })
    }

    /// Pins a memory the session can read, because the person asked (Jev checks their words).
    fn pin(&self, id: i64, judging: &Judging) -> Result<(), String> {
        self.change(
            id,
            judging,
            Request::Pin,
            |r| r.state == State::Active,
            |link, row| link.pin(row.id),
        )
    }

    fn forget(&self, id: i64, judging: &Judging) -> Result<(), String> {
        self.change(
            id,
            judging,
            Request::Forget,
            |r| r.state == State::Active,
            |link, row| link.archive(row.id),
        )
    }

    /// Only what the model proposed in THIS session and the person could hear about, and never a
    /// procedure: the model cannot read the quarantine, so it could not show the person anything
    /// else, and a procedure acts as an instruction later (S-26). Those go through `clio memory`.
    fn promote(&self, id: i64, judging: &Judging) -> Result<(), String> {
        let here = MemoryLink::agent_session(&judging.session);
        self.change(
            id,
            judging,
            Request::Promote,
            |r| {
                r.state == State::Quarantine
                    && r.kind != MemoryType::Procedural
                    && r.session.as_deref() == Some(here.as_str())
            },
            |link, row| link.promote(row.id),
        )
    }

    fn undo(&self, change: i64, judging: &Judging) -> Result<(), String> {
        let judge = self.judge.as_ref().ok_or(NO_JUDGE)?;
        // Read-only, and BEFORE Jev: only a change the judge made (an add or a replacement) in
        // this scope; anything else is the same answer as a change that does not exist.
        let content = {
            let link = self.reader()?;
            let op = link
                .engine()
                .operation(OpId(change))
                .map_err(|_| NOT_READABLE.to_owned())?;
            let by_judge = link.judge_actor().is_some_and(|j| j == op.actor);
            if !by_judge
                || link.place_of(op.scope).is_none()
                || !matches!(op.kind, OpKind::Add | OpKind::Update)
            {
                return Err(NOT_READABLE.to_owned());
            }
            let memory = op.memory.ok_or_else(|| NOT_READABLE.to_owned())?;
            link.get(memory)
                .map_err(|_| NOT_READABLE.to_owned())?
                .content
        };
        if !judge.confirm(
            Request::Undo,
            &content,
            &judging.person_said,
            judging.tainted,
        ) {
            return Err(NOT_CONFIRMED.to_owned());
        }
        // The writer only now; `revert_judged` checks again that the judge made the change.
        self.writer()?.revert_judged(OpId(change))
    }

    fn search(&self, query: &str, limit: usize) -> Result<Found, String> {
        self.with_link(|link| {
            // The engine's own message may repeat the query; the model gets a fixed one.
            let (rows, abstained) = link
                .search_all(query, limit)
                .map_err(|_| "la búsqueda en la memoria falló".to_owned())?;
            Ok(Found {
                items: rows.iter().map(|r| item(link, r)).collect(),
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
                    Ok(item(link, &row))
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
                .map(|r| item(link, r))
                .collect())
        })
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
            Ok(out.iter().map(|r| item(link, r)).collect())
        })
    }
}

fn item(link: &MemoryLink, row: &MemoryRow) -> MemoryItem {
    MemoryItem {
        id: row.id.0,
        kind: kind_label(row.kind).to_owned(),
        origin: match row.origin {
            Written::User => Origin::Person,
            Written::Agent => Origin::Agent,
            Written::External => Origin::External,
        },
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
