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
    assert!(out.contains("[secret:pattern redacted"), "{out}");
    assert!(out.contains("vault") && out.contains("person"), "{out}");
    assert!(!out.contains("ghp_"), "{out}");
}

#[test]
fn has_mask_tells_a_mark_from_ordinary_brackets() {
    let s = Scrubber::new(Arc::new(SystemClock));
    s.add("github", "a-very-long-secret-value-123");
    for masked in [
        s.scrub("x a-very-long-secret-value-123 y"),
        s.scrub(concat!("ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8")),
        "[secret:notion redacted]".to_owned(),
        // The Spanish spelling, in sessions, files and memories from before the English pass.
        "[secreto:notion tachado]".to_owned(),
        "[secreto:patrón tachado: it looks like a key and is not in the vault; do not write it back, ask the person for it]".to_owned(),
    ] {
        assert!(has_mask(&masked), "{masked}");
    }
    for plain in [
        "",
        "a list [1, 2, 3] and [secreto]",
        "[secreto:x without the word and closing bracket",
        "[secreto:x] tachado",
        "a list [1, 2, 3] and [secret]",
        "[secret:x without the word and closing bracket",
        "[secret:x] redacted",
        // Each spelling has its own word: a mix is neither.
        "[secret:x tachado]",
        "[secreto:x redacted]",
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
    assert_eq!(count_masks("[secret:x redacted"), 0);
    assert_eq!(count_masks("[secret:a redacted][secret:b redacted]"), 2);
    // Old and new marks in one text are each counted once.
    assert_eq!(count_masks(&format!("{two} [secreto:a tachado]")), 3);
}

// The marks are English now; nothing else about the filter changed: the same values and key
// shapes go, and what replaces them is the mark, word for word.
#[test]
fn a_value_and_a_key_shape_become_the_english_marks() {
    let s = Scrubber::new(Arc::new(SystemClock));
    s.add("github", "a-very-long-secret-value-123");
    let out = s.scrub(concat!(
        "a a-very-long-secret-value-123 b ghp_",
        "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8 c"
    ));
    assert_eq!(
        out,
        "a [secret:github redacted] b [secret:pattern redacted: it looks like a key and is not in the vault; do not write it back, ask the person for it] c"
    );
    let names: Vec<_> = s.redactions().into_iter().map(|r| r.name).collect();
    assert_eq!(names, ["github", "pattern"]);
}

// Striking text that already carries marks, of either spelling, changes nothing and logs nothing:
// no mark is marked again, and a mark cannot be used to hide a value from the filter.
#[test]
fn striking_marked_text_again_changes_nothing() {
    let s = Scrubber::new(Arc::new(SystemClock));
    s.add("github", "a-very-long-secret-value-123");
    let once = s.scrub(concat!(
        "a-very-long-secret-value-123 and ghp_",
        "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"
    ));
    let old = "[secreto:github tachado] [secreto:patrón tachado: it looks like a key and is not in the vault; do not write it back, ask the person for it]";
    let before = s.redaction_count();
    for marked in [once.as_str(), old] {
        assert_eq!(s.scrub(marked), marked);
    }
    assert_eq!(s.redaction_count(), before, "a mark is not a value");
    // A value written inside something shaped like a mark is still struck out.
    for fake in [
        "[secret:x a-very-long-secret-value-123 redacted]",
        "[secreto:x a-very-long-secret-value-123 tachado]",
    ] {
        let out = s.scrub(fake);
        assert!(!out.contains("a-very-long-secret-value-123"), "{out}");
    }
}
