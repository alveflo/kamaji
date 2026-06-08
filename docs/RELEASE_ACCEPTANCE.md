# kamaji — pre-release acceptance checklist

Run this before tagging a release. It's a **manual QA sweep** covering the major
user-facing features; most items are integration-level and won't be caught by
`cargo test`.

**How to use it**
- **Major releases:** run the whole checklist.
- **Point releases / hotfixes:** run the [Quick smoke subset](#quick-smoke-subset)
  plus the sections touching whatever changed.
- Each item traces to the PR(s) that introduced it — prune items as features
  stabilize, and add a row whenever a notable feature lands.

**Preconditions:** `zellij`, `git`, and at least one agent CLI
(`claude`/`codex`/`copilot`) on `PATH`. Exercise both the browser board
(`http://127.0.0.1:8755`) **and** the TUI (`kamaji`) where noted — they share one
backend, so a regression can hide on one surface.

---

## Quick smoke subset

The minimum to call a build shippable:

- [ ] `cargo build --release && cargo test` clean; `kamaji --version` and
  `GET /healthz` report the new version.
- [ ] TUI auto-spawns the daemon and connects; board loads at `:8755`.
- [ ] Create a ticket → start it → attach in the browser: agent is live and
  interactive.
- [ ] Move a card in the browser → it updates in the TUI within ~1s.
- [ ] Done/Delete use the in-page modal and update the board live.

---

## 0. Build & version sanity
- [ ] **Workspace builds & tests** — `cargo build --release && cargo test` is clean.
- [ ] **fmt/clippy clean** — `cargo fmt --check && cargo clippy --all-targets -- -D warnings`.
- [ ] **Version reported** — `kamaji --version` and `GET /healthz` both return the new version string.

## 1. Daemon lifecycle & discovery *(#70, #77, #136)*
- [ ] **TUI auto-spawns the daemon** — with no daemon running, launch `kamaji`; it should start `kamajid` detached and connect. `make status` then reports up.
- [ ] **Make controls** — `make start` / `make stop` / `make restart` / `make status` / `make logs` each behave as labeled; pid/addr files appear in the runtime dir and are removed on clean stop.
- [ ] **Stale pidfile recovery** — `kill -9` the daemon, then launch `kamaji`; it detects the stale pidfile, clears it, and respawns cleanly.
- [ ] **Health address correct** — daemon binds and advertises the address it actually listens on (no localhost/0.0.0.0 mismatch).

## 2. Reboot persistence & autostart *(#89, #147)*
- [ ] **Service install** — `make install-service` installs `kamajid` to `~/.local/bin`, enables the systemd user unit, and reports linger status. `systemctl --user status kamajid` shows active.
- [ ] **Survives logout/reboot** — start a ticket session, reboot, log in; the board comes back on its own and `systemctl --user status kamajid` is active without manual start.
- [ ] **Session resurrects on attach** — after the reboot, open the in-progress ticket; the agent relaunches via its resume command (`claude resume` / `codex resume --last` / `copilot --continue`) in the original worktree and the conversation continues.
- [ ] **Serialization forced** — inspect kamaji's generated session config (`$TMPDIR/kamaji-zellij/web-config.kdl`); it contains `session_serialization true` even if the user's `~/.config/zellij/config.kdl` sets it false.
- [ ] **Uninstall clean** — `make uninstall-service` disables/removes the unit; daemon no longer auto-starts.

## 3. Tickets & board CRUD *(#95, #132, #138, #139, #142)*
- [ ] **Create (browser)** — `n` (or the New button) opens the in-page modal; create a ticket; card appears in Todo. Agent picker actually switches the selected agent *(#138)*.
- [ ] **Create (TUI / CLI)** — `n` in the TUI and `kamaji ticket "<title>"` both create; `--background` also starts the session.
- [ ] **Edit / move** — drag a card across columns in the browser; move it via keys in the TUI; status updates persist.
- [ ] **Delete / Done use in-page modal** — Done/Delete trigger the styled modal, *not* a native `confirm()` *(#139)*; confirming removes/advances the card live.

## 4. Agent sessions & worktrees *(#72, #76, #123, #124, #130)*
- [ ] **Start session** — starting a ticket creates a git worktree + branch and spawns a detached zellij session `kamaji-<id>-<slug>` running the chosen agent with the initial prompt.
- [ ] **Each agent type launches** — verify Claude, Codex, and Copilot each start with the correct command template.
- [ ] **Empty command rejected** — a misconfigured/empty agent layout command is rejected with a clear error, not a broken session *(#130)*.
- [ ] **No env bleed** — start kamaji from *inside* a zellij session; new sessions are still created as real sessions (`ZELLIJ*` scrubbed), not injected as a tab *(#123)*.
- [ ] **Layout cleanup** — single-use layout files under `$TMPDIR/kamaji-layouts/` are deleted after zellij consumes them *(#124)*.

## 5. Browser terminal — zellij web + proxy *(#93, #100, #101, #129, #131, #132, #133, #134)*
- [ ] **Inline attach** — clicking a started ticket opens the embedded terminal modal; the agent is live and interactive, no token prompt *(#93, #131)*.
- [ ] **Glyphs render** — Nerd Font icons in the agent UI show as glyphs, not tofu *(#133)*.
- [ ] **Theme match** — with `web_theme` set, the browser terminal palette matches the board *(#134)*.
- [ ] **Reconnect resilience** — kill/restart the daemon or drop the network briefly; the inline terminal reconnects quietly and bounded (no infinite flicker loop) *(#100, #101)*.
- [ ] **Close terminal** — `Ctrl-Q` (and the ✕) close the terminal panel cleanly *(#132)*.
- [ ] **Large output stable** — run a command producing a big burst of output; the proxy caps bodies without crashing *(#129)*.

## 6. Live updates / SSE sync *(#88, #90)*
- [ ] **Cross-surface sync** — move a card in the browser; it updates in the TUI and in a second browser tab within ~1s (and vice versa).
- [ ] **Working indicator** — while an agent is actively running, the card shows the "working" bullet/indicator; it clears when idle *(#88)*.
- [ ] **Reconnect refetch** — disconnect a client past the SSE buffer; on reconnect it re-fetches full board state (no stale cards).

## 7. Auto-review (idle-session detection) *(#72)*
- [ ] **Idle → review** — let an in-progress agent session go idle (finished/awaiting input); the poll loop moves the ticket to the review column and emits the event to both surfaces.
- [ ] **Reconcile on restart** — restart the daemon; tickets whose zellij sessions vanished are reconciled (EXITED-but-serialized sessions are *kept*, truly-gone ones cleared).

## 8. PWA *(#141)*
- [ ] **Installable** — the browser offers "Install"; `manifest.webmanifest` and `sw.js` load; the installed window opens the board. (No offline mode by design — confirm it requires the live daemon.)

## 9. Session cleanup modal *(#142)*
- [ ] **List & delete** — the cleanup modal lists current zellij sessions and lets you delete them from the board; deleted sessions disappear from `zellij list-sessions`.

## 10. Container mode *(#143)*
- [ ] **`kamaji up`** — brings up the daemon in a container; board reachable on `:8755`, proxy `:8756`; worktrees + agent creds resolve at identical paths.
- [ ] **`kamaji down` / `kamaji logs` / `kamaji status`** — manage the container as labeled.
- [ ] **Quadlet path** — the `kamaji.container` Quadlet starts via `systemctl --user start kamaji` (rootless Podman).

## 11. Self-update & version-skew *(#87)*
- [ ] **Update check** — with an older binary, the TUI surfaces the "update available" toast from the background check.
- [ ] **Skew warning** — run a TUI whose version differs from the daemon's; the version-skew warning toast appears.

## 12. Config & cross-platform
- [ ] **Config respected** — `[daemon] bind`, `zellij_bar`, `web_theme`, and agent command overrides in `~/.config/kamaji/config.toml` take effect.
- [ ] **XDG paths** — DB at `~/.local/share/kamaji/kamaji.db`, config at `~/.config/kamaji/`, runtime files in `$XDG_RUNTIME_DIR/kamaji/`.
- [ ] **(If shipping Windows)** known gap: the `container::plan` mount-path test currently fails on Windows — verify it's build-only and doesn't affect the shipped binary.

---

## Known pre-existing CI reds

Track (don't necessarily block) these — they are red on `main` independent of any
given release:

- **Browser smoke** — the board Delete-card end-to-end test is flaky.
- **Windows build** — `container::plan::tests::derives_root_and_worktree_mounts_identical_paths`
  fails on Windows path separators (from #143).
