# UI restyle + colorscheme themes

**Date:** 2026-05-24
**Status:** Approved (design)

## Goal

Make kamaji look less like a raw terminal dump and more like a polished Kanban
app (Jira/Linear feel) without leaving the terminal, and let users pick a
colorscheme. Two parts:

1. **Restyle the board** ("refined terminal" direction): rounded card borders, a
   colored left accent strip per card, lighter column headers with a rule
   (dropping the heavy outer column box), and a clearer selected-card treatment.
   Recolor the modals and status bar to match so the whole app is coherent.
2. **Themes**: a small library of built-in colorschemes — **Catppuccin Mocha,
   Tokyo Night, Gruvbox Dark, Nord**, plus a **Default** mode that uses the
   terminal's own 16 ANSI colors. Selectable from a config key *and* from an
   in-app picker with live preview.

Every visible color in the app is sourced from the active theme; no rendering
code hard-codes a `Color` anymore (except the theme definitions themselves).

## Current state

- Rendering entry point is `ui::render(frame, &engine.app, &engine.last_level)`
  (`src/ui/mod.rs`), which calls `board::render_board` and the modal renderers.
- `src/ui/board.rs` draws each ticket as its own **square double-bordered** card
  (`Block::bordered`, `CARD_HEIGHT = 3`) stacked inside a bordered column box.
  Colors are hard-coded: `Color::Cyan` (focus/selection), `Color::DarkGray` /
  `Color::Gray` (idle), `ORANGE`/`Color::Green` (status bullets),
  `Color::Yellow` (labels), `Color::Red` (errors).
- `src/ui/modals.rs` hard-codes the same `Cyan`/`Yellow`/`Gray`/`Red` set across
  the form, move, confirm, and help modals.
- `bullet_color(status, level)` already maps status/activity → a bullet color
  using the `SignalLevel` map threaded in from `Engine::last_level`.
- `Engine` (`src/engine.rs`) owns `db`, `config`, `app`. `on_key` takes the
  modal by `mem::replace` and dispatches per-`Modal`. Board keys are handled in
  `on_board_key`. No theme state exists anywhere yet.
- `App` (`src/app.rs`) holds UI state and the `Modal` enum.
- `Config` (`src/config.rs`) is TOML-backed; fields use `#[serde(default = …)]`
  for back-compat. Persistence is `config::save_to(path, &cfg)` /
  `config::config_path()`.

## Design

### 1. The `Theme` model (`src/theme.rs`, new)

A `Theme` is a flat struct of **semantic roles**, each a `ratatui::style::Color`,
plus an optional background:

```rust
pub struct Theme {
    pub name: &'static str,        // stable key, e.g. "catppuccin"
    pub label: &'static str,       // display, e.g. "Catppuccin Mocha"
    pub base: Option<Color>,       // app background; None = terminal default
    pub surface: Color,            // selected-card fill, subtle panels
    pub text: Color,               // primary text
    pub muted: Color,              // dim text, idle bullets, rules, empty cols
    pub border: Color,             // idle card + modal borders
    pub todo: Color,               // per-column accents …
    pub in_progress: Color,
    pub review: Color,
    pub done: Color,
    pub active: Color,             // green "agent working" bullet
    pub attention: Color,          // Needs-attention pulse
    pub error: Color,              // error/toast text
}
```

Helper for the per-column accent so callers don't match on `Status` inline:

```rust
impl Theme {
    pub fn status_color(&self, status: Status) -> Color { … } // todo/in_progress/review/done
}
```

**Built-ins** as `const` values (or `fn`s returning `Theme`):
`catppuccin()`, `tokyo_night()`, `gruvbox()`, `nord()`, `default_ansi()`.

- The four named themes use truecolor `Color::Rgb(..)` with the palettes shown
  in the approved mockups, and a concrete `base`.
- `default_ansi()` uses the 16 ANSI names only (`Color::Blue`, `Color::Green`,
  `Color::Yellow`, `Color::Red`, `Color::Cyan`, `Color::Magenta`, `Color::Gray`,
  `Color::DarkGray`, `Color::White`) and `base = None`, so it inherits whatever
  theme the user's terminal already runs.

Lookup + ordering for the picker:

