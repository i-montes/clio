//! A unified text diff with the standard library only (LCS over the lines that differ).
//! shortcut: quadratic table, capped at 4 million cells; a bigger difference is reported as
//! "too large to show line by line". Upgrade to Myers (or the `similar` crate) if that bites.

const CONTEXT: usize = 3;
const MAX_CELLS: usize = 4_000_000;
/// Bytes of a side that are split into lines at all. The line count (and so the table) is checked
/// only after splitting, and a file of line breaks has one line per byte.
const MAX_BYTES: usize = 8 << 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Del,
    Add,
}

fn is_text(b: &[u8]) -> bool {
    !b.contains(&0) && std::str::from_utf8(b).is_ok()
}

/// The lines of a text WITH their line endings: a file that differs only in `\r\n` against `\n`,
/// or in the final newline, must differ here (`str::lines` hides both).
fn lines_of(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// A line as shown: no line break, a visible mark for a carriage return, and git's note when the
/// last line has no newline.
fn shown(line: &str) -> String {
    let body = line.strip_suffix('\n');
    let text = body.unwrap_or(line).replace('\r', "\u{240d}");
    match body {
        Some(_) => text,
        None => format!("{text}\n\\ No newline at end of file"),
    }
}

fn ops(a: &[&str], b: &[&str]) -> Option<Vec<Op>> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    if (ma.len() + 1).saturating_mul(mb.len() + 1) > MAX_CELLS {
        return None;
    }
    let w = mb.len() + 1;
    let mut t = vec![0u32; (ma.len() + 1) * w];
    for i in (0..ma.len()).rev() {
        for j in (0..mb.len()).rev() {
            t[i * w + j] = if ma[i] == mb[j] {
                t[(i + 1) * w + j + 1] + 1
            } else {
                t[(i + 1) * w + j].max(t[i * w + j + 1])
            };
        }
    }
    let mut out = vec![Op::Same; pre];
    let (mut i, mut j) = (0, 0);
    while i < ma.len() || j < mb.len() {
        if i < ma.len() && j < mb.len() && ma[i] == mb[j] {
            out.push(Op::Same);
            i += 1;
            j += 1;
        } else if i < ma.len() && (j == mb.len() || t[(i + 1) * w + j] >= t[i * w + j + 1]) {
            out.push(Op::Del);
            i += 1;
        } else {
            out.push(Op::Add);
            j += 1;
        }
    }
    out.extend(std::iter::repeat_n(Op::Same, suf));
    Some(out)
}

/// Whether `file_diff` shows every changed line: false for a binary file and for a difference too
/// large to show line by line. What cannot be shown cannot be reviewed.
pub fn fully_shown(old: Option<&[u8]>, new: Option<&[u8]>) -> bool {
    if old.is_some_and(|b| !is_text(b)) || new.is_some_and(|b| !is_text(b)) {
        return false;
    }
    if old.is_some_and(|b| b.len() > MAX_BYTES) || new.is_some_and(|b| b.len() > MAX_BYTES) {
        return false;
    }
    let text = |b: Option<&[u8]>| String::from_utf8_lossy(b.unwrap_or_default()).into_owned();
    let (ot, nt) = (text(old), text(new));
    let (a, b) = (lines_of(&ot), lines_of(&nt));
    ops(&a, &b).is_some()
}

