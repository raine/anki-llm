use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::config::store::read_config;

// ---------------------------------------------------------------------------
// Glyph sets (Nerd Font vs plain fallback)
// ---------------------------------------------------------------------------

pub(super) struct Glyphs {
    pub(super) checkbox_checked: &'static str,
    pub(super) checkbox_unchecked: &'static str,
}

impl Glyphs {
    fn nerd() -> Self {
        Self {
            checkbox_checked: "\u{f4a7} ",
            checkbox_unchecked: "\u{f0131} ",
        }
    }

    fn plain() -> Self {
        Self {
            checkbox_checked: "[x] ",
            checkbox_unchecked: "[ ] ",
        }
    }

    pub(super) fn from_config() -> Self {
        let nerd_font = read_config().ok().and_then(|c| c.nerd_font).unwrap_or(true);
        if nerd_font {
            Self::nerd()
        } else {
            Self::plain()
        }
    }
}

// ---------------------------------------------------------------------------
// Theme palette (matches workmux default dark)
// ---------------------------------------------------------------------------

pub(super) struct Palette {
    pub(super) dimmed: Color,
    pub(super) text: Color,
    pub(super) border: Color,
    pub(super) info: Color,
    pub(super) success: Color,
    pub(super) warning: Color,
    pub(super) danger: Color,
    pub(super) highlight_bg: Color,
    pub(super) highlight_fg: Color,
    pub(super) help_border: Color,
    pub(super) help_muted: Color,
    pub(super) header: Color,
}

pub(super) const THEME: Palette = Palette {
    dimmed: Color::Rgb(108, 112, 134),
    text: Color::Rgb(205, 214, 244),
    border: Color::Rgb(58, 74, 94),
    info: Color::Rgb(120, 225, 213),
    success: Color::Rgb(166, 218, 149),
    warning: Color::Rgb(249, 226, 175),
    danger: Color::Rgb(237, 135, 150),
    highlight_bg: Color::Rgb(40, 48, 62),
    highlight_fg: Color::Rgb(244, 248, 255),
    help_border: Color::Rgb(81, 104, 130),
    help_muted: Color::Rgb(112, 126, 144),
    header: Color::Rgb(180, 200, 220),
};

pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Build a footer span pair: key in dimmed, label in bold text.
pub(super) fn footer_cmd(key: &str, label: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(key.to_string(), Style::default().fg(THEME.dimmed)),
        Span::styled(
            format!(" {label}"),
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
    ]
}

pub(super) fn footer_pipe() -> Span<'static> {
    Span::styled(" \u{2502} ", Style::default().fg(THEME.border))
}
