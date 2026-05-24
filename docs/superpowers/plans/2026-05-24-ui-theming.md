# UI restyle + colorscheme themes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle the kamaji Kanban board into a polished "refined terminal" look (rounded cards, colored accent strips, lighter column headers) and add a library of switchable colorschemes (Catppuccin, Tokyo Night, Gruvbox, Nord, and a terminal-16-color Default), selectable via a config key and an in-app live-preview picker.

**Architecture:** A new pure `theme.rs` module defines a `Theme` struct of semantic color roles plus built-in palettes and name lookup. The active `Theme` lives on `App`, set at startup from a new `Config.theme` key. All rendering code (`board.rs`, `modals.rs`) sources every color from the active theme instead of hard-coding `Color::*`. A new `Modal::ThemePicker` plus a `t` key let the user preview themes live and persist the choice back to `config.toml`.

**Tech Stack:** Rust, ratatui 0.29 (`BorderType::Rounded`, `TestBackend` for buffer assertions), serde/toml (config), rusqlite (unchanged).

**Working directory:** All work happens in the `ui-theming` worktree (`../kamaji-worktrees/ui-theming`). Run all `cargo`/`git` commands from there.

---

## File Structure

- **Create** `src/theme.rs` — the `Theme` struct, semantic role fields, built-in palette constructors, `ALL`/`by_name`/`index_of`/`status_color`. Pure, no I/O. Owns all color literals in the app.
- **Modify** `src/main.rs` — add `mod theme;`; set `app.theme` from `config.theme` at startup.
- **Modify** `src/config.rs` — add the `theme: String` field + `default_theme()` + back-compat test.
- **Modify** `src/app.rs` — add `theme: Theme` field to `App` (defaulted in `App::new`); add the `Modal::ThemePicker` variant.
- **Modify** `src/ui/mod.rs` — pass `&app.theme` into every modal renderer; add the `ThemePicker` render arm.
- **Modify** `src/ui/board.rs` — restyle: themed background, header+rule columns, rounded accent cards, themed selection/bullets/status bar; update existing tests.
- **Modify** `src/ui/modals.rs` — thread `&Theme` into every modal renderer; replace hard-coded colors; rounded borders; add `render_theme_picker`; add the theme line to help.
- **Modify** `src/engine.rs` — add `config_path` field; handle the `t` key and the `ThemePicker` modal (preview/persist/revert); tests.

---

## Task 1: The `Theme` model

**Files:**
- Create: `src/theme.rs`
- Modify: `src/main.rs` (add `mod theme;`)

- [ ] **Step 1: Add the module declaration**

In `src/main.rs`, find the block of `mod …;` declarations near the top and add:

```rust
mod theme;
```

- [ ] **Step 2: Write `src/theme.rs` with the type, palettes, and lookup**

Create `src/theme.rs`:

```rust
use ratatui::style::Color;

use crate::models::Status;

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
    pub todo: Color,
    pub in_progress: Color,
    pub review: Color,
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

    /// Index of `name` in `ALL`, or the default's index (Catppuccin) if unknown.
    pub fn index_of(name: &str) -> usize {
        Theme::ALL
            .iter()
            .position(|f| f().name == name)
            .unwrap_or(1)
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
```

- [ ] **Step 3: Run the theme tests to verify they pass**

Run: `cargo test --lib theme`
Expected: PASS (4 tests in the `theme` module). The crate should compile; `Theme` is unused elsewhere for now, which is fine.

- [ ] **Step 4: Commit**

```bash
git add src/theme.rs src/main.rs
git commit -m "feat(theme): add Theme model with built-in colorschemes"
```

---

## Task 2: Config `theme` key

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Write the failing back-compat test**

In `src/config.rs`, inside `mod tests`, add:

```rust
#[test]
fn missing_theme_defaults_to_catppuccin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // Write a config that predates the theme key by stripping it out.
    let text = toml::to_string_pretty(&Config::default())
        .unwrap()
        .lines()
        .filter(|l| !l.trim_start().starts_with("theme"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("theme"));
    fs::write(&path, text).unwrap();
    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded.theme, "catppuccin");
}

#[test]
fn default_config_theme_is_catppuccin() {
    assert_eq!(Config::default().theme, "catppuccin");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib config::tests::default_config_theme_is_catppuccin`
Expected: FAIL to compile — `Config` has no field `theme`.

- [ ] **Step 3: Add the field and default**

In `src/config.rs`, add a default helper near the other `default_*` fns (e.g. after `default_zellij_bar`):

