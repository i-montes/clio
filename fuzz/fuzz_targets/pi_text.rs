#![no_main]
use libfuzzer_sys::fuzz_target;

// Text from Pi (its stderr, titles and names inside its session files) is hostile. Whatever the
// bytes, what comes out has no control character and nothing that hides or reorders text, and a
// cut never exceeds what was asked.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for max in [0usize, 1, 8, 200] {
        let one = clio_brain::pi::text::line(&text, max);
        assert!(one.chars().count() <= max.max(1));
        assert!(!one.chars().any(|c| c.is_control() || clio_brain::pi::text::is_deceptive(c)));
        let many = clio_brain::pi::text::block(&text, max);
        assert!(many.chars().count() <= max + 1);
        assert!(
            !many
                .chars()
                .any(|c| (c.is_control() && c != '\n') || clio_brain::pi::text::is_deceptive(c))
        );
    }
});
