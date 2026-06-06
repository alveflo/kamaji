# Dense Board Redesign — FOUNDATION (issue #104)

Executes issue #104, the foundation for the dense board redesign. Design spec:
`docs/superpowers/specs/2026-06-06-dense-board-redesign-design.md` (lands via
PR #103); visual reference: the validated prototype in `specs/assets/`.

This foundation establishes the shared design system + app shell that all five
later feature slices build on, and splits the CSS so the slices touch disjoint
files. **It changes views + CSS only — no command logic, no new write routes.**

## Hard invariants (must survive)

1. **SSE patch ids.** `#card-<id>` (`views::card`) and `#col-<status>`
   (`views::board::column`) are the live-patch targets. **Do NOT touch
   `card.rs` or `board.rs`** — they are Slice 1's. The board renders through the
   new shell with its *current* markup.
2. **Datastar RC.6 colon syntax.** All event bindings use `data-on:click`
   (colon, not hyphen). Pure client-side JS expressions are allowed in the value
   (the codebase already does `window.location=…`, `document.getElementById(
   'modal').replaceChildren()`). The body-carrying command pattern is unchanged.
3. **Asset revalidation.** `routes/assets.rs` serves every embedded file
   `Cache-Control: no-cache` + ETag; `rust-embed` globs `src/assets/`, so new
   `.css` files are served automatically with no route change.
4. **Browser-smoke invariants** (`crates/kamajid/smoke/board.smoke.mjs`, an
   existing CI job — must stay green without editing it):
   - The topbar primary button is `button.new-ticket` and still fires
     `@get('/ui/tickets/new?project=<id>')`.
   - The in-progress card still has a **Move** button (card.rs is untouched).
   - The new-ticket modal: its dialog is `#ticket-dialog`, its title field is
     `#f-title`, it has a `button[type="submit"]` and a `Cancel` button, Escape
     closes it. The dialog computes `position: fixed` and sits within the
     viewport, and `#modal::before` paints the dimmed/blurred backdrop
     (`content` ≠ `none`). **These three CSS facts are load-bearing for the
     #95 regression step — preserve them.**

## Token vocabulary (the contract)

`tokens.css` is the single `:root`. It adopts the **prototype's palette names**
(catppuccin-derived dark) and *additionally* keeps the utility tokens the
carried board/terminal/modal CSS already depends on. Canonical set:

```css
:root{
  /* prototype palette */
  --bg:#16161f; --bg-2:#1b1b27; --surface:#1f2030; --surface-2:#26273a;
  --hair:#2a2b3c; --hair-2:#34354a;
  --text:#cdd6f4; --dim:#a6adc8; --muted:#6c7086; --faint:#45475a;
  --todo:#9399b2; --prog:#89b4fa; --review:#fab387; --done:#a6e3a1; --active:#a6e3a1;
  --accent:#89b4fa; --danger:#f38ba8;
  --rail:#0e0e16; --rail-hair:#22222e;
  --mono:"SF Mono",ui-monospace,"JetBrains Mono","Cascadia Code",monospace;
  --font:ui-sans-serif,system-ui,"Inter","SF Pro Text","Segoe UI",roboto,sans-serif;
  --r:7px;
  /* utility tokens retained for the carried board/terminal/modal CSS */
  --surface-3:#45475a;
  --radius:12px; --radius-sm:8px; --gap:14px; --pad:16px;
  --t-xs:11px; --t-sm:12.5px; --t-base:13px; --t-md:17.5px; --t-lg:22px; --t-xl:28px;
  --ease:cubic-bezier(0.22,1,0.36,1); --slide:180ms;
}
```

**Old → new var rename** (apply across every carried rule):
`--bg-raise`→`--bg-2`, `--hairline`→`--hair`, `--hairline-2`→`--hair-2`,
`--text-dim`→`--dim`, `--col-todo`→`--todo`, `--col-in_progress`→`--prog`,
`--col-review`→`--review`, `--col-done`→`--done`. Everything else
(`--bg`, `--surface`, `--surface-2`, `--surface-3`, `--muted`, `--faint`,
`--accent`, `--danger`, `--active`, `--radius`, `--radius-sm`, `--gap`, `--pad`,
`--t-*`, `--ease`, `--slide`, `--font`, `--mono`) keeps its name.

## CSS file layout (all `<link>`ed from `page.rs` head, in this order)

| File          | Owns (this foundation)                                                |
|---------------|----------------------------------------------------------------------|
| `tokens.css`  | the `:root` above + reset (`*{box-sizing}`, `html,body`) + base type (the prototype's `body` flex-shell rule) |
| `layout.css`  | `.main`, `.topbar`, `.crumb`/`.crumb-dot`/`.crumb-name`, `.search-slot` (empty placeholder), `.spacer`, `.new-ticket` (topbar primary btn), `.board` grid (prototype version) |
| `sidebar.css` | the workspace rail: `.rail`, `.rail-head`, `.rail-mark`, `.rail-word`, `.rail-toggle` (+ `.ic-open`/`.ic-collapsed`), `.rail-list`, `.ws`, `.ws-ind`, `.ws-tile`(+`.c1`..`.c4`), `.ws-badge`, `.ws-label`, `.rail-add`, `.rail-spacer` |
| `modal.css`   | shared chrome `.modal-overlay`/`.modal`/`.modal-head`/`.modal-title`/`.modal-idpill`/`.modal-close`/`.modal-body`/`.field`/`.seg`/`.modal-foot`/`.btn`/`.btn-primary`/`.btn-danger` (+ `.req`/`.hint` support) **and** the carried dialog-positioning rules that keep smoke green: `#modal:empty{display:none}`, `#modal:has(dialog.modal)::before{…backdrop…}`, `dialog.modal{position:fixed;inset:0;margin:auto;…}` |
| `board.css`   | carried `.column`/`.col-head`/`.col-title`/`.col-count`/`.col-body`/`.col-empty`/`.card`/`.bullet`/`.card-id`/`.card-title`/`.card-meta`/`.agent`/`.chip`/`.card-actions`/`.act` + `card-in`/`pulse` keyframes, retokenized. (Slice 1 rewrites this for dense cards + DnD.) |
| `terminal.css`| carried `.term-modal`/`.term-bar`/`.term-title`/`.term-close`/`.term-frame` + the `#modal:has(.term-modal)::before` backdrop, retokenized. (Slice 5 restyles.) |

Also: scrollbar/selection polish and `prefers-reduced-motion` block → put in
`tokens.css` (global). Delete `app.css`.

## Tasks

### Task 1 — CSS design-system split

**Create** in `crates/kamajid/src/assets/`: `tokens.css`, `layout.css`,
`sidebar.css`, `modal.css`, `board.css`, `terminal.css` per the table above,
porting the current `app.css` rules into the right file and applying the token
rename. `layout.css` `.board` + `tokens.css` `body` come from the **prototype**
(flex shell: `body{display:flex;height:100vh;overflow:hidden}`, `.main{flex:1;
display:flex;flex-direction:column;overflow:hidden}`, `.board{flex:1;overflow-y:
auto;display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:var(--gap);
padding:18px 16px;align-content:start}`). Drop the old body grid-texture
atmosphere (`body`/`body::before`/`body>*` layering) — the prototype shell is
plain. **Delete** `app.css`.

`.new-ticket` keeps a real primary-button look (it is the topbar "+ New").
`.search-slot` is an empty flex placeholder (Slice 4 fills it) — give it
`flex:1;max-width:340px` so the layout already reserves the slot.

**Fix the now-broken asset tests** (app.css no longer exists):
- `crates/kamajid/src/routes/assets.rs`: the 4 `"app.css"` literals in its
  `#[cfg(test)]` block → `"tokens.css"`. Update the module doc-comment mentions
  of `app.css` to `tokens.css`/"the stylesheets" as appropriate.
- `crates/kamajid/tests/ui.rs` (`serves_embedded_datastar_and_css`): the
  `"{base}/assets/app.css"` fetch → `"{base}/assets/tokens.css"`.

**Verify:** `cargo test -p kamajid` (the two asset tests now pass against
`tokens.css`); `cargo fmt --check`; `cargo clippy`. Do **not** run the browser
smoke here (page.rs still links app.css until Task 2 — that's fine, this task is
green at the Rust-test level because the asset tests target tokens.css).

> NOTE for the implementer: after this task `page.rs` still `<link>`s
> `app.css` (a 404 at runtime) — Task 2 swaps the links. The unit/integration
> test suite is green; the app is not "run" between tasks.

### Task 2 — App shell + workspace rail

**New file** `crates/kamajid/src/views/sidebar.rs`, exported from
`views/mod.rs`. One pure partial:

```rust
/// The Slack-style workspace rail. `projects` renders as tiles (active one
/// highlighted); `active_id` is the current project; `attention` is the
/// current project's "needs attention" (Review) count, shown as a badge on the
/// active tile (0 → no badge).
pub fn rail(projects: &[Project], active_id: i64, attention: usize) -> Markup
```

Rail structure (from the prototype, ported to maud):
- `<aside class="rail">`
  - `.rail-head`: `.rail-mark` + `.rail-word` "kamaji" + a `.rail-toggle`
    button. The toggle flips the shell: `data-on:click=
    "document.body.classList.toggle('rail-open')"`. Inside it,
    `<span class="ic-collapsed">☰</span><span class="ic-open">‹</span>` — CSS
    shows the hamburger when collapsed, the chevron when expanded, so the toggle
    lives **inside** the rail in both states.
  - `.rail-list`: one `.ws` per project. Active project gets `class="ws active"`.
    Each: `.ws-ind` + `.ws-tile cN` (N = `(index % 4) + 1`) containing the
    project **initials** (+ a `.ws-badge` with `attention` on the active tile
    when `attention > 0`) + `.ws-label` with the project name. The whole tile
    navigates: `data-on:click="window.location='/?project=<id>'"`.
  - `.rail-spacer`
  - `.rail-add` "+ Add project" tile pinned at the bottom — **rendered but
    inert** (Slice 3 wires it; no click handler, just the `.ws-tile` "+" and the
    `.ws-label`).

**Initials helper** (private fn): uppercase the first letter of each of the
first two whitespace-separated words; if the name is a single word, the first
two letters (uppercased). e.g. "My Project"→"MP", "kamaji"→"KA". Strip
non-alphanumerics defensively; fall back to "?" for an empty name.

**Rewrite `crates/kamajid/src/views/page.rs`** to the shell. The `page` signature
is unchanged (`project`, `projects`, `by_status`). Compute `attention` =
count of Review-status tickets for the current project from `by_status`. Body:

```
body class="rail-open" data-init="@get('/ui/events')" {
    (sidebar::rail(projects, project.id, attention))
    div class="main" {
        header class="topbar" {
            span class="crumb" {
                span class=(format!("crumb-dot c{n}", …active project's cN…)) {}
                span class="crumb-name" { (project.name) }
            }
            div class="search-slot" {}             // empty; Slice 4 fills it
            span class="spacer" {}
            button class="new-ticket"
                   data-on:click=(PreEscaped(format!("@get('/ui/tickets/new?project={}')", project.id))) { "+ New" }
        }
        (board(by_status))
        div id="modal" {}
    }
}
```

Head: drop the single `app.css` link; `<link>` **all six** stylesheets in the
order tokens → layout → sidebar → modal → board → terminal. Keep the vendored
`datastar.js` module + viewport/charset/title.

The crumb-dot `cN` must match the active project's tile `cN` (same
`(index%4)+1`), so factor the c-index out of a small shared helper if convenient
(or compute both from the active project's position in `projects`).

**Project selection moves out of the top-bar dropdown into the rail** — remove
the old `.project-switcher`/`<select>` entirely.

**Update `page.rs` tests** to the new shell:
- `page_links_css_and_vendored_datastar`: assert the head links `tokens.css`,
  `layout.css`, `sidebar.css`, `modal.css`, `board.css`, `terminal.css` and the
  datastar module; assert it no longer links `app.css`.
- `page_opens_ui_events_on_init`: unchanged intent (`data-init="@get('/ui/events')"`,
  no `data-on-load`).
- `page_has_modal_mount_and_switcher` → rename/retarget to the rail: assert the
  `#modal` mount exists, a `.rail` is present, the active project renders a tile
  with `ws active`, and the project name "acme" appears as a `.ws-label` (no
  `<select id="project-select">`).
- Add: collapse toggle is present in the rail and flips `rail-open`
  (`document.body.classList.toggle('rail-open')`), and the topbar button is
  `button class="new-ticket"` firing `@get('/ui/tickets/new?project=…')`.

Add unit tests in `sidebar.rs` for: initials derivation (multi-word, single
word, empty), active tile gets `ws active` + the others don't, the badge shows
only when `attention>0` and only on the active tile, tiles navigate to
`/?project=<id>` with colon bindings (no hyphen), "+ Add project" present.

**Verify:** `cargo test -p kamajid`, `cargo fmt --check`, `cargo clippy`.

### Task 3 — Shared modal chrome adoption

**Rewrite the markup in `crates/kamajid/src/views/modal.rs`** (`ticket_form`) so
the new-ticket / edit-ticket dialog renders **through the shared chrome** while
preserving every smoke/test invariant. Keep:
- the fragment rooted at `<div id="modal">` (morph target),
- the dialog `<dialog open class="modal" id="ticket-dialog">`,
- `data-on:keydown__window` Escape handler clearing `#modal`,
- the `data-on:submit` fetch (POST `/tickets` create / PATCH `/tickets/:id`
  edit) with the exact field-reading JS and the `if(r.ok){…replaceChildren()}`
  success-close — **byte-identical** strings (an integration test asserts
  `if(r.ok){document.getElementById('modal').replaceChildren()}` and the smoke
  drives `#f-title` + `button[type="submit"]`),
- the title input `id="f-title" name="title"` and the other named controls
  (`description`, `initial_prompt`, `agent`), `required` on title,
- the inline `.form-error` path (`error` arg) — keep it working; you may render
  it as a `.field`-level message, but keep the text visible.

Restructure the **visual** markup to the chrome:
- `.modal-head` with `.modal-title` (the heading) + (edit mode only) a
  `.modal-idpill` `#<id>`, and a `.modal-close` ✕ button that clears `#modal`
  (`data-on:click`).
- `.modal-body` containing `.field` blocks (`<label>` + control), each
  `<label>` may carry a `.req` "*" on required fields and a `.hint` where
  useful. Keep the Agent control as the existing `<select name="agent">` for
  now — **do not** build the segmented control (that is Slice 2). Wrap each
  field so it reads cleanly with the chrome.
- `.modal-foot` with the actions: a `.btn` "Cancel" (clears `#modal`) and a
  `.btn.btn-primary` `type="submit"` ("Create ticket" / "Save changes"). In
  edit mode you MAY add a left-pinned `.btn.btn-danger` placeholder only if it
  is wired to nothing harmful — **simplest is to omit Delete** (Slice 2 owns the
  edit-modal redesign). Keep the foot minimal: Cancel + submit.

Because `dialog.modal` is still the element and `modal.css` still pins it
`position:fixed` with the `#modal::before` backdrop, the smoke #95 step passes
unchanged. Do **not** introduce a `.modal-overlay` wrapper around the dialog
(it would break the `position:fixed`/`#modal::before` assertions); the
`.modal-overlay` class exists in `modal.css` as chrome for later slices.

**Update `modal.rs` tests** to the chrome markup: keep the existing assertions
that still hold (fetch strings, `#modal` rooting, `<dialog>` present, colon
bindings, success-close, Escape, validation error, default/prefilled agent),
and add/retarget assertions for the chrome classes actually rendered
(`.modal-head`, `.modal-title`, `.modal-body`, `.field`, `.modal-foot`, `.btn`,
`.btn-primary`). The `class="act"` buttons become `.btn`/`.btn-primary` — update
those assertions accordingly.

**Verify:** `cargo test -p kamajid`, `cargo fmt --check`, `cargo clippy`.

## After all tasks — whole-branch verification + ship

1. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean.
2. `cargo test` green (workspace).
3. **Browser smoke** green: `cd crates/kamajid/smoke` and run it the way CI does
   (see `.github/workflows/ci.yml` "Browser smoke" job) — build kamajid, run
   playwright. Every step (board loads, SSE live create, Delete, Move, modal
   open as centered overlay over a dimmed board, Save, Cancel, Escape, 400
   keeps-open) must pass.
4. Final whole-branch code review.
5. PR: `gh pr create --fill --base main`, note the deliberate carry of
   `board.css`/`terminal.css` (per the issue scoping decision) so Slices 1 & 5
   extend rather than create them. Then `gh pr merge --squash --auto
   --delete-branch`.
6. Mark the slay task done when the PR merges / issue closes.
```
