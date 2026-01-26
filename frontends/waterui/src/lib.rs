mod render;

use std::path::PathBuf;
use std::sync::Arc;

use waterui::app::App;
use waterui::prelude::*;
use waterui::reactive::binding;

use holon_api::widget_spec::WidgetSpec;
use holon_frontend::render_interpreter::RenderInterpreter;
use holon_frontend::cdc::AppState;
use holon_frontend::{FrontendConfig, FrontendSession, RenderContext};

fn render_widget_spec(
    widget_spec: &WidgetSpec,
    session: &Arc<FrontendSession>,
    rt: &tokio::runtime::Handle,
    interpreter: &RenderInterpreter<AnyView>,
) -> AnyView {
    let ctx = RenderContext::new(Arc::clone(session), rt.clone());
    let render_ctx = ctx.with_data_rows(widget_spec.data.clone());
    interpreter.interpret(&widget_spec.render_expr, &render_ctx)
}

pub fn app(env: Environment) -> App {
    let config = parse_env_config();

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let interpreter = render::builders::create_interpreter();

    let initial_spec = WidgetSpec::from_rows(vec![]);
    let widget_spec_binding: Binding<WidgetSpec> = binding(initial_spec.clone());
    let mailbox = Arc::new(widget_spec_binding.mailbox());

    let session = runtime
        .block_on(async {
            let frontend_config = build_frontend_config(&config, &interpreter);
            tracing::info!("Starting WaterUI frontend...");
            let session = Arc::new(
                FrontendSession::new(frontend_config)
                    .await
                    .expect("FrontendSession::new failed"),
            );

            let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();

            let watch = session
                .watch_ui(root_id.clone(), None, true)
                .await
                .expect("watch_ui failed");

            tracing::info!("watch_ui({root_id}) stream established");

            let app_state: AppState = holon_frontend::cdc::spawn_ui_listener(watch);

            // Bridge AppState changes into WaterUI's reactive binding system
            let bridge_mailbox = Arc::clone(&mailbox);
            let mut bridge_state = app_state.clone();
            tokio::spawn(async move {
                while bridge_state.changed().await {
                    let ws = bridge_state.widget_spec();
                    bridge_mailbox.handle(move |b| b.set(ws));
                }
            });

            Ok::<_, anyhow::Error>(session)
        })
        .expect("Startup failed");

    let rt_handle = runtime.handle().clone();

    std::thread::spawn(move || {
        runtime.block_on(std::future::pending::<()>());
    });

    let interpreter = Arc::new(interpreter);

    App::new(
        move || {
            let session = Arc::clone(&session);
            let rt = rt_handle.clone();
            let interp = Arc::clone(&interpreter);
            watch(widget_spec_binding.clone(), move |ws: WidgetSpec| {
                render_widget_spec(&ws, &session, &rt, &interp)
            })
        },
        env,
    )
}

struct EnvConfig {
    db_path: Option<PathBuf>,
    orgmode_root: Option<PathBuf>,
    loro_enabled: bool,
}

fn parse_env_config() -> EnvConfig {
    EnvConfig {
        db_path: std::env::var("HOLON_DB_PATH").ok().map(PathBuf::from),
        orgmode_root: std::env::var("HOLON_ORGMODE_ROOT").ok().map(PathBuf::from),
        loro_enabled: std::env::var("HOLON_LORO_ENABLED")
            .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
            .unwrap_or(false),
    }
}

fn build_frontend_config(
    cli: &EnvConfig,
    interpreter: &RenderInterpreter<AnyView>,
) -> FrontendConfig {
    let ui_info = holon_api::UiInfo {
        available_widgets: interpreter.supported_widgets(),
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

waterui_ffi::export!();
