//! Text that came from outside (file contents, file names, test output) is cleaned before it
//! goes anywhere a terminal or a log could show it: terminal escape sequences and invisible
//! characters that could hide or fake text are removed.

/// Removes ANSI escape sequences (CSI, OSC, and two-byte ones), control characters other than
/// `\n` and `\t`, and the characters that reorder or hide text (bidi controls, zero-width,
/// byte-order mark, line and paragraph separators).
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.peek().copied() {
                // CSI: parameters, then one final byte 0x40..=0x7e.
                Some('[') => {
                    it.next();
                    for n in it.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC / DCS / APC / PM / SOS: until BEL or ST (ESC \).
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    it.next();
                    while let Some(n) = it.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            if it.peek() == Some(&'\\') {
                                it.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    it.next();
                }
                None => {}
            },
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn escape_sequences_and_invisible_text_are_removed() {
        assert_eq!(sanitize("a\u{1b}[31mred\u{1b}[0m b"), "ared b");
        assert_eq!(sanitize("x\u{1b}]0;title\u{7}y"), "xy");
        assert_eq!(
            sanitize("x\u{1b}]8;;http://e\u{1b}\\link\u{1b}]8;;\u{1b}\\"),
            "xlink"
        );
        assert_eq!(sanitize("a\rb\u{8}c\u{0}d"), "abcd");
        assert_eq!(sanitize("a\u{202e}b\u{200b}c\u{feff}"), "abc");
        assert_eq!(
            sanitize("keep\nlines\tand tabs é"),
            "keep\nlines\tand tabs é"
        );
        assert_eq!(sanitize("end\u{1b}"), "end");
    }

    /// A control character in the middle of a secret must not survive: the scrubber runs after
    /// this, on the cleaned text, so a value can not hide behind an invisible character.
    #[test]
    fn a_hidden_character_cannot_split_a_value() {
        assert_eq!(sanitize("sk-ab\u{200b}cd"), "sk-abcd");
    }
}
