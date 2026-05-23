mod board;
mod modals;

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::Frame;

use crate::app::{App, Modal};

pub(crate) use modals::render_field_modal;

pub fn render(frame: &mut Frame, app: &App) {
    board::render_board(frame, app);
    match &app.modal {
        Modal::None => {}
        Modal::Form(form) => modals::render_form(frame, form),
        Modal::Move { target, .. } => modals::render_move(frame, *target),
        Modal::ConfirmDone { .. } => {
            modals::render_confirm(
                frame,
                "Move to Done",
                "Clean up worktree + session? [y]es / [n]o / Esc",
            );
        }
        Modal::ConfirmDelete { .. } => {
            modals::render_confirm(frame, "Delete ticket", "Delete and clean up? [y]es / Esc");
        }
        Modal::Help => modals::render_help(frame),
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
