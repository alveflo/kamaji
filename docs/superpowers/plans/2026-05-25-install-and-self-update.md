# Install script + in-app self-update — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users install kamaji with `curl …/install.sh | sh` (no cargo), and let a running kamaji detect a newer release and self-update on one keypress.

**Architecture:** A tag-triggered GitHub Actions workflow cross-compiles four Unix targets and uploads `kamaji-<target>.tar.gz` + `.sha256` to the Release. A POSIX `install.sh` downloads the matching asset from the `latest/download` redirect. Inside the app, a new `src/update.rs` module checks the GitHub API on launch (cached 24h, on a background thread, results surfaced via a shared `Arc<Mutex<…>>`), shows a status-bar banner, and on `u` downloads + atomically replaces the running binary.

**Tech Stack:** Rust, ratatui, `ureq` (rustls TLS), `serde_json`, `sha2`; GitHub Actions; POSIX shell.

**Reference spec:** `docs/superpowers/specs/2026-05-25-install-and-self-update-design.md`

**Shared contract:** release asset name is `kamaji-<target>.tar.gz` where `<target>` ∈
`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`. The workflow, `install.sh`, and `src/update.rs` must all agree on it.

---

## File structure

- **Create** `install.sh` (repo root) — POSIX install script.
- **Create** `.github/workflows/release.yml` — tag-triggered build + upload.
- **Modify** `.github/workflows/ci.yml` — add a `shellcheck` job.
- **Modify** `Cargo.toml` — add `ureq`, `serde_json`, `sha2` deps.
- **Create** `src/update.rs` — version parse/compare, target mapping, API parse, cache, network fetch, `check()`, `self_update()`.
- **Modify** `src/main.rs` — declare `mod update`; handle `Command::Version`; spawn the background check thread; thread the shared status into `run`/`run_board`; handle `Effect::SelfUpdate`.
- **Modify** `src/cli.rs` — add `Command::Version` and parse `--version`/`-V`/`version`.
- **Modify** `src/engine.rs` — add `Effect::SelfUpdate { version }`; handle the `u` key.
- **Modify** `src/app.rs` — add `App.update: Option<String>` field.
- **Modify** `src/ui/board.rs` — render the update banner in the status line.
- **Modify** `src/ui/modals.rs` — add the `u` hint to the help text.
- **Modify** `README.md` — add an Install section.

---

# Phase 1 — Distribution

## Task 1: Add a `--version` flag to the CLI

The install script confirms success with `kamaji --version`, and it's the natural place to expose the version.

**Files:**
- Modify: `src/cli.rs` (the `Command` enum near line 18; `parse` near line 60; `USAGE` near line 9)
- Modify: `src/main.rs` (the `match` in `main` near line 35)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of `src/cli.rs` (if no test module exists, create `#[cfg(test)] mod tests { use super::*;` … `}` at end of file):

```rust
#[test]
fn parses_version_flag() {
    assert_eq!(parse(["--version"]).unwrap(), Command::Version);
    assert_eq!(parse(["-V"]).unwrap(), Command::Version);
    assert_eq!(parse(["version"]).unwrap(), Command::Version);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib cli::tests::parses_version_flag`
Expected: FAIL — `no variant named Version found for enum Command`.

- [ ] **Step 3: Add the `Version` variant**

