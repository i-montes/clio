//! Printing helpers. Write errors (a closed pipe, `| head`) are ignored: there is nobody left
//! to tell.

use std::io::Write;

pub fn line(text: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{text}");
}

pub fn error(text: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{text}");
}

/// Characters that draw nothing or reorder what is around them: text that reads differently from
/// what is stored. The brain's own cleaning replaces the same ones.
fn deceptive(c: char) -> bool {
    matches!(
        c,
        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
    )
}

/// Memory text is data from anywhere, so a terminal must never interpret it: control
/// characters (an ANSI escape, a carriage return that overwrites the line) and invisible or
/// reordering ones (a right-to-left override) are shown as `�`.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || deceptive(c) {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

/// Like `clean`, for fields shown over several lines: newlines and tabs stay.
pub fn clean_multiline(text: &str) -> String {
    text.chars()
        .map(|c| {
            if (c.is_control() && c != '\n' && c != '\t') || deceptive(c) {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Second audit: memory text from `import-memories` did not pass through the brain's cleaning,
    // and a right-to-left override made `review` show text that reads differently from what is
    // stored, on the screen where a person decides to promote it.
    #[test]
    fn invisible_and_reordering_characters_are_shown_as_replacements() {
        for c in [
            '\u{202e}', '\u{2066}', '\u{200b}', '\u{200f}', '\u{feff}', '\u{1b}',
        ] {
            let t = format!("a{c}b");
            assert_eq!(clean(&t), "a\u{fffd}b", "{c:?}");
            assert_eq!(clean_multiline(&t), "a\u{fffd}b", "{c:?}");
        }
        assert_eq!(clean("vive en Medellín, ñandú"), "vive en Medellín, ñandú");
        assert_eq!(clean_multiline("uno\n\tdos"), "uno\n\tdos");
    }
}
