mod app;
mod cli;
mod client;
mod daemon;
mod dir_select;
mod engine;
mod picker;
mod sse;
#[cfg(test)]
mod test_support;
mod theme;
mod ui;
mod update;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sse::SseMsg;

use app::App;
use engine::{Effect, Engine};
use kamaji_core::db::Db;
use kamaji_core::{config, detect, models, paths, zellij};

fn db_path() -> Result<PathBuf> {
    Ok(paths::data_dir()
        .context("cannot determine data dir")?
        .join("kamaji.db"))
}

fn main() -> Result<()> {
    // Clear any binary set aside by a prior Windows self-update (no-op on Unix).
    update::cleanup_stale_update();

    match cli::parse(std::env::args().skip(1))? {
        cli::Command::Tui(opts) => run_tui(opts),
        cli::Command::Help => {
            print!("{}", cli::usage());
            Ok(())
        }
        cli::Command::Version => {
            println!("kamaji {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        cli::Command::CreateTicket(args) => {
            let config = config::load_or_init()?;
            let db = Db::open(&db_path()?)?;
            let cwd = std::env::current_dir().context("determining current directory")?;
            let state_dir = detect::default_state_dir();
            let outcome = cli::run_create_ticket(&db, &config, &args, &cwd, &state_dir)?;
            println!("{}", outcome.message);
            if let Some(spec) = outcome.launch {
                match zellij::create_session_background(&spec.name, &spec.layout_path, &spec.cwd) {
                    Ok(()) => println!("Started '{}' in the background", spec.name),
                    Err(e) => {
                        eprintln!("could not start session: {e}");
                        // Tear down the half-started session so the card is left
                        // clean (no session, back in Todo) and recoverable.
                        zellij::terminate_session(&spec.name);
                        let _ = std::fs::remove_file(detect::marker_path(&state_dir, &spec.name));
                        db.clear_ticket_session(spec.ticket_id)?;
                        db.set_ticket_status(spec.ticket_id, models::Status::Todo)?;
                        std::process::exit(1);
                    }
                }
            } else if outcome.background_failed {
                if let Some(warning) = outcome.warning {
                    eprintln!("{warning}");
                }
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

fn run_tui(opts: cli::DaemonOpts) -> Result<()> {
    let config = config::load_or_init()?;

    // Ensure a healthy kamajid is up (reuse an existing one or spawn one
    // detached) and connect to it. `--daemon <addr>` forces a fixed address;
    // `--no-spawn` refuses to spawn one.
    let client = daemon::ensure_daemon(&config, opts.forced_addr.as_deref(), !opts.no_spawn)
        .map_err(|e| anyhow::anyhow!("could not start kamaji: {e}"))?;

    // SSE listener thread (2a: events are drained-and-discarded; applied in 2b).
    let (sse_tx, sse_rx) = std::sync::mpsc::channel::<sse::SseMsg>();
    let _sse_handle = sse::spawn(client.base().to_string(), sse_tx);

    // Background, best-effort "newer version available" check. Never blocks the
    // UI; failures are silent. Result lands in this shared slot.
    let update_status: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    if let Some(path) = update::cache_path() {
        let slot = Arc::clone(&update_status);
        std::thread::spawn(move || {
            if let Some(v) = update::check(&path) {
                if let Ok(mut guard) = slot.lock() {
                    *guard = Some(v);
                }
            }
        });
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, client, config, update_status, sse_rx);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut DefaultTerminal,
    mut client: client::DaemonClient,
    config: config::Config,
    update_status: Arc<Mutex<Option<String>>>,
    sse_rx: Receiver<SseMsg>,
) -> Result<()> {
    // Theme/agent come from the daemon's loaded config so the TUI reflects the
    // daemon's state. Fall back to the locally-loaded config if the daemon
    // can't be reached for it.
    let mut config = client
        .get_config()
        .map_err(picker::client_err)
        .unwrap_or(config);
    loop {
        let theme = crate::theme::Theme::by_name(&config.theme);
        let Some(project) = picker::run(terminal, &client, theme)? else {
            return Ok(());
        };

        let tickets = client
            .list_tickets(project.id)
            .map_err(picker::client_err)?;
        let mut app = App::new(project, tickets);
        app.theme = theme;
        let mut engine = Engine::new(client, config, app);

        let switch_project = run_board(terminal, &mut engine, &update_status, &sse_rx)?;

        // Reclaim client + config for the next project (or to drop on quit).
        client = engine.client;
        config = engine.config;

        if !switch_project {
            return Ok(());
        }
    }
}

/// Run the board event loop for one project. Returns `true` if the user asked to
/// switch projects (return to the picker), `false` to quit the app.
fn run_board(
    terminal: &mut DefaultTerminal,
    engine: &mut Engine,
    update_status: &Arc<Mutex<Option<String>>>,
    sse_rx: &Receiver<SseMsg>,
) -> Result<bool> {
    loop {
        // Drain SSE messages and apply them to the board. On (re)connect the
        // whole list is re-fetched so we never miss deltas dropped while
        // disconnected; a lost stream just shows an info toast.
        while let Ok(msg) = sse_rx.try_recv() {
            match msg {
                SseMsg::Connected => {
                    let _ = engine.refresh_from_client();
                }
                SseMsg::Disconnected => engine.app.set_info("daemon stream lost — reconnecting…"),
                SseMsg::Event(ev) => engine.apply_sse_event(*ev),
            }
        }
        if let Ok(guard) = update_status.lock() {
            engine.app.update = guard.clone();
        }
        // Auto-review detection now runs in the daemon; its moves arrive via SSE.
        // The "working" bullet that read these levels is a follow-up, so pass an
        // empty map (the `ui/` signature is unchanged).
        terminal.draw(|frame| ui::render(frame, &engine.app, &std::collections::HashMap::new()))?;

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
            Effect::Attach { name } => {
                run_zellij(terminal, engine, |_| zellij::attach_session(&name))?;
            }
            Effect::SelfUpdate { version } => {
                ratatui::restore();
                match update::self_update() {
                    Ok(()) => {
                        println!("Updated to v{version} — restart kamaji to use it.");
                        std::process::exit(0);
                    }
                    Err(e) => {
                        eprintln!("update failed: {e}");
                        std::process::exit(1);
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
/// re-initialize ratatui and refresh the board from the daemon.
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
        engine.app.set_error(format!("zellij error: {e}"));
    }
    // The daemon reconciles session state; re-fetch the board after attach.
    engine.refresh_from_client()?;
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
