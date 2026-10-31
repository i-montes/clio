//! How the plain (non-TUI) commands look: color and symbols only on a real terminal, plain
//! stable text everywhere else (pipes, CI, files). Honors `NO_COLOR` and `TERM=dumb`.

use std::io::IsTerminal;

/// Whether to color and whether to use Unicode symbols, decided once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// stdout is a terminal: external text must be sanitized before it is printed.
    pub tty: bool,
    pub color: bool,
    pub unicode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Pass,
    Warn,
    Fail,
}

impl Style {
    /// Plain text: what a pipe gets.
    #[cfg(test)]
    pub const PLAIN: Self = Self {
        tty: false,
        color: false,
        unicode: false,
    };

    /// For stdout of this process.
    pub fn stdout() -> Self {
        let env = |k: &str| std::env::var(k).ok();
        Self::decide(
            std::io::stdout().is_terminal(),
            env("NO_COLOR").as_deref(),
            env("TERM").as_deref(),
            [env("LC_ALL"), env("LC_CTYPE"), env("LANG")]
                .into_iter()
                .flatten()
                .find(|v| !v.is_empty())
                .as_deref(),
        )
    }

    /// The decision as a pure function, so it can be tested without touching the environment.
    pub fn decide(
        tty: bool,
        no_color: Option<&str>,
        term: Option<&str>,
        locale: Option<&str>,
    ) -> Self {
        let dumb = term.is_none_or(|t| t == "dumb");
        let utf8 = locale.is_some_and(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("utf-8") || l.contains("utf8")
        });
        Self {
            tty,
            color: tty && !dumb && no_color.is_none_or(str::is_empty),
            unicode: tty && !dumb && term != Some("linux") && utf8,
        }
    }

    fn paint(self, code: &str, s: &str) -> String {
        if self.color {
            format!("\u{1b}[{code}m{s}\u{1b}[0m")
        } else {
            s.to_owned()
        }
    }

    pub fn bold(self, s: &str) -> String {
        self.paint("1", s)
    }
    pub fn dim(self, s: &str) -> String {
        self.paint("2", s)
    }
    pub fn green(self, s: &str) -> String {
        self.paint("32", s)
    }
    pub fn yellow(self, s: &str) -> String {
        self.paint("33", s)
    }
    pub fn red(self, s: &str) -> String {
        self.paint("31", s)
    }
    pub fn cyan(self, s: &str) -> String {
        self.paint("36", s)
    }

    /// The mark of a check: a symbol on a Unicode terminal, the word otherwise.
    pub fn mark(self, l: Level) -> String {
        match (l, self.unicode) {
            (Level::Pass, true) => self.green("✓"),
            (Level::Warn, true) => self.yellow("⚠"),
            (Level::Fail, true) => self.red("✗"),
            (Level::Pass, false) => self.green("PASS"),
            (Level::Warn, false) => self.yellow("WARN"),
            (Level::Fail, false) => self.red("FAIL"),
        }
    }

    /// Width the mark occupies once printed, to align what follows.
    pub fn mark_width(self) -> usize {
        if self.unicode { 1 } else { 4 }
    }

    pub fn arrow(self) -> &'static str {
        if self.unicode { "→" } else { "->" }
    }

    pub fn bullet(self) -> &'static str {
        if self.unicode { "·" } else { "-" }
    }

    pub fn ellipsis(self) -> &'static str {
        if self.unicode { "…" } else { "..." }
    }
}

/// `s` cut to `n` characters (one line), with an ellipsis when it was longer.
pub fn clip(st: Style, s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > n {
        let keep = n.saturating_sub(st.ellipsis().chars().count());
        format!(
            "{}{}",
            one.chars().take(keep).collect::<String>(),
            st.ellipsis()
        )
    } else {
        one
    }
}

/// `text` broken into lines of at most `width` characters, at spaces when it can; each source
/// line is kept apart. Long words are cut.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for src in text.lines() {
        let mut cur = String::new();
        for word in src.split_whitespace() {
            let mut word = word.to_owned();
            while word.chars().count() > width {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(word.chars().take(width).collect());
                word = word.chars().skip(width).collect();
            }
            let extra = usize::from(!cur.is_empty());
            if cur.chars().count() + extra + word.chars().count() > width {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(&word);
        }
        out.push(cur);
    }
    out
}

/// Terminal width from `COLUMNS`, else 80.
pub fn width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .filter(|w| *w >= 40)
        .map_or(80, |w| w.min(120))
}

/// Pads to `n` columns by characters (the text is already printable, no escapes).
pub fn pad(s: &str, n: usize) -> String {
    let w = s.chars().count();
    format!("{s}{}", " ".repeat(n.saturating_sub(w)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_gets_plain_ascii() {
        let s = Style::decide(false, None, Some("xterm-256color"), Some("es_CO.UTF-8"));
        assert_eq!(s, Style::PLAIN);
        assert_eq!(s.mark(Level::Fail), "FAIL");
        assert_eq!(s.bold("x"), "x");
    }

    #[test]
    fn a_terminal_gets_color_and_symbols() {
        let s = Style::decide(true, None, Some("xterm-256color"), Some("es_CO.UTF-8"));
        assert!(s.color && s.unicode);
        assert!(s.mark(Level::Pass).contains('✓') && s.red("x").starts_with("\u{1b}[31m"));
    }

    #[test]
    fn no_color_and_dumb_and_non_utf8_are_honored() {
        let t = |nc, term, loc| Style::decide(true, nc, term, loc);
        assert!(!t(Some("1"), Some("xterm"), Some("C.UTF-8")).color);
        assert!(t(Some("1"), Some("xterm"), Some("C.UTF-8")).unicode);
        assert!(
            t(Some(""), Some("xterm"), Some("C.UTF-8")).color,
            "empty NO_COLOR is unset"
        );
        for s in [
            t(None, Some("dumb"), Some("C.UTF-8")),
            t(None, None, Some("C.UTF-8")),
        ] {
            assert!(!s.color && !s.unicode, "{s:?}");
        }
        let latin = t(None, Some("xterm"), Some("es_CO.ISO-8859-1"));
        assert!(latin.color && !latin.unicode);
        assert!(!t(None, Some("linux"), Some("C.UTF-8")).unicode);
    }

    #[test]
    fn wrap_breaks_at_spaces_and_cuts_long_words() {
        assert_eq!(wrap("aaa bbb ccc", 8), vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap("x\n\ny", 8), vec!["x", "", "y"]);
        assert_eq!(
            wrap(&"z".repeat(20), 8),
            vec!["zzzzzzzz", "zzzzzzzz", "zzzz"]
        );
    }

    #[test]
    fn clip_and_pad() {
        assert_eq!(clip(Style::PLAIN, "a  b\nc", 10), "a b c");
        assert_eq!(clip(Style::PLAIN, "abcdefghij", 6), "abc...");
        assert_eq!(pad("ab", 4), "ab  ");
    }
}
