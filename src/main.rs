mod agent;
mod app;
mod config;
mod db;
mod engine;
mod git;
mod layout;
mod models;
mod picker;
mod slug;
mod ui;
mod zellij;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use std::path::PathBuf;
use std::time::Duration;

use app::App;
use db::Db;
use engine::{Effect, Engine};

fn db_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "kamaji").context("cannot determine data dir")?;
    Ok(dirs.data_dir().join("kamaji.db"))
}

fn main() -> Result<()> {
    let config = config::load_or_init()?;
    let db = Db::open(&db_path()?)?;

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, db, config);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, db: Db, config: config::Config) -> Result<()> {
    let Some(project) = picker::run(terminal, &db)? else {
        return Ok(());
    };

    let tickets = db.list_tickets(project.id)?;
    let app = App::new(project, tickets);
    let mut engine = Engine::new(db, config, app);

    loop {
        terminal.draw(|frame| ui::render(frame, &engine.app))?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        let effect = engine.on_key(key)?;
        match effect {
            Effect::None => {}
            Effect::RunSession { name, layout_path } => {
                run_zellij(terminal, &mut engine, |_| {
                    zellij::create_session(&name, &layout_path)
                })?;
            }
            Effect::Attach { name } => {
                run_zellij(terminal, &mut engine, |_| zellij::attach_session(&name))?;
            }
        }

        if engine.app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Release the terminal, run a zellij command (inherits the real TTY), then
/// re-initialize ratatui and reload tickets.
fn run_zellij<F>(terminal: &mut DefaultTerminal, engine: &mut Engine, f: F) -> Result<()>
where
    F: FnOnce(()) -> Result<std::process::ExitStatus>,
{
    ratatui::restore();
    let outcome = f(());
    *terminal = ratatui::init();
    if let Err(e) = outcome {
        engine.app.status_message = Some(format!("zellij error: {e}"));
    }
    engine.reload()?;
    Ok(())
}
