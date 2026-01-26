use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use dioxus::prelude::*;
use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::ModuleLifecycleFuture;
use fluxdi::Shared;
use futures::StreamExt;
use holon::di::CoreInfraModule;
use holon_app::HolonFrontendModule;
use holon_frontend::FrontendSession;
use holon_frontend::cli;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::preferences::PrefKey;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::BuilderServicesSlot;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::RenderInterpreterInjectorExt;
use holon_frontend::reactive::make_interpret_fn;
use holon_frontend::view_model::ViewModel;

mod editor;
mod render;

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

struct DioxusModule {
    holon_config: HolonConfig,
    session_config: SessionConfig,
    config_dir: PathBuf,
    locked_keys: HashSet<PrefKey>,
}

impl DioxusModule {
    fn core_module(&self) -> CoreInfraModule {
        CoreInfraModule {
            db_path: self.holon_config.resolve_db_path(&self.config_dir),
        }
    }

    fn frontend_module(&self) -> HolonFrontendModule {
        HolonFrontendModule {
            holon_config: self.holon_config.clone(),
            session_config: self.session_config.clone(),
            config_dir: self.config_dir.clone(),
            locked_keys: self.locked_keys.clone(),
        }
    }
}

impl Module for DioxusModule {
    fn configure(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        self.core_module().configure(injector)?;
        self.frontend_module().configure(injector)?;

        // The ReactiveEngine produces ViewModels by interpreting render-DSL
        // exprs through the shadow builders; wire the shared interpret fn.
        let slot = injector.resolve::<BuilderServicesSlot>();
        injector.set_render_interpreter(make_interpret_fn(slot.0.clone()));

        Ok(())
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let _session = injector.resolve_async::<FrontendSession>().await;

            // Populate BuilderServicesSlot with the live engine so the
            // interpret fn (and shadow builders) can resolve services.
            let engine = injector.resolve::<ReactiveEngine>();
            let slot = injector.resolve::<BuilderServicesSlot>();
            let services: Arc<dyn BuilderServices> = engine.clone();
            slot.0.set(services).ok();

            Ok(())
        })
    }
}

// ── Main ────────────────────────────────────────────────────────────────────

fn main() {
    // MUST be before any allocations — holds the profiler alive until main()
    // returns
    #[cfg(feature = "heap-profile")]
    let _profiler = holon_frontend::memory_monitor::heap_profile::start();

    tracing_subscriber::fmt::init();

    holon_frontend::shadow_builders::register_render_dsl_widget_names();

    let (holon_config, session_config, config_dir, locked) =
        cli::build_session(render_supported_widgets()).expect("Failed to load config");
    // Don't block window paint on the OrgMode initial scan; the reactive
    // layer fills in data as it arrives.
    let session_config = session_config.without_wait();

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
    let engine = injector.resolve::<ReactiveEngine>();
    let rt_handle = runtime.handle().clone();

    // Keep the tokio runtime alive on a background thread; the webview blocks
    // the main thread in launch() below.
    std::thread::spawn(move || {
        runtime.block_on(std::future::pending::<()>());
    });

    LaunchBuilder::new()
        .with_context(session)
        .with_context(engine)
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
    let engine: Arc<ReactiveEngine> = use_context();
    let rt: tokio::runtime::Handle = use_context();
    let session_keys: Arc<FrontendSession> = use_context();
    let mut view_model: Signal<Option<ViewModel>> = use_signal(|| None);

    // Bridge: tokio watch stream (Send) -> dioxus signal (!Send), carrying the
    // root-layout ViewModel snapshots produced by ReactiveEngine::watch.
    let watch_rx = use_hook({
        let engine = engine.clone();
        let rt = rt.clone();
        move || {
            let (tx, rx) = tokio::sync::watch::channel::<Option<ViewModel>>(None);
            rt.spawn(async move {
                let uri = holon_api::root_layout_block_uri();
                let mut stream = engine.watch(&uri);
                while let Some(rvm) = stream.next().await {
                    if tx.send(Some(rvm.snapshot())).is_err() {
                        break;
                    }
                }
            });
            rx
        }
    });

    use_future(move || {
        let mut rx = watch_rx.clone();
        async move {
            while rx.changed().await.is_ok() {
                view_model.set(rx.borrow_and_update().clone());
            }
        }
    });

    let content = match &*view_model.read() {
        Some(vm) => rsx! { render::RenderNode { node: vm.clone() } },
        None => rsx! { div { style: "padding: 16px; color: var(--text-muted);", "Loading…" } },
    };

    rsx! {
        div {
            onkeydown: move |evt: KeyboardEvent| {
                let meta = evt.modifiers().meta();
                let shift = evt.modifiers().shift();
                match (meta, shift, evt.key()) {
                    (true, false, Key::Character(c)) if c == "z" => {
                        let s = session_keys.clone();
                        rt.spawn(async move {
                            if let Err(e) = s.undo().await {
                                tracing::error!("Undo failed: {e}");
                            }
                        });
                    }
                    (true, true, Key::Character(c)) if c == "z" || c == "Z" => {
                        let s = session_keys.clone();
                        rt.spawn(async move {
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

/// Widget names the render layer supports — derived from the macro-generated
/// builder set plus the collection layouts handled via the reactive shell.
fn render_supported_widgets() -> HashSet<String> {
    let mut widgets: HashSet<String> = render::builders::builder_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    for name in ["table", "tree", "list", "outline", "columns"] {
        widgets.insert(name.to_string());
    }
    widgets
}
