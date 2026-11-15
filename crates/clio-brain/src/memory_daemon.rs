//! Vectors from the clio-memory daemon: one process per data folder holds the only copy of the
//! model, and Clio asks it instead of loading its own. Without the `clio-memory` binary on the
//! `PATH`, with `CLIO_MEMORY_NO_DAEMON=1`, or when the daemon does not answer with the model Clio
//! would load itself, Clio loads it as before.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use clio_memory::Embedder;
use clio_memory::agent::EmbedModel;
use clio_memory::daemon::{Launch, RemoteEmbedder};

type Connect = Box<dyn Fn() -> Result<Arc<dyn Embedder>, String> + Send + Sync>;

/// `local` (the model Clio would load), with its vectors taken from the daemon of `data` when
/// the clio-memory binary is installed. The daemon is asked for (and started) only when the first
/// vector is needed. Not for a process whose environment holds a secret (the tool host): the
/// daemon it starts would keep that environment.
pub fn shared(data: &Path, local: Arc<EmbedModel>) -> Arc<EmbedModel> {
    let path = std::env::var("PATH").unwrap_or_default();
    let Some(exe) = daemon_exe(&path, std::env::var_os("CLIO_MEMORY_NO_DAEMON").as_deref()) else {
        return local;
    };
    let data = data.to_path_buf();
    with_daemon(
        local,
        Box::new(move || {
            let launch = Launch {
                exe: exe.clone(),
                env: vec![],
            };
            RemoteEmbedder::connect(&data, launch).map(|r| Arc::new(r) as Arc<dyn Embedder>)
        }),
    )
}

/// The clio-memory binary in `path_var`, absolute; none when `no_daemon` is `1`.
fn daemon_exe(path_var: &str, no_daemon: Option<&OsStr>) -> Option<PathBuf> {
    if no_daemon == Some(OsStr::new("1")) {
        return None;
    }
    crate::pi::install::find_in_path("clio-memory", path_var)
        .and_then(|p| std::path::absolute(p).ok())
}

/// `local` when it has no model; otherwise its model, whose vectors come from `connect` when that
/// gives the same model.
fn with_daemon(local: Arc<EmbedModel>, connect: Connect) -> Arc<EmbedModel> {
    let Ok(Some(own)) = local.get() else {
        return local;
    };
    EmbedModel::from_embedder(Arc::new(FromDaemon {
        own,
        connect,
        daemon: OnceLock::new(),
    }))
}

/// Clio's own model (its id and size; reading it takes the weights only when it embeds) and the
/// daemon, asked once.
struct FromDaemon {
    own: Arc<dyn Embedder>,
    connect: Connect,
    daemon: OnceLock<Option<Arc<dyn Embedder>>>,
}

impl FromDaemon {
    /// The daemon, if it answered with the model Clio has: other vectors would not compare.
    fn daemon(&self) -> Option<&Arc<dyn Embedder>> {
        self.daemon
            .get_or_init(|| {
                let d = (self.connect)().ok()?;
                (d.model_id() == self.own.model_id() && d.dim() == self.own.dim()).then_some(d)
            })
            .as_ref()
    }
}

impl Embedder for FromDaemon {
    fn model_id(&self) -> &str {
        self.own.model_id()
    }
    fn dim(&self) -> usize {
        self.own.dim()
    }
    fn embed(&self, texts: &[&str]) -> clio_memory::Result<Vec<Vec<f32>>> {
        match self.daemon() {
            Some(d) => d.embed(texts).or_else(|_| self.own.embed(texts)),
            None => self.own.embed(texts),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A model that answers `value` for each text, or fails.
    struct Fake {
        id: &'static str,
        value: f32,
        fails: bool,
    }

    impl Embedder for Fake {
        fn model_id(&self) -> &str {
            self.id
        }
        fn dim(&self) -> usize {
            2
        }
        fn embed(&self, texts: &[&str]) -> clio_memory::Result<Vec<Vec<f32>>> {
            if self.fails {
                return Err(clio_memory::Error::Embedding("caído".into()));
            }
            Ok(texts.iter().map(|_| vec![self.value; 2]).collect())
        }
    }

    fn fake(id: &'static str, value: f32, fails: bool) -> Arc<dyn Embedder> {
        Arc::new(Fake { id, value, fails })
    }

    fn local() -> Arc<EmbedModel> {
        EmbedModel::from_embedder(fake("granite", 1.0, false))
    }

    /// `with_daemon` with a daemon that gives `daemon`, and how many times it was asked for.
    fn model(daemon: Result<Arc<dyn Embedder>, String>) -> (Arc<EmbedModel>, Arc<AtomicUsize>) {
        let asked = Arc::new(AtomicUsize::new(0));
        let n = asked.clone();
        let m = with_daemon(
            local(),
            Box::new(move || {
                n.fetch_add(1, Ordering::SeqCst);
                daemon.clone()
            }),
        );
        (m, asked)
    }

    fn first_value(m: &EmbedModel) -> f32 {
        m.get().unwrap().unwrap().embed(&["hola"]).unwrap()[0][0]
    }

    #[test]
    fn the_daemon_is_asked_for_the_first_vector_and_only_once() {
        let (m, asked) = model(Ok(fake("granite", 2.0, false)));
        let e = m.get().unwrap().unwrap();
        assert_eq!((e.model_id(), e.dim()), ("granite", 2));
        assert_eq!(asked.load(Ordering::SeqCst), 0, "opening asks nothing");
        assert_eq!(first_value(&m), 2.0);
        assert_eq!(first_value(&m), 2.0);
        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn without_the_daemon_clio_uses_its_own_model() {
        let (m, _) = model(Err("no hay demonio".into()));
        assert_eq!(first_value(&m), 1.0);
    }

    #[test]
    fn a_daemon_with_another_model_is_not_used() {
        let (m, _) = model(Ok(fake("potion", 2.0, false)));
        assert_eq!(first_value(&m), 1.0);
    }

    #[test]
    fn a_daemon_that_fails_to_embed_leaves_it_to_clio() {
        let (m, _) = model(Ok(fake("granite", 2.0, true)));
        assert_eq!(first_value(&m), 1.0);
    }

    #[test]
    fn without_a_model_of_its_own_clio_never_asks_the_daemon() {
        let asked = Arc::new(AtomicUsize::new(0));
        let n = asked.clone();
        let m = with_daemon(
            EmbedModel::none(),
            Box::new(move || {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(fake("granite", 2.0, false))
            }),
        );
        assert!(m.get().unwrap().is_none());
        assert_eq!(asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_binary_is_looked_up_in_the_path_and_can_be_turned_off() {
        let t = tempfile::tempdir().unwrap();
        let empty = t.path().join("empty");
        let bin = t.path().join("bin");
        std::fs::create_dir(&empty).unwrap();
        std::fs::create_dir(&bin).unwrap();
        assert_eq!(daemon_exe(empty.to_str().unwrap(), None), None);
        let exe = bin.join("clio-memory");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", empty.display(), bin.display());
        assert_eq!(daemon_exe(&path, None), Some(exe.clone()));
        assert_eq!(daemon_exe(&path, Some(OsStr::new("0"))), Some(exe));
        assert_eq!(daemon_exe(&path, Some(OsStr::new("1"))), None);
    }
}
