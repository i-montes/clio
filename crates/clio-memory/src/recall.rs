use std::collections::HashMap;

use crate::model::MemoryId;

/// Reciprocal Rank Fusion constant.
pub(crate) const RRF_K: f64 = 60.0;
/// A fact learned just now gets this much extra weight; it halves every 30 days.
pub(crate) const RECENCY_WEIGHT: f64 = 0.1;
/// The most extra weight frequent use can give; kept below recency so it only breaks near-ties.
pub(crate) const USAGE_WEIGHT: f64 = 0.05;
// Checked when compiling: use must never outweigh recency.
const _: () = assert!(USAGE_WEIGHT < RECENCY_WEIGHT);
/// How far feedback can move a score: confidence 0 scales it by 0.9, 1 by 1.1, 0.5 leaves it.
pub(crate) const CONFIDENCE_WEIGHT: f64 = 0.1;
// Checked when compiling: trust may break ties but never outweigh how recent something is.
const _: () = assert!(CONFIDENCE_WEIGHT <= RECENCY_WEIGHT);
const HALF_LIFE_SECS: f64 = 30.0 * 86_400.0;

/// 1.0 for something learned just now, 0.5 after 30 days, and so on (future dates count as now).
pub(crate) fn recency(now: i64, known_since: i64) -> f64 {
    let age = (now - known_since).max(0) as f64;
    0.5f64.powf(age / HALF_LIFE_SECS)
}

/// What feedback adds up to: starts at 0.5, each useful signal adds 0.1, each unspent wrong takes 0.2.
pub(crate) fn confidence_of(useful: u32, wrong: u32) -> f32 {
    (0.5 + 0.1 * f64::from(useful) - 0.2 * f64::from(wrong)).clamp(0.0, 1.0) as f32
}

pub(crate) struct Fused {
    pub id: MemoryId,
    pub score: f64,
    pub text: Option<f64>,
    pub vector: Option<f32>,
    pub recency: f64,
    pub confidence: f32,
}

/// Merges two ranked lists (best first) by reciprocal rank: each list an id appears in adds
/// `1 / (RRF_K + rank)`; the sum is then nudged by recency, use and confidence. Best first, ties by id.
pub(crate) fn fuse(
    text: &[(MemoryId, f64)],
    vector: &[(MemoryId, f32)],
    now: i64,
    known_since: impl Fn(MemoryId) -> i64,
    uses: impl Fn(MemoryId) -> u32,
    confidence: impl Fn(MemoryId) -> f32,
) -> Vec<Fused> {
    let mut by_id: HashMap<MemoryId, Fused> = HashMap::new();
    let fresh = |id: MemoryId| Fused {
        id,
        score: 0.0,
        text: None,
        vector: None,
        recency: recency(now, known_since(id)),
        confidence: confidence(id),
    };
    for (rank, (id, s)) in text.iter().enumerate() {
        let e = by_id.entry(*id).or_insert_with(|| fresh(*id));
        e.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        e.text = Some(*s);
    }
    for (rank, (id, s)) in vector.iter().enumerate() {
        let e = by_id.entry(*id).or_insert_with(|| fresh(*id));
        e.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        e.vector = Some(*s);
    }
    let mut out: Vec<Fused> = by_id.into_values().collect();
    for f in &mut out {
        let usage = ((1.0 + f64::from(uses(f.id))).ln() / 21f64.ln()).min(1.0);
        let trust = CONFIDENCE_WEIGHT * (2.0 * f64::from(f.confidence) - 1.0);
        f.score *= 1.0 + RECENCY_WEIGHT * f.recency + USAGE_WEIGHT * usage + trust;
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn rrf_adds_reciprocal_ranks() {
        let now = 1_000_000;
        let text = [(MemoryId(1), 2.0), (MemoryId(2), 1.0)];
        let vector = [(MemoryId(1), 0.9), (MemoryId(3), 0.8)];
        let fused = fuse(&text, &vector, now, |_| now, |_| 0, |_| 0.5);
        let ids: Vec<i64> = fused.iter().map(|f| f.id.0).collect();
        assert_eq!(ids, [1, 2, 3]);
        let boost = 1.0 + RECENCY_WEIGHT;
        assert!((fused[0].score - 2.0 / (RRF_K + 1.0) * boost).abs() < 1e-9);
        assert!((fused[1].score - 1.0 / (RRF_K + 2.0) * boost).abs() < 1e-9);
        assert_eq!((fused[0].text, fused[0].vector), (Some(2.0), Some(0.9)));
        assert_eq!((fused[1].text, fused[1].vector), (Some(1.0), None));
    }

    #[test]
    fn recency_breaks_ties_for_equal_relevance() {
        let now = 1_000_000;
        let text = [(MemoryId(1), 1.0)];
        let vector = [(MemoryId(2), 0.9)];
        let known = |id: MemoryId| if id.0 == 1 { now - 90 * DAY } else { now };
        let fused = fuse(&text, &vector, now, known, |_| 0, |_| 0.5);
        assert_eq!(fused[0].id, MemoryId(2));
    }

    #[test]
    fn usage_breaks_ties_in_favor_of_the_used_memory() {
        let now = 1_000_000;
        let text = [(MemoryId(1), 1.0)];
        let vector = [(MemoryId(2), 0.9)];
        let fused = fuse(
            &text,
            &vector,
            now,
            |_| now,
            |id| if id.0 == 2 { 10 } else { 0 },
            |_| 0.5,
        );
        assert_eq!(fused[0].id, MemoryId(2));
    }

    #[test]
    fn usage_factor_is_capped() {
        let now = 1_000_000;
        let score = |uses| {
            fuse(
                &[(MemoryId(1), 1.0)],
                &[],
                now,
                |_| now,
                move |_| uses,
                |_| 0.5,
            )[0]
            .score
        };
        assert_eq!(score(1000), score(20));
        assert!(score(1) > score(0));
    }

    #[test]
    fn confidence_breaks_ties() {
        let now = 1_000_000;
        let text = [(MemoryId(1), 1.0)];
        let vector = [(MemoryId(2), 0.9)];
        let trusted = |id: MemoryId| if id.0 == 2 { 0.9 } else { 0.5 };
        let fused = fuse(&text, &vector, now, |_| now, |_| 0, trusted);
        assert_eq!(fused[0].id, MemoryId(2));
        assert_eq!(fused[0].confidence, 0.9f32);
    }

    #[test]
    fn confidence_factor_is_bounded() {
        let now = 1_000_000;
        let score =
            |c: f32| fuse(&[(MemoryId(1), 1.0)], &[], now, |_| now, |_| 0, move |_| c)[0].score;
        let base = 1.0 / (RRF_K + 1.0);
        assert!((score(1.0) - score(0.5) - base * CONFIDENCE_WEIGHT).abs() < 1e-12);
        assert!((score(0.5) - score(0.0) - base * CONFIDENCE_WEIGHT).abs() < 1e-12);
    }

    #[test]
    fn recency_halves_every_30_days() {
        let now = 10 * 365 * DAY;
        assert!((recency(now, now) - 1.0).abs() < 1e-12);
        assert!((recency(now, now - 30 * DAY) - 0.5).abs() < 1e-9);
        assert!(
            (recency(now, now + DAY) - 1.0).abs() < 1e-12,
            "future dates clamp to 1"
        );
    }
}
