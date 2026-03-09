extern crate macroquad_ply as macroquad;

mod cdc;
mod render;
mod state;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use ply_engine::grow;
use ply_engine::layout::LayoutDirection;
use ply_engine::renderer::FontAsset;

use holon_frontend::{FrontendConfig, FrontendSession};

use state::AppState;

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
                eprintln!("Usage: holon-ply [OPTIONS] [DATABASE_PATH]");
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

fn supported_widgets() -> std::collections::HashSet<String> {
    render::builders::builder_names()
        .iter()
        .map(|s| String::from(*s))
        .collect()
}

fn build_frontend_config(cli: &CliConfig) -> FrontendConfig {
    let ui_info = holon_api::UiInfo {
        available_widgets: supported_widgets(),
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

struct HolonState {
    session: Arc<FrontendSession>,
    app_state: AppState,
    rt_handle: tokio::runtime::Handle,
    block_cache: holon_frontend::BlockRenderCache,
    /// Block ID of the left sidebar, discovered during screen layout render.
    left_sidebar_block_id: Arc<std::sync::Mutex<Option<String>>>,
}

#[macroquad::main("Holon")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "holon_ply=info,holon=info".into()),
        )
        .init();

    let config = parse_args().expect("Failed to parse CLI args");

    eprintln!(
        "Ply frontend: db={}, orgmode={:?}, loro={}",
        config
            .db_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or("(temp)".into()),
        config.orgmode_root,
        config.loro_enabled
    );

    static DEFAULT_FONT: FontAsset = FontAsset::Bytes {
        file_name: "Inter-Regular.ttf",
        data: include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
    };
    let mut ply = ply_engine::Ply::<()>::new(&DEFAULT_FONT).await;

    // Start holon init on a background thread (non-blocking for macroquad)
    let init_result: Arc<std::sync::Mutex<Option<Result<HolonState>>>> =
        Arc::new(std::sync::Mutex::new(None));

    {
        let result_slot = Arc::clone(&init_result);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

            let init = runtime.block_on(async {
                let frontend_config = build_frontend_config(&config);
                tracing::info!("Starting Ply frontend...");
                let session = Arc::new(FrontendSession::new(frontend_config).await?);

                // Start MCP server
                {
                    let mcp_engine = session.engine().clone();
                    let mcp_port: u16 = std::env::var("MCP_SERVER_PORT")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(8521);
                    let bind_address = std::net::SocketAddr::from(([127, 0, 0, 1], mcp_port));
                    let cancellation_token = tokio_util::sync::CancellationToken::new();
                    tracing::info!("Starting MCP server on http://{}", bind_address);
                    tokio::spawn(async move {
                        if let Err(e) = holon_mcp::di::run_http_server(
                            mcp_engine,
                            Arc::new(holon_mcp::server::DebugServices::default()),
                            bind_address,
                            cancellation_token,
                        )
                        .await
                        {
                            tracing::error!("MCP server error: {}", e);
                        }
                    });
                }

                let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();
                let app_state =
                    AppState::new(holon_api::widget_spec::WidgetSpec::from_rows(vec![]));

                let mut watch = session.watch_ui(root_id.clone(), None, true).await?;

                tracing::info!("watch_ui({root_id}) stream established");

                let rt_handle = runtime.handle().clone();

                // Spawn CDC listener — owns the WatchHandle, keeping the UiWatcher alive
                let cdc_state = app_state.clone_handle();
                rt_handle.spawn(async move {
                    while let Some(event) = watch.recv().await {
                        cdc::apply_event(&cdc_state, event);
                    }
                    tracing::info!("UiEvent stream ended");
                });

                let block_cache =
                    holon_frontend::BlockRenderCache::new(Arc::clone(&session), rt_handle.clone());

                Ok::<_, anyhow::Error>(HolonState {
                    session,
                    app_state,
                    rt_handle,
                    block_cache,
                    left_sidebar_block_id: Arc::new(std::sync::Mutex::new(None)),
                })
            });

            *result_slot.lock().unwrap() = Some(init);

            // Keep the runtime alive
            runtime.block_on(std::future::pending::<()>());
        });
    }

    let mut holon: Option<HolonState> = None;

    loop {
        // Check if init completed
        if holon.is_none() {
            let mut slot = init_result.lock().unwrap();
            if let Some(result) = slot.take() {
                match result {
                    Ok(state) => {
                        tracing::info!("Holon backend ready");
                        holon = Some(state);
                    }
                    Err(e) => {
                        tracing::error!("Holon init failed: {e}");
                        // Show error and keep looping
                    }
                }
            }
        }

        macroquad::prelude::clear_background(macroquad::prelude::Color::new(0.1, 0.1, 0.1, 1.0));

        let mut ui = ply.begin();
        ui.element()
            .width(grow!())
            .height(grow!())
            .background_color(0x1A1A1Au32)
            .layout(|l| l.direction(LayoutDirection::TopToBottom))
            .children(|ui| {
                // Title bar
                {
                    let sidebar_bid = holon
                        .as_ref()
                        .and_then(|h| h.left_sidebar_block_id.lock().unwrap().clone());
                    let is_open = holon
                        .as_ref()
                        .and_then(|h| {
                            sidebar_bid.as_ref().and_then(|bid| {
                                h.session.ui_settings().widgets.get(bid).map(|ws| ws.open)
                            })
                        })
                        .unwrap_or(true);

                    ui.element()
                        .width(grow!())
                        .height(ply_engine::layout::Sizing::Fixed(32.0))
                        .background_color(0x222222u32)
                        .layout(|l| {
                            l.direction(LayoutDirection::LeftToRight)
                                .padding(ply_engine::layout::Padding::new(4, 0, 0, 0))
                                .align(
                                    ply_engine::align::AlignX::Left,
                                    ply_engine::align::AlignY::CenterY,
                                )
                        })
                        .children(|ui| {
                            if sidebar_bid.is_some() {
                                let label = if is_open { "\u{2630}" } else { "\u{2261}" };
                                let session = holon.as_ref().map(|h| Arc::clone(&h.session));
                                let bid = sidebar_bid.clone();
                                ui.element()
                                    .width(ply_engine::layout::Sizing::Fixed(28.0))
                                    .height(ply_engine::layout::Sizing::Fixed(28.0))
                                    .layout(|l| {
                                        l.align(
                                            ply_engine::align::AlignX::CenterX,
                                            ply_engine::align::AlignY::CenterY,
                                        )
                                    })
                                    .on_press(move |_id, _pointer| {
                                        if let (Some(ref s), Some(ref b)) = (&session, &bid) {
                                            s.set_widget_open(b, !is_open);
                                        }
                                    })
                                    .children(|ui| {
                                        ui.text(label, |t| t.font_size(16).color(0xCCCCCCu32));
                                    });
                            }
                            ui.text("Holon", |t| t.font_size(14).color(0xE0E0E0u32));
                        });
                }

                // Main content
                ui.element()
                    .width(grow!())
                    .height(grow!())
                    .layout(|l| {
                        l.direction(LayoutDirection::TopToBottom)
                            .padding(8u16)
                            .align(
                                ply_engine::align::AlignX::Left,
                                ply_engine::align::AlignY::CenterY,
                            )
                    })
                    .children(|ui| {
                        if let Some(ref h) = holon {
                            let widget_spec = h.app_state.widget_spec();
                            let data_rows: Vec<_> =
                                widget_spec.data.iter().map(|r| r.data.clone()).collect();
                            let mut render_ctx = render::context::new_render_context(
                                Arc::clone(&h.session),
                                h.rt_handle.clone(),
                                h.block_cache.clone(),
                                Arc::clone(&h.left_sidebar_block_id),
                            );
                            render_ctx.data_rows = data_rows;
                            render_ctx.is_screen_layout = true;

                            let root_widget = render::interpreter::interpret(
                                &widget_spec.render_expr,
                                &render_ctx,
                            );
                            root_widget(ui);
                        } else {
                            ui.text("Loading...", |t| t.font_size(16).color(0x888888u32));
                        }
                    });
            });

        ply.show(|_command| {}).await;
        macroquad::prelude::next_frame().await;
    }
}
