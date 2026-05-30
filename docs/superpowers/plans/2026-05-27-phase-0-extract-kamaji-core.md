# Phase 0 — Extract `kamaji-core` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the single-crate `kamaji` repo into a two-member Cargo workspace, extracting the pure domain modules into a new `kamaji-core` library crate, with **zero behavior change** and every commit passing CI.

**Architecture:** A workspace `Cargo.toml` at the repo root holds two members under `crates/`: `kamaji-core` (library, no UI) and `kamaji` (binary, ratatui TUI + CLI, depends on `kamaji-core`). Modules move in dependency-tier order (leaves first); after each tier the workspace builds clean, `cargo fmt`/`clippy` are quiet, and `cargo test --workspace` passes. The binary keeps its name (`target/release/kamaji`) so the existing release workflow keeps working unchanged.

**Tech Stack:** Cargo workspaces, Rust 2021 (toolchain unchanged from today's `Cargo.toml`).

**Background spec:** `docs/superpowers/specs/2026-05-27-browser-first-pivot-design.md`, §8 Phase 0.

**Repo conventions reminder (from `CLAUDE.md`):**
- All work happens on a branch in `../kamaji-worktrees/<branch>/`, never on `main` directly. The executing skill creates this; the plan assumes you are already in it.
- Commit format mirrors the existing history (`feat(scope): …`, `chore: …`, `refactor: …`).
- Ship flow at the end: `gh pr create --fill --base main` → `gh pr merge --squash --auto --delete-branch`.
- After the merge, `gh pr merge --delete-branch` is known to error from inside a worktree but the merge itself still succeeds — verify with `gh pr view` and clean up the worktree + remote branch manually.

---

## File Structure (target layout after Phase 0)

```
kamaji/                                 # repo root
├── Cargo.toml                          # workspace manifest only
├── Cargo.lock
├── crates/
│   ├── kamaji-core/                    # NEW: pure-domain library
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                  # NEW: re-exports the modules below
│   │       ├── agent.rs                # moved from src/
│   │       ├── config.rs               # moved
│   │       ├── db.rs                   # moved
│   │       ├── detect.rs               # moved
│   │       ├── git.rs                  # moved
│   │       ├── layout.rs               # moved
│   │       ├── models.rs               # moved
│   │       ├── session.rs              # moved
│   │       ├── slug.rs                 # moved
│   │       ├── zellij.rs               # moved
│   │       └── zellij_config.rs        # moved
│   └── kamaji/                         # binary, stays the user-facing CLI
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs                 # moved from src/
│           ├── cli.rs                  # moved
│           ├── app.rs                  # moved (TUI-coupled)
│           ├── dir_select.rs           # moved (only TUI uses it)
│           ├── engine.rs               # moved (uses ratatui::crossterm)
│           ├── picker.rs               # moved
│           ├── theme.rs                # moved
│           ├── update.rs               # moved (self-update; binary concern)
│           └── ui/
│               ├── mod.rs              # moved
│               ├── board.rs            # moved
│               └── modals.rs           # moved
└── docs/, .github/, install.sh, …     # unchanged
```

### Why this split

A grep for `ratatui|crossterm` across `src/` confirms the UI-coupled set:
`engine.rs`, `main.rs`, `picker.rs`, `theme.rs`, `ui/board.rs`, `ui/modals.rs`,
`ui/mod.rs`. Plus `app.rs` (uses `theme::Theme` which is `ratatui::style::Color`)
and `cli.rs` (binary subcommand dispatcher). `update.rs` (self-update) and
`dir_select.rs` are pure logic today but are only consumed by the binary, so
they stay in the binary to keep the `kamaji-core` surface focused on what the
daemon will need in later phases. Everything else is pure domain logic with no
UI dependency and moves to `kamaji-core`.

### Dependency tiers (move order — leaves first)

Each tier compiles and passes tests on its own once its imports are rewired:

- **Tier 1 (no internal deps):** `slug`, `models`, `layout`, `git`, `zellij`
- **Tier 2 (depend on Tier 1):** `config` (uses `models`), `zellij_config` (uses `layout`)
- **Tier 3 (depend on Tiers 1–2):** `db` (uses `models`), `agent` (uses `config`), `detect` (uses `models`)
- **Tier 4 (depends on Tiers 1–3):** `session` (uses `config`, `db`, `models`, `agent`, `detect`, `git`, `layout`, `slug`, `zellij_config`)

---

## Verification commands (used at every checkpoint)

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

These mirror exactly what `.github/workflows/ci.yml` runs on every PR. If any of them fail at a checkpoint, **fix it before committing** — never proceed with a red commit.

To make the diffs surgical, prefer **`git mv`** over delete-then-create (preserves blame).

---

## Task 1: Convert to Cargo workspace (binary only, no code changes)

This task does not move any `.rs` files between crates. It only relocates the existing single binary crate into `crates/kamaji/` and adds a workspace manifest at the root. Nothing else changes; the build output (`target/release/kamaji`) and behavior are byte-equivalent.

**Files:**
- Create: `Cargo.toml` (NEW workspace manifest at root, replacing the current package manifest)
- Create: `crates/kamaji/Cargo.toml` (the existing package manifest, moved)
- Move: `src/**` → `crates/kamaji/src/**` (preserves layout including `src/ui/`)
- Move: `tests/**` → `crates/kamaji/tests/**` if `tests/` exists at the root (it currently does not, but check)

- [ ] **Step 1: Move the existing binary crate into `crates/kamaji/`**

```bash
mkdir -p crates/kamaji
git mv src crates/kamaji/src
# Move tests/ only if it exists at the repo root:
if [ -d tests ]; then git mv tests crates/kamaji/tests; fi
git mv Cargo.toml crates/kamaji/Cargo.toml
```

- [ ] **Step 2: Create the new root workspace manifest**

Write `Cargo.toml` (at the repo root, NOT inside `crates/`) with this exact content:

```toml
[workspace]
resolver = "2"
members = ["crates/kamaji"]
```

Note `resolver = "2"` — required for the 2021 edition in workspaces; without it cargo will warn.

- [ ] **Step 3: Let cargo update `Cargo.lock` in place**

`Cargo.lock` stays at the repo root (workspaces have a single lockfile there). `git mv` only moved `Cargo.toml`; the lockfile is untouched. Do NOT delete it — that would unpin dep versions during a refactor that should not change them.

```bash
cargo build --workspace
git diff Cargo.lock | head -40
```

Expected: build succeeds. The `Cargo.lock` diff is trivial workspace-metadata adjustments only — **no dep version changes**. If a version did change, stop and investigate before committing; this task must not bump dependencies.

Also confirm only one lockfile exists:

```bash
find . -name Cargo.lock -not -path './target/*'
```

Expected: a single line, `./Cargo.lock`. No lockfile inside `crates/kamaji/`.

- [ ] **Step 4: Run the full verification suite**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Expected: all three pass. Same test count as before the move (no tests added or removed).

- [ ] **Step 5: Smoke-test the binary path**

```bash
cargo build --release
ls -la target/release/kamaji   # must exist (Linux/macOS) — Windows: kamaji.exe
```

Expected: binary at the same path the release workflow packages from. Do NOT run the binary against your real `~/.local/share/kamaji/kamaji.db` — there is no behavior change, but be cautious; a `--help` invocation is enough confirmation:

```bash
./target/release/kamaji --help
```

Expected: same help text as before the refactor.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: convert to cargo workspace with single binary member

Pure relocation: src/ → crates/kamaji/src/, package manifest →
crates/kamaji/Cargo.toml. Root Cargo.toml is a workspace manifest.
No code changes; binary path and behavior unchanged. Phase 0 step 1
of the browser-first pivot (kamaji-core extraction)."
```

---

## Task 2: Add the empty `kamaji-core` library crate

Create `kamaji-core` with all the dependencies it will eventually need, so subsequent move tasks can drop a module in and call `cargo build` without juggling `Cargo.toml`. The binary depends on `kamaji-core` from this point on, but the binary's source still uses its own modules — so `kamaji-core` is an unused dependency for now (one clippy warning to address by re-exporting nothing yet; we suppress it by adding a single trivial item to `lib.rs`).

**Files:**
- Create: `crates/kamaji-core/Cargo.toml`
- Create: `crates/kamaji-core/src/lib.rs`
- Modify: `Cargo.toml` (workspace members list)
- Modify: `crates/kamaji/Cargo.toml` (add `kamaji-core` dependency)

- [ ] **Step 1: Write `crates/kamaji-core/Cargo.toml`**

```toml
[package]
name = "kamaji-core"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
anyhow = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
directories = "5"

[dev-dependencies]
tempfile = "3"
```

Rationale: these are the deps the eventual move targets need —
`anyhow` (most modules), `rusqlite` (`db`), `serde`+`toml` (`config`),
`directories` (`config` for XDG paths, `detect` for state dir),
`tempfile` (existing `session` and `detect` tests). `ureq`, `serde_json`,
`sha2`, and `zip` stay in the binary (only `update.rs` uses them, and
`update.rs` stays in the binary).

- [ ] **Step 2: Write `crates/kamaji-core/src/lib.rs`**

```rust
//! Pure domain logic for kamaji: SQLite-backed Kanban model, git worktree
//! orchestration, zellij CLI integration, and agent command templates. No UI,
//! no transport. Both the existing TUI binary and the upcoming `kamajid`
//! daemon depend on this crate.
//!
//! Modules are added incrementally as Phase 0 extracts them from the binary;
//! see `docs/superpowers/plans/2026-05-27-phase-0-extract-kamaji-core.md`.
```

(Intentionally bare — Tier 1 adds the first `pub mod` declarations. Without any items the crate still compiles; an empty `lib.rs` is valid Rust.)

- [ ] **Step 3: Add `kamaji-core` to the workspace members**

Edit the root `Cargo.toml` to:

```toml
[workspace]
resolver = "2"
members = [
    "crates/kamaji-core",
    "crates/kamaji",
]
```

- [ ] **Step 4: Add `kamaji-core` as a path dependency from the binary**

Edit `crates/kamaji/Cargo.toml`, adding under `[dependencies]` (alongside the existing entries, exact placement doesn't matter):

```toml
kamaji-core = { path = "../kamaji-core" }
```

- [ ] **Step 5: Verify the workspace still builds and tests pass**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Expected: all pass. The new `kamaji-core` crate has no source items, so it builds trivially. Because the binary depends on it but uses nothing from it yet, clippy may emit `unused_crate_dependencies` only if that lint is enabled — the project does not enable it (verified by the `ci.yml` clippy invocation using the default lints + `-D warnings`), so this is fine. If you see an unexpected warning, do not silence it with `#[allow]`; instead delete this paragraph's assumption and fix the underlying cause.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: add empty kamaji-core library crate

Workspace member with the dependency set required by the modules
that will move into it during Tier 1–4 tasks. The binary now
depends on kamaji-core but does not yet use it. Phase 0 step 2."
```

---

## Task 3: Move Tier 1 modules (`slug`, `models`, `layout`, `git`, `zellij`) into `kamaji-core`

Tier 1 are pure leaves — they import nothing from any other `crate::` module today. They move first, alone.

**Files:**
- Move: `crates/kamaji/src/{slug,models,layout,git,zellij}.rs` → `crates/kamaji-core/src/`
- Modify: `crates/kamaji-core/src/lib.rs` (declare the moved modules as `pub mod`)
- Modify: any file in `crates/kamaji/src/**` that contains `use crate::{slug, models, layout, git, zellij}` (or `crate::slug`, `crate::models::*`, etc.) — rewrite to `kamaji_core::…`

- [ ] **Step 1: Move the five module files**

```bash
git mv crates/kamaji/src/slug.rs    crates/kamaji-core/src/slug.rs
git mv crates/kamaji/src/models.rs  crates/kamaji-core/src/models.rs
git mv crates/kamaji/src/layout.rs  crates/kamaji-core/src/layout.rs
git mv crates/kamaji/src/git.rs     crates/kamaji-core/src/git.rs
git mv crates/kamaji/src/zellij.rs  crates/kamaji-core/src/zellij.rs
```

- [ ] **Step 2: Declare the modules in `kamaji-core/src/lib.rs`**

Append to `crates/kamaji-core/src/lib.rs`:

```rust
pub mod git;
pub mod layout;
pub mod models;
pub mod slug;
pub mod zellij;
```

(Alphabetical within the tier — keeps later tier additions easy to merge.)

- [ ] **Step 3: Find every binary-side reference to the moved modules**

```bash
grep -rnE 'crate::(slug|models|layout|git|zellij)\b' crates/kamaji/src/
```

Expected output: a list of files (currently `app.rs`, `cli.rs`, `engine.rs`, `picker.rs`, `theme.rs`, `ui/board.rs`, `ui/modals.rs`, `ui/mod.rs`). Note the file list — these are the only files needing edits.

- [ ] **Step 4: Rewrite the imports in each listed file**

In every file the grep listed, replace the relevant `use crate::X;` lines with `use kamaji_core::X;`. Examples (the exact lines depend on the file):

- `use crate::models::{Agent, Project, Status, Ticket};` → `use kamaji_core::models::{Agent, Project, Status, Ticket};`
- `use crate::{git, slug, zellij};` (in `engine.rs`) → `use kamaji_core::{git, slug, zellij};`
- `use crate::models::Status;` (in `theme.rs`) → `use kamaji_core::models::Status;`

Inline `crate::X::…` paths (not in a `use`) must change too — re-run the grep with `-E 'crate::(slug|models|layout|git|zellij)::'` to catch any. The standard refactor is to add a `use` line and shorten, or just write `kamaji_core::X::Y` inline. Either is fine; pick the form the surrounding file already uses.

- [ ] **Step 5: Verify the workspace builds**

```bash
cargo build --workspace
```

Expected: clean build. If you see `unresolved import crate::slug` or similar, you missed a reference — re-run the Step 3 grep.

- [ ] **Step 6: Run the full verification suite**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Expected: all pass. Test count unchanged from before this task — the tests embedded in the moved files (`#[cfg(test)] mod tests { … }`) now run as part of `kamaji-core`'s test binary instead of `kamaji`'s, but the same assertions still execute.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor: move tier-1 pure modules to kamaji-core

slug, models, layout, git, zellij — no internal cross-deps; moved
as-is with import paths in the binary rewritten from crate::X to
kamaji_core::X. Phase 0 step 3."
```

---

## Task 4: Move Tier 2 modules (`config`, `zellij_config`) into `kamaji-core`

Tier 2 depends on Tier 1 (which is already inside `kamaji-core`), so the intra-core `use crate::layout::BarStyle` etc. inside these files continues to work unchanged after the move.

**Files:**
- Move: `crates/kamaji/src/{config,zellij_config}.rs` → `crates/kamaji-core/src/`
- Modify: `crates/kamaji-core/src/lib.rs`
- Modify: binary files that import `crate::{config, zellij_config}`

- [ ] **Step 1: Move the two files**

```bash
git mv crates/kamaji/src/config.rs        crates/kamaji-core/src/config.rs
git mv crates/kamaji/src/zellij_config.rs crates/kamaji-core/src/zellij_config.rs
```

- [ ] **Step 2: Declare the modules in `kamaji-core/src/lib.rs`**

Add to the `pub mod` block (keep alphabetical):

```rust
pub mod config;
pub mod git;
pub mod layout;
pub mod models;
pub mod slug;
pub mod zellij;
pub mod zellij_config;
```

(Now the block is the union of Tier 1 + Tier 2.)

- [ ] **Step 3: Confirm intra-core imports are correct**

The moved `config.rs` has `use crate::models::Agent;` — still correct (both modules are now in `kamaji-core`). The moved `zellij_config.rs` has `use crate::layout::BarStyle;` — still correct. No edits needed inside the moved files.

- [ ] **Step 4: Find and rewrite binary-side references**

```bash
grep -rnE 'crate::(config|zellij_config)\b' crates/kamaji/src/
```

For every match, replace `crate::config` → `kamaji_core::config` and `crate::zellij_config` → `kamaji_core::zellij_config`. Expected consumers: `cli.rs`, `engine.rs`, possibly `main.rs`.

Also catch the bare-call form: `crate::config::config_path()` in `engine.rs` becomes `kamaji_core::config::config_path()`. Re-grep with `crate::(config|zellij_config)::` to be sure.

- [ ] **Step 5: Verify**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: move tier-2 config modules to kamaji-core

config and zellij_config — depend on tier-1 modules already in core;
binary imports rewritten to kamaji_core::. Phase 0 step 4."
```

---

## Task 5: Move Tier 3 modules (`db`, `agent`, `detect`) into `kamaji-core`

**Files:**
- Move: `crates/kamaji/src/{db,agent,detect}.rs` → `crates/kamaji-core/src/`
- Modify: `crates/kamaji-core/src/lib.rs`
- Modify: binary files importing `crate::{db, agent, detect}`

- [ ] **Step 1: Move the three files**

```bash
git mv crates/kamaji/src/db.rs     crates/kamaji-core/src/db.rs
git mv crates/kamaji/src/agent.rs  crates/kamaji-core/src/agent.rs
git mv crates/kamaji/src/detect.rs crates/kamaji-core/src/detect.rs
```

- [ ] **Step 2: Declare the modules in `kamaji-core/src/lib.rs`**

```rust
pub mod agent;
pub mod config;
pub mod db;
pub mod detect;
pub mod git;
pub mod layout;
pub mod models;
pub mod slug;
pub mod zellij;
pub mod zellij_config;
```

- [ ] **Step 3: Intra-core imports are already correct**

`db.rs` uses `crate::models::{…}`; `agent.rs` uses `crate::config::AgentCommands`; `detect.rs` uses `crate::models::Status`. All three target modules are already inside `kamaji-core`, so the `crate::…` paths still resolve. No edits inside moved files.

- [ ] **Step 4: Find and rewrite binary-side references**

```bash
grep -rnE 'crate::(db|agent|detect)\b' crates/kamaji/src/
```

For every match: `crate::db` → `kamaji_core::db`, `crate::agent` → `kamaji_core::agent`, `crate::detect` → `kamaji_core::detect`. Expected consumers include `app.rs`, `cli.rs`, `engine.rs`, `main.rs`, `picker.rs`, `ui/board.rs`, `ui/mod.rs`. Re-grep with `crate::(db|agent|detect)::` to catch bare-call forms (e.g. `crate::detect::default_state_dir()` in `engine.rs`).

- [ ] **Step 5: Verify**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Expected: all pass. The `detect` module tests come along with it; the `db` module tests likewise.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: move tier-3 modules to kamaji-core

db, agent, detect — depend on tiers 1–2 already in core; binary
imports rewritten to kamaji_core::. Phase 0 step 5."
```

---

## Task 6: Move Tier 4 module (`session`) into `kamaji-core`

Last module move. After this, `kamaji-core` contains the entire pure-domain layer described in the spec.

**Files:**
- Move: `crates/kamaji/src/session.rs` → `crates/kamaji-core/src/session.rs`
- Modify: `crates/kamaji-core/src/lib.rs`
- Modify: `crates/kamaji/src/{cli,engine}.rs` (the only known consumers of `crate::session`)

- [ ] **Step 1: Move the file**

```bash
git mv crates/kamaji/src/session.rs crates/kamaji-core/src/session.rs
```

- [ ] **Step 2: Declare the module in `kamaji-core/src/lib.rs`**

```rust
pub mod agent;
pub mod config;
pub mod db;
pub mod detect;
pub mod git;
pub mod layout;
pub mod models;
pub mod session;
pub mod slug;
pub mod zellij;
pub mod zellij_config;
```

- [ ] **Step 3: Intra-core imports remain correct**

`session.rs` imports `crate::{agent, detect, git, layout, slug, zellij_config}` and `crate::config::Config`, `crate::db::Db`, `crate::models::{Agent, Project, Status, Ticket}`. All of these now resolve within `kamaji-core`. No edits inside the moved file.

- [ ] **Step 4: Find and rewrite binary-side references**

```bash
grep -rnE 'crate::session\b' crates/kamaji/src/
```

Replace `crate::session` with `kamaji_core::session` (and `crate::session::Prepared` → `kamaji_core::session::Prepared`, etc.). Expected consumers: `cli.rs`, `engine.rs`.

- [ ] **Step 5: Verify**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Expected: all pass. The `session.rs` tests (`prepare_main_session_*`) now run as part of `kamaji-core`'s test binary.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: move session orchestration to kamaji-core

session.rs depends on every other tier-1–3 module; this completes
the kamaji-core extraction. The binary now contains only UI-coupled
modules (engine, app, ui, picker, theme, dir_select, update, cli,
main) and depends on kamaji-core for all domain logic. Phase 0
step 6."
```

---

## Task 7: Final hygiene — prune now-unused binary deps, smoke-test the user flow, PR

After all moves the binary's `Cargo.toml` lists deps that may now only be used through `kamaji-core` (and `kamaji-core` re-exports nothing — the binary uses them via `kamaji_core::…` paths, not directly). Confirm with `cargo` what's actually unused, prune, then run the full board flow end-to-end before opening the PR.

**Files:**
- Modify: `crates/kamaji/Cargo.toml` (prune unused deps)

- [ ] **Step 1: Identify unused binary deps**

```bash
cargo build --workspace 2>&1 | grep -i 'unused'
```

If no output, install `cargo-udeps` only if you already have it; otherwise inspect manually. Expected candidates to prune from `crates/kamaji/Cargo.toml` `[dependencies]`:

- `rusqlite` — only `kamaji-core::db` touches SQLite.
- `toml` — only `kamaji-core::config` parses TOML.

**Keep** these in the binary because UI-coupled modules still use them directly:
- `anyhow` (most binary files have `use anyhow::Result`)
- `directories` (`update.rs` resolves XDG paths)
- `serde` + `serde_json` (`update.rs` deserializes the GitHub Releases JSON)
- `ureq` (`update.rs` does HTTP)
- `sha2` (`update.rs` checks release archive hashes)
- `ratatui` (UI)
- `[target.'cfg(windows)'.dependencies] zip` (`update.rs` extracts Windows .zip archives)
- `tempfile` (dev-dep, used by tests of binary-only modules; if `cargo test -p kamaji` runs no tests that need it, you may remove — verify by leaving in if unsure, this is a hygiene step, not a correctness one)

If a dep you'd expect to prune is still being used somewhere in `crates/kamaji/src/`, leave it in. The rule is: only remove deps that produce an actual compile error if absent.

- [ ] **Step 2: Apply the pruning**

Edit `crates/kamaji/Cargo.toml` to remove the deps Step 1 identified. After pruning, the file's `[dependencies]` table should be roughly:

```toml
[dependencies]
kamaji-core = { path = "../kamaji-core" }
ratatui = "0.29"
anyhow = "1"
directories = "5"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ureq = "2"
sha2 = "0.10"

[target.'cfg(windows)'.dependencies]
zip = { version = "2", default-features = false, features = ["deflate"] }

[dev-dependencies]
tempfile = "3"
```

(If a dep you removed turns out to be needed, `cargo build` fails and you put it back. That's the verification.)

- [ ] **Step 3: Verify**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Expected: all pass.

- [ ] **Step 4: Smoke-test the binary end-to-end**

Build and run a sanity check that exercises both the binary→core boundary (CLI subcommands hitting the DB through `kamaji_core::db`) and the layout path the release uses:

```bash
cargo build --release
ls -la target/release/kamaji
./target/release/kamaji --help
./target/release/kamaji projects list   # or whichever read-only subcommand exists
```

Expected: identical output to running the pre-refactor binary. **Do not** create/modify projects or tickets against your real DB during this smoke — `--help` plus one read-only subcommand is enough.

If you want a stronger end-to-end check, launch the TUI in a throwaway tmp project: set `XDG_DATA_HOME` and `XDG_CONFIG_HOME` to tmp dirs first so it can't touch your real state:

```bash
XDG_DATA_HOME=$(mktemp -d) XDG_CONFIG_HOME=$(mktemp -d) ./target/release/kamaji
# Press 'q' to quit. Expected: the project picker opens and quits cleanly.
```

- [ ] **Step 5: Commit the pruning (if any)**

```bash
git add crates/kamaji/Cargo.toml
git commit -m "chore: prune deps from kamaji binary now covered by kamaji-core

After the tier moves, rusqlite and toml are only used through
kamaji_core. Removing them from the binary crate keeps deps
honest. Phase 0 step 7 (final hygiene)."
```

(If Step 1 found nothing to prune, skip this commit and proceed.)

- [ ] **Step 6: Push and open the PR**

```bash
git push -u origin "$(git branch --show-current)"
gh pr create --fill --base main
```

The commit history (six or seven commits, one per task) becomes the PR's body via `--fill`. Each commit is independently green, so reviewers can bisect.

- [ ] **Step 7: Enable auto-merge with branch delete**

```bash
gh pr merge --squash --auto --delete-branch
```

Per the known worktree gotcha, the post-merge local-checkout step may error from inside the worktree; that's fine. Verify the merge took with:

```bash
gh pr view --json state,mergedAt -q .
```

Expected: `state` is `MERGED` (or `OPEN` with auto-merge enabled if CI is still running). If `MERGED`, finish the cleanup (back in the primary worktree at `/home/victor/dev/kamaji`):

```bash
cd /home/victor/dev/kamaji
git checkout main && git pull --ff-only
git worktree remove ../kamaji-worktrees/<branch>
git branch -d <branch>
# Delete remote branch if --delete-branch didn't (it usually doesn't from a worktree):
git push origin --delete <branch> 2>/dev/null || true
git fetch --prune origin
```

---

## End-of-Phase verification

After the PR merges, on `main`:

```bash
git checkout main && git pull --ff-only
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
./target/release/kamaji --help
```

All should pass / produce the same output as before Phase 0. If anything regresses on `main`, that's the signal to revert the PR rather than patch forward.

## What this plan deliberately does NOT do

- **No daemon, no HTTP API, no SSE, no browser, no `kamajid` crate.** Those are Phase 1+, each their own spec → plan.
- **No splitting of `engine.rs`** (2065 lines today). It stays in the binary as-is. A future phase will replace much of it with daemon API calls, at which point its remainder will shrink naturally; splitting it pre-emptively would be churn against unknown future shape.
- **No moving `update.rs` to `kamaji-core`.** It's binary-only today; in a future phase it likely moves to `kamajid` (the daemon also needs self-update), not to `kamaji-core`.
- **No new behavior, no new tests.** Phase 0 is a pure structural refactor. The existing test suite is the regression check.
