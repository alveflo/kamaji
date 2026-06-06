# Dense Board Redesign — Ticket modals (new + edit) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish Slice 2 of the dense redesign — the new/edit ticket modals — by replacing the agent `<select>` with a segmented control, adding a "start in background" checkbox to the new-ticket modal, and locking the initial prompt (with a header live-dot and a footer Delete) for started tickets in the edit modal.

**Architecture:** All markup lives in the single pure partial `views::modal::ticket_form` (rooted at `#modal`, morphed in by Datastar `@get`). The form submits through an explicit `fetch()` to the existing JSON command API (`POST /tickets`, `PATCH /tickets/:id`); the authoritative board update arrives over `/ui/events`. The `.check` / lock / live-dot CSS extras are added to the foundation's shared `modal.css`. "Has a session / running" is derived from `ticket.session_name.is_some()` — the same signal `views::card` uses for the bullet.

**Tech Stack:** Rust + maud (server-rendered views), vendored Datastar v1.0.0-RC.6 (colon bindings, `data-on:click` / `data-on:submit`), axum daemon (`kamajid`).

---

## Hard invariants (must survive — verified against the smoke test + integration tests)

1. **`#modal` rooting + `dialog.modal`.** The fragment's top-level element stays `<div id="modal">` (morph-by-id); the dialog stays `<dialog open class="modal" id="ticket-dialog">`. `modal.css` pins `dialog.modal{position:fixed}` and paints `#modal:has(dialog.modal)::before` — do **not** wrap the dialog in `.modal-overlay` (that would break the #95 regression step).
2. **Smoke-driven controls.** Keep `input#f-title name="title" required`, a `button[type="submit"]` inside `#ticket-dialog`, a `Cancel` button (clears `#modal`), and the `data-on:keydown__window` Escape handler clearing `#modal`. Smoke fills `#f-title` and clicks submit; it never touches the agent control or any checkbox.
3. **RC.6 colon bindings only.** Every binding uses `data-on:click` / `data-on:submit` (colon). No hyphen forms.
4. **Submit reads named controls.** The submit JS reads `f.elements['title']`, `['description']`, `['initial_prompt']`, `['agent']`. The agent value must therefore remain readable as a form-named control — the segmented control writes into a hidden `<input name="agent">`.
5. **Default-unchecked checkbox = unchanged create.** With the box unchecked the create flow is byte-equivalent in behavior to today (POST `/tickets`, create in Todo). Ticking it additionally fires `POST /tickets/:id/start`.

## File structure

- **Modify** `crates/kamajid/src/views/modal.rs` — the `ticket_form` partial + its `#[cfg(test)]` module. (Sole owner of the modal markup.)
- **Modify** `crates/kamajid/src/assets/modal.css` — append the Slice 2 extras (`.check`, `.check-text`, `.lock-tag`, `.modal-livedot`, readonly-field styling). Foundation chrome (`.seg`, `.btn-danger`, `.foot-spacer`, `.req`, `.hint`) is already present — do not duplicate it.
- **Modify** `crates/kamajid/tests/ui.rs` — update the one assertion that pins the old contiguous create-close string (the create-close now branches on the checkbox).
- **No change** to `crates/kamajid/src/routes/ui.rs` `edit_ticket`: it already fetches the full `Ticket` (including `session_name`), which is everything `ticket_form` needs to lock the prompt / show the live-dot. Task 3 verifies this and only touches it if a change proves necessary.

---

## Task 1: Agent picker → segmented control

Replace the `<select id="f-agent" name="agent">` with a `.seg` of `type="button"` buttons plus a hidden `<input name="agent">`. Each seg button, on click, writes its agent value into the hidden input and moves the `.on` highlight. The submit JS is unchanged (still reads `f.elements['agent'].value`).

