# Dense Board Redesign — Design

> **Status:** Approved direction (prototype-validated). Umbrella design; ships as
> a sequential **foundation** plus **5 parallel feature slices**, each its own
> issue → plan → PR.

**Goal:** Reskin and restructure the browser board into the "dense pro-tool"
visual language (Linear/Height energy) with a collapsible Slack-style workspace
sidebar, restyled modals, drag-and-drop column moves, and live search — matching
the validated static prototype.

**Visual reference:** `docs/superpowers/specs/assets/2026-06-06-dense-redesign-prototype.html`
(self-contained; open in a browser. Every screen/modal/interaction below is in it.)

**Tech stack (unchanged):** maud server-rendered views + vendored Datastar
v1.0.0-RC.6 + a single SSE stream (`/ui/events`). Rust/axum daemon (`kamajid`).

---

## 1. What carries over unchanged (hard invariants)

These are load-bearing and must survive the redesign:

- **SSE patch targets.** Live updates target `#card-<id>` and
  `#col-<status.as_str()>`. The SSE serializer (`routes/ui_events.rs`) **reuses
  `views::board::column` and `views::card::card`**, so card/column markup changes
  flow to live patches automatically — but the stable ids MUST be preserved.
- **The JSON command API is the writer.** UI actions fire the existing JSON
  endpoints (`POST /tickets`, `/tickets/:id/move`, `/done`, `DELETE`, …); the
  authoritative board update arrives over `/ui/events`. No new command logic in
  views.
- **Datastar RC.6 colon syntax.** Parameterised bindings use a colon
  (`data-on:click`, `data-on:submit`), and the once-on-load hook is `data-init`.
  The hyphen form is silently ignored. An action's 2nd arg is request *options*,
  not a body — body-carrying commands use an explicit `fetch()`.
- **Asset revalidation.** Assets are served `Cache-Control: no-cache` + ETag, so
  a daemon restart serves fresh CSS/JS. New CSS/JS files inherit this.

## 2. Design system (the foundation everything builds on)

### 2a. Tokens
Port the prototype's `:root` palette (catppuccin-derived, dark): surfaces
`--bg/--bg-2/--surface/--surface-2`, hairlines `--hair/--hair-2`, text
`--text/--dim/--muted/--faint`, status accents `--todo/--prog/--review/--done`,
`--accent`, `--danger`, the rail tones `--rail/--rail-hair`, and the `--mono`/
`--font` stacks. These match the TUI theme (`crates/kamaji/src/theme.rs`).

### 2b. CSS architecture — split `app.css` into per-component files
Today there is one `app.css`. The redesign **splits it** so parallel slices own
disjoint files. The page `<head>` links each:

```
/assets/tokens.css     (foundation) — :root tokens + reset + base typography
/assets/layout.css     (foundation) — shell: rail + main + topbar, board grid
/assets/sidebar.css    (foundation) — the workspace rail
/assets/modal.css       (foundation chrome + ticket-slice extras) — overlay/dialog/fields/seg/buttons
/assets/board.css       (board slice) — columns, cards, status stripes, dnd states
/assets/search.css      (search slice)
/assets/terminal.css    (terminal slice)
```

`rust-embed` already globs `src/assets/`, so new files are served automatically;
only the `<link>` list in `page.rs` grows. This is the key parallelism enabler.

### 2c. Shared modal chrome (foundation)
One reusable chrome — `.modal-overlay`/`.modal`/`.modal-head`/`.modal-title`/
`.modal-idpill`/`.modal-close`/`.modal-body`/`.field`/`.seg`/`.modal-foot`/
`.btn`/`.btn-primary`/`.btn-danger` — fixed centered overlay over a dimmed,
blurred board. All three modals compose from these classes (see prototype).
This fixes today's bug where the dialog rendered in-flow below the board.

## 3. App shell (foundation)

`page.rs` becomes a flex row: a **workspace rail** (`<aside>`) + a `.main` column
(topbar + board). Collapse state lives on `<body class="rail-open">`.

- **Workspace rail (Slack-style).** Rounded-square project tiles with initials +
  a per-project gradient; the active project gets the white left-indicator pill +
  ring; a "needs attention" count badge rides the active tile. A wordmark +
  collapse toggle sit in the rail header (a centered hamburger when collapsed, a
  chevron when expanded — the toggle lives **inside** the rail in both states).
  "+ Add project" pinned at the bottom. Project selection moves OUT of the
  top-bar dropdown into the rail. Tiles render from the real project list;
  clicking navigates `/?project=<id>`.
- **Topbar.** Current-project breadcrumb, a **search slot** (filled by the search
  slice), and the primary "+ New" button.
- **Board grid.** Four columns, dense gutters, `align-content:start`.

New file: `views/sidebar.rs` (the rail partial, fed the project list + active id).

## 4. Feature slices (parallel, after foundation)

Each slice owns disjoint files. Shared touch-points (`lib.rs` router,
`routes/ui.rs`) take only additive route lines — trivial merges.

