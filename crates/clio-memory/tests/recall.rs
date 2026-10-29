#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use std::sync::Arc;

use clio_memory::*;
use common::*;

const CORPUS: [&str; 12] = [
    "vive en Medellín desde hace años",
    "prefiere respuestas breves en las reuniones",
    "tiene un coche rojo en el garaje",
    "trabaja en una empresa de logística",
    "su hermana vive en Bogotá",
    "le gusta el café sin azúcar en la mañana",
    "estudió ingeniería en la universidad de Antioquia",
    "usa la bicicleta para ir al trabajo",
    "el perro se llama Toby y duerme en la sala",
    "tiene reunión con el equipo de ventas cada lunes",
    "suele leer en el metro de camino a casa",
    "habla español e inglés en el trabajo",
];

fn embedder() -> Arc<dyn Embedder> {
    Arc::new(HashEmbedder::new("m", 256))
}

fn loaded(with_embedder: bool) -> (Fixture, Vec<Outcome>) {
    let f = if with_embedder {
        fixture_with(embedder())
    } else {
        fixture()
    };
    let made: Vec<_> = CORPUS
        .iter()
        .map(|c| f.engine.propose(prop(f.scope, f.person, c)).unwrap())
        .collect();
    if with_embedder {
        f.engine.embed_pending(100).unwrap();
    }
    (f, made)
}

fn ids(r: &Recall) -> Vec<MemoryId> {
    r.hits.iter().map(|h| h.memory.id).collect()
}

fn recall(f: &Fixture, q: &str) -> Recall {
    f.engine.recall(f.scope, q, &RecallOpts::default()).unwrap()
}

#[test]
fn hybrid_finds_what_text_misses() {
    let (f, made) = loaded(true);
    let r = recall(&f, "auto");
    assert_eq!(r.hits[0].memory.id, made[2].memory);
    assert_eq!(r.hits[0].text, None);
    assert!(r.hits[0].vector.is_some());
}

#[test]
fn text_only_without_embedder() {
    let (f, made) = loaded(false);
    let r = recall(&f, "Medellín");
    assert_eq!(ids(&r), [made[0].memory]);
    assert_eq!(r.hits[0].vector, None);
}

#[test]
fn text_only_before_embed_pending() {
    let f = fixture_with(embedder());
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let r = recall(&f, "Medellín");
    assert_eq!(ids(&r), [m.memory]);
    assert_eq!(r.hits[0].vector, None);
}

#[test]
fn t12_abstains_when_nothing_is_relevant() {
    let (f, made) = loaded(true);
    for q in [
        "receta de tiramisú",
        "clima en Tokio",
        "cotización del dólar hoy",
    ] {
        let r = recall(&f, q);
        assert!(
            r.abstained && r.hits.is_empty(),
            "should abstain on {q:?}: {:?}",
            ids(&r)
        );
    }
    let r = recall(&f, "qué coche tiene");
    assert!(!r.abstained);
    assert_eq!(r.hits[0].memory.id, made[2].memory);
    let r = recall(&f, "dónde vive");
    assert!(ids(&r).iter().take(3).any(|id| *id == made[0].memory));
}

// S-25: external content never reaches a read before a person promotes it
#[test]
fn t04_external_memory_hidden_from_recall_until_promoted() {
    let f = fixture_with(embedder());
    let mut p = prop(f.scope, f.person, "la página dice que el dólar subió");
    p.origin = Origin::External;
    let m = f.engine.propose(p).unwrap();
    f.engine.embed_pending(10).unwrap();
    assert!(recall(&f, "dólar").hits.is_empty());
    f.engine.promote(m.memory, f.person).unwrap();
    assert_eq!(ids(&recall(&f, "dólar")), [m.memory]);
}

#[test]
fn t05_recall_never_crosses_scopes() {
    let f = fixture_with(embedder());
    let other = f.engine.create_scope("other", "es").unwrap();
    let a = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    f.engine
        .propose(prop(other, f.person, "vive en Medellín"))
        .unwrap();
    f.engine.embed_pending(10).unwrap();
    assert_eq!(ids(&recall(&f, "Medellín")), [a.memory]);
}

