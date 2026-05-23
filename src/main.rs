mod agent;
mod app;
mod config;
mod db;
mod detect;
mod engine;
mod git;
mod layout;
mod models;
mod picker;
mod slug;
mod ui;
mod zellij;
mod zellij_config;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

fn run(terminal: &mut DefaultTerminal, mut db: Db, mut config: config::Config) -> Result<()> {
    loop {
        let Some(project) = picker::run(terminal, &db)? else {
            return Ok(());
        };

        let tickets = db.list_tickets(project.id)?;
        let app = App::new(project, tickets);
        let mut engine = Engine::new(db, config, app);
        // Drop any recorded sessions that no longer exist in zellij.
        engine.reconcile()?;

        let switch_project = run_board(terminal, &mut engine)?;

        // Reclaim db + config for the next project (or to drop on quit).
        db = engine.db;
        config = engine.config;

        if !switch_project {
            return Ok(());
        }
    }
}

/// Run the board event loop for one project. Returns `true` if the user asked to
/// switch projects (return to the picker), `false` to quit the app.
fn run_board(terminal: &mut DefaultTerminal, engine: &mut Engine) -> Result<bool> {
    let mut last_tick = Instant::now();
    loop {
        if engine.config.auto_review.enabled && last_tick.elapsed() >= engine.config.poll_interval()
        {
            engine.detect_tick()?;
            last_tick = Instant::now();
        }
        terminal.draw(|frame| ui::render(frame, &engine.app, &engine.last_level))?;

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
            Effect::SwitchProject => return Ok(true),
            Effect::RunSession { name, layout_path } => {
                run_zellij(terminal, engine, |_| {
                    zellij::create_session(&name, &layout_path)
                })?;
            }
            Effect::Attach { name } => {
                run_zellij(terminal, engine, |_| zellij::attach_session(&name))?;
            }
            Effect::RunSessionBackground {
                name,
                layout_path,
                cwd,
            } => {
                match zellij::create_session_background(&name, &layout_path, &cwd) {
                    Ok(()) => {
                        engine.app.status_message =
                            Some(format!("Started '{name}' in the background"));
                    }
                    Err(e) => {
                        engine.app.status_message = Some(format!("background session failed: {e}"));
                        // Tear down any half-created session first: the first
                        // command may have made the (agent-less) session before
                        // the second failed, and reconcile only clears sessions
                        // that are ABSENT from `list-sessions`. Killing it makes
                        // reconcile drop the columns. Then the card has no
                        // session (status stays In Progress; recoverable via
                        // Enter, which starts a fresh session).
                        zellij::terminate_session(&name);
                        engine.reconcile()?;
                    }
                }
            }
        }

        if engine.app.should_quit {
            return Ok(false);
        }
    }
}

/// Release the terminal, run a zellij command (inherits the real TTY), then
/// re-initialize ratatui and reconcile session state.
fn run_zellij<F>(terminal: &mut DefaultTerminal, engine: &mut Engine, f: F) -> Result<()>
where
    F: FnOnce(()) -> Result<std::process::ExitStatus>,
{
    ratatui::restore();
    let outcome = f(());
    if outcome.as_ref().is_ok_and(|status| status.success()) {
        clear_zellij_detach_banner();
    }
    *terminal = ratatui::init();
    if let Err(e) = outcome {
        engine.app.status_message = Some(format!("zellij error: {e}"));
    }
    // reconcile() reloads tickets and drops any sessions that vanished.
    engine.reconcile()?;
    Ok(())
}

fn clear_zellij_detach_banner() {
    let mut stdout = io::stdout();
    // Zellij prints "Bye from Zellij!" after detach. Erase that normal-screen
    // line before Kamaji re-enters its alternate-screen TUI, otherwise every
    // attach/detach leaves a stale line visible after Kamaji exits.
    let _ = stdout.write_all(b"\r\x1b[1A\x1b[2K");
    let _ = stdout.flush();
}
