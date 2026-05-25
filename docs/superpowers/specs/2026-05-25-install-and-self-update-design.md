# Install script + in-app self-update — design

**Date:** 2026-05-25
**Status:** Approved (brainstorming)

## Problem

kamaji can only be installed today by building from source with `cargo`. Users
without a Rust toolchain have no way in. We want a one-line install:

```sh
curl -fsSL https://raw.githubusercontent.com/alveflo/kamaji/main/install.sh | sh
```

We also want kamaji to notice when a newer version exists and let the user
upgrade from inside the TUI with a single keypress — surfaced as
`New version vX.Y.Z available — press u to update`.

## Overview

Three cohesive pieces, built in two phases:

- **Phase 1 — Distribution:** a GitHub Actions release workflow that produces
  prebuilt binaries, plus a POSIX `install.sh` that downloads and installs them.
- **Phase 2 — In-app updates:** a background version check on launch and a
  one-keypress self-update.

The **asset naming convention `kamaji-<target>.tar.gz`** is the shared contract
between the release workflow, `install.sh`, and the in-app updater. All three
must agree on it.

Supported targets (all Unix — consistent with the existing zellij dependency,
no Windows):

| OS / arch         | Rust target triple             |
|-------------------|--------------------------------|
| Linux x86_64      | `x86_64-unknown-linux-musl`    |
| Linux aarch64     | `aarch64-unknown-linux-musl`   |
| macOS x86_64      | `x86_64-apple-darwin`          |
| macOS aarch64     | `aarch64-apple-darwin`         |

Linux targets are static musl builds so the shipped binary has no system
OpenSSL dependency — this pairs with rustls for TLS (see §3).

## 1. Release pipeline (`.github/workflows/release.yml`)

- **Trigger:** push of a tag matching `v*` (e.g. `v0.2.0`).
- **Build matrix:** the four targets above. Linux targets cross-compile via
  `cross`; macOS targets build natively on `macos-latest`.
- For each target:
  - Build `--release`.
  - Package the `kamaji` binary as `kamaji-<target>.tar.gz`.
  - Generate `kamaji-<target>.tar.gz.sha256`.
  - Upload both to the GitHub Release for the tag.
- A separate **`shellcheck` lint job** for `install.sh` is added to the existing
  CI (`.github/workflows/ci.yml`) so the script can't rot.

Releasing is a manual `git tag vX.Y.Z && git push --tags` after bumping the
version in `Cargo.toml`. The tag drives the workflow; the workflow creates/fills
the Release.

## 2. `install.sh` (repo root)

Pure POSIX `sh` (it is piped into `sh`, so no bash-isms).

Steps:

1. Detect OS via `uname -s` (`Linux` → linux, `Darwin` → darwin) and arch via
   `uname -m` (`x86_64`/`amd64` → x86_64, `aarch64`/`arm64` → aarch64). Map to
   one of the four target triples. Error clearly and exit non-zero on anything
   unsupported.
2. Download
   `https://github.com/alveflo/kamaji/releases/latest/download/kamaji-<target>.tar.gz`.
   The `latest/download` redirect resolves to the newest release asset, so **no
   GitHub API call and no `jq` are required**. Use `curl -fsSL` (fall back to
   `wget` if `curl` is absent).
3. Download the matching `.sha256` and verify (`sha256sum` or `shasum -a 256`);
   abort on mismatch.
4. Extract the tarball to a temp dir and move the `kamaji` binary into the
   install dir, `chmod +x`.
5. **Install dir:** `$KAMAJI_INSTALL_DIR` if set, else `$HOME/.local/bin`.
   Create it if missing. If the install dir is not on `$PATH`, print a hint
   telling the user to add it.
6. Print the installed version (`kamaji --version`) on success.

`README.md` gains an **Install** section documenting the one-liner and the
`KAMAJI_INSTALL_DIR` override.

> Note: `kamaji --version` does not exist yet. Phase 1 adds a `--version` /
> `-V` flag to `cli::parse` that prints `kamaji <CARGO_PKG_VERSION>`, both so the
> install script can confirm success and because the updater logic needs the
> current version anyway (§3).

## 3. In-app version check (`src/update.rs`, new module)

A new self-contained module. Responsibilities, each independently testable:

- **`current_version() -> &'static str`** — `env!("CARGO_PKG_VERSION")`.
- **Version parse/compare** — parse `vX.Y.Z` (tolerating a leading `v`) into
  `(u64, u64, u64)` and compare. `is_newer(latest, current) -> bool`. No extra
  semver dependency.
