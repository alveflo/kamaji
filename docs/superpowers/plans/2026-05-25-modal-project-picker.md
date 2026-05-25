# Modal Project Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the full-screen startup project picker into a centered, floating modal that matches the app's existing modal style, sitting on a dimmed backdrop.

**Architecture:** Add a `Theme::backdrop()` color helper and a `centered_fixed()` layout helper, then rewrite `picker::render()` to paint a dimmed backdrop over the whole frame and draw a fixed-size, content-aware, rounded-bordered modal box (title ` kamaji `, a subtitle, the project list, and a hint footer) centered on top. Event handling and the new-project form are untouched.

**Tech Stack:** Rust, ratatui (TUI), crossterm. Tests use ratatui's `TestBackend`.

---

## File Structure

- `src/theme.rs` — add `Theme::backdrop()` (dimmed-backdrop color) + unit test.
- `src/ui/mod.rs` — add `centered_fixed(width, height, area)` (fixed-size centered/clamped rect) + unit test, alongside the existing percentage-based `centered_rect`.
- `src/picker.rs` — rewrite the `render()` function to draw the backdrop + modal; add a `TestBackend` render test. Event loop, `PickerState`, `ProjectForm`, and existing form-logic tests are unchanged.

---

## Task 1: `Theme::backdrop()` color helper

