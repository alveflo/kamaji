mod board;
mod modals;

use std::collections::HashMap;

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::Frame;

use crate::app::{App, Modal};
use crate::detect::SignalLevel;

pub(crate) use modals::render_field_modal;

pub fn render(frame: &mut Frame, app: &App, levels: &HashMap<i64, SignalLevel>) {
    board::render_board(frame, app, levels);
    match &app.modal {
        Modal::None => {}
        Modal::Form(form) => modals::render_form(frame, &app.theme, form),
        Modal::Move { target, .. } => modals::render_move(frame, &app.theme, *target),
        Modal::ConfirmDone { .. } => {
            modals::render_confirm(
                frame,
                &app.theme,
                "Move to Done",
                "Clean up worktree + session? [y]es / [n]o / Esc",
            );
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
}
