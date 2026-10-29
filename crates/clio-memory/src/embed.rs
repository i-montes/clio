use crate::error::{Error, Result};

/// Turns text into vectors. The model id names the vector space: vectors from different
/// models are never compared.
pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    fn dim(&self) -> usize;
    /// One vector per text, in the same order.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Runs `f`; a panic comes back as its message instead of unwinding through the caller.
pub(crate) fn panic_message<T>(f: impl FnOnce() -> T) -> std::result::Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string())
    })
}

/// Runs `f` and turns a panic into `Error::Embedding`. Some model libraries `expect()` on
/// tokenizer errors; an embedder must not take the process (or the write lock) down with it.
#[cfg_attr(not(feature = "model2vec"), allow(dead_code))]
pub(crate) fn catch_panic<T>(f: impl FnOnce() -> T) -> Result<T> {
    panic_message(f).map_err(|msg| Error::Embedding(format!("the embedding model panicked: {msg}")))
}

/// Checks length and finiteness and returns the L2-normalized vector (dot product = cosine).
/// An all-zero vector has no direction and is rejected.
pub(crate) fn unit_vector(mut v: Vec<f32>, dim: usize) -> Result<Vec<f32>> {
    if v.len() != dim || v.iter().any(|x| !x.is_finite()) {
        return Err(Error::BadVector);
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < 1e-12 {
        return Err(Error::BadVector);
    }
    v.iter_mut().for_each(|x| *x /= norm);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn catch_panic_passes_values_through() {
        assert_eq!(catch_panic(|| 41 + 1).unwrap(), 42);
    }

    #[test]
    fn catch_panic_turns_a_panic_into_an_error() {
        let r: Result<()> = catch_panic(|| panic!("tokenization failed"));
        match r {
            Err(Error::Embedding(msg)) => assert!(msg.contains("tokenization failed"), "{msg}"),
            other => panic!("expected Embedding error, got {other:?}"),
        }
    }

    #[test]
    fn unit_vector_rejects_bad_input() {
        assert!(matches!(
            unit_vector(vec![1.0, 2.0], 3),
            Err(Error::BadVector)
        ));
        assert!(matches!(
            unit_vector(vec![1.0, f32::NAN, 0.0], 3),
            Err(Error::BadVector)
        ));
        assert!(matches!(
            unit_vector(vec![f32::INFINITY, 0.0, 0.0], 3),
            Err(Error::BadVector)
        ));
        assert!(matches!(
            unit_vector(vec![0.0, 0.0, 0.0], 3),
            Err(Error::BadVector)
        ));
    }

    #[test]
    fn unit_vector_normalizes() {
        let v = unit_vector(vec![3.0, 4.0, 0.0], 3).unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((v[0] - 0.6).abs() < 1e-6);
    }
}
