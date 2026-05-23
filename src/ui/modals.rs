use ratatui::Frame;
use crate::app::TicketForm;
use crate::models::Status;

pub fn render_form(_f: &mut Frame, _form: &TicketForm) {}
pub fn render_move(_f: &mut Frame, _target: Status) {}
pub fn render_confirm(_f: &mut Frame, _title: &str, _body: &str) {}
pub fn render_help(_f: &mut Frame) {}
