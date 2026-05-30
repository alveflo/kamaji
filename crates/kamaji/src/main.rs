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
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use sse::SseMsg;

/// Owns the live SSE listener thread and the channel the UI loop drains. Kept in
/// one place so a reconnect can atomically swap in a fresh thread + channel
/// (dropping the old receiver ends the old thread on its next send).
struct Sse {
    rx: Receiver<SseMsg>,
    _handle: JoinHandle<()>,
}

impl Sse {
    fn spawn(base: &str) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<SseMsg>();
        let handle = sse::spawn(base.to_string(), tx);
        Sse {
            rx,
            _handle: handle,
        }
    }
}

use app::App;
use engine::{Effect, Engine};
use kamaji_core::{config, zellij};

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
            // Ensure a healthy kamajid is up (reuse or spawn detached); the
            // daemon owns ticket creation and the background session start.
            let client = daemon::ensure_daemon(&config, None, true)
                .map_err(|e| anyhow::anyhow!("could not start kamaji: {e}"))?;
            let cwd = std::env::current_dir().context("determining current directory")?;
            let outcome = cli::run_create_ticket(&client, &config, &args, &cwd)?;
            println!("{}", outcome.message);
            if outcome.background_failed {
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

    // SSE listener thread. Held in a swappable holder so a reconnect can drop
    // the dead stream and re-spawn it against the new daemon (Task 2c-3).
    let sse = Sse::spawn(client.base());

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
    let result = run(&mut terminal, client, config, opts, update_status, sse);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut DefaultTerminal,
    mut client: client::DaemonClient,
    config: config::Config,
    opts: cli::DaemonOpts,
    update_status: Arc<Mutex<Option<String>>>,
    mut sse: Sse,
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

        let switch_project = run_board(terminal, &mut engine, &opts, &update_status, &mut sse)?;

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
    opts: &cli::DaemonOpts,
    update_status: &Arc<Mutex<Option<String>>>,
    sse: &mut Sse,
) -> Result<bool> {
    // Count consecutive SSE `Disconnected` reports. The listener retries on its
    // own, so a single blip is just an info toast; only a run of them (the
    // daemon is actually gone, not a momentary stream reset) escalates to a
    // full re-probe/respawn.
    let mut disconnects: u32 = 0;
    loop {
        // Drain SSE messages and apply them to the board. On (re)connect the
        // whole list is re-fetched so we never miss deltas dropped while
        // disconnected; a lost stream just shows an info toast.
        while let Ok(msg) = sse.rx.try_recv() {
            match msg {
                SseMsg::Connected => {
                    disconnects = 0;
                    let _ = engine.refresh_from_client();
                }
                SseMsg::Disconnected => {
                    disconnects = disconnects.saturating_add(1);
                    engine.app.set_info("daemon stream lost — reconnecting…");
                    // The listener retries a stream blip on its own; escalate to
                    // a full re-probe/respawn only after a run of failures (the
                    // daemon is actually gone, not a momentary stream reset).
                    if disconnects >= 3 {
                        engine.flag_connection_lost();
                    }
                }
                SseMsg::Event(ev) => engine.apply_sse_event(*ev),
            }
        }
        if let Ok(guard) = update_status.lock() {
            engine.app.update = guard.clone();
        }

        // Reconnect trigger: a command (or a board refresh) saw `Unreachable`,
        // or the SSE stream reported repeated disconnects (flagged above). Either
        // way the daemon looks gone — re-probe/respawn it once (bounded) and
        // rebuild the stream. The flag is read-and-cleared so each loss is
        // handled once; on success the disconnect counter resets.
        if engine.take_connection_lost() && try_reconnect(engine, opts, sse) {
            disconnects = 0;
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

/// Re-probe / respawn the daemon after a connection loss and rebuild the SSE
/// stream against it. Bounded: `daemon::ensure_daemon` itself does only a couple
/// of health-waited attempts, so this never blocks the UI loop indefinitely.
///
/// On success the new `DaemonClient` replaces `engine.client`, the SSE listener
/// is re-spawned against the new base (the old thread ends when its receiver is
/// dropped), the board is refreshed, and `true` is returned. On give-up the
/// sticky "unreachable — reconnecting…" toast is left in place, the stale board
/// stays usable, and `false` is returned so the loop remains responsive and will
/// retry on the next trigger.
fn try_reconnect(engine: &mut Engine, opts: &cli::DaemonOpts, sse: &mut Sse) -> bool {
    engine.app.set_info("daemon unreachable — reconnecting…");
    // Clone the config so we can re-probe while mutating `engine.client`; this
    // path is rare (only on an actual daemon loss).
    let config = engine.config.clone();
    match daemon::ensure_daemon(&config, opts.forced_addr.as_deref(), !opts.no_spawn) {
        Ok(new_client) => {
            // Drop the dead stream and start a fresh one against the new daemon.
            *sse = Sse::spawn(new_client.base());
            engine.client = new_client;
            // Pull the current board from the reconnected daemon. If this still
            // fails it re-flags connection loss, so the loop will retry.
            let _ = engine.refresh_from_client();
            if !engine.take_connection_lost() {
                engine.app.set_info("daemon reconnected");
                return true;
            }
            false
        }
        Err(_) => false,
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
