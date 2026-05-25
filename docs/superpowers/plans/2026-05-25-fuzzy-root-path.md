# Fuzzy Root-Path Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add live shell-style path-segment completion to the Root directory field of the new-project modal, so typing a path shows fuzzy-matching subdirectories the user can navigate and accept.

**Architecture:** Pure helper functions (`split_root`, `fuzzy_subsequence`, `dir_suggestions`) do the matching/filesystem work and are unit-tested in isolation. `ProjectForm` in `src/picker.rs` holds suggestion state and gains methods (`refresh_suggestions`, `accept_suggestion`, `move_suggestion`) that drive it. The event loop wires `↑/↓/Tab` to those methods on the Root field. `render_field_modal` in `src/ui/modals.rs` is extended to draw an optional suggestion list.

**Tech Stack:** Rust, ratatui 0.29, `directories` crate (already used by `shellexpand`), `tempfile` (dev-dep) for filesystem tests.

---

## File Structure

- `src/picker.rs` — Modify. Add pure helpers (`split_root`, `fuzzy_subsequence`, `dir_suggestions`) next to `shellexpand`; add `suggestions`/`suggestion_idx` fields and methods to `ProjectForm`; wire keys in the event loop; pass suggestions to the renderer. All new unit tests go in its existing `#[cfg(test)] mod tests`.
- `src/ui/modals.rs` — Modify. Extend `render_field_modal` to accept a suggestion slice + selected index and draw a highlighted list.

No new files; no new dependencies.

---

## Task 1: Pure helper — `split_root`