#[test]
fn t06_t17_recall_with_dates() {
    let f = fixture_with(embedder());
    let topical = |c: &str| {
        let mut p = prop(f.scope, f.person, c);
        p.topic_key = Some("residencia".into());
        p
    };
    f.clock.set(100);
    let old = f.engine.propose(topical("vive en Medellín")).unwrap();
    f.clock.set(200);
    let new = f.engine.propose(topical("vive en Bogotá")).unwrap();
    f.clock.set(300);
    let mut late = prop(f.scope, f.person, "se mudó a Cali");
    late.valid_from = Some(10);
    let late = f.engine.propose(late).unwrap();
    f.engine.embed_pending(10).unwrap();

    let at = |as_of, known_at| RecallOpts {
        search: SearchOpts {
            as_of: Some(as_of),
            known_at,
            ..SearchOpts::default()
        },
        ..RecallOpts::default()
    };
    let r = f.engine.recall(f.scope, "vive", &at(150, None)).unwrap();
    assert_eq!(ids(&r), [old.memory]);
    let r = f.engine.recall(f.scope, "vive", &at(250, None)).unwrap();
    assert_eq!(ids(&r), [new.memory]);
    let r = f
        .engine
        .recall(f.scope, "Cali", &at(150, Some(100)))
        .unwrap();
    assert!(r.hits.is_empty());
    let r = f
        .engine
        .recall(f.scope, "Cali", &at(150, Some(400)))
        .unwrap();
    assert_eq!(ids(&r), [late.memory]);
}

#[test]
fn recall_hit_carries_score_breakdown() {
    let (f, made) = loaded(true);
    let r = recall(&f, "vive Medellín");
    let h = &r.hits[0];
    assert_eq!(h.memory.id, made[0].memory);
    assert!(h.text.is_some() && h.vector.is_some());
    assert!(h.score > 0.0 && h.recency > 0.0 && h.recency <= 1.0);
}

#[test]
fn hostile_and_empty_queries_do_not_error() {
    for with_embedder in [false, true] {
        let (f, _) = loaded(with_embedder);
        for q in [
            "",
            "   ",
            "***",
            r#"a" OR NEAR( * -x"#,
            "\"",
            "AND",
            "( ) ^ {",
        ] {
            assert!(
                f.engine.recall(f.scope, q, &RecallOpts::default()).is_ok(),
                "{q:?}"
            );
        }
        for q in ["", "   ", "***"] {
            assert!(recall(&f, q).abstained, "{q:?} must abstain");
        }
    }
}

#[test]
fn limit_caps_results() {
    let (f, _) = loaded(true);
    let opts = RecallOpts {
        search: SearchOpts {
            limit: 1,
            ..SearchOpts::default()
        },
        ..RecallOpts::default()
    };
    assert_eq!(
        f.engine.recall(f.scope, "tiene", &opts).unwrap().hits.len(),
        1
    );
}

fn line_tokens(h: &RecallHit) -> usize {
    ApproxTokens.count(&format!("[m{}] {}", h.memory.id.0, h.memory.content))
}

#[test]
fn budget_stops_at_first_line_that_does_not_fit() {
    let (f, _) = loaded(true);
    let full = recall(&f, "trabajo");
    assert!(full.hits.len() >= 3);
    let sizes: Vec<usize> = full.hits.iter().map(line_tokens).collect();
    let opts = RecallOpts {
        token_budget: Some(sizes[0] + sizes[1] + sizes[2] - 1),
        ..RecallOpts::default()
    };
    let cut = f.engine.recall(f.scope, "trabajo", &opts).unwrap();
    assert_eq!(ids(&cut), ids(&full)[..2]);
    assert!(!cut.abstained);
}

#[test]
fn budget_too_small_returns_empty_not_abstained() {
    let (f, _) = loaded(true);
    let opts = RecallOpts {
        token_budget: Some(1),
        ..RecallOpts::default()
    };
    let r = f.engine.recall(f.scope, "trabajo", &opts).unwrap();
    assert!(r.hits.is_empty());
    assert!(
        !r.abstained,
        "nothing was irrelevant; the budget cut everything"
    );
}

