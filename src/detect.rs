use crate::models::Status;

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
}
