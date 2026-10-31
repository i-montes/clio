//! Made-up data for tests and benchmarks: facts for a memory and files for a node to read.
//! Everything comes from fictional names and a seed, so a run is reproducible and no real
//! person, address or document is ever involved. It depends on no other Clio crate.

use std::io;
use std::path::{Path, PathBuf};

const PEOPLE: [&str; 8] = [
    "Marta", "Julián", "Dora", "Tomás", "Lucía", "Hernán", "Inés", "Camilo",
];
const CITIES: [&str; 6] = [
    "Aldea Norte",
    "Puerto Claro",
    "Villa Alta",
    "Monte Sol",
    "Río Hondo",
    "Bahía Mansa",
];
const THINGS: [&str; 8] = [
    "el té frío",
    "las respuestas breves",
    "madrugar",
    "los mapas de papel",
    "el jazz",
    "caminar de noche",
    "los cuadernos de hojas lisas",
    "el pan de ayer",
];
const JOBS: [&str; 6] = [
    "carpintería",
    "cartografía",
    "panadería",
    "enfermería",
    "contabilidad",
    "jardinería",
];

/// A fact of the kind a memory holds. `topic` is stable, so a newer fact with the same topic
/// replaces an older one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub text: String,
    pub topic: String,
}

/// splitmix64: small, fast and the same everywhere; this is not for security.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

/// `n` facts, the same ones for the same seed. Each one carries its own number, so no two
/// texts are equal and none counts as a duplicate of another.
pub fn facts(seed: u64, n: usize) -> Vec<Fact> {
    let mut rng = Rng(seed);
    (0..n)
        .map(|i| {
            let person = rng.pick(&PEOPLE);
            let who = format!("{person} {i}");
            let year = 2020 + rng.next() % 7;
            let month = 1 + rng.next() % 12;
            match rng.next() % 3 {
                0 => Fact {
                    text: format!(
                        "{who} vive en {} desde {year}-{month:02}",
                        rng.pick(&CITIES)
                    ),
                    topic: format!("residencia:{who}"),
                },
                1 => Fact {
                    text: format!("{who} prefiere {}", rng.pick(&THINGS)),
                    topic: format!("gusto:{who}"),
                },
                _ => Fact {
                    text: format!("{who} trabaja en {}", rng.pick(&JOBS)),
                    topic: format!("trabajo:{who}"),
                },
            }
        })
        .collect()
}

/// Writes `n` small text files under `dir` (created if missing) and returns their paths. The
/// emails in them end in `.invalid`, which can never be delivered.
pub fn write_files(seed: u64, dir: &Path, n: usize) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut rng = Rng(seed ^ 0xF11E);
    (0..n)
        .map(|i| {
            let person = rng.pick(&PEOPLE);
            let path = dir.join(format!("nota-{i:04}.txt"));
            let body = format!(
                "Nota {i}\nDe: {person} <{}{i}@ejemplo.invalid>\nAsunto: {}\n\nHoy hablamos de {} en {}.\n",
                person.to_lowercase(),
                rng.pick(&JOBS),
                rng.pick(&THINGS),
                rng.pick(&CITIES),
            );
            std::fs::write(&path, body)?;
            Ok(path)
        })
        .collect()
}
