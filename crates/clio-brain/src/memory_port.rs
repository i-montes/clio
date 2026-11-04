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
    Error, Judged, MemoryId, MemoryRow, MemoryType, OpKind, Origin as Written, State,
};
use clio_toolhost::{
    Found, Judging, MemoryItem, MemoryPort, Origin, ProposalKind, Proposed, Saved,
};

use crate::memory_judge::{Call, MemoryJudge, Neighbor};
use crate::memory_link::{EmbedModel, MemoryLink, Res, explain, kind_label};

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
    /// Jev, who decides what becomes of a proposal. Without it every proposal waits for the person.
    judge: Option<Arc<MemoryJudge>>,
}

impl LinkPort {
    /// `model_dir` is the vector model, if there is one; it is only loaded when a search needs it.
    pub fn new(data: &Path, model_dir: Option<&Path>) -> Self {
        Self {
            data: data.to_path_buf(),
            model: EmbedModel::new(model_dir.map(Path::to_path_buf)),
            link: Mutex::new(None),
            judge: None,
        }
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
        if slot.is_none() {
            match MemoryLink::open_read(&self.data, &self.model) {
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

impl MemoryPort for LinkPort {
    fn propose(
        &self,
        text: &str,
        kind: ProposalKind,
        session: &str,
        judging: &Judging,
    ) -> Result<Proposed, String> {
        // Opened for this call only (it waits a few seconds for another writer), dropped at the end.
        let link = MemoryLink::open_write(&self.data, &EmbedModel::none()).map_err(|why| {
            // The detail names a path of the person's: the service's log, not the model.
            eprintln!("clio-toolhost: memory: {why}");
            "la memoria no se puede abrir para escribir ahora".to_owned()
        })?;
        let kind = match kind {
            ProposalKind::Semantic => MemoryType::Semantic,
            ProposalKind::Procedural => MemoryType::Procedural,
        };
        if let Some(judge) = &self.judge {
            let stored = MemoryLink::agent_text(text);
            let neighbors: Vec<Neighbor> = link
                .search(&stored, 3)
                .map(|(rows, _)| {
                    rows.into_iter()
                        .map(|r| Neighbor {
                            id: r.id.0,
                            protected: link.is_protected(&r),
                            text: r.content,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let v = judge.decide(
                &stored,
                kind == MemoryType::Procedural,
                &neighbors,
                judging.tainted,
                &judging.person_said,
            );
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
                match link.propose_judged_as_agent(text, kind, session, how, v.confidence) {
                    Ok(o) => {
                        let saved = match (o.kind, o.state) {
                            (OpKind::Noop, _) => Saved::Known,
                            (OpKind::Update, State::Active) => Saved::Replaced,
                            (_, State::Active) => Saved::Active,
                            _ => Saved::Waiting,
                        };
                        return Ok(Proposed {
                            id: o.memory.0,
                            saved,
                        });
                    }
                    // The engine refused what the judge asked (a neighbor that stopped being
                    // active, one the person owns): the person decides, as before the judge.
                    Err(Error::NotAllowed | Error::InvalidState | Error::NotFound) => {}
                    Err(e) => return Err(explain(&e)),
                }
            }
        }
        link.propose_as_agent(text, kind, session)
            .map(|id| Proposed {
                id: id.0,
                saved: Saved::Waiting,
            })
    }

    fn search(&self, query: &str, limit: usize) -> Result<Found, String> {
        self.with_link(|link| {
            // The engine's own message may repeat the query; the model gets a fixed one.
            let (rows, abstained) = link
                .search(query, limit)
                .map_err(|_| "la búsqueda en la memoria falló".to_owned())?;
            Ok(Found {
                items: rows.iter().map(item).collect(),
                abstained,
                note: (!link.has_embeddings()).then(|| NO_VECTORS.to_owned()),
            })
        })
    }

    fn get(&self, id: i64) -> Result<MemoryItem, String> {
        self.with_link(|link| {
            // `MemoryLink::get` already refuses another scope; its text is dropped with the rest.
            match link.get(MemoryId(id)) {
                Ok(row) if row.state == State::Active && row.scope == link.scope() => {
                    Ok(item(&row))
                }
                _ => Err(NOT_READABLE.into()),
            }
        })
    }
}

fn item(row: &MemoryRow) -> MemoryItem {
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
    }
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
