# Modal project picker — design

## Problem

The startup project picker (`src/picker.rs`) renders full-screen as a three-row
vertical layout: a title line, a bordered project `List` that fills the height,
and a hint line. It works, but it doesn't match the rest of the UI, which uses
centered, rounded-bordered **modals** (`src/ui/modals.rs`) for every overlay
(ticket form, move, confirm, theme picker, help). The picker should look like one
of those modals — a polished, floating box — rather than a full-screen view.

## Goal

Turn the picker into a centered, floating modal that matches the app's modal
language, sitting on a dimmed backdrop. Behavior (event loop, key handling, the
new-project form) is unchanged; only the rendering changes, plus one small theme
helper.

## Design

### Layout — compact unified modal

A single centered, rounded-bordered box (the same `BorderType::Rounded` +
`theme.border` frame as every other modal), top to bottom:

- **Border title:** ` kamaji ` — the modal's frame title, in `theme.border`.
- **Subtitle line:** `Select a project`, in `theme.muted`.
- **Blank spacer line.**
- **Project list:** one row per project — `name` in `theme.text` followed by the
  `~/path` in `theme.muted`. The selected row is marked with a `› ` highlight
  symbol and a `theme.accent()` background with `theme.base` (fallback black)
  foreground and `BOLD`, i.e. the same selection treatment used elsewhere
  (`field_line`, theme picker, move modal).
- **Blank spacer line.**
- **Footer hint:** `↑/↓ select · ↵ open · n new · q quit`, in `theme.muted`.

**Empty state:** when there are no projects, the list area shows
`No projects yet — press n to create one.` in `theme.muted` and no selection
highlight is drawn.

### Sizing — fixed, content-aware

- **Width:** fixed at **52 columns** (enough for a name plus a `~/path`),
  clamped to the frame width on very narrow terminals.
- **Height:** computed from content =
  `2 (top+bottom border) + 1 (subtitle) + 1 (blank) + N (rows) + 1 (blank) + 1 (hint)`,
  where `N = projects.len().clamp(1, MAX_VISIBLE_ROWS)` with
  `MAX_VISIBLE_ROWS = 12`. When there are more projects than the cap, the inner
  list scrolls via `ListState` (which keeps the selected index visible); the box
  height stays fixed at the cap. Height is also clamped to the frame height.
- **Position:** centered horizontally and vertically. A new helper centers a
  **fixed-size** rect (the existing `centered_rect` takes percentages, which is
  the wrong tool here), e.g. `centered_fixed(width, height, frame.area())`,
  clamping the size to the area before centering.

### Backdrop — dimmed variant of the modal color

Before drawing the modal, fill the entire frame with a dimmed backdrop so the
modal reads as floating/elevated above it:

- For themes with a background (`base: Some(color)`), the backdrop is that base
  color **darkened toward black** (multiply each RGB channel by ~`0.6`). The
  modal interior is then drawn on the normal, brighter `theme.base` (after a
  `Clear`), so it stands out against the dimmed field — a "dimmed variant of the
  modal's color" sitting behind it.
- For `default_ansi` (`base: None`, no forced background), the backdrop falls
  back to `Color::Black` and the modal interior stays the terminal default.

This adds a small theme helper that lives with the theme so the rule is reusable
and testable, e.g.:

```rust
impl Theme {
    /// The dimmed backdrop drawn behind a full-screen modal (the picker).
    /// A darkened variant of `base`; black when the theme forces no background.
    pub fn backdrop(&self) -> Color {
        match self.base {
            Some(Color::Rgb(r, g, b)) => Color::Rgb(
                (r as f32 * 0.6) as u8,
                (g as f32 * 0.6) as u8,
                (b as f32 * 0.6) as u8,
            ),
            _ => Color::Black,
        }
    }
}
```

The backdrop is painted by rendering a `Block` (or a `Clear`-then-styled
`Paragraph`) with that background color over `frame.area()`.

### Unchanged

- `picker::run` — the event loop and all key handling (`q`, `n`, `↑/↓`/`j`/`k`,
  `Enter`, and the form's `Tab`/`Esc`/typing) are untouched.
- The new-project form still overlays via the shared `render_field_modal`. With
  the new backdrop it layers naturally: dimmed backdrop → picker modal → form
  modal on top.
- All colors continue to come from `Theme`; no hardcoded colors except the
  existing black fallbacks.

## Components touched

- `src/picker.rs` — rewrite `render()` to draw: (1) the dimmed backdrop over the
  whole frame, (2) the centered fixed-size modal box, (3) the subtitle, list, and
  hint inside it. Add the row-formatting (name + muted path) and empty-state.
- `src/theme.rs` — add `Theme::backdrop()` and a unit test for it.
- `src/ui/mod.rs` — add `centered_fixed(width, height, area)` (fixed-size,
  clamped, centered) alongside the existing `centered_rect`, with a unit test.

## Testing

- **Theme:** `backdrop()` darkens an RGB base and returns black for `default_ansi`.
- **Layout:** `centered_fixed` centers a fixed-size rect and clamps to the area
  when the requested size exceeds it.
- **Render (TestBackend, in `picker.rs`):** render the picker with a couple of
  projects on an 80×24 backend and assert both:
  1. some cell carries `theme.border` (the modal frame is drawn), and
  2. at least one corner/edge cell carries the backdrop background color and lies
     outside the modal box — i.e. it is genuinely a centered modal, not
     full-screen.
- Existing form-logic tests in `picker.rs` are unchanged.

## Out of scope

- No changes to picker behavior, key bindings, or the new-project form flow.
- No decorative wordmark/banner behind the modal (considered and declined).
- No mouse support.
