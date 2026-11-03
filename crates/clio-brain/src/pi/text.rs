//! Cleaning of text that comes from outside (stderr of Pi, names and titles inside session files,
//! the model's words when they are shown): no escape sequences, no control or bidi characters.

/// Characters that draw nothing or reorder what is around them, so text can say one thing and read
/// another or carry a message nobody sees: bidi controls, zero-width and joiner characters, soft
/// hyphen, the fillers that render as blanks (Hangul, braille blank, combining grapheme joiner),
/// variation selectors (except U+FE0F, which emoji need) and the tag characters (U+E0000), which
/// are the way to hide a whole sentence inside text a model will read. One list for every place
/// that cleans outside text, so they cannot drift apart.
pub fn is_deceptive(c: char) -> bool {
    matches!(c,
        '\u{034F}' | '\u{061C}' | '\u{00AD}' | '\u{115F}' | '\u{1160}' | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{2028}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'
        | '\u{2800}' | '\u{3164}'
        | '\u{FE00}'..='\u{FE0E}'
        | '\u{FEFF}' | '\u{FFA0}'
        | '\u{E0000}'..='\u{E007F}')
}

/// Consumes a CSI sequence after `ESC [`: parameter bytes, intermediate bytes, one final byte. A
/// character that cannot be part of one (a line break, a letter after a space...) ends it WITHOUT
/// being consumed, so a sequence that never ends does not eat the text after it.
fn skip_csi(it: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(&n) = it.peek() {
        match n {
            '\u{30}'..='\u{3f}' | '\u{20}'..='\u{2f}' => {
                it.next();
            }
            '\u{40}'..='\u{7e}' => {
                it.next();
                return;
            }
            _ => return,
        }
    }
}

/// Consumes a string sequence (OSC, DCS, APC, PM, SOS) up to BEL or ST. An unterminated one ends at
/// the end of its line, so it cannot hide what follows.
fn skip_string(it: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(&n) = it.peek() {
        match n {
            '\u{7}' => {
                it.next();
                return;
            }
            '\u{1b}' => {
                it.next();
                it.next(); // the `\` of ST
                return;
            }
            '\n' => return,
            _ => {
                it.next();
            }
        }
    }
}

/// Removes escape sequences (CSI, OSC, two-byte), C0/C1 controls, `is_deceptive` characters and
/// U+2028/U+2029. `keep_newlines` keeps `\n` (and turns `\t` into a space); otherwise every line
/// break becomes a space.
fn strip(s: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.peek().copied() {
                Some('[') => {
                    it.next();
                    skip_csi(&mut it);
                }
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    it.next();
                    skip_string(&mut it);
                }
                // A two-character sequence: the byte after ESC goes with it.
                Some(_) => {
                    it.next();
                }
                None => {}
            },
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => {
                if keep_newlines && c == '\n' {
                    out.push('\n');
                } else if !out.ends_with(' ') && !out.ends_with('\n') {
                    out.push(' ');
                }
            }
            '\t' => out.push(' '),
            c if c.is_control() || is_deceptive(c) => {}
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

    // Second audit: an escape sequence that never ends must not eat the rest of the text, line
    // breaks included (a title or a name is cut by the next line, not hidden to the end).
    #[test]
    fn an_unterminated_escape_does_not_swallow_the_rest() {
        assert_eq!(
            block("titulo\u{1b}[\nsegunda linea", 100),
            "titulo\nsegunda linea"
        );
        assert_eq!(block("a\u{1b}]0;sin cierre\nsegunda", 100), "a\nsegunda");
        assert_eq!(block("a\u{1b}Pdatos\nsegunda", 100), "a\nsegunda");
        // A well-formed sequence is still removed whole, with intermediates and parameters.
        assert_eq!(line("a\u{1b}[38;5;196mrojo\u{1b}[0m b", 100), "arojo b");
        assert_eq!(
            line("a\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7}", 100),
            "alink"
        );
    }

    // Second audit: characters that draw nothing but travel in a name or a title. Tag characters
    // (U+E0000) are the ones used to hide a whole instruction inside text a model will read.
    #[test]
    fn invisible_characters_that_hide_text_are_removed() {
        let hidden = "a\u{e0041}\u{e0042}b\u{3164}c\u{ffa0}d\u{2800}e\u{34f}f\u{115f}g\u{1160}h";
        assert_eq!(line(hidden, 100), "abcdefgh");
        assert_eq!(line("a\u{61c}b\u{ad}c\u{2060}d\u{206a}e", 100), "abcde");
        // What is visible is left alone: accents, emoji with their variation selector.
        assert_eq!(line("ñandú ❤\u{fe0f}", 100), "ñandú ❤\u{fe0f}");
    }

    #[test]
    fn long_text_is_cut_with_a_mark() {
        assert_eq!(line("abcdefghij", 5), "abcd…");
        assert_eq!(block("abcdefghij", 4), "…ghij");
        assert_eq!(line("ñandú ✓", 100), "ñandú ✓");
    }
}
