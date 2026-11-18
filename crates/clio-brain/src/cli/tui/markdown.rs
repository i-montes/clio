//! The agent's text drawn as markdown, wrapped to the column by display width: paragraphs, bold
//! and italic, inline code, headings, lists (nested, numbered), quotes, code blocks, links, rules
//! and tables as plain rows.
//!
//! The text is the model's, so it is not trusted: every piece that reaches a line (text, code,
//! a link's address, raw HTML) goes through `sanitize` after the parser, because the parser itself
//! turns `&#27;` into an escape byte. Nothing here builds an escape sequence: a link is its text
//! underlined and its address after it, never an OSC 8 hyperlink.

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::sanitize;
use super::text::{char_w, width};
use super::theme::Theme;

/// `src` as lines at most `w` columns wide. `live`: the text is still streaming, so a `**` or a
/// backtick still open in its last paragraph is closed for the drawing (the style does not flip
/// on and off as the rest arrives).
pub fn render(src: &str, w: usize, t: Theme, live: bool) -> Vec<Line<'static>> {
    // Clean before parsing too, so a whole escape sequence goes as one, wherever it falls.
    let mut src = sanitize::multiline(src);
    if live {
        src = close_open_markers(&src);
    }
    let mut r = Render {
        t,
        w: w.max(8),
        out: Vec::new(),
        boxes: Vec::new(),
        styles: vec![Style::new()],
        inline: Vec::new(),
        blank: false,
        lists: Vec::new(),
        code: None,
        links: Vec::new(),
        row: Vec::new(),
        head: false,
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for ev in Parser::new_ext(&src, opts) {
        r.event(ev);
    }
    r.flush();
    r.out
}

/// Closes an inline marker left open at the end of a streaming text: an odd number of backticks
/// or of `**` in the last paragraph. Not inside an open code fence, where they are text.
fn close_open_markers(src: &str) -> String {
    let fences = src
        .lines()
        .filter(|l| {
            let l = l.trim_start();
            l.starts_with("```") || l.starts_with("~~~")
        })
        .count();
    let mut out = src.to_owned();
    if fences % 2 == 1 {
        return out;
    }
    let last = src.rsplit("\n\n").next().unwrap_or(src);
    if last.matches('`').count() % 2 == 1 {
        out.push('`');
    }
    if last.matches("**").count() % 2 == 1 {
        out.push_str("**");
    }
    out
}

/// A container the lines of a block sit in.
enum Boxed {
    Quote,
    /// A list item: its bullet (`• `, `3. `), and whether its first line was drawn.
    Item {
        bullet: String,
        used: bool,
    },
}

struct Render {
    t: Theme,
    w: usize,
    out: Vec<Line<'static>>,
    boxes: Vec<Boxed>,
    styles: Vec<Style>,
    /// The words of the block being read.
    inline: Vec<(String, Style)>,
    /// A blank line is owed before the next block.
    blank: bool,
    /// The open lists: the next number, or none for bullets.
    lists: Vec<Option<u64>>,
    /// An open code block: its language and its text.
    code: Option<(String, String)>,
    links: Vec<(LinkType, String)>,
    /// The cells of a table row.
    row: Vec<Vec<(String, Style)>>,
    head: bool,
}

impl Render {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, s: Style) {
        let next = self.style().patch(s);
        self.styles.push(next);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn text(&mut self, s: &str) {
        if let Some((_, body)) = &mut self.code {
            body.push_str(s);
            return;
        }
        let style = self.style();
        self.inline.push((sanitize::line(s), style));
    }

    /// What goes in front of a line: the quote bars and the list indents; the bullets only on the
    /// first line of an item (`first`).
    fn prefix(&self, first: bool) -> (Vec<Span<'static>>, usize) {
        let t = self.t;
        let mut spans = Vec::new();
        let mut w = 0;
        for b in &self.boxes {
            match b {
                Boxed::Quote => {
                    spans.push(Span::styled(format!("{} ", t.rule_v()), t.faint()));
                    w += 2;
                }
                Boxed::Item { bullet, used } => {
                    let bw = width(bullet);
                    if first && !used {
                        spans.push(Span::styled(bullet.clone(), t.accent_line()));
                    } else {
                        spans.push(Span::raw(" ".repeat(bw)));
                    }
                    w += bw;
                }
            }
        }
        (spans, w)
    }

    fn mark_used(&mut self) {
        for b in &mut self.boxes {
            if let Boxed::Item { used, .. } = b {
                *used = true;
            }
        }
    }