```rust
impl Theme {
    /// Display order in the picker; first entry is the safe fallback.
    pub const ALL: &'static [fn() -> Theme] =
        &[default_ansi, catppuccin, tokyo_night, gruvbox, nord];
    pub fn by_name(name: &str) -> Theme;  // unknown -> catppuccin (the default)
    pub fn index_of(name: &str) -> usize; // for picker init; unknown -> default's index
}
```

Concrete palettes (RGB) to encode, matching the mockups:

| Theme | base | text | muted | border | todo | in_progress | review | done | active | attention |
|-------|------|------|-------|--------|------|-------------|--------|------|--------|-----------|
| Catppuccin | `#1e1e2e` | `#cdd6f4` | `#6c7086` | `#45475a` | `#9399b2` | `#89b4fa` | `#fab387` | `#a6e3a1` | `#a6e3a1` | `#fab387` |
| Tokyo Night | `#1a1b26` | `#c0caf5` | `#565f89` | `#414868` | `#565f89` | `#7aa2f7` | `#ff9e64` | `#9ece6a` | `#9ece6a` | `#ff9e64` |
| Gruvbox | `#282828` | `#ebdbb2` | `#928374` | `#504945` | `#a89984` | `#83a598` | `#fe8019` | `#b8bb26` | `#b8bb26` | `#fe8019` |
| Nord | `#2e3440` | `#d8dee9` | `#616e88` | `#434c5e` | `#616e88` | `#88c0d0` | `#d08770` | `#a3be8c` | `#a3be8c` | `#d08770` |

`surface` is one notch above `base` (e.g. Catppuccin `#313244`); `error` reuses
each palette's red. Default mode: `base = None`, `surface = Color::DarkGray`,
`text = Color::Gray`, `muted = Color::DarkGray`, `border = Color::DarkGray`,
`todo = Gray`, `in_progress = Cyan`, `review = Yellow`, `done = Green`,
`active = Green`, `attention = Yellow`, `error = Red`.

### 2. Config integration

Add to `Config`:

```rust
#[serde(default = "default_theme")]
pub theme: String,                 // theme name; e.g. "catppuccin"
fn default_theme() -> String { "catppuccin".to_string() }
```

- `Config::default()` sets `theme: default_theme()`.
- Missing key in an existing `config.toml` loads fine (serde default) — covered
  by a back-compat test like the existing `missing_zellij_bar_defaults_to_auto`.
- **Out-of-box default is `"catppuccin"`** (approved) so first run looks polished;
  users who prefer their terminal's colors set `theme = "default"`.

### 3. Theme lives on `App`

`App` gains `pub theme: Theme`. The picker mutates it for live preview, so it's
UI state and belongs on `App` (mirrors how `modal` lives there).

- `App::new(project, tickets)` defaults `theme: Theme::by_name("catppuccin")`
  so existing call sites and tests keep compiling; the real value is set at
  startup.
- At startup (where `Engine`/`App` are constructed in `main.rs`), set
  `app.theme = Theme::by_name(&config.theme)`.

Render functions read `app.theme`; modal renderers take `&Theme` as a parameter
(they don't get `&App` today). No `SignalLevel`/theme state is duplicated into
the DB.

### 4. Board restyle (`src/ui/board.rs`)

The "refined terminal" look from the approved mockup:

- **Background:** if `theme.base` is `Some`, paint the whole frame with a
  `Block::default().style(Style::new().bg(base))` before drawing columns; if
  `None` (default mode), skip — terminal background shows through.
- **Columns:** drop the outer bordered box. Each column is a header line
  `IN PROGRESS · 3` styled in `theme.status_color(status)` (bold), followed by a
  one-cell horizontal rule in the same color. The **focused** column's header +
  rule are drawn bright/bold; unfocused use `muted`. The card-stacking and
  scrolling math (`visible_cards`, `first_visible`, slot height) is reused,
  adjusted for the header rows replacing the box border.
- **Cards:** each card is a `Block` with `BorderType::Rounded`, border in
  `theme.border`, and a **1-column accent strip** drawn to the card's left in
  `theme.status_color(status)` (a filled `▎`/block-styled cell column). Content
  line is `<bullet> #<id> <title>` with id in the column accent and title in
  `theme.text`.
- **Selected card:** border switches to the column accent color, bold; the card
  area is filled with `theme.surface`; text bold. (Replaces today's flat cyan
  fill.)
- **Bullets:** keep `bullet_color`'s shape but source from the theme —
  `Status::Review → theme.attention`, `InProgress + Active → theme.active`,
  else `None` (inherit card text style, which is `muted` when idle). `●`/`○`
  session-presence glyph unchanged.
- **Status bar:** `project: …` in a theme color, the message in `theme.error`,
  hints in `theme.muted`. Add `[t]heme` to the hint string.

`BorderType::Rounded` is set via `Block::bordered().border_type(BorderType::Rounded)`.

### 5. Modals + status bar (`src/ui/modals.rs`)

Thread `&Theme` into every modal renderer (`render_form`, `render_move`,
`render_confirm`, `render_help`, `render_field_modal`, `field_line`) and replace
the hard-coded colors:

- borders → `theme.border`, with `BorderType::Rounded`; the active/title accent
  → the theme's accent (use `in_progress` as the generic "accent" role, or add a
  dedicated `accent` field — **decision: reuse `in_progress` as accent** to keep
  the struct small).
