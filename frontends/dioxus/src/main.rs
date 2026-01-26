use dioxus::prelude::*;

mod operations;
mod render;

use std::sync::Arc;

use holon_api::widget_spec::WidgetSpec;
use holon_frontend::cli;
use holon_frontend::FrontendSession;

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

fn main() {
    // MUST be before any allocations — holds the profiler alive until main() returns
    #[cfg(feature = "heap-profile")]
    let _profiler = holon_frontend::memory_monitor::heap_profile::start();

    tracing_subscriber::fmt::init();

    let config = cli::CliConfig::from_env();
    let frontend_config = cli::build_frontend_config(&config, dioxus_widgets());

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let session = runtime
        .block_on(async {
            tracing::info!("Starting Dioxus frontend...");
            FrontendSession::new(frontend_config).await
        })
        .expect("FrontendSession::new failed");

    let session = Arc::new(session);
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
    let mut widget_spec = use_signal(|| WidgetSpec::from_rows(vec![]));

    // Bridge: tokio watch channel (Send) -> dioxus signal (!Send)
    let watch_rx = use_hook(|| {
        let (tx, rx) = tokio::sync::watch::channel(WidgetSpec::from_rows(vec![]));

        let session = session.clone();
        rt.spawn(async move {
            let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();
            let watch = session
                .watch_ui(root_id.clone(), None, true)
                .await
                .expect("watch_ui failed");

            tracing::info!("watch_ui({root_id}) stream established");

            let cdc_state =
                holon_frontend::CdcState::new(WidgetSpec::from_rows(vec![]), move |ws| {
                    let _ = tx.send(ws);
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
                let ws = rx.borrow_and_update().clone();
                widget_spec.set(ws);
            }
        }
    });

    let session_render: Arc<FrontendSession> = use_context();
    let rt_render: tokio::runtime::Handle = use_context();
    let ws = widget_spec.read();

    let session_keys: Arc<FrontendSession> = use_context();
    let content = render::render_widget_spec(&ws, &session_render, &rt_render);

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
        "col",
        "spacer",
        "list",
        "tree",
        "columns",
        "editable_text",
        "selectable",
        "icon",
        "live_query",
        "render_block",
        "block_ref",
        "table",
        "section",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
