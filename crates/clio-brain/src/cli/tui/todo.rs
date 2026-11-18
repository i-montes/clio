//! The model's checklist (`todo_write`): the guard keeps the list and shows it as Pi's widget
//! `clio-todo`, one line per item, `[x] `, `[>] ` or `[ ] ` and the text. The chat pins it above
//! the status line; `clio run` prints it when it changes. Only that widget key is read, and every
//! line is cleaned and capped: the text is the model's.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::theme::Theme;
use super::{sanitize, text};

/// The guard's widget key: any other widget is not drawn.
pub const KEY: &str = "clio-todo";
/// Rows the checklist takes at most (then `+N more`).
pub const ROWS: usize = 6;
const MAX_ITEMS: usize = 50;
const MAX_TEXT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    InProgress,
    Done,
}

pub type Item = (Status, String);

/// The widget's lines as items: an unknown line is dropped; text cleaned, capped.
pub fn parse(lines: &[String]) -> Vec<Item> {
    lines
        .iter()
        .filter_map(|l| {
            let (status, rest) = if let Some(r) = l.strip_prefix("[x] ") {
                (Status::Done, r)
            } else if let Some(r) = l.strip_prefix("[>] ") {
                (Status::InProgress, r)
            } else {
                (Status::Pending, l.strip_prefix("[ ] ")?)
            };
            let t: String = sanitize::line(rest).trim().chars().take(MAX_TEXT).collect();
            (!t.is_empty()).then_some((status, t))
        })
        .take(MAX_ITEMS)
        .collect()
}

/// The list as plain lines, for `clio run`.
pub fn plain(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|(s, t)| {
            let mark = match s {
                Status::Done => "[x]",
                Status::InProgress => "[>]",
                Status::Pending => "[ ]",
            };
            format!("  {mark} {t}")
        })
        .collect()
}

/// The rows the checklist needs: none for no list, at most `ROWS` (the last one `+N more`).
pub fn height(items: &[Item]) -> u16 {
    u16::try_from(items.len().min(ROWS)).unwrap_or(0)
}

/// The checklist, one item a row: done dim with a check, in progress in the accent color.
pub fn draw(f: &mut Frame, area: Rect, t: Theme, items: &[Item]) {
    let w = usize::from(area.width.saturating_sub(5));
    let over = items.len() > ROWS;
    let shown = if over { ROWS - 1 } else { items.len() };
    let mut rows: Vec<Line> = items[..shown]
        .iter()
        .map(|(s, txt)| {
            let (mark, style) = match s {
                Status::Done => (t.check(), t.dim()),
                Status::InProgress => (t.arrow(), t.accent()),
                Status::Pending => (t.bullet(), t.plain()),
            };
            Line::from(vec![
                Span::styled(format!("  {mark} "), style),
                Span::styled(text::fit(txt, w, t.ellipsis()), style),
            ])
        })
        .collect();
    if over {
        rows.push(Line::from(Span::styled(
            format!("    +{} more", items.len() - shown),
            t.dim(),
        )));
    }
    f.render_widget(Paragraph::new(rows), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_marked_lines_are_items_cleaned_and_capped() {
        let long = "y".repeat(500);
        let lines: Vec<String> = [
            "[x] read the config",
            "[>] fix\u{1b}[2J the \u{202e}parser",
            "[ ] run the tests",
            "no mark",
            "[ ]    ",
            &format!("[ ] {long}"),
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let items = parse(&lines);
        assert_eq!(items.len(), 4, "{items:?}");
        assert_eq!(items[0], (Status::Done, "read the config".into()));
        assert_eq!(items[1].0, Status::InProgress);
        assert!(!items[1].1.contains('\u{1b}') && !items[1].1.contains('\u{202e}'));
        assert_eq!(items[3].1.chars().count(), MAX_TEXT);
        assert_eq!(plain(&items[..1]), ["  [x] read the config"]);
        assert_eq!(height(&items), 4);
        assert_eq!(height(&parse(&vec!["[ ] a".to_owned(); 9])), 6);
    }
}