```rust
fn default_theme() -> String {
    "catppuccin".to_string()
}
```

Add the field to the `Config` struct (after `zellij_bar`):

```rust
    /// Active colorscheme name. One of the built-in theme keys (see
    /// `crate::theme::Theme::ALL`), e.g. "catppuccin" or "default". Tolerates
    /// older configs that omit the key.
    #[serde(default = "default_theme")]
    pub theme: String,
```

In `Config::default()`, add the field to the constructed struct (after `zellij_bar: default_zellij_bar(),`):

```rust
            theme: default_theme(),
```

- [ ] **Step 4: Run the config tests to verify they pass**

Run: `cargo test --lib config`
Expected: PASS (existing config tests plus the two new ones).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add theme key (defaults to catppuccin)"
```

---

## Task 3: Theme on `App` + startup wiring

**Files:**
- Modify: `src/app.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write the failing test**

In `src/app.rs`, inside `mod tests`, add:

```rust
#[test]
fn app_has_a_default_theme() {
    let app = App::new(project(), vec![]);
    assert_eq!(app.theme.name, "catppuccin");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib app::tests::app_has_a_default_theme`
Expected: FAIL to compile — `App` has no field `theme`.

- [ ] **Step 3: Add the field to `App`**

In `src/app.rs`, add the import at the top (extend the existing `use crate::...` line or add a new one):

```rust
use crate::theme::Theme;
```

Add the field to the `App` struct (after `should_quit: bool,`):

```rust
    pub theme: Theme,
```

In `App::new`, set it in the constructed struct (after `should_quit: false,`):

```rust
            theme: Theme::by_name("catppuccin"),
```

- [ ] **Step 4: Run to verify the test passes**

Run: `cargo test --lib app::tests::app_has_a_default_theme`
Expected: PASS.

- [ ] **Step 5: Wire the configured theme at startup**

In `src/main.rs`, find where `App::new(...)` is constructed and the `Config` is available (the board setup path, before the `Engine` is built). Immediately after the `App` is created, set its theme from config. For example, if the code reads `let app = App::new(project, tickets);`, change to:

```rust
let mut app = App::new(project, tickets);
app.theme = crate::theme::Theme::by_name(&config.theme);
```

(Use the in-scope `config` binding. If `App::new` is called inside a larger expression, hoist it to a `let mut app` first. If `app` is already `mut`, just add the assignment line.)

- [ ] **Step 6: Verify the build**

Run: `cargo build`
Expected: compiles. A `field is never read: theme` warning is expected until Task 4/5 consume it — acceptable for this commit.

- [ ] **Step 7: Commit**

```bash
git add src/app.rs src/main.rs
git commit -m "feat(app): hold active Theme on App, set from config at startup"
```

---

## Task 4: Recolor modals from the theme

**Files:**
- Modify: `src/ui/modals.rs`
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: Write a failing test for themed modal borders**

In `src/ui/modals.rs`, add a `mod tests` at the end (the file currently has none):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::Terminal;

    #[test]
    fn confirm_modal_border_uses_theme() {
        let theme = Theme::by_name("nord");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_confirm(f, &theme, "T", "body"))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        // Some cell must carry the theme's border color (the modal frame).
        let found = (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| buf[Position::new(x, y)].fg == theme.border)
        });
        assert!(found, "confirm modal should draw its border in theme.border");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib modals`
Expected: FAIL to compile — `render_confirm` takes `(frame, title, body)`, not `(frame, theme, title, body)`.

- [ ] **Step 3: Thread `&Theme` through every modal renderer and replace colors**

In `src/ui/modals.rs`, update the imports to include `BorderType` and `Theme`:

```rust
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
```
```rust
use crate::theme::Theme;
```

Rewrite the helper and renderers so each takes `theme: &Theme` and pulls colors from it. Replace the existing functions with these signatures and bodies:

```rust
pub(crate) fn field_line(theme: &Theme, label: &str, value: &str, active: bool) -> Line<'static> {
    let style = if active {
        Style::new()
            .fg(theme.base.unwrap_or(Color::Black))
            .bg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.text)
    };
    let cursor = if active { "_" } else { "" };
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::new().fg(theme.accent())),
        Span::styled(format!("{value}{cursor}"), style),
    ])
}

