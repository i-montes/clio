//! The engine with 10 000 memories in three places (a project, the person's, the lessons), the
//! size Engram is used at: how long a search over the three takes, and the recent memories and a
//! timeline. Ignored by default (it takes a while to fill); run it on purpose:
//!
//! `cargo test --release -p clio-memory --test scale -- --ignored --nocapture`
//!
//! The numbers are written to the output; the asserts are generous bounds that only catch a
//! search that stopped using its indexes.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use clio_memory::{Category, RecallOpts, SearchOpts};
use common::{HashEmbedder, fixture_with, prop};

const N: usize = 10_000;
const WORDS: [&str; 24] = [
    "postgres",
    "redis",
    "docker",
    "compose",
    "healthcheck",
    "migración",
    "índice",
    "caché",
    "puerto",
    "despliegue",
    "prueba",
    "error",
    "timeout",
    "socket",
    "clave",
    "vault",
    "rust",
    "python",
    "jaula",
    "memoria",
    "proyecto",
    "sesión",
    "resumen",
    "jev",
];

fn percentile(v: &mut [Duration], p: f64) -> Duration {
    v.sort();
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

#[test]
#[ignore = "fills 10 000 memories; run with --ignored"]
fn ten_thousand_memories_in_three_places() {
    let f = fixture_with(Arc::new(HashEmbedder::new("hash-256", 256)));
    let project = f.engine.create_scope("proyecto:alfa", "es").unwrap();
    let lessons = f.engine.create_scope("lecciones", "es").unwrap();
    let scopes = [project, f.scope, lessons];
    let t = Instant::now();
    for i in 0..N {
        let w = |k: usize| WORDS[(i * 7 + k * 13) % WORDS.len()];
        let mut p = prop(
            scopes[i % 3],
            f.person,
            &format!(
                "Qué: {} con {} en el servicio {i} · Por qué: {} · Dónde: src/{}.rs · Aprendido: {}",
                w(0),
                w(1),
                w(2),
                w(3),
                w(4)
            ),
        );
        p.title = Some(format!("{} {} {i}", w(0), w(1)));
        p.category = Some(Category::ALL[i % Category::ALL.len()]);
        p.episode = Some(format!("sesion-{}", i / 20));
        // The person's quota is per hour: a minute between writes stays under it.
        f.clock.set(1000 + 60 * i as i64);
        f.engine.propose(p).unwrap();
    }
    let fill = t.elapsed();
    let opts = RecallOpts {
        search: SearchOpts {
            limit: 8,
            ..SearchOpts::default()
        },
        ..RecallOpts::default()
    };
    let mut search = Vec::new();
    for q in 0..100 {
        let query = format!(
            "{} {}",
            WORDS[q % WORDS.len()],
            WORDS[(q * 5 + 3) % WORDS.len()]
        );
        let t = Instant::now();
        let r = f.engine.recall_many(&scopes, &query, &opts).unwrap();
        search.push(t.elapsed());
        assert!(!r.hits.is_empty() || r.abstained, "{query}");
    }
    let mut recent = Vec::new();
    let mut timeline = Vec::new();
    for k in 0..50 {
        let t = Instant::now();
        let rows = f.engine.recent(project, 30).unwrap();
        recent.push(t.elapsed());
        let t = Instant::now();
        f.engine.timeline(rows[k % rows.len()].id, 5, 5).unwrap();
        timeline.push(t.elapsed());
    }
    let (p50, p95) = (percentile(&mut search, 0.5), percentile(&mut search, 0.95));
    println!(
        "scale: {N} memories filled in {:.1} s ({:.2} ms each)",
        fill.as_secs_f64(),
        fill.as_secs_f64() * 1000.0 / N as f64
    );
    println!("scale: search over 3 places p50 {p50:?} p95 {p95:?}");
    println!(
        "scale: recent p95 {:?}, timeline p95 {:?}",
        percentile(&mut recent, 0.95),
        percentile(&mut timeline, 0.95)
    );
    assert!(p95 < Duration::from_millis(500), "search p95 {p95:?}");
    assert!(percentile(&mut recent, 0.95) < Duration::from_millis(100));
    assert!(percentile(&mut timeline, 0.95) < Duration::from_millis(100));
}
