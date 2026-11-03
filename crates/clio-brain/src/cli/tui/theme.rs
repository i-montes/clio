//! Colors and symbols. Only the 8 basic ANSI colors plus bold and dim, so the terminal's own
//! palette decides the real shades (it works on dark and light themes and without 24-bit color).
//! The default foreground and background are never set. One accent (cyan) marks the selection and
//! the focus; green, yellow and red are only for ok, warning, failure and the diff. Without color
//! (`NO_COLOR`, `TERM=dumb`) everything falls back to bold and dim; without Unicode every symbol
//! has an ASCII stand-in.

use ratatui::style::{Color, Modifier, Style};

use crate::cli::runs::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub color: bool,
    pub unicode: bool,
}

impl Theme {
    pub fn from_env() -> Self {
        let s = crate::cli::ui::Style::stdout();
        Self {
            color: s.color,
            unicode: s.unicode,
        }
    }

    fn fg(self, c: Color) -> Style {
        if self.color {
            Style::new().fg(c)
        } else {
            Style::new()
        }
    }

    fn pick(self, unicode: &'static str, ascii: &'static str) -> &'static str {
        if self.unicode { unicode } else { ascii }
    }

    pub fn plain(self) -> Style {
        Style::new()
    }
    pub fn bold(self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
    pub fn dim(self) -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }
    /// The one accent: selection and focus.
    pub fn accent(self) -> Style {
        self.fg(Color::Cyan).add_modifier(Modifier::BOLD)
    }
    pub fn ok(self) -> Style {
        self.fg(Color::Green)
    }
    pub fn warn(self) -> Style {
        self.fg(Color::Yellow).add_modifier(Modifier::BOLD)
    }
    pub fn fail(self) -> Style {
        self.fg(Color::Red).add_modifier(Modifier::BOLD)
    }
    /// The selected row: accent, and bold when there is no color.
    pub fn selected(self) -> Style {
        self.accent()
    }
    pub fn add(self) -> Style {
        self.fg(Color::Green)
    }
    pub fn del(self) -> Style {
        self.fg(Color::Red)
    }
    pub fn hunk(self) -> Style {
        self.dim()
    }
    /// A key name in the hints.
    pub fn key(self) -> Style {
        self.bold()
    }

    pub fn status(self, s: Status) -> Style {
        match s {
            Status::Applied => self.ok(),
            Status::Pending => self.warn(),
            Status::Failed | Status::Incomplete => self.fail(),
            Status::Undone | Status::NoChanges | Status::Cancelled => self.dim(),
        }
    }

    // ---- symbols
    pub fn dot(self) -> &'static str {
        self.pick("·", "-")
    }
    pub fn sel(self) -> &'static str {
        self.pick("▸", ">")
    }
    pub fn check(self) -> &'static str {
        self.pick("✓", "+")
    }
    pub fn bang(self) -> &'static str {
        "!"
    }
    pub fn cross(self) -> &'static str {
        self.pick("✗", "x")
    }
    pub fn up(self) -> &'static str {
        self.pick("↑", "^")
    }
    pub fn down(self) -> &'static str {
        self.pick("↓", "v")
    }
    pub fn rule(self) -> &'static str {
        self.pick("─", "-")
    }
    pub fn rule_v(self) -> &'static str {
        self.pick("│", "|")
    }
    pub fn ellipsis(self) -> &'static str {
        self.pick("…", "...")
    }
    pub fn arrow(self) -> &'static str {
        self.pick("→", "->")
    }
    pub fn cursor(self) -> &'static str {
        self.pick("▏", "|")
    }

    /// A bar of `width` cells with `percent` filled, between end marks.
    pub fn bar(self, percent: u64, width: usize) -> String {
        let filled = usize::try_from(percent.min(100) * width as u64 / 100).unwrap_or(width);
        let (l, r, on, off) = if self.unicode {
            ("▕", "▏", "█", "░")
        } else {
            ("[", "]", "#", ".")
        };
        format!("{l}{}{}{r}", on.repeat(filled), off.repeat(width - filled))
    }

    pub fn spinner(self, now_ms: u64) -> &'static str {
        const U: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        const A: [&str; 4] = ["|", "/", "-", "\\"];
        if self.unicode {
            U[(now_ms / 100 % 10) as usize]
        } else {
            A[(now_ms / 100 % 4) as usize]
        }
    }

    /// The bar's color by how full the context is: quiet until 70 %, then warning, then failure.
    pub fn level(self, percent: u64) -> Style {
        match percent {
            0..=69 => self.dim(),
            70..=89 => self.warn(),
            _ => self.fail(),
        }
    }
}