pub(crate) fn render_field_modal(
    frame: &mut Frame,
    theme: &Theme,
    title: &str,
    fields: &[(&str, &str, bool)],
    hint: &str,
    error: Option<&str>,
) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = themed_block(theme, format!(" {title} "));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, (label, value, active)) in fields.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(field_line(theme, label, value, *active));
    }
    lines.push(Line::raw(""));
    if let Some(err) = error {
        lines.push(Line::styled(err.to_string(), Style::new().fg(theme.error)));
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled(hint.to_string(), Style::new().fg(theme.muted)));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
```

Add a small shared helper for the rounded, themed frame (place it above `field_line`):

```rust
/// A rounded modal frame titled `title`, bordered in the theme's border color.
fn themed_block(theme: &Theme, title: String) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .title(title)
}
```

Update `render_form` to take `theme` and use it:

```rust
pub fn render_form(frame: &mut Frame, theme: &Theme, form: &TicketForm) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let title = if form.editing_id.is_some() {
        " Edit ticket "
    } else {
        " New ticket "
    };
    let block = themed_block(theme, title.to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        field_line(theme, "Title", &form.title, form.field == FormField::Title),
        Line::raw(""),
        field_line(
            theme,
            "Description",
            &form.description,
            form.field == FormField::Description,
        ),
    ];
    if form.editing_id.is_none() {
        lines.push(Line::raw(""));
        lines.push(field_line(
            theme,
            "Prompt",
            &form.initial_prompt,
            form.field == FormField::InitialPrompt,
        ));
        lines.push(Line::raw(""));
        let agents: Vec<Span> = Agent::all()
            .into_iter()
            .flat_map(|a| {
                let sel = a == form.agent && form.field == FormField::Agent;
                let style = if sel {
                    Style::new().fg(theme.base.unwrap_or(Color::Black)).bg(theme.accent())
                } else if a == form.agent {
                    Style::new().fg(theme.accent())
                } else {
                    Style::new().fg(theme.muted)
                };
                vec![
                    Span::styled(format!(" {} ", a.label()), style),
                    Span::raw(" "),
                ]
            })
            .collect();
        let mut agent_line = vec![Span::styled("Agent: ", Style::new().fg(theme.accent()))];
        agent_line.extend(agents);
        lines.push(Line::from(agent_line));

        lines.push(Line::raw(""));
        let checkbox = if form.start_in_background {
            "[x]"
        } else {
            "[ ]"
        };
        lines.push(field_line(
            theme,
            "Start in background",
            checkbox,
            form.field == FormField::Background,
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Tab/Shift-Tab: field   ←/→: agent / toggle   Enter: save   Esc: cancel",
        Style::new().fg(theme.muted),
    ));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
```

Update `render_move`:

```rust
pub fn render_move(frame: &mut Frame, theme: &Theme, target: Status) {
    let area = centered_rect(60, 25, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, " Move ticket ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [cols_area, hint_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(2)]).areas(inner);
    let spans: Vec<Span> = Status::all()
        .into_iter()
        .map(|s| {
            let style = if s == target {
                Style::new()
                    .fg(theme.base.unwrap_or(Color::Black))
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme.text)
            };
            Span::styled(format!(" {} ", s.title()), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), cols_area);
    frame.render_widget(
        Paragraph::new("←/→: choose   Enter: confirm   Esc: cancel")
            .style(Style::new().fg(theme.muted)),
        hint_area,
    );
}
```

Update `render_confirm`:

```rust
pub fn render_confirm(frame: &mut Frame, theme: &Theme, title: &str, body: &str) {
    let area = centered_rect(50, 20, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, format!(" {title} "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::new().fg(theme.text))
            .wrap(Wrap { trim: true }),
        inner,
    );
}
```

Update `render_help` (add the theme line to the body text):

```rust
pub fn render_help(frame: &mut Frame, theme: &Theme) {
    let area = centered_rect(50, 60, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, " Help ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = "\
↑/↓ j/k   select ticket
←/→ h/l   change column
c         create ticket (auto-starts a background session)
e         edit ticket
Enter     attach / start session
m         move ticket (then ←/→, Enter)
d         delete ticket
t         switch theme (live preview)
p         switch project
?         this help
q         quit

Any key closes this help.";
    frame.render_widget(
        Paragraph::new(text).style(Style::new().fg(theme.text)),
        inner,
    );
}
```

- [ ] **Step 4: Update `ui/mod.rs` to pass the theme**

In `src/ui/mod.rs`, update the `render` match arms to pass `&app.theme`:

```rust
    match &app.modal {
        Modal::None => {}
        Modal::Form(form) => modals::render_form(frame, &app.theme, form),
        Modal::Move { target, .. } => modals::render_move(frame, &app.theme, *target),
        Modal::ConfirmDone { .. } => {
            modals::render_confirm(
                frame,
                &app.theme,
                "Move to Done",
                "Clean up worktree + session? [y]es / [n]o / Esc",
            );
        }
        Modal::ConfirmDelete { .. } => {
            modals::render_confirm(
                frame,
                &app.theme,
                "Delete ticket",
                "Delete and clean up? [y]es / Esc",
            );
        }
        Modal::Help => modals::render_help(frame, &app.theme),
    }
```

- [ ] **Step 5: Run the modal test to verify it passes**

Run: `cargo test --lib modals`
Expected: PASS. Also run `cargo build` — it should compile (the `theme` field on `App` is now read).

- [ ] **Step 6: Commit**

```bash
git add src/ui/modals.rs src/ui/mod.rs
git commit -m "feat(ui): source modal colors from the active theme; rounded borders"
```

---

## Task 5: Restyle the board

**Files:**
- Modify: `src/ui/board.rs`

This task rewrites the board's drawing functions and updates the existing tests for the new look. Work top-to-bottom.

- [ ] **Step 1: Update imports and the bullet-color helper**

In `src/ui/board.rs`, update imports:

```rust
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use ratatui::Frame;

use std::collections::HashMap;

use crate::app::App;
use crate::detect::SignalLevel;
use crate::models::{Status, Ticket};
use crate::theme::Theme;
```

Delete the `ORANGE` const (color now comes from the theme). Replace `bullet_color` so it takes the theme:

```rust
/// The fg color for a ticket's bullet, or `None` to inherit the card's text
/// style. Needs-attention bullets use the attention color; an actively working
/// In Progress bullet uses the active color; otherwise it inherits (idle).
fn bullet_color(theme: &Theme, status: Status, level: Option<SignalLevel>) -> Option<Color> {
    match status {
        Status::Review => Some(theme.attention),
        Status::InProgress if level == Some(SignalLevel::Active) => Some(theme.active),
        _ => None,
    }
}
```

- [ ] **Step 2: Rewrite `render_board` (themed background + headered columns)**

Replace `render_board` with:

```rust
pub fn render_board(frame: &mut Frame, app: &App, levels: &HashMap<i64, SignalLevel>) {
    let theme = &app.theme;

    // Paint the themed background (skip in default mode to keep the terminal's).
    if let Some(bg) = theme.base {
        frame.render_widget(Block::default().style(Style::new().bg(bg)), frame.area());
    }

    let [board_area, status_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());

    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(board_area);

    for (col_idx, status) in Status::all().into_iter().enumerate() {
        let tickets = app.column_tickets(status);
        let focused = col_idx == app.selected_col;
        render_column(
            frame,
            theme,
            columns[col_idx],
            status,
            &tickets,
            focused,
            app.selected_row,
            levels,
        );
    }

    let hints = " [↵]attach [e]dit [c]reate [m]ove [d]elete [t]heme [p]roject [?]help [q]uit";
    let left = format!(" project: {} ", app.project.name);
    let msg = app.status_message.clone().unwrap_or_default();
    let status_line = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::new().fg(theme.accent())),
        Span::styled(msg, Style::new().fg(theme.error)),
        Span::styled(hints, Style::new().fg(theme.muted)),
    ]));
    frame.render_widget(status_line, status_area);
}
```

- [ ] **Step 3: Rewrite `render_column` (header + rule, no outer box)**

Replace `render_column` with:

```rust
/// Render one Kanban column: a colored header (`TITLE · n`) and rule, then the
/// tickets as vertically stacked cards. The focused column's header is drawn in
/// the status accent; unfocused columns use the muted color.
#[allow(clippy::too_many_arguments)]
fn render_column(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    status: Status,
    tickets: &[&Ticket],
    focused: bool,
    selected_row: usize,
    levels: &HashMap<i64, SignalLevel>,
) {
    let accent = theme.status_color(status);
    let header_color = if focused { accent } else { theme.muted };

    let [header_area, rule_area, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);

    let title = format!(
        " {} · {}",
        status.title().to_uppercase(),
        tickets.len()
    );
    let mut header_style = Style::new().fg(header_color);
    if focused {
        header_style = header_style.add_modifier(Modifier::BOLD);
    }
    frame.render_widget(
        Paragraph::new(Line::styled(title, header_style)),
        header_area,
    );
    let rule = "─".repeat(rule_area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::styled(rule, Style::new().fg(header_color))),
        rule_area,
    );

    if tickets.is_empty() || body.height == 0 {
        return;
    }

    let visible = visible_cards(body.height);
    let offset = if focused {
        first_visible(selected_row, visible, tickets.len())
    } else {
        0
    };

    let slot = CARD_HEIGHT + CARD_GAP;
    let bottom = body.y + body.height;
    for (i, ticket) in tickets.iter().enumerate().skip(offset) {
        let y = body.y + (i - offset) as u16 * slot;
        if y >= bottom {
            break;
        }
        let height = CARD_HEIGHT.min(bottom - y);
        let card = Rect {
            x: body.x,
            y,
            width: body.width,
            height,
        };
        let selected = focused && i == selected_row;
        let level = levels.get(&ticket.id).copied();
        render_card(frame, theme, card, ticket, selected, level);
    }
}
```

- [ ] **Step 4: Rewrite `render_card` (accent strip + rounded box)**

Replace `render_card` with:

```rust
/// Render a single ticket as a rounded card with a colored left accent strip.
/// The selected card gets an accent border and a `surface` fill.
fn render_card(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    ticket: &Ticket,
    selected: bool,
    level: Option<SignalLevel>,
) {
    let accent = theme.status_color(ticket.status);

    // 1-cell accent strip on the far left; the rounded box fills the rest.
    let strip = Rect {
        x: area.x,
        y: area.y,
        width: 1,
        height: area.height,
    };
    let box_area = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };
    frame.render_widget(Block::default().style(Style::new().bg(accent)), strip);

    let (border_color, fill, base_text) = if selected {
        (
            accent,
            Some(theme.surface),
            Style::new().fg(theme.text).bg(theme.surface).add_modifier(Modifier::BOLD),
        )
    } else {
        (theme.border, None, Style::new().fg(theme.muted))
    };

    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color))
        .padding(Padding::horizontal(1));
    if let Some(bg) = fill {
        block = block.style(Style::new().bg(bg));
    }

    let marker = if ticket.session_name.is_some() {
        "●"
    } else {
        "○"
    };
    let marker_span = match bullet_color(theme, ticket.status, level) {
        Some(c) => Span::styled(marker, Style::new().fg(c)),
        None => Span::raw(marker),
    };

    let line = Line::from(vec![
        marker_span,
        Span::styled(format!(" #{} ", ticket.id), Style::new().fg(accent)),
        Span::styled(ticket.title.clone(), Style::new().fg(theme.text)),
    ])
    .style(base_text);

    frame.render_widget(Paragraph::new(line).block(block), box_area);
}
```

(`CARD_HEIGHT`, `CARD_GAP`, `visible_cards`, `first_visible` are unchanged — keep them as-is.)

- [ ] **Step 5: Update the existing tests for the new look**

In `src/ui/board.rs` `mod tests`, make these changes:

Update the `render` helper to seed a known theme (insert after building `app` — note the helper takes `&App`, so set the theme on the caller's `app`; instead, seed it inside each test that needs determinism). Simplest: add a helper that builds an app with a chosen theme:

```rust
fn app_with_theme(tickets: Vec<Ticket>, theme_name: &str) -> App {
    let mut app = App::new(project(), tickets);
    app.theme = crate::theme::Theme::by_name(theme_name);
    app
}
```

Replace `bullet_color_maps_status_and_activity` with a theme-aware version:

```rust
#[test]
fn bullet_color_maps_status_and_activity() {
    let t = crate::theme::Theme::by_name("catppuccin");
    assert_eq!(bullet_color(&t, Status::Review, None), Some(t.attention));
    assert_eq!(
        bullet_color(&t, Status::Review, Some(SignalLevel::Active)),
        Some(t.attention)
    );
    assert_eq!(
        bullet_color(&t, Status::InProgress, Some(SignalLevel::Active)),
        Some(t.active)
    );
    assert_eq!(bullet_color(&t, Status::InProgress, Some(SignalLevel::Idle)), None);
    assert_eq!(bullet_color(&t, Status::InProgress, Some(SignalLevel::Unknown)), None);
    assert_eq!(bullet_color(&t, Status::InProgress, None), None);
    assert_eq!(bullet_color(&t, Status::Todo, Some(SignalLevel::Active)), None);
    assert_eq!(bullet_color(&t, Status::Done, None), None);
}
```

Replace `renders_tickets_as_cards_with_borders` to assert rounded borders:

```rust
#[test]
fn renders_tickets_as_cards_with_borders() {
    let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
    let buf = render(&app, &HashMap::new(), 80, 20);
    let text = buffer_text(&buf);
    assert!(text.contains("#1"), "expected ticket id in:\n{text}");
    assert!(text.contains("title1"), "expected title in:\n{text}");
    // Rounded card corners.
    assert!(
        text.contains('╭') && text.contains('╰'),
        "expected rounded card borders in:\n{text}"
    );
}
```

Replace `selected_card_has_filled_background` to assert the theme surface fill:

```rust
#[test]
fn selected_card_has_filled_background() {
    let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
    let theme = crate::theme::Theme::by_name("catppuccin");
    let buf = render(&app, &HashMap::new(), 80, 20);
    let has_surface = (0..buf.area.height)
        .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].bg == theme.surface));
    assert!(has_surface, "selected card should be filled with theme.surface");
}
```

Update the bullet-color buffer tests to use `live_ticket` + a known theme and assert against theme roles:

```rust
#[test]
fn needs_attention_bullet_is_attention_color() {
    let app = app_with_theme(vec![live_ticket(1, Status::Review)], "catppuccin");
    let theme = crate::theme::Theme::by_name("catppuccin");
    let buf = render(&app, &HashMap::new(), 80, 20);
    assert_eq!(bullet_fg(&buf), Some(theme.attention));
}

