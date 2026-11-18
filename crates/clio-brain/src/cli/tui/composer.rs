//! Editing the message in the chat's composer: a cursor in the text, moved and edited by the
//! usual keys. Pure functions over the text and a byte position; the position is put back on a
//! character boundary inside the text first, so a text changed elsewhere never makes it panic.

use super::text::{char_w, wrap_ranges};

/// `at` inside `s`, on a character boundary.
pub fn clamp(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn prev(s: &str, at: usize) -> usize {
    s[..at].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next(s: &str, at: usize) -> usize {
    s[at..].chars().next().map_or(at, |c| at + c.len_utf8())
}

pub fn left(s: &str, at: usize) -> usize {
    prev(s, clamp(s, at))
}

pub fn right(s: &str, at: usize) -> usize {
    next(s, clamp(s, at))
}

/// The start of the line the cursor is on.
pub fn line_start(s: &str, at: usize) -> usize {
    let at = clamp(s, at);
    s[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// The end of the line the cursor is on.
pub fn line_end(s: &str, at: usize) -> usize {
    let at = clamp(s, at);
    s[at..].find('\n').map_or(s.len(), |i| at + i)
}

/// The start of the word before the cursor (spaces first, then the word).
pub fn word_left(s: &str, at: usize) -> usize {
    let at = clamp(s, at);
    let head = s[..at].trim_end_matches(char::is_whitespace);
    head.rfind(char::is_whitespace).map_or(0, |i| {
        i + head[i..].chars().next().map_or(1, char::len_utf8)
    })
}

/// The end of the word after the cursor.
pub fn word_right(s: &str, at: usize) -> usize {
    let at = clamp(s, at);
    let tail = &s[at..];
    let skip = tail.len() - tail.trim_start_matches(char::is_whitespace).len();
    let rest = &tail[skip..];
    at + skip + rest.find(char::is_whitespace).unwrap_or(rest.len())
}

pub fn insert(s: &mut String, at: &mut usize, text: &str) {
    *at = clamp(s, *at);
    s.insert_str(*at, text);
    *at += text.len();
}

/// Removes `from..to` and leaves the cursor at `from`.
fn cut(s: &mut String, at: &mut usize, from: usize, to: usize) {
    s.replace_range(from..to, "");
    *at = from;
}

pub fn backspace(s: &mut String, at: &mut usize) {
    let to = clamp(s, *at);
    cut(s, at, prev(s, to), to);
}

pub fn delete(s: &mut String, at: &mut usize) {
    let from = clamp(s, *at);
    cut(s, at, from, next(s, from));
}

/// Ctrl-W: the word before the cursor.
pub fn delete_word(s: &mut String, at: &mut usize) {
    let to = clamp(s, *at);
    cut(s, at, word_left(s, to), to);
}

/// Ctrl-U: from the start of the line to the cursor (on a one-line message, all of it before).
pub fn delete_to_line_start(s: &mut String, at: &mut usize) {
    let to = clamp(s, *at);
    let from = line_start(s, to);
    // At the start of a line already: join it to the line above.
    let from = if from == to { prev(s, to) } else { from };
    cut(s, at, from, to);
}

/// Where the cursor is in `s` wrapped to `w` columns: (row, column).
pub fn locate(s: &str, at: usize, w: usize) -> (usize, usize) {
    let at = clamp(s, at);
    let rows = wrap_ranges(s, w);
    let row = rows.iter().rposition(|(a, _)| *a <= at).unwrap_or(0);
    let (a, b) = rows[row];
    (row, col_of(&s[a..at.min(b).max(a)]))
}

fn col_of(s: &str) -> usize {
    let mut p = None;
    s.chars()
        .map(|c| {
            let w = char_w(p, c);
            p = Some(c);
            w
        })
        .sum()
}

/// The cursor one row up (`up`) or down in `s` wrapped to `w` columns, at the same column or the
/// row's end; `None` on the first (last) row.
pub fn vertical(s: &str, at: usize, w: usize, up: bool) -> Option<usize> {
    let rows = wrap_ranges(s, w);
    let (row, col) = locate(s, at, w);
    let target = if up { row.checked_sub(1)? } else { row + 1 };
    let (a, b) = *rows.get(target)?;
    let mut used = 0;
    let mut p = None;
    for (i, c) in s[a..b].char_indices() {
        let cw = char_w(p, c);
        p = Some(c);
        if used + cw > col {
            return Some(a + i);
        }
        used += cw;
    }
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_by_character_word_and_line() {
        let s = "hola señor\nqué tal";
        let end = s.len();
        assert_eq!(left(s, end), end - 1);
        assert_eq!(left(s, 0), 0);
        assert_eq!(right(s, 7), 9, "ñ is two bytes");
        assert_eq!(line_start(s, end), s.find('q').unwrap());
        assert_eq!(line_end(s, 0), s.find('\n').unwrap());
        assert_eq!(word_left(s, end), s.find("tal").unwrap());
        assert_eq!(word_left(s, s.find("tal").unwrap()), s.find("qué").unwrap());
        assert_eq!(word_right(s, 0), 4);
        assert_eq!(word_right(s, 4), s.find('\n').unwrap());
        // A position inside a character is taken back to its start.
        assert_eq!(clamp(s, 8), 7);
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut s = String::from("hola mundo");
        let mut at = 4;
        insert(&mut s, &mut at, ", ñ");
        assert_eq!((s.as_str(), at), ("hola, ñ mundo", 8));
        backspace(&mut s, &mut at);
        assert_eq!((s.as_str(), at), ("hola,  mundo", 6));
        delete(&mut s, &mut at);
        assert_eq!(s, "hola, mundo");
        at = s.len();
        delete_word(&mut s, &mut at);
        assert_eq!((s.as_str(), at), ("hola, ", 6));
        let mut s = String::from("uno\ndos tres");
        let mut at = s.len();
        delete_to_line_start(&mut s, &mut at);
        assert_eq!((s.as_str(), at), ("uno\n", 4));
        delete_to_line_start(&mut s, &mut at);
        assert_eq!((s.as_str(), at), ("uno", 3));
        delete_to_line_start(&mut s, &mut at);
        assert_eq!((s.as_str(), at), ("", 0));
        // Nothing to remove: nothing happens.
        backspace(&mut s, &mut at);
        delete(&mut s, &mut at);
        assert_eq!((s.as_str(), at), ("", 0));
    }

    #[test]
    fn up_and_down_keep_the_column_across_wrapped_rows() {
        let s = "abcdef ghijkl\nxy";
        // Rows at 8 columns: "abcdef", "ghijkl", "xy".
        let at = s.find('i').unwrap();
        assert_eq!(locate(s, at, 8), (1, 2));
        assert_eq!(vertical(s, at, 8, true), Some(2));
        assert_eq!(vertical(s, at, 8, false), Some(s.len()));
        assert_eq!(vertical(s, 0, 8, true), None);
        assert_eq!(vertical(s, s.len(), 8, false), None);
        assert_eq!(locate("日本語", 6, 8), (0, 4));
    }
}
