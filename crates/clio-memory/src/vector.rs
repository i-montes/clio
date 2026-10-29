use crate::error::Result;
use crate::model::{MemoryId, ScopeId, SearchOpts};

/// Nearest-neighbour search over stored vectors, best first. Honors the same visibility rules
/// as text search (scope, state, validity, knowledge date, kind).
pub trait VectorIndex {
    fn nearest(
        &self,
        scope: ScopeId,
        model: &str,
        query: &[f32],
        opts: &SearchOpts,
        k: usize,
    ) -> Result<Vec<(MemoryId, f32)>>;
}

pub(crate) fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn f32_at(blob: &[u8], i: usize) -> f32 {
    let o = i * 4;
    f32::from_le_bytes([blob[o], blob[o + 1], blob[o + 2], blob[o + 3]])
}

/// Dot product of a stored little-endian f32 BLOB with `q`, accumulating in 8 independent
/// lanes so the compiler can vectorize it. `blob` must hold `q.len()` floats.
pub(crate) fn dot(blob: &[u8], q: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let body = q.len() - q.len() % 8;
    for base in (0..body).step_by(8) {
        for (lane, a) in acc.iter_mut().enumerate() {
            *a += f32_at(blob, base + lane) * q[base + lane];
        }
    }
    let tail: f32 = (body..q.len()).map(|i| f32_at(blob, i) * q[i]).sum();
    acc.iter().sum::<f32>() + tail
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(n: usize, phase: f32) -> Vec<f32> {
        (0..n).map(|i| ((i as f32) * 0.37 + phase).sin()).collect()
    }

    #[test]
    fn dot_matches_naive_sum() {
        for n in [256, 37, 8, 1, 0] {
            let (a, b) = (wave(n, 0.0), wave(n, 1.3));
            let naive: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            assert!((dot(&to_blob(&a), &b) - naive).abs() < 1e-3, "n={n}");
        }
    }
}