#[test]
fn in_progress_active_bullet_is_active_color() {
    let app = app_with_theme(vec![live_ticket(1, Status::InProgress)], "catppuccin");
    let theme = crate::theme::Theme::by_name("catppuccin");
    let mut levels = HashMap::new();
    levels.insert(1, SignalLevel::Active);
    let buf = render(&app, &levels, 80, 20);
    assert_eq!(bullet_fg(&buf), Some(theme.active));
}

#[test]
fn in_progress_idle_bullet_keeps_default_color() {
    let app = app_with_theme(vec![live_ticket(1, Status::InProgress)], "catppuccin");
    let theme = crate::theme::Theme::by_name("catppuccin");
    let mut levels = HashMap::new();
    levels.insert(1, SignalLevel::Idle);
    let buf = render(&app, &levels, 80, 20);
    // Idle: the selected card's bullet inherits the card text color (muted),
    // not the active/attention color.
    assert_eq!(bullet_fg(&buf), Some(theme.muted));
}
```

Update `overflowing_column_keeps_selection_visible_without_panic` to use `app_with_theme` (the assertion on `#20` stays valid):

```rust
#[test]
fn overflowing_column_keeps_selection_visible_without_panic() {
    let tickets: Vec<Ticket> = (1..=20).map(|i| ticket(i, Status::Todo)).collect();
    let mut app = app_with_theme(tickets, "catppuccin");
    app.selected_row = 19;
    let buf = render(&app, &HashMap::new(), 80, 12);
    let text = buffer_text(&buf);
    assert!(text.contains("#20"), "selected card should be visible:\n{text}");
}
```

