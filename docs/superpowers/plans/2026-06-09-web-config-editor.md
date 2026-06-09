# Web Config Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the browser board a full structured config-editor modal (gear icon at the bottom of the side rail) covering every config field including the three built-in agents' command templates, and document the config file in the README.

**Architecture:** A new `PUT /config` route does a validated full-replace of the in-memory + on-disk `Config` (the existing `PATCH /config` stays for the TUI's partial edits). A `Config::validate()` helper in `kamaji-core` is the single validation source. A new `config_form` maud view renders the form rooted at `#modal`, opened by a `GET /ui/config` fragment route from a gear row in the sidebar. README gains a lead-in describing the editor and the precise file path.

**Tech Stack:** Rust, axum, maud (server-rendered HTML), Datastar (RC.6 `data-on:` colon bindings), serde/toml, the existing `kamajid` integration-test harness.

---

## File structure

- `crates/kamaji-core/src/config.rs` — add `Config::validate()` + unit tests (validation logic, single source of truth).
- `crates/kamajid/src/routes/config.rs` — add `put_config` handler calling `Config::validate()`.
- `crates/kamajid/src/lib.rs` — add `.put(...)` to the `/config` route; add the `GET /ui/config` route.
- `crates/kamajid/src/views/config_form.rs` — **new** view module: `config_form(&Config) -> Markup`.
- `crates/kamajid/src/views/mod.rs` — register `pub mod config_form;`.
- `crates/kamajid/src/routes/ui.rs` — add `config` handler returning the fragment.
- `crates/kamajid/src/views/sidebar.rs` — add the gear `rail-settings` row + test.
- `crates/kamajid/src/assets/sidebar.css` — minimal style for `rail-settings` if the existing `rail-add` style doesn't cover it (reuse `ws-tile`/`ws-label`; add only if needed).
- `crates/kamajid/tests/api.rs` — `PUT /config` integration tests.
- `README.md` — augment the `## Configuration` section.

---

### Task 1: `Config::validate()` in kamaji-core

**Files:**
- Modify: `crates/kamaji-core/src/config.rs`
- Test: `crates/kamaji-core/src/config.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block at the bottom of `crates/kamaji-core/src/config.rs`:

```rust
    #[test]
    fn validate_accepts_default_config() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn validate_rejects_unknown_default_agent() {
        let mut c = Config::default();
        c.default_agent = "ollama".into();
        let err = c.validate().unwrap_err();
        assert!(err.contains("default_agent"), "{err}");
    }

    #[test]
    fn validate_rejects_empty_with_prompt() {
        let mut c = Config::default();
        c.agents.claude.with_prompt = vec![];
        let err = c.validate().unwrap_err();
        assert!(err.contains("with_prompt"), "{err}");
    }

    #[test]
    fn validate_rejects_empty_no_prompt() {
        let mut c = Config::default();
        c.agents.codex.no_prompt = vec![];
        let err = c.validate().unwrap_err();
        assert!(err.contains("no_prompt"), "{err}");
    }

    #[test]
    fn validate_rejects_with_prompt_missing_placeholder() {
        let mut c = Config::default();
        c.agents.claude.with_prompt = vec!["claude".into()];
        let err = c.validate().unwrap_err();
        assert!(err.contains("{prompt}"), "{err}");
    }

    #[test]
    fn validate_rejects_unparseable_bind() {
        let mut c = Config::default();
        c.daemon.bind = "not-an-addr".into();
        let err = c.validate().unwrap_err();
        assert!(err.contains("bind"), "{err}");
    }

    #[test]
    fn validate_rejects_bad_log_format() {
        let mut c = Config::default();
        c.daemon.log_format = "xml".into();
        let err = c.validate().unwrap_err();
        assert!(err.contains("log_format"), "{err}");
    }

    #[test]
    fn validate_rejects_zero_poll_interval() {
        let mut c = Config::default();
        c.auto_review.poll_interval_secs = 0;
        let err = c.validate().unwrap_err();
        assert!(err.contains("poll_interval"), "{err}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji-core validate_`
Expected: FAIL — `no method named validate found for struct Config`.

- [ ] **Step 3: Implement `Config::validate()`**

Add this method inside `impl Config { ... }` in `crates/kamaji-core/src/config.rs` (after `worktree_dir`). Note `use crate::models::Agent;` is already imported at the top of the file.

```rust
    /// Validate a config before it is persisted by the web editor's full
    /// replace (`PUT /config`). Returns a human-readable message on the first
    /// failure. `PATCH /config` does not call this — it only ever sets a small
    /// set of already-constrained fields.
    pub fn validate(&self) -> Result<(), String> {
        self.default_agent
            .parse::<Agent>()
            .map_err(|e| format!("invalid default_agent: {e}"))?;

        for (name, cmds) in [
            ("claude", &self.agents.claude),
            ("codex", &self.agents.codex),
            ("copilot", &self.agents.copilot),
        ] {
            if cmds.with_prompt.is_empty() {
                return Err(format!("agents.{name}.with_prompt must not be empty"));
            }
            if cmds.no_prompt.is_empty() {
                return Err(format!("agents.{name}.no_prompt must not be empty"));
            }
            if !cmds.with_prompt.iter().any(|p| p.contains("{prompt}")) {
                return Err(format!(
                    "agents.{name}.with_prompt must contain a {{prompt}} token"
                ));
            }
        }

        self.daemon
            .bind
            .parse::<std::net::SocketAddr>()
            .map_err(|e| format!("invalid daemon.bind: {e}"))?;

        if !matches!(self.daemon.log_format.as_str(), "human" | "json") {
            return Err(format!(
                "invalid daemon.log_format: {} (expected \"human\" or \"json\")",
                self.daemon.log_format
            ));
        }

        if self.auto_review.poll_interval_secs == 0 {
            return Err("auto_review.poll_interval_secs must be at least 1".to_string());
        }

        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji-core validate_`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/config.rs
git commit -m "feat(core): add Config::validate for the web config editor"
```

---

### Task 2: `PUT /config` route

**Files:**
- Modify: `crates/kamajid/src/routes/config.rs`
- Modify: `crates/kamajid/src/lib.rs:38-41` (the `/config` route)
- Test: `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Write the failing integration tests**

First inspect how existing tests in `crates/kamajid/tests/api.rs` boot the daemon and issue requests (look for an existing `/config` or `PATCH` test and a `reqwest`/client helper). Mirror that harness. Add these tests (adapt the client/spawn helper names to the ones already in the file — e.g. a `spawn_app()`/`TestApp` helper and its `base_url`/client):

```rust
#[tokio::test]
async fn put_config_replaces_and_persists() {
    let app = spawn_app().await;
    let mut cfg: serde_json::Value = app
        .client
        .get(format!("{}/config", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    cfg["theme"] = serde_json::json!("nord");
    cfg["agents"]["claude"]["with_prompt"] =
        serde_json::json!(["claude", "--foo", "{prompt}"]);

    let resp = app
        .client
        .put(format!("{}/config", app.base_url))
        .json(&cfg)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Re-read: the change is reflected by the daemon.
    let after: serde_json::Value = app
        .client
        .get(format!("{}/config", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["theme"], "nord");
    assert_eq!(
        after["agents"]["claude"]["with_prompt"],
        serde_json::json!(["claude", "--foo", "{prompt}"])
    );
}

#[tokio::test]
async fn put_config_rejects_invalid_with_400() {
    let app = spawn_app().await;
    let mut cfg: serde_json::Value = app
        .client
        .get(format!("{}/config", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // with_prompt without a {prompt} token is invalid.
    cfg["agents"]["claude"]["with_prompt"] = serde_json::json!(["claude"]);

    let resp = app
        .client
        .put(format!("{}/config", app.base_url))
        .json(&cfg)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamajid put_config`
Expected: FAIL — `PUT /config` returns 405 (method not allowed) so the 200/400 assertions fail.

- [ ] **Step 3: Add the `put_config` handler**

In `crates/kamajid/src/routes/config.rs`, add (the file already imports `axum::extract::State`, `axum::Json`, `kamaji_core::config::Config`, `crate::error::ApiError`, `crate::state::AppState`):

```rust
/// `PUT /config` → full-replace the daemon's config from a complete `Config`
/// body. Used by the web config editor, which renders every field from a prior
/// `GET /config`. Validated (`Config::validate`) before persisting; on failure
/// returns 400 with a human message the modal shows inline. `PATCH /config`
/// remains the TUI's partial-edit path and is unaffected.
pub async fn put_config(
    State(state): State<AppState>,
    Json(body): Json<Config>,
) -> Result<Json<Config>, ApiError> {
    body.validate().map_err(ApiError::BadRequest)?;

    {
        let mut guard = state.config.write().await;
        *guard = body.clone();
    }

    let path = kamaji_core::config::config_path().map_err(ApiError::Internal)?;
    let to_save = body.clone();
    tokio::task::spawn_blocking(move || kamaji_core::config::save_to(&path, &to_save))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("config save task panicked: {e}")))?
        .map_err(ApiError::Internal)?;
    Ok(Json(body))
}
```

- [ ] **Step 4: Wire the route**

In `crates/kamajid/src/lib.rs`, change the `/config` route (currently `get(...).patch(...)`) to also accept PUT:

```rust
        .route(
            "/config",
            get(routes::config::get_config)
                .patch(routes::config::patch_config)
                .put(routes::config::put_config),
        )
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p kamajid put_config`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/kamajid/src/routes/config.rs crates/kamajid/src/lib.rs crates/kamajid/tests/api.rs
git commit -m "feat(daemon): PUT /config full-replace endpoint for the web editor"
```

---

### Task 3: `config_form` view

**Files:**
- Create: `crates/kamajid/src/views/config_form.rs`
- Modify: `crates/kamajid/src/views/mod.rs`
- Test: in `config_form.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Register the module**

In `crates/kamajid/src/views/mod.rs`, add (keep alphabetical-ish ordering, after `confirm`):

```rust
pub mod config_form;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/kamajid/src/views/config_form.rs` with ONLY the tests first (the `config_form` fn will be added in Step 4):

```rust
//! The config-editor modal fragment. Returned by `GET /ui/config`. Rooted at
//! `#modal` so Datastar's `@get` morph-by-id replaces the page's `<div
//! id="modal">` mount. Submit builds a full `Config` JSON from the form's named
//! controls and PUTs it to `/config` via an explicit `fetch()` (a Datastar
//! `@post`/`@put`'s 2nd argument is request *options*, not a body). Argv command
//! templates are edited as space-separated single-line inputs; auto-review
//! patterns (which may contain spaces) as newline-separated textareas. Bindings
//! use the RC.6 colon form (`data-on:click`); the hyphen form is inert. Built
//! from the shared modal chrome classes — no new CSS.

use kamaji_core::config::Config;
use kamaji_core::models::Agent;
use maud::{html, Markup, PreEscaped};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_is_rooted_at_modal() {
        let html = config_form(&Config::default()).into_string();
        assert!(
            html.starts_with(r#"<div id="modal">"#),
            "fragment rooted at #modal:\n{html}"
        );
        assert!(html.contains("<dialog"), "carries the dialog:\n{html}");
    }

    #[test]
    fn renders_shared_modal_chrome() {
        let html = config_form(&Config::default()).into_string();
        for cls in [
            r#"class="modal-head""#,
            r#"class="modal-body""#,
            r#"class="modal-foot""#,
            r#"class="modal-close""#,
            r#"class="field""#,
        ] {
            assert!(html.contains(cls), "chrome {cls} present:\n{html}");
        }
        assert!(
            html.contains(r#"class="modal-title">Settings"#),
            "heading in modal-title chrome:\n{html}"
        );
    }

    #[test]
    fn submit_puts_to_config_via_fetch() {
        let html = config_form(&Config::default()).into_string();
        assert!(
            html.contains("fetch('/config',{method:'PUT'"),
            "PUTs to /config:\n{html}"
        );
    }

    #[test]
    fn prefills_general_fields() {
        let mut c = Config::default();
        c.theme = "nord".into();
        c.base_branch = "main".into();
        c.worktree_base = Some("/wt".into());
        let html = config_form(&c).into_string();
        // theme select option preselected
        assert!(
            html.contains(r#"<option value="nord" selected"#),
            "theme preselected:\n{html}"
        );
        assert!(
            html.contains(r#"name="base_branch" value="main""#),
            "base_branch prefilled:\n{html}"
        );
        assert!(
            html.contains(r#"name="worktree_base" value="/wt""#),
            "worktree_base prefilled:\n{html}"
        );
    }

    #[test]
    fn agent_argvs_render_space_joined() {
        let html = config_form(&Config::default()).into_string();
        // copilot with_prompt is ["copilot","-i","{prompt}"] → "copilot -i {prompt}"
        assert!(
            html.contains(r#"name="copilot.with_prompt" value="copilot -i {prompt}""#),
            "copilot with_prompt space-joined:\n{html}"
        );
    }

    #[test]
    fn auto_review_patterns_render_newline_joined() {
        let mut c = Config::default();
        c.auto_review.patterns.codex = vec!["foo".into(), "bar".into()];
        let html = config_form(&c).into_string();
        assert!(
            html.contains("foo\nbar"),
            "codex patterns newline-joined in a textarea:\n{html}"
        );
    }

    #[test]
    fn daemon_restart_fields_are_labeled() {
        let html = config_form(&Config::default()).into_string();
        assert!(
            html.contains("applies after daemon restart"),
            "bind labeled:\n{html}"
        );
    }

    #[test]
    fn bindings_use_rc6_colon_syntax_not_hyphen() {
        let html = config_form(&Config::default()).into_string();
        assert!(html.contains("data-on:submit="), "colon submit:\n{html}");
        assert!(
            !html.contains("data-on-submit") && !html.contains("data-on-click"),
            "no inert hyphen bindings:\n{html}"
        );
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p kamajid --lib config_form`
Expected: FAIL — `cannot find function config_form in this scope`.

- [ ] **Step 4: Implement `config_form`**

Add the function above the `#[cfg(test)]` block in `crates/kamajid/src/views/config_form.rs`. The submit JS reads every named control, splits argv inputs on whitespace and pattern textareas on newlines (dropping blank entries), coerces the number/checkbox, and assembles the full `Config` shape before PUTting it.

```rust
/// The five built-in theme keys. The canonical registry lives in the TUI crate
/// (`kamaji/src/theme.rs`), which `kamajid` does not depend on; kept in sync
/// manually (theme keys change rarely).
const THEME_KEYS: [&str; 5] = ["default", "catppuccin", "tokyonight", "gruvbox", "nord"];
const ZELLIJ_BARS: [&str; 4] = ["auto", "compact", "default", "none"];

const CLEAR_MODAL_JS: &str = "document.getElementById('modal').replaceChildren()";

/// One labeled text input whose value is `parts` joined by a single space —
/// the space-separated argv editor for an agent command template.
fn argv_input(name: &str, label: &str, parts: &[String]) -> Markup {
    let joined = parts.join(" ");
    html! {
        div class="field" {
            label { (label) }
            input name=(name) class="mono" value=(joined);
        }
    }
}

/// Render the config-editor modal, pre-filled from `cfg`.
pub fn config_form(cfg: &Config) -> Markup {
    // Build the full Config JSON from the named controls. Helpers in the JS:
    //  - sp(name): split a space-separated argv input into a trimmed array
    //  - nl(name): split a newline-separated textarea into a trimmed array
    // Read controls via els['name'] (els = f.elements). Single-quoted JS only,
    // so the whole expression is safe inside the double-quoted PreEscaped attr.
    let submit_action = format!(
        "evt.preventDefault();const f=evt.target,els=f.elements;\
const sp=n=>els[n].value.split(/\\s+/).filter(Boolean);\
const nl=n=>els[n].value.split('\\n').map(s=>s.trim()).filter(Boolean);\
const body={{\
default_agent:els['default_agent'].value,\
theme:els['theme'].value,\
worktree_base:els['worktree_base'].value||null,\
base_branch:els['base_branch'].value,\
zellij_bar:els['zellij_bar'].value,\
agents:{{\
claude:{{with_prompt:sp('claude.with_prompt'),no_prompt:sp('claude.no_prompt'),resume:sp('claude.resume')}},\
codex:{{with_prompt:sp('codex.with_prompt'),no_prompt:sp('codex.no_prompt'),resume:sp('codex.resume')}},\
copilot:{{with_prompt:sp('copilot.with_prompt'),no_prompt:sp('copilot.no_prompt'),resume:sp('copilot.resume')}}\
}},\
auto_review:{{enabled:els['ar_enabled'].checked,poll_interval_secs:parseInt(els['poll_interval_secs'].value,10)||1,patterns:{{codex:nl('patterns.codex'),copilot:nl('patterns.copilot')}}}},\
daemon:{{bind:els['bind'].value,log_format:els['log_format'].value,log_level:els['log_level'].value,web_theme:els['web_theme'].value}}\
}};\
fetch('/config',{{method:'PUT',headers:{{'content-type':'application/json'}},body:JSON.stringify(body)}}).then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}else{{r.text().then(t=>{{document.getElementById('cfg-error').textContent=t}})}}}})"
    );
    let escape_handler = format!("if(evt.key==='Escape'){{{CLEAR_MODAL_JS}}}");
    let default_agent = cfg.default_agent();

    html! {
        div id="modal" {
            dialog open class="modal" id="config-dialog"
                   data-on:keydown__window=(PreEscaped(escape_handler)) {
                form data-on:submit=(PreEscaped(submit_action)) {
                    div class="modal-head" {
                        span class="modal-title" { "Settings" }
                        button type="button" class="modal-close"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "✕" }
                    }
                    div class="modal-body" {
                        // ---- General ----
                        h3 class="config-section" { "General" }
                        div class="field" {
                            label { "Default agent" }
                            input type="hidden" name="default_agent" id="cfg-agent"
                                  value=(default_agent.as_str());
                            div class="seg" role="group" aria-label="Default agent" {
                                @for a in Agent::all() {
                                    button type="button"
                                           class=[(a == default_agent).then_some("on")]
                                           data-on:click=(PreEscaped(format!(
                                               "el.form.elements['default_agent'].value='{val}';el.closest('.seg').querySelectorAll('button').forEach(b=>b.classList.remove('on'));el.classList.add('on')",
                                               val = a.as_str()
                                           ))) { (a.label()) }
                                }
                            }
                        }
                        div class="field" {
                            label { "Theme" }
                            select name="theme" {
                                @for key in THEME_KEYS {
                                    option value=(key) selected[key == cfg.theme] { (key) }
                                }
                            }
                        }
                        div class="field" {
                            label { "Worktree base" }
                            input name="worktree_base" class="mono"
                                  value=(cfg.worktree_base.clone().unwrap_or_default());
                            div class="hint" { "Where worktrees are created. {root} expands to the project root." }
                        }
                        div class="field" {
                            label { "Base branch" }
                            input name="base_branch" value=(cfg.base_branch);
                        }
                        div class="field" {
                            label { "Zellij bar" }
                            select name="zellij_bar" {
                                @for b in ZELLIJ_BARS {
                                    option value=(b) selected[b == cfg.zellij_bar] { (b) }
                                }
                            }
                        }

                        // ---- Agents ----
                        h3 class="config-section" { "Agents" }
                        div class="hint" { "Command templates as space-separated argv. {prompt} is replaced with the ticket prompt." }
                        @for (name, cmds) in [
                            ("claude", &cfg.agents.claude),
                            ("codex", &cfg.agents.codex),
                            ("copilot", &cfg.agents.copilot),
                        ] {
                            h4 class="config-subsection" { (name) }
                            (argv_input(&format!("{name}.with_prompt"), "With prompt", &cmds.with_prompt))
                            (argv_input(&format!("{name}.no_prompt"), "No prompt", &cmds.no_prompt))
                            (argv_input(&format!("{name}.resume"), "Resume", &cmds.resume))
                        }

                        // ---- Auto-review ----
                        h3 class="config-section" { "Auto-review" }
                        label class="check" {
                            input type="checkbox" name="ar_enabled" checked[cfg.auto_review.enabled];
                            span class="check-text" { b { "Enabled" } "Move a ticket to Needs attention when its agent goes idle." }
                        }
                        div class="field" {
                            label { "Poll interval (seconds)" }
                            input type="number" name="poll_interval_secs" min="1"
                                  value=(cfg.auto_review.poll_interval_secs);
                        }
                        div class="field" {
                            label { "Codex idle patterns" }
                            textarea name="patterns.codex" rows="2" { (cfg.auto_review.patterns.codex.join("\n")) }
                            div class="hint" { "One substring per line; a match means idle." }
                        }
                        div class="field" {
                            label { "Copilot idle patterns" }
                            textarea name="patterns.copilot" rows="2" { (cfg.auto_review.patterns.copilot.join("\n")) }
                        }

                        // ---- Daemon ----
                        h3 class="config-section" { "Daemon" }
                        div class="field" {
                            label { "Bind address" }
                            input name="bind" class="mono" value=(cfg.daemon.bind);
                            div class="hint" { "applies after daemon restart. The terminal proxy binds the next port up." }
                        }
                        div class="field" {
                            label { "Log format" }
                            select name="log_format" {
                                @for v in ["human", "json"] {
                                    option value=(v) selected[v == cfg.daemon.log_format] { (v) }
                                }
                            }
                        }
                        div class="field" {
                            label { "Log level" }
                            input name="log_level" value=(cfg.daemon.log_level);
                        }
                        div class="field" {
                            label { "Web theme" }
                            input name="web_theme" value=(cfg.daemon.web_theme);
                            div class="hint" { "applies to sessions created after a daemon restart." }
                        }

                        p id="cfg-error" class="form-error" {}
                    }
                    div class="modal-foot" {
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                        button type="submit" class="btn btn-primary" { "Save settings" }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p kamajid --lib config_form`
Expected: PASS (8 tests).

- [ ] **Step 6: Add minimal CSS for the section headings**

The form reuses existing chrome (`field`, `seg`, `check`, `hint`, `form-error`, `mono`). Only the `config-section`/`config-subsection` headings are new. Append to `crates/kamajid/src/assets/modal.css` (the modal stylesheet — confirm the filename by checking which CSS file defines `.modal-head`; use that one):

```css
.config-section {
  margin: 1.25rem 0 0.5rem;
  font-size: 0.8rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  opacity: 0.7;
}
.config-section:first-of-type { margin-top: 0; }
.config-subsection {
  margin: 0.75rem 0 0.25rem;
  font-size: 0.85rem;
  opacity: 0.85;
}
```

- [ ] **Step 7: Commit**

```bash
git add crates/kamajid/src/views/config_form.rs crates/kamajid/src/views/mod.rs crates/kamajid/src/assets/
git commit -m "feat(web): config-editor modal view"
```

---

### Task 4: `GET /ui/config` route

**Files:**
- Modify: `crates/kamajid/src/routes/ui.rs`
- Modify: `crates/kamajid/src/lib.rs`

- [ ] **Step 1: Add the handler**

In `crates/kamajid/src/routes/ui.rs`, add (the file already imports `axum::extract::State`, `maud::Markup`, `crate::state::AppState`, `crate::views`):

```rust
/// `GET /ui/config` → the config-editor modal fragment, pre-filled from the
/// daemon's loaded config. Submit PUTs the whole config to `/config`.
pub async fn config(State(state): State<AppState>) -> Markup {
    let cfg = state.config_async().await;
    views::config_form::config_form(&cfg)
}
```

- [ ] **Step 2: Wire the route**

In `crates/kamajid/src/lib.rs`, next to the other `/ui/*` routes (e.g. after the `/ui/projects/new` line):

```rust
        .route("/ui/config", get(routes::ui::config))
```

- [ ] **Step 3: Verify it builds and the route responds**

Run: `cargo build -p kamajid`
Expected: builds clean.

Run: `cargo test -p kamajid` (the existing suite still passes; no new behavior test needed here — the view is covered in Task 3, and a route-level smoke test is added in Task 5's sidebar wiring if desired).
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/kamajid/src/routes/ui.rs crates/kamajid/src/lib.rs
git commit -m "feat(web): GET /ui/config serves the config-editor fragment"
```

---

### Task 5: Gear icon in the side rail

**Files:**
- Modify: `crates/kamajid/src/views/sidebar.rs`
- Test: `crates/kamajid/src/views/sidebar.rs` (inline `#[cfg(test)]`)
- Maybe modify: `crates/kamajid/src/assets/sidebar.css`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/kamajid/src/views/sidebar.rs`:

```rust
    #[test]
    fn settings_gear_present_and_opens_config_modal() {
        let ps = [project(1, "acme")];
        let html = rail(&ps, 1, 0).into_string();
        assert!(
            html.contains(r#"class="rail-settings""#),
            "settings row present:\n{html}"
        );
        assert!(
            html.contains(r#"data-on:click="@get('/ui/config')""#),
            "gear opens the config modal:\n{html}"
        );
        assert!(html.contains("Settings"), "settings label:\n{html}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamajid --lib settings_gear`
Expected: FAIL — no `rail-settings`.

- [ ] **Step 3: Add the gear row**

In `crates/kamajid/src/views/sidebar.rs`, inside the `rail` function, add a settings row immediately after the existing `rail-add` div (still inside the `aside.rail`):

```rust
            // Gear pinned at the rail bottom — opens the config-editor modal by
            // morphing the `#modal` mount with the `/ui/config` fragment.
            div class="rail-settings" data-on:click="@get('/ui/config')" {
                span class="ws-tile" { "⚙" }
                span class="ws-label" { "Settings" }
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamajid --lib settings_gear`
Expected: PASS.

- [ ] **Step 5: Style parity (only if needed)**

If `rail-settings` does not inherit the `rail-add` row layout, add a rule mirroring `.rail-add` in `crates/kamajid/src/assets/sidebar.css` (find the `.rail-add` selector and add `.rail-settings` to it, or duplicate its block). If `.rail-add` is already a generic row style the gear inherits, skip this step.

- [ ] **Step 6: Commit**

```bash
git add crates/kamajid/src/views/sidebar.rs crates/kamajid/src/assets/
git commit -m "feat(web): settings gear in the side rail opens the config editor"
```

---

### Task 6: README documentation

**Files:**
- Modify: `README.md` (the `## Configuration` section, around line 240)

- [ ] **Step 1: Add the lead-in paragraph**

In `README.md`, immediately under the `## Configuration` heading (before the existing `` `~/.config/kamaji/config.toml`: `` line), insert:

```markdown
The browser board has a full **GUI editor** for everything below: click the
**gear icon at the bottom of the side rail** to open a form covering general
settings, the per-agent command templates, auto-review, and daemon options.
Fields that only take effect after a daemon restart are labeled in the form.
The **TUI** edits only the theme live (press `t`); every other field is changed
by editing the file directly or via the web editor.

The config file lives at `$XDG_CONFIG_HOME/kamaji/config.toml` (default
`~/.config/kamaji/config.toml`; Windows uses the native config directory). It is
written with defaults on first run.

```

- [ ] **Step 2: Verify the section reads cleanly**

Run: `sed -n '238,300p' README.md`
Expected: the new lead-in appears under `## Configuration`, followed by the existing TOML example and field table.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document the web config editor and config file location"
```

---

### Task 7: Whole-branch verification

- [ ] **Step 1: Format, lint, test the workspace**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: fmt clean, no clippy warnings, all tests pass.

- [ ] **Step 2: Manual smoke (optional but recommended)**

Run: `make restart` then open `http://127.0.0.1:8755`, click the gear, change the theme + a flag, Save, and confirm the modal closes and `~/.config/kamaji/config.toml` reflects the change. Re-open the gear to confirm values round-trip.

- [ ] **Step 3: Commit any fmt-only changes**

```bash
git add -A && git commit -m "style: cargo fmt" || true
```

---

## Self-review notes

- **Spec coverage:** Part A (PUT + validate) → Tasks 1–2. Part B (view + gear + route) → Tasks 3–5. Part C (README) → Task 6. Testing requirements → covered in Tasks 1, 2, 3, 5 + Task 7 whole-branch.
- **Type consistency:** `Config::validate()` (Task 1) is called by `put_config` (Task 2); `config_form(&Config)` (Task 3) is called by `routes::ui::config` (Task 4); `THEME_KEYS`/`ZELLIJ_BARS`/`argv_input` are defined and used within Task 3. Form control names (`claude.with_prompt`, `patterns.codex`, `ar_enabled`, `poll_interval_secs`, `bind`, …) match between the rendered inputs and the submit JS within Task 3.
- **Caveat carried from spec:** space-separated argv inputs can't express a token containing a space (documented in the field hint); auto-review patterns use newline-separated textareas because they legitimately contain spaces.
- **Harness note:** Task 2's tests use placeholder helper names (`spawn_app`, `app.client`, `app.base_url`); the implementer must adapt them to the actual harness already in `crates/kamajid/tests/api.rs` (read an existing test first).