**Files:**
- Modify: `src/picker.rs` (add function after `shellexpand`, ~line 155)
- Test: `src/picker.rs` (in `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/picker.rs`:

```rust
#[test]
fn split_root_splits_at_last_slash() {
    assert_eq!(split_root("~/dev/kam"), ("~/dev/", "kam"));
    assert_eq!(split_root("~/dev/"), ("~/dev/", ""));
    assert_eq!(split_root("/abs/path/to/x"), ("/abs/path/to/", "x"));
}

#[test]
fn split_root_with_no_slash_has_empty_parent() {
    assert_eq!(split_root("kam"), ("", "kam"));
    assert_eq!(split_root(""), ("", ""));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji split_root`
Expected: FAIL — `cannot find function split_root in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add after `shellexpand` in `src/picker.rs`:

```rust
/// Split a raw path string at its last `/` into `(parent, partial)`.
/// `parent` keeps its trailing slash (or is empty when there is no slash);
/// `partial` is the in-progress final segment.
fn split_root(raw: &str) -> (&str, &str) {
    match raw.rfind('/') {
        Some(i) => (&raw[..=i], &raw[i + 1..]),
        None => ("", raw),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji split_root`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): add split_root path-segment helper"
```

---

## Task 2: Pure helper — `fuzzy_subsequence`

**Files:**
- Modify: `src/picker.rs` (add function after `split_root`)
- Test: `src/picker.rs` (in `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
#[test]
fn fuzzy_subsequence_matches_in_order() {
    assert!(fuzzy_subsequence("km", "kamaji"));
    assert!(fuzzy_subsequence("kam", "kamaji"));
    assert!(!fuzzy_subsequence("mk", "kamaji")); // wrong order
    assert!(!fuzzy_subsequence("xyz", "kamaji"));
}

#[test]
fn fuzzy_subsequence_is_case_insensitive() {
    assert!(fuzzy_subsequence("KM", "kamaji"));
    assert!(fuzzy_subsequence("km", "KamAji"));
}

#[test]
fn fuzzy_subsequence_empty_partial_matches_everything() {
    assert!(fuzzy_subsequence("", "anything"));
    assert!(fuzzy_subsequence("", ""));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji fuzzy_subsequence`
Expected: FAIL — `cannot find function fuzzy_subsequence in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add after `split_root`:

```rust
/// Case-insensitive subsequence test: are all chars of `partial` found in
/// `candidate` in order (not necessarily contiguous)? Empty `partial` matches.
fn fuzzy_subsequence(partial: &str, candidate: &str) -> bool {
    let mut cand = candidate.chars().flat_map(char::to_lowercase);
    'outer: for pc in partial.chars().flat_map(char::to_lowercase) {
        for cc in cand.by_ref() {
            if cc == pc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji fuzzy_subsequence`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): add case-insensitive fuzzy subsequence matcher"
```

---

## Task 3: Pure helper — `dir_suggestions`

**Files:**
- Modify: `src/picker.rs` (add function after `fuzzy_subsequence`; add `use std::path::Path;`)
- Test: `src/picker.rs` (in `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
#[test]
fn dir_suggestions_returns_only_matching_subdirs_sorted() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    std::fs::create_dir(base.join("kamaji")).unwrap();
    std::fs::create_dir(base.join("kafka")).unwrap();
    std::fs::create_dir(base.join("zzz")).unwrap();
    std::fs::write(base.join("kamfile.txt"), b"x").unwrap(); // a file, must be excluded

    // partial "ka" matches the two k-dirs (prefix matches first, alphabetical)
    let got = dir_suggestions(base, "ka");
    assert_eq!(got, vec!["kafka".to_string(), "kamaji".to_string()]);

    // empty partial lists all subdirs, prefix group is empty so plain alphabetical
    let all = dir_suggestions(base, "");
    assert_eq!(
        all,
        vec!["kafka".to_string(), "kamaji".to_string(), "zzz".to_string()]
    );
}

#[test]
fn dir_suggestions_orders_prefix_matches_first() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    std::fs::create_dir(base.join("alpha")).unwrap();   // subsequence "a..a" matches "ba"? no
    std::fs::create_dir(base.join("banana")).unwrap();  // contains "a","n"
    std::fs::create_dir(base.join("ant")).unwrap();     // prefix "an"
    // partial "an": "ant" is a prefix match, "banana" only a subsequence match.
    let got = dir_suggestions(base, "an");
    assert_eq!(got, vec!["ant".to_string(), "banana".to_string()]);
}

#[test]
fn dir_suggestions_nonexistent_parent_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist");
    assert!(dir_suggestions(&missing, "x").is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji dir_suggestions`
Expected: FAIL — `cannot find function dir_suggestions in this scope`.

- [ ] **Step 3: Write minimal implementation**

Ensure the import at the top of `src/picker.rs` covers `Path`. The file currently has `use std::path::PathBuf;` — change it to:

```rust
use std::path::{Path, PathBuf};
```

Add after `fuzzy_subsequence`:

```rust
/// List subdirectory names of `parent` whose name fuzzy-matches `partial`.
/// Names that start with `partial` (case-insensitive) sort first; the rest
/// follow, each group alphabetical (case-insensitive). A parent that cannot be
/// read yields an empty list.
fn dir_suggestions(parent: &Path, partial: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let lower_partial = partial.to_lowercase();
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| fuzzy_subsequence(partial, name))
        .collect();
    names.sort_by(|a, b| {
        let a_pref = a.to_lowercase().starts_with(&lower_partial);
        let b_pref = b.to_lowercase().starts_with(&lower_partial);
        b_pref
            .cmp(&a_pref)
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });
    names
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji dir_suggestions`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): add dir_suggestions directory matcher"
```

---

## Task 4: `ProjectForm` suggestion state + `refresh_suggestions`

**Files:**
- Modify: `src/picker.rs` — `ProjectForm` struct (lines 23-28), `ProjectForm::new` (lines 31-38), add method.
- Test: `src/picker.rs` (in `mod tests`)

- [ ] **Step 1: Write the failing test**

Add to `mod tests`:

```rust
#[test]
fn refresh_suggestions_lists_subdirs_of_parent() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("kamaji")).unwrap();
    std::fs::create_dir(tmp.path().join("other")).unwrap();

    let mut form = ProjectForm::new();
    form.field = ProjectField::Root;
    // Type an absolute parent path with partial "kam".
    form.root = format!("{}/kam", tmp.path().display());
    form.refresh_suggestions();

    assert_eq!(form.suggestions, vec!["kamaji".to_string()]);
    assert_eq!(form.suggestion_idx, 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamaji refresh_suggestions`
Expected: FAIL — no field `suggestions` / no method `refresh_suggestions`.

- [ ] **Step 3: Write minimal implementation**

Update the struct (lines 23-28) to:

```rust
struct ProjectForm {
    name: String,
    root: String,
    field: ProjectField,
    error: Option<String>,
    /// Subdirectory names matching the current Root field segment.
    suggestions: Vec<String>,
    /// Highlighted entry in `suggestions`.
    suggestion_idx: usize,
}
```

Update `ProjectForm::new` (lines 31-38) to initialize the new fields:

```rust
fn new() -> Self {
    ProjectForm {
        name: String::new(),
        root: String::new(),
        field: ProjectField::Name,
        error: None,
        suggestions: Vec::new(),
        suggestion_idx: 0,
    }
}
```

Add this method inside `impl ProjectForm` (e.g. after `resolved_root`):

```rust
/// Recompute suggestions for the Root field from its current text, expanding a
/// leading `~` only to read the filesystem. Resets the highlight to the top.
fn refresh_suggestions(&mut self) {
    let (parent, partial) = split_root(&self.root);
    let parent_expanded = if parent.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(shellexpand(parent))
    };
    self.suggestions = dir_suggestions(&parent_expanded, partial);
    self.suggestion_idx = 0;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamaji refresh_suggestions`
Expected: PASS.

- [ ] **Step 5: Run the full picker test module to confirm nothing regressed**

Run: `cargo test -p kamaji --lib picker`
Expected: PASS (existing form tests still compile/pass; `ProjectForm` literals in older tests use `new()` so they are unaffected).

- [ ] **Step 6: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): add suggestion state and refresh_suggestions"
```

---

## Task 5: `move_suggestion` and `accept_suggestion`

**Files:**
- Modify: `src/picker.rs` — add two methods to `impl ProjectForm`.
- Test: `src/picker.rs` (in `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
#[test]
fn move_suggestion_clamps_at_both_ends() {
    let mut form = ProjectForm::new();
    form.suggestions = vec!["a".into(), "b".into(), "c".into()];
    form.suggestion_idx = 0;

    form.move_suggestion(-1); // already at top
    assert_eq!(form.suggestion_idx, 0);

    form.move_suggestion(1);
    form.move_suggestion(1);
    assert_eq!(form.suggestion_idx, 2);

    form.move_suggestion(1); // already at bottom
    assert_eq!(form.suggestion_idx, 2);
}

#[test]
fn accept_suggestion_replaces_partial_and_appends_slash() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("kamaji")).unwrap();

    let mut form = ProjectForm::new();
    form.field = ProjectField::Root;
    form.root = format!("{}/kam", tmp.path().display());
    form.refresh_suggestions();
    assert_eq!(form.suggestions, vec!["kamaji".to_string()]);

    form.accept_suggestion();
    assert_eq!(form.root, format!("{}/kamaji/", tmp.path().display()));
}

#[test]
fn accept_suggestion_preserves_tilde_parent() {
    // No filesystem read needed: drive the suggestion list directly.
    let mut form = ProjectForm::new();
    form.field = ProjectField::Root;
    form.root = "~/dev/kam".into();
    form.suggestions = vec!["kamaji".into()];
    form.suggestion_idx = 0;

    form.accept_suggestion();
    // Parent text (including ~/) is preserved; partial replaced + trailing slash.
    assert!(form.root.starts_with("~/dev/kamaji/"));
}

#[test]
fn accept_suggestion_with_empty_list_is_noop() {
    let mut form = ProjectForm::new();
    form.field = ProjectField::Root;
    form.root = "~/dev/".into();
    form.suggestions.clear();
    form.accept_suggestion();
    assert_eq!(form.root, "~/dev/");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji suggestion`
Expected: FAIL — no method `move_suggestion` / `accept_suggestion`.

- [ ] **Step 3: Write minimal implementation**

Add to `impl ProjectForm` (after `refresh_suggestions`):

```rust
/// Move the suggestion highlight by `delta`, clamped to the list bounds.
fn move_suggestion(&mut self, delta: isize) {
    if self.suggestions.is_empty() {
        return;
    }
    let max = self.suggestions.len() as isize - 1;
    let next = (self.suggestion_idx as isize + delta).clamp(0, max);
    self.suggestion_idx = next as usize;
}

/// Accept the highlighted suggestion: replace the in-progress segment with the
/// chosen directory name plus a trailing `/`, keeping the literal parent text
/// (e.g. a `~/` prefix). Then refresh suggestions for the new level.
fn accept_suggestion(&mut self) {
    let Some(name) = self.suggestions.get(self.suggestion_idx).cloned() else {
        return;
    };
    let (parent, _partial) = split_root(&self.root);
    self.root = format!("{parent}{name}/");
    self.refresh_suggestions();
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji suggestion`
Expected: PASS (move + accept tests).

- [ ] **Step 5: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): add move_suggestion and accept_suggestion"
```

---

## Task 6: Extend `render_field_modal` to draw suggestions

**Files:**
- Modify: `src/ui/modals.rs` — `render_field_modal` signature + body (lines 39-69).
- Modify: `src/picker.rs` — its one call to `render_field_modal` (lines 245-259).
- Test: `src/ui/modals.rs` (in `mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` in `src/ui/modals.rs`:

```rust
#[test]
fn field_modal_draws_suggestions() {
    let theme = Theme::by_name("catppuccin");
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let suggestions = ["kamaji".to_string(), "kafka".to_string()];
    terminal
        .draw(|f| {
            render_field_modal(
                f,
                &theme,
                "New project",
                &[("Name", "x", false), ("Root", "~/dev/kam", true)],
                "hint",
                None,
                &suggestions,
                0,
            )
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[Position::new(x, y)].symbol());
        }
    }
    assert!(text.contains("kamaji"), "suggestion list should render:\n{text}");
    assert!(text.contains("kafka"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamaji field_modal_draws_suggestions`
Expected: FAIL — `render_field_modal` takes 6 args, not 8 (compile error).

- [ ] **Step 3: Update `render_field_modal`**

Replace the signature and body in `src/ui/modals.rs` (lines 39-69). New signature adds `suggestions` and `selected`; the body appends a highlighted suggestion list when `suggestions` is non-empty:

```rust
pub(crate) fn render_field_modal(
    frame: &mut Frame,
    theme: &Theme,
    title: &str,
    fields: &[(&str, &str, bool)],
    hint: &str,
    error: Option<&str>,
    suggestions: &[String],
    selected: usize,
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

    if !suggestions.is_empty() {
        lines.push(Line::raw(""));
        for (i, name) in suggestions.iter().enumerate() {
            let style = if i == selected {
                Style::new()
                    .fg(theme.base.unwrap_or(Color::Black))
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme.text)
            };
            let marker = if i == selected { "› " } else { "  " };
            lines.push(Line::styled(format!("{marker}{name}"), style));
        }
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

(`Color`, `Modifier`, `Style` are already imported at the top of `modals.rs`.)

- [ ] **Step 4: Update the caller in `src/picker.rs`**

Replace the `render_field_modal` call (lines 245-259) so it passes a Root-field hint, the suggestions, and the selected index. Suggestions are only shown when the Root field is active:

```rust
    if let Some(form) = &state.form {
        let on_root = form.field == ProjectField::Root;
        let hint = if on_root {
            "↑/↓ choose · Tab complete · ↵ create · Esc cancel"
        } else {
            "Tab/Shift-Tab: field   Enter: create   Esc: cancel"
        };
        let suggestions: &[String] = if on_root { &form.suggestions } else { &[] };
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
            hint,
            form.error.as_deref(),
            suggestions,
            form.suggestion_idx,
        );
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p kamaji field_modal_draws_suggestions && cargo build`
Expected: test PASSES and the crate builds (caller now matches the new signature).

- [ ] **Step 6: Commit**

```bash
git add src/ui/modals.rs src/picker.rs
git commit -m "feat(ui): render root-directory suggestion list in field modal"
```

---

## Task 7: Wire keys in the picker event loop

**Files:**
- Modify: `src/picker.rs` — the `Some(form)` match arm of the event loop (lines 122-142) and the `KeyCode::Char('n')` arm (lines 106-108).
- Test: manual (TUI event loop is not unit-tested in this repo; logic is covered by Tasks 1-5).

- [ ] **Step 1: Make suggestions appear immediately when entering the Root field via Tab/BackTab and on open**

The Name→Root transition and field toggles must refresh suggestions so the list reflects the (possibly empty) Root text. Update the `Some(form)` arm (lines 122-142). Replace it with:

```rust
            Some(form) => match key.code {
                KeyCode::Esc => state.form = None,
                KeyCode::Tab => {
                    if form.field == ProjectField::Root && !form.suggestions.is_empty() {
                        // On Root with matches, Tab completes the highlighted entry.
                        form.accept_suggestion();
                    } else {
                        form.next_field();
                        form.refresh_suggestions();
                    }
                }
                KeyCode::BackTab => {
                    form.prev_field();
                    form.refresh_suggestions();
                }
                KeyCode::Up if form.field == ProjectField::Root => form.move_suggestion(-1),
                KeyCode::Down if form.field == ProjectField::Root => form.move_suggestion(1),
                KeyCode::Enter => {
                    if form.name.trim().is_empty() {
                        form.error = Some("Name is required".into());
                    } else {
                        let root = form.resolved_root();
                        if !root.is_dir() {
                            form.error = Some(format!("Not a directory: {}", root.display()));
                        } else {
                            let project = db.create_project(form.name.trim(), &root, None)?;
                            return Ok(Some(project));
                        }
                    }
                }
                KeyCode::Backspace => {
                    form.backspace();
                    if form.field == ProjectField::Root {
                        form.refresh_suggestions();
                    }
                }
                KeyCode::Char(c) => {
                    form.input_char(c);
                    if form.field == ProjectField::Root {
                        form.refresh_suggestions();
                    }
                }
                _ => {}
            },
```

- [ ] **Step 2: Verify the crate builds**

Run: `cargo build`
Expected: builds with no errors.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test`
Expected: PASS — all existing and new tests green.

- [ ] **Step 4: Lint and format**

Run: `cargo clippy --all-targets && cargo fmt`
Expected: clippy clean (no warnings on the new code); `cargo fmt` leaves no diff after staging.

- [ ] **Step 5: Manual smoke test (interactive)**

Run the app, open the project picker, press `n`, Tab to the Root field, and type a partial path (e.g. `~/dev/`). Confirm:
- a suggestion list of subdirectories appears,
- typing narrows it (fuzzy, case-insensitive),
- `↑/↓` move the highlight,
- `Tab` fills the highlighted dir and appends `/`, refreshing the list,
- `Shift-Tab` returns to the Name field,
- `Enter` on a valid directory creates the project.

- [ ] **Step 6: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): wire arrow/Tab keys for root-path completion"
```

---

## Self-Review Notes

- **Spec coverage:** split/expand (Task 1, 4), fuzzy matching (Task 2), dir listing + sort (Task 3), state + refresh (Task 4), accept + move (Task 5), rendering (Task 6), key bindings incl. Name-field Tab unchanged and empty-list Tab no-op (Task 7). Out-of-scope items are not implemented.
- **Type consistency:** method names `refresh_suggestions`, `accept_suggestion`, `move_suggestion`, fields `suggestions: Vec<String>` / `suggestion_idx: usize`, and the 8-arg `render_field_modal` signature are used identically across tasks.
- **Note on `Tab` field nav:** once on the Root field with a non-empty suggestion list, `Tab` completes rather than moving fields; `Shift-Tab` (BackTab) is the documented way back to Name. This matches the spec.
