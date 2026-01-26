extern crate macroquad_ply as macroquad;

mod render;
mod state;

use std::sync::Arc;

use anyhow::Result;
use ply_engine::grow;
use ply_engine::layout::LayoutDirection;
use ply_engine::renderer::FontAsset;

use holon_frontend::cli;
use holon_frontend::FrontendSession;

use state::AppState;

struct HolonState {
    session: Arc<FrontendSession>,
    app_state: AppState,
    rt_handle: tokio::runtime::Handle,
    block_watch: holon_frontend::BlockWatchRegistry,
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

    let config = cli::parse_args("holon-ply").expect("Failed to parse CLI args");
    config.log_summary("Ply");

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
                let widgets: std::collections::HashSet<String> = render::builders::builder_names()
                    .iter()
                    .map(|s| String::from(*s))
                    .collect();
                let frontend_config = cli::build_frontend_config(&config, widgets);
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

                let root_uri = holon_api::root_layout_block_uri();

                let watch = session.watch_ui(&root_uri, true).await?;

                tracing::info!("watch_ui({root_uri}) stream established");

                let rt_handle = runtime.handle().clone();

                let app_state = holon_frontend::cdc::spawn_ui_listener(watch);

                let block_watch = holon_frontend::BlockWatchRegistry::new(
                    Arc::clone(&session),
                    rt_handle.clone(),
                );

                Ok::<_, anyhow::Error>(HolonState {
                    session,
                    app_state,
                    rt_handle,
                    block_watch,
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
                            let mut render_ctx = render::context::new_render_context(
                                Arc::clone(&h.session),
                                h.rt_handle.clone(),
                                h.block_watch.clone(),
                                Arc::clone(&h.left_sidebar_block_id),
                            );
                            render_ctx.ctx.data_rows = widget_spec.data.clone();

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
