# Auto-Background-Session-On-Create Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a user creates a ticket with the (default-on) "Start in background" toggle, kamaji spins up the agent's Zellij session detached and moves the card to In Progress, without attaching the user.

**Architecture:** Reuse the existing worktree/layout/instrumentation machinery by extracting a `prepare_session` helper from `start_session`. Add a new `Effect::RunSessionBackground` that the main loop performs by shelling out to two non-attaching zellij commands (`attach --create-background` then `action new-tab --layout`) without releasing the TUI terminal. Creation degrades gracefully to a Todo card if no session can be started.

**Tech Stack:** Rust, ratatui (TUI), zellij 0.43.1 (session orchestration), rusqlite (SQLite), anyhow.

---

## File Structure

- `src/app.rs` — `TicketForm` gains a `start_in_background` bool and a `Background` `FormField` variant (create-mode only). Toggle logic.
- `src/ui/modals.rs` — render the toggle in the create form; update the hint line.
- `src/zellij.rs` — new `create_session_background()` wrapper (the two-command detached launch). Thin, untested, like `create_session`.
- `src/engine.rs` — extract `prepare_session()` from `start_session`; `submit_form` returns an `Effect` and starts a background session on create when the toggle is on, with graceful failure. New `Effect::RunSessionBackground` variant.
- `src/main.rs` — handle `Effect::RunSessionBackground` without releasing the terminal.

---

## Task 1: Add the `start_in_background` field + `Background` form field

**Files:**
- Modify: `src/app.rs`
- Test: `src/app.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/app.rs`:

```rust
#[test]
fn create_form_has_background_toggle_on_by_default() {
    let mut f = TicketForm::new_create(Agent::Claude);
    assert!(f.start_in_background, "background toggle defaults on");
    // Background is reachable by tabbing in create mode.
    f.field = FormField::Background;
    f.toggle_background();
    assert!(!f.start_in_background);
    f.toggle_background();
    assert!(f.start_in_background);
}

#[test]
fn edit_form_omits_background_field() {
    let mut f = TicketForm::from_ticket(&ticket(5, Status::Todo));
    // Cycle through every field; Background must never appear in edit mode.
    for _ in 0..4 {
        assert_ne!(f.field, FormField::Background);
        f.next_field();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib app::tests::create_form_has_background_toggle_on_by_default`
Expected: FAIL — `no variant named Background`, `no field start_in_background`, `no method toggle_background`.

- [ ] **Step 3: Write minimal implementation**

In `src/app.rs`, add the enum variant:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Title,
    Description,
    InitialPrompt,
    Agent,
    Background,
}
```

Add the field to the struct:

```rust
#[derive(Debug, Clone)]
pub struct TicketForm {
    pub editing_id: Option<i64>,
    pub title: String,
    pub description: String,
    pub initial_prompt: String,
    pub agent: Agent,
    pub start_in_background: bool,
    pub field: FormField,
}
```

Set it in both constructors:

```rust
    pub fn new_create(default_agent: Agent) -> Self {
        TicketForm {
            editing_id: None,
            title: String::new(),
            description: String::new(),
            initial_prompt: String::new(),
            agent: default_agent,
            start_in_background: true,
            field: FormField::Title,
        }
    }

    pub fn from_ticket(t: &Ticket) -> Self {
        TicketForm {
            editing_id: Some(t.id),
            title: t.title.clone(),
            description: t.description.clone(),
            initial_prompt: t.initial_prompt.clone().unwrap_or_default(),
            agent: t.agent,
            start_in_background: false,
            field: FormField::Title,
        }
    }
```

Add `Background` to the create-mode field list (not edit):

```rust
    fn fields(&self) -> &'static [FormField] {
        if self.editing_id.is_some() {
            &[FormField::Title, FormField::Description]
        } else {
            &[
                FormField::Title,
                FormField::Description,
                FormField::InitialPrompt,
                FormField::Agent,
                FormField::Background,
            ]
        }
    }