    /// The owed blank line, inside the quotes it belongs to.
    fn open_block(&mut self) {
        if self.blank && !self.out.is_empty() {
            let bars = self
                .boxes
                .iter()
                .filter(|b| matches!(b, Boxed::Quote))
                .map(|_| Span::styled(self.t.rule_v().to_owned(), self.t.faint()));
            self.out.push(Line::from(bars.collect::<Vec<_>>()));
        }
        self.blank = false;
    }

    /// The words read so far, as wrapped lines.
    fn flush(&mut self) {
        if self.inline.iter().all(|(s, _)| s.trim().is_empty()) {
            self.inline.clear();
            return;
        }
        self.open_block();
        let first = self.prefix(true);
        let rest = self.prefix(false);
        let segs = std::mem::take(&mut self.inline);
        self.out.extend(wrap_spans(&segs, self.w, first, rest));
        self.mark_used();
    }

    fn event(&mut self, ev: Event) {
        let t = self.t;
        match ev {
            Event::Text(s) | Event::Html(s) | Event::InlineHtml(s) => self.text(&s),
            Event::Code(s) => {
                let style = self.style().patch(t.code());
                self.inline.push((sanitize::line(&s), style));
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => {
                let style = self.style();
                self.inline.push(("\n".into(), style));
            }
            Event::Rule => {
                self.flush();
                self.open_block();
                let (mut p, pw) = self.prefix(false);
                p.push(Span::styled(
                    t.rule().repeat(self.w.saturating_sub(pw)),
                    t.faint(),
                ));
                self.out.push(Line::from(p));
                self.blank = true;
            }
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        let t = self.t;
        match tag {
            Tag::Paragraph | Tag::HtmlBlock | Tag::TableCell => {}
            Tag::Heading { .. } => {
                self.flush();
                self.push_style(t.heading());
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.open_block();
                self.boxes.push(Boxed::Quote);
                self.push_style(t.dim());
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        sanitize::line(l.split_whitespace().next().unwrap_or(""))
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(first) => {
                self.flush();
                self.lists.push(first);
            }
            Tag::Item => {
                self.flush();
                let bullet = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let b = format!("{n}. ");
                        *n += 1;
                        b
                    }
                    _ => format!("{} ", t.bullet()),
                };
                self.boxes.push(Boxed::Item {
                    bullet,
                    used: false,
                });
            }
            Tag::Emphasis => self.push_style(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(Style::new().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link {
                link_type,
                dest_url,
                ..
            }
            | Tag::Image {
                link_type,
                dest_url,
                ..
            } => {
                self.links.push((link_type, dest_url.to_string()));
                self.push_style(Style::new().add_modifier(Modifier::UNDERLINED));
            }
            Tag::Table(_) => {
                self.flush();
                self.open_block();
            }
            Tag::TableHead => {
                self.head = true;
                self.push_style(Style::new().add_modifier(Modifier::BOLD));
            }
            Tag::TableRow => self.row.clear(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        let t = self.t;
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => {
                self.flush();
                self.blank = true;
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.pop_style();
                self.blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.boxes.pop();
                self.pop_style();
                self.blank = true;
            }
            TagEnd::CodeBlock => self.code_block(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank = true;
                }
            }
            TagEnd::Item => {
                self.flush();
                self.boxes.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link | TagEnd::Image => {
                self.pop_style();
                if let Some((kind, url)) = self.links.pop()
                    && !url.is_empty()
                    && !matches!(kind, LinkType::Autolink | LinkType::Email)
                {
                    self.inline
                        .push((format!(" ({})", sanitize::line(&url)), t.dim()));
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                self.row.push(cell);
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if self.head {
                    self.pop_style();
                    self.head = false;
                }
                let mut segs = Vec::new();
                for (i, cell) in std::mem::take(&mut self.row).into_iter().enumerate() {
                    if i > 0 {
                        segs.push((format!(" {} ", t.rule_v()), t.faint()));
                    }
                    segs.extend(cell);
                }
                let first = self.prefix(true);
                let rest = self.prefix(false);
                self.out.extend(wrap_spans(&segs, self.w, first, rest));
            }
            TagEnd::Table => self.blank = true,
            _ => {}
        }
    }

    /// A code block: a dim bar on the left, the language on top, every line whole (cut into rows,
    /// never cut off).
    fn code_block(&mut self) {
        let t = self.t;
        let Some((lang, body)) = self.code.take() else {
            return;
        };
        self.open_block();
        let (p, pw) = self.prefix(false);
        let bar = Span::styled(format!("{} ", t.rule_v()), t.faint());
        let room = self.w.saturating_sub(pw + 2).max(4);
        if !lang.is_empty() {
            let mut l = p.clone();
            l.extend([bar.clone(), Span::styled(lang, t.dim())]);
            self.out.push(Line::from(l));
        }
        let body = sanitize::multiline(&body);
        for line in body.strip_suffix('\n').unwrap_or(&body).split('\n') {
            for row in hard_wrap(line, room) {
                let mut l = p.clone();
                l.extend([bar.clone(), Span::styled(row, t.code())]);
                self.out.push(Line::from(l));
            }
        }
        self.mark_used();
        self.blank = true;
    }
}

/// `line` in rows of at most `w` columns, spaces kept: code is cut, not reflowed.
fn hard_wrap(line: &str, w: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let (mut used, mut prev) = (0, None);
    for c in line.chars() {
        let cw = char_w(prev, c);
        prev = Some(c);
        if used + cw > w && used > 0 {
            rows.push(String::new());
            used = 0;
        }
        used += cw;
        if let Some(r) = rows.last_mut() {
            r.push(c);
        }
    }
    rows
}

/// Adds `s` to a run of styled pieces, joining it to the last one when the style is the same.
fn push_piece(v: &mut Vec<(String, Style)>, s: &str, style: Style) {
    match v.last_mut() {
        Some((text, st)) if *st == style => text.push_str(s),
        _ => v.push((s.to_owned(), style)),
    }
}

/// Styled pieces wrapped at spaces to `w` columns, the first line after `first` and the others
/// after `rest`. A word wider than the row is cut. A `"\n"` piece breaks the line.
pub fn wrap_spans(
    segs: &[(String, Style)],
    w: usize,
    first: (Vec<Span<'static>>, usize),
    rest: (Vec<Span<'static>>, usize),
) -> Vec<Line<'static>> {
    let mut wr = Wrapper {
        out: Vec::new(),
        w,
        first,
        rest,
        cur: Vec::new(),
        cur_w: 0,
        word: Vec::new(),
        word_w: 0,
        space: None,
        prev: None,
    };
    for (text, style) in segs {
        if text == "\n" {
            wr.word_end();
            wr.line_end();
            continue;
        }
        let mut buf = [0u8; 4];
        for c in text.chars() {
            if c == ' ' {
                wr.word_end();
                if wr.cur_w > 0 {
                    wr.space = Some(*style);
                }
            } else {
                wr.word_w += char_w(wr.prev, c);
                push_piece(&mut wr.word, c.encode_utf8(&mut buf), *style);
            }
            wr.prev = Some(c);
        }
    }
    wr.word_end();
    if !wr.cur.is_empty() || wr.out.is_empty() {
        wr.line_end();
    }
    wr.out
}

struct Wrapper {
    out: Vec<Line<'static>>,
    w: usize,
    first: (Vec<Span<'static>>, usize),
    rest: (Vec<Span<'static>>, usize),
    cur: Vec<(String, Style)>,
    cur_w: usize,
    word: Vec<(String, Style)>,
    word_w: usize,
    space: Option<Style>,
    prev: Option<char>,
}

impl Wrapper {
    fn room(&self) -> usize {
        let p = if self.out.is_empty() {
            self.first.1
        } else {
            self.rest.1
        };
        self.w.saturating_sub(p).max(1)
    }

    fn line_end(&mut self) {
        let mut spans = if self.out.is_empty() {
            self.first.0.clone()
        } else {
            self.rest.0.clone()
        };
        spans.extend(
            std::mem::take(&mut self.cur)
                .into_iter()
                .map(|(s, st)| Span::styled(s, st)),
        );
        self.out.push(Line::from(spans));
        self.cur_w = 0;
        self.space = None;
    }

    fn word_end(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let word = std::mem::take(&mut self.word);
        let ww = std::mem::take(&mut self.word_w);
        let mut gap = usize::from(self.space.is_some() && self.cur_w > 0);
        if self.cur_w + gap + ww > self.room() && self.cur_w > 0 {
            self.line_end();
            gap = 0;
        }
        if let Some(st) = self.space.take()
            && gap == 1
        {
            push_piece(&mut self.cur, " ", st);
            self.cur_w += 1;
        }
        if ww <= self.room() {
            for (s, st) in &word {
                push_piece(&mut self.cur, s, *st);
            }
            self.cur_w += ww;
            return;
        }
        // Wider than a whole row: cut it, character by character.
        let mut buf = [0u8; 4];
        let mut prev = None;
        for (s, st) in &word {
            for c in s.chars() {
                let cw = char_w(prev, c);
                prev = Some(c);
                if self.cur_w + cw > self.room() && self.cur_w > 0 {
                    self.line_end();
                }
                push_piece(&mut self.cur, c.encode_utf8(&mut buf), *st);
                self.cur_w += cw;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const T: Theme = Theme {
        color: true,
        unicode: true,
        rgb: false,
    };

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn style_of<'a>(lines: &'a [Line<'static>], text: &str) -> &'a Span<'static> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains(text))
            .unwrap_or_else(|| panic!("{text:?} not in {:?}", plain(lines)))
    }

    #[test]
    fn paragraphs_bold_italic_and_inline_code() {
        let l = render(
            "Hola **mundo** y *tú*.\n\nUsa `cargo test` ya.",
            60,
            T,
            false,
        );
        assert_eq!(plain(&l), ["Hola mundo y tú.", "", "Usa cargo test ya."]);
        assert!(
            style_of(&l, "mundo")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            style_of(&l, "tú")
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert_eq!(style_of(&l, "cargo test").style.fg, T.code().fg);
    }

    #[test]
    fn headings_lose_the_hash_and_take_the_accent() {
        let l = render("# Título\n\ntexto", 40, T, false);
        assert_eq!(plain(&l), ["Título", "", "texto"]);
        assert_eq!(style_of(&l, "Título").style.fg, T.accent().fg);
    }

    #[test]
    fn lists_have_bullets_numbers_hanging_indent_and_nesting() {
        let l = render(
            "- uno dos tres cuatro cinco seis\n- dos\n  - hondo\n\n1. a\n2. b",
            20,
            T,
            false,
        );
        assert_eq!(
            plain(&l),
            [
                "• uno dos tres",
                "  cuatro cinco seis",
                "• dos",
                "  • hondo",
                "",
                "1. a",
                "2. b"
            ]
        );
    }

    #[test]
    fn quotes_code_blocks_links_rules_and_tables() {
        let src = "> citado\n\n```rust\nfn main() { let x = 1234567890; }\n```\n\n[web](https://e.com) y <https://a.b>\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |";
        let l = render(src, 24, T, false);
        let p = plain(&l);
        assert_eq!(p[0], "│ citado");
        assert!(p.contains(&"│ rust".to_owned()), "{p:?}");
        // The long code line is cut into rows, every character kept.
        let code: String = p
            .iter()
            .filter(|r| r.starts_with("│ ") && !r.contains("rust") && !r.contains("citado"))
            .map(|r| &r[4..])
            .collect();
        assert_eq!(code, "fn main() { let x = 1234567890; }");
        assert!(p.iter().all(|r| width(r) <= 24), "{p:?}");
        assert!(p.iter().any(|r| r == "web (https://e.com) y"), "{p:?}");
        assert!(p.iter().any(|r| r == "https://a.b"), "{p:?}");
        assert!(
            style_of(&l, "web")
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(p.iter().any(|r| r == &"─".repeat(24)), "{p:?}");
        assert!(
            p.iter().any(|r| r == "a │ b") && p.iter().any(|r| r == "1 │ 2"),
            "{p:?}"
        );
    }

    #[test]
    fn wrapping_goes_by_display_width_and_keeps_the_indent() {
        let l = render("- 日本語の文章です とても長い文章", 12, T, false);
        let p = plain(&l);
        assert!(p.iter().all(|r| width(r) <= 12), "{p:?}");
        assert!(p[1..].iter().all(|r| r.starts_with("  ")), "{p:?}");
    }

    // A text still streaming: an open `**` or fence never shows its markers, and each new piece
    // only adds to what was drawn.
    #[test]
    fn streaming_with_open_markers_is_stable() {
        let l = render("Esto es **impor", 40, T, true);
        assert_eq!(plain(&l), ["Esto es impor"]);
        assert!(
            style_of(&l, "impor")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        let l = render("Mira `ls -l", 40, T, true);
        assert_eq!(plain(&l), ["Mira ls -l"]);
        let l = render("Así:\n\n```sh\necho **hola", 40, T, true);
        assert_eq!(plain(&l), ["Así:", "", "│ sh", "│ echo **hola"]);
        // Every prefix of a text renders without panicking.
        let full = "# T\n\n- a **b** `c`\n  1. d\n\n> e\n\n```\nf\n```\n[g](h) | x |";
        for (i, _) in full.char_indices() {
            render(&full[..i], 30, T, true);
        }
    }

    // The model's text can carry escape sequences, in a link, in code, in a heading, or spelled
    // as an HTML entity that the parser turns into the escape byte: none reaches a line.
    #[test]
    fn escape_bytes_in_markdown_come_out_clean() {
        let src = "# ti\u{1b}]0;pwn\u{7}tulo\n\n[cl\u{1b}[2Jic](http://e\u{1b}]8;;x\u{7}.com) `co\u{1b}[31mde` &#27;[2J &#x9b;31m\n\n```\nx\u{1b}[?1049h\r\n```";
        let l = render(src, 60, T, false);
        for row in plain(&l) {
            assert!(
                !row.chars().any(|c| c.is_control() || c == '\u{9b}'),
                "{row:?}"
            );
        }
    }
}
