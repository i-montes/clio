#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use clio_memory::*;
use common::*;

fn hash(model: &str, dim: usize) -> Arc<dyn Embedder> {
    Arc::new(HashEmbedder::new(model, dim))
}

fn bad(model: &str, fault: Fault) -> Arc<dyn Embedder> {
    Arc::new(BadEmbedder {
        model: model.into(),
        fault,
    })
}

fn propose_three(f: &Fixture) -> Vec<Outcome> {
    [
        "vive en Medellín",
        "prefiere respuestas breves",
        "tiene un coche rojo",
    ]
    .iter()
    .map(|c| f.engine.propose(prop(f.scope, f.person, c)).unwrap())
    .collect()
}

#[test]
fn embed_pending_embeds_everything_once() {
    let f = fixture_with(hash("m", 64));
    propose_three(&f);
    assert_eq!(f.engine.embed_pending(10).unwrap(), 3);
    assert_eq!(f.engine.embed_pending(10).unwrap(), 0);
}

#[test]
fn embed_pending_skips_archived() {
    let f = fixture_with(hash("m", 64));
    let made = propose_three(&f);
    f.engine.archive(made[0].memory, f.person).unwrap();
    assert_eq!(f.engine.embed_pending(10).unwrap(), 2);
}

#[test]
fn embed_pending_respects_limit() {
    let f = fixture_with(hash("m", 64));
    propose_three(&f);
    assert_eq!(f.engine.embed_pending(2).unwrap(), 2);
    assert_eq!(f.engine.embed_pending(10).unwrap(), 1);
}

#[test]
fn changing_model_makes_everything_pending_again() {
    let f = fixture_with(hash("a", 64));
    propose_three(&f);
    assert_eq!(f.engine.embed_pending(10).unwrap(), 3);
    let engine = f.engine.with_embedder(hash("b", 32));
    assert_eq!(engine.embed_pending(10).unwrap(), 3);
    assert_eq!(engine.embed_pending(10).unwrap(), 0);
}

#[test]
fn bad_embedder_output_stores_nothing_and_keeps_earlier_vectors() {
    for fault in [Fault::Zeros, Fault::WrongDim, Fault::TooFew] {
        let f = fixture_with(hash("m", 8));
        f.engine
            .propose(prop(f.scope, f.person, "primero"))
            .unwrap();
        assert_eq!(f.engine.embed_pending(10).unwrap(), 1);
        f.engine
            .propose(prop(f.scope, f.person, "segundo"))
            .unwrap();

        let engine = f.engine.with_embedder(bad("m", fault));
        assert!(matches!(engine.embed_pending(10), Err(Error::BadVector)));

        // Nothing from the bad batch was stored, and the earlier vector is still there.
        let engine = engine.with_embedder(hash("m", 8));
        assert_eq!(engine.embed_pending(10).unwrap(), 1);
    }
}

#[test]
fn no_embedder_is_an_error() {
    let f = fixture();
    assert!(matches!(f.engine.embed_pending(10), Err(Error::NoEmbedder)));
}

#[test]
fn propose_never_embeds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let embedder = CountingEmbedder {
        inner: HashEmbedder::new("m", 64),
        calls: calls.clone(),
    };
    let f = fixture_with(Arc::new(embedder));
    propose_three(&f);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    f.engine.embed_pending(10).unwrap();
    assert!(calls.load(Ordering::SeqCst) >= 1);
}

// S-24: vectors are as sensitive as the text: same file, same 0600 permissions
#[test]
fn vectors_live_in_the_same_0600_file() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture_with(hash("m", 64));
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    f.engine.embed_pending(10).unwrap();

    let mode = std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let raw = rusqlite::Connection::open(&f.path).unwrap();
    let stored: i64 = raw
        .query_row("SELECT COUNT(*) FROM vectors", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, 1);

    // vectors are derived data: rebuilding the projection leaves them usable
    f.engine.rebuild_projection().unwrap();
    let r = f
        .engine
        .recall(f.scope, "vive Medellín", &RecallOpts::default())
        .unwrap();
    assert!(r.hits[0].vector.is_some());
}

/// Needs a Model2Vec model on disk: `CLIO_MODEL2VEC_DIR=/path/to/model cargo test -p clio-memory
/// --features model2vec --test embed -- --ignored`
#[cfg(feature = "model2vec")]
#[test]
#[ignore]
fn model2vec_real_model_embeds_spanish() {
    let Ok(dir) = std::env::var("CLIO_MODEL2VEC_DIR") else {
        eprintln!("CLIO_MODEL2VEC_DIR is not set; skipping");
        return;
    };
    let embedder = Model2VecEmbedder::from_dir(std::path::Path::new(&dir)).unwrap();
    let f = fixture_with(Arc::new(embedder));
    f.engine
        .propose(prop(f.scope, f.person, "Vive en Medellín desde hace años"))
        .unwrap();
    f.engine
        .propose(prop(f.scope, f.person, "Tiene un coche rojo"))
        .unwrap();
    assert_eq!(f.engine.embed_pending(10).unwrap(), 2);
    let r = f
        .engine
        .recall(f.scope, "automóvil", &RecallOpts::default())
        .unwrap();
    assert!(
        !r.abstained,
        "a real model should relate 'automóvil' to 'coche'"
    );
}

fn poison(model: &str, word: &str) -> Arc<dyn Embedder> {
    Arc::new(PoisonEmbedder {
        inner: HashEmbedder::new(model, 64),
        poison: word.into(),
    })
}

#[test]
fn an_unrepresentable_memory_does_not_block_the_others() {
    let f = fixture_with(poison("m", "🙂"));
    for c in ["vive en Medellín", "🙂", "tiene un coche rojo"] {
        f.engine.propose(prop(f.scope, f.person, c)).unwrap();
    }
    assert_eq!(f.engine.embed_pending(100).unwrap(), 2);
    assert_eq!(
        f.engine.embed_pending(100).unwrap(),
        0,
        "the skipped one is not retried forever"
    );
    let r = f
        .engine
        .recall(f.scope, "coche", &RecallOpts::default())
        .unwrap();
    assert!(r.hits[0].vector.is_some());
}

#[test]
fn a_batch_of_only_unrepresentable_memories_is_skipped_not_an_error() {
    let f = fixture_with(poison("m", "🙂"));
    for c in ["🙂", "🙂 🙂", "🙂🙂🙂"] {
        f.engine.propose(prop(f.scope, f.person, c)).unwrap();
    }
    assert_eq!(f.engine.embed_pending(100).unwrap(), 0);
    f.engine
        .propose(prop(f.scope, f.person, "tiene un coche rojo"))
        .unwrap();
    assert_eq!(
        f.engine.embed_pending(100).unwrap(),
        1,
        "later memories are still embedded"
    );
}

#[test]
fn read_only_engine_does_not_embed() {
    let f = fixture();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let embedder = CountingEmbedder {
        inner: HashEmbedder::new("m", 64),
        calls: calls.clone(),
    };
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(1000));
    let ro = Engine::open(&f.path, OpenMode::ReadOnly, clock)
        .unwrap()
        .with_embedder(Arc::new(embedder));
    assert!(matches!(ro.embed_pending(10), Err(Error::ReadOnly)));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "no CPU may be spent on work that cannot be stored"
    );
}
