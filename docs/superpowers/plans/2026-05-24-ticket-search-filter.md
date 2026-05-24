# Ticket Search / Filter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an in-place search that filters tickets by title across all four Kanban columns, leaving the board fully interactive while a filter is applied.

**Architecture:** Search state lives on `App` (not the `Modal` enum). `App::column_tickets` becomes the single source of truth and applies a case-insensitive substring predicate, so navigation, selection, and rendering all operate on the visible set automatically. `Engine::on_board_key` gets an input-capture branch that runs before the normal hotkeys while the query is being edited. Detection (`gather_levels`/`reconcile`) keeps iterating `app.tickets` directly and is unaffected.

**Tech Stack:** Rust, ratatui, crossterm. Tests are inline `#[cfg(test)] mod tests` blocks per module, run with `cargo test`.

**Spec:** `docs/superpowers/specs/2026-05-24-ticket-search-filter-design.md`

---

### Task 1: `Search` struct and title predicate

**Files:**
- Modify: `src/app.rs` (add struct after the imports / before `FormField`, around line 2)
- Test: `src/app.rs` (inline `tests` module)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module at the bottom of `src/app.rs`:

```rust
#[test]
fn search_matches_is_case_insensitive_substring() {
    let mut t = ticket(1, Status::Todo);
    t.title = "Add Login".into();

    let mut s = Search::default();
    assert!(s.matches(&t), "an empty query matches everything");

    s.query = "login".into();
    assert!(s.matches(&t), "case-insensitive substring matches");

    s.query = "LOG".into();
    assert!(s.matches(&t));

    s.query = "logout".into();
    assert!(!s.matches(&t), "a non-substring does not match");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib search_matches_is_case_insensitive_substring`
Expected: FAIL — `cannot find type Search in this scope`.

- [ ] **Step 3: Write minimal implementation**

In `src/app.rs`, just below the `use` line (line 1) and before `FormField`, add:

```rust
/// Board search/filter state. An empty query means no filter is applied.
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// The current query text.
    pub query: String,
    /// True while the user is typing the query (board input is captured by
    /// search instead of the normal hotkeys).
    pub editing: bool,
}

impl Search {
    /// True when no query is set (the board shows every ticket).
    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    /// Case-insensitive substring match against the ticket title. An empty
    /// query matches every ticket.
    pub fn matches(&self, ticket: &Ticket) -> bool {
        if self.query.is_empty() {
            return true;
        }
        ticket
            .title
            .to_lowercase()
            .contains(&self.query.to_lowercase())
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib search_matches_is_case_insensitive_substring`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(search): add Search struct with title predicate"
```

---

### Task 2: Wire search into `App` (field + filtered `column_tickets` + total)

**Files:**
- Modify: `src/app.rs` — `App` struct (around line 128), `App::new` (around line 139), `column_tickets` (line 155)
- Test: `src/app.rs` (inline `tests` module)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/app.rs`:

```rust
#[test]
fn column_tickets_filters_by_search_and_total_ignores_it() {
    let mut t1 = ticket(1, Status::Todo);
    t1.title = "Add login".into();
    let mut t2 = ticket(2, Status::Todo);
    t2.title = "Fix logout bug".into();
    let mut t3 = ticket(3, Status::Todo);
    t3.title = "Update README".into();
    let mut app = App::new(project(), vec![t1, t2, t3]);

    app.search.query = "log".into();
    let matches = app.column_tickets(Status::Todo);
    assert_eq!(matches.len(), 2, "login + logout match 'log'");
    assert_eq!(
        app.column_total(Status::Todo),
        3,
        "the unfiltered total ignores the search query"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib column_tickets_filters_by_search_and_total_ignores_it`
Expected: FAIL — `no field search on type &App` and `no method named column_total`.

- [ ] **Step 3: Write minimal implementation**

In `src/app.rs`, add a field to the `App` struct (after `pub status_message: Option<String>,` at line 134):

```rust
    pub search: Search,
```

In `App::new`, add to the constructed `App` (after `status_message: None,` at line 146):

```rust
            search: Search::default(),
```

Replace `column_tickets` (lines 155-157) with the filtered version, and add `column_total` right after it:

```rust
    pub fn column_tickets(&self, status: Status) -> Vec<&Ticket> {
        self.tickets
            .iter()
            .filter(|t| t.status == status && self.search.matches(t))
            .collect()
    }

    /// Count of all tickets in a column, ignoring the active search filter
    /// (used to render the `matches/total` count in the column title).
    pub fn column_total(&self, status: Status) -> usize {
        self.tickets.iter().filter(|t| t.status == status).count()
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib column_tickets_filters_by_search_and_total_ignores_it`
Expected: PASS.