```

Add the toggle method (place it near `cycle_agent`):

```rust
    pub fn toggle_background(&mut self) {
        self.start_in_background = !self.start_in_background;
    }
```

Ignore the field in text editing — update the match arms in `input_char` and `backspace`:

```rust
    pub fn input_char(&mut self, c: char) {
        match self.field {
            FormField::Title => self.title.push(c),
            FormField::Description => self.description.push(c),
            FormField::InitialPrompt => self.initial_prompt.push(c),
            FormField::Agent | FormField::Background => {}
        }
    }

    pub fn backspace(&mut self) {
        match self.field {
            FormField::Title => self.title.pop(),
            FormField::Description => self.description.pop(),
            FormField::InitialPrompt => self.initial_prompt.pop(),
            FormField::Agent | FormField::Background => None,
        };
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib app::tests`
Expected: PASS (all app tests, including the two new ones).

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(form): add start-in-background toggle field (create mode)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Render the toggle in the create form

**Files:**
- Modify: `src/ui/modals.rs`

No unit test — this is pure rendering. Verify by `cargo build` + manual read. (The repo's other render functions are likewise not unit-tested.)

- [ ] **Step 1: Add the toggle line in `render_form`**

In `src/ui/modals.rs`, inside the `if form.editing_id.is_none()` block in `render_form`, after the agent line is pushed (after `lines.push(Line::from(agent_line));`), add:

```rust
        lines.push(Line::raw(""));
        let checkbox = if form.start_in_background { "[x]" } else { "[ ]" };
        lines.push(field_line(
            "Start in background",
            checkbox,
            form.field == FormField::Background,
        ));
```

- [ ] **Step 2: Update the hint line**

Still in `render_form`, change the hint line text to mention the toggle:

```rust
    lines.push(Line::styled(
        "Tab/Shift-Tab: field   ←/→: agent / toggle   Enter: save   Esc: cancel",
        Style::new().fg(Color::DarkGray),
    ));
```

- [ ] **Step 3: Build to verify it compiles**

Run: `cargo build`
Expected: builds clean (no errors). `FormField::Background` is in scope via the existing `use crate::app::{FormField, TicketForm};`.

- [ ] **Step 4: Commit**

```bash
git add src/ui/modals.rs
git commit -m "feat(ui): render start-in-background toggle in create form

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Wire toggle key handling (Space / Left / Right on the Background field)

**Files:**
- Modify: `src/engine.rs` (the `Modal::Form` arm of `on_key`)
- Test: `src/engine.rs` (inline tests)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/engine.rs` (helpers `key`, `engine_with_project`, and `KeyEvent`/`KeyCode`/`KeyModifiers` imports already exist):

```rust
#[test]
fn space_toggles_background_field_in_form() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    e.on_key(key('c')).unwrap();
    // Walk to the Background field via Tab (Title→Desc→Prompt→Agent→Background).
    for _ in 0..4 {
        e.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).unwrap();
    }
    match &e.app.modal {
        Modal::Form(f) => assert_eq!(f.field, FormField::Background),
        other => panic!("expected form, got {other:?}"),
    }
    // Space flips it off.
    e.on_key(key(' ')).unwrap();
    match &e.app.modal {
        Modal::Form(f) => assert!(!f.start_in_background),
        other => panic!("expected form, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib engine::tests::space_toggles_background_field_in_form`
Expected: FAIL — Space currently routes to `input_char(' ')`, which is a no-op on the Background field, so `start_in_background` stays `true`.

- [ ] **Step 3: Implement the toggle key handling**

In `src/engine.rs`, in the `Modal::Form(mut form)` arm of `on_key`, add handling for the Background field. Place these arms *before* the generic `KeyCode::Char(c)` arm. Reuse the existing Left/Right arms by extending their guards:

Change the two agent arms from:

```rust
                    KeyCode::Left if form.field == FormField::Agent => {
                        form.cycle_agent(false);
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Right if form.field == FormField::Agent => {
                        form.cycle_agent(true);
                        self.app.modal = Modal::Form(form);
                    }
```

to:

```rust
                    KeyCode::Left if form.field == FormField::Agent => {
                        form.cycle_agent(false);
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Right if form.field == FormField::Agent => {
                        form.cycle_agent(true);
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Left | KeyCode::Right
                        if form.field == FormField::Background =>
                    {
                        form.toggle_background();
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Char(' ') if form.field == FormField::Background => {
                        form.toggle_background();
                        self.app.modal = Modal::Form(form);
                    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib engine::tests::space_toggles_background_field_in_form`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/engine.rs
git commit -m "feat(form): toggle start-in-background with space/left/right

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Extract `prepare_session` from `start_session`

This is a pure refactor — no behavior change. The existing `start_session` tests must stay green.

**Files:**
- Modify: `src/engine.rs`

- [ ] **Step 1: Add the `Prepared` struct and `prepare_session` method**

In `src/engine.rs`, add a struct (near the `Effect` enum) describing a prepared-but-not-launched session:

```rust
/// Everything needed to launch a session, produced by `prepare_session`
/// before any DB session/status columns are written.
pub struct Prepared {
    pub name: String,
    pub layout_path: PathBuf,
    pub worktree: PathBuf,
    pub instrumented: bool,
}
```

Add the method to `impl Engine` (lift the body of the current `start_session` up to the point just before the DB writes):

```rust
    /// Build the worktree + layout for a ticket without writing any DB
    /// session/status columns. Shared by foreground and background start.
    fn prepare_session(&mut self, ticket: &Ticket) -> Result<Prepared> {
        let root = self.app.project.root_dir.clone();
        if !git::is_git_repo(&root) {
            bail!("project root is not a git repository: {}", root.display());
        }
        let name = slug::ticket_name(ticket.id, &ticket.title);
        let base = if self.config.base_branch == "auto" {
            git::default_branch(&root)?
        } else {
            self.config.base_branch.clone()
        };
        let worktree = self.config.worktree_dir(&root, &name);
        if !worktree.exists() {
            git::add_worktree(&root, &worktree, &name, &base)?;
        }
        let argv = agent::build_command(
            self.config.commands_for(ticket.agent),
            ticket.initial_prompt.as_deref(),
        );
        let instrumented = self.config.auto_review.enabled && ticket.agent == Agent::Claude;
        let argv = if instrumented {
            let marker = detect::marker_path(&self.state_dir, &name);
            let _ = std::fs::create_dir_all(&self.state_dir);
            let _ = std::fs::remove_file(&marker);
            detect::inject_claude_settings(argv, &marker.to_string_lossy())
        } else {
            argv
        };
        let bar = zellij_config::resolve_bar_style(
            &self.config.zellij_bar,
            zellij_config::detect_default_layout().as_deref(),
        );
        let kdl = layout::render_layout(&worktree.to_string_lossy(), &argv, bar);
        let layout_path = self.layout_file(&name, &kdl)?;
        Ok(Prepared {
            name,
            layout_path,
            worktree,
            instrumented,
        })
    }
```

- [ ] **Step 2: Rewrite `start_session` to use `prepare_session`**

Replace the existing `start_session` body with:

```rust
    /// Create the worktree + layout for a ticket and return the RunSession effect.
    fn start_session(&mut self, ticket: &Ticket) -> Result<Effect> {
        let p = self.prepare_session(ticket)?;
        self.db.set_ticket_session(
            ticket.id,
            &p.name,
            &p.worktree.to_string_lossy(),
            &p.name,
        )?;
        self.db.set_ticket_instrumented(ticket.id, p.instrumented)?;
        self.db.set_ticket_status(ticket.id, Status::InProgress)?;
        self.reload()?;
        Ok(Effect::RunSession {
            name: p.name,
            layout_path: p.layout_path,
        })
    }
```

- [ ] **Step 3: Run the existing session tests to verify no regression**

Run: `cargo test --lib engine::tests`
Expected: PASS — including `start_session_creates_worktree_and_effect`, `enter_starts_session_for_todo_ticket`, and `start_session_honors_compact_bar_override`.

- [ ] **Step 4: Commit**

```bash
git add src/engine.rs
git commit -m "refactor(engine): extract prepare_session from start_session

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Add `Effect::RunSessionBackground` and background start in `submit_form`

**Files:**
- Modify: `src/engine.rs`
- Test: `src/engine.rs` (inline tests)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/engine.rs` (the `init_repo` and `enter` helpers already exist):

```rust
/// Submitting the create form with the background toggle on (in a real git
/// repo) prepares a session and returns RunSessionBackground; the ticket is
/// moved to In Progress with a recorded session.
#[test]
fn create_with_background_toggle_starts_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    init_repo(&root);

    let mut e = engine_with_project(root.clone());
    e.config.worktree_base = dir.path().join("wts").to_string_lossy().to_string();
    e.state_dir = dir.path().join("state");

    e.on_key(key('c')).unwrap();
    for ch in "Add login".chars() {
        e.on_key(key(ch)).unwrap();
    }
    let effect = e
        .on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Enter,
            ratatui::crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();

    let t = &e.app.tickets[0];
    let name = slug::ticket_name(t.id, "Add login");
    match effect {
        Effect::RunSessionBackground {
            name: n,
            layout_path,
            cwd,
        } => {
            assert_eq!(n, name);
            assert!(layout_path.exists());
            assert!(cwd.ends_with(&name));
        }
        other => panic!("expected RunSessionBackground, got {other:?}"),
    }
    assert_eq!(t.status, Status::InProgress);
    assert_eq!(t.session_name.as_deref(), Some(name.as_str()));

    e.cleanup_ticket(e.app.tickets[0].id).unwrap();
}

/// With the toggle off, creation is the classic Todo card with no session.
#[test]
fn create_without_background_toggle_makes_todo_card() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    init_repo(&root);
    let mut e = engine_with_project(root.clone());
    e.config.worktree_base = dir.path().join("wts").to_string_lossy().to_string();

    e.on_key(key('c')).unwrap();
    for ch in "Plan only".chars() {
        e.on_key(key(ch)).unwrap();
    }
    // Tab to Background and turn it off.
    for _ in 0..4 {
        e.on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Tab,
            ratatui::crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
    }
    e.on_key(key(' ')).unwrap();
    let effect = e
        .on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Enter,
            ratatui::crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(effect, Effect::None);
    assert_eq!(e.app.tickets[0].status, Status::Todo);
    assert_eq!(e.app.tickets[0].session_name, None);
}

/// Toggle on but the project root is not a git repo: the ticket is still
/// created, left in Todo, with an error toast (graceful failure).
#[test]
fn create_with_background_toggle_in_non_git_root_stays_todo() {
    let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
    e.on_key(key('c')).unwrap();
    for ch in "No repo".chars() {
        e.on_key(key(ch)).unwrap();
    }
    let effect = e
        .on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Enter,
            ratatui::crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(effect, Effect::None);
    assert_eq!(e.app.tickets.len(), 1);
    assert_eq!(e.app.tickets[0].status, Status::Todo);
    assert_eq!(e.app.tickets[0].session_name, None);
    assert!(e.app.status_message.is_some(), "an error toast is shown");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib engine::tests::create_with_background_toggle_starts_session`
Expected: FAIL — `no variant RunSessionBackground` and `submit_form` returns `()`.

- [ ] **Step 3: Add the `Effect` variant**

In `src/engine.rs`, extend the `Effect` enum:

```rust
#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    RunSession {
        name: String,
        layout_path: PathBuf,
    },
    RunSessionBackground {
        name: String,
        layout_path: PathBuf,
        cwd: PathBuf,
    },
    Attach {
        name: String,
    },
    /// Leave the board and return to the project picker.
    SwitchProject,
}
```

- [ ] **Step 4: Change `submit_form` to return an `Effect` and start the background session**

Replace `submit_form` with:

```rust
    fn submit_form(&mut self, form: &TicketForm) -> Result<Effect> {
        match form.editing_id {
            Some(id) => {
                self.db
                    .update_ticket_fields(id, &form.title, &form.description)?;
                self.reload()?;
                Ok(Effect::None)
            }
            None => {
                let ticket = self.db.create_ticket(
                    self.app.project.id,
                    &form.title,
                    &form.description,
                    form.prompt_opt().as_deref(),
                    form.agent,
                )?;
                self.reload()?;
                if !form.start_in_background {
                    return Ok(Effect::None);
                }
                // Background start: prepare the session, then commit DB state.
                // On any preparation error, leave the card in Todo with a toast.
                match self.prepare_session(&ticket) {
                    Ok(p) => {
                        self.db.set_ticket_session(
                            ticket.id,
                            &p.name,
                            &p.worktree.to_string_lossy(),
                            &p.name,
                        )?;
                        self.db.set_ticket_instrumented(ticket.id, p.instrumented)?;
                        self.db.set_ticket_status(ticket.id, Status::InProgress)?;
                        self.reload()?;
                        Ok(Effect::RunSessionBackground {
                            name: p.name,
                            layout_path: p.layout_path,
                            cwd: p.worktree,
                        })
                    }
                    Err(err) => {
                        self.app.status_message =
                            Some(format!("could not start session: {err}"));
                        Ok(Effect::None)
                    }
                }
            }
        }
    }
```

- [ ] **Step 5: Propagate the effect from the `Modal::Form` Enter arm**

In `on_key`, the `Modal::Form` arm currently returns `Ok(Effect::None)` unconditionally. Capture the submit result. Change the `KeyCode::Enter` handling inside the form arm from:

```rust
                    KeyCode::Enter => {
                        if !form.title.trim().is_empty() {
                            self.submit_form(&form)?;
                        } else {
                            self.app.modal = Modal::Form(form);
                            self.app.status_message = Some("Title is required".into());
                        }
                    }
```

to (introduce a mutable effect the arm returns at the end):

```rust
                    KeyCode::Enter => {
                        if !form.title.trim().is_empty() {
                            return self.submit_form(&form);
                        } else {
                            self.app.modal = Modal::Form(form);
                            self.app.status_message = Some("Title is required".into());
                        }
                    }
```

The form arm already ends with `Ok(Effect::None)` for all other keys, so the early `return` on a valid submit is the only change needed.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib engine::tests`
Expected: PASS — the three new tests plus all existing ones (including `create_ticket_via_form`, which uses `/tmp/none`: the toggle is on by default but the non-git root makes preparation fail gracefully, leaving the ticket in Todo, so its assertions still hold).

- [ ] **Step 7: Commit**

```bash
git add src/engine.rs
git commit -m "feat(engine): start a background session on create when toggle is on

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Add the zellij background-launch wrapper

**Files:**
- Modify: `src/zellij.rs`

Not unit-tested (shells out to zellij), consistent with `create_session` / `attach_session`.

- [ ] **Step 1: Add `create_session_background`**

In `src/zellij.rs`, add:

```rust
/// Create a DETACHED session and run `layout_path` inside it, without attaching
/// the caller. Two steps: `attach --create-background` makes the session, then
/// `action new-tab --layout` runs the agent layout in it (the new tab becomes
/// focused, so a later `attach` lands on the agent). Commands run from `cwd` and
/// use `output()` so zellij's stdout/stderr are captured rather than painted
/// onto the live TUI (same rationale as `dump_screen`).
pub fn create_session_background(name: &str, layout_path: &Path, cwd: &Path) -> Result<()> {
    let created = Command::new("zellij")
        .current_dir(cwd)
        .args(["attach", "--create-background", name])
        .output()?;
    if !created.status.success() {
        anyhow::bail!(
            "zellij attach --create-background failed: {}",
            String::from_utf8_lossy(&created.stderr)
        );
    }
    let tab = Command::new("zellij")
        .current_dir(cwd)
        .args(["--session", name, "action", "new-tab", "--layout"])
        .arg(layout_path)
        .output()?;
    if !tab.status.success() {
        anyhow::bail!(
            "zellij action new-tab failed: {}",
            String::from_utf8_lossy(&tab.stderr)
        );
    }
    Ok(())
}
```

- [ ] **Step 2: Build to verify it compiles**

Run: `cargo build`
Expected: builds clean. `Result`, `Command`, `Path` are already imported at the top of `zellij.rs`.

- [ ] **Step 3: Commit**

```bash
git add src/zellij.rs
git commit -m "feat(zellij): add detached create_session_background wrapper

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Handle `Effect::RunSessionBackground` in the main loop

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Handle the new effect without releasing the terminal**

In `src/main.rs`, in the `match effect` block of `run_board`, add an arm for the new variant. It must NOT call `run_zellij` (which releases the TUI) — the background commands return immediately. Add after the `Effect::RunSession { .. } => { ... }` arm:

```rust
            Effect::RunSessionBackground {
                name,
                layout_path,
                cwd,
            } => {
                match zellij::create_session_background(&name, &layout_path, &cwd) {
                    Ok(()) => {
                        engine.app.status_message =
                            Some(format!("Started '{name}' in the background"));
                    }
                    Err(e) => {
                        engine.app.status_message =
                            Some(format!("background session failed: {e}"));
                        // Drop the dangling session columns for the session that
                        // never came up (status stays In Progress; recoverable
                        // via Enter, which starts a fresh session).
                        engine.reconcile()?;
                    }
                }
            }
```

- [ ] **Step 2: Build to verify it compiles**

Run: `cargo build`
Expected: builds clean. The `match effect` is now exhaustive over all `Effect` variants.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test`
Expected: PASS — all tests.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): perform background session start without releasing the TUI

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Update the help text

**Files:**
- Modify: `src/ui/modals.rs` (`render_help`)

- [ ] **Step 1: Reflect the new create behavior in `?` help**

In `render_help` in `src/ui/modals.rs`, change the `c` line so it reads:

```rust
    let text = "\
↑/↓ j/k   select ticket
←/→ h/l   change column
c         create ticket (auto-starts a background session)
e         edit ticket
Enter     attach / start session
m         move ticket (then ←/→, Enter)
d         delete ticket
p         switch project
?         this help
q         quit

Any key closes this help.";
```

- [ ] **Step 2: Build to verify it compiles**

Run: `cargo build`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add src/ui/modals.rs
git commit -m "docs(ui): note background-session auto-start in help

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Format**

Run: `cargo fmt`
Expected: no diff, or run it and commit any formatting (`git commit -am "style: cargo fmt"` only if changed).

- [ ] **Step 2: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings/errors.

- [ ] **Step 3: Full test suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 4: Manual smoke test (optional but recommended)**

Run kamaji in a real git project, press `c`, type a title, leave the toggle on, press Enter. Expected: the card appears in **In Progress**, a toast says "Started '…' in the background", and `zellij list-sessions` shows the new detached session. Press `Enter` on the card to attach and confirm the agent is running in its worktree.

---

## Notes for the implementer

- The repo's tests run real `git` (see `init_repo`) but never launch real zellij; keep it that way — assert on the returned `Effect`, not on zellij side effects.
- `submit_form` changed signature from `Result<()>` to `Result<Effect>`; the only caller is the `Modal::Form` Enter arm (Task 5, Step 5).
- Graceful failure is intentional: a non-git root or missing zellij must never prevent ticket creation. Two layers cover this — `prepare_session` errors are caught in `submit_form` (Task 5), and `create_session_background` errors are caught in `main.rs` (Task 7).