In `src/cli.rs`, add to the `Command` enum (currently `Tui`, `Help`, `CreateTicket`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Tui,
    Help,
    Version,
    CreateTicket(CreateTicketArgs),
}
```

- [ ] **Step 4: Handle the flag in `parse`**

In `parse`, just after the existing `--help`/`-h`/`help` check, add:

```rust
    if args == ["--version"] || args == ["-V"] || args == ["version"] {
        return Ok(Command::Version);
    }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib cli::tests::parses_version_flag`
Expected: PASS.

- [ ] **Step 6: Handle `Command::Version` in `main`**

In `src/main.rs`, add an arm to the `match cli::parse(...)?` in `main` (alongside `Command::Help`):

```rust
        cli::Command::Version => {
            println!("kamaji {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
```

- [ ] **Step 7: Verify it builds and runs**

Run: `cargo run -- --version`
Expected: prints `kamaji 0.1.0`.

- [ ] **Step 8: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat(cli): add --version flag"
```

---

## Task 2: Write `install.sh`

**Files:**
- Create: `install.sh` (repo root)

- [ ] **Step 1: Write the script**

Create `install.sh` with exactly this content:

```sh
#!/bin/sh
# kamaji installer. Usage:
#   curl -fsSL https://raw.githubusercontent.com/alveflo/kamaji/main/install.sh | sh
# Override the install directory with KAMAJI_INSTALL_DIR (default: ~/.local/bin).
set -eu

REPO="alveflo/kamaji"
INSTALL_DIR="${KAMAJI_INSTALL_DIR:-$HOME/.local/bin}"

err() { printf 'error: %s\n' "$1" >&2; exit 1; }

# Pick a downloader.
if command -v curl >/dev/null 2>&1; then
  dl() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  dl() { wget -qO "$2" "$1"; }
else
  err "need curl or wget"
fi

# Map uname -> Rust target triple (must match release asset names).
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)  os_part="unknown-linux-musl" ;;
  Darwin) os_part="apple-darwin" ;;
  *) err "unsupported OS: $os" ;;
esac
case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  aarch64|arm64) arch_part="aarch64" ;;
  *) err "unsupported architecture: $arch" ;;
esac
target="${arch_part}-${os_part}"
asset="kamaji-${target}.tar.gz"
base="https://github.com/${REPO}/releases/latest/download"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf 'Downloading %s ...\n' "$asset"
dl "${base}/${asset}" "${tmp}/${asset}"
dl "${base}/${asset}.sha256" "${tmp}/${asset}.sha256"

# Verify checksum (sha256sum on Linux, shasum on macOS).
printf 'Verifying checksum ...\n'
expected="$(awk '{print $1}' "${tmp}/${asset}.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${tmp}/${asset}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "${tmp}/${asset}" | awk '{print $1}')"
else
  err "need sha256sum or shasum to verify download"
fi
[ "$expected" = "$actual" ] || err "checksum mismatch (expected $expected, got $actual)"

printf 'Installing to %s ...\n' "$INSTALL_DIR"
tar -xzf "${tmp}/${asset}" -C "$tmp"
mkdir -p "$INSTALL_DIR"
mv "${tmp}/kamaji" "${INSTALL_DIR}/kamaji"
chmod +x "${INSTALL_DIR}/kamaji"

printf 'Installed: '
"${INSTALL_DIR}/kamaji" --version || true

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf '\nNote: %s is not on your PATH. Add it, e.g.:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR" "$INSTALL_DIR" ;;
esac
```

- [ ] **Step 2: Make it executable and lint it**

Run:
```bash
chmod +x install.sh
shellcheck install.sh
```
Expected: no warnings. (If `shellcheck` isn't installed: `sudo apt-get install -y shellcheck` or skip — CI Task 3 enforces it.)

- [ ] **Step 3: Sanity-check the target-detection logic in isolation**

Run:
```bash
sh -c 'os=Linux; arch=x86_64; case "$os" in Linux) o=unknown-linux-musl;; esac; case "$arch" in x86_64) a=x86_64;; esac; echo "kamaji-${a}-${o}.tar.gz"'
```
Expected: `kamaji-x86_64-unknown-linux-musl.tar.gz`.

- [ ] **Step 4: Commit**

```bash
git add install.sh
git commit -m "feat(install): add curl-pipe install script"
```

---

## Task 3: Release workflow + shellcheck CI job

**Files:**
- Create: `.github/workflows/release.yml`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Create the release workflow**

Create `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags:
      - "v*"

permissions:
  contents: write

jobs:
  build:
    name: Build ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
            cross: true
          - target: aarch64-unknown-linux-musl
            os: ubuntu-latest
            cross: true
          - target: x86_64-apple-darwin
            os: macos-latest
            cross: false
          - target: aarch64-apple-darwin
            os: macos-latest
            cross: false
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install cross
        if: matrix.cross
        run: cargo install cross --locked

      - name: Build (cross)
        if: matrix.cross
        run: cross build --release --target ${{ matrix.target }}

      - name: Build (native)
        if: ${{ !matrix.cross }}
        run: cargo build --release --target ${{ matrix.target }}

      - name: Package
        run: |
          bin="target/${{ matrix.target }}/release/kamaji"
          asset="kamaji-${{ matrix.target }}.tar.gz"
          tar -czf "$asset" -C "$(dirname "$bin")" kamaji
          if command -v sha256sum >/dev/null 2>&1; then
            sha256sum "$asset" > "$asset.sha256"
          else
            shasum -a 256 "$asset" > "$asset.sha256"
          fi

      - name: Upload to release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            kamaji-${{ matrix.target }}.tar.gz
            kamaji-${{ matrix.target }}.tar.gz.sha256
```

- [ ] **Step 2: Add a shellcheck job to CI**

In `.github/workflows/ci.yml`, add a second job under `jobs:` (after the existing `test:` job, same indentation level):

```yaml
  shellcheck:
    name: Shellcheck
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run shellcheck
        run: shellcheck install.sh
```

- [ ] **Step 3: Validate YAML syntax**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"
```
Expected: `ok`. (If PyYAML is missing, `pip install pyyaml` or visually inspect indentation.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml .github/workflows/ci.yml
git commit -m "ci: add release workflow and install.sh shellcheck"
```

---

# Phase 2 — In-app updates

## Task 4: Add deps + version parse/compare + target mapping

**Files:**
- Modify: `Cargo.toml`
- Create: `src/update.rs`
- Modify: `src/main.rs` (module declarations near line 1)

- [ ] **Step 1: Add dependencies**

In `Cargo.toml`, under `[dependencies]`, add:

```toml
ureq = "2"
serde_json = "1"
sha2 = "0.10"
```

(`ureq` 2.x uses rustls by default — no extra feature flags needed.)

- [ ] **Step 2: Create the module with failing tests**

Create `src/update.rs`:

```rust
//! Version checking and self-update against GitHub Releases.

/// This binary's version, baked in at compile time.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Parse a `vX.Y.Z` (or `X.Y.Z`) string into a comparable tuple.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().strip_prefix('v').unwrap_or(s.trim());
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// True when `latest` is a strictly newer version than `current`.
/// Unparseable inputs yield `false` (never nag on garbage).
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// The release asset target triple for this build. Linux always maps to the
/// musl asset (that is what the release workflow ships), regardless of the
/// toolchain used to compile this binary.
pub fn current_target() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-musl"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-musl"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_with_and_without_v() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.10.0"), Some((0, 10, 0)));
    }

    #[test]
    fn rejects_malformed_versions() {
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("nope"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn is_newer_compares_correctly() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn current_target_is_one_of_the_known_triples() {
        let known = [
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
        ];
        assert!(known.contains(&current_target()));
    }
}
```

- [ ] **Step 3: Declare the module**

In `src/main.rs`, add `mod update;` to the module list at the top (keep alphabetical: after `mod ui;` / before `mod zellij;` is fine, ordering isn't enforced).

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib update::tests`
Expected: PASS (4 tests). Note: on platforms other than the four supported, `current_target` won't compile — that is intentional; CI and releases only target these four.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/update.rs src/main.rs
git commit -m "feat(update): version parsing, comparison, target mapping"
```

---

## Task 5: GitHub API response parsing + cache

**Files:**
- Modify: `src/update.rs`

- [ ] **Step 1: Write failing tests**

Add these tests inside the existing `mod tests` in `src/update.rs`:

```rust
    #[test]
    fn parses_tag_from_release_json() {
        let json = r#"{"url":"x","tag_name":"v0.3.1","name":"0.3.1","draft":false}"#;
        assert_eq!(parse_latest_tag(json).as_deref(), Some("v0.3.1"));
    }

    #[test]
    fn parse_tag_returns_none_on_bad_json() {
        assert_eq!(parse_latest_tag("not json"), None);
        assert_eq!(parse_latest_tag("{}"), None);
    }

    #[test]
    fn cache_round_trips_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");
        let entry = CacheEntry { checked_at: 1000, latest_version: "0.3.0".into() };
        write_cache(&path, &entry).unwrap();

        let read = read_cache(&path).unwrap();
        assert_eq!(read.latest_version, "0.3.0");
        assert_eq!(read.checked_at, 1000);

        // Fresh within TTL, stale past it.
        assert!(is_fresh(&read, 1000 + 100, 3600));
        assert!(!is_fresh(&read, 1000 + 4000, 3600));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib update::tests`
Expected: FAIL — `cannot find function parse_latest_tag` / `CacheEntry`.

- [ ] **Step 3: Implement parsing + cache**

Add to `src/update.rs` (above the `#[cfg(test)]` block). Add the imports at the top of the file:

```rust
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 24h between network checks.
pub const TTL_SECS: u64 = 24 * 60 * 60;

/// Extract `tag_name` from a GitHub `releases/latest` response body.
pub fn parse_latest_tag(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value.get("tag_name")?.as_str().map(|s| s.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Unix seconds when the check ran.
    pub checked_at: u64,
    /// The latest version string observed (tag, e.g. "v0.3.0").
    pub latest_version: String,
}

pub fn read_cache(path: &Path) -> Option<CacheEntry> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_cache(path: &Path, entry: &CacheEntry) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string(entry).expect("serializing cache entry");
    std::fs::write(path, text)
}

/// True if `entry` was written within `ttl_secs` of `now` (both unix seconds).
pub fn is_fresh(entry: &CacheEntry, now: u64, ttl_secs: u64) -> bool {
    now.saturating_sub(entry.checked_at) < ttl_secs
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib update::tests`
Expected: PASS (7 tests total).

- [ ] **Step 5: Commit**

```bash
git add src/update.rs
git commit -m "feat(update): GitHub API tag parsing + on-disk cache"
```

---

## Task 6: Network fetch + `check()` orchestration

This is thin glue over tested units; the network call itself is verified manually, not unit-tested.

**Files:**
- Modify: `src/update.rs`

- [ ] **Step 1: Implement fetch, cache path, and `check`**

Add to `src/update.rs` (above the `#[cfg(test)]` block):

```rust
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::path::PathBuf;

const RELEASES_API: &str =
    "https://api.github.com/repos/alveflo/kamaji/releases/latest";

/// On-disk cache path: `<cache_dir>/update-check.json`.
pub fn cache_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "kamaji")?;
    Some(dirs.cache_dir().join("update-check.json"))
}

/// GET the latest release tag from the GitHub API. GitHub rejects requests
/// without a User-Agent.
fn fetch_latest_tag() -> Result<String> {
    let body = ureq::get(RELEASES_API)
        .set("User-Agent", concat!("kamaji/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("requesting latest release")?
        .into_string()
        .context("reading release response")?;
    parse_latest_tag(&body).context("no tag_name in release response")
}

/// Return `Some(version)` if a newer release than this build is available.
/// Uses the on-disk cache (TTL `TTL_SECS`); refreshes it on a miss. Any error
/// (network down, no cache dir, rate-limited) yields `None` — the check is
/// best-effort and never surfaces failures.
pub fn check(cache_path: &Path) -> Option<String> {
    let now = now_secs();

    let latest = match read_cache(cache_path) {
        Some(entry) if is_fresh(&entry, now, TTL_SECS) => entry.latest_version,
        _ => {
            let tag = fetch_latest_tag().ok()?;
            let _ = write_cache(
                cache_path,
                &CacheEntry { checked_at: now, latest_version: tag.clone() },
            );
            tag
        }
    };

    if is_newer(&latest, current_version()) {
        Some(latest)
    } else {
        None
    }
}
```

- [ ] **Step 2: Verify it compiles and clippy is clean**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings, builds.

- [ ] **Step 3: Smoke-test the live fetch (optional, needs network + an existing release)**

Until a release exists this returns `None`; just confirm it doesn't panic:
Run: `cargo test --lib update::tests && echo ok`
Expected: PASS + `ok`.

- [ ] **Step 4: Commit**

```bash
git add src/update.rs
git commit -m "feat(update): networked latest-version check with caching"
```

---

## Task 7: App field + background thread wiring

**Files:**
- Modify: `src/app.rs` (the `App` struct near line 161; `App::new` near line 174)
- Modify: `src/main.rs` (`run_tui`, `run`, `run_board`)

- [ ] **Step 1: Add the `update` field with a failing test**

Add to the existing `#[cfg(test)] mod tests` block in `src/app.rs` (it already has a `project()` helper near line 288 returning a `Project`):

```rust
    #[test]
    fn new_app_has_no_update() {
        let app = App::new(project(), vec![]);
        assert!(app.update.is_none());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib app::tests::new_app_has_no_update`
Expected: FAIL — `no field 'update' on type App`.

- [ ] **Step 3: Add the field**

In `src/app.rs`, add to the `App` struct (after `pub theme: Theme,`):

```rust
    /// Newer version available (set by the background update check), shown in
    /// the status bar and triggering self-update on `u`.
    pub update: Option<String>,
```

And in `App::new`, add to the constructed struct (after `theme: Theme::default(),`):

```rust
            update: None,
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib app::tests::new_app_has_no_update`
Expected: PASS.

- [ ] **Step 5: Spawn the background thread in `run_tui`**

In `src/main.rs`, add imports at the top:

```rust
use std::sync::{Arc, Mutex};
```

Replace `run_tui` with:

```rust
fn run_tui() -> Result<()> {
    let config = config::load_or_init()?;
    let db = Db::open(&db_path()?)?;

    // Background, best-effort "newer version available" check. Never blocks the
    // UI; failures are silent. Result lands in this shared slot.
    let update_status: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    if let Some(path) = update::cache_path() {
        let slot = Arc::clone(&update_status);
        std::thread::spawn(move || {
            if let Some(v) = update::check(&path) {
                if let Ok(mut guard) = slot.lock() {
                    *guard = Some(v);
                }
            }
        });
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, db, config, update_status);
    ratatui::restore();
    result
}
```

- [ ] **Step 6: Thread the slot through `run` and into `run_board`**

In `src/main.rs`, change `run`'s signature and pass the slot down:

```rust
fn run(
    terminal: &mut DefaultTerminal,
    mut db: Db,
    mut config: config::Config,
    update_status: Arc<Mutex<Option<String>>>,
) -> Result<()> {
```

Inside `run`'s loop, change the `run_board` call to pass the slot:

```rust
        let switch_project = run_board(terminal, &mut engine, &update_status)?;
```

Change `run_board`'s signature:

```rust
fn run_board(
    terminal: &mut DefaultTerminal,
    engine: &mut Engine,
    update_status: &Arc<Mutex<Option<String>>>,
) -> Result<bool> {
```

At the top of `run_board`'s `loop {`, before `terminal.draw(...)`, copy the shared status into the app each frame (cheap clone; re-applies after project switches):

```rust
        if let Ok(guard) = update_status.lock() {
            engine.app.update = guard.clone();
        }
```

- [ ] **Step 7: Verify build + full test suite**

Run: `cargo build && cargo test`
Expected: builds; all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/app.rs src/main.rs
git commit -m "feat(update): background version check wired into the app"
```

---

## Task 8: Status-bar banner + help hint

**Files:**
- Modify: `src/ui/board.rs` (status line near lines 78–104)
- Modify: `src/ui/modals.rs` (help text in `render_help`)

- [ ] **Step 1: Write a failing render test**

Add to the `#[cfg(test)] mod tests` block in `src/ui/board.rs`. It already has `project()`, `ticket(id, status)`, `render(app, levels, w, h) -> Buffer`, and `buffer_text(buf) -> String` helpers (the sibling `status_bar_lists_the_search_hint` test uses them). Add:

```rust
    #[test]
    fn status_bar_shows_update_banner_when_available() {
        let mut app = App::new(project(), vec![ticket(1, Status::Todo)]);
        app.update = Some("0.9.0".to_string());
        let buf = render(&app, &HashMap::new(), 120, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("0.9.0"), "version present:\n{text}");
        assert!(text.contains("[u]"), "update hint present:\n{text}");
    }
```

(Width 120 keeps the banner — which renders before the long hints string — from being truncated.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib ui::board::tests::status_bar_shows_update_banner_when_available`
Expected: FAIL — banner text absent.

- [ ] **Step 3: Render the banner**

In `src/ui/board.rs`, in `render_board`, just before the `let status_line = Paragraph::new(...)` (around line 95), build an update span:

```rust
    let update_span = match &app.update {
        Some(v) => Span::styled(
            format!(" New version v{v} available — press [u] to update "),
            Style::new().fg(theme.active),
        ),
        None => Span::raw(""),
    };
```

Then insert `update_span` into the `Line::from(vec![...])`, between `search_span` and the `msg` span:

```rust
    let status_line = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::new().fg(theme.accent())),
        search_span,
        update_span,
        Span::styled(msg, Style::new().fg(theme.error)),
        Span::styled(hints, Style::new().fg(theme.muted)),
    ]));
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib ui::board::tests::status_bar_shows_update_banner_when_available`
Expected: PASS.

- [ ] **Step 5: Add the `u` hint to help**

In `src/ui/modals.rs`, in `render_help`, add this line to the `text` block (after the `t` line, before `p`):

```
u         update kamaji (shown when a new version is available)
```

- [ ] **Step 6: Verify build + tests**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/ui/board.rs src/ui/modals.rs
git commit -m "feat(update): status-bar update banner and help hint"
```

---

## Task 9: `u` keypress → self-update

**Files:**
- Modify: `src/engine.rs` (`Effect` enum near line 17; top-level key handler near line 579)
- Modify: `src/update.rs` (add `self_update`)
- Modify: `src/main.rs` (handle `Effect::SelfUpdate` in `run_board`)

- [ ] **Step 1: Add the `Effect::SelfUpdate` variant with a failing test**

Add to the `#[cfg(test)] mod tests` block in `src/engine.rs`. It already has a `key(c)` helper (line ~649) and an `engine_with_project(root)` helper (line ~652). Add:

```rust
    #[test]
    fn u_triggers_self_update_when_update_available() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.app.update = Some("0.9.0".into());
        assert_eq!(
            e.on_key(key('u')).unwrap(),
            Effect::SelfUpdate {
                version: "0.9.0".into()
            }
        );
    }

    #[test]
    fn u_does_nothing_without_an_update() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        assert_eq!(e.on_key(key('u')).unwrap(), Effect::None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib engine::tests::u_triggers_self_update_when_update_available`
Expected: FAIL — `no variant named SelfUpdate`.

- [ ] **Step 3: Add the variant and key handler**

In `src/engine.rs`, add to the `Effect` enum (after `SwitchProject,`):

```rust
    /// Download the latest release and replace the running binary.
    SelfUpdate {
        version: String,
    },
```

In the top-level `match key.code` (near line 579), add an arm (place it near the `'t'`/`'p'` arms):

```rust
            KeyCode::Char('u') => {
                if let Some(version) = self.app.update.clone() {
                    return Ok(Effect::SelfUpdate { version });
                }
            }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib engine::tests::u_`
Expected: both new tests PASS.

- [ ] **Step 5: Implement `self_update` in `src/update.rs`**

Add to `src/update.rs` (above the test module). Add `use sha2::{Digest, Sha256};` and `use std::io::Read;` to the imports:

```rust
/// Download the latest release asset for this platform, verify its checksum,
/// and atomically replace the running executable. The new binary takes effect
/// on the next launch (the caller should ask the user to restart).
pub fn self_update() -> Result<()> {
    let exe = std::env::current_exe().context("locating current executable")?;
    let dir = exe.parent().context("executable has no parent dir")?;

    let asset = format!("kamaji-{}.tar.gz", current_target());
    let base = "https://github.com/alveflo/kamaji/releases/latest/download";

    // Download tarball bytes.
    let tarball = http_get_bytes(&format!("{base}/{asset}"))
        .context("downloading release archive")?;

    // Download + verify checksum.
    let sums = ureq::get(&format!("{base}/{asset}.sha256"))
        .set("User-Agent", concat!("kamaji/", env!("CARGO_PKG_VERSION")))
        .call()
        .context("downloading checksum")?
        .into_string()
        .context("reading checksum")?;
    let expected = sums.split_whitespace().next().context("empty checksum file")?;
    let mut hasher = Sha256::new();
    hasher.update(&tarball);
    let actual = hasher.finalize();
    let actual_hex: String = actual.iter().map(|b| format!("{b:02x}")).collect();
    if !actual_hex.eq_ignore_ascii_case(expected) {
        anyhow::bail!("checksum mismatch (expected {expected}, got {actual_hex})");
    }

    // Extract into a temp dir on the same filesystem as the executable, so the
    // final rename is atomic.
    let tmp = dir.join(".kamaji-update-tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).context("creating temp dir")?;
    let archive = tmp.join(&asset);
    std::fs::write(&archive, &tarball).context("writing archive")?;

    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&tmp)
        .status()
        .context("running tar")?;
    if !status.success() {
        anyhow::bail!("tar extraction failed");
    }

    let new_bin = tmp.join("kamaji");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755))
            .context("setting executable bit")?;
    }

    std::fs::rename(&new_bin, &exe).context("replacing executable")?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .set("User-Agent", concat!("kamaji/", env!("CARGO_PKG_VERSION")))
        .call()?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf)?;
    Ok(buf)
}
```

- [ ] **Step 6: Handle `Effect::SelfUpdate` in `run_board`**

In `src/main.rs`, add an arm to the `match effect` in `run_board` (alongside `Effect::Attach`, `Effect::RunSession`, etc.):

```rust
            Effect::SelfUpdate { version } => {
                ratatui::restore();
                match update::self_update() {
                    Ok(()) => {
                        println!("Updated to v{version} — restart kamaji to use it.");
                        std::process::exit(0);
                    }
                    Err(e) => {
                        eprintln!("update failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
```

- [ ] **Step 7: Verify build, clippy, and full suite**

Run:
```bash
cargo build
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo fmt --all --check
```
Expected: builds clean, no clippy warnings, all tests pass, formatting clean. (If `fmt --check` fails, run `cargo fmt --all` and re-commit.)

- [ ] **Step 8: Commit**

```bash
git add src/engine.rs src/update.rs src/main.rs
git commit -m "feat(update): self-update on 'u' keypress"
```

---

## Task 10: README install docs + final verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add an Install section**

In `README.md`, add near the top (after the project intro, before any "Build from source" content):

```markdown
## Install

```sh
curl -fsSL https://raw.githubusercontent.com/alveflo/kamaji/main/install.sh | sh
```

This downloads a prebuilt binary for your platform (Linux/macOS, x86_64/aarch64)
to `~/.local/bin`. Override the location with `KAMAJI_INSTALL_DIR`:

```sh
curl -fsSL https://raw.githubusercontent.com/alveflo/kamaji/main/install.sh | KAMAJI_INSTALL_DIR=/usr/local/bin sh
```

kamaji checks for new releases on launch. When one is available the status bar
shows `New version vX.Y.Z available — press u to update`; press `u` to download
and replace the binary in place, then restart.
```

- [ ] **Step 2: Final full verification**

Run:
```bash
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test
```
Expected: all pass.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document install one-liner and self-update"
```

---

## Done criteria

- `cargo fmt --all --check`, `cargo clippy … -D warnings`, and `cargo test` all pass.
- `kamaji --version` prints the version.
- `shellcheck install.sh` is clean (and enforced in CI).
- `.github/workflows/release.yml` builds all four targets on a `v*` tag and uploads `kamaji-<target>.tar.gz` + `.sha256`.
- Launching kamaji performs a non-blocking version check; when a newer release exists the banner appears and `u` replaces the binary.

> **Note on end-to-end testing:** the release workflow and self-update download can only be fully exercised once a real `v*` tag is pushed and a Release exists. After merge, cut a test tag (e.g. `v0.1.1`) to validate the pipeline, then verify `install.sh` and the in-app `u` flow against it.