- [ ] **Step 5: Run the full app test suite to confirm no regressions**

Run: `cargo test --lib`
Expected: PASS (existing navigation tests still pass — with an empty query `column_tickets` is unchanged).

- [ ] **Step 6: Commit**

```bash
git add src/app.rs
git commit -m "feat(search): filter column_tickets by query, add column_total"
```

---

### Task 3: Search editing methods on `App` (with cursor reclamp)

**Files:**
- Modify: `src/app.rs` — add methods in the `impl App` block (after `reclamp`, around line 204)
- Test: `src/app.rs` (inline `tests` module)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/app.rs`:

```rust
#[test]
fn narrowing_search_reclamps_selected_row() {
    let mut t1 = ticket(1, Status::Todo);
    t1.title = "alpha".into();
    let mut t2 = ticket(2, Status::Todo);
    t2.title = "alpine".into();
    let mut t3 = ticket(3, Status::Todo);
    t3.title = "beta".into();
    let mut app = App::new(project(), vec![t1, t2, t3]);
    app.selected_row = 2; // "beta"

    app.search_start();
    assert!(app.search.editing);
    for c in "alp".chars() {
        app.search_push(c);
    }
    // Only alpha + alpine remain (2 cards); the cursor must clamp into range.
    assert_eq!(app.column_tickets(Status::Todo).len(), 2);
    assert!(app.selected_row <= 1, "row clamped into the filtered range");
    assert!(app.selected_ticket().is_some());

    // Backspace re-widens the filter and stays valid.
    app.search_backspace();
    assert_eq!(app.search.query, "al");

    // Esc clears the query and exits editing.
    app.search_clear();
    assert!(app.search.is_empty());
    assert!(!app.search.editing);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib narrowing_search_reclamps_selected_row`
Expected: FAIL — `no method named search_start`.

- [ ] **Step 3: Write minimal implementation**

In `src/app.rs`, inside `impl App`, add after `reclamp` (which ends around line 204, before the closing `}` of the impl):

```rust
    /// Begin editing the search query (keeps any existing query so `/` re-edits).
    pub fn search_start(&mut self) {
        self.search.editing = true;
    }

    /// Append a character to the query and re-clamp the cursor to the now-
    /// filtered column.
    pub fn search_push(&mut self, c: char) {
        self.search.query.push(c);
        self.clamp_row();
    }

    /// Delete the last query character and re-clamp the cursor.
    pub fn search_backspace(&mut self) {
        self.search.query.pop();
        self.clamp_row();
    }

    /// Commit the current filter: stop editing but keep the query applied.
    pub fn search_commit(&mut self) {
        self.search.editing = false;
    }

    /// Clear the filter entirely and exit editing.
    pub fn search_clear(&mut self) {
        self.search.query.clear();
        self.search.editing = false;
        self.clamp_row();
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib narrowing_search_reclamps_selected_row`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(search): add search editing methods with cursor reclamp"
```

---

### Task 4: Engine key handling for search

**Files:**
- Modify: `src/engine.rs` — `on_board_key` (starts line 515)
- Test: `src/engine.rs` (inline `tests` module)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/engine.rs` (the `key`, `enter`, and `engine_with_project` helpers already exist; `KeyCode`/`KeyEvent`/`KeyModifiers` are already imported):

```rust
fn esc() -> KeyEvent {
    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
}

#[test]
fn slash_enters_search_and_typing_does_not_trigger_hotkeys() {
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    e.on_key(key('/')).unwrap();
    assert!(e.app.search.editing, "/ starts search editing");
    // 'q' is captured as query text, not treated as quit.
    e.on_key(key('q')).unwrap();
    assert!(!e.app.should_quit, "q is typed into the query while editing");
    assert_eq!(e.app.search.query, "q");
}

#[test]
fn enter_commits_search_filter() {
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    e.on_key(key('/')).unwrap();
    for c in "log".chars() {
        e.on_key(key(c)).unwrap();
    }
    e.on_key(enter()).unwrap();
    assert!(!e.app.search.editing, "Enter commits and stops editing");
    assert_eq!(e.app.search.query, "log", "the filter persists after commit");
}

#[test]
fn esc_clears_query_while_editing_then_clears_filter_when_applied() {
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    // While editing, Esc clears the query and exits search.
    e.on_key(key('/')).unwrap();
    for c in "log".chars() {
        e.on_key(key(c)).unwrap();
    }
    e.on_key(esc()).unwrap();
    assert!(!e.app.search.editing);
    assert!(e.app.search.query.is_empty());

    // Apply and commit a filter, then Esc on the board clears it.
    e.on_key(key('/')).unwrap();
    e.on_key(key('x')).unwrap();
    e.on_key(enter()).unwrap();
    assert_eq!(e.app.search.query, "x");
    e.on_key(esc()).unwrap();
    assert!(e.app.search.query.is_empty(), "Esc clears the applied filter");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib slash_enters_search_and_typing_does_not_trigger_hotkeys enter_commits_search_filter esc_clears_query_while_editing_then_clears_filter_when_applied`
Expected: FAIL — the `/` key is not handled, so `editing` is never set.

- [ ] **Step 3: Write minimal implementation**

In `src/engine.rs`, in `on_board_key`, insert the editing-capture branch immediately after `self.app.status_message = None;` (line 516) and before the `match key.code {` (line 517):

```rust
        // While editing the search query, capture input before the board
        // hotkeys so typed characters edit the query (and 'q' doesn't quit).
        if self.app.search.editing {
            match key.code {
                KeyCode::Esc => self.app.search_clear(),
                KeyCode::Enter => self.app.search_commit(),
                KeyCode::Backspace => self.app.search_backspace(),
                KeyCode::Char(c) => self.app.search_push(c),
                _ => {}
            }
            return Ok(Effect::None);
        }
```

Then add two arms inside the existing `match key.code` block. Add the `/` arm next to the other character hotkeys (e.g. right after the `KeyCode::Char('q')` arm at line 518), and the board-level `Esc` arm as well:

```rust
            KeyCode::Char('/') => self.app.search_start(),
            KeyCode::Esc if !self.app.search.is_empty() => self.app.search_clear(),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib slash_enters_search_and_typing_does_not_trigger_hotkeys enter_commits_search_filter esc_clears_query_while_editing_then_clears_filter_when_applied`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/engine.rs
git commit -m "feat(search): handle / and search editing keys in on_board_key"
```

---

### Task 5: Regression test — filtering does not stop detection

**Files:**
- Test only: `src/engine.rs` (inline `tests` module). No production code change; this guards the spec's "detection independence" requirement.

- [ ] **Step 1: Write the test**

Add to the `tests` module in `src/engine.rs` (the `in_progress_ticket` and `levels` helpers already exist):

```rust
#[test]
fn filter_does_not_stop_detection() {
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    let id = in_progress_ticket(&mut e); // title "t", In Progress, has a session
    // Apply a filter that hides the in-progress card.
    e.app.search.query = "zzz".into();
    assert!(
        e.app.column_tickets(Status::InProgress).is_empty(),
        "the filter hides the in-progress card"
    );
    // Detection still sees the hidden ticket and auto-moves it on idle.
    e.detect_tick_with(&levels(id, SignalLevel::Active)).unwrap();
    e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
    assert_eq!(
        e.db.get_ticket(id).unwrap().unwrap().status,
        Status::Review,
        "a hidden ticket is still auto-moved to Needs attention"
    );
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test --lib filter_does_not_stop_detection`
Expected: PASS (detection iterates `app.tickets`, not `column_tickets`).

- [ ] **Step 3: Commit**

```bash
git add src/engine.rs
git commit -m "test(search): detection ignores the board filter"
```

---

### Task 6: Board rendering — counts, status-bar prompt, hint

**Files:**
- Modify: `src/ui/board.rs` — `render_board` (line 32), `render_column` (line 65), hints line (line 52)
- Test: `src/ui/board.rs` (inline `tests` module)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/ui/board.rs` (the `project`, `ticket`, `render`, and `buffer_text` helpers already exist):

```rust
#[test]
fn column_title_shows_matches_over_total_when_filtering() {
    let mut app = App::new(project(), vec![ticket(1, Status::Todo), ticket(2, Status::Todo)]);
    // ticket() titles are "title1" / "title2"; "title1" matches only the first.
    app.search.query = "title1".into();
    let buf = render(&app, &HashMap::new(), 80, 20);
    let text = buffer_text(&buf);
    assert!(
        text.contains("Todo (1/2)"),
        "expected matches/total count in title:\n{text}"
    );
}

#[test]
fn status_bar_shows_search_prompt_while_editing() {
    let mut app = App::new(project(), vec![ticket(1, Status::Todo)]);
    app.search.editing = true;
    app.search.query = "lo".into();
    let buf = render(&app, &HashMap::new(), 80, 20);
    let text = buffer_text(&buf);
    assert!(
        text.contains("search: lo"),
        "expected the search prompt in the status bar:\n{text}"
    );
}

#[test]
fn status_bar_lists_the_search_hint() {
    let app = App::new(project(), vec![ticket(1, Status::Todo)]);
    let buf = render(&app, &HashMap::new(), 120, 20);
    let text = buffer_text(&buf);
    assert!(text.contains("[/]search"), "search hint present:\n{text}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib column_title_shows_matches_over_total_when_filtering status_bar_shows_search_prompt_while_editing status_bar_lists_the_search_hint`
Expected: FAIL — title shows `Todo (1)` not `Todo (1/2)`; no `search:` prompt; no `[/]search` hint.

- [ ] **Step 3: Write the implementation**

In `src/ui/board.rs`, change the column loop in `render_board` (lines 38-50) to pass the total and a `filtering` flag:

```rust
    let filtering = !app.search.is_empty();
    for (col_idx, status) in Status::all().into_iter().enumerate() {
        let tickets = app.column_tickets(status);
        let total = app.column_total(status);
        let focused = col_idx == app.selected_col;
        render_column(
            frame,
            columns[col_idx],
            status,
            &tickets,
            total,
            filtering,
            focused,
            app.selected_row,
            levels,
        );
    }
```

Update the `render_column` signature (lines 65-73) to accept the new params (add `total: usize,` and `filtering: bool,` after the `tickets` param):

```rust
fn render_column(
    frame: &mut Frame,
    area: Rect,
    status: Status,
    tickets: &[&Ticket],
    total: usize,
    filtering: bool,
    focused: bool,
    selected_row: usize,
    levels: &HashMap<i64, SignalLevel>,
) {
```

Replace the `block` title construction (lines 80-84) with a matches/total-aware count:

```rust
    let count = if filtering {
        format!("{}/{}", tickets.len(), total)
    } else {
        total.to_string()
    };
    let block = Block::bordered()
        .border_style(border_style)
        .title(format!(" {} ({}) ", status.title(), count));
```

Update the hints line (line 52) to include the search key:

```rust
    let hints =
        " [↵]attach [e]dit [c]reate [m]ove [d]elete [/]search [p]roject [?]help [q]uit";
```

Replace the status-line construction (lines 53-60) to add the search/filter segment between the project label and the message:

```rust
    let left = format!(" project: {} ", app.project.name);
    let search_span = if app.search.editing {
        Span::styled(
            format!("search: {}_ ", app.search.query),
            Style::new().fg(Color::Cyan),
        )
    } else if !app.search.is_empty() {
        Span::styled(
            format!("filter: {} — Esc to clear ", app.search.query),
            Style::new().fg(Color::Cyan),
        )
    } else {
        Span::raw("")
    };
    let msg = app.status_message.clone().unwrap_or_default();
    let status_line = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::new().fg(Color::Yellow)),
        search_span,
        Span::styled(msg, Style::new().fg(Color::Red)),
        Span::raw(hints),
    ]));
    frame.render_widget(status_line, status_area);
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib column_title_shows_matches_over_total_when_filtering status_bar_shows_search_prompt_while_editing status_bar_lists_the_search_hint`
Expected: PASS.

- [ ] **Step 5: Run the full board test suite**

Run: `cargo test --lib board`
Expected: PASS (existing rendering tests still pass; with no filter the title shows `(total)` exactly as before).

- [ ] **Step 6: Commit**

```bash
git add src/ui/board.rs
git commit -m "feat(search): show matches/total count, search prompt, and hint"
```

---

### Task 7: Help modal entry for search

**Files:**
- Modify: `src/ui/modals.rs` — `render_help` text (lines 192-204)
- Test: `src/ui/modals.rs` (new inline `tests` module)

- [ ] **Step 1: Write the failing test**

Add a new `tests` module at the bottom of `src/ui/modals.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::Terminal;

    #[test]
    fn help_lists_the_search_key() {
        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(render_help).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        assert!(text.contains("search"), "help should mention search:\n{text}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib help_lists_the_search_key`
Expected: FAIL — the help text has no `search` line.

- [ ] **Step 3: Write the implementation**

In `src/ui/modals.rs`, in `render_help`, add a `/` line to the help text (insert after the `Enter     attach / start session` line, around line 197):

```rust
    let text = "\
↑/↓ j/k   select ticket
←/→ h/l   change column
c         create ticket (auto-starts a background session)
e         edit ticket
Enter     attach / start session
/         search / filter tickets (Esc clears)
m         move ticket (then ←/→, Enter)
d         delete ticket
p         switch project
?         this help
q         quit

Any key closes this help.";
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib help_lists_the_search_key`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/modals.rs
git commit -m "docs(search): document the / search key in the help modal"
```

---

### Task 8: Final verification

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt`
Expected: no diff (or apply and re-commit if it reformats).

- [ ] **Step 2: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings. (Watch for an unused-import or dead-code warning; if `column_total`/`Search` methods warn as unused, that means a wiring step was missed — fix the call site, don't add `#[allow]`.)

- [ ] **Step 3: Full test suite**

Run: `cargo test`
Expected: all tests PASS.

- [ ] **Step 4: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore(search): cargo fmt" || echo "nothing to commit"
```
