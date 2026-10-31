//! A unified text diff with the standard library only (LCS over the lines that differ).
//! shortcut: quadratic table, capped at 4 million cells; a bigger difference is reported as
//! "too large to show line by line". Upgrade to Myers (or the `similar` crate) if that bites.

const CONTEXT: usize = 3;
const MAX_CELLS: usize = 4_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Del,
    Add,
}

fn is_text(b: &[u8]) -> bool {
    !b.contains(&0) && std::str::from_utf8(b).is_ok()
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

/// The diff of one file; `None` on a side means the file does not exist there.
pub fn file_diff(rel: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> String {
    let head = format!(
        "--- {}\n+++ {}\n",
        old.map_or("/dev/null".to_owned(), |_| format!("a/{rel}")),
        new.map_or("/dev/null".to_owned(), |_| format!("b/{rel}"))
    );
    if old.is_some_and(|b| !is_text(b)) || new.is_some_and(|b| !is_text(b)) {
        return format!("{head}binary file: changed\n");
    }
    let text = |b: Option<&[u8]>| String::from_utf8_lossy(b.unwrap_or_default()).into_owned();
    let (ot, nt) = (text(old), text(new));
    let (a, b): (Vec<&str>, Vec<&str>) = (ot.lines().collect(), nt.lines().collect());
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
                Op::Same => out += &format!(" {}\n", a[pa]),
                Op::Del => out += &format!("-{}\n", a[pa]),
                Op::Add => out += &format!("+{}\n", b[pb]),
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

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
    fn distant_changes_make_two_hunks() {
        let a: String = (0..40).map(|i| format!("{i}\n")).collect();
        let b = a.replace("\n2\n", "\nX\n").replace("\n35\n", "\nY\n");
        let d = file_diff("f", Some(a.as_bytes()), Some(b.as_bytes()));
        assert_eq!(d.matches("@@ ").count(), 2, "{d}");
    }
}