Note for the idle test: `bullet_fg` returns the fg of the bullet cell. For a selected idle card the line's `base_text` is `fg(theme.text).bg(surface)` and the bullet inherits it — so the bullet would be `theme.text`, not `theme.muted`. Since the single rendered card is selected (row 0 focused), set the expectation to `theme.text`:

```rust
    // Selected idle card: bullet inherits the selected text color.
    assert_eq!(bullet_fg(&buf), Some(theme.text));
```

Use this `theme.text` expectation (the card under test is selected). Keep the comment accurate.

- [ ] **Step 6: Run the board tests**

Run: `cargo test --lib board`
Expected: PASS. If `bullet_fg` finds the accent strip or a border glyph instead of the bullet, confirm `bullet_fg` scans for `●`/`○` specifically (it does) — it returns the bullet cell, unaffected by the strip.

- [ ] **Step 7: Run the whole suite and lint**

Run: `cargo test && cargo clippy`
Expected: all tests PASS; no clippy errors (warnings are acceptable but prefer none).

- [ ] **Step 8: Commit**

```bash
git add src/ui/board.rs
git commit -m "feat(ui): refined board — rounded accent cards, headered columns, themed colors"
```

---

## Task 6: In-app theme picker

**Files:**
- Modify: `src/app.rs` (add `Modal::ThemePicker`)
- Modify: `src/ui/modals.rs` (add `render_theme_picker`)
- Modify: `src/ui/mod.rs` (render the picker)
- Modify: `src/engine.rs` (add `config_path`; handle `t` + the picker modal)

