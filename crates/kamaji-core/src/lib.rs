//! Pure domain logic for kamaji: SQLite-backed Kanban model, git worktree
//! orchestration, zellij CLI integration, and agent command templates. No UI,
//! no transport. Both the existing TUI binary and the upcoming `kamajid`
//! daemon depend on this crate.
//!
//! Extracted from the binary crate in Phase 0;
//! see `docs/superpowers/plans/2026-05-27-phase-0-extract-kamaji-core.md`.

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
