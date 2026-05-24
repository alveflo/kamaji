use ratatui::style::Color;

use crate::models::Status;

/// The out-of-box default theme; also the fallback for unknown names.
const DEFAULT_THEME: &str = "catppuccin";

/// A complete colorscheme as a set of semantic roles. Every color the UI draws
/// comes from one of these fields, so swapping a `Theme` re-skins the whole app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Stable key persisted in config (e.g. "catppuccin").
    pub name: &'static str,
    /// Human label shown in the picker (e.g. "Catppuccin Mocha").
    pub label: &'static str,
    /// App background; `None` means "use the terminal's own background".
    pub base: Option<Color>,
    /// Selected-card fill and subtle panels.
    pub surface: Color,
    /// Primary text.
    pub text: Color,
    /// Dim text: idle bullets, rules, empty columns, hints.
    pub muted: Color,
    /// Idle card + modal borders.
    pub border: Color,
    /// Column header + card accent for the Todo column.
    pub todo: Color,
    /// Column header + card accent for the In Progress column.
    pub in_progress: Color,
    /// Column header + card accent for the Needs attention (Review) column.
    pub review: Color,
    /// Column header + card accent for the Done column.
    pub done: Color,
    /// Green "agent actively working" bullet.
    pub active: Color,
    /// Needs-attention pulse color.
    pub attention: Color,
    /// Error / toast text.
    pub error: Color,
}

impl Theme {
    /// Per-column accent color.
    pub fn status_color(&self, status: Status) -> Color {
        match status {
            Status::Todo => self.todo,
            Status::InProgress => self.in_progress,
            Status::Review => self.review,
            Status::Done => self.done,
        }
    }

    /// Generic accent (selection, active form field, modal title). Reuses the
    /// in-progress hue to keep the role set small.
    pub fn accent(&self) -> Color {
        self.in_progress
    }

    /// All built-ins in picker display order. The default (terminal) theme is
    /// first; Catppuccin is the out-of-box default (index 1).
    pub const ALL: &'static [fn() -> Theme] =
        &[default_ansi, catppuccin, tokyo_night, gruvbox, nord];

    /// Index of `name` in `ALL`, falling back to the default theme
    /// (Catppuccin) — by name, not by position — if `name` is unknown.
    pub fn index_of(name: &str) -> usize {
        let find = |target: &str| Theme::ALL.iter().position(|f| f().name == target);
        find(name).or_else(|| find(DEFAULT_THEME)).unwrap_or(0)
    }

