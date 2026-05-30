# kamaji — Phase 3: Browser Board (the headline)

- **Date:** 2026-05-30
- **Status:** Approved (design)
- **Author:** Victor Alveflo
- **Parent spec:** `docs/superpowers/specs/2026-05-27-browser-first-pivot-design.md` (§6, §8 Phase 3)
- **Companion spec:** `docs/superpowers/specs/2026-05-30-phase-1-kamajid-design.md` (the daemon this UI is served from)
- **Precondition:** Phase 2 merged (TUI is a daemon client; auto-spawn works), or at minimum the Phase 1 daemon API + `/events` SSE stable. Phase 3 adds only *new routes* to `kamajid`; it does not change the existing JSON API or event taxonomy.

> **Finalization note.** Drafted autonomously (the owner delegated the spec→implementation decision). The three judgment calls in "§13 Owner decisions" were resolved with the recommended answers and are baked into the Decisions table and body below.

## 1. Overview

Phase 3 makes `kamajid` serve a **web board** — the first-class, better-looking
surface the whole browser-first pivot is for. The board is **server-rendered
HTML** (`maud` templates) made reactive with **Datastar** over the daemon's
existing SSE stream. It runs on the *same* daemon, the *same* port, the *same*
domain logic, and the *same* event broadcaster the TUI uses. There is no separate
frontend app, no SPA, no JS build pipeline — that is the entire point of
"Approach A: Rust all the way."

The browser renders the four-column Kanban board (Todo → In Progress → Needs
attention → Done), lets the user create/edit/move/start/done/delete tickets, and
— the headline feature — **attaches to a ticket's agent terminal in the browser**
by handing off to `zellij web` at the `web_url` already returned by
`POST /tickets/:id/attach`. Every board mutation fans out over `/events` to all
connected browsers (and the TUI), so two tabs and a terminal stay in lockstep.

### Goals
- A polished, distinctive Kanban board served at `GET /` by `kamajid`, with no
  network dependency (Datastar vendored, CSS owned).
- **Live reactivity** driven by the *existing* `ticket.*` / `session.*` SSE
  events — a card another client moves, or that the auto-review poll loop moves
  to "Needs attention," relocates in this browser within the SSE round-trip.
- All board commands reuse the **existing REST API** — Phase 3 adds *no* new
  command endpoints, only HTML-serving routes.
- **Attach** opens the agent's live terminal via `zellij web`, using the
  `AttachInfo { session_name, web_url, token }` the daemon already produces.
- Server-rendered → straightforwardly testable with HTML render-assertions.

### Non-goals (Phase 3)
- Remote / multi-user / auth / TLS (deferred; binds `127.0.0.1`, §6).
- A bespoke browser terminal — `zellij web` owns it.
- Any heavy JS framework or build step. The only JS is the vendored Datastar
  runtime plus a tiny inline glue snippet.
- TUI work (Phase 2) and browser-first feature expansion (Phase 4+).
- Editing config from the browser (`GET /config` only).
- Rich project management — a minimal switcher + create affordance only (§3.2).

## 2. Decisions

| Question | Decision |
|----------|----------|
| Where the UI is served | The **same `kamajid` daemon / same port** (`127.0.0.1:8755`). New routes `GET /`, `GET /assets/*`, plus a browser-oriented SSE endpoint. |
| Templating | **`maud`** — compile-time-checked HTML in Rust; one crate. |
| Reactivity library | **Datastar**, **vendored** as a static asset (no CDN) — a localhost tool must work offline. |
| SSE for the browser | **A new `GET /ui/events` that emits HTML fragments** (Datastar element patches), driven by the *same* broadcast channel. The existing `GET /events` stays the JSON API for the TUI, unchanged. |
| Fragment strategy | On each event, the daemon **re-renders the affected column(s)** server-side and patches them by id. Coarse-but-correct beats per-card diffing for a board this small. |
| Commands from the browser | **Reuse the existing JSON REST API** via Datastar actions. No new command routes. |
| Move interaction | **Buttons / a move menu**, not drag-and-drop, for v1. DnD is a Phase 4 polish item. |
| Create / edit ticket | A **server-rendered modal** whose form posts to the API; the response closes the modal and the SSE fan-out inserts the card. |
| Attach in the browser | **Open `web_url` in a new tab/window by default**; an inline iframe panel only as a progressive enhancement *gated on a runtime framing-header check*. |
| Styling | A **single hand-written `app.css`** served as a static asset. No CSS framework; design tokens (CSS custom properties) give a small utility layer. |
| Auth | **None** (localhost). zellij web has its own token auth; the board hands the user `web_url` and zellij web's login page consumes the token. Seams noted for a future remote mode. |
| New crate deps | `maud`, `rust-embed` (embed assets in the binary — single self-contained `kamajid`), `mime_guess`. |
| Visual direction | **Dark-first palette echoing the TUI's catppuccin-style themes** + per-column status colors, so browser + TUI read as one product. The 3e frontend-design pass refines. |

