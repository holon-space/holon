use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;

use anyhow::Context;
use holon_app::session::first_then;
use holon_app::stop_signal::StopSignals;
use holon_app::vault_lock::SessionVault;
use holon_frontend::FrontendSession;
use holon_frontend::ReactiveViewModel;
use holon_frontend::cli;
use holon_frontend::reactive::ReactiveEngine;
use holon_tui::app_main::AppMain;
use holon_tui::app_main::NO_FOCUS;
use holon_tui::app_main::TuiState;
use holon_tui::di::TuiModule;
use holon_tui::stderr_to_log::StderrToLog;
use r3bl_tui::CommonResult;
use r3bl_tui::InputEvent;
use r3bl_tui::Key;
use r3bl_tui::KeyPress;
use r3bl_tui::KeyState;
use r3bl_tui::OutputDevice;
use r3bl_tui::RawMode;
use r3bl_tui::TerminalWindow;
use r3bl_tui::height;
use r3bl_tui::lock_output_device_as_mut;
use r3bl_tui::log::try_initialize_logging_global;
use r3bl_tui::width;

#[tokio::main]
async fn main() -> CommonResult<()> {
    // This process is a user's installed app, so it may hold the user's
    // credentials. Nothing below reaches the login keychain without it.
    holon_secrets::grant_login_keychain();

    // Disable r3bl logging to prevent breaking TUI display
    try_initialize_logging_global(tracing_core::LevelFilter::OFF).ok(); // ALLOW(ok): best-effort logging init

    let log = tui_log_path().map_err(|e| miette::miette!("{e:#}"))?;
    // TUI defaults to file logging (stderr/stdout would corrupt the terminal).
    // Override with HOLON_LOG env var if set.
    let _log_guard = if std::env::var("HOLON_LOG").is_ok() {
        holon_frontend::logging::init()
    } else {
        holon_frontend::logging::init_from(&format!("file://{}", log.display()))
    };

    let stderr = StderrToLog::redirect(&log)
        .map_err(|e| miette::miette!("pointing stderr at {}: {e}", log.display()))?;
    let ran = run().await;
    let restored = stderr
        .restore()
        .map_err(|e| anyhow::anyhow!("pointing stderr back at the terminal: {e}"));
    let result = first_then(ran, restored).map_err(|e| miette::miette!("{e:#}"));
    // Once the terminal is gone, the log is the only place an error reaches.
    if let Err(e) = &result {
        tracing::error!("holon-tui exits with an error: {e}");
    }
    result
}

async fn run() -> anyhow::Result<()> {
    let widgets = holon_tui::render_supported_widgets();
    let (holon_config, session_config, config_dir, locked) = cli::build_session(widgets)?;
    let vault = SessionVault::acquire(holon_config.vault.root.as_deref())?;

    // A stop during boot waits for the boot to finish and then takes the one
    // shutdown path, so the vault is left as a clean quit leaves it.
    let stop = StopSignals::install()?;

    let module = TuiModule {
        holon_config,
        session_config,
        config_dir,
        locked_keys: locked,
        vault: Mutex::new(Some(vault)),
    };
    run_session(module, stop).await
}

/// Boot the session, draw it until a quit, then shut it down.
async fn run_session(module: TuiModule, mut stop: StopSignals) -> anyhow::Result<()> {
    tracing::info!("Starting TUI frontend...");
    let mut app = fluxdi::Application::new(module);
    app.bootstrap()
        .await
        .map_err(|e| anyhow::anyhow!("Bootstrap failed: {e}"))?;
    tracing::info!("Session ready");

    let injector = app.injector();
    if let Some(signal) = stop.arrived() {
        tracing::info!("Received {signal} during boot, shutting the session down");
        return shut_down(app).await;
    }
    let session = injector.resolve::<FrontendSession>();
    let engine = injector.resolve::<ReactiveEngine>();
    let rt_handle = tokio::runtime::Handle::current();

    let initial_state = TuiState {
        session,
        engine,
        rt_handle,
        status_message: "Ready".to_string(),
        current_model: Arc::new(Mutex::new(Arc::new(ReactiveViewModel::empty()))),
        watch_started: Arc::new(AtomicBool::new(false)),
        last_registry: holon_tui::geometry::TuiGeometry::new(),
        focus_index: Arc::new(AtomicUsize::new(NO_FOCUS)),
        focus_pin: Arc::new(Mutex::new(None)),
        edit_state: Arc::new(Mutex::new(None)),
        leader_pending: Arc::new(AtomicBool::new(false)),
    };

    let tui_app = AppMain::new_boxed();

    let exit_keys = &[InputEvent::Keyboard(KeyPress::WithModifiers {
        key: Key::Character('q'),
        mask: r3bl_tui::ModifierKeysMask {
            ctrl_key_state: KeyState::Pressed,
            shift_key_state: KeyState::NotPressed,
            alt_key_state: KeyState::NotPressed,
        },
    })];

    // The reactive watch task is spawned lazily on the first render so it can
    // grab the main_thread_channel_sender from `GlobalData`. See
    // `app_main::ensure_watch_task_started`.
    let ran = tokio::select! {
        ran = async { TerminalWindow::main_event_loop(tui_app, exit_keys, initial_state)?.await } => {
            ran.map(|_| ()).map_err(|e| {
                let causes: Vec<String> = e.chain().map(ToString::to_string).collect();
                anyhow::anyhow!("{}", causes.join(": "))
            })
        }
        hung_up = holon_tui::terminal_hangup::hung_up() => {
            tracing::info!("the terminal hung up, quitting");
            hung_up.map_err(|e| anyhow::anyhow!("watching the terminal for a hangup failed: {e}"))
        }
        signal = stop.recv() => {
            tracing::info!("{signal} received, quitting");
            // The event loop leaves raw mode only on its own exit. Leaving it
            // reads no window size.
            RawMode::end(
                width(0) + height(0),
                lock_output_device_as_mut!(OutputDevice::new_stdout()),
                false,
            );
            Ok(())
        }
    };
    let session_shut_down = shut_down(app).await;
    first_then(ran, session_shut_down)
}

/// Stop the session's watchers, then close the store, then tear the container
/// down. The session shutdown's error is the process's exit status, returned
/// after the teardown has still run.
async fn shut_down(mut app: fluxdi::Application) -> anyhow::Result<()> {
    let session_shutdown = holon_app::shutdown_session(&app.injector()).await;

    // Container teardown — fires TuiModule::on_stop (MCP server stop, etc.)
    let timeout = std::time::Duration::from_secs(10);
    match tokio::time::timeout(timeout, app.shutdown()).await {
        Ok(Ok(())) => tracing::info!("Shutdown complete"),
        Ok(Err(e)) => tracing::warn!("Shutdown error: {e}"),
        Err(_) => tracing::warn!("Shutdown timed out after {timeout:?}"),
    }

    session_shutdown.map_err(|e| anyhow::anyhow!("Session shutdown failed: {e:#}"))
}

fn tui_log_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; holon-tui logs to $HOME/.config/holon"))?;
    let dir = PathBuf::from(home).join(".config").join("holon");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join("tui.log"))
}
