use holon_frontend::cli;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::{FrontendSession, ReactiveViewModel};
use holon_tui::di::TuiModule;
use r3bl_tui::{
    log::try_initialize_logging_global, CommonResult, InputEvent, Key, KeyPress, KeyState,
    TerminalWindow,
};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use holon_tui::app_main::{AppMain, TuiState, NO_FOCUS};
use holon_tui::render::RenderRegistry;

#[tokio::main]
async fn main() -> CommonResult<()> {
    // Disable r3bl logging to prevent breaking TUI display
    try_initialize_logging_global(tracing_core::LevelFilter::OFF).ok(); // ALLOW(ok): best-effort logging init

    // TUI defaults to file logging (stderr/stdout would corrupt the terminal).
    // Override with HOLON_LOG env var if set.
    let _log_guard = if std::env::var("HOLON_LOG").is_ok() {
        holon_frontend::logging::init()
    } else {
        let log_file_path = tui_log_path();
        holon_frontend::logging::init_from(&format!("file://{}", log_file_path.display()))
    };

    let widgets = holon_tui::render_supported_widgets();
    let (holon_config, session_config, config_dir, locked) =
        cli::build_session(widgets).map_err(|e| miette::miette!("{}", e))?;

    let mut app = fluxdi::Application::new(TuiModule {
        holon_config,
        session_config,
        config_dir,
        locked_keys: locked,
    });
    app.bootstrap()
        .await
        .map_err(|e| miette::miette!("Bootstrap failed: {e}"))?;

    let injector = app.injector();
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
        last_registry: Arc::new(Mutex::new(RenderRegistry::default())),
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
    TerminalWindow::main_event_loop(tui_app, exit_keys, initial_state)?.await?;

    // Graceful shutdown — fires TuiModule::on_stop (MCP server stop, etc.)
    let timeout = std::time::Duration::from_secs(10);
    match tokio::time::timeout(timeout, app.shutdown()).await {
        Ok(Ok(())) => tracing::info!("Shutdown complete"),
        Ok(Err(e)) => tracing::warn!("Shutdown error: {e}"),
        Err(_) => tracing::warn!("Shutdown timed out after {timeout:?}"),
    }

    Ok(())
}

fn tui_log_path() -> std::path::PathBuf {
    let mut path = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    path.push(".config");
    path.push("holon");
    std::fs::create_dir_all(&path).ok(); // ALLOW(ok): best-effort dir creation
    path.push("tui.log");
    path
}
