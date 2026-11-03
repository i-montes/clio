use std::sync::LazyLock;

use regex::{Regex, RegexSet};

static KNOWN_FORMATS: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        r"\bsk-[A-Za-z0-9_-]{20,}",
        // No `\b` before these: they are specific enough, and padding glued in front of a key
        // (`aaaaAKIA...`) must not hide it. `sk-` keeps it: `task-management-system-design`
        // would match otherwise; a glued `sk-` key is found by its high-entropy body instead.
        r"gh[pousr]_[A-Za-z0-9]{36,}",
        r"AKIA[0-9A-Z]{16}",
        r"xox[abprs]-[A-Za-z0-9-]{10,}",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
    ])
    .expect("static secret patterns compile")
});

static LONG_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/=_-]{32,}").expect("static token pattern compiles"));

/// Shannon entropy (bits per byte); hex digests top out at 4.0, random base64 sits above 5.
fn entropy(s: &[u8]) -> f64 {
    let mut counts = [0usize; 256];
    for b in s {
        counts[usize::from(*b)] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

/// Entropy that marks a random token.
const RANDOM: f64 = 4.2;
/// A token is looked for in windows of this many bytes inside a long run, so that padding of
/// plain characters around it does not average its entropy away.
const WINDOW: usize = 40;

/// A run of token characters (always ASCII) that is, or hides, a random token.
fn is_random(run: &str) -> bool {
    let b = run.as_bytes();
    entropy(b) >= RANDOM
        || (b.len() > 2 * WINDOW && b.windows(WINDOW).step_by(8).any(|w| entropy(w) >= RANDOM))
}

/// True when the text looks like it contains a key or token.
pub fn find_secret(s: &str) -> bool {
    KNOWN_FORMATS.is_match(s) || LONG_TOKEN.find_iter(s).any(|m| is_random(m.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_known_formats() {
        let samples = [
            format!("my key is sk-{}", "a1B2c3D4e5F6g7H8i9J0k1L2"),
            format!("ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
            "AKIAABCDEFGHIJKLMNOP".to_string(),
            "token xoxb-1234567890-abcdefghij".to_string(),
            "-----BEGIN RSA PRIVATE KEY-----".to_string(),
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV"
                .to_string(),
        ];
        for s in samples {
            assert!(find_secret(&s), "should flag: {s}");
        }
    }

    #[test]
    fn flags_long_random_token() {
        assert!(find_secret("qZ8rT2vLx9KdWm4NpB7cYh3JfS6gAe1UoXi5VtRn0MbQ"));
    }

    // Second audit: padding in front of a key must not hide it. A word boundary before the
    // prefix and an entropy taken over the whole run both stopped seeing it.
    #[test]
    fn padding_does_not_hide_a_key() {
        let pad = "a".repeat(200);
        // In pieces: a scanner reads the source, not the program.
        let key = concat!("qZ8rT2vLx9KdWm4N", "pB7cYh3JfS6gAe1U", "oXi5VtRn0MbQ");
        for s in [
            format!("{pad}AKIAIOSFODNN7EXAMPLE"),
            format!("{pad}ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
            format!("{pad}{key}"),
            format!("{pad}sk-{key}"),
            format!("{key}{pad}"),
        ] {
            assert!(
                find_secret(&s),
                "should flag: {}...",
                &s[s.len().saturating_sub(70)..]
            );
        }
        // Long and boring is still boring.
        for s in [
            "a".repeat(5000),
            "ab".repeat(2500),
            "task-management-system-design-guidelines-for-the-whole-team".to_string(),
            format!(
                "{} {}",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".repeat(3),
                "fin"
            ),
        ] {
            assert!(!find_secret(&s), "should not flag a long plain run");
        }
    }

    #[test]
    fn looking_inside_long_runs_stays_fast() {
        let big = "Zq8rT2vLx9KdWm4NpB7cYh3".repeat(100_000); // 2.3 MB in one run
        let t = std::time::Instant::now();
        let _ = find_secret(&big);
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn ignores_ordinary_text() {
        let samples = [
            "A Marta le gusta correr por el parque los domingos por la mañana",
            "https://example.com/a/b?c=d",
            "123e4567-e89b-12d3-a456-426614174000",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "vive en Medellín desde 2026-03",
        ];
        for s in samples {
            assert!(!find_secret(s), "should not flag: {s}");
        }
    }
}
