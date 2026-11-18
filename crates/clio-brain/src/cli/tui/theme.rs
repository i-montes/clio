//! Colors and symbols, in three tiers. Without color (`NO_COLOR`, `TERM=dumb`) only bold and dim.
//! With color, the 8 basic ANSI colors, so the terminal's own palette decides the real shades: one
//! accent (cyan) marks the selection and the focus; green, yellow and red are only for ok, warning,
//! failure and the diff. When the terminal says it has 24-bit color (`COLORTERM=truecolor`), a
//! fixed palette of mid-tones that read on dark and on light backgrounds, and gray text instead
//! of the DIM modifier (which terminals render unevenly). The default foreground and background
//! are never set. Without Unicode every symbol has an ASCII stand-in.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;

use crate::cli::runs::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Theme {
    pub color: bool,
    pub unicode: bool,
    /// 24-bit color (only with `color`): the fixed palette below instead of the ANSI names.
    pub rgb: bool,
}

/// The 24-bit palette.
const ACCENT: Color = Color::Rgb(0x8B, 0x7C, 0xF6);
const OK: Color = Color::Rgb(0x3F, 0xB9, 0x50);
const WARN: Color = Color::Rgb(0xD2, 0x99, 0x22);
const FAIL: Color = Color::Rgb(0xF8, 0x51, 0x49);
const CODE: Color = Color::Rgb(0x56, 0xB6, 0xC2);
const MUTED: Color = Color::Rgb(0x8B, 0x94, 0x9E);
const FAINT: Color = Color::Rgb(0x48, 0x4F, 0x58);
const ON_ACCENT: Color = Color::Rgb(0x11, 0x11, 0x1B);

impl Theme {
    pub fn from_env() -> Self {
        let s = crate::cli::ui::Style::stdout();
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        Self::decide(s.color, s.unicode, &colorterm)
    }

    /// The tier as a pure function of what the environment says.
    pub fn decide(color: bool, unicode: bool, colorterm: &str) -> Self {
        Self {
            color,
            unicode,
            rgb: color && matches!(colorterm, "truecolor" | "24bit"),
        }
    }

    /// `ansi` in the ANSI tier, `rgb` in the 24-bit one, nothing without color.
    fn fg2(self, ansi: Color, rgb: Color) -> Style {
        match (self.color, self.rgb) {
            (false, _) => Style::new(),
            (true, false) => Style::new().fg(ansi),
            (true, true) => Style::new().fg(rgb),
        }
    }

    fn fg(self, c: Color) -> Style {
        let rgb = match c {
            Color::Cyan => ACCENT,
            Color::Green => OK,
            Color::Yellow => WARN,
            Color::Red => FAIL,
            other => other,
        };
        self.fg2(c, rgb)
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
    /// Secondary text: gray in the 24-bit tier, the DIM modifier otherwise.
    pub fn dim(self) -> Style {
        if self.color && self.rgb {
            Style::new().fg(MUTED)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }
    /// Rules, unfocused borders, the scrollbar's track: quieter than muted text.
    pub fn faint(self) -> Style {
        if self.color && self.rgb {
            Style::new().fg(FAINT)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }
    /// Code, inline and in blocks.
    pub fn code(self) -> Style {
        self.fg2(Color::Magenta, CODE)
    }
    /// A heading in the agent's text.
    pub fn heading(self) -> Style {
        self.accent()
    }
    /// The accent without bold: the person's bar, a focused border.
    pub fn accent_line(self) -> Style {
        self.fg(Color::Cyan)
    }
    /// A selected row drawn across its width: the accent behind dark bold text.
    pub fn selection(self) -> Style {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        match (self.color, self.rgb) {
            (false, _) => bold.add_modifier(Modifier::REVERSED),
            (true, false) => bold.bg(Color::Cyan).fg(Color::Black),
            (true, true) => bold.bg(ACCENT).fg(ON_ACCENT),
        }
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
    /// A tool call set aside for the person's approval.
    pub fn aside(self) -> &'static str {
        self.pick("◇", "?")
    }
    /// A tool that was still running when its turn ended.
    pub fn stopped(self) -> &'static str {
        self.pick("■", "#")
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
    /// Where a table's rule meets a column bar.
    pub fn rule_cross(self) -> &'static str {
        self.pick("┼", "+")
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
    /// The bar beside the person's message.
    pub fn bar_left(self) -> &'static str {
        self.pick("▌", "|")
    }
    pub fn bullet(self) -> &'static str {
        self.pick("•", "*")
    }
    /// The composer's prompt.
    pub fn prompt(self) -> &'static str {
        self.pick("›", ">")
    }
    /// The session open in the chat.
    pub fn open_mark(self) -> &'static str {
        self.pick("●", "*")
    }
    /// The borders of the boxes (the composer, the drawer, the dialogs): rounded, or ASCII.
    pub fn border(self) -> border::Set<'static> {
        if self.unicode {
            border::ROUNDED
        } else {
            border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_tiers_no_color_ansi_and_24_bit() {
        let none = Theme::decide(false, true, "truecolor");
        assert!(!none.rgb, "no color wins over COLORTERM");
        for s in [
            none.accent(),
            none.dim(),
            none.faint(),
            none.code(),
            none.selection(),
        ] {
            assert!(s.fg.is_none() && s.bg.is_none(), "{s:?}");
        }
        assert!(none.selection().add_modifier.contains(Modifier::REVERSED));

        let ansi = Theme::decide(true, true, "");
        assert!(!ansi.rgb);
        assert_eq!(ansi.accent().fg, Some(Color::Cyan));
        assert!(ansi.dim().add_modifier.contains(Modifier::DIM));
        assert_eq!(ansi.selection().bg, Some(Color::Cyan));

        for ct in ["truecolor", "24bit"] {
            let rgb = Theme::decide(true, true, ct);
            assert!(rgb.rgb);
            assert_eq!(rgb.accent().fg, Some(ACCENT));
            assert_eq!(rgb.ok().fg, Some(OK));
            assert_eq!(rgb.warn().fg, Some(WARN));
            assert_eq!(rgb.fail().fg, Some(FAIL));
            assert_eq!(rgb.code().fg, Some(CODE));
            assert_eq!(rgb.faint().fg, Some(FAINT));
            // Gray text, not the DIM modifier; never a background but the selection's.
            assert_eq!(rgb.dim().fg, Some(MUTED));
            assert!(!rgb.dim().add_modifier.contains(Modifier::DIM));
            assert_eq!(
                (rgb.selection().bg, rgb.selection().fg),
                (Some(ACCENT), Some(ON_ACCENT))
            );
            for s in [rgb.plain(), rgb.bold(), rgb.dim(), rgb.accent(), rgb.code()] {
                assert!(s.bg.is_none(), "{s:?}");
            }
        }
    }

    #[test]
    fn every_new_symbol_has_an_ascii_stand_in() {
        let t = Theme::decide(false, false, "");
        for sym in [
            t.bar_left(),
            t.bullet(),
            t.prompt(),
            t.open_mark(),
            t.stopped(),
        ] {
            assert!(sym.is_ascii(), "{sym:?}");
        }
        let b = t.border();
        for sym in [b.top_left, b.top_right, b.horizontal_top, b.vertical_left] {
            assert!(sym.is_ascii(), "{sym:?}");
        }
    }
}
