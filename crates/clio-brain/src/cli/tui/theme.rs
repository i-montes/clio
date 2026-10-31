//! Colors. Only the 8 basic ANSI colors plus bold, dim and reverse, so the terminal's own palette
//! decides the real shades (it works on dark and light themes and without 24-bit color). The
//! default foreground and background are never set: text stays readable whatever the theme is,
//! and selection uses reverse video, which is legible on any palette. Without color (`NO_COLOR`,
//! `TERM=dumb`) everything falls back to bold, dim and reverse.

use ratatui::style::{Color, Modifier, Style};

use crate::cli::runs::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub color: bool,
}

impl Theme {
    pub fn from_env() -> Self {
        Self {
            color: crate::cli::ui::Style::stdout().color,
        }
    }

    fn fg(self, c: Color) -> Style {
        if self.color {
            Style::new().fg(c)
        } else {
            Style::new()
        }
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
    /// The selected row.
    pub fn selected(self) -> Style {
        Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
    pub fn add(self) -> Style {
        self.fg(Color::Green)
    }
    pub fn del(self) -> Style {
        self.fg(Color::Red)
    }
    pub fn hunk(self) -> Style {
        self.fg(Color::Cyan)
    }
    pub fn key(self) -> Style {
        self.fg(Color::Yellow).add_modifier(Modifier::BOLD)
    }

    pub fn status(self, s: Status) -> Style {
        match s {
            Status::Applied => self.ok(),
            Status::Pending => self.warn(),
            Status::Failed | Status::Incomplete => self.fail(),
            Status::Undone | Status::NoChanges | Status::Cancelled => self.dim(),
        }
    }
}
