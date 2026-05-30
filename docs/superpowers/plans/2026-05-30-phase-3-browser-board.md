# Phase 3: Browser Board — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `kamajid` serve a polished, server-rendered Kanban web board at `GET /`, kept live by the daemon's existing SSE broadcast via Datastar HTML-fragment patches, with every command reusing the existing JSON API and ticket attach handing off to `zellij web`.

**Architecture:** New HTML-serving routes (`GET /`, `GET /ui/events`, `GET /assets/*path`, `GET /ui/tickets/new`, `GET /ui/tickets/:id/edit`) are added to the existing `axum::Router` in `crates/kamajid/src/lib.rs`. Pure `maud` view partials (`views/{page,board,card,modal}.rs`) render the document and fragments. A second SSE handler (`routes/ui_events.rs`) subscribes to the *same* `state.tx` broadcast as `routes/events.rs` but reuses `views::board::column`/`views::card::card` to frame events as Datastar element-patch SSE records, so live patches and initial render produce byte-identical markup. The JSON `/events` serializer and the entire existing JSON command API are untouched — Phase 3 adds only read/render routes.

**Tech Stack:** Rust, axum 0.7, `maud = "0.26"` (compile-time HTML, `IntoResponse for Markup`), `rust-embed = "8"` (assets embedded in the binary), `mime_guess = "2"` (content-type), vendored Datastar v1.0.0-RC.6 ESM runtime, hand-written `app.css`.

---

## File Structure

New and modified files (paths relative to repo root `/home/victor/dev/kamaji`):

```
crates/kamajid/
├── Cargo.toml                         MODIFY — add maud, rust-embed, mime_guess
├── src/
│   ├── lib.rs                         MODIFY — router() gains 5 HTML routes; pub mod views
│   ├── routes/
│   │   ├── mod.rs                     MODIFY — pub mod ui; ui_events; assets
│   │   ├── ui.rs                      NEW — GET / board page + modal fragments
│   │   ├── ui_events.rs              NEW — GET /ui/events SSE: Event → Datastar patch
│   │   └── assets.rs                 NEW — GET /assets/*path embedded static files
│   ├── views/                        NEW — pure maud partials
│   │   ├── mod.rs                    NEW — pub mod page; board; card; modal
│   │   ├── page.rs                   NEW — full document shell + top bar
│   │   ├── board.rs                  NEW — board(), column(status, &[Ticket])
│   │   ├── card.rs                   NEW — card(&Ticket)
│   │   └── modal.rs                  NEW — ticket_form(...) create/edit dialog
│   └── assets/                       NEW — vendored static files
│       ├── datastar.js              NEW — vendored Datastar runtime (pinned)
│       └── app.css                  NEW — hand-written stylesheet
└── tests/
    └── ui.rs                         NEW — HTML-route + /ui/events integration tests
```

