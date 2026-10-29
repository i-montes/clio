use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// Canonical form used for hashing and indexing: NFC, trimmed, whitespace runs collapsed.
pub fn normalize(s: &str) -> String {
    let nfc: String = s.nfc().collect();
    nfc.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// SHA-256 (hex) of already-normalized text.
pub fn content_hash(normalized: &str) -> String {
    Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composed_and_decomposed_hash_equal() {
        let a = content_hash(&normalize("caf\u{e9}"));
        let b = content_hash(&normalize("cafe\u{301}"));
        assert_eq!(a, b);
    }

    #[test]
    fn whitespace_collapsed() {
        assert_eq!(normalize("  a \n\t b "), "a b");
    }
}
