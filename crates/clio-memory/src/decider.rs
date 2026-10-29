use crate::error::Result;
use crate::model::{Choice, Decision, Question};

/// Something that can opine on what to do with a proposal the rules left undecided: a
/// classifier, Jev, an LLM. `Ok(None)` means "I don't know". Errors and panics are recorded and
/// skipped; they never block a write (a caught panic still runs the process-wide panic hook,
/// which prints to stderr unless the embedding program replaces it).
///
/// `decide` runs while the engine holds its write lock: it may read from the engine but must not
/// call any of its write methods (`propose`, `promote`, `archive`, `revert`, ...), which would
/// wait for the lock the caller is holding. A slow decider delays every write. The `reason` it
/// returns and its `name` are capped and checked for secrets before they are logged.
pub trait Decider: Send + Sync {
    /// Stable name, used as the provider in the decision log.
    fn name(&self) -> &str;
    fn decide(&self, q: &Question) -> Result<Option<Decision>>;
}

/// Sends a proposal to human review when its closest neighbor is almost the same text. It never
/// drops or replaces anything. Off by default: with a real model a paraphrase and a contradiction
/// can score alike, so a person decides. Register it with `Engine::with_decider`.
pub struct NearDuplicateDecider {
    /// Cosine similarity from which the closest neighbor counts as a probable duplicate.
    pub threshold: f32,
}

impl Default for NearDuplicateDecider {
    fn default() -> Self {
        Self { threshold: 0.97 }
    }
}

impl Decider for NearDuplicateDecider {
    fn name(&self) -> &str {
        "near-duplicate"
    }

    fn decide(&self, q: &Question) -> Result<Option<Decision>> {
        let Some(nearest) = q.neighbors.first() else {
            return Ok(None);
        };
        if nearest.memory.kind != q.candidate.kind || nearest.similarity < self.threshold {
            return Ok(None);
        }
        Ok(Some(Decision {
            choice: Choice::Review,
            confidence: nearest.similarity.clamp(0.0, 1.0),
            reason: Some(format!("parece duplicado de m{}", nearest.memory.id.0)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ActorId, ActorKind, Candidate, Choice, MemoryId, MemoryRow, MemoryType, Neighbor, Origin,
        ScopeId, State,
    };

    fn row(id: i64, kind: MemoryType) -> MemoryRow {
        MemoryRow {
            id: MemoryId(id),
            scope: ScopeId(1),
            kind,
            content: "x".into(),
            content_hash: "h".into(),
            topic_key: None,
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
        }
    }

    fn question(kind: MemoryType, neighbors: Vec<Neighbor>) -> Question {
        Question {
            candidate: Candidate {
                scope: ScopeId(1),
                kind,
                content: "y".into(),
                topic_key: None,
                origin: Origin::User,
                actor_kind: ActorKind::Person,
            },
            neighbors,
        }
    }

    fn near(id: i64, kind: MemoryType, similarity: f32) -> Neighbor {
        Neighbor {
            memory: row(id, kind),
            similarity,
        }
    }

    #[test]
    fn default_threshold_is_097() {
        assert_eq!(NearDuplicateDecider::default().threshold, 0.97);
        assert_eq!(NearDuplicateDecider::default().name(), "near-duplicate");
    }

    #[test]
    fn flags_a_close_neighbor_of_the_same_kind() {
        let d = NearDuplicateDecider { threshold: 0.9 };
        let q = question(
            MemoryType::Semantic,
            vec![near(7, MemoryType::Semantic, 0.95)],
        );
        let got = d.decide(&q).unwrap().unwrap();
        assert_eq!(got.choice, Choice::Review);
        assert!((got.confidence - 0.95).abs() < 1e-6);
        assert!(got.reason.unwrap().contains("m7"));
    }

    #[test]
    fn ignores_other_kinds_and_far_neighbors() {
        let d = NearDuplicateDecider { threshold: 0.9 };
        let other_kind = question(
            MemoryType::Semantic,
            vec![near(7, MemoryType::Procedural, 0.99)],
        );
        assert!(d.decide(&other_kind).unwrap().is_none());
        let far = question(
            MemoryType::Semantic,
            vec![near(7, MemoryType::Semantic, 0.5)],
        );
        assert!(d.decide(&far).unwrap().is_none());
        assert!(
            d.decide(&question(MemoryType::Semantic, vec![]))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn similarity_slightly_above_one_is_clamped() {
        let d = NearDuplicateDecider { threshold: 0.9 };
        let q = question(
            MemoryType::Semantic,
            vec![near(7, MemoryType::Semantic, 1.000_000_1)],
        );
        assert_eq!(d.decide(&q).unwrap().unwrap().confidence, 1.0);
    }
}
