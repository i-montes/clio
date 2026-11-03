#![allow(clippy::unwrap_used)]
//! The marks the filter leaves in text (S-18, S-20): a mark says what it is and what to do, and
//! a text that carries one can be recognised, so it is never written back over a real value.

use std::sync::Arc;

use clio_core::SystemClock;
use clio_vault::{Scrubber, count_masks, has_mask};

#[test]
fn a_mark_for_a_value_the_vault_does_not_know_says_what_to_do() {
    let s = Scrubber::new(Arc::new(SystemClock));
    let out = s.scrub(concat!(
        "TOKEN=ghp_",
        "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8",
        "\n"
    ));
    assert!(out.contains("[secreto:patrón tachado"), "{out}");
    assert!(out.contains("baúl") && out.contains("persona"), "{out}");
    assert!(!out.contains("ghp_"), "{out}");
}

#[test]
fn has_mask_tells_a_mark_from_ordinary_brackets() {
    let s = Scrubber::new(Arc::new(SystemClock));
    s.add("github", "a-very-long-secret-value-123");
    for masked in [
        s.scrub("x a-very-long-secret-value-123 y"),
        s.scrub(concat!("ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8")),
        "[secreto:notion tachado]".to_owned(),
    ] {
        assert!(has_mask(&masked), "{masked}");
    }
    for plain in [
        "",
        "a list [1, 2, 3] and [secreto]",
        "[secreto:x without the word and closing bracket",
        "[secreto:x] tachado",
    ] {
        assert!(!has_mask(plain), "{plain}");
    }
}

// More shapes of provider keys than before: they were read by the model as plain text.
#[test]
fn more_key_shapes_are_masked() {
    let s = Scrubber::new(Arc::new(SystemClock));
    for token in [
        format!("npm_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8"),
        format!("glpat-{}", "a1B2c3D4e5F6g7H8i9J0"),
        format!("sk_live_{}", "a1B2c3D4e5F6g7H8i9J0k1L2"),
        format!("ghu_{}", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
        format!("hf_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7"),
    ] {
        let out = s.scrub(&format!("key={token}\n"));
        assert!(!out.contains(&token), "{out}");
    }
}

#[test]
fn count_masks_counts_every_mark_of_both_kinds() {
    let s = Scrubber::new(Arc::new(SystemClock));
    s.add("github", "a-very-long-secret-value-123");
    assert_eq!(count_masks(""), 0);
    assert_eq!(count_masks("nothing [secreto] here, [1, 2]"), 0);
    let two = s.scrub(concat!(
        "a-very-long-secret-value-123 and ghp_",
        "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"
    ));
    assert_eq!(count_masks(&two), 2, "{two}");
    // A mark the person typed by hand, with no closing bracket, is not one.
    assert_eq!(count_masks("[secreto:x tachado"), 0);
    assert_eq!(count_masks("[secreto:a tachado][secreto:b tachado]"), 2);
}
