use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use holon_frontend::{FrontendConfig, FrontendSession};
use r3bl_tui::{
    log::try_initialize_logging_global, CommonResult, InputEvent, Key, KeyPress, KeyState,
    TerminalWindow,
};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use holon_tui::app_main::{AppMain, TuiState};
use holon_tui::state::AppState;

struct CliConfig {
    db_path: Option<PathBuf>,
    orgmode_root: Option<PathBuf>,
    loro_enabled: bool,
}

fn parse_args() -> Result<CliConfig> {
    let mut args = std::env::args().skip(1);
    let mut db_path: Option<PathBuf> = std::env::var("HOLON_DB_PATH").ok().map(PathBuf::from);
    let mut orgmode_root: Option<PathBuf> =
        std::env::var("HOLON_ORGMODE_ROOT").ok().map(PathBuf::from);
    let mut loro_enabled = std::env::var("HOLON_LORO_ENABLED")
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--orgmode-root" | "--orgmode-dir" => {
                let path_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--orgmode-root requires a path argument"))?;
                orgmode_root = Some(PathBuf::from(path_str));
            }
            "--loro" => {
                loro_enabled = true;
            }
            "--help" | "-h" => {
                eprintln!("Usage: holon-tui [OPTIONS] [DATABASE_PATH]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --orgmode-root PATH  OrgMode root directory");
                eprintln!("  --loro               Enable Loro CRDT layer");
                eprintln!("  --help, -h           Show this help message");
                std::process::exit(0);
            }
            _ => {
                if !arg.starts_with("--") {
                    db_path = Some(PathBuf::from(arg));
                }
            }
        }
    }

    Ok(CliConfig {
        db_path,
        orgmode_root,
        loro_enabled,
    })
}

fn build_frontend_config(cli: &CliConfig) -> FrontendConfig {
    let widgets = holon_tui::render_supported_widgets();
    let ui_info = holon_api::UiInfo {
        available_widgets: widgets,
        screen_size: None,
    };
    let mut config = FrontendConfig::new(ui_info);

    if let Some(ref db) = cli.db_path {
        config = config.with_db_path(db.clone());
    }
    if let Some(ref org) = cli.orgmode_root {
        config = config.with_orgmode(org.clone());
    }
    if cli.loro_enabled {
        config = config.with_loro();
    }

    config
}

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

    let cli_config = parse_args().map_err(|e| miette::miette!("{}", e))?;

    eprintln!(
        "TUI frontend: db={}, orgmode={:?}, loro={}",
        cli_config
            .db_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or("(temp)".into()),
        cli_config.orgmode_root,
        cli_config.loro_enabled
    );

    let frontend_config = build_frontend_config(&cli_config);
    let session = Arc::new(
        FrontendSession::new(frontend_config)
            .await
            .map_err(|e| miette::miette!("Failed to create frontend session: {}", e))?,
    );

    let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();
    let app_state = AppState::new(holon_api::widget_spec::WidgetSpec::from_rows(vec![]));

    let mut watch_handle = session
        .watch_ui(root_id.clone(), None, true)
        .await
        .map_err(|e| miette::miette!("watch_ui failed: {}", e))?;

    tracing::info!("watch_ui({root_id}) stream established");

    let rt_handle = tokio::runtime::Handle::current();

    // Spawn CDC listener that feeds UiEvents into AppState
    let cdc_state = app_state.clone_handle();
    let cdc_task = tokio::spawn(async move {
        while let Some(event) = watch_handle.recv().await {
            holon_tui::state::apply_event(&cdc_state, event);
        }
    });

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

    cdc_task.abort();
    render_task.abort();

    Ok(())
}