### Slice 1 — Board, cards & drag-and-drop
**Owns:** `views/board.rs`, `views/card.rs`, `assets/board.css`,
`assets/board-dnd.js` (+ link in page head).
- Dense card: status-coloured left stripe, `#id` (mono) + title, session dot
  (●/○ live/idle), agent label, active/idle chip; **actions reveal on hover**.
- Column header: status dot + UPPERCASE title + count pill.
- **Remove the Move / ↩ In Progress buttons.** Column moves happen by
  **drag-and-drop**: cards `draggable`, `.col-body` drop zones, drop fires the
  existing `POST /tickets/:id/move {target}` via `fetch()`; the authoritative
  re-render arrives over SSE. Drag affordances: grab cursor, dragging dims, drop
  target highlights.
- Keep `Start` / `✓ Done` / `Attach` / `Edit` / `Delete` as explicit buttons
  (session side-effects). **Preserve `#card-<id>` / `#col-<status>` ids.**
- **Drag into Done sets status only** (via `/move`); it does NOT tear down the
  session. The `✓ Done` button remains the path that tears down + cleans up.

> **modal.css ownership note:** the foundation creates `modal.css` (shared
> chrome); the ticket-modals slice (Slice 2) is the *only* later slice that edits
> it (adding `.check`/lock/live-dot). Because the foundation merges first, this is
> sequential, not a parallel conflict — the 5 parallel slices touch disjoint CSS.

### Slice 2 — Ticket modals (new + edit)
**Owns:** `views/modal.rs`, `assets/modal.css` (the `.check`, lock, live-dot
extras on top of the shared chrome), the `edit_ticket` handler in `routes/ui.rs`.
- Both modals compose from the shared chrome; **agent picker = segmented control**
  (`.seg`), not a dropdown.
- **New ticket:** add a "**Start the agent now, in the background**" checkbox
  (default decision: **unchecked** — create in Todo unless ticked; ticking
  starts the session immediately with the initial prompt). On submit it calls
  `POST /tickets` and, if ticked, `POST /tickets/:id/start`.
- **Edit ticket:** when the ticket **has a session** (started), the **Initial
  prompt is read-only** (🔒 locked tag + hint: only used at session creation);
  editable otherwise. A live-dot in the header when a session is running.
  Delete pinned left in the footer; Cancel + Save right.

### Slice 3 — New-project modal
**Owns:** new `views/project_form.rs`, a `GET /ui/projects/new` route
(`routes/ui.rs` + `lib.rs`), "+ Add project" wiring in the rail.
- Reuses the shared modal chrome (no new CSS). Fields: Name (required), Root
  directory (mono path input + hint), Default agent (segmented). Submit →
  `POST /projects`; the new tile appears in the rail.

### Slice 4 — Search / filter
**Owns:** `assets/search.css`, `assets/search.js`, the topbar search markup.
- Live in-place filter as you type; non-matches hide across all columns.
- Match scope: title + `#id` + agent (NOT action labels). Highlight the matched
  substring (`<mark>`). Column counts recount to matches; a "N results" total;
  per-column "No matches"; active-filter chrome (magnifier, accent border, ✕
  clear). `/` focuses, `Esc` clears. Client-side (board is already loaded).

### Slice 5 — Terminal panel restyle
**Owns:** `views/terminal.rs`, `assets/terminal.css`.
- The near-fullscreen terminal-window look: title bar (live-dot + `#id` + task
  name left, **✕ close upper-right**), black terminal body (the real iframe),
  zellij-style status bar. No Escape-to-close (terminal owns the keyboard).
  Behaviour/proxy unchanged — view + CSS only.

## 5. Sequencing & parallelism

```
  ┌─────────────────────────────┐
  │  Foundation (blocks all)    │   tokens, CSS split, shell + rail, modal chrome
  └──────────────┬──────────────┘
                 │  (merged to main)
   ┌──────┬──────┼──────┬──────┬──────┐
   ▼      ▼      ▼      ▼      ▼      ▼
 Board  Modals  New-   Search Terminal      ← 5 slices, parallel
 +DnD   (tkt)   proj
```

- **Foundation merges first.** Each slice branches off the updated `main`.
- Slices own disjoint view + CSS files → no heavy conflicts. The router/`ui.rs`
  additive lines are trivial; branch-protection (up-to-date-before-merge) means
  each slice rebases/merges on main as it lands, resolving any 1-liner.
- Execution: subagent-driven per the repo workflow (fresh implementer + spec +
  quality review per task), each slice its own worktree/branch/PR.

## 6. Testing

- View partials: maud render assertions (as today) — assert the new classes,
  the preserved `#card-/#col-` ids, the colon binding syntax, modal `#modal`
  rooting, segmented agent picker, read-only-prompt-when-started, DnD `target`
  body, search markup.
- Daemon: existing integration tests stay green; add coverage for the
  new-project UI route and the edit read-only path.
- **Browser smoke** (the existing CI job) must pass: SSE opens, a card moves
  (now via DnD), a modal opens/closes, search filters. Extend it per slice.
- CI unchanged: fmt + clippy + test + Windows build + browser smoke, on the
  workspace.

## 7. Out of scope (future, separate cycles)

⌘K command palette; mobile/responsive pass; settings UI; light theme. The
prototype's board/sidebar/modals/dnd/search are the committed surface here.