## 3. Pages, views & interactions

A **single-page board** that mutates in place via Datastar — *not* a JS SPA:
every fragment is rendered by Rust on the server.

### 3.0 Information architecture

```
 ┌───────────────────────────────────────────────────────────────────────┐
 │  kamaji            [ project: acme-api ▾ ]                  [+ Ticket]  │  ← top bar
 ├──────────────┬──────────────────┬──────────────────┬──────────────────┤
 │  Todo  · 2   │  In Progress · 1 │  Needs attention·1│  Done · 3        │  ← column heads
 ├──────────────┼──────────────────┼──────────────────┼──────────────────┤
 │ ┌──────────┐ │ ┌──────────────┐ │ ┌──────────────┐ │ ┌──────────────┐ │
 │ │○ #3 Login│ │ │● #1 Refactor │ │ │● #5 Flaky    │ │ │  #2 Bump deps│ │  ← cards
 │ │ claude   │ │ │ claude ·active│ │ │ claude ·idle │ │ │              │ │
 │ │ [▸ start]│ │ │ [⤢ attach]   │ │ │ [⤢ attach]   │ │ │              │ │
 │ └──────────┘ │ │ [⠿ move]     │ │ │ [✓ done]     │ │ └──────────────┘ │
 │              │ └──────────────┘ │ └──────────────┘ │                  │
 └──────────────┴──────────────────┴──────────────────┴──────────────────┘
```

Conceptually consistent with the TUI board (`crates/kamaji/src/ui/board.rs`) and
original design §7: the same four columns, the same `Status::title()` labels —
**Review renders as "Needs attention"** (DB key stays `review`). The `●`/`○`
session bullet and activity coloring carry over (§3.3).

### 3.1 The board page — `GET /`

A full HTML document:
- `<head>`: `<link rel="stylesheet" href="/assets/app.css">`, the vendored
  Datastar `<script type="module" src="/assets/datastar.js">`, a viewport meta.
- `<body data-on-load="@get('/ui/events')">` — on load, Datastar opens the SSE
  connection to the browser event stream and applies patches as events arrive
  (§4). Background tabs keep the stream live.
- A **top bar**: wordmark, a **project switcher** (over `GET /projects`), a
  **"+ Ticket"** button (opens the create modal).
- The **board**: four `<section class="column" id="col-…">` (one per `Status`),
  each rendered by a `column(status, &[Ticket])` maud partial. Stable DOM ids
  (`col-todo`, `col-in_progress`, `col-review`, `col-done`) keyed off
  `Status::as_str()` — the patch targets for SSE fragments (§4).
- A **modal mount point** (`<div id="modal"></div>`).

Which project shows comes from `?project=<id>` (defaulting to the first project,
or a last-used cookie — optional).

### 3.2 Cards & per-card actions

A `card(&Ticket, Option<SignalLevel>)` partial renders each ticket with id
`card-<id>`: the session bullet (`●`/`○`), `#<id>` + title, the agent label
(`Agent::label()`) and an activity chip ("active"/"idle") for in-progress/
needs-attention cards, and **context actions** by state:
- Todo → **Start** (`@post('/tickets/{id}/start')`), **Edit**, **Delete**.
- In Progress → **Attach** (§5), **Move** (menu → `@post('…/move')`), **Edit**,
  **Done**.
