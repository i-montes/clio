//! Text measured in terminal columns, not in characters: an accent is one column, a CJK character
//! or an emoji two, a combining mark none. Wrapping and cutting go by that width, in one pass, so
//! a huge word costs its length and no more.

use unicode_width::UnicodeWidthChar;

/// The columns of `c` after `prev`: a variation selector that asks for the emoji form (U+FE0F)
/// widens a one-column character to two, as the terminal draws it.
pub fn char_w(prev: Option<char>, c: char) -> usize {
    if c == '\u{FE0F}' {
        return usize::from(prev.is_some_and(|p| p.width() == Some(1)));
    }
    c.width().unwrap_or(0)
}

/// The columns of `s`.
pub fn width(s: &str) -> usize {
    let mut prev = None;
    s.chars()
        .map(|c| {
            let w = char_w(prev, c);
            prev = Some(c);
            w
        })
        .sum()
}

/// `s` cut to `w` columns, ending in `ellipsis` when something was cut.
pub fn fit(s: &str, w: usize, ellipsis: &str) -> String {
    if width(s) <= w {
        return s.to_owned();
    }
    let room = w.saturating_sub(width(ellipsis));
    let mut out = String::new();
    let (mut used, mut prev) = (0, None);
    for c in s.chars() {
        let cw = char_w(prev, c);
        if used + cw > room {
            break;
        }
        used += cw;
        out.push(c);
        prev = Some(c);
    }
    out.push_str(ellipsis);
    out
}

/// Where `text` breaks into rows of at most `w` columns: byte ranges, one per row. A newline always
/// breaks; a row breaks at its last space when it can (the space is left out of both rows) and
/// inside a word only when the word alone is wider than the row. Nothing but those spaces and the
/// newlines is lost: every other byte is in exactly one range.
pub fn wrap_ranges(text: &str, w: usize) -> Vec<(usize, usize)> {
    let w = w.max(2);
    let mut out = Vec::new();
    let mut start = 0;
    let mut used = 0;
    // The last space of the row and the width after it.
    let mut space: Option<usize> = None;
    let mut after = 0;
    let mut prev = None;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            out.push((start, i));
            (start, used, space, after, prev) = (i + 1, 0, None, 0, None);
            continue;
        }
        let cw = char_w(prev, c);
        prev = Some(c);
        if used + cw > w && i > start {
            if c == ' ' {
                out.push((start, i));
                (start, used, space, after) = (i + 1, 0, None, 0);
                continue;
            }
            match space {
                Some(sp) if after + cw <= w => {
                    out.push((start, sp));
                    (start, used) = (sp + 1, after);
                }
                _ => {
                    out.push((start, i));
                    (start, used) = (i, 0);
                }
            }
            (space, after) = (None, 0);
        }
        used += cw;
        if c == ' ' {
            (space, after) = (Some(i), 0);
        } else {
            after += cw;
        }
    }
    out.push((start, text.len()));
    out
}

/// `text` broken into rows of at most `w` columns (see `wrap_ranges`).
pub fn wrap(text: &str, w: usize) -> Vec<String> {
    wrap_ranges(text, w)
        .into_iter()
        .map(|(a, b)| text[a..b].to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_columns_not_characters() {
        assert_eq!(width("año"), 3);
        assert_eq!(width("日本"), 4);
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(width("😀"), 2);
        assert_eq!(width("❤\u{fe0f}"), 2);
    }

    #[test]
    fn wrapping_goes_by_display_width() {
        let rows = wrap("日本語の文章です とても長い", 8);
        assert!(rows.iter().all(|r| width(r) <= 8), "{rows:?}");
        assert_eq!(rows.concat().replace(' ', ""), "日本語の文章ですとても長い");
        let rows = wrap("ñandú camión acción ñoño", 12);
        assert_eq!(rows, ["ñandú camión", "acción ñoño"]);
        // A word that fits is never split; one that does not is cut on a character boundary.
        assert_eq!(wrap("ab abcdefghij", 6), ["ab", "abcdef", "ghij"]);
    }

    #[test]
    fn fit_cuts_by_width_and_marks_the_cut() {
        assert_eq!(fit("hola", 10, "…"), "hola");
        assert_eq!(fit("日本語の文章", 7, "…"), "日本語…");
        assert!(width(&fit("日本語の文章", 7, "…")) <= 7);
    }
}
