//! The config-editor modal fragment. Returned by `GET /ui/config`. Rooted at
//! `#modal` so Datastar's `@get` morph-by-id replaces the page's `<div
//! id="modal">` mount. Submit builds a full `Config` JSON from the form's named
//! controls and PUTs it to `/config` via an explicit `fetch()` (a Datastar
//! `@post`/`@put`'s 2nd argument is request *options*, not a body). Argv command
//! templates are edited as space-separated single-line inputs. Bindings use the
//! RC.6 colon form (`data-on:click`); the hyphen form is inert. Built from the
//! shared modal chrome classes — no new CSS beyond section headings.

use kamaji_core::config::Config;
use kamaji_core::models::Agent;
use maud::{html, Markup, PreEscaped};

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
    let submit_action = format!(
        "evt.preventDefault();const f=evt.target,els=f.elements;\
const sp=n=>els[n].value.split(/\\s+/).filter(Boolean);\
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
auto_review:{{enabled:els['ar_enabled'].checked,poll_interval_secs:parseInt(els['poll_interval_secs'].value,10)||1,copilot_idle_secs:parseInt(els['copilot_idle_secs'].value,10)||1}},\
daemon:{{bind:els['bind'].value,log_format:els['log_format'].value,log_level:els['log_level'].value,web_theme:els['web_theme'].value}}\
}};\
fetch('/config',{{method:'PUT',headers:{{'content-type':'application/json'}},body:JSON.stringify(body)}}).then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}else{{r.json().then(j=>{{document.getElementById('cfg-error').textContent=(j&&j.error)?j.error:'Save failed'}}).catch(()=>{{document.getElementById('cfg-error').textContent='Save failed'}})}}}})"
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

                        h3 class="config-section" { "Agents" }
                        div class="hint" { "Command templates as space-separated argv. {prompt} is replaced with the ticket prompt." }
                        @let agent_cmds = [
                            ("claude", &cfg.agents.claude),
                            ("codex", &cfg.agents.codex),
                            ("copilot", &cfg.agents.copilot),
                        ];
                        @for (name, cmds) in agent_cmds {
                            h4 class="config-subsection" { (name) }
                            (argv_input(&format!("{name}.with_prompt"), "With prompt", &cmds.with_prompt))
                            (argv_input(&format!("{name}.no_prompt"), "No prompt", &cmds.no_prompt))
                            (argv_input(&format!("{name}.resume"), "Resume", &cmds.resume))
                        }

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
                            label { "Copilot idle timeout (seconds)" }
                            input type="number" name="copilot_idle_secs" min="1"
                                  value=(cfg.auto_review.copilot_idle_secs);
                        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_is_rooted_at_modal() {
        let html = config_form(&Config::default()).into_string();
        assert!(
            html.starts_with(r#"<div id="modal">"#),
            "rooted at #modal:\n{html}"
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
            "title:\n{html}"
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
    fn submit_shows_human_error_on_failure() {
        let html = config_form(&Config::default()).into_string();
        assert!(
            html.contains("r.json().then("),
            "parses JSON error body:\n{html}"
        );
        assert!(
            html.contains("j.error"),
            "shows the human error message:\n{html}"
        );
        assert!(
            !html.contains("r.text().then(t=>"),
            "no raw-text error dump:\n{html}"
        );
    }

    #[test]
    fn prefills_general_fields() {
        let c = Config {
            theme: "nord".into(),
            base_branch: "main".into(),
            worktree_base: Some("/wt".into()),
            ..Default::default()
        };
        let html = config_form(&c).into_string();
        assert!(
            html.contains(r#"<option value="nord" selected"#),
            "theme preselected:\n{html}"
        );
        assert!(
            html.contains(r#"name="base_branch" value="main""#),
            "base_branch:\n{html}"
        );
        // The worktree-base input carries `class="mono"`, so `name` and `value`
        // are not adjacent; assert each independently.
        assert!(
            html.contains(r#"name="worktree_base""#) && html.contains(r#"value="/wt""#),
            "worktree_base:\n{html}"
        );
    }

    #[test]
    fn agent_argvs_render_space_joined() {
        let html = config_form(&Config::default()).into_string();
        // copilot with_prompt is ["copilot","-i","{prompt}"] → "copilot -i {prompt}".
        // The input carries `class="mono"`, so `name` and `value` are not
        // adjacent; assert each independently.
        assert!(
            html.contains(r#"name="copilot.with_prompt""#)
                && html.contains(r#"value="copilot -i {prompt}""#),
            "copilot with_prompt space-joined:\n{html}"
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
    fn auto_review_section_includes_copilot_idle_secs() {
        use kamaji_core::config::AutoReview;
        let c = Config {
            auto_review: AutoReview {
                copilot_idle_secs: 42,
                poll_interval_secs: 5,
                ..AutoReview::default()
            },
            ..Default::default()
        };
        let html = config_form(&c).into_string();
        // Input field is present and pre-filled with the current value.
        assert!(
            html.contains(r#"name="copilot_idle_secs""#) && html.contains(r#"value="42""#),
            "copilot_idle_secs input pre-filled:\n{html}"
        );
        // Submit JS reads and includes the field.
        assert!(
            html.contains("copilot_idle_secs:parseInt(els['copilot_idle_secs'].value,10)"),
            "submit JS includes copilot_idle_secs:\n{html}"
        );
    }

    #[test]
    fn bindings_use_rc6_colon_syntax_not_hyphen() {
        let html = config_form(&Config::default()).into_string();
        assert!(html.contains("data-on:submit="), "colon submit:\n{html}");
        assert!(
            !html.contains("data-on-submit") && !html.contains("data-on-click"),
            "no hyphen bindings:\n{html}"
        );
    }
}