- Needs attention → **Attach**, **Move back to In Progress**, **Done**, **Edit**.
- Done → **Delete** (and a nice-to-have "reopen" = move to In Progress).

A click on the card body opens the **edit modal** (Title, Description, Initial
Prompt, Agent). All actions are Datastar attributes firing the **existing API**;
the authoritative UI update arrives via SSE (§4 round-trip).

### 3.3 Activity / status semantics (reused from core)

The chip and bullet color mirror the TUI's `bullet_color` (`board.rs`) and
`detect::SignalLevel`: Needs attention → attention styling; In Progress + Active
→ "active"; Idle/Unknown/non-instrumented → neutral. Per-ticket `SignalLevel`
isn't in the SSE payloads today; Phase 3 treats the arrival of a `session.idle`
event (and the accompanying `ticket.moved`) as the trigger to re-render the card
in its new column with attention styling — no new event types. A live "active
pulse" bullet (without a column change) is deferred (§9; would need a new
`session.active` event).

### 3.4 Create / edit ticket modal

- **Open:** `GET /ui/tickets/new?project=<id>` / `GET /ui/tickets/:id/edit`
  returns a **modal fragment** targeting `#modal` (a `<dialog open>` with the
  form).
- **Fields:** Title (required), Description, Initial Prompt (optional), Agent
  (`<select>` over `Agent::all()`, pre-filled with the project's `default_agent`)
  — map 1:1 to `CreateTicket`/`UpdateTicket`.
- **Submit:** `@post('/tickets')` / `@patch('/tickets/:id')` with the form
  signals as the JSON body. On success the normal handler emits
  `ticket.created`/`ticket.updated`; SSE inserts/updates the card; the submit
  response patches `#modal` to empty (closes the dialog).
- **Validation:** the API returns `400 { kind: "bad_request" }` for an empty
  title; the daemon re-renders the modal fragment *with* the error (validation
  stays server-rendered).

### 3.5 Move, start, done, delete

- **Move** — a per-card menu of the other three columns → `@post('…/move')` with
  `{ "target": "<status>" }`. Server emits `ticket.moved`; SSE relocates the card
  in both columns.
- **Start** — `@post('/tickets/{id}/start')`. Emits `session.started` +
  `ticket.moved` (to In Progress).
- **Done** — `@post('/tickets/{id}/done')` with `{ "cleanup": <bool> }` behind a
  small confirm (cleanup checkbox). Emits `ticket.moved` (to Done) and, when
  cleaned, `session.exited`.
- **Delete** — `@delete('/tickets/{id}')` behind a confirm. Emits
  `ticket.deleted`; SSE removes the card.

## 4. The reactivity model (Datastar + SSE) — the core of this spec

### 4.1 Two SSE endpoints, one broadcast

The daemon has one `broadcast::Sender<Event>` (`AppState.tx`); every mutation
emits to it. Phase 3 adds a **second SSE handler** subscribing to the *same*
channel but framing events as **HTML fragments**:

| Endpoint | Audience | Wire format | Status |
|----------|----------|-------------|--------|
| `GET /events` | TUI (Phase 2 client) | named SSE events, JSON `data:` | **unchanged** |
| `GET /ui/events` | browser (Datastar) | Datastar element-patch SSE events whose `data:` carries rendered **HTML fragments** | **new in Phase 3** |

Both call `state.tx.subscribe()`. The JSON serializer in `routes/events.rs` is
untouched; the new handler (`routes/ui_events.rs`) has its own `Event → SSE
fragment` serializer. Keeping them separate means the TUI's contract never moves
and the browser gets the shape Datastar wants.

**Why HTML fragments over SSE (not client re-render, not client signals):** the
server already owns all state and rendering (maud). Pushing rendered HTML the
client blindly merges by id is the simplest *and* most robust path for a
server-rendered Rust app, and is exactly what Datastar's element-patch SSE is
built for. Client re-fetch doubles round-trips; client signals push rendering
into JS — which Approach A rejects.

### 4.2 What each event renders to

| `Event` | Browser SSE fragment patch |
|---------|----------------------------|
| `TicketCreated(t)` | render `card(t)`, **append** into the target column container |
| `TicketUpdated(t)` | render `card(t)`, **replace** `#card-<id>` |
| `TicketMoved{id,from,to,…}` | re-render **both** affected columns (`#col-<from>`, `#col-<to>`) and **replace** them — moves the card, fixes both counts |
| `TicketDeleted{id}` | **remove** `#card-<id>` |
| `SessionStarted{ticket_id,…}` | re-render `#card-<ticket_id>` (now `●` + Attach) |
| `SessionIdle{ticket_id}` | re-render `#card-<ticket_id>` for attention styling (the loop usually also emitted a `TicketMoved` to Review) |
| `SessionExited{ticket_id,…}` | re-render `#card-<ticket_id>` (bullet → `○`, Attach gone) |

For id-only events the serializer loads the current ticket(s) via `with_db` to
render the fragment — a trivially cheap read on the broadcast path for a
single-user localhost board. `TicketMoved` carries `from`+`to`, so re-rendering
both columns needs only those statuses + their lists. **Column-level granularity
for moves** avoids per-card insert choreography (ordering, siblings,
empty-states) — a few KB per move keeps everything correct.

### 4.3 The command round-trip (one client acts → all clients update)

Moving card #5 In Progress → Needs attention:
1. The Move button fires `@post('/tickets/5/move')` `{"target":"review"}` to the
   **existing** API.
2. `routes/tickets.rs::move_ticket` updates the DB and calls
   `state.emit(Event::TicketMoved { id:5, from:in_progress, to:review, … })`.
3. The event hits the broadcast → every `/ui/events` subscriber renders the
   `#col-in_progress` and `#col-review` fragments → Datastar patches them. Card
   #5 relocates in *all* browsers. The TUI (JSON `/events`) updates as it already
   does.
4. The originating `@post`'s response body is **ignored** by the DOM — the
   authoritative update for *every* client, including the actor, is the SSE patch
   in step 3. Acting and observing tabs run the identical path; no optimistic
   local update, no divergence.

This is the "LiveView feel without LiveView": commands down (REST), deltas up
(SSE → HTML patches), server owns all rendering.

### 4.4 Reconnect & lag

The broadcast is **lossy by design**; Datastar auto-reconnects. On every
`/ui/events` connection the daemon emits a one-shot **full-board patch**
(re-render all four columns) *before* streaming live events. So every
(re)connection self-heals — no gap tracking, no replay buffer. (The TUI re-syncs
via `GET /projects/:id/tickets`.)

### 4.5 Datastar specifics

- The vendored runtime (`/assets/datastar.js`, ESM ~14 KB).
- `data-on-load` opens `@get('/ui/events')`; Datastar treats the response as an
  SSE stream and applies element-patch events automatically.
- Patch modes used: replace-outer (columns, cards), append (new card), remove
  (deleted card) — set per event in §4.2.
- Element identity by `id` (`#card-5`, `#col-review`), rendered stably from
  `Status::as_str()`/`ticket.id`.
- **The exact Datastar event/attribute spelling is pinned to the vendored
  version** at implementation time — we vendor one version and write the
  serializer to match it, so there is no drift (§9).

## 5. Attach in the browser — the headline feature

The daemon already returns `POST /tickets/:id/attach` → `AttachInfo {
session_name, web_url, token }`, where `web_url` is `http://127.0.0.1:8082/<session>`
and zellij web owns the terminal + its own token auth.

### 5.1 Flow
1. The card's **Attach** fires `@post('/tickets/{id}/attach')`. The daemon
   ensures `zellij web` is running (Phase 1 lazy-spawn + token) and returns
   `AttachInfo`.
2. The browser **opens `web_url` in a new tab/window**. zellij web serves its own
   login page there; the `token` is consumed by zellij web's login; the live
   session opens. zellij web persists its auth, so later attaches go straight in.
3. kamaji's job ends at "open the right URL." The terminal-in-browser problem is
   entirely zellij web's.

### 5.2 Token handling
The `token` is **zellij web's** login token, not kamaji's. The board may display
it (or pre-fill it into the URL if zellij web supports query-param login — a
small spike confirms the mechanism). kamaji never validates/stores user auth of
its own — there is none on localhost.

### 5.3 iframe vs. new tab (the real judgment call)
zellij web may send `X-Frame-Options: DENY`/CSP `frame-ancestors` that **blocks
iframing**. Design for both, default to robust:
- **Default: new tab/window.** Always works, full viewport, survives any framing
  restriction. Ships in 3d.
- **Progressive enhancement: an inline iframe panel** — only if a one-time
  runtime probe shows zellij web permits framing. The daemon can do this during
  attach (HEAD/GET `web_url`, inspect `X-Frame-Options`/CSP) and include
  `iframeable: bool` in the attach response, so the UI offers the inline option
  only when it will work. Needs a **quick check during implementation** against
  real zellij 0.43+ (the Phase 1 §6 spike).

## 6. Auth & security

- **None for Phase 3 / localhost.** The daemon binds `127.0.0.1:8755`; board,
  assets, and `/ui/events` are reachable only locally, like the JSON API. No
  login, no CSRF tokens, no cookies-as-auth.
- **CSRF posture:** no ambient credential ⇒ classic CSRF doesn't apply. We scope
  to `127.0.0.1`, add no permissive CORS (the board is same-origin with the
  daemon).
- **zellij web's own auth is separate and unchanged** (it consumes its `token`).
  kamaji is not in that trust path.
- **Future remote seams (build-aware, not built):** bind address + an auth
  middleware layer are config, no-op on localhost; when remote mode lands, the
  HTML + `/ui/events` routes go through the *same* `axum::Router` middleware as
  the JSON API, so auth is added in one place. zellij web already supports
  `web_server_ip`, TLS, and `base_url` for proxies; `zellij_web::web_url`'s base
  is configurable, so a remote base flows through without UI changes.

## 7. Visual & structural design direction

Sets **direction**, not pixels — the actual visual build invokes the
**frontend-design skill** at implementation time (3e).

### 7.1 Aesthetic intent
Clean, modern, **distinctive — deliberately not generic-AI-looking**. A focused
operator's tool: dense enough to see the whole project at a glance, calm enough
to live in all day.
- **Dark-first** palette with a single confident accent, conceptually aligned
  with the TUI's catppuccin-style themes so the two surfaces feel like one
  product. Per-column accent colors echo the TUI's `status_color`.
- **Typography-led, not chrome-led:** a strong type scale carries hierarchy; 1px
  hairline borders, a low-contrast `--surface` fill for hover/active cards,
  generous whitespace (translating the TUI's thin rounded borders + subtle fill).
- A small **motion** budget: cards slide/fade when SSE relocates them
  (sub-200ms; respect `prefers-reduced-motion`).
- The **session bullet** + **activity chip** are the board's most
  information-dense elements ("which agents need me" is the primary question) —
  design them as crisp status indicators.

### 7.2 Information architecture (load-bearing)
One screen, top bar + four columns (§3.0). No nav/sidebar in v1. Cards show
answerable-at-a-glance facts (id, title, agent, session presence, activity);
detail lives in the edit modal. Empty columns show a quiet placeholder.

### 7.3 Styling mechanics
A single hand-written `app.css` served from `/assets/`. No Tailwind/Bootstrap. A
minimal utility layer via CSS custom properties (`--accent`, `--surface`,
`--col-todo`, spacing/radius/type scales) keeps the maud templates clean and the
theme in one place. maud renders semantic HTML; CSS does the looks.

## 8. Serving the UI from the daemon

### 8.1 New routes (added to `router()` in `crates/kamajid/src/lib.rs`)

```
GET  /                          board page (HTML)               routes::ui::board
GET  /ui/events                 SSE → Datastar fragment patches routes::ui_events::events
GET  /assets/*path              embedded static assets          routes::assets
GET  /ui/tickets/new            create-ticket modal fragment    routes::ui::new_ticket
GET  /ui/tickets/:id/edit       edit-ticket modal fragment      routes::ui::edit_ticket
```

No new *command* routes — create/move/start/done/delete/edit reuse the existing
JSON API. The HTML routes are read/render only.

### 8.2 Module shape

```
crates/kamajid/src/
├── lib.rs                 router() gains the 5 routes above
├── routes/
│   ├── ui.rs              NEW — board page + modal fragments (maud)
│   ├── ui_events.rs       NEW — /ui/events SSE: Event → fragment patch serializer
│   └── assets.rs          NEW — serve embedded /assets/* (rust-embed)
└── views/                 NEW — maud partials
    ├── mod.rs
    ├── page.rs            full document shell
    ├── board.rs           board() + column(status, &[Ticket])
    ├── card.rs            card(&Ticket, Option<SignalLevel>)
    └── modal.rs           ticket_form(...) create/edit dialog
└── assets/                NEW — vendored static files
    ├── datastar.js        vendored Datastar runtime (pinned version)
    └── app.css            hand-written stylesheet
```

`views/` partials are pure `(&data) -> maud::Markup` — trivially unit-testable.
`ui_events.rs` reuses `views::board::column` and `views::card::card`, so live
patches and the initial page render identical markup.

### 8.3 Serving Datastar & CSS — vendored, embedded

**Vendor Datastar, don't CDN it** (a localhost tool must work offline).
**Chosen: embed with `rust-embed`** — `datastar.js` + `app.css` compiled into
`kamajid`, served by `routes::assets`; the daemon stays a single self-contained
binary (matches how kamaji ships today via `install.sh`). Content-type per
extension; an ETag from a build-time content hash for browser caching across
restarts. (`tower-http::ServeDir` from a runtime dir is rejected — it adds a
deploy-path dependency a single-binary CLI shouldn't have; it stays a fine
fallback if embedding annoys in dev.)

### 8.4 Dependencies

`crates/kamajid/Cargo.toml` gains:
```toml
maud = "0.26"          # compile-time-checked HTML; axum IntoResponse for Markup
rust-embed = "8"       # embed assets/ into the binary
mime_guess = "2"       # content-type for embedded assets
```
No new async/runtime deps (the SSE machinery is already present from Phase 1). No
JS toolchain, no npm.

## 9. Risks & open items
- **Datastar version/attribute drift.** Pinned by vendoring one version; the
  serializer matches it; upgrades are deliberate + tested. Low risk once pinned.
- **iframe-ability of zellij web** (§5.3 spike). Mitigated by defaulting to a new
  tab; iframe is upside only, gated on a runtime probe.
- **zellij web token-in-URL vs. login-page** (Phase 1 §6 carryover). A small
  spike; either way attach works, only polish is affected.
- **Per-card "active pulse."** A live green bullet without a column change needs
  a new `session.active` event; deferred to Phase 4.
- **Re-render cost on the broadcast path.** A DB read + maud render per event;
  trivial for single-user localhost; revisit only for a future heavy-fan-out
  remote mode (cache rendered columns then).
- **Reconnect full-board patch** (§4.4) — a few KB per connect; cheap, self-healing.

## 10. Testing strategy

### 10.1 View partials (unit tests)
`card`/`column`/`page`/`ticket_form` render the expected ids, titles, agent
labels, bullets (`●`/`○`), state-appropriate actions, per-column counts,
empty-state placeholders, the **"Needs attention"** header for `Review`, and the
`data-on-load` hook + asset `<link>`/`<script>`. `Markup -> String`
`contains`-style assertions (the style of the TUI's `board.rs` buffer tests, on
HTML).

### 10.2 HTTP routes (integration, extending the Phase 1 `TestDaemon`)
- `GET /` → 200 `text/html`, body contains the four columns + seeded cards in the
  right column.
- `GET /assets/datastar.js` / `app.css` → 200 with the right content-type.
- `GET /ui/tickets/new` → 200, modal fragment with the form.
- **The round-trip:** open `/ui/events`, `POST /tickets/:id/move`, assert a
  Datastar fragment patch for the affected columns arrives (the inline SSE-parser
  approach the Phase 1 tests already use). One test per event type confirms the
  serializer.
- **Reconnect re-sync:** connect `/ui/events`, assert the initial full-board
  patch arrives before any live event.

### 10.3 Live reactivity (real browser)
Render-assertion tests prove the *server* emits the correct fragments (the half
we own). A **documented manual smoke** (two tabs + the TUI move a card; trigger
auto-review) is the Phase 3 verification gate (parent spec §8). Playwright/
headless E2E deferred to Phase 4+.

### 10.4 Browser attach (real zellij web)
`#[ignore]`d test (like Phase 1d's `zellij_web_real_attach_info`): with real
zellij ≥0.43, `POST /tickets/:id/attach` returns a reachable `web_url`; manually
confirm a live terminal. Also the §5.3 framing probe (assert whether `web_url`
sends `X-Frame-Options`/`frame-ancestors`).

### 10.5 CI
Unchanged philosophy; the new view/route code is covered by `cargo test`
(embedded-asset + HTML-route tests run with no zellij). Real-zellij + browser
smokes stay manual.

## 11. Phased rollout

Each step ends green on `main` with a working daemon; the JSON API + TUI keep
working throughout; the browser surface grows step by step.

- **3a — Static board page (read-only).** Add `maud` + `rust-embed`, `views/`
  partials, `GET /` rendering the current board for a project, `/assets/*`
  (Datastar + CSS embedded). No reactivity/commands yet. *Verify:* `GET /`
  integration test; manual page load shows seeded tickets in the right columns.
- **3b — Commands wired to the existing API.** Card actions + the project
  switcher fire Datastar `@post/@patch/@delete` against existing endpoints. The
  page does a **full reload after each command** for now (no SSE yet). *Verify:*
  clicking Move/Start/Done/Delete mutates the board.
- **3c — Live SSE reactivity.** Add `GET /ui/events` + the `Event → fragment`
  serializer + `data-on-load` + the reconnect full-board patch; remove the
  full-reload crutch. *Verify:* round-trip integration tests; manual two-tab +
  TUI smoke.
- **3d — Browser attach (the headline).** Attach → `@post('/attach')` → open
  `web_url` in a new tab. Run the framing spike; if allowed, add the gated inline
  iframe panel. *Verify:* `#[ignore]`d real-zellij attach; manual "attach opens a
  live session."
- **3e — Create/edit forms + polish.** The modal fragments, done-with-cleanup
  confirm, delete confirm, empty-column placeholders, and the
  **frontend-design-skill pass** for real visual quality (§7). *Verify:* modal
  render tests; create→card-appears round-trip; manual visual review.

## 12. What stays out of Phase 3 (explicit)
- Remote / multi-user / auth / TLS (deferred; §6 seams only).
- A bespoke browser terminal (zellij web owns it).
- Any JS build pipeline/framework (only vendored Datastar + maud).
- Drag-and-drop card moves (buttons in v1; DnD → Phase 4).
- A `session.active` event / live "active pulse" bullet (Phase 4).
- Config *editing* from the browser (`GET /config` only).
- Rich project management (minimal switcher + create only).
- Heavy browser-automation E2E (smoke + render-assertions for now).
- TUI work (Phase 2).

## 13. Owner decisions (resolved)

The owner delegated these; resolved as follows (baked in above):

1. **Attach UX:** **open `web_url` in a new tab/window by default**; the inline
   iframe panel is a gated progressive enhancement after the §5.3 framing spike.
   (Adopted.)
2. **Move interaction:** **buttons / a small move menu** for v1; drag-and-drop
   deferred to Phase 4. (Adopted.)
3. **Visual theme:** **dark-first, echoing the TUI's catppuccin-style themes** +
   per-column status colors so browser + TUI read as one product; the 3e
   frontend-design pass sets the final identity. (Adopted.)
