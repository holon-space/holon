use dioxus::prelude::*;

mod operations;
mod render;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::{Injector, Module, ModuleLifecycleFuture, Shared};
use holon_api::render_types::RenderExpr;
use holon_frontend::cli;
use holon_frontend::config::{HolonConfig, SessionConfig};
use holon_frontend::frontend_module::FrontendInjectorExt;
use holon_frontend::preferences::PrefKey;
use holon_frontend::reactive::RenderInterpreterInjectorExt;
use holon_frontend::{FrontendSession, RenderSnapshot};

const BASE_CSS: &str = r#"<style>
:root {
    --bg: #121212;
    --bg-sidebar: #1E1E1E;
    --surface: #1A1A1A;
    --surface-elevated: #2A2A2A;
    --border: #333333;
    --text-primary: #E0E0E0;
    --text-secondary: #B0B0B0;
    --text-muted: #808080;
    --accent: #7B9FFF;
    --success: #4CAF50;
    --warning: #FFA726;
    --info: #42A5F5;
    --error: #FF5252;
}
html, body {
    margin: 0;
    padding: 0;
    background: var(--bg);
    color: var(--text-primary);
    font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    font-size: 14px;
    line-height: 1.5;
    -webkit-font-smoothing: antialiased;
}
* { box-sizing: border-box; }
::-webkit-scrollbar { width: 8px; height: 8px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: #444; border-radius: 4px; }
::-webkit-scrollbar-thumb:hover { background: #555; }
::selection { background: rgba(123, 159, 255, 0.3); }
input, textarea {
    font-family: inherit;
    font-size: inherit;
}
pre, code {
    font-family: "SF Mono", "Fira Code", "Cascadia Code", Menlo, monospace;
}
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
</style>"#;

// ── DioxusModule ─────────────────────────────────────────────────────────────

fn to_di_err(phase: &str, e: &dyn std::fmt::Display) -> fluxdi::Error {
    fluxdi::Error::module_lifecycle_failed("DioxusModule", phase, &e.to_string())
}

struct DioxusModule {
    holon_config: HolonConfig,
    session_config: SessionConfig,
    config_dir: PathBuf,
    locked_keys: HashSet<PrefKey>,
}

impl Module for DioxusModule {
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

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let _session = injector.resolve_async::<FrontendSession>().await;

            Ok(())
        })
    }
}

// ── Main ────────────────────────────────────────────────────────────────────

fn main() {
    // MUST be before any allocations — holds the profiler alive until main() returns
    #[cfg(feature = "heap-profile")]
    let _profiler = holon_frontend::memory_monitor::heap_profile::start();

    tracing_subscriber::fmt::init();

    let (holon_config, session_config, config_dir, locked) =
        cli::build_session(dioxus_widgets()).expect("Failed to load config");

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let app = runtime
        .block_on(async {
            tracing::info!("Starting Dioxus frontend...");

            let mut app = fluxdi::Application::new(DioxusModule {
                holon_config,
                session_config,
                config_dir,
                locked_keys: locked,
            });
            app.bootstrap()
                .await
                .map_err(|e| anyhow::anyhow!("Bootstrap failed: {e}"))?;

            tracing::info!("Session ready");
            Ok::<_, anyhow::Error>(app)
        })
        .expect("Bootstrap failed");

    let injector = app.injector();
    let session = injector.resolve::<FrontendSession>();
    let rt_handle = runtime.handle().clone();

    // Keep the tokio runtime alive in a background thread
    std::thread::spawn(move || {
        runtime.block_on(std::future::pending::<()>());
    });

    LaunchBuilder::new()
        .with_context(session)
        .with_context(rt_handle)
        .with_cfg(
            dioxus::desktop::Config::new()
                .with_custom_head(BASE_CSS.to_string())
                .with_window(
                    dioxus::desktop::WindowBuilder::new()
                        .with_title("Holon")
                        .with_inner_size(dioxus::desktop::LogicalSize::new(1200.0, 800.0)),
                ),
        )
        .launch(App);
}

#[component]
fn App() -> Element {
    let session: Arc<FrontendSession> = use_context();
    let rt: tokio::runtime::Handle = use_context();
    let default_snapshot: RenderSnapshot = (
        RenderExpr::FunctionCall { name: "spacer".into(), args: vec![] },
        vec![],
    );
    let mut render_snapshot = use_signal(|| default_snapshot.clone());

    // Bridge: tokio watch channel (Send) -> dioxus signal (!Send)
    let watch_rx = use_hook(|| {
        let (tx, rx) = tokio::sync::watch::channel(default_snapshot);

        let session = session.clone();
        rt.spawn(async move {
            let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();
            let watch = session
                .watch_ui(root_id.clone(), None, true)
                .await
                .expect("watch_ui failed");

            tracing::info!("watch_ui({root_id}) stream established");

            let initial_expr = RenderExpr::FunctionCall { name: "spacer".into(), args: vec![] };
            let cdc_state =
                holon_frontend::CdcState::new(initial_expr, move |snapshot| {
                    let _ = tx.send(snapshot);
                });
            holon_frontend::cdc::ui_event_listener(watch, cdc_state).await;
        });

        rx
    });

    // Poll the watch channel on the UI thread
    use_future(move || {
        let mut rx = watch_rx.clone();
        async move {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let snapshot = rx.borrow_and_update().clone();
                render_snapshot.set(snapshot);
            }
        }
    });

    let session_render: Arc<FrontendSession> = use_context();
    let rt_render: tokio::runtime::Handle = use_context();
    let (ref render_expr, ref data_rows) = *render_snapshot.read();

    let session_keys: Arc<FrontendSession> = use_context();
    let content = render::render_snapshot(render_expr, data_rows, &session_render, &rt_render);

    rsx! {
        div {
            onkeydown: move |evt: KeyboardEvent| {
                let meta = evt.modifiers().meta();
                let shift = evt.modifiers().shift();
                match (meta, shift, evt.key()) {
                    (true, false, Key::Character(c)) if c == "z" => {
                        let s = session_keys.clone();
                        tokio::spawn(async move {
                            if let Err(e) = s.undo().await {
                                tracing::error!("Undo failed: {e}");
                            }
                        });
                    }
                    (true, true, Key::Character(c)) if c == "z" || c == "Z" => {
                        let s = session_keys.clone();
                        tokio::spawn(async move {
                            if let Err(e) = s.redo().await {
                                tracing::error!("Redo failed: {e}");
                            }
                        });
                    }
                    _ => {}
                }
            },
            {content}
        }
    }
}

fn dioxus_widgets() -> std::collections::HashSet<String> {
    [
        "text",
        "row",
        "column",
        "spacer",
        "list",
        "tree",
        "columns",
        "editable_text",
        "selectable",
        "icon",
        "live_query",
        "render_entity",
        "live_block",
        "table",
        "section",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