- **Target mapping** — `current_target() -> &'static str` returning this
  build's target triple, used to build the asset URL for self-update. Derived
  from `cfg!(target_os)` / `cfg!(target_arch)`.
- **API response parse** — `parse_latest_tag(json: &str) -> Option<String>`
  pulls `tag_name` out of the GitHub `releases/latest` response. Separated from
  the network call so it is unit-testable against a sample JSON fixture.
- **Network fetch** — `fetch_latest_tag() -> Result<String>` does the actual
  `ureq` GET of
  `https://api.github.com/repos/alveflo/kamaji/releases/latest` (with a
  `User-Agent` header, as GitHub requires) and feeds the body to
  `parse_latest_tag`.
- **Cache** — read/write `update-check.json` in the cache dir
  (`ProjectDirs::from("", "", "kamaji").cache_dir()`), shape
  `{ "checked_at": <unix_secs>, "latest_version": "X.Y.Z" }`. A `check()` entry
  point returns the cached value when younger than the 24h TTL, otherwise
  fetches, rewrites the cache, and returns the fresh value. Cache read/write and
  TTL expiry are unit-tested with a `tempfile` dir.

HTTP uses **`ureq` with the `rustls` TLS backend** (sync, fits the non-async
codebase; rustls keeps the static musl build clean).

### Wiring into the app

- On launch (in `run_tui`, before/around the picker loop), spawn a background
  `std::thread` that runs `update::check()` and sends the result over an
  `mpsc::Sender<Option<String>>` (the `Some` payload is the newer version
  string; `None`/dropped channel means "no update / check failed" and is
  silent).
- `App` gains a field `update: Option<String>` (the available newer version).
- The board event loop (`run_board`) already wakes every 200ms; each iteration
  it does a `rx.try_recv()` and, on a `Some(version)`, sets
  `engine.app.update = Some(version)`. This never blocks.

## 4. Self-update UX

- **Display:** when `App.update` is `Some(v)`, the status bar / footer renders
  `New version v{v} available — press u to update`. (`u` is currently unbound;
  bound keys are `q / Esc p ? t h l j k c e m d Enter` and `/`.)
- **Trigger:** pressing `u` on the board (top-level key handler in
  `Engine::on_key`, only when `app.update.is_some()`) returns a new
  `Effect::SelfUpdate`.
- **Perform:** `main.rs` handles `Effect::SelfUpdate` by:
  1. `ratatui::restore()` to give back the terminal.
  2. Download `kamaji-<current_target()>.tar.gz` from the latest release,
     verify its `.sha256`, extract the binary to a **temp file in the same
     directory as `std::env::current_exe()`** (same filesystem, so `rename` is
     atomic), `chmod +x`.
  3. `std::fs::rename` the temp file over the running executable. On Linux/macOS
     renaming over a running binary is valid — the running process keeps its
     open inode; the next launch picks up the new file.
  4. Print `Updated to vX.Y.Z — restart kamaji` and exit (do **not**
     auto-relaunch).
- On any failure, restore the terminal, print the error, and exit non-zero (the
  old binary is untouched because the rename never happened).

## Dependencies added

- `ureq` with rustls TLS — runtime HTTP for the version check and self-update
  download.

## Testing strategy

Rust unit tests (in `src/update.rs`):

- version parse + `is_newer` (equal, older, newer, leading `v`, malformed).
- `parse_latest_tag` against a sample GitHub `releases/latest` JSON fixture.
- cache write→read round-trip and TTL expiry, using a `tempfile` cache dir.
- `current_target` returns one of the four known triples.

The network fetch (`fetch_latest_tag`) and the self-replace file dance are thin
glue over tested units and are validated manually (and implicitly by a real
release), not mocked.

`install.sh` is covered by `shellcheck` in CI.

## Out of scope

- Windows support.
- Package-manager distribution (Homebrew tap, AUR, etc.).
- Background auto-download or auto-apply of updates — the user always presses
  `u`.
- Rollback / multi-version management.

## Open risks

- `cross` aarch64-musl builds can be finicky; if the workflow fails for that
  target, fall back to building it with the `rust-cross`/`messense` musl
  toolchain action. The other three targets are low-risk.
- GitHub API rate limiting for unauthenticated version checks (60 req/hr/IP) is
  a non-issue given the 24h cache.
