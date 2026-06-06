# Contributing to kamaji

Thanks for hacking on kamaji! This is a Rust workspace: `kamaji` (the TUI),
`kamajid` (the daemon serving the browser board + zellij terminal proxy), and
`kamaji-core` (shared library). This file covers the human basics — building,
testing, running, and the PR flow.

> **AI coding agents:** see [AGENTS.md](AGENTS.md) for the agent working
> agreement (worktrees, issue→task flow, subagent-driven plan execution).
> `CLAUDE.md` is a symlink to it.

## Prerequisites

- A recent stable Rust toolchain (`rustup update stable`). The CI lint/format
  checks run on stable, so match it locally to avoid surprises.
- [zellij](https://zellij.dev) on your `PATH` for the agent-session features.

## Build

```sh
cargo build              # whole workspace
cargo build -p kamajid   # just the daemon
```

## Test, format, lint

Run these before opening a PR — CI runs format and clippy on every PR:

```sh
cargo test               # whole workspace
cargo fmt --all          # format (CI checks with --check)
cargo clippy --all-targets --all-features
```

## Run the daemon

The `Makefile` wraps the daemon lifecycle. The browser board lives at
<http://127.0.0.1:8755> and the zellij terminal proxy at `:8756`.

```sh
make start      # build, then start kamajid in the background
make restart    # rebuild + relaunch (what you usually want after pulling)
make status     # is the daemon responding?
make logs       # follow the daemon log
make stop       # stop it
make help       # list all targets
```

To run the TUI directly:

```sh
cargo run -p kamaji
```

## Browser smoke tests

End-to-end browser checks live in `crates/kamajid/smoke/` (Playwright). See that
directory's `README.md` for how to run them.

## Pull request flow

1. Branch off `main` (a git worktree keeps parallel work isolated — see
   [AGENTS.md](AGENTS.md)).
2. Make your change; keep modules small and single-purpose.
3. `cargo fmt --all`, `cargo clippy --all-targets`, and `cargo test` clean.
4. Open a PR against `main` and squash-merge once checks are green:

   ```sh
   gh pr create --fill --base main
   gh pr merge --squash --auto --delete-branch
   ```

Never commit secrets, and never force-push shared branches.