- [ ] **Step 1: Add the `Modal::ThemePicker` variant**

In `src/app.rs`, add to the `Modal` enum (after `Help,`):

```rust
    /// Theme picker: live-previews `Theme::ALL[selected]`; `original` is the
    /// index to restore on cancel.
    ThemePicker { selected: usize, original: usize },
```

- [ ] **Step 2: Add `config_path` to `Engine` and write the failing picker tests**

In `src/engine.rs`, add to the `Engine` struct (after `state_dir`):

```rust
    /// Where the theme picker persists the chosen theme. Defaults to the real
    /// config path; tests override it.
    pub config_path: std::path::PathBuf,
```

In `Engine::new`, initialize it (after `state_dir: detect::default_state_dir(),`):

```rust
            config_path: crate::config::config_path().unwrap_or_default(),
```

Add these tests in `src/engine.rs` `mod tests`:

```rust
#[test]
fn t_opens_theme_picker_at_current_theme() {
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    e.config.theme = "nord".to_string();
    e.app.theme = crate::theme::Theme::by_name("nord");
    e.on_key(key('t')).unwrap();
    match e.app.modal {
        Modal::ThemePicker { selected, original } => {
            let idx = crate::theme::Theme::index_of("nord");
            assert_eq!(selected, idx);
            assert_eq!(original, idx);
        }
        ref other => panic!("expected ThemePicker, got {other:?}"),
    }
}

#[test]
fn picker_down_previews_next_theme() {
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    e.app.modal = Modal::ThemePicker { selected: 0, original: 0 };
    e.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)).unwrap();
    // app.theme now previews ALL[1].
    assert_eq!(e.app.theme.name, crate::theme::Theme::ALL[1]().name);
    match e.app.modal {
        Modal::ThemePicker { selected, .. } => assert_eq!(selected, 1),
        ref other => panic!("expected ThemePicker, got {other:?}"),
    }
}

#[test]
fn picker_enter_persists_theme_to_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    e.config_path = dir.path().join("config.toml");
    // Preview Nord (find its index), then Enter.
    let nord = crate::theme::Theme::index_of("nord");
    e.app.modal = Modal::ThemePicker { selected: nord, original: 0 };
    e.app.theme = crate::theme::Theme::ALL[nord]();
    e.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
    assert!(matches!(e.app.modal, Modal::None));
    assert_eq!(e.config.theme, "nord");
    let saved = crate::config::load_from(&e.config_path).unwrap();
    assert_eq!(saved.theme, "nord");
}

#[test]
fn picker_esc_reverts_to_original_theme() {
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    e.app.theme = crate::theme::Theme::ALL[0]();
    let nord = crate::theme::Theme::index_of("nord");
    e.app.modal = Modal::ThemePicker { selected: nord, original: 0 };
    e.app.theme = crate::theme::Theme::ALL[nord](); // mid-preview
    e.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
    assert!(matches!(e.app.modal, Modal::None));
    assert_eq!(e.app.theme.name, crate::theme::Theme::ALL[0]().name);
}
```

