use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use model2vec_rs::model::StaticModel;
use sha2::{Digest, Sha256};

use crate::embed::{Embedder, catch_panic};
use crate::error::{Error, Result};
use crate::idle::{DEFAULT_IDLE, IdleCache};

/// Embeddings from a Model2Vec static model (CPU only: encoding a query takes microseconds, so a
/// GPU would add nothing). The model is read from a local directory; this crate never downloads
/// anything. It takes about 1 GB of memory, so it is dropped after five idle minutes and read
/// again on the next use.
pub struct Model2VecEmbedder {
    model: IdleCache<StaticModel>,
    id: String,
    dim: usize,
}

const MODEL_FILES: [&str; 3] = ["model.safetensors", "tokenizer.json", "config.json"];

fn collect(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() && depth < 2 {
            collect(&path, depth + 1, out)?;
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| MODEL_FILES.contains(&n))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// A short hash of the model's weights, tokenizer and config. It goes into the model id so that
/// replacing the files in a directory (same name, new weights) counts as a different vector
/// space and every memory is re-embedded, instead of mixing old and new vectors.
fn fingerprint(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect(dir, 0, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    for path in files {
        hasher.update(
            path.strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .as_bytes(),
        );
        let mut file = std::fs::File::open(&path)?;
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect())
}

impl Model2VecEmbedder {
    /// `dir` holds the model files (`model.safetensors`, `tokenizer.json`, `config.json`).
    pub fn from_dir(dir: &Path) -> Result<Self> {
        Self::from_dir_idle(dir, Some(DEFAULT_IDLE))
    }

    /// Like `from_dir`, with the idle time after which the model leaves memory (`None`: never).
    pub fn from_dir_idle(dir: &Path, idle: Option<Duration>) -> Result<Self> {
        if !dir.is_dir() {
            return Err(Error::Embedding(format!(
                "{} is not a directory",
                dir.display()
            )));
        }
        let model = read_model(dir)?;
        let dim = model.encode_single("a").len();
        if dim == 0 {
            return Err(Error::Embedding("model produced an empty vector".into()));
        }
        let name = dir
            .file_name()
            .map_or_else(|| "model".into(), |n| n.to_string_lossy().into_owned());
        let print = fingerprint(dir)?;
        let id = format!("model2vec:{name}:{dim}:{print}");
        let dir = dir.to_path_buf();
        // a reload must find the same files, or old and new vectors would mix
        let reload = move || {
            if fingerprint(&dir)? != print {
                return Err(Error::Embedding(format!(
                    "the model files in {} changed while it was unloaded",
                    dir.display()
                )));
            }
            read_model(&dir)
        };
        Ok(Self {
            model: IdleCache::new(model, idle, reload),
            id,
            dim,
        })
    }
}

fn read_model(dir: &Path) -> Result<StaticModel> {
    StaticModel::from_pretrained(dir, None, None, None).map_err(|e| Error::Embedding(e.to_string()))
}

impl Embedder for Model2VecEmbedder {
    fn model_id(&self) -> &str {
        &self.id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let owned: Vec<String> = texts.iter().map(|t| (*t).to_string()).collect();
        self.model.with(|m| catch_panic(|| m.encode(&owned)))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_follows_the_weights_and_ignores_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let put = |name: &str, body: &str| std::fs::write(dir.path().join(name), body).unwrap();
        put("model.safetensors", "weights-1");
        put("tokenizer.json", "tok");
        put("config.json", "cfg");
        put("README.md", "docs");
        let first = fingerprint(dir.path()).unwrap();
        put("README.md", "other docs");
        assert_eq!(fingerprint(dir.path()).unwrap(), first);
        put("model.safetensors", "weights-2");
        assert_ne!(fingerprint(dir.path()).unwrap(), first);
    }
}
