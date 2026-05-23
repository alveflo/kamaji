use crate::models::Status;
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

/// What a detector believes about an agent session right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalLevel {
    /// Agent is waiting for user input (finished, or needs permission).
    Idle,
    /// Agent is actively working.
    Active,
    /// No information this poll (e.g. screen dump failed). Never moves a ticket.
    Unknown,
}

/// Pure, edge-triggered move decision. Returns the column to move to, or `None`.
///
/// - First observation (`last == None`) only establishes a baseline: no move.
/// - `Active -> Idle` while In Progress  => move to Review.
/// - `Idle -> Active` while in Review AND kamaji auto-moved it => move to In Progress.
/// - `Unknown` current level never moves anything.
pub fn decide(
    last: Option<SignalLevel>,
    current: SignalLevel,
    status: Status,
    was_auto_reviewed: bool,
) -> Option<Status> {
    if current == SignalLevel::Unknown {
        return None;
    }
    let last = last?;
    match (last, current) {
        (SignalLevel::Active, SignalLevel::Idle) if status == Status::InProgress => {
            Some(Status::Review)
        }
        (SignalLevel::Idle, SignalLevel::Active)
            if status == Status::Review && was_auto_reviewed =>
        {
            Some(Status::InProgress)
        }
        _ => None,
    }
}

/// Directory holding per-session idle markers (XDG data dir; temp fallback).
pub fn default_state_dir() -> PathBuf {
    ProjectDirs::from("", "", "kamaji")
        .map(|d| d.data_dir().join("state"))
        .unwrap_or_else(|| std::env::temp_dir().join("kamaji").join("state"))
}

/// Absolute marker path for a session.
pub fn marker_path(state_dir: &Path, session: &str) -> PathBuf {
    state_dir.join(format!("{session}.idle"))
}

/// Claude detector: marker present => Idle, absent => Active. Absence is
/// meaningful (the agent is working), so this never returns Unknown.
pub fn marker_level(path: &Path) -> SignalLevel {
    if path.exists() {
        SignalLevel::Idle
    } else {
        SignalLevel::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_observation_is_baseline_only() {
        assert_eq!(
            decide(None, SignalLevel::Idle, Status::InProgress, false),
            None
        );
    }

    #[test]
    fn finished_in_progress_moves_to_review() {
        assert_eq!(
            decide(Some(SignalLevel::Active), SignalLevel::Idle, Status::InProgress, false),
            Some(Status::Review)
        );
    }

    #[test]
    fn resumed_auto_reviewed_card_moves_back() {
        assert_eq!(
            decide(Some(SignalLevel::Idle), SignalLevel::Active, Status::Review, true),
            Some(Status::InProgress)
        );
    }

    #[test]
    fn never_drags_manually_placed_review_card() {
        assert_eq!(
            decide(Some(SignalLevel::Idle), SignalLevel::Active, Status::Review, false),
            None
        );
    }

    #[test]
    fn no_move_without_a_transition() {
        assert_eq!(
            decide(Some(SignalLevel::Idle), SignalLevel::Idle, Status::InProgress, false),
            None
        );
        assert_eq!(
            decide(Some(SignalLevel::Active), SignalLevel::Active, Status::Review, true),
            None
        );
    }

    #[test]
    fn unknown_never_moves() {
        assert_eq!(
            decide(Some(SignalLevel::Active), SignalLevel::Unknown, Status::InProgress, false),
            None
        );
    }

    #[test]
    fn idle_while_already_in_review_does_not_move() {
        assert_eq!(
            decide(Some(SignalLevel::Active), SignalLevel::Idle, Status::Review, true),
            None
        );
    }

    #[test]
    fn marker_path_is_session_dot_idle() {
        let p = marker_path(std::path::Path::new("/var/state"), "kamaji-1-x");
        assert_eq!(p, std::path::PathBuf::from("/var/state/kamaji-1-x.idle"));
    }

    #[test]
    fn marker_present_is_idle_absent_is_active() {
        let dir = tempfile::tempdir().unwrap();
        let p = marker_path(dir.path(), "s");
        assert_eq!(marker_level(&p), SignalLevel::Active); // absent
        std::fs::write(&p, "").unwrap();
        assert_eq!(marker_level(&p), SignalLevel::Idle); // present
    }
}