- [ ] **Step 3: Run to verify the tests fail**

Run: `cargo test --lib engine::tests::t_opens_theme_picker_at_current_theme`
Expected: FAIL to compile — `Modal::ThemePicker` handling and the `t` key don't exist yet (or compile error on the new struct field if Step 2 struct edits are incomplete).

- [ ] **Step 4: Handle the `t` key on the board**

In `src/engine.rs`, in `on_board_key`, add an arm (next to `'p'`/`'?'`):

```rust
            KeyCode::Char('t') => {
                let idx = Theme::index_of(&self.config.theme);
                self.app.modal = Modal::ThemePicker {
                    selected: idx,
                    original: idx,
                };
            }
```

Add the import at the top of `src/engine.rs`:

```rust
use crate::theme::Theme;
```

- [ ] **Step 5: Handle the `ThemePicker` modal in `on_key`**

In `src/engine.rs`, in the `on_key` `match modal { … }`, add a new arm (before `Modal::Help`):

```rust
            Modal::ThemePicker {
                mut selected,
                original,
            } => match key.code {
                KeyCode::Esc => {
                    self.app.theme = Theme::ALL[original]();
                    Ok(Effect::None)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                    self.app.theme = Theme::ALL[selected]();
                    self.app.modal = Modal::ThemePicker { selected, original };
                    Ok(Effect::None)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1).min(Theme::ALL.len() - 1);
                    self.app.theme = Theme::ALL[selected]();
                    self.app.modal = Modal::ThemePicker { selected, original };
                    Ok(Effect::None)
                }
                KeyCode::Enter => {
                    self.config.theme = self.app.theme.name.to_string();
                    crate::config::save_to(&self.config_path, &self.config)?;
                    self.app.status_message = Some(format!("theme: {}", self.app.theme.label));
                    Ok(Effect::None)
                }
                _ => {
                    self.app.modal = Modal::ThemePicker { selected, original };
                    Ok(Effect::None)
                }
            },
```