**Files:**
- Modify: `src/theme.rs` (add a method in the `impl Theme` block near `accent()`, ~line 55; add a test in the `tests` module, ~line 186)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/theme.rs`:

```rust
    #[test]
    fn backdrop_darkens_base_and_is_black_without_one() {
        // Catppuccin base 0x1e,0x1e,0x2e darkened by ~0.6.
        assert_eq!(catppuccin().backdrop(), Color::Rgb(0x12, 0x12, 0x1b));
        // The terminal theme forces no background, so the backdrop is black.
        assert_eq!(default_ansi().backdrop(), Color::Black);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamaji backdrop_darkens_base 2>&1 | tail -20`
(If the package name differs, use `cargo test backdrop_darkens_base`.)
Expected: FAIL — `no method named backdrop found for struct Theme`.

- [ ] **Step 3: Write minimal implementation**

Add this method inside `impl Theme` in `src/theme.rs`, right after the `accent()` method (after line 57):

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test backdrop_darkens_base 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/theme.rs
git commit -m "feat(theme): add backdrop() color for modal dimming"
```

---

## Task 2: `centered_fixed()` layout helper

**Files:**
- Modify: `src/ui/mod.rs` (add the function after `centered_rect`, ~line 52; add a test in the `tests` module, ~line 54)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/ui/mod.rs`:

```rust
    #[test]
    fn centered_fixed_centers_and_clamps() {
        let area = Rect::new(0, 0, 100, 40);
        // 52x12 centered in 100x40 -> x=(100-52)/2=24, y=(40-12)/2=14.
        assert_eq!(centered_fixed(52, 12, area), Rect::new(24, 14, 52, 12));
        // Requested size larger than the area is clamped to the area.
        assert_eq!(centered_fixed(200, 80, area), Rect::new(0, 0, 100, 40));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test centered_fixed_centers_and_clamps 2>&1 | tail -20`
Expected: FAIL — `cannot find function centered_fixed in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add this function in `src/ui/mod.rs` immediately after `centered_rect` (after line 52). It reuses the already-imported `Constraint`, `Flex`, `Layout`, `Rect`:

```rust
/// A centered rect of fixed `width` x `height`, clamped to fit `area`.
pub fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let [area] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(w)])
        .flex(Flex::Center)
        .areas(area);
    area
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test centered_fixed_centers_and_clamps 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat(ui): add centered_fixed layout helper"
```

---

## Task 3: Render the picker as a centered modal

**Files:**
- Modify: `src/picker.rs` — imports (lines 1-8), the `render()` function (lines 156-222), and the `tests` module (add a render test).

- [ ] **Step 1: Update imports**

In `src/picker.rs`, replace the widget and add the text import. Change line 5:

```rust
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};
```

And add, after the existing `use ratatui::style::...` line (line 4):

```rust
use ratatui::text::{Line, Span};
```

- [ ] **Step 2: Write the failing render test**

Add to the `#[cfg(test)] mod tests` block in `src/picker.rs`:

```rust
    #[test]
    fn picker_renders_as_centered_modal() {
        use ratatui::backend::TestBackend;
        use ratatui::layout::Position;
        use ratatui::Terminal;
        use std::path::PathBuf;

        let theme = Theme::by_name("catppuccin");
        let state = PickerState {
            projects: vec![Project {
                id: 1,
                name: "kamaji".into(),
                root_dir: PathBuf::from("/home/u/dev/kamaji"),
                default_agent: None,
                created_at: String::new(),
            }],
            selected: 0,
            form: None,
            theme,
        };

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        // (a) The modal frame is drawn in the theme's border color.
        let border_found = (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| buf[Position::new(x, y)].fg == theme.border)
        });
        assert!(border_found, "modal frame should use theme.border");

        // (b) The top-left corner lies outside the centered modal and carries
        // the dimmed backdrop — proving it is a modal, not full-screen.
        assert_eq!(buf[Position::new(0, 0)].bg, theme.backdrop());
    }
```

The test references `Project`, `PickerState`, `Theme`, and `render` — all already in scope in `picker.rs` (`Project` and `Theme` via the top-of-file `use`, `PickerState`/`render` via `use super::*`).

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test picker_renders_as_centered_modal 2>&1 | tail -30`
Expected: FAIL — the current full-screen `render()` paints no backdrop, so `buf[(0,0)].bg` is not `theme.backdrop()` (assertion (b) fails).

- [ ] **Step 4: Rewrite `render()`**

Replace the entire `render()` function (lines 156-222) in `src/picker.rs` with:

```rust
/// Visible project rows before the list starts scrolling.
const MAX_VISIBLE_ROWS: usize = 12;
/// Fixed modal width in columns.
const MODAL_WIDTH: u16 = 52;

fn render(frame: &mut Frame, state: &PickerState) {
    let theme = &state.theme;

    // 1. Dimmed backdrop over the whole screen so the modal reads as elevated.
    frame.render_widget(
        Block::default().style(Style::new().bg(theme.backdrop())),
        frame.area(),
    );

    // 2. Centered, fixed-size, content-aware modal box.
    //    height = border(2) + subtitle(1) + blank(1) + rows + blank(1) + hint(1)
    let rows = state.projects.len().clamp(1, MAX_VISIBLE_ROWS) as u16;
    let area = crate::ui::centered_fixed(MODAL_WIDTH, rows + 6, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .title(" kamaji ")
        .style(Style::new().bg(theme.base.unwrap_or(Color::Reset)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // 3. Inner layout: subtitle, blank, list, blank, hint.
    let [subtitle_area, _, list_area, _, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new("Select a project").style(Style::new().fg(theme.muted)),
        subtitle_area,
    );

    if state.projects.is_empty() {
        frame.render_widget(
            Paragraph::new("No projects yet — press n to create one.")
                .style(Style::new().fg(theme.muted)),
            list_area,
        );
    } else {
        let items: Vec<ListItem> = state
            .projects
            .iter()
            .map(|p| {
                ListItem::new(Line::from(vec![
                    Span::styled(p.name.clone(), Style::new().fg(theme.text)),
                    Span::raw("  "),
                    Span::styled(
                        p.root_dir.display().to_string(),
                        Style::new().fg(theme.muted),
                    ),
                ]))
            })
            .collect();
        let mut list_state = ListState::default();
        list_state.select(Some(state.selected));
        let list = List::new(items).highlight_symbol("› ").highlight_style(
            Style::new()
                .fg(theme.base.unwrap_or(Color::Black))
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, list_area, &mut list_state);
    }

    frame.render_widget(
        Paragraph::new("↑/↓ select · ↵ open · n new · q quit")
            .style(Style::new().fg(theme.muted)),
        hint_area,
    );

    // 4. The new-project form overlays everything when open.
    if let Some(form) = &state.form {
        crate::ui::render_field_modal(
            frame,
            &state.theme,
            "New project",
            &[
                ("Name", &form.name, form.field == ProjectField::Name),
                (
                    "Root directory (~ ok)",
                    &form.root,
                    form.field == ProjectField::Root,
                ),
            ],
            "Tab/Shift-Tab: field   Enter: create   Esc: cancel",
            form.error.as_deref(),
        );
    }
}
```

- [ ] **Step 5: Run the render test to verify it passes**

Run: `cargo test picker_renders_as_centered_modal 2>&1 | tail -30`
Expected: PASS.

- [ ] **Step 6: Run the full picker test module + clippy + fmt**

Run: `cargo test picker 2>&1 | tail -20`
Expected: PASS (all existing form-logic tests plus the new render test).

Run: `cargo clippy --all-targets 2>&1 | tail -20`
Expected: no warnings introduced by these changes.

Run: `cargo fmt`
Expected: no diff, or only formatting of the new code.

- [ ] **Step 7: Commit**

```bash
git add src/picker.rs
git commit -m "feat(ui): render project picker as a centered modal"
```

---

## Task 4: Manual verification

**Files:** none (manual run).

- [ ] **Step 1: Build and run**

Run: `cargo run 2>&1 | tail -20` (or the project's normal launch command).
Expected: the picker appears as a centered, rounded box titled ` kamaji ` on a dimmed background, with `Select a project`, the project list (selected row marked `› ` with an accent highlight, paths dimmed), and the `↑/↓ select · ↵ open · n new · q quit` hint. `↑/↓` move the selection, `n` opens the new-project form over the dimmed backdrop, `Enter` opens a project, `q` quits.

- [ ] **Step 2: Check a couple of themes**

Switch the configured theme to `default` (terminal) and to `nord`, relaunch, and confirm the backdrop dims appropriately and the modal stays readable in each.

---

## Self-Review Notes

- **Spec coverage:** layout (Task 3 render), fixed content-aware sizing (`MAX_VISIBLE_ROWS`, `MODAL_WIDTH`, `centered_fixed` — Tasks 2 & 3), dimmed backdrop (Task 1 + Task 3 step 4), empty state (Task 3 render), unchanged event loop / form (Task 3 keeps `run()` and re-uses `render_field_modal`), and all three test types (backdrop unit, `centered_fixed` unit, picker render). Covered.
- **Type consistency:** `centered_fixed(width, height, area)`, `Theme::backdrop()`, and `MAX_VISIBLE_ROWS`/`MODAL_WIDTH` are named identically everywhere they appear.
- **No placeholders:** every code step shows complete code and exact commands.