**Files:**
- Modify: `crates/kamajid/src/views/modal.rs` (the Agent `div.field` in `ticket_form`; tests)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/kamajid/src/views/modal.rs`:

```rust
    #[test]
    fn agent_picker_is_segmented_control_not_select() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(html.contains(r#"class="seg""#), "renders a segmented control:\n{html}");
        // The default agent's button is highlighted.
        assert!(
            html.contains(r#"class="on" data-on:click"#),
            "default agent button carries `on`:\n{html}"
        );
        // The value is still a form-named control the submit JS can read.
        assert!(
            html.contains(r#"<input type="hidden" name="agent" value="claude">"#),
            "hidden agent input carries the selected value:\n{html}"
        );
        // The seg buttons set the hidden input + move the highlight, client-side.
        assert!(
            html.contains("this.form.elements['agent'].value='codex'"),
            "a seg button writes its value into the hidden input:\n{html}"
        );
        assert!(!html.contains("<select"), "the old dropdown is gone:\n{html}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamajid --lib agent_picker_is_segmented_control_not_select`
Expected: FAIL (still renders `<select`; no `class="seg"`).

- [ ] **Step 3: Replace the Agent field markup**

In `ticket_form`, replace the Agent `div class="field"` block (the one containing `select id="f-agent"`) with:

```rust
                        div class="field" {
                            label { "Agent" }
                            div class="seg" {
                                @for a in Agent::all() {
                                    button type="button"
                                           class=[(a == agent).then_some("on")]
                                           data-on:click=(PreEscaped(format!(
                                               "this.form.elements['agent'].value='{val}';this.closest('.seg').querySelectorAll('button').forEach(b=>b.classList.remove('on'));this.classList.add('on')",
                                               val = a.as_str()
                                           ))) { (a.label()) }
                                }
                            }
                            input type="hidden" name="agent" value=(agent.as_str());
                        }
```

- [ ] **Step 4: Update the two existing tests that assumed a `<select>`**

In `create_form_posts_to_tickets_with_default_agent`, replace the assertion block:

```rust
        assert!(
            html.contains(r#"value="claude" selected"#),
            "default agent preselected:\n{html}"
        );
```

with:

```rust
        assert!(
            html.contains(r#"<input type="hidden" name="agent" value="claude">"#),
            "default agent is the hidden input value:\n{html}"
        );
```

In `edit_form_patches_and_prefills`, replace:

```rust
        assert!(
            html.contains(r#"value="codex" selected"#),
            "agent prefilled:\n{html}"
        );
```

with:

```rust
        assert!(
            html.contains(r#"<input type="hidden" name="agent" value="codex">"#),
            "agent prefilled as the hidden input value:\n{html}"
        );
```

- [ ] **Step 5: Run the modal tests to verify they pass**

Run: `cargo test -p kamajid --lib views::modal`
Expected: PASS (all modal tests, including the new segmented-control test).

- [ ] **Step 6: fmt + clippy + commit**

Run: `cargo fmt && cargo clippy -p kamajid --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/kamajid/src/views/modal.rs
git commit -m "feat(modal): agent picker is a segmented control, not a dropdown"
```

---

## Task 2: New-ticket "start in background" checkbox (default unchecked)

Add a `.check` row to the new-ticket modal only. Default **unchecked** (create in Todo). When ticked, the create submit additionally fires `POST /tickets/:id/start` with the id from the `POST /tickets` 201 response, then clears `#modal`.

**Files:**
- Modify: `crates/kamajid/src/views/modal.rs` (the create-submit JS, the checkbox markup, tests)
- Modify: `crates/kamajid/src/assets/modal.css` (`.check`, `.check-text`)
- Modify: `crates/kamajid/tests/ui.rs` (the create-close assertion)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/kamajid/src/views/modal.rs`:

```rust
    #[test]
    fn new_ticket_has_unchecked_background_checkbox_that_starts_on_create() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(html.contains(r#"class="check""#), "renders the checkbox row:\n{html}");
        assert!(
            html.contains(r#"<input type="checkbox" name="start_now""#),
            "named start_now checkbox:\n{html}"
        );
        assert!(
            !html.contains(r#"name="start_now" checked"#)
                && !html.contains(r#"name="start_now" id="f-start" checked"#),
            "checkbox defaults unchecked:\n{html}"
        );
        // When ticked, the create-submit reads the 201 body and starts the session.
        assert!(
            html.contains("f.elements['start_now'].checked"),
            "create-submit branches on the checkbox:\n{html}"
        );
        assert!(
            html.contains("r.json().then(t=>fetch('/tickets/'+t.id+'/start',{method:'POST'}))"),
            "ticking starts the new ticket's session:\n{html}"
        );
    }

    #[test]
    fn edit_ticket_has_no_background_checkbox() {
        let t = ticket();
        let html = ticket_form(1, Some(&t), Agent::Claude, None).into_string();
        assert!(!html.contains(r#"class="check""#), "edit mode has no checkbox:\n{html}");
        assert!(!html.contains("start_now"), "edit mode never starts on save:\n{html}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamajid --lib new_ticket_has_unchecked_background_checkbox`
Expected: FAIL (no `.check` row; create-submit doesn't branch on a checkbox).

- [ ] **Step 3: Branch the create-submit JS on the checkbox**

In `ticket_form`, the submit action is currently built from a shared `close_on_ok`. Replace the `close_on_ok` + `submit_action` block:

```rust
    let close_on_ok = format!("then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}}})");
    let submit_action = match editing {
        Some(t) => format!(
            "evt.preventDefault();const f=evt.target;fetch('/tickets/{id}',{{method:'PATCH',headers:{{'content-type':'application/json'}},body:JSON.stringify({{{fields}}})}}).{close_on_ok}",
            id = t.id,
        ),
        None => format!(
            "evt.preventDefault();const f=evt.target;fetch('/tickets',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{project_id:{project_id},{fields}}})}}).{close_on_ok}",
        ),
    };
```

with (edit keeps the simple close; create branches on `start_now`, reading the 201 `Ticket` body for the id):

```rust
    let submit_action = match editing {
        // Edit: PATCH, then clear the mount on a 2xx (a 4xx leaves the inline error visible).
        Some(t) => format!(
            "evt.preventDefault();const f=evt.target;fetch('/tickets/{id}',{{method:'PATCH',headers:{{'content-type':'application/json'}},body:JSON.stringify({{{fields}}})}}).then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}}})",
            id = t.id,
        ),
        // Create: POST, then (if the background box is ticked) read the 201 body and
        // start the new ticket's session before clearing the mount. The start fetch
        // is fire-and-forget — the board update arrives over /ui/events.
        None => format!(
            "evt.preventDefault();const f=evt.target;fetch('/tickets',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{project_id:{project_id},{fields}}})}}).then(r=>{{if(r.ok){{if(f.elements['start_now'].checked){{r.json().then(t=>fetch('/tickets/'+t.id+'/start',{{method:'POST'}}))}}{CLEAR_MODAL_JS}}}}})",
        ),
    };
```

- [ ] **Step 4: Add the checkbox row (create mode only)**

In `ticket_form`, immediately after the closing brace of the Agent `div class="field"` block and before `@if let Some(e) = error` (i.e. still inside `div class="modal-body"`), add:

```rust
                        @if editing.is_none() {
                            label class="check" {
                                input type="checkbox" name="start_now" id="f-start";
                                span class="check-text" {
                                    b { "Start the agent now, in the background" }
                                    "Spawns the session immediately and sends the initial prompt. Leave off to create it in Todo and start later."
                                }
                            }
                        }
```

- [ ] **Step 5: Add the `.check` CSS**

Append to `crates/kamajid/src/assets/modal.css` (before the smoke-critical positioning section, or at end — placement is cosmetic):

```css
/* ----------------------------------------------------------------------------
   Slice 2 extras: background-start checkbox
   -------------------------------------------------------------------------- */
.check {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin-top: 15px;
  cursor: pointer;
}
.check input {
  appearance: none;
  -webkit-appearance: none;
  flex: 0 0 auto;
  width: 17px;
  height: 17px;
  margin: 1px 0 0;
  border: 1px solid var(--hair-2);
  border-radius: 5px;
  background: var(--surface);
  cursor: pointer;
  position: relative;
  transition: background 0.12s var(--ease), border-color 0.12s var(--ease);
}
.check input:checked {
  background: var(--accent);
  border-color: var(--accent);
}
.check input:checked::after {
  content: "✓";
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 11px;
  font-weight: 800;
  color: #0b0b14;
}
.check-text {
  font-size: 12px;
  color: var(--muted);
  line-height: 1.45;
}
.check-text b {
  display: block;
  font-weight: 500;
  font-size: 12.5px;
  color: var(--text);
  margin-bottom: 1px;
}
```

- [ ] **Step 6: Update the integration test that pinned the old create-close string**

In `crates/kamajid/tests/ui.rs`, the test `new_ticket_fragment_mounts_and_self_closes` asserts the old contiguous string. Replace:

```rust
    assert!(
        body.contains("if(r.ok){document.getElementById('modal').replaceChildren()}"),
        "submit closes the modal on a 2xx:\n{body}"
    );
```

with:

```rust
    assert!(
        body.contains("document.getElementById('modal').replaceChildren()"),
        "submit closes the modal on a 2xx:\n{body}"
    );
    assert!(
        body.contains("f.elements['start_now'].checked"),
        "create-submit branches on the background-start checkbox:\n{body}"
    );
```

- [ ] **Step 7: Run the modal + ui tests**

Run: `cargo test -p kamajid --lib views::modal && cargo test -p kamajid --test ui new_ticket_fragment_mounts_and_self_closes`
Expected: PASS.

- [ ] **Step 8: fmt + clippy + commit**

Run: `cargo fmt && cargo clippy -p kamajid --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/kamajid/src/views/modal.rs crates/kamajid/src/assets/modal.css crates/kamajid/tests/ui.rs
git commit -m "feat(modal): new-ticket background-start checkbox (default unchecked)"
```

---

## Task 3: Edit-ticket — locked prompt, header live-dot, footer Delete

For a **started** ticket (`session_name.is_some()`): make the initial-prompt textarea `readonly` with a `🔒 locked` tag and a "read-only once started" hint, and show a `.modal-livedot` in the header. For an unstarted ticket the prompt stays editable. In edit mode, the footer gets a left-pinned `.btn.btn-danger` Delete (confirm-guarded `DELETE /tickets/:id`, clears `#modal` on success), a `.foot-spacer`, then Cancel + Save. Create mode's footer is unchanged (Cancel + Create, right-aligned).

**Files:**
- Modify: `crates/kamajid/src/views/modal.rs` (prompt field, header, footer; tests)
- Modify: `crates/kamajid/src/assets/modal.css` (`.lock-tag`, `.modal-livedot`, readonly-field styling)
- Verify (likely no change): `crates/kamajid/src/routes/ui.rs` `edit_ticket`

- [ ] **Step 1: Write the failing tests**

First, the existing `ticket()` test fixture has `session_name: None` (unstarted). Add a started variant and the new tests to the `tests` module in `crates/kamajid/src/views/modal.rs`:

```rust
    fn started_ticket() -> Ticket {
        let mut t = ticket();
        t.session_name = Some("kamaji-9".into());
        t
    }

    #[test]
    fn edit_started_ticket_locks_prompt_and_shows_livedot() {
        let t = started_ticket();
        let html = ticket_form(1, Some(&t), Agent::Claude, None).into_string();
        assert!(
            html.contains(r#"name="initial_prompt" rows="3" readonly"#),
            "started ticket: prompt is read-only:\n{html}"
        );
        assert!(html.contains(r#"class="lock-tag""#), "shows the locked tag:\n{html}");
        assert!(html.contains(r#"class="modal-livedot""#), "header live-dot:\n{html}");
        assert!(
            html.contains("read-only once the agent has started"),
            "explains why the prompt is locked:\n{html}"
        );
    }

    #[test]
    fn edit_unstarted_ticket_keeps_prompt_editable() {
        let t = ticket(); // session_name: None
        let html = ticket_form(1, Some(&t), Agent::Claude, None).into_string();
        assert!(
            !html.contains("readonly"),
            "unstarted ticket: prompt stays editable:\n{html}"
        );
        assert!(!html.contains("lock-tag"), "no locked tag:\n{html}");
        assert!(!html.contains("modal-livedot"), "no live-dot:\n{html}");
    }

    #[test]
    fn edit_footer_has_left_pinned_delete() {
        let t = ticket();
        let html = ticket_form(1, Some(&t), Agent::Claude, None).into_string();
        assert!(
            html.contains(r#"class="btn btn-danger""#) && html.contains("Delete"),
            "edit footer has a danger Delete:\n{html}"
        );
        assert!(html.contains(r#"class="foot-spacer""#), "Delete is pinned left:\n{html}");
        assert!(
            html.contains("confirm(") && html.contains("fetch('/tickets/9',{method:'DELETE'})"),
            "Delete is confirm-guarded and hits the JSON API:\n{html}"
        );
    }

    #[test]
    fn create_footer_has_no_delete() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(!html.contains("btn-danger"), "create has no Delete:\n{html}");
        assert!(!html.contains("foot-spacer"), "create footer is right-aligned only:\n{html}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamajid --lib edit_started_ticket_locks_prompt_and_shows_livedot`
Expected: FAIL (no `readonly`/`lock-tag`/`modal-livedot`).

- [ ] **Step 3: Compute the locked flag**

In `ticket_form`, after the `let (title, desc, prompt, agent, heading, submit_label) = ...;` match block, add:

```rust
    // A started ticket (it has a session) locks the initial prompt — that text is
    // only consumed at session creation — and flags the session as running.
    let session_running = editing.map(|t| t.session_name.is_some()).unwrap_or(false);
```

- [ ] **Step 4: Live-dot in the header**

In the `div class="modal-head"`, replace the editing id-pill block:

```rust
                        @if let Some(t) = editing {
                            span class="modal-idpill" { "#" (t.id) }
                        }
```

with:

```rust
                        @if let Some(t) = editing {
                            span class="modal-idpill" { "#" (t.id) }
                            @if session_running {
                                span class="modal-livedot" title="session running" {}
                            }
                        }
```

- [ ] **Step 5: Lock the prompt field**

Replace the initial-prompt `div class="field"` block:

```rust
                        div class="field" {
                            label for="f-prompt" { "Initial prompt" }
                            textarea id="f-prompt" name="initial_prompt" rows="3" { (prompt) }
                            div class="hint" {
                                "The first message handed to the agent when it starts."
                            }
                        }
```

with:

```rust
                        div class="field" {
                            label for="f-prompt" {
                                "Initial prompt"
                                @if session_running { span class="lock-tag" { "🔒 locked" } }
                            }
                            textarea id="f-prompt" name="initial_prompt" rows="3" readonly[session_running] { (prompt) }
                            div class="hint" {
                                @if session_running {
                                    "Only used when the session is first created — read-only once the agent has started."
                                } @else {
                                    "The first message handed to the agent when it starts."
                                }
                            }
                        }
```

- [ ] **Step 6: Footer Delete (edit mode, pinned left)**

Replace the `div class="modal-foot"` block:

```rust
                    div class="modal-foot" {
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                        button type="submit" class="btn btn-primary" { (submit_label) }
                    }
```

with:

```rust
                    div class="modal-foot" {
                        @if let Some(t) = editing {
                            button type="button" class="btn btn-danger"
                                   data-on:click=(PreEscaped(format!(
                                       "confirm('Delete #{id}? This cannot be undone.')&&fetch('/tickets/{id}',{{method:'DELETE'}}).then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}}})",
                                       id = t.id
                                   ))) { "Delete" }
                            span class="foot-spacer" {}
                        }
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                        button type="submit" class="btn btn-primary" { (submit_label) }
                    }
```

- [ ] **Step 7: Add the lock / live-dot / readonly CSS**

Append to `crates/kamajid/src/assets/modal.css`:

```css
/* ----------------------------------------------------------------------------
   Slice 2 extras: locked (read-only) prompt + running-session live-dot
   -------------------------------------------------------------------------- */
.field textarea[readonly],
.field input[readonly] {
  color: var(--dim);
  background: var(--bg);
  border-color: var(--hair);
  cursor: default;
}
.field textarea[readonly]:focus,
.field input[readonly]:focus {
  border-color: var(--hair);
  box-shadow: none;
}
.lock-tag {
  float: right;
  font-family: var(--font);
  font-size: 10px;
  text-transform: none;
  letter-spacing: 0;
  color: var(--muted);
  font-weight: 400;
}
.modal-livedot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--active);
  box-shadow: 0 0 7px color-mix(in srgb, var(--active) 70%, transparent);
  flex: 0 0 auto;
}
```

- [ ] **Step 8: Verify the edit_ticket route needs no change**

Read `crates/kamajid/src/routes/ui.rs` `edit_ticket`. It calls `db.get_ticket(id)` and passes `Some(&ticket)` to `ticket_form`. `Ticket` carries `session_name`, so the locked/live-dot logic works with no handler change. Confirm and leave it as-is.

- [ ] **Step 9: Run the modal tests**

Run: `cargo test -p kamajid --lib views::modal`
Expected: PASS (all modal tests).

- [ ] **Step 10: fmt + clippy + commit**

Run: `cargo fmt && cargo clippy -p kamajid --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/kamajid/src/views/modal.rs crates/kamajid/src/assets/modal.css
git commit -m "feat(modal): edit modal locks prompt + live-dot + footer delete for started tickets"
```

---

## After all tasks — whole-branch verification + ship

1. `cargo fmt --check` clean.
2. `cargo clippy --all-targets -- -D warnings` clean (workspace).
3. `cargo test` green (workspace).
4. **Browser smoke** green: build `kamajid`, run the smoke the way CI does (`.github/workflows/ci.yml` "Browser smoke" job — `cd crates/kamajid/smoke`, playwright). All steps must pass (board loads, SSE live create, Delete, Move, modal opens as a centered overlay over the dimmed board, Save, Cancel, Escape, the empty/whitespace-title 400 keeps the modal open).
5. Final whole-branch code review (`superpowers:requesting-code-review`).
6. PR: `gh pr create --fill --base main`, then `gh pr merge --squash --auto --delete-branch`.
7. When merged / the issue closes: remove the worktree + branch; mark the slay task done (`slay tasks done <id> --close`).

## Self-review notes (author)

- **Spec coverage:** segmented agent picker (Task 1) ✓; new-ticket unchecked background checkbox that starts on create (Task 2) ✓; edit read-only prompt for started tickets + editable otherwise (Task 3) ✓; header live-dot when running (Task 3) ✓; footer Delete pinned left, Cancel + Save right (Task 3) ✓; `.check`/lock/live-dot CSS extras in `modal.css` (Tasks 2,3) ✓; `#modal` rooting + colon bindings + fetch-then-clear invariants preserved (all tasks) ✓; render-assertion tests for both modals (all tasks) ✓.
- **"Has a session / running" definition:** uses `session_name.is_some()`, the same signal `views::card` uses for the bullet — the only session-state signal in the `Ticket` model. Both the read-only-prompt condition and the live-dot share it.
- **`edit_ticket` handler ownership:** the issue lists it as owned; Task 3 Step 8 confirms no change is required (the full `Ticket` already carries `session_name`). Listing-as-owned ≠ must-edit.