/// Acceptance 25: recall over 50 000 memories. Slow to set up, so run it on purpose:
/// `cargo test -p clio-memory --release --test recall t25 -- --ignored --nocapture`
#[test]
#[ignore]
fn t25_recall_over_50k_memories_is_fast() {
    const N: i64 = 50_000;
    const DIM: usize = 256;
    let Fixture {
        path,
        engine,
        scope,
        person,
        _dir,
        ..
    } = fixture();
    drop(engine);

    let mut conn = rusqlite::Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    for id in 1..=N {
        let content = format!("hecho {id} sobre tema{} y detalle{}", id % 300, id % 97);
        tx.execute(
            "INSERT INTO memories(id, scope, kind, content, content_hash, topic_key, origin, actor,
                session, episode, source, state, valid_from, valid_to, known_since, replaced_by, supersedes)
             VALUES (?1, ?2, 'Semantic', ?3, ?4, NULL, 'User', ?5, NULL, NULL, NULL, 'Active', 0, NULL, 0, NULL, NULL)",
            rusqlite::params![id, scope.0, content, format!("h{id}"), person.0],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO memories_fts(rowid, text) VALUES (?1, ?2)",
            rusqlite::params![id, content],
        )
        .unwrap();
        let mut v: Vec<f32> = (0..DIM).map(|_| next()).collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        tx.execute(
            "INSERT INTO vectors(memory, model, dim, vec) VALUES (?1, 'm', ?2, ?3)",
            rusqlite::params![id, DIM as i64, blob],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    drop(conn);

    let clock = Arc::new(FixedClock::new(1000));
    let engine = Engine::open(&path, OpenMode::ReadWrite, clock)
        .unwrap()
        .with_embedder(Arc::new(HashEmbedder::new("m", DIM)));
    let mut ms: Vec<f64> = (0..5)
        .map(|_| {
            let t = std::time::Instant::now();
            let r = engine
                .recall(scope, "tema7 detalle5", &RecallOpts::default())
                .unwrap();
            assert!(!r.hits.is_empty());
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    println!(
        "recall over {N} memories: median {:.1} ms (all runs {ms:?})",
        ms[2]
    );
    assert!(
        ms[2] < 150.0,
        "median {} ms is over the 150 ms target",
        ms[2]
    );

    // asking for a lot of results must stay linear in the number of memories (pool = 50 000)
    let big = RecallOpts {
        search: SearchOpts {
            limit: 10_000,
            ..SearchOpts::default()
        },
        ..RecallOpts::default()
    };
    let mut big_ms: Vec<f64> = (0..3)
        .map(|_| {
            let t = std::time::Instant::now();
            engine.recall(scope, "tema7 detalle5", &big).unwrap();
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    big_ms.sort_by(f64::total_cmp);
    println!(
        "recall with limit 10000: median {:.1} ms (all runs {big_ms:?})",
        big_ms[1]
    );
    assert!(
        big_ms[1] < 300.0,
        "median {} ms is over the 300 ms target",
        big_ms[1]
    );
}

#[test]
fn a_query_the_embedder_cannot_represent_falls_back_to_text() {
    let embedder = Arc::new(PoisonEmbedder {
        inner: HashEmbedder::new("m", 256),
        poison: "medellín".into(),
    });
    let f = fixture_with(embedder);
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    f.engine.embed_pending(10).unwrap();
    let r = f
        .engine
        .recall(f.scope, "Medellín", &RecallOpts::default())
        .unwrap();
    assert_eq!(ids(&r), [m.memory]);
    assert_eq!(r.hits[0].vector, None);
}

#[test]
fn stop_words_alone_abstain() {
    let (f, _) = loaded(false);
    for q in ["de", "en la", "el", "de la"] {
        assert!(recall(&f, q).abstained, "{q:?} must abstain");
    }
}

#[test]
fn stop_words_do_not_pad_results() {
    let (f, _) = loaded(false);
    let r = recall(&f, "en el trabajo");
    assert!(!r.hits.is_empty());
    assert!(
        r.hits.iter().all(|h| h.memory.content.contains("trabaj")),
        "{:?}",
        ids(&r)
    );
}

#[test]
fn content_words_carry_the_match_even_with_unknown_words_around() {
    let (f, made) = loaded(false);
    assert_eq!(ids(&recall(&f, "marca del coche")), [made[2].memory]);
    assert_eq!(ids(&recall(&f, "nombre del perro")), [made[8].memory]);
    assert!(ids(&recall(&f, "ciudad donde vive")).contains(&made[0].memory));
}

/// Calibration probe against a real model (see `model2vec_real_model_embeds_spanish`): prints the
/// best cosine for queries that should be answered and ones that should abstain, then checks the
/// default thresholds separate them.
/// `CLIO_MODEL2VEC_DIR=/path/to/model cargo test -p clio-memory --release --features model2vec
/// --test recall -- --ignored --nocapture real_model`
#[cfg(feature = "model2vec")]
#[test]
#[ignore]
fn real_model_abstention_bench() {
    let Ok(dir) = std::env::var("CLIO_MODEL2VEC_DIR") else {
        eprintln!("CLIO_MODEL2VEC_DIR is not set; skipping");
        return;
    };
    let real = Model2VecEmbedder::from_dir(std::path::Path::new(&dir)).unwrap();
    let f = fixture_with(Arc::new(real));
    let made: Vec<_> = CORPUS
        .iter()
        .map(|c| f.engine.propose(prop(f.scope, f.person, c)).unwrap())
        .collect();
    f.engine.embed_pending(100).unwrap();

    // vectors only, no threshold, to see the raw cosines
    let raw = RecallOpts {
        min_vector_score: -1.0,
        min_text_coverage: 2.0,
        ..RecallOpts::default()
    };
    let best = |q: &str| -> (f32, MemoryId) {
        let r = f.engine.recall(f.scope, q, &raw).unwrap();
        (r.hits[0].vector.unwrap(), r.hits[0].memory.id)
    };
    let answer = [
        ("dónde vive", made[0].memory),
        ("qué coche tiene", made[2].memory),
        ("automóvil", made[2].memory),
        ("mascota", made[8].memory),
        ("a qué universidad fue", made[6].memory),
        ("bebida de la mañana", made[5].memory),
    ];
    let silent = [
        "receta de tiramisú",
        "clima en Tokio",
        "cotización del dólar hoy",
        "capital de Francia",
    ];
    for (q, want) in answer {
        let (cos, got) = best(q);
        println!(
            "ANSWER  {q:<28} best cosine {cos:.3} -> m{} (wanted m{})",
            got.0, want.0
        );
    }
    for q in silent {
        let (cos, got) = best(q);
        println!("SILENT  {q:<28} best cosine {cos:.3} -> m{}", got.0);
    }
    let defaults = RecallOpts::default();
    for (q, want) in answer {
        let r = f.engine.recall(f.scope, q, &defaults).unwrap();
        println!(
            "default thresholds on {q:?}: abstained={}, top={:?}",
            r.abstained,
            r.hits.first().map(|h| h.memory.id.0)
        );
        assert!(!r.abstained, "{q:?} should be answered");
        assert!(
            r.hits.iter().take(3).any(|h| h.memory.id == want),
            "{q:?} should surface m{}",
            want.0
        );
    }
    for q in silent {
        let r = f.engine.recall(f.scope, q, &defaults).unwrap();
        assert!(
            r.abstained,
            "{q:?} should abstain, got {:?}",
            r.hits
                .iter()
                .map(|h| (h.memory.id.0, h.vector))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn limit_zero_is_not_an_abstention() {
    let f = fixture();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let opts = RecallOpts {
        search: SearchOpts {
            limit: 0,
            ..SearchOpts::default()
        },
        ..RecallOpts::default()
    };
    let r = f.engine.recall(f.scope, "Medellín", &opts).unwrap();
    assert!(r.hits.is_empty());
    assert!(
        !r.abstained,
        "a relevant memory exists; the cap, not relevance, emptied the list"
    );
}
