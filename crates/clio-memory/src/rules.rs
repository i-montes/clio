use crate::model::{ActorKind, MemoryId, MemoryRow, MemoryType, Origin, State};

/// An agent or session can never vouch for user-origin content; only people and importers can.
pub fn effective_origin(actor: ActorKind, declared: Origin) -> Origin {
    match (actor, declared) {
        (ActorKind::Agent | ActorKind::Session, Origin::User) => Origin::Agent,
        _ => declared,
    }
}

/// What the store already holds that matches the proposal; both must be active, in the
/// proposal's scope (and, for `by_topic`, of the same type).
#[derive(Clone, Copy)]
pub struct Existing<'a> {
    pub by_hash: Option<&'a MemoryRow>,
    pub by_topic: Option<&'a MemoryRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Noop(MemoryId),
    Add {
        state: State,
        /// A quarantined candidate that would replace this memory once promoted.
        supersedes: Option<MemoryId>,
    },
    Update {
        old: MemoryId,
    },
}

/// Deterministic rules, in order: duplicate, then same topic, then new. Nothing waits for a
/// person: the memory is the agent's, whatever the origin, the kind or who wrote what it replaces
/// (the origin is kept and shown; every change stays in the log and can be reverted).
pub fn decide(_kind: MemoryType, _effective: Origin, existing: Existing) -> Verdict {
    if let Some(dup) = existing.by_hash {
        return Verdict::Noop(dup.id);
    }
    if let Some(same) = existing.by_topic {
        return Verdict::Update { old: same.id };
    }
    Verdict::Add {
        state: State::Active,
        supersedes: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ActorId, ScopeId};

    fn row(id: i64) -> MemoryRow {
        MemoryRow {
            id: MemoryId(id),
            scope: ScopeId(1),
            kind: MemoryType::Semantic,
            content: "x".into(),
            content_hash: "h".into(),
            topic_key: Some("t".into()),
            origin: Origin::User,
            actor: ActorId(1),
            session: None,
            episode: None,
            source: None,
            state: State::Active,
            valid_from: 0,
            valid_to: None,
            known_since: 0,
            replaced_by: None,
            supersedes: None,
            title: None,
            category: None,
        }
    }

    const NONE: Existing = Existing {
        by_hash: None,
        by_topic: None,
    };

    #[test]
    fn agent_cannot_claim_user_origin() {
        assert_eq!(
            effective_origin(ActorKind::Agent, Origin::User),
            Origin::Agent
        );
        assert_eq!(
            effective_origin(ActorKind::Session, Origin::User),
            Origin::Agent
        );
    }

    #[test]
    fn person_keeps_declared_origin() {
        assert_eq!(
            effective_origin(ActorKind::Person, Origin::User),
            Origin::User
        );
        assert_eq!(
            effective_origin(ActorKind::Importer, Origin::External),
            Origin::External
        );
    }

    #[test]
    fn hash_match_is_noop_even_for_external() {
        let r = row(7);
        let e = Existing {
            by_hash: Some(&r),
            by_topic: None,
        };
        assert_eq!(
            decide(MemoryType::Semantic, Origin::External, e),
            Verdict::Noop(MemoryId(7))
        );
    }

    // The memory is the agent's: outside content and an agent's procedure are active at once, and
    // a candidate with the topic of the person's memory replaces it (the log keeps the old one).
    #[test]
    fn nothing_waits_for_a_person() {
        let persons = row(3); // origin User
        let e = Existing {
            by_hash: None,
            by_topic: Some(&persons),
        };
        for origin in [Origin::External, Origin::Agent, Origin::User] {
            assert_eq!(
                decide(MemoryType::Semantic, origin, e),
                Verdict::Update { old: MemoryId(3) },
                "{origin:?}"
            );
        }
        for origin in [Origin::External, Origin::Agent] {
            assert_eq!(
                decide(MemoryType::Procedural, origin, NONE),
                Verdict::Add {
                    state: State::Active,
                    supersedes: None
                },
                "{origin:?}"
            );
        }
    }

    #[test]
    fn user_procedural_is_active() {
        assert_eq!(
            decide(MemoryType::Procedural, Origin::User, NONE),
            Verdict::Add {
                state: State::Active,
                supersedes: None
            }
        );
    }

    #[test]
    fn topic_match_updates() {
        let r = row(3);
        let e = Existing {
            by_hash: None,
            by_topic: Some(&r),
        };
        assert_eq!(
            decide(MemoryType::Semantic, Origin::User, e),
            Verdict::Update { old: MemoryId(3) }
        );
    }

    #[test]
    fn no_match_adds_active() {
        assert_eq!(
            decide(MemoryType::Semantic, Origin::Agent, NONE),
            Verdict::Add {
                state: State::Active,
                supersedes: None
            }
        );
    }
}
