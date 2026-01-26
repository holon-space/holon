use std::path::PathBuf;
use std::sync::Arc;

use holon_frontend::cli;
use holon_frontend::FrontendSession;
use r3bl_tui::{
    log::try_initialize_logging_global, CommonResult, InputEvent, Key, KeyPress, KeyState,
    TerminalWindow,
};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use holon_frontend::cdc::spawn_ui_listener;
use holon_tui::app_main::{AppMain, TuiState};

#[tokio::main]
async fn main() -> CommonResult<()> {
    // Disable r3bl logging to prevent breaking TUI display
    try_initialize_logging_global(tracing_core::LevelFilter::OFF).ok();

    // Set up file-based logging
    let log_file_path = if let Some(home) = std::env::var_os("HOME") {
        let mut path = PathBuf::from(home);
        path.push(".config");
        path.push("holon");
        std::fs::create_dir_all(&path).ok();
        path.push("tui.log");
        path
    } else {
        PathBuf::from("tui.log")
    };

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path)
        .unwrap_or_else(|_| std::fs::File::create("tui.log").unwrap());

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("turso_core=warn".parse().unwrap())
        .add_directive("turso_core::storage=warn".parse().unwrap())
        .add_directive("turso_core::vdbe=warn".parse().unwrap());

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(log_file).with_ansi(false))
        .init();

    let cli_config = cli::parse_args("holon-tui").map_err(|e| miette::miette!("{}", e))?;
    cli_config.log_summary("TUI");

    let widgets = holon_tui::render_supported_widgets();
    let frontend_config = cli::build_frontend_config(&cli_config, widgets);
    let session = Arc::new(
        FrontendSession::new(frontend_config)
            .await
            .map_err(|e| miette::miette!("Failed to create frontend session: {}", e))?,
    );

    let root_uri = holon_api::root_layout_block_uri();

    let watch_handle = session
        .watch_ui(&root_uri, true)
        .await
        .map_err(|e| miette::miette!("watch_ui failed: {}", e))?;

    tracing::info!("watch_ui({root_uri}) stream established");

    let rt_handle = tokio::runtime::Handle::current();

    let app_state = spawn_ui_listener(watch_handle);

    let initial_state = TuiState {
        session,
        app_state,
        rt_handle,
        status_message: "Ready".to_string(),
    };

    let app = AppMain::new_boxed();

    let exit_keys = &[InputEvent::Keyboard(KeyPress::WithModifiers {
        key: Key::Character('q'),
        mask: r3bl_tui::ModifierKeysMask {
            ctrl_key_state: KeyState::Pressed,
            shift_key_state: KeyState::NotPressed,
            alt_key_state: KeyState::NotPressed,
        },
    })];

    // Spawn a periodic render trigger to pick up CDC changes
    let render_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            // The r3bl framework doesn't have an easy way to trigger rerender from outside,
            // so we rely on the framework's polling behavior.
        }
    });

    TerminalWindow::main_event_loop(app, exit_keys, initial_state)?.await?;

    render_task.abort();

    Ok(())
}
