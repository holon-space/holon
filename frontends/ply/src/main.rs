extern crate macroquad_ply as macroquad;

mod render;
mod state;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use fluxdi::{Injector, Module, ModuleLifecycleFuture, Shared};
use ply_engine::grow;
use ply_engine::layout::LayoutDirection;
use ply_engine::renderer::FontAsset;

use holon_frontend::cli;
use holon_frontend::config::{HolonConfig, SessionConfig};
use holon_frontend::frontend_module::FrontendInjectorExt;
use holon_frontend::preferences::PrefKey;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine, RenderInterpreterInjectorExt};
use holon_frontend::FrontendSession;
use holon_mcp::di::McpServerHandle;
use holon_mcp::McpInjectorExt;

// ── PlyModule ───────────────────────────────────────────────────────────────

fn to_di_err(phase: &str, e: &dyn std::fmt::Display) -> fluxdi::Error {
    fluxdi::Error::module_lifecycle_failed("PlyModule", phase, &e.to_string())
}

struct PlyModule {
    holon_config: HolonConfig,
    session_config: SessionConfig,
    config_dir: PathBuf,
    locked_keys: HashSet<PrefKey>,
}

impl Module for PlyModule {
    fn configure(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        let db_path = self.holon_config.resolve_db_path(&self.config_dir);

        holon::di::open_and_register_core(injector, db_path)
            .map_err(|e| to_di_err("configure", &e))?;

        injector
            .add_frontend(
                self.holon_config.clone(),
                self.session_config.clone(),
                self.config_dir.clone(),
                self.locked_keys.clone(),
            )
            .map_err(|e| to_di_err("configure", &e))?;

        injector.set_render_interpreter(|_expr, _rows| {
            holon_frontend::reactive_view_model::ReactiveViewModel::empty()
        });

        injector.add_mcp_server(8521)?;

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let _session = injector.resolve_async::<FrontendSession>().await;

            let mcp = injector.resolve::<McpServerHandle>();
            mcp.start().await.map_err(|e| to_di_err("on_start", &e))?;

            Ok(())
        })
    }

    fn on_stop(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let mcp = injector.resolve::<McpServerHandle>();
            mcp.stop().await.map_err(|e| to_di_err("on_stop", &e))?;
            Ok(())
        })
    }
}

// ── Main ────────────────────────────────────────────────────────────────────

struct HolonState {
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
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

    let widgets: std::collections::HashSet<String> = render::builders::builder_names()
        .iter()
        .map(|s| String::from(*s))
        .collect();
    let (holon_config, session_config, config_dir, locked) =
        cli::build_session(widgets).expect("Failed to load config");

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
                tracing::info!("Starting Ply frontend...");

                let mut app = fluxdi::Application::new(PlyModule {
                    holon_config,
                    session_config,
                    config_dir,
                    locked_keys: locked,
                });
                app.bootstrap()
                    .await
                    .map_err(|e| anyhow::anyhow!("Bootstrap failed: {e}"))?;

                let injector = app.injector();
                let session = injector.resolve::<FrontendSession>();
                let engine = injector.resolve::<ReactiveEngine>();

                Ok::<_, anyhow::Error>(HolonState {
                    session,
                    engine,
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
                            let root_uri = holon_api::root_layout_block_uri();
                            let results = h.engine.ensure_watching(&root_uri);
                            let (render_expr, data_rows) = results.snapshot();
                            let services: Arc<dyn BuilderServices> = h.engine.clone();
                            let mut render_ctx = render::context::new_render_context(
                                services,
                                Arc::clone(&h.left_sidebar_block_id),
                            );
                            render_ctx.ctx.data_rows = data_rows;

                            let root_widget =
                                render::interpreter::interpret(&render_expr, &render_ctx);
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
