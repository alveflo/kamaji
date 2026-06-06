//! Pure `maud` view partials shared by the HTML page routes and the
//! `/ui/events` fragment serializer. Each is `(&data) -> maud::Markup`.

pub mod board;
pub mod card;
pub mod confirm;
pub mod modal;
pub mod page;
pub mod project_form;
pub mod sessions;
pub mod sidebar;
pub mod terminal;
