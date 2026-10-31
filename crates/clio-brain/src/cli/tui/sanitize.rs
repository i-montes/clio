//! Text that does not come from us (file names, file contents, diffs, the model's messages, the
//! instruction) can carry escape sequences and control characters that drive the terminal: change
//! the title, clear or redraw the screen, overwrite a line with `\r`, hide text, or flip it with
//! bidirectional marks. Everything external goes through here BEFORE it is drawn or printed.
//!
//! Rules: complete escape sequences (CSI, OSC, DCS/SOS/PM/APC, two-character ones) are removed;
//! any other control character, line/paragraph separator, bidi control or invisible format
//! character becomes `U+FFFD`; a tab becomes four spaces; a newline stays only in the multi-line
//! form.
//! shortcut: invisible joiners (ZWJ) are replaced too, so a joined emoji shows as separate ones.

const REPLACEMENT: char = '\u{FFFD}';

/// Bidi controls and invisible characters that can reorder or hide what the person reads.
fn is_deceptive(c: char) -> bool {
    matches!(c,
        '\u{061C}' | '\u{00AD}' | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{2028}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}')
}

/// One-line text: newlines become spaces.
pub fn line(text: &str) -> String {
    run(text, false)
}

/// Several lines: newlines stay.
pub fn multiline(text: &str) -> String {
    run(text, true)
}

fn run(text: &str, keep_newline: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => skip_escape(&mut it),
            '\u{9b}' => skip_csi(&mut it),
            '\u{9d}' => skip_string(&mut it),
            '\n' => out.push(if keep_newline { '\n' } else { ' ' }),
            '\t' => out.push_str("    "),
            c if c.is_control() || is_deceptive(c) => out.push(REPLACEMENT),
            c => out.push(c),
        }
    }
    out
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// After `ESC`: consume the whole sequence.
fn skip_escape(it: &mut Chars) {
    match it.peek().copied() {
        Some('[') => {
            it.next();
            skip_csi(it);
        }
        // OSC, DCS, SOS, PM, APC: a string that ends at BEL or ST.
        Some(']' | 'P' | 'X' | '^' | '_') => {
            it.next();
            skip_string(it);
        }
        // Two- or three-character sequences: intermediates 0x20-0x2F, then a final 0x30-0x7E.
        Some(c) if ('\u{20}'..='\u{2f}').contains(&c) => {
            while it.next_if(|c| ('\u{20}'..='\u{2f}').contains(c)).is_some() {}
            it.next_if(|c| ('\u{30}'..='\u{7e}').contains(c));
        }
        Some(c) if ('\u{30}'..='\u{7e}').contains(&c) => {
            it.next();
        }
        _ => {}
    }
}

/// Parameters 0x30-0x3F, intermediates 0x20-0x2F, one final byte 0x40-0x7E.
fn skip_csi(it: &mut Chars) {
    while it.next_if(|c| ('\u{30}'..='\u{3f}').contains(c)).is_some() {}
    while it.next_if(|c| ('\u{20}'..='\u{2f}').contains(c)).is_some() {}
    it.next_if(|c| ('\u{40}'..='\u{7e}').contains(c));
}

/// Up to BEL or `ESC \`. It also stops at a newline (left in place), so an unterminated string
/// cannot swallow the lines that follow.
fn skip_string(it: &mut Chars) {
    while let Some(&c) = it.peek() {
        match c {
            '\n' => return,
            '\u{7}' | '\u{9c}' => {
                it.next();
                return;
            }
            '\u{1b}' => {
                it.next();
                if it.next_if(|c| *c == '\\').is_some() {
                    return;
                }
            }
            _ => {
                it.next();
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn nasty(s: &str) -> bool {
        s.chars()
            .any(|c| (c.is_control() && c != '\n') || is_deceptive(c))
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(line("hola, año 2026 ✓ 日本"), "hola, año 2026 ✓ 日本");
        assert_eq!(multiline("a\nb"), "a\nb");
        assert_eq!(line("a\nb"), "a b");
        assert_eq!(line("a\tb"), "a    b");
    }

    #[test]
    fn escape_sequences_are_removed_whole() {
        for (hostile, shown) in [
            ("a\u{1b}[2Jb", "ab"),
            ("a\u{1b}[31;1mred\u{1b}[0m", "ared"),
            ("a\u{1b}[?1049hb", "ab"),
            ("a\u{1b}]0;pwned\u{7}b", "ab"),
            ("a\u{1b}]2;pwned\u{1b}\\b", "ab"),
            ("a\u{1b}]8;;http://evil\u{7}link\u{1b}]8;;\u{7}", "alink"),
            ("a\u{1b}Pq#0;2;0;0;0\u{1b}\\b", "ab"),
            ("a\u{1b}_Gf=100;AAAA\u{1b}\\b", "ab"),
            ("a\u{1b}(Bb", "ab"),
            ("a\u{1b}cb", "ab"),
            ("a\u{9b}2Jb", "ab"),
            ("a\u{9d}0;t\u{7}b", "ab"),
        ] {
            let got = line(hostile);
            assert_eq!(got, shown, "{hostile:?}");
            assert!(!nasty(&got));
        }
    }

    #[test]
    fn an_unterminated_string_does_not_eat_the_next_line() {
        assert_eq!(multiline("x\u{1b}]0;title\nsecond line"), "x\nsecond line");
    }

    #[test]
    fn carriage_return_and_other_controls_become_a_visible_mark() {
        assert_eq!(line("safe\rEVIL"), "safe\u{FFFD}EVIL");
        assert_eq!(line("a\u{0}b\u{7}c\u{7f}d\u{8}e"), "a\u{FFFD}b\u{FFFD}c\u{FFFD}d\u{FFFD}e");
        assert_eq!(line("a\u{2028}b"), "a\u{FFFD}b");
    }

    #[test]
    fn bidi_and_invisible_characters_are_marked() {
        // "txt.exe" shown as "exe.txt" by a right-to-left override.
        let name = "report\u{202E}txt.exe";
        let got = line(name);
        assert_eq!(got, "report\u{FFFD}txt.exe");
        for c in ['\u{202A}', '\u{202D}', '\u{2066}', '\u{2069}', '\u{200E}', '\u{200B}', '\u{FEFF}', '\u{061C}'] {
            assert_eq!(line(&format!("a{c}b")), "a\u{FFFD}b", "{c:?}");
        }
    }

    #[test]
    fn a_lone_escape_at_the_end_is_dropped() {
        assert_eq!(line("abc\u{1b}"), "abc");
        assert_eq!(line("abc\u{1b}["), "abc");
    }
}
