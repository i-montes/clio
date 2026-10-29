use std::sync::LazyLock;

use regex::{Regex, RegexSet};

static KNOWN_FORMATS: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        r"\bsk-[A-Za-z0-9_-]{20,}",
        r"\bgh[pousr]_[A-Za-z0-9]{36,}",
        r"\bAKIA[0-9A-Z]{16}",
        r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        r"\beyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
    ])
    .expect("static secret patterns compile")
});

static LONG_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/=_-]{32,}").expect("static token pattern compiles"));

/// Shannon entropy (bits per byte); hex digests top out at 4.0, random base64 sits above 5.
fn entropy(s: &str) -> f64 {
    let mut counts = [0usize; 256];
    for b in s.bytes() {
        counts[usize::from(b)] += 1;
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

/// True when the text looks like it contains a key or token.
pub fn find_secret(s: &str) -> bool {
    KNOWN_FORMATS.is_match(s) || LONG_TOKEN.find_iter(s).any(|m| entropy(m.as_str()) >= 4.2)
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
