//! Pure domain logic for kamaji: SQLite-backed Kanban model, git worktree
//! orchestration, zellij CLI integration, and agent command templates. No UI,
//! no transport. Both the existing TUI binary and the upcoming `kamajid`
//! daemon depend on this crate.
//!
//! Modules are added incrementally as Phase 0 extracts them from the binary;
//! see `docs/superpowers/plans/2026-05-27-phase-0-extract-kamaji-core.md`.

pub mod git;
pub mod layout;
pub mod models;
pub mod slug;
pub mod zellij;
