//! Printing helpers. Write errors (a closed pipe, `| head`) are ignored: there is nobody left
//! to tell.

use std::io::Write;

pub fn line(text: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{text}");
}

pub fn error(text: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{text}");
}

/// Memory text is data from anywhere, so a terminal must never interpret it: control
/// characters (an ANSI escape, a carriage return that overwrites the line) are shown as `�`.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

/// Like `clean`, for fields shown over several lines: newlines and tabs stay.
pub fn clean_multiline(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}