- [ ] **Step 6: Run the engine picker tests**

Run: `cargo test --lib engine`
Expected: PASS (existing engine tests plus the four new picker tests).

- [ ] **Step 7: Add `render_theme_picker` and render it**

In `src/ui/modals.rs`, add:

```rust
pub fn render_theme_picker(frame: &mut Frame, theme: &Theme, selected: usize) {
    let area = centered_rect(40, 50, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, " Theme ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, make) in Theme::ALL.iter().enumerate() {
        let label = make().label;
        let marker = if i == selected { "▸ " } else { "  " };
        let style = if i == selected {
            Style::new()
                .fg(theme.base.unwrap_or(Color::Black))
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.text)
        };
        lines.push(Line::styled(format!("{marker}{label}"), style));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "↑/↓ preview · ↵ save · Esc cancel",
        Style::new().fg(theme.muted),
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
```

Add the import to `src/ui/modals.rs` (extend the existing `use crate::theme::Theme;` — already added in Task 4 — and ensure `Theme::ALL` is reachable; it is via that import).

In `src/ui/mod.rs`, add the render arm (after the `Modal::Help` arm):

```rust
        Modal::ThemePicker { selected, .. } => {
            modals::render_theme_picker(frame, &app.theme, *selected)
        }
```

- [ ] **Step 8: Verify build, full suite, and lint**

Run: `cargo build && cargo test && cargo clippy`
Expected: compiles; all tests PASS; no clippy errors.

- [ ] **Step 9: Commit**

```bash
git add src/app.rs src/engine.rs src/ui/modals.rs src/ui/mod.rs
git commit -m "feat(ui): in-app theme picker with live preview, persists to config"
```

---

## Task 7: Manual verification + docs

**Files:**
- Modify: `README.md` (if it documents config keys or features)

- [ ] **Step 1: Smoke-test the running app**

Run: `cargo run`
Verify by eye:
- The board shows rounded cards with colored left accent strips and headered columns; the focused column header is bright/bold.
- Selected card has the surface fill + accent border.
- Press `t`: the picker opens; `↑/↓` recolors the board live; `Enter` keeps the choice and shows a `theme: …` toast; reopening shows the saved selection. `Esc` reverts.
- Quit and relaunch (`cargo run`): the chosen theme persists (read from `config.toml`).
- Set `theme = "default"` in `config.toml`, relaunch: no forced background; the UI uses the terminal's 16 colors.

- [ ] **Step 2: Update the README if needed**

If `README.md` lists config keys or a feature list, add a short "Themes" note: available theme names (`default`, `catppuccin`, `tokyonight`, `gruvbox`, `nord`), the `theme` config key, and the `t` in-app picker. Keep it to a few lines matching the existing README tone.

- [ ] **Step 3: Commit (if the README changed)**

```bash
git add README.md
git commit -m "docs: document theme config key and in-app picker"
```

---

## Self-Review notes (already applied)

- **Spec coverage:** Theme model + built-ins + default (Task 1); config key + back-compat (Task 2); theme on App + startup (Task 3); modal recolor + rounded borders (Task 4); board restyle incl. background, headers, accent cards, themed bullets/status bar (Task 5); in-app picker with preview/persist/revert + help/hints (Tasks 5–6). README (Task 7). All spec sections map to a task.
- **Naming consistency:** `Theme::ALL` (`&[fn() -> Theme]`), `by_name`, `index_of`, `status_color`, `accent`, `themed_block`, `render_theme_picker`, `config_path`, `Modal::ThemePicker { selected, original }`, theme keys `default`/`catppuccin`/`tokyonight`/`gruvbox`/`nord` — used identically across tasks.
- **Selected-idle bullet:** the rendered single card is selected, so its idle bullet inherits the selected text color; the test asserts `theme.text` (noted in Task 5 Step 5).
```
