use std::collections::HashSet;

use clio_synthetic::{facts, write_files};

#[test]
fn the_same_seed_gives_the_same_facts_and_another_seed_differs() {
    assert_eq!(facts(1, 200), facts(1, 200));
    assert_ne!(facts(1, 200), facts(2, 200));
}

#[test]
fn no_two_facts_are_equal_and_every_topic_is_unique() {
    let all = facts(7, 5_000);
    assert_eq!(
        all.iter().map(|f| &f.text).collect::<HashSet<_>>().len(),
        5_000
    );
    assert_eq!(
        all.iter().map(|f| &f.topic).collect::<HashSet<_>>().len(),
        5_000
    );
}

#[test]
fn nothing_generated_looks_like_a_secret_or_a_real_address() {
    let dir = std::env::temp_dir().join(format!("clio-synthetic-{}", std::process::id()));
    let files = write_files(3, &dir, 20).expect("files");
    assert_eq!(files.len(), 20);
    let mut text: Vec<String> = facts(3, 500).into_iter().map(|f| f.text).collect();
    for f in &files {
        text.push(std::fs::read_to_string(f).expect("read"));
    }
    for t in &text {
        // The longest run of letters, digits and symbols a key would be made of.
        let run = t
            .split(|c: char| !(c.is_ascii_alphanumeric() || "+/=_-".contains(c)))
            .map(str::len)
            .max()
            .unwrap_or(0);
        assert!(run < 32, "a long token-like run in {t:?}");
        if t.contains('@') {
            assert!(t.contains("@ejemplo.invalid"), "{t:?}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}