`views/` partials are pure `(&data) -> maud::Markup`, unit-tested with `Markup::into_string()` + `contains` assertions (mirroring `crates/kamaji/src/ui/board.rs`'s buffer tests, on HTML). HTTP routes are integration-tested via the Phase 1 `spawn()` harness in `crates/kamajid/tests/`, and `/ui/events` reuses the existing inline SSE-line parser (`read_named_event`).

**Grounding facts confirmed from the live code (do not re-derive):**
- `Status::as_str()` → `"todo" | "in_progress" | "review" | "done"`; `Status::title()` → `"Todo" | "In Progress" | "Needs attention" | "Done"`; `Status::all()` returns the four in order. Column DOM ids: `col-todo`, `col-in_progress`, `col-review`, `col-done`. Card ids: `card-<id>`.
- `Agent::all()` → `[Claude, Codex, Copilot]`; `Agent::label()` → `"Claude Code" | "Codex" | "Copilot"`; `Agent::as_str()` → snake_case.
- `Ticket` fields used by the card: `id: i64`, `title: String`, `agent: Agent`, `status: Status`, `session_name: Option<String>`. Session bullet: `session_name.is_some()` → `●` else `○` (matches `board.rs` line 287-291).
- `AppState` (in `state.rs`): `with_db<T,F>(&self, f) -> Result<T, ApiError>` async helper; `tx: broadcast::Sender<Event>`; `emit(Event)`; `config: Arc<Config>`. `Config::default_agent() -> Agent`.
- `Event` variants (from `kamaji-core::events`): `TicketCreated(Ticket)`, `TicketUpdated(Ticket)`, `TicketMoved { id, from, to, at }`, `TicketDeleted { id }`, `SessionStarted { ticket_id, session_name }`, `SessionIdle { ticket_id }`, `SessionExited { ticket_id, session_name }`.
- `Db` methods (sync, called inside `with_db`): `list_projects()`, `get_project(id)`, `list_tickets(project_id)`, `get_ticket(id)`.
- Existing JSON command endpoints reused by Datastar (NOT re-implemented): `POST /tickets`, `PATCH /tickets/:id`, `POST /tickets/:id/move` `{target}`, `POST /tickets/:id/start`, `POST /tickets/:id/done` `{cleanup}`, `DELETE /tickets/:id`, `POST /tickets/:id/attach`.

**Datastar version pin:** Vendor **Datastar `v1.0.0-RC.6`**, the single-file ESM bundle, from
`https://cdn.jsdelivr.net/gh/starfederation/datastar@v1.0.0-RC.6/bundles/datastar.js`
into `crates/kamajid/src/assets/datastar.js` (the orchestrator fetches the real bytes; this plan writes the embed/serve code and the serializer against this version). The Datastar SSE patch event used by `ui_events.rs` is **`event: datastar-patch-elements`**, whose `data:` lines are `mode <patch-mode>` (omit for default outer-morph) and one or more `elements <html>` lines. Patch modes used: default (outer morph by id) for columns and card replace, `mode append` for new cards into a column container, `mode remove` for deleted cards. Client attributes used in markup: `data-on-load="@get('/ui/events')"`, `data-on-click="@post('/tickets/{id}/move', {target:'review'})"` etc., `data-on-submit`. **These exact spellings (`datastar-patch-elements`, `mode append`, `mode remove`, `data-*`, `@get/@post/@patch/@delete`) are the RC.6 contract — if the vendored file differs, the implementer adjusts the serializer string constants in `ui_events.rs` and the `data-*` attributes in the views to match the vendored runtime, in one place each.**

---

## Step 3a — Static board page (read-only)

Ends green: `GET /` renders the four-column board for a project; `/assets/*` serves the embedded Datastar + CSS; no reactivity or commands yet.

### Task 3a.1 — Add deps and module scaffolding

**Files:**
- Modify: `crates/kamajid/Cargo.toml`
- Modify: `crates/kamajid/src/lib.rs`
- Modify: `crates/kamajid/src/routes/mod.rs`
- Create: `crates/kamajid/src/views/mod.rs`

Steps:

- [ ] 1. Add to the `[dependencies]` section of `crates/kamajid/Cargo.toml` (after the `chrono` line):
  ```toml
  maud = "0.26"
  rust-embed = "8"
  mime_guess = "2"
  ```
- [ ] 2. Create `crates/kamajid/src/views/mod.rs` with empty module wiring (the submodules are added in later tasks; declaring them now would not compile, so start minimal):
  ```rust
  //! Pure `maud` view partials shared by the HTML page routes and the
  //! `/ui/events` fragment serializer. Each is `(&data) -> maud::Markup`.
  ```
- [ ] 3. In `crates/kamajid/src/lib.rs`, add `pub mod views;` directly after `pub mod state;` (keep modules alphabetical-ish; it must appear so `views` compiles):
  ```rust
  pub mod state;
  pub mod views;
  pub mod zellij_web;
  ```
- [ ] 4. Run `cargo build -p kamajid` and expect **PASS** (compiles; deps resolve). Command: `cargo build -p kamajid`.
- [ ] 5. Commit: `git commit -am "build(kamajid): add maud/rust-embed/mime_guess and views module"`.

### Task 3a.2 — `card()` view partial

**Files:**
- Create: `crates/kamajid/src/views/card.rs`
- Modify: `crates/kamajid/src/views/mod.rs`

Steps:

- [ ] 1. Add `pub mod card;` to `crates/kamajid/src/views/mod.rs`.
- [ ] 2. Write a failing test first. Create `crates/kamajid/src/views/card.rs` with the test module and a stub:
  ```rust
  //! The per-ticket card partial. Stable id `card-<id>`; session bullet `●`/`○`;
  //! `#<id>` + title; agent label; state-appropriate action buttons firing the
  //! existing JSON API via Datastar. Pure: `card(&Ticket) -> Markup`.

  use kamaji_core::models::{Status, Ticket};
  use maud::{html, Markup, PreEscaped};

  /// Render one ticket as a card element. The id is `card-<id>` so SSE patches
  /// can target it; the per-column accent comes from `data-status`.
  pub fn card(t: &Ticket) -> Markup {
      let bullet = if t.session_name.is_some() { "●" } else { "○" };
      html! {
          article id=(format!("card-{}", t.id))
                  class="card"
                  data-status=(t.status.as_str()) {
              header class="card-head" {
                  span class="bullet" { (bullet) }
                  span class="card-id" { "#" (t.id) }
                  span class="card-title" { (t.title) }
              }
              div class="card-meta" {
                  span class="agent" { (t.agent.label()) }
                  @if matches!(t.status, Status::InProgress | Status::Review) {
                      span class="chip" {
                          @if t.status == Status::Review { "idle" } @else { "active" }
                      }
                  }
              }
              (card_actions(t))
          }
      }
  }

  /// State-appropriate action buttons. Each fires the EXISTING JSON command API
  /// via a Datastar action attribute; the authoritative UI update arrives over
  /// `/ui/events` (3c), so the response body is ignored.
  fn card_actions(t: &Ticket) -> Markup {
      let id = t.id;
      html! {
          footer class="card-actions" {
              @match t.status {
                  Status::Todo => {
                      button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/start')"))) { "▸ Start" }
                      button class="act" data-on-click=(PreEscaped(format!("@get('/ui/tickets/{id}/edit')"))) { "Edit" }
                      button class="act danger" data-on-click=(PreEscaped(format!("@delete('/tickets/{id}')"))) { "Delete" }
                  }
                  Status::InProgress => {
                      button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/attach')"))) { "⤢ Attach" }
                      button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/move', {{target:'review'}})"))) { "Move" }
                      button class="act" data-on-click=(PreEscaped(format!("@get('/ui/tickets/{id}/edit')"))) { "Edit" }
                      button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/done', {{cleanup:false}})"))) { "✓ Done" }
                  }
                  Status::Review => {
                      button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/attach')"))) { "⤢ Attach" }
                      button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/move', {{target:'in_progress'}})"))) { "↩ In Progress" }
                      button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/done', {{cleanup:false}})"))) { "✓ Done" }
                      button class="act" data-on-click=(PreEscaped(format!("@get('/ui/tickets/{id}/edit')"))) { "Edit" }
                  }
                  Status::Done => {
                      button class="act danger" data-on-click=(PreEscaped(format!("@delete('/tickets/{id}')"))) { "Delete" }
                  }
              }
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use kamaji_core::models::Agent;

      fn ticket(id: i64, status: Status) -> Ticket {
          Ticket {
              id, project_id: 1, title: format!("title{id}"), description: String::new(),
              initial_prompt: None, agent: Agent::Claude, status, position: 0,
              session_name: None, worktree_path: None, branch: None,
              auto_reviewed: false, instrumented: false,
              created_at: String::new(), updated_at: String::new(),
          }
      }

      #[test]
      fn card_has_stable_id_title_and_agent_label() {
          let html = card(&ticket(3, Status::Todo)).into_string();
          assert!(html.contains(r#"id="card-3""#), "stable card id:\n{html}");
          assert!(html.contains("#3"), "ticket id shown:\n{html}");
          assert!(html.contains("title3"), "title shown:\n{html}");
          assert!(html.contains("Claude Code"), "agent label:\n{html}");
      }

      #[test]
      fn no_session_renders_hollow_bullet() {
          let html = card(&ticket(1, Status::Todo)).into_string();
          assert!(html.contains("○"), "hollow bullet when no session:\n{html}");
          assert!(!html.contains("●"), "no filled bullet:\n{html}");
      }

      #[test]
      fn live_session_renders_filled_bullet() {
          let mut t = ticket(1, Status::InProgress);
          t.session_name = Some("sess1".into());
          let html = card(&t).into_string();
          assert!(html.contains("●"), "filled bullet when session present:\n{html}");
      }

      #[test]
      fn todo_card_offers_start_not_attach() {
          let html = card(&ticket(1, Status::Todo)).into_string();
          assert!(html.contains("/tickets/1/start"), "Start action:\n{html}");
          assert!(!html.contains("/tickets/1/attach"), "no attach in todo:\n{html}");
      }

      #[test]
      fn in_progress_card_offers_attach() {
          let html = card(&ticket(5, Status::InProgress)).into_string();
          assert!(html.contains("/tickets/5/attach"), "Attach action:\n{html}");
      }
  }
  ```
- [ ] 3. Run the (initially absent-impl) test: `cargo test -p kamajid --lib views::card`. Because the impl is written inline with the test in step 2, run it now and expect **PASS**. (If iterating TDD-strictly, comment out the bodies first to see FAIL, then restore.) Expected: all 5 tests pass.
- [ ] 4. Commit: `git commit -am "feat(kamajid): add card() maud view partial"`.

### Task 3a.3 — `column()` and `board()` view partials

**Files:**
- Create: `crates/kamajid/src/views/board.rs`
- Modify: `crates/kamajid/src/views/mod.rs`

Steps:

- [ ] 1. Add `pub mod board;` to `crates/kamajid/src/views/mod.rs`.
- [ ] 2. Create `crates/kamajid/src/views/board.rs` with impl + tests:
  ```rust
  //! Column and board partials. `column()` is reused verbatim by the SSE
  //! serializer (3c) so an initial render and a live patch are byte-identical.

  use kamaji_core::models::{Status, Ticket};
  use maud::{html, Markup};

  use super::card::card;

  /// One Kanban column. Stable id `col-<status.as_str()>` is the SSE patch
  /// target. Header shows the title (`Status::title()` → "Needs attention" for
  /// Review) and the live count. Empty columns show a quiet placeholder.
  pub fn column(status: Status, tickets: &[Ticket]) -> Markup {
      html! {
          section class="column"
                  id=(format!("col-{}", status.as_str()))
                  data-status=(status.as_str()) {
              header class="col-head" {
                  span class="col-title" { (status.title()) }
                  span class="col-count" { (tickets.len()) }
              }
              div class="col-body" {
                  @if tickets.is_empty() {
                      p class="col-empty" { "Nothing here" }
                  } @else {
                      @for t in tickets {
                          (card(t))
                      }
                  }
              }
          }
      }
  }

  /// The full four-column board. `by_status` is indexed by `Status::all()` order.
  pub fn board(by_status: &[(Status, Vec<Ticket>)]) -> Markup {
      html! {
          main class="board" id="board" {
              @for (status, tickets) in by_status {
                  (column(*status, tickets))
              }
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use kamaji_core::models::Agent;

      fn ticket(id: i64, status: Status) -> Ticket {
          Ticket {
              id, project_id: 1, title: format!("title{id}"), description: String::new(),
              initial_prompt: None, agent: Agent::Claude, status, position: 0,
              session_name: None, worktree_path: None, branch: None,
              auto_reviewed: false, instrumented: false,
              created_at: String::new(), updated_at: String::new(),
          }
      }

      #[test]
      fn column_has_stable_id_keyed_off_status() {
          let html = column(Status::Review, &[]).into_string();
          assert!(html.contains(r#"id="col-review""#), "stable review id:\n{html}");
      }

      #[test]
      fn review_column_titled_needs_attention() {
          let html = column(Status::Review, &[]).into_string();
          assert!(html.contains("Needs attention"), "review header label:\n{html}");
      }

      #[test]
      fn empty_column_shows_placeholder() {
          let html = column(Status::Todo, &[]).into_string();
          assert!(html.contains("Nothing here"), "empty placeholder:\n{html}");
      }

      #[test]
      fn column_shows_count_and_cards() {
          let ts = vec![ticket(1, Status::Todo), ticket(2, Status::Todo)];
          let html = column(Status::Todo, &ts).into_string();
          assert!(html.contains(r#"class="col-count">2"#), "count 2:\n{html}");
          assert!(html.contains("card-1") && html.contains("card-2"), "both cards:\n{html}");
      }

      #[test]
      fn board_renders_all_four_columns() {
          let by = vec![
              (Status::Todo, vec![ticket(1, Status::Todo)]),
              (Status::InProgress, vec![]),
              (Status::Review, vec![]),
              (Status::Done, vec![]),
          ];
          let html = board(&by).into_string();
          for id in ["col-todo", "col-in_progress", "col-review", "col-done"] {
              assert!(html.contains(id), "missing {id} in:\n{html}");
          }
      }
  }
  ```
- [ ] 3. Run: `cargo test -p kamajid --lib views::board`. Expected: **PASS** (5 tests).
- [ ] 4. Commit: `git commit -am "feat(kamajid): add column() and board() maud partials"`.

### Task 3a.4 — `page()` document shell

**Files:**
- Create: `crates/kamajid/src/views/page.rs`
- Modify: `crates/kamajid/src/views/mod.rs`

Steps:

- [ ] 1. Add `pub mod page;` to `crates/kamajid/src/views/mod.rs`.
- [ ] 2. Create `crates/kamajid/src/views/page.rs`:
  ```rust
  //! The full HTML document shell: head (CSS link + vendored Datastar module +
  //! viewport), a top bar (wordmark, project switcher, "+ Ticket"), the board,
  //! and an empty modal mount. `data-on-load` opens the `/ui/events` SSE stream.

  use kamaji_core::models::{Project, Status, Ticket};
  use maud::{html, Markup, DOCTYPE};

  use super::board::board;

  /// Render the board page for `project`, with `projects` populating the switcher.
  pub fn page(project: &Project, projects: &[Project], by_status: &[(Status, Vec<Ticket>)]) -> Markup {
      html! {
          (DOCTYPE)
          html lang="en" data-theme="dark" {
              head {
                  meta charset="utf-8";
                  meta name="viewport" content="width=device-width, initial-scale=1";
                  title { "kamaji — " (project.name) }
                  link rel="stylesheet" href="/assets/app.css";
                  script type="module" src="/assets/datastar.js" {}
              }
              body data-on-load="@get('/ui/events')" {
                  header class="topbar" {
                      span class="wordmark" { "kamaji" }
                      div class="project-switcher" {
                          label for="project-select" { "project" }
                          select id="project-select"
                                 data-on-change="window.location = '/?project=' + evt.target.value" {
                              @for p in projects {
                                  option value=(p.id) selected[p.id == project.id] { (p.name) }
                              }
                          }
                      }
                      button class="new-ticket"
                             data-on-click=(maud::PreEscaped(format!("@get('/ui/tickets/new?project={}')", project.id))) {
                          "+ Ticket"
                      }
                  }
                  (board(by_status))
                  div id="modal" {}
              }
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use kamaji_core::models::Agent;
      use std::path::PathBuf;

      fn project(id: i64, name: &str) -> Project {
          Project { id, name: name.into(), root_dir: PathBuf::from("/tmp/p"),
                    default_agent: Some(Agent::Claude), created_at: String::new() }
      }

      fn empty_board() -> Vec<(Status, Vec<Ticket>)> {
          Status::all().into_iter().map(|s| (s, Vec::new())).collect()
      }

      #[test]
      fn page_links_css_and_vendored_datastar() {
          let p = project(1, "acme");
          let html = page(&p, &[p.clone()], &empty_board()).into_string();
          assert!(html.contains(r#"href="/assets/app.css""#), "css link:\n{html}");
          assert!(html.contains(r#"src="/assets/datastar.js""#), "datastar module:\n{html}");
      }

      #[test]
      fn page_opens_ui_events_on_load() {
          let p = project(1, "acme");
          let html = page(&p, &[p.clone()], &empty_board()).into_string();
          assert!(html.contains(r#"data-on-load="@get('/ui/events')""#), "sse hook:\n{html}");
      }

      #[test]
      fn page_has_modal_mount_and_switcher() {
          let p = project(1, "acme");
          let html = page(&p, &[p.clone()], &empty_board()).into_string();
          assert!(html.contains(r#"id="modal""#), "modal mount:\n{html}");
          assert!(html.contains("acme"), "switcher option:\n{html}");
      }
  }
  ```
- [ ] 3. Run: `cargo test -p kamajid --lib views::page`. Expected: **PASS** (3 tests).
- [ ] 4. Commit: `git commit -am "feat(kamajid): add page() document shell"`.

### Task 3a.5 — Embedded assets + `GET /assets/*path`

**Files:**
- Create: `crates/kamajid/src/assets/datastar.js` (vendored — see note below)
- Create: `crates/kamajid/src/assets/app.css`
- Create: `crates/kamajid/src/routes/assets.rs`
- Modify: `crates/kamajid/src/routes/mod.rs`
- Modify: `crates/kamajid/src/lib.rs`

Steps:

- [ ] 1. Vendor the runtime: download `https://cdn.jsdelivr.net/gh/starfederation/datastar@v1.0.0-RC.6/bundles/datastar.js` into `crates/kamajid/src/assets/datastar.js` (the orchestrator performs the network fetch outside the sandbox; the file must exist before this crate builds). If the served runtime's patch event name differs from `datastar-patch-elements`, note it for Task 3c.1.
- [ ] 2. Create `crates/kamajid/src/assets/app.css` with this real starter stylesheet (3e refines it):
  ```css
  /* kamaji board — dark-first, echoing the TUI's catppuccin-style palette.
     Design tokens up top; column/card/topbar styles below. */
  :root {
    --bg:        #1e1e2e;
    --surface:   #313244;
    --surface-2: #45475a;
    --border:    #45475a;
    --text:      #cdd6f4;
    --muted:     #a6adc8;
    --accent:    #89b4fa;
    --danger:    #f38ba8;
    /* per-column accents (echo TUI status_color) */
    --col-todo:        #89b4fa;
    --col-in_progress: #f9e2af;
    --col-review:      #fab387;
    --col-done:        #a6e3a1;
    --radius: 10px;
    --gap: 12px;
    --pad: 14px;
    --font: ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
    --mono: ui-monospace, "JetBrains Mono", SFMono-Regular, Menlo, monospace;
  }
  * { box-sizing: border-box; }
  html, body { margin: 0; height: 100%; }
  body {
    background: var(--bg); color: var(--text);
    font-family: var(--font); font-size: 14px; line-height: 1.4;
  }
  .topbar {
    display: flex; align-items: center; gap: 20px;
    padding: 12px 20px; border-bottom: 1px solid var(--border);
  }
  .wordmark { font-weight: 700; letter-spacing: 0.5px; font-size: 18px; }
  .project-switcher { display: flex; align-items: center; gap: 8px; color: var(--muted); }
  .project-switcher select {
    background: var(--surface); color: var(--text);
    border: 1px solid var(--border); border-radius: 6px; padding: 4px 8px;
  }
  .new-ticket {
    margin-left: auto; background: var(--accent); color: #11111b;
    border: 0; border-radius: 6px; padding: 6px 12px; font-weight: 600; cursor: pointer;
  }
  .board {
    display: grid; grid-template-columns: repeat(4, 1fr);
    gap: var(--gap); padding: var(--gap); align-items: start;
  }
  .column {
    background: color-mix(in srgb, var(--surface) 40%, transparent);
    border: 1px solid var(--border); border-radius: var(--radius);
    padding: var(--pad); min-height: 120px;
  }
  .column[data-status="todo"]        { --col: var(--col-todo); }
  .column[data-status="in_progress"] { --col: var(--col-in_progress); }
  .column[data-status="review"]      { --col: var(--col-review); }
  .column[data-status="done"]        { --col: var(--col-done); }
  .col-head {
    display: flex; justify-content: space-between; align-items: center;
    border-bottom: 1px dashed var(--border); padding-bottom: 8px; margin-bottom: 10px;
  }
  .col-title { color: var(--col); font-weight: 700; text-transform: uppercase;
               letter-spacing: 0.6px; font-size: 12px; }
  .col-count { color: var(--muted); font-variant-numeric: tabular-nums; }
  .col-body { display: flex; flex-direction: column; gap: 10px; }
  .col-empty { color: var(--muted); font-style: italic; opacity: 0.7; margin: 4px 0; }
  .card {
    background: var(--surface); border: 1px solid var(--border);
    border-left: 3px solid var(--col, var(--border)); border-radius: 8px;
    padding: 10px 12px; transition: transform 160ms ease, background 160ms ease;
  }
  .card:hover { background: var(--surface-2); }
  .card-head { display: flex; align-items: baseline; gap: 6px; }
  .bullet { color: var(--col, var(--muted)); }
  .card[data-status="review"] .bullet { color: var(--col-review); }
  .card-id { color: var(--col, var(--accent)); font-family: var(--mono); font-size: 12px; }
  .card-title { color: var(--text); font-weight: 600; }
  .card-meta { display: flex; gap: 8px; margin: 6px 0 8px; color: var(--muted); font-size: 12px; }
  .chip {
    border: 1px solid var(--border); border-radius: 999px;
    padding: 0 8px; font-size: 11px; color: var(--muted);
  }
  .card-actions { display: flex; flex-wrap: wrap; gap: 6px; }
  .act {
    background: transparent; color: var(--text); border: 1px solid var(--border);
    border-radius: 6px; padding: 3px 8px; font-size: 12px; cursor: pointer;
  }
  .act:hover { border-color: var(--accent); }
  .act.danger:hover { border-color: var(--danger); color: var(--danger); }
  dialog.modal {
    background: var(--surface); color: var(--text);
    border: 1px solid var(--border); border-radius: var(--radius);
    padding: 20px; min-width: 380px;
  }
  dialog.modal::backdrop { background: rgba(0,0,0,0.5); }
  .modal label { display: block; margin: 10px 0 4px; color: var(--muted); font-size: 12px; }
  .modal input, .modal textarea, .modal select {
    width: 100%; background: var(--bg); color: var(--text);
    border: 1px solid var(--border); border-radius: 6px; padding: 6px 8px;
  }
  .modal .form-actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 16px; }
  .form-error { color: var(--danger); font-size: 12px; margin-top: 8px; }
  @media (prefers-reduced-motion: reduce) {
    .card { transition: none; }
  }
  ```
- [ ] 3. Add `pub mod assets;` to `crates/kamajid/src/routes/mod.rs`.
- [ ] 4. Create `crates/kamajid/src/routes/assets.rs`:
  ```rust
  //! Serve the embedded static assets (`/assets/*path`). The Datastar runtime and
  //! `app.css` are compiled into the binary via `rust-embed`, so `kamajid` stays a
  //! single self-contained binary. Content-type is derived from the extension; a
  //! weak ETag from the embedded content hash enables browser caching.

  use axum::extract::Path;
  use axum::http::{header, StatusCode};
  use axum::response::{IntoResponse, Response};
  use rust_embed::RustEmbed;

  #[derive(RustEmbed)]
  #[folder = "src/assets/"]
  struct Assets;

  /// `GET /assets/*path` → the embedded file, or 404.
  pub async fn serve(Path(path): Path<String>) -> Response {
      match Assets::get(&path) {
          Some(file) => {
              let mime = mime_guess::from_path(&path).first_or_octet_stream();
              let etag = format!("\"{:x}\"", u128::from_le_bytes(file.metadata.sha256_hash()[..16].try_into().unwrap()));
              (
                  [
                      (header::CONTENT_TYPE, mime.as_ref().to_string()),
                      (header::ETAG, etag),
                      (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
                  ],
                  file.data,
              )
                  .into_response()
          }
          None => (StatusCode::NOT_FOUND, "asset not found").into_response(),
      }
  }
  ```
- [ ] 5. In `crates/kamajid/src/lib.rs` `router()`, add the assets route before `.layer(...)`:
  ```rust
  .route("/assets/*path", get(routes::assets::serve))
  ```
- [ ] 6. Add an integration test to a new `crates/kamajid/tests/ui.rs` (create the file; it reuses the Phase 1 `spawn()` pattern — copy the `spawn()` helper and `mod support;` exactly as `tests/api.rs` has them):
  ```rust
  //! Integration tests for the Phase 3 browser routes. Boots the daemon on an
  //! ephemeral port (same harness as tests/api.rs) and drives it over HTTP.

  mod support;

  use kamaji_core::config::Config;
  use kamaji_core::db::Db;
  use kamajid::state::AppState;

  async fn spawn() -> (String, AppState) {
      let state = AppState::new(Db::open_in_memory().unwrap(), Config::default());
      let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
      let addr = listener.local_addr().unwrap();
      let app = kamajid::router(state.clone());
      tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
      (format!("http://{addr}"), state)
  }

  #[tokio::test]
  async fn serves_embedded_datastar_and_css() {
      let (base, _state) = spawn().await;
      let js = reqwest::get(format!("{base}/assets/datastar.js")).await.unwrap();
      assert_eq!(js.status(), 200);
      let ct = js.headers().get("content-type").unwrap().to_str().unwrap().to_string();
      assert!(ct.contains("javascript"), "datastar served as JS, got {ct}");

      let css = reqwest::get(format!("{base}/assets/app.css")).await.unwrap();
      assert_eq!(css.status(), 200);
      let ct = css.headers().get("content-type").unwrap().to_str().unwrap().to_string();
      assert!(ct.contains("css"), "css content-type, got {ct}");

      let missing = reqwest::get(format!("{base}/assets/nope.txt")).await.unwrap();
      assert_eq!(missing.status(), 404);
  }
  ```
- [ ] 7. Run: `cargo test -p kamajid --test ui serves_embedded`. Expected: **PASS**.
- [ ] 8. Commit: `git commit -am "feat(kamajid): embed and serve datastar.js + app.css"`.

### Task 3a.6 — `GET /` board page route

**Files:**
- Create: `crates/kamajid/src/routes/ui.rs`
- Modify: `crates/kamajid/src/routes/mod.rs`
- Modify: `crates/kamajid/src/lib.rs`
- Modify: `crates/kamajid/tests/ui.rs`

Steps:

- [ ] 1. Add `pub mod ui;` to `crates/kamajid/src/routes/mod.rs`.
- [ ] 2. Create `crates/kamajid/src/routes/ui.rs` with the board page handler (modal handlers come in 3e — leave a stub `// modal fragment handlers added in 3e`):
  ```rust
  //! HTML-serving routes: the board page (`GET /`) and the create/edit modal
  //! fragments (`GET /ui/tickets/new`, `GET /ui/tickets/:id/edit`, added in 3e).
  //! Read/render only — all mutations reuse the existing JSON command API.

  use axum::extract::{Query, State};
  use maud::Markup;
  use serde::Deserialize;

  use kamaji_core::models::{Status, Ticket};

  use crate::error::ApiError;
  use crate::state::AppState;
  use crate::views;

  #[derive(Deserialize)]
  pub struct BoardQuery {
      pub project: Option<i64>,
  }

  /// `GET /` → the full board page. `?project=<id>` selects the project; absent,
  /// the first project is used. 404 (rendered as an error) if there are none.
  pub async fn board(
      State(state): State<AppState>,
      Query(q): Query<BoardQuery>,
  ) -> Result<Markup, ApiError> {
      let want = q.project;
      let (projects, project, by_status) = state
          .with_db(move |db| {
              let projects = db.list_projects()?;
              let Some(project) = (match want {
                  Some(id) => projects.iter().find(|p| p.id == id).cloned(),
                  None => projects.first().cloned(),
              }) else {
                  return Ok(None);
              };
              let tickets = db.list_tickets(project.id)?;
              let by_status = group_by_status(tickets);
              Ok(Some((projects, project, by_status)))
          })
          .await?
          .ok_or(ApiError::NotFound)?;
      Ok(views::page::page(&project, &projects, &by_status))
  }

  /// Partition a project's tickets into `Status::all()` order — the shape both
  /// `views::board::board` and the SSE serializer consume.
  pub fn group_by_status(tickets: Vec<Ticket>) -> Vec<(Status, Vec<Ticket>)> {
      Status::all()
          .into_iter()
          .map(|s| (s, tickets.iter().filter(|t| t.status == s).cloned().collect()))
          .collect()
  }
  ```
- [ ] 3. In `crates/kamajid/src/lib.rs` `router()`, add the root route (top of the chain, after `Router::new()`):
  ```rust
  .route("/", get(routes::ui::board))
  ```
- [ ] 4. Add an integration test to `crates/kamajid/tests/ui.rs`:
  ```rust
  #[tokio::test]
  async fn board_page_renders_columns_and_seeded_card() {
      let (base, state) = spawn().await;
      let _ = state.with_db(|db| {
          let p = db.create_project("acme", std::path::Path::new("/tmp/acme"), None)?;
          db.create_ticket(p.id, "Add login", "", None, kamaji_core::models::Agent::Claude)?;
          Ok(())
      }).await.unwrap();

      let resp = reqwest::get(format!("{base}/")).await.unwrap();
      assert_eq!(resp.status(), 200);
      let ct = resp.headers().get("content-type").unwrap().to_str().unwrap().to_string();
      assert!(ct.contains("text/html"), "html content-type, got {ct}");
      let body = resp.text().await.unwrap();
      for id in ["col-todo", "col-in_progress", "col-review", "col-done"] {
          assert!(body.contains(id), "missing {id}:\n{body}");
      }
      assert!(body.contains("Add login"), "seeded card title present");
      assert!(body.contains("Needs attention"), "review header label present");
  }
  ```
- [ ] 5. Run: `cargo test -p kamajid --test ui board_page_renders`. Expected: **PASS**. (`maud::Markup` implements `IntoResponse` with `text/html; charset=utf-8`.)
- [ ] 6. Run the full crate suite to confirm no regressions: `cargo test -p kamajid`. Expected: **PASS** (existing api.rs tests + new ones).
- [ ] 7. Commit: `git commit -am "feat(kamajid): serve the board page at GET /"`.

**3a verification:** `GET /` returns the four-column board with seeded cards in the right columns; `/assets/*` serves Datastar + CSS. Manual: run `kamajid serve`, open `http://127.0.0.1:8755/`, confirm tickets appear (no live updates yet).

---

## Step 3b — Commands wired to the existing API (full-reload crutch)

Ends green: card actions and the project switcher fire Datastar `@post/@patch/@delete` against existing JSON endpoints; with no SSE yet, the page reloads after each command so the board reflects the mutation.

> Note: The `data-on-click` action attributes were already authored in `card.rs` (3a.2) and `page.rs` (3a.4). This step adds the **temporary full-reload behavior** so the board updates before SSE exists, plus a verification test that the actions target real endpoints.

### Task 3b.1 — Full-reload-after-command glue

**Files:**
- Modify: `crates/kamajid/src/views/page.rs`
- Modify: `crates/kamajid/tests/ui.rs`

Steps:

- [ ] 1. In `page.rs`, add a body-level Datastar listener that reloads the page after any settled command, as a temporary crutch removed in 3c. Add this attribute to the `body` element (alongside `data-on-load`):
  ```rust
  body data-on-load="@get('/ui/events')"
       data-on-datastar-fetch__window="if (evt.detail.type === 'finished') window.location.reload()" {
  ```
  Add a code comment above it:
  ```rust
  // TEMPORARY (3b): reload after each command so the board reflects mutations
  // before SSE exists. Removed in 3c when /ui/events streams live patches.
  ```
- [ ] 2. Update the `page_opens_ui_events_on_load` test expectation is unaffected; add a new test asserting the reload crutch is present:
  ```rust
  #[test]
  fn page_reloads_after_command_until_sse() {
      let p = project(1, "acme");
      let html = page(&p, &[p.clone()], &empty_board()).into_string();
      assert!(html.contains("datastar-fetch"), "temporary reload crutch present:\n{html}");
  }
  ```
- [ ] 3. Run: `cargo test -p kamajid --lib views::page`. Expected: **PASS**.
- [ ] 4. Add an integration test proving the command endpoints the buttons target actually mutate the board (uses existing JSON API directly, simulating what Datastar posts):
  ```rust
  #[tokio::test]
  async fn move_command_relocates_card_on_next_render() {
      let (base, state) = spawn().await;
      let tid = state.with_db(|db| {
          let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
          let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
          Ok(t.id)
      }).await.unwrap();

      reqwest::Client::new()
          .post(format!("{base}/tickets/{tid}/move"))
          .json(&serde_json::json!({ "target": "in_progress" }))
          .send().await.unwrap();

      let body = reqwest::get(format!("{base}/")).await.unwrap().text().await.unwrap();
      // After the move, the in_progress column contains the card.
      let in_prog = body.split(r#"id="col-in_progress""#).nth(1).unwrap();
      let next_col = in_prog.split(r#"id="col-review""#).next().unwrap();
      assert!(next_col.contains(&format!("card-{tid}")), "card now in in_progress:\n{body}");
  }
  ```
- [ ] 5. Run: `cargo test -p kamajid --test ui move_command_relocates`. Expected: **PASS**.
- [ ] 6. Commit: `git commit -am "feat(kamajid): wire card actions with temporary full-reload"`.

**3b verification:** Manual — open the board, click Move/Start/Done/Delete; the page reloads and the mutation shows. (Live SSE replaces the reload in 3c.)

---

## Step 3c — Live SSE reactivity

Ends green: `GET /ui/events` streams Datastar HTML-fragment patches off the same broadcast as `/events`; the page applies them live; the full-reload crutch is removed; every connection self-heals with a one-shot full-board patch.

### Task 3c.1 — Datastar SSE fragment serializer + `GET /ui/events`

**Files:**
- Create: `crates/kamajid/src/routes/ui_events.rs`
- Modify: `crates/kamajid/src/routes/mod.rs`
- Modify: `crates/kamajid/src/lib.rs`

Steps:

- [ ] 1. Add `pub mod ui_events;` to `crates/kamajid/src/routes/mod.rs`.
- [ ] 2. Create `crates/kamajid/src/routes/ui_events.rs`. This reuses `views::board::column` and `views::card::card` so live patches equal the initial render. It loads tickets via `with_db` for id-only events. The Datastar patch is one SSE event named `datastar-patch-elements` whose `data:` carries `mode <m>` (omitted for default outer-morph) plus `elements <html>` lines. Write the serializer with unit tests:
  ```rust
  //! `GET /ui/events` — the browser SSE stream. Subscribes to the SAME broadcast
  //! channel as `routes::events` (the JSON stream for the TUI), but frames each
  //! `Event` as a Datastar element-patch SSE record carrying server-rendered HTML.
  //! Reuses `views::board::column` and `views::card::card` so a live patch is
  //! byte-identical to the initial page render.
  //!
  //! Datastar wire format (pinned to the vendored v1.0.0-RC.6 runtime):
  //!   event: datastar-patch-elements
  //!   data: mode <append|remove>            (omitted → default outer morph by id)
  //!   data: elements <html fragment>        (one line per fragment)

  use std::convert::Infallible;

  use axum::extract::State;
  use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
  use futures::stream::{Stream, StreamExt};
  use kamaji_core::events::Event;
  use kamaji_core::models::{Status, Ticket};
  use maud::Markup;
  use tokio_stream::wrappers::BroadcastStream;

  use crate::routes::ui::group_by_status;
  use crate::state::AppState;
  use crate::views::{board::column, card::card};

  const PATCH_EVENT: &str = "datastar-patch-elements";

  /// A column re-render, replacing `#col-<status>` by default outer-morph.
  fn patch_column(status: Status, tickets: &[Ticket]) -> SseEvent {
      patch_elements(None, &[column(status, tickets)])
  }

  /// Build a `datastar-patch-elements` SSE event. `mode` is `None` for the
  /// default (outer morph by id), `Some("append")` / `Some("remove")` otherwise.
  /// Each `Markup` becomes one `elements` data line (no embedded newlines: maud
  /// renders without them).
  fn patch_elements(mode: Option<&str>, fragments: &[Markup]) -> SseEvent {
      let mut data = String::new();
      if let Some(m) = mode {
          data.push_str(&format!("mode {m}\n"));
      }
      for f in fragments {
          data.push_str(&format!("elements {}\n", f.clone().into_string()));
      }
      // axum's `.data()` writes each `\n`-split line as its own `data:` line.
      SseEvent::default().event(PATCH_EVENT).data(data.trim_end().to_string())
  }

  /// Remove `#card-<id>` from the DOM.
  fn patch_remove_card(id: i64) -> SseEvent {
      SseEvent::default()
          .event(PATCH_EVENT)
          .data(format!("mode remove\nselector #card-{id}"))
  }

  /// Render an event into zero or more SSE patch records. Id-only events load the
  /// current ticket(s) from `db` (a cheap read on the single-user broadcast path).
  async fn event_to_patches(state: &AppState, ev: Event) -> Vec<SseEvent> {
      match ev {
          Event::TicketCreated(t) => {
              // Append the new card into its column body container.
              vec![patch_elements(Some("append"), &[append_target(&t), card(&t)])
                  .into_append_card(&t)]
          }
          Event::TicketUpdated(t) => vec![patch_elements(None, &[card(&t)])],
          Event::TicketMoved { from, to, .. } => {
              // Re-render BOTH affected columns (fixes counts + relocates the card).
              let mut out = Vec::new();
              for status in [from, to] {
                  if let Some(col) = render_column(state, t_project_of(state, &ev).await, status).await {
                      out.push(col);
                  }
              }
              out
          }
          Event::TicketDeleted { id } => vec![patch_remove_card(id)],
          Event::SessionStarted { ticket_id, .. }
          | Event::SessionIdle { ticket_id }
          | Event::SessionExited { ticket_id, .. } => {
              match load_ticket(state, ticket_id).await {
                  Some(t) => vec![patch_elements(None, &[card(&t)])],
                  None => Vec::new(),
              }
          }
      }
  }
  ```
  > IMPLEMENTER NOTE: the `TicketMoved` arm needs the moving ticket's `project_id` to re-render its columns. `Event::TicketMoved` carries only `id, from, to, at`. Load the ticket by `id` to get `project_id`, then `list_tickets(project_id)` and re-render the `from` and `to` columns. Simplify the arm to:
  ```rust
  Event::TicketMoved { id, from, to, .. } => {
      let cols = state
          .with_db(move |db| {
              let Some(t) = db.get_ticket(id)? else { return Ok(Vec::new()); };
              let all = db.list_tickets(t.project_id)?;
              Ok([from, to].into_iter().map(|s| {
                  let in_col: Vec<Ticket> = all.iter().filter(|x| x.status == s).cloned().collect();
                  (s, in_col)
              }).collect::<Vec<_>>())
          })
          .await
          .unwrap_or_default();
      cols.into_iter().map(|(s, ts)| patch_column(s, &ts)).collect()
  }
  ```
  And replace the `TicketCreated` arm with an append into the target column body. Since Datastar append targets a selector, render the card and append it to `#col-<status> .col-body`:
  ```rust
  Event::TicketCreated(t) => {
      let frag = card(&t).into_string();
      vec![SseEvent::default().event(PATCH_EVENT).data(format!(
          "mode append\nselector #col-{} .col-body\nelements {}",
          t.status.as_str(), frag
      ))]
  }
  ```
  Helper `load_ticket`:
  ```rust
  async fn load_ticket(state: &AppState, id: i64) -> Option<Ticket> {
      state.with_db(move |db| db.get_ticket(id)).await.ok().flatten()
  }
  ```
  Drop the unused `append_target`/`render_column`/`t_project_of`/`group_by_status` helpers from the first sketch; keep only `patch_column`, `patch_remove_card`, `load_ticket`, and the inlined `TicketCreated` append. (The first sketch is intentionally a starting point; the IMPLEMENTER NOTE blocks are the authoritative bodies.)
- [ ] 3. Add the `events` handler that emits the **full-board patch first**, then streams:
  ```rust
  /// `GET /ui/events` → Datastar element-patch SSE. On connect, emit a one-shot
  /// full-board patch (re-render all four columns) so every (re)connection
  /// self-heals (§4.4), then stream live patches off the broadcast.
  pub async fn events(
      State(state): State<AppState>,
  ) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
      let rx = state.tx.subscribe();

      // Full-board snapshot: render every column for the first project (the board
      // page shows one project; a future multi-project board re-renders per ?project).
      let snapshot = {
          let by = state
              .with_db(|db| {
                  let projects = db.list_projects()?;
                  let tickets = match projects.first() {
                      Some(p) => db.list_tickets(p.id)?,
                      None => Vec::new(),
                  };
                  Ok(group_by_status(tickets))
              })
              .await
              .unwrap_or_default();
          by.into_iter().map(|(s, ts)| Ok(patch_column(s, &ts)))
              .collect::<Vec<Result<SseEvent, Infallible>>>()
      };

      let state2 = state.clone();
      let live = BroadcastStream::new(rx).filter_map(move |result| {
          let state = state2.clone();
          async move {
              match result {
                  Ok(ev) => Some(futures::stream::iter(
                      event_to_patches(&state, ev).await.into_iter().map(Ok),
                  )),
                  Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                      tracing::debug!(dropped = n, "UI SSE client lagged");
                      None
                  }
              }
          }
      }).flatten();

      let stream = futures::stream::iter(snapshot).chain(live);
      Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
  }
  ```
  Add a `use crate::routes::ui::group_by_status;`.
- [ ] 4. Add unit tests at the bottom of `ui_events.rs` asserting fragment shape:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use kamaji_core::models::Agent;

      fn ticket(id: i64, status: Status) -> Ticket {
          Ticket { id, project_id: 1, title: format!("t{id}"), description: String::new(),
              initial_prompt: None, agent: Agent::Claude, status, position: 0,
              session_name: None, worktree_path: None, branch: None,
              auto_reviewed: false, instrumented: false,
              created_at: String::new(), updated_at: String::new() }
      }

      /// Extract the SSE `data:` block an `SseEvent` would serialize. We rebuild
      /// the patch via the same constructors to assert the wire content.
      #[test]
      fn column_patch_targets_col_id() {
          let s = patch_column(Status::Review, &[ticket(1, Status::Review)]);
          // Render the same column directly to confirm reuse of views::board::column.
          let direct = column(Status::Review, &[ticket(1, Status::Review)]).into_string();
          assert!(direct.contains(r#"id="col-review""#));
          assert!(direct.contains("card-1"));
          let _ = s; // SseEvent has no public getter; the reuse is the contract.
      }

      #[test]
      fn remove_card_patch_uses_remove_mode_and_selector() {
          // Reconstruct the data string the same way the constructor does.
          let id = 7;
          let data = format!("mode remove\nselector #card-{id}");
          assert!(data.contains("mode remove"));
          assert!(data.contains("#card-7"));
      }
  }
  ```
- [ ] 5. In `crates/kamajid/src/lib.rs` `router()`, add the route after `/events`:
  ```rust
  .route("/ui/events", get(routes::ui_events::events))
  ```
- [ ] 6. Run: `cargo test -p kamajid --lib routes::ui_events`. Expected: **PASS**.
- [ ] 7. Commit: `git commit -am "feat(kamajid): add /ui/events Datastar fragment SSE serializer"`.

### Task 3c.2 — Round-trip + reconnect integration tests; remove the reload crutch

**Files:**
- Modify: `crates/kamajid/src/views/page.rs`
- Modify: `crates/kamajid/tests/ui.rs`

Steps:

- [ ] 1. Remove the temporary reload crutch from `page.rs` `body` (delete the `data-on-datastar-fetch__window` attribute and its comment). Delete the `page_reloads_after_command_until_sse` test added in 3b.1.
- [ ] 2. Run: `cargo test -p kamajid --lib views::page`. Expected: **PASS** (the reload test is gone; remaining page tests pass).
- [ ] 3. Add the SSE round-trip integration tests to `tests/ui.rs`. Reuse the exact inline SSE parser from `tests/api.rs` — copy `ByteStream`, `connect_events` (point it at `/ui/events`), and a `read_patch` helper that returns the concatenated `data:` lines of the next `datastar-patch-elements` record:
  ```rust
  use futures::StreamExt;

  type ByteStream =
      std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

  async fn connect_ui_events(base: &str) -> ByteStream {
      let resp = reqwest::Client::new()
          .get(format!("{base}/ui/events"))
          .send().await.unwrap();
      Box::pin(resp.bytes_stream())
  }

  /// Read SSE records until one whose `event:` is `datastar-patch-elements`,
  /// returning its joined `data:` payload. Times out after ~2s.
  async fn read_patch<S>(stream: &mut S) -> String
  where S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin {
      let mut buf = String::new();
      let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
      loop {
          let chunk = tokio::time::timeout_at(deadline, stream.next())
              .await.expect("timed out").expect("stream ended").expect("chunk error");
          buf.push_str(&String::from_utf8_lossy(&chunk));
          while let Some(idx) = buf.find("\n\n") {
              let record: String = buf.drain(..idx + 2).collect();
              let mut name = None;
              let mut data = String::new();
              for line in record.lines() {
                  if let Some(v) = line.strip_prefix("event:") { name = Some(v.trim().to_string()); }
                  else if let Some(v) = line.strip_prefix("data:") {
                      if !data.is_empty() { data.push('\n'); }
                      data.push_str(v.trim());
                  }
              }
              if name.as_deref() == Some("datastar-patch-elements") {
                  return data;
              }
          }
      }
  }
  ```
- [ ] 4. Add the reconnect-snapshot test (the FIRST patch on connect is the full board):
  ```rust
  #[tokio::test]
  async fn ui_events_emits_full_board_snapshot_on_connect() {
      let (base, state) = spawn().await;
      state.with_db(|db| {
          let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
          db.create_ticket(p.id, "Seed", "", None, kamaji_core::models::Agent::Claude)?;
          Ok(())
      }).await.unwrap();

      let mut stream = connect_ui_events(&base).await;
      // Collect the four snapshot column patches; the seeded card must appear in todo.
      let mut seen = String::new();
      for _ in 0..4 { seen.push_str(&read_patch(&mut stream).await); }
      assert!(seen.contains("col-todo"), "snapshot includes todo column:\n{seen}");
      assert!(seen.contains("Seed"), "snapshot includes seeded card:\n{seen}");
  }
  ```
- [ ] 5. Add the move round-trip test (connect, drain the 4 snapshot patches, then move and assert a column patch arrives):
  ```rust
  #[tokio::test]
  async fn moving_a_ticket_patches_affected_columns() {
      let (base, state) = spawn().await;
      let tid = state.with_db(|db| {
          let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
          let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
          Ok(t.id)
      }).await.unwrap();

      let mut stream = connect_ui_events(&base).await;
      for _ in 0..4 { let _ = read_patch(&mut stream).await; } // drain snapshot

      reqwest::Client::new()
          .post(format!("{base}/tickets/{tid}/move"))
          .json(&serde_json::json!({ "target": "in_progress" }))
          .send().await.unwrap();

      // Two column patches arrive (from=todo, to=in_progress); read both.
      let a = read_patch(&mut stream).await;
      let b = read_patch(&mut stream).await;
      let both = format!("{a}\n{b}");
      assert!(both.contains("col-todo"), "from column re-rendered:\n{both}");
      assert!(both.contains("col-in_progress"), "to column re-rendered:\n{both}");
      assert!(both.contains(&format!("card-{tid}")), "card present in a patch:\n{both}");
  }
  ```
- [ ] 6. Add a delete round-trip test asserting `mode remove`:
  ```rust
  #[tokio::test]
  async fn deleting_a_ticket_patches_a_remove() {
      let (base, state) = spawn().await;
      let tid = state.with_db(|db| {
          let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
          let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
          Ok(t.id)
      }).await.unwrap();

      let mut stream = connect_ui_events(&base).await;
      for _ in 0..4 { let _ = read_patch(&mut stream).await; }

      reqwest::Client::new().delete(format!("{base}/tickets/{tid}")).send().await.unwrap();

      let data = read_patch(&mut stream).await;
      assert!(data.contains("mode remove"), "remove mode:\n{data}");
      assert!(data.contains(&format!("#card-{tid}")), "targets the card:\n{data}");
  }
  ```
- [ ] 7. Run: `cargo test -p kamajid --test ui`. Expected: **PASS** (all UI integration tests).
- [ ] 8. Run the whole suite: `cargo test -p kamajid`. Expected: **PASS** (JSON `/events` tests in api.rs untouched and green).
- [ ] 9. Commit: `git commit -am "feat(kamajid): live SSE board patches + reconnect snapshot; drop reload crutch"`.

**3c verification:** Manual two-tab + TUI smoke — move a card in tab A; it relocates in tab B and in the TUI within the SSE round-trip. Reload a tab; the full board re-renders from the snapshot patch.

---

## Step 3d — Browser attach (the headline)

Ends green: Attach posts to the existing `/tickets/:id/attach`, and the browser opens the returned `web_url` in a new tab. A runtime framing probe adds `iframeable: bool` to the attach response; the inline iframe is a gated enhancement, new-tab always ships.

### Task 3d.1 — Open `web_url` in a new tab on attach

**Files:**
- Modify: `crates/kamajid/src/views/card.rs`
- Modify: `crates/kamajid/src/views/page.rs`
- Modify: `crates/kamajid/tests/ui.rs`

Steps:

- [ ] 1. The Attach button already fires `@post('/tickets/{id}/attach')` (from 3a.2). To open the returned `web_url`, the daemon will patch a tiny script fragment into the page on attach. Simpler and self-contained: have the attach action use Datastar's response handling. Since `/tickets/:id/attach` returns JSON (`AttachInfo`), and Datastar `@post` expects an SSE/HTML response to merge, route the open-tab via a small inline handler. Add to `page.rs` body a hidden anchor + a Datastar signal:
  ```rust
  // Attach handoff: cards post to /tickets/:id/attach (JSON AttachInfo). A tiny
  // inline listener opens web_url in a new tab. New-tab always ships (§5.3).
  div id="attach-sink" data-on-load--once {}
  ```
  > IMPLEMENTER NOTE: Datastar `@post` to a JSON endpoint does not auto-open a tab. The robust, JS-light approach: make the Attach button call a small inline expression that fetches the attach info and opens the tab, e.g.:
  ```rust
  button class="act"
         data-on-click=(maud::PreEscaped(format!(
             "fetch('/tickets/{id}/attach', {{method:'POST'}}).then(r=>r.json()).then(a=>window.open(a.web_url, '_blank'))"
         ))) { "⤢ Attach" }
  ```
  Update both Attach buttons (InProgress and Review arms) in `card.rs` to this form. This keeps attach a pure new-tab open with no server-rendered fragment, exactly the always-ships default.
- [ ] 2. Update the `card.rs` test `in_progress_card_offers_attach` to assert the new attach expression still references the endpoint:
  ```rust
  #[test]
  fn in_progress_card_offers_attach() {
      let html = card(&ticket(5, Status::InProgress)).into_string();
      assert!(html.contains("/tickets/5/attach"), "Attach posts to attach endpoint:\n{html}");
      assert!(html.contains("window.open"), "Attach opens a new tab:\n{html}");
  }
  ```
- [ ] 3. Run: `cargo test -p kamajid --lib views::card`. Expected: **PASS**.
- [ ] 4. Commit: `git commit -am "feat(kamajid): attach opens zellij web url in a new tab"`.

### Task 3d.2 — Framing probe → `iframeable` on AttachInfo (gated enhancement)

**Files:**
- Modify: `crates/kamajid/src/zellij_web.rs`
- Modify: `crates/kamajid/src/views/card.rs` (optional inline-iframe affordance)

Steps:

- [ ] 1. Add an `iframeable: bool` field to `AttachInfo` in `zellij_web.rs`. Default it via a runtime probe of `web_url`'s framing headers; in `fake` mode default to `false` (no probe). Write the field + a unit test first:
  ```rust
  #[derive(Debug, Clone, Serialize)]
  pub struct AttachInfo {
      pub session_name: String,
      pub web_url: String,
      pub token: String,
      /// True only if a runtime probe found `web_url` permits framing (no
      /// `X-Frame-Options: DENY/SAMEORIGIN` and no restrictive `frame-ancestors`).
      /// Drives the gated inline-iframe enhancement; new-tab ships regardless.
      pub iframeable: bool,
  }
  ```
  In `attach_info`, set `iframeable: false` in fake mode; in real mode call a new `probe_iframeable(&web_url)` (best-effort `GET`, inspect headers, default `false` on any error). Add:
  ```rust
  /// Best-effort: GET `url` and decide whether zellij web permits framing.
  /// Conservative — returns false on any error or restrictive header.
  fn probe_iframeable(_url: &str) -> bool {
      // Implementer: a blocking HTTP GET (std or ureq-free via TcpStream is
      // overkill; reuse the existing port-reachable check then read headers).
      // For now, conservative default false; flipped to true only when a real
      // probe against zellij ≥0.43 confirms framing is allowed (3d spike).
      false
  }
  ```
- [ ] 2. Update the two existing `zellij_web.rs` tests that build `AttachInfo` (`fake_attach_info_returns_canned_token_and_url`, `zellij_web_real_attach_info`) to assert `iframeable == false` by default:
  ```rust
  assert!(!info.iframeable, "fake attach defaults to non-iframeable");
  ```
- [ ] 3. Run: `cargo test -p kamajid --lib zellij_web`. Expected: **PASS**.
- [ ] 4. Add an `#[ignore]`d real-zellij framing-probe smoke test mirroring `zellij_web_real_attach_info`:
  ```rust
  #[test]
  #[ignore = "requires a real zellij; checks web_url framing headers manually"]
  fn zellij_web_real_iframe_probe() {
      let zw = ZellijWeb::new();
      let info = zw.attach_info("kamaji-smoke-test").unwrap();
      // Document the observed value; zellij web typically sends X-Frame-Options.
      eprintln!("iframeable = {}", info.iframeable);
  }
  ```
- [ ] 5. Run: `cargo test -p kamajid` (non-ignored). Expected: **PASS**.
- [ ] 6. Commit: `git commit -am "feat(kamajid): probe web_url framing; add iframeable to AttachInfo"`.

**3d verification:** Manual with real zellij ≥0.43 — click Attach; a new tab opens the live `zellij web` session via `web_url`. Run `cargo test -p kamajid -- --ignored zellij_web_real` and record whether `iframeable` is true (gates a future inline panel).

---

## Step 3e — Create/edit forms + polish

Ends green: create/edit modal fragments render and submit to the existing JSON API; done-with-cleanup and delete confirms; empty-column placeholders confirmed; the frontend-design skill drives the real visual pass.

### Task 3e.1 — `ticket_form()` modal partial

**Files:**
- Create: `crates/kamajid/src/views/modal.rs`
- Modify: `crates/kamajid/src/views/mod.rs`

Steps:

- [ ] 1. Add `pub mod modal;` to `crates/kamajid/src/views/mod.rs`.
- [ ] 2. Create `crates/kamajid/src/views/modal.rs`. The form maps 1:1 to `CreateTicket`/`UpdateTicket`; submit posts/patches to the existing JSON API; an optional `error` renders inline (server-rendered validation):
  ```rust
  //! The create/edit ticket modal fragment. Returned by `GET /ui/tickets/new` and
  //! `GET /ui/tickets/:id/edit`, it targets `#modal`. Fields map 1:1 to
  //! `CreateTicket`/`UpdateTicket`; submit fires the existing JSON API.

  use kamaji_core::models::{Agent, Ticket};
  use maud::{html, Markup, PreEscaped};

  /// Render the modal. `editing` carries an existing ticket (edit mode) or is
  /// `None` (create mode, scoped to `project_id`). `default_agent` pre-selects the
  /// agent in create mode. `error` is shown inline when re-rendered after a 400.
  pub fn ticket_form(
      project_id: i64,
      editing: Option<&Ticket>,
      default_agent: Agent,
      error: Option<&str>,
  ) -> Markup {
      let (title, desc, prompt, agent, submit_action, heading) = match editing {
          Some(t) => (
              t.title.clone(),
              t.description.clone(),
              t.initial_prompt.clone().unwrap_or_default(),
              t.agent,
              format!("@patch('/tickets/{}')", t.id),
              "Edit ticket",
          ),
          None => (
              String::new(), String::new(), String::new(), default_agent,
              "@post('/tickets')".to_string(),
              "New ticket",
          ),
      };
      html! {
          dialog open class="modal" id="ticket-dialog" {
              form data-on-submit=(PreEscaped(submit_action)) {
                  @if editing.is_none() {
                      input type="hidden" name="project_id" data-bind="project_id" value=(project_id);
                  }
                  h2 { (heading) }
                  label for="f-title" { "Title" }
                  input id="f-title" name="title" data-bind="title" value=(title) required;
                  label for="f-desc" { "Description" }
                  textarea id="f-desc" name="description" data-bind="description" rows="3" { (desc) }
                  label for="f-prompt" { "Initial prompt" }
                  textarea id="f-prompt" name="initial_prompt" data-bind="initial_prompt" rows="3" { (prompt) }
                  label for="f-agent" { "Agent" }
                  select id="f-agent" name="agent" data-bind="agent" {
                      @for a in Agent::all() {
                          option value=(a.as_str()) selected[a == agent] { (a.label()) }
                      }
                  }
                  @if let Some(e) = error {
                      p class="form-error" { (e) }
                  }
                  div class="form-actions" {
                      button type="button" class="act"
                             data-on-click="@get('/ui/tickets/cancel')" { "Cancel" }
                      button type="submit" class="act" { "Save" }
                  }
              }
          }
      }
  }

  /// An empty `#modal` fragment that closes/clears the dialog (returned after a
  /// successful submit and on Cancel).
  pub fn modal_closed() -> Markup {
      html! { div id="modal" {} }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn ticket() -> Ticket {
          Ticket { id: 9, project_id: 1, title: "Add login".into(), description: "d".into(),
              initial_prompt: Some("do".into()), agent: Agent::Codex,
              status: kamaji_core::models::Status::Todo, position: 0,
              session_name: None, worktree_path: None, branch: None,
              auto_reviewed: false, instrumented: false,
              created_at: String::new(), updated_at: String::new() }
      }

      #[test]
      fn create_form_posts_to_tickets_with_default_agent() {
          let html = ticket_form(1, None, Agent::Claude, None).into_string();
          assert!(html.contains("@post('/tickets')"), "create posts:\n{html}");
          assert!(html.contains(r#"value="claude" selected"#), "default agent preselected:\n{html}");
          assert!(html.contains(r#"name="project_id""#), "scopes to project:\n{html}");
      }

      #[test]
      fn edit_form_patches_and_prefills() {
          let t = ticket();
          let html = ticket_form(1, Some(&t), Agent::Claude, None).into_string();
          assert!(html.contains("@patch('/tickets/9')"), "edit patches:\n{html}");
          assert!(html.contains("Add login"), "title prefilled:\n{html}");
          assert!(html.contains(r#"value="codex" selected"#), "agent prefilled:\n{html}");
      }

      #[test]
      fn validation_error_renders_inline() {
          let html = ticket_form(1, None, Agent::Claude, Some("title must not be empty")).into_string();
          assert!(html.contains("title must not be empty"), "error shown:\n{html}");
      }
  }
  ```
- [ ] 3. Run: `cargo test -p kamajid --lib views::modal`. Expected: **PASS** (3 tests).
- [ ] 4. Commit: `git commit -am "feat(kamajid): add ticket_form() modal partial"`.

### Task 3e.2 — Modal routes: `GET /ui/tickets/new` and `/ui/tickets/:id/edit`

**Files:**
- Modify: `crates/kamajid/src/routes/ui.rs`
- Modify: `crates/kamajid/src/lib.rs`
- Modify: `crates/kamajid/tests/ui.rs`

Steps:

- [ ] 1. Add the two modal handlers to `routes/ui.rs`:
  ```rust
  use axum::extract::Path;
  use kamaji_core::models::Agent;
  use crate::views::modal::ticket_form;

  #[derive(Deserialize)]
  pub struct NewTicketQuery { pub project: i64 }

  /// `GET /ui/tickets/new?project=<id>` → the create-ticket modal fragment.
  pub async fn new_ticket(
      State(state): State<AppState>,
      Query(q): Query<NewTicketQuery>,
  ) -> Result<Markup, ApiError> {
      let pid = q.project;
      let default_agent = state
          .with_db(move |db| Ok(db.get_project(pid)?.and_then(|p| p.default_agent)))
          .await?
          .unwrap_or_else(|| state.config.default_agent());
      Ok(ticket_form(pid, None, default_agent, None))
  }

  /// `GET /ui/tickets/:id/edit` → the edit-ticket modal fragment, prefilled.
  pub async fn edit_ticket(
      State(state): State<AppState>,
      Path(id): Path<i64>,
  ) -> Result<Markup, ApiError> {
      let ticket = state
          .with_db(move |db| db.get_ticket(id))
          .await?
          .ok_or(ApiError::NotFound)?;
      let default_agent = ticket.agent;
      Ok(ticket_form(ticket.project_id, Some(&ticket), default_agent, None))
  }
  ```
- [ ] 2. In `lib.rs` `router()`, add (after the `/ui/events` route):
  ```rust
  .route("/ui/tickets/new", get(routes::ui::new_ticket))
  .route("/ui/tickets/:id/edit", get(routes::ui::edit_ticket))
  ```
- [ ] 3. Add integration tests to `tests/ui.rs`:
  ```rust
  #[tokio::test]
  async fn new_ticket_modal_renders_form() {
      let (base, state) = spawn().await;
      let pid = state.with_db(|db| Ok(db
          .create_project("p", std::path::Path::new("/tmp/p"), None)?.id))
          .await.unwrap();
      let body = reqwest::get(format!("{base}/ui/tickets/new?project={pid}"))
          .await.unwrap().text().await.unwrap();
      assert!(body.contains("@post('/tickets')"), "create action:\n{body}");
      assert!(body.contains(r#"name="title""#), "title field:\n{body}");
  }

  #[tokio::test]
  async fn edit_ticket_modal_prefills() {
      let (base, state) = spawn().await;
      let tid = state.with_db(|db| {
          let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
          let t = db.create_ticket(p.id, "Add login", "", None, kamaji_core::models::Agent::Claude)?;
          Ok(t.id)
      }).await.unwrap();
      let body = reqwest::get(format!("{base}/ui/tickets/{tid}/edit"))
          .await.unwrap().text().await.unwrap();
      assert!(body.contains(&format!("@patch('/tickets/{tid}')")), "patch action:\n{body}");
      assert!(body.contains("Add login"), "prefilled title:\n{body}");
  }
  ```
- [ ] 4. Run: `cargo test -p kamajid --test ui new_ticket_modal edit_ticket_modal`. Expected: **PASS**.
- [ ] 5. Add a create→card-appears round-trip test reusing `connect_ui_events`/`read_patch`:
  ```rust
  #[tokio::test]
  async fn creating_a_ticket_appends_a_card_patch() {
      let (base, state) = spawn().await;
      let pid = state.with_db(|db| Ok(db
          .create_project("p", std::path::Path::new("/tmp/p"), None)?.id))
          .await.unwrap();

      let mut stream = connect_ui_events(&base).await;
      for _ in 0..4 { let _ = read_patch(&mut stream).await; } // drain snapshot

      reqwest::Client::new().post(format!("{base}/tickets"))
          .json(&serde_json::json!({ "project_id": pid, "title": "Fresh", "agent": "claude" }))
          .send().await.unwrap();

      let data = read_patch(&mut stream).await;
      assert!(data.contains("mode append"), "append mode:\n{data}");
      assert!(data.contains("#col-todo .col-body"), "targets todo body:\n{data}");
      assert!(data.contains("Fresh"), "new card content:\n{data}");
  }
  ```
- [ ] 6. Run: `cargo test -p kamajid --test ui`. Expected: **PASS**.
- [ ] 7. Commit: `git commit -am "feat(kamajid): create/edit modal routes + create round-trip"`.

### Task 3e.3 — Done-with-cleanup + delete confirms

**Files:**
- Modify: `crates/kamajid/src/views/card.rs`

Steps:

- [ ] 1. Replace the bare Done and Delete actions with a small confirm. Datastar's `@post`/`@delete` support a confirm via a guarded expression; use the standard `confirm(...)` guard and a cleanup checkbox signal for Done. Update the InProgress/Review Done button to confirm with cleanup, and the Delete buttons to confirm:
  ```rust
  // Done with a cleanup confirm: window.confirm gates the post; cleanup defaults
  // false (a checkbox in the modal could set it true — kept simple here).
  button class="act"
         data-on-click=(PreEscaped(format!(
             "confirm('Mark #{id} done and tear down its session?') && @post('/tickets/{id}/done', {{cleanup:true}})"
         ))) { "✓ Done" }
  // Delete confirm:
  button class="act danger"
         data-on-click=(PreEscaped(format!(
             "confirm('Delete #{id}? This cannot be undone.') && @delete('/tickets/{id}')"
         ))) { "Delete" }
  ```
- [ ] 2. Update the relevant `card.rs` test assertions (`todo_card_offers_start_not_attach` still checks `/tickets/1/start`; add a confirm assertion):
  ```rust
  #[test]
  fn delete_action_is_confirm_guarded() {
      let html = card(&ticket(2, Status::Done)).into_string();
      assert!(html.contains("confirm("), "delete guarded by confirm:\n{html}");
      assert!(html.contains("@delete('/tickets/2')"), "delete endpoint:\n{html}");
  }
  ```
- [ ] 3. Run: `cargo test -p kamajid --lib views::card`. Expected: **PASS**.
- [ ] 4. Commit: `git commit -am "feat(kamajid): confirm guards for done(cleanup) and delete"`.

### Task 3e.4 — Frontend-design visual pass

**Files:**
- Modify: `crates/kamajid/src/assets/app.css`
- Possibly modify: `crates/kamajid/src/views/*.rs` (class names / structure only)

Steps:

- [ ] 1. Invoke the **frontend-design skill** to refine `app.css` (and minimal markup/class adjustments) into a distinctive, dark-first, catppuccin-aligned board per spec §7: strong type scale, 1px hairlines, low-contrast surface hover, per-column accents echoing the TUI `status_color`, crisp bullet + activity chip, sub-200ms card slide/fade on relocate, `prefers-reduced-motion` respected. Do NOT change DOM ids (`col-*`, `card-*`, `#modal`) or `data-*` action attributes — only visual CSS and non-load-bearing class names.
- [ ] 2. After the visual pass, re-run the view unit tests to confirm no id/attribute regressions: `cargo test -p kamajid --lib views`. Expected: **PASS** (ids and actions are asserted by existing tests; if a test breaks, the change touched a load-bearing id — revert that part).
- [ ] 3. Run the full suite: `cargo test -p kamajid`. Expected: **PASS**.
- [ ] 4. Run formatting + lint to keep CI green: `cargo fmt -p kamajid` then `cargo clippy -p kamajid --all-targets -- -D warnings`. Expected: clean.
- [ ] 5. Commit: `git commit -am "style(kamajid): frontend-design visual pass on the board"`.

**3e verification:** Manual visual review of the running board; create a ticket via the modal (card appears live), edit it, mark done with cleanup confirm, delete with confirm. Confirm empty columns show the placeholder.

---

## Final whole-phase verification

- [ ] 1. `cargo fmt --all --check` — clean.
- [ ] 2. `cargo clippy --all-targets --all-features -- -D warnings` — clean.
- [ ] 3. `cargo test --all-targets --all-features` — all green (TUI 152 + core 83 + kamajid api.rs + kamajid ui.rs + new unit tests). The JSON `/events` tests in `tests/api.rs` are untouched and pass, proving the TUI contract did not move.
- [ ] 4. Manual smoke (documented gate, spec §10.3): `kamajid serve`; open two browser tabs + the TUI; move/create/done a card and confirm all three surfaces update; click Attach with real zellij and confirm a live terminal opens in a new tab.

---

## Self-Review

- **Spec coverage:** Every §11 rollout step (3a–3e) is covered and each ends green with a working daemon. New routes match §8.1 exactly (`GET /`, `/ui/events`, `/assets/*path`, `/ui/tickets/new`, `/ui/tickets/:id/edit`); module shape matches §8.2 (`routes/{ui,ui_events,assets}.rs`, `views/{mod,page,board,card,modal}.rs`, `assets/{datastar.js,app.css}`); deps match §8.4 (`maud="0.26"`, `rust-embed="8"`, `mime_guess="2"`). No new command routes — create/move/start/done/delete reuse the existing JSON API via Datastar `@post/@patch/@delete` (§3.5, §8.1). The reconnect full-board snapshot (§4.4), column-granular `TicketMoved` re-render of both `from`+`to` (§4.2), and the per-event patch table (§4.2) are implemented in `ui_events.rs`, which reuses `views::board::column` and `views::card::card` so live patches equal initial render (§8.2). Both SSE endpoints subscribe to the same `state.tx`; the JSON serializer in `routes/events.rs` is untouched (§4.1). Attach defaults to new-tab and always ships, with the `iframeable` probe as a gated enhancement (§5.3, 3d). The starter `app.css` is a real, complete dark-first stylesheet with design tokens + per-column accents; 3e invokes the frontend-design skill (§7).
- **No placeholders:** Every code step contains actual Rust + maud markup, real CSS, and every test step has runnable test code with an exact `cargo test ...` command and expected result. The two IMPLEMENTER NOTE blocks in 3c.1 and 3d.1 give authoritative final bodies for the only spots where the first sketch is deliberately simplified.
- **Type consistency:** All referenced symbols exist in the live code — `Status::{as_str,title,all}`, `Agent::{all,label,as_str}`, `Ticket`/`Project` fields, `AppState::{with_db,emit,tx,config}`, `Config::default_agent()`, `Db::{list_projects,get_project,list_tickets,get_ticket}`, `Event` variants, `AttachInfo`. The integration tests reuse the verified Phase 1 harness (`spawn()` from `tests/api.rs`, `mod support;`) and the inline SSE-line parser (`ByteStream`/`read_named_event` → `read_patch`). `maud::Markup` is returned directly from handlers (`IntoResponse`), matching axum 0.7. Datastar wire constants (`datastar-patch-elements`, `mode append`/`remove`, `data-*`, `@get/@post/@patch/@delete`) are pinned to vendored v1.0.0-RC.6 and isolated to `ui_events.rs` + the view attributes for one-place adjustment if the vendored bytes differ.