- active field highlight → `fg(base or black) bg(accent)`; labels → a theme
  color; hints → `muted`; errors → `theme.error`.
- `ui::render` passes `&app.theme` to each modal arm.

### 6. In-app theme picker

- New `Modal` variant:

  ```rust
  ThemePicker { selected: usize, original: usize }, // indices into Theme::ALL
  ```

- Key **`t`** on the board (`on_board_key`) opens it with
  `selected = original = Theme::index_of(&self.config.theme)`.
- Handling in `on_key` (new `Modal::ThemePicker` arm):
  - `Up`/`Down` (and `k`/`j`): move `selected` within `0..ALL.len()` and
    **live-apply** `self.app.theme = Theme::ALL[selected]()` so the board behind
    the modal recolors immediately.
  - `Enter`: commit — `self.config.theme = self.app.theme.name.to_string()`,
    `config::save_to(&config::config_path()?, &self.config)?`, close. Toast:
    `"theme: <label>"`.
  - `Esc`: revert — `self.app.theme = Theme::ALL[original]()`, close.
- Picker rendering (`render_theme_picker` in `modals.rs`): a centered rounded
  modal listing `Theme::ALL` labels, the `selected` row highlighted with the
  accent, hint line `↑/↓ preview · ↵ save · Esc cancel`.
- Add `t  switch theme (live preview)` to the help screen.

The config-key path (§2) and the picker both end at the same `config.theme`
string, so they're consistent: the picker just writes that key.

## Testing

Reuse the repo's `TestBackend` buffer-inspection style (as in `board.rs`) and
unit tests (as in `config.rs`/`engine.rs`).

- **theme.rs:** `by_name` returns the matching theme; unknown name →
  Catppuccin; `index_of` round-trips with `ALL`; `default_ansi().base` is
  `None`; a named theme's `base` is `Some`.
- **config.rs:** a `config.toml` without `theme` loads with
  `theme == "catppuccin"` (back-compat); `Config::default().theme` is
  `"catppuccin"`; save/load round-trips `theme`.
- **board.rs:** with a known theme, the selected column header cell uses the
  status accent fg; a card border cell is the rounded glyph (`╭`/`╰`); the
  accent strip cell carries the status color; an In-Progress+Active bullet uses
  `theme.active`. Update existing `render`/`render_board` test helpers to seed
  `app.theme`.
- **engine.rs:** pressing `t` opens `Modal::ThemePicker` with `selected ==
  index_of(config.theme)`; `Down` then advances `selected` and mutates
  `app.theme`; `Enter` writes the new name into `config.theme` (assert via an
  in-memory/temp config path) and closes; `Esc` restores `app.theme` to the
  original and closes.

## Out of scope

- User-defined / custom themes via config (only the built-in library ships).
  The `Theme` struct is shaped to allow it later (TOML-deserializable roles) but
  no parsing is built now.
- Per-project themes (theme is a single global config key).
- Live config-file hot-reload (theme changes via the file take effect on next
  launch; the picker is the live path).
- Animations / "pulsing" bullets (the `attention` role is a static color).
- Changing the board's information architecture (columns, card contents, keys)
  beyond what the restyle requires.
