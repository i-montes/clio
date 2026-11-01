//! Cleaning of text that comes from outside (stderr of Pi, names and titles inside session files,
//! the model's words when they are shown): no escape sequences, no control or bidi characters.

/// Removes escape sequences (CSI, OSC, two-byte), C0/C1 controls, bidi controls, zero-width
/// characters and U+2028/U+2029. `keep_newlines` keeps `\n` (and turns `\t` into a space);
/// otherwise every line break becomes a space.
fn strip(s: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.next() {
                Some('[') => {
                    for n in it.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    while let Some(n) = it.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            it.next(); // the `\` of ST
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => {
                if keep_newlines && c == '\n' {
                    out.push('\n');
                } else if !out.ends_with(' ') && !out.ends_with('\n') {
                    out.push(' ');
                }
            }
            '\t' => out.push(' '),
            c if c.is_control()
                || ('\u{202a}'..='\u{202e}').contains(&c)
                || ('\u{2066}'..='\u{2069}').contains(&c)
                || ('\u{200b}'..='\u{200f}').contains(&c)
                || c == '\u{feff}' => {}
            c => out.push(c),
        }
    }
    out
}

/// One clean line, at most `max_chars` characters (an ellipsis marks a cut), trimmed.
pub fn line(s: &str, max_chars: usize) -> String {
    let t = strip(s, false);
    let t = t.trim();
    if t.chars().count() <= max_chars {
        return t.to_owned();
    }
    let mut cut: String = t.chars().take(max_chars.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Clean text that keeps its line breaks (for stderr), at most `max_chars` characters from the END
/// (the last lines are the ones that explain a failure).
pub fn block(s: &str, max_chars: usize) -> String {
    let t = strip(s, true);
    let t = t.trim();
    let n = t.chars().count();
    if n <= max_chars {
        return t.to_owned();
    }
    let tail: String = t.chars().skip(n - max_chars).collect();
    format!("…{tail}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn escapes_controls_and_bidi_go_away() {
        let hostile = "a\u{1b}[31mred\u{1b}[0m\u{1b}]0;title\u{7}b\u{202e}c\u{0}d\u{85}e\u{2028}f";
        assert_eq!(line(hostile, 100), "aredbcde f");
        assert_eq!(block("x\u{1b}[2J\ny\tz", 100), "x\ny z");
    }

    #[test]
    fn long_text_is_cut_with_a_mark() {
        assert_eq!(line("abcdefghij", 5), "abcd…");
        assert_eq!(block("abcdefghij", 4), "…ghij");
        assert_eq!(line("ñandú ✓", 100), "ñandú ✓");
    }
}