    /// Look up a theme by name; unknown names fall back to Catppuccin.
    pub fn by_name(name: &str) -> Theme {
        Theme::ALL[Theme::index_of(name)]()
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Terminal-native: 16 ANSI colors and no forced background.
pub fn default_ansi() -> Theme {
    Theme {
        name: "default",
        label: "Default (terminal)",
        base: None,
        // Limited 16-color palette: surface/muted/border share DarkGray.
        surface: Color::DarkGray,
        text: Color::Gray,
        muted: Color::DarkGray,
        border: Color::DarkGray,
        todo: Color::Gray,
        in_progress: Color::Cyan,
        review: Color::Yellow,
        done: Color::Green,
        active: Color::Green,
        attention: Color::Yellow,
        error: Color::Red,
    }
}

pub fn catppuccin() -> Theme {
    Theme {
        name: "catppuccin",
        label: "Catppuccin Mocha",
        base: Some(rgb(0x1e, 0x1e, 0x2e)),
        surface: rgb(0x31, 0x32, 0x44),
        text: rgb(0xcd, 0xd6, 0xf4),
        muted: rgb(0x6c, 0x70, 0x86),
        border: rgb(0x45, 0x47, 0x5a),
        todo: rgb(0x93, 0x99, 0xb2),
        in_progress: rgb(0x89, 0xb4, 0xfa),
        review: rgb(0xfa, 0xb3, 0x87),
        done: rgb(0xa6, 0xe3, 0xa1),
        active: rgb(0xa6, 0xe3, 0xa1),
        attention: rgb(0xfa, 0xb3, 0x87),
        error: rgb(0xf3, 0x8b, 0xa8),
    }
}

// Config key "tokyonight" omits the underscore in the fn name.
pub fn tokyo_night() -> Theme {
    Theme {
        name: "tokyonight",
        label: "Tokyo Night",
        base: Some(rgb(0x1a, 0x1b, 0x26)),
        surface: rgb(0x29, 0x2e, 0x42),
        text: rgb(0xc0, 0xca, 0xf5),
        muted: rgb(0x56, 0x5f, 0x89),
        border: rgb(0x41, 0x48, 0x68),
        todo: rgb(0x56, 0x5f, 0x89),
        in_progress: rgb(0x7a, 0xa2, 0xf7),
        review: rgb(0xff, 0x9e, 0x64),
        done: rgb(0x9e, 0xce, 0x6a),
        active: rgb(0x9e, 0xce, 0x6a),
        attention: rgb(0xff, 0x9e, 0x64),
        error: rgb(0xf7, 0x76, 0x8e),
    }
}

pub fn gruvbox() -> Theme {
    Theme {
        name: "gruvbox",
        label: "Gruvbox Dark",
        base: Some(rgb(0x28, 0x28, 0x28)),
        surface: rgb(0x3c, 0x38, 0x36),
        text: rgb(0xeb, 0xdb, 0xb2),
        muted: rgb(0x92, 0x83, 0x74),
        border: rgb(0x50, 0x49, 0x45),
        todo: rgb(0xa8, 0x99, 0x84),
        in_progress: rgb(0x83, 0xa5, 0x98),
        review: rgb(0xfe, 0x80, 0x19),
        done: rgb(0xb8, 0xbb, 0x26),
        active: rgb(0xb8, 0xbb, 0x26),
        attention: rgb(0xfe, 0x80, 0x19),
        error: rgb(0xfb, 0x49, 0x34),
    }
}

pub fn nord() -> Theme {
    Theme {
        name: "nord",
        label: "Nord",
        base: Some(rgb(0x2e, 0x34, 0x40)),
        surface: rgb(0x3b, 0x42, 0x52),
        text: rgb(0xd8, 0xde, 0xe9),
        muted: rgb(0x61, 0x6e, 0x88),
        border: rgb(0x43, 0x4c, 0x5e),
        todo: rgb(0x61, 0x6e, 0x88),
        in_progress: rgb(0x88, 0xc0, 0xd0),
        review: rgb(0xd0, 0x87, 0x70),
        done: rgb(0xa3, 0xbe, 0x8c),
        active: rgb(0xa3, 0xbe, 0x8c),
        attention: rgb(0xd0, 0x87, 0x70),
        error: rgb(0xbf, 0x61, 0x6a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_matches_known_and_falls_back_to_catppuccin() {
        assert_eq!(Theme::by_name("nord").name, "nord");
        assert_eq!(Theme::by_name("catppuccin").name, "catppuccin");
        // Unknown names fall back to the out-of-box default, Catppuccin.
        assert_eq!(Theme::by_name("nope").name, "catppuccin");
    }

    #[test]
    fn index_of_roundtrips_with_all() {
        for (i, f) in Theme::ALL.iter().enumerate() {
            assert_eq!(Theme::index_of(f().name), i);
        }
        // Unknown -> Catppuccin's index (1).
        assert_eq!(Theme::index_of("nope"), 1);
        assert_eq!(Theme::ALL[1]().name, "catppuccin");
    }

    #[test]
    fn default_theme_has_no_forced_background() {
        assert!(default_ansi().base.is_none());
        // Named themes paint a background.
        assert!(catppuccin().base.is_some());
        assert!(nord().base.is_some());
    }

    #[test]
    fn status_color_maps_each_column() {
        let t = catppuccin();
        assert_eq!(t.status_color(Status::Todo), t.todo);
        assert_eq!(t.status_color(Status::InProgress), t.in_progress);
        assert_eq!(t.status_color(Status::Review), t.review);
        assert_eq!(t.status_color(Status::Done), t.done);
    }
}