/// The diff of one file; `None` on a side means the file does not exist there.
pub fn file_diff(rel: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> String {
    // A line break in a name would start a line of its own and could pass for a header or a hunk.
    let rel = &rel.replace(['\n', '\r'], "\u{23ce}");
    let head = format!(
        "--- {}\n+++ {}\n",
        old.map_or("/dev/null".to_owned(), |_| format!("a/{rel}")),
        new.map_or("/dev/null".to_owned(), |_| format!("b/{rel}"))
    );
    if old.is_some_and(|b| !is_text(b)) || new.is_some_and(|b| !is_text(b)) {
        return format!("{head}binary file: changed\n");
    }
    if old.is_some_and(|b| b.len() > MAX_BYTES) || new.is_some_and(|b| b.len() > MAX_BYTES) {
        return format!(
            "{head}too large to show line by line: {} -> {} bytes\n",
            old.map_or(0, <[u8]>::len),
            new.map_or(0, <[u8]>::len)
        );
    }
    let text = |b: Option<&[u8]>| String::from_utf8_lossy(b.unwrap_or_default()).into_owned();
    let (ot, nt) = (text(old), text(new));
    let (a, b) = (lines_of(&ot), lines_of(&nt));
    let Some(ops) = ops(&a, &b) else {
        return format!(
            "{head}too large to show line by line: {} -> {} lines\n",
            a.len(),
            b.len()
        );
    };
    let mut out = head;
    // Line numbers (0-based) in a and b at each op.
    let mut pos = Vec::with_capacity(ops.len());
    let (mut i, mut j) = (0usize, 0usize);
    for op in &ops {
        pos.push((i, j));
        match op {
            Op::Same => (i, j) = (i + 1, j + 1),
            Op::Del => i += 1,
            Op::Add => j += 1,
        }
    }
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k] != Op::Same).collect();
    let mut k = 0;
    while k < changed.len() {
        let start = changed[k].saturating_sub(CONTEXT);
        let mut end = changed[k];
        while k + 1 < changed.len() && changed[k + 1] <= end + 2 * CONTEXT + 1 {
            k += 1;
            end = changed[k];
        }
        k += 1;
        let end = (end + CONTEXT + 1).min(ops.len());
        let (a0, b0) = pos[start];
        let na = ops[start..end].iter().filter(|o| **o != Op::Add).count();
        let nb = ops[start..end].iter().filter(|o| **o != Op::Del).count();
        out += &format!(
            "@@ -{},{} +{},{} @@\n",
            a0 + usize::from(na > 0),
            na,
            b0 + usize::from(nb > 0),
            nb
        );
        for m in start..end {
            let (pa, pb) = pos[m];
            match ops[m] {
                Op::Same => out += &format!(" {}\n", shown(a[pa])),
                Op::Del => out += &format!("-{}\n", shown(a[pa])),
                Op::Add => out += &format!("+{}\n", shown(b[pb])),
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // S-36 (second audit, finding 7): a file name is the model's. A newline in it must not forge
    // headers of the diff the person reviews.
    #[test]
    fn a_newline_in_a_file_name_cannot_forge_diff_headers() {
        let d = file_diff("a\n+++ b/README.md\n@@ -1 +1 @@\n+ok", None, Some(b"1\n"));
        assert_eq!(
            d.lines().filter(|l| l.starts_with("+++ ")).count(),
            1,
            "{d}"
        );
        assert_eq!(
            d.lines().filter(|l| l.starts_with("--- ")).count(),
            1,
            "{d}"
        );
        assert!(d.contains("b/a"), "the name is still recognisable: {d}");
        // An ordinary name, with spaces and accents, is untouched.
        assert!(
            file_diff("mi carpeta/ñandú.txt", None, Some(b"1\n"))
                .contains("+++ b/mi carpeta/ñandú.txt\n")
        );
    }

    // Second audit, finding 10: a change of line endings or of the final newline is a change the
    // person must see (a shebang line `#!/bin/sh\r` does not run). `lines()` hid both, and the file
    // counted as fully shown.
    #[test]
    fn a_change_of_line_ending_or_final_newline_is_shown() {
        let crlf = file_diff(
            "f.sh",
            Some(b"#!/bin/sh\r\necho\r\n"),
            Some(b"#!/bin/sh\necho\n"),
        );
        assert!(crlf.contains("@@") && crlf.contains('\u{240d}'), "{crlf}");
        let eof = file_diff("f", Some(b"a\n"), Some(b"a"));
        assert!(
            eof.contains("@@") && eof.contains("No newline at end of file"),
            "{eof}"
        );
        let same = file_diff("f", Some(b"a\r\nb\n"), Some(b"a\r\nb\n"));
        assert!(!same.contains("@@"), "{same}");
        assert!(fully_shown(Some(b"a\r\n"), Some(b"a\n")));
        // An ordinary edit is as before.
        assert!(file_diff("f", Some(b"a\nb\n"), Some(b"a\nB\n")).contains("-b\n+B\n"));
    }

    // Second audit, finding 11: the lines were all collected before the size cap was checked, so a
    // file of 64 MB of line breaks asked for about a gigabyte just to be refused.
    #[test]
    fn a_huge_file_is_refused_before_it_is_split_into_lines() {
        let big = vec![b'\n'; 40 << 20];
        let t = std::time::Instant::now();
        let d = file_diff("f", Some(&big), Some(b"x\n"));
        assert!(d.contains("too large to show line by line"), "{d}");
        assert!(!fully_shown(Some(&big), Some(b"x\n")));
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn modified_line_shows_both_sides() {
        let d = file_diff("f.txt", Some(b"a\nb\nc\n"), Some(b"a\nB\nc\n"));
        assert!(d.contains("--- a/f.txt\n+++ b/f.txt\n"));
        assert!(d.contains("@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"), "{d}");
    }

    #[test]
    fn created_and_deleted_and_binary() {
        assert!(
            file_diff("n", None, Some(b"x\n"))
                .contains("--- /dev/null\n+++ b/n\n@@ -0,0 +1,1 @@\n+x\n")
        );
        assert!(
            file_diff("n", Some(b"x\n"), None).contains("+++ /dev/null\n@@ -1,1 +0,0 @@\n-x\n")
        );
        assert!(file_diff("n", Some(&[0, 1]), Some(&[0, 2])).contains("binary file: changed"));
    }

    #[test]
    fn what_cannot_be_shown_is_not_fully_shown() {
        assert!(fully_shown(Some(b"a\n"), Some(b"b\n")));
        assert!(fully_shown(None, Some(b"new\n")));
        assert!(!fully_shown(Some(b"a\n"), Some(&[0xff, 0xfe])));
        // Changing only the first and last line of a big file leaves a middle too big to diff.
        let big =
            |edge: &str, mid: &str| format!("{edge}\n{}{edge}\n", format!("{mid}\n").repeat(2500));
        let (a, b) = (big("a", "x"), big("b", "y"));
        assert!(!fully_shown(Some(a.as_bytes()), Some(b.as_bytes())));
    }

    #[test]
    fn distant_changes_make_two_hunks() {
        let a: String = (0..40).map(|i| format!("{i}\n")).collect();
        let b = a.replace("\n2\n", "\nX\n").replace("\n35\n", "\nY\n");
        let d = file_diff("f", Some(a.as_bytes()), Some(b.as_bytes()));
        assert_eq!(d.matches("@@ ").count(), 2, "{d}");
    }
}
