mod board;
mod modals;

use std::collections::HashMap;

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::Frame;

use crate::app::{App, Modal};
use kamaji_core::detect::SignalLevel;

pub(crate) use modals::{render_field_modal, themed_block};

pub fn render(frame: &mut Frame, app: &App, levels: &HashMap<i64, SignalLevel>) {
    board::render_board(frame, app, levels);
    match &app.modal {
        Modal::None => {}
        Modal::Form(form) => modals::render_form(frame, &app.theme, form),
        Modal::Move { target, .. } => modals::render_move(frame, &app.theme, *target),
        Modal::ConfirmDone { ticket_ids } => {
            let n = ticket_ids.len();
            let (title, body) = if n > 1 {
                (
                    format!("Close {n} tickets"),
                    format!("Clean up {n} worktrees + sessions? [y]es / [n]o / Esc"),
                )
            } else {
                (
                    "Move to Done".to_string(),
                    "Clean up worktree + session? [y]es / [n]o / Esc".to_string(),
                )
            };
            modals::render_confirm(frame, &app.theme, &title, &body);
        }
        Modal::ConfirmDelete { .. } => {
            modals::render_confirm(
                frame,
                &app.theme,
                "Delete ticket",
                "Delete and clean up? [y]es / Esc",
            );
        }
        Modal::Help => modals::render_help(frame, &app.theme),
        Modal::ThemePicker { selected, .. } => {
            modals::render_theme_picker(frame, &app.theme, *selected)
        }
        Modal::AgentPicker { selected } => {
            modals::render_agent_picker(frame, &app.theme, *selected)
        }
        Modal::WorktreeLocation(form) => modals::render_worktree_location(frame, &app.theme, form),
    }
}

/// A centered rect `pct_x` x `pct_y` percent of the frame.
pub fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let [area] = Layout::vertical([Constraint::Percentage(pct_y)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Percentage(pct_x)])
        .flex(Flex::Center)
        .areas(area);
    area
}

/// A centered rect of fixed `width` x `height`, clamped to fit `area`.
pub fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let [area] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(w)])
        .flex(Flex::Center)
        .areas(area);
    area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_centers_requested_percentage() {
        let area = Rect::new(0, 0, 100, 40);

        let rect = centered_rect(60, 50, area);

        assert_eq!(rect, Rect::new(20, 10, 60, 20));
    }

    #[test]
    fn centered_rect_respects_area_origin() {
        let area = Rect::new(10, 5, 80, 20);

        let rect = centered_rect(50, 50, area);

        assert_eq!(rect, Rect::new(30, 10, 40, 10));
    }

    #[test]
    fn centered_fixed_centers_and_clamps() {
        let area = Rect::new(0, 0, 100, 40);
        // 52x12 centered in 100x40 -> x=(100-52)/2=24, y=(40-12)/2=14.
        assert_eq!(centered_fixed(52, 12, area), Rect::new(24, 14, 52, 12));
        // Requested size larger than the area is clamped to the area.
        assert_eq!(centered_fixed(200, 80, area), Rect::new(0, 0, 100, 40));
    }
}
