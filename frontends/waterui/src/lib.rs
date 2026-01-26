mod render;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::{Injector, Module, ModuleLifecycleFuture, Shared};
use waterui::app::App;
use waterui::prelude::*;
use waterui::reactive::binding;

use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::config::{HolonConfig, SessionConfig};
use holon_frontend::frontend_module::FrontendInjectorExt;
use holon_frontend::reactive::RenderInterpreterInjectorExt;
use holon_frontend::render_interpreter::RenderInterpreter;
use holon_frontend::{FrontendSession, RenderContext};

// ── WaterUiModule ───────────────────────────────────────────────────────────

fn to_di_err(phase: &str, e: &dyn std::fmt::Display) -> fluxdi::Error {
    fluxdi::Error::module_lifecycle_failed("WaterUiModule", phase, &e.to_string())
}

struct WaterUiModule {
    holon_config: HolonConfig,
    session_config: SessionConfig,
    config_dir: PathBuf,
}

impl Module for WaterUiModule {
    fn configure(&self, injector: &Injector) -> Result<(), fluxdi::Error> {
        let db_path = self.holon_config.resolve_db_path(&self.config_dir);

        holon::di::open_and_register_core(injector, db_path)
            .map_err(|e| to_di_err("configure", &e))?;

        injector
            .add_frontend(
                self.holon_config.clone(),
                self.session_config.clone(),
                self.config_dir.clone(),
                HashSet::new(),
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

// ── Rendering helper ────────────────────────────────────────────────────────

fn render_from_snapshot(
    render_expr: &RenderExpr,
    data_rows: &[Arc<DataRow>],
    session: &Arc<FrontendSession>,
    rt: &tokio::runtime::Handle,
    interpreter: &RenderInterpreter<AnyView>,
) -> AnyView {
    let ctx = RenderContext::new(Arc::clone(session), rt.clone());
    let render_ctx = ctx.with_data_rows(data_rows.to_vec());
    interpreter.interpret(render_expr, &render_ctx)
}

// ── App entry point ─────────────────────────────────────────────────────────

pub fn app(env: Environment) -> App {
    let env_config = parse_env_config();

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let interpreter = render::builders::create_interpreter();

    let initial_snapshot: RenderSnapshot = (
        RenderExpr::FunctionCall { name: "spacer".into(), args: vec![] },
        vec![],
    );
    let snapshot_binding: Binding<RenderSnapshot> = binding(initial_snapshot.clone());
    let mailbox = Arc::new(snapshot_binding.mailbox());

    let holon_config = HolonConfig {
        db_path: env_config.db_path,
        orgmode: holon_frontend::config::OrgmodeConfig {
            root_directory: env_config.orgmode_root,
        },
        loro: holon_frontend::config::LoroPreferences {
            enabled: if env_config.loro_enabled { Some(true) } else { None },
            storage_dir: None,
        },
        ..Default::default()
    };
    let config_dir = holon_frontend::config::resolve_config_dir(None);
    let ui_info = holon_api::UiInfo {
        available_widgets: interpreter.supported_widgets(),
        screen_size: None,
    };
    let session_config = SessionConfig::new(ui_info);

    let session = runtime
        .block_on(async {
            tracing::info!("Starting WaterUI frontend...");

            let mut di_app = fluxdi::Application::new(WaterUiModule {
                holon_config,
                session_config,
                config_dir,
            });
            di_app
                .bootstrap()
                .await
                .map_err(|e| anyhow::anyhow!("Bootstrap failed: {e}"))?;

            let injector = di_app.injector();
            let session = injector.resolve::<FrontendSession>();

            let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();

            let watch = session
                .watch_ui(root_id.clone(), None, true)
                .await
                .expect("watch_ui failed");

            tracing::info!("watch_ui({root_id}) stream established");

            let app_state: AppState = holon_frontend::cdc::spawn_ui_listener(watch);

            let bridge_mailbox = Arc::clone(&mailbox);
            let mut bridge_state = app_state.clone();
            tokio::spawn(async move {
                while bridge_state.changed().await {
                    let snapshot = bridge_state.snapshot();
                    bridge_mailbox.handle(move |b| b.set(snapshot));
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
            watch(snapshot_binding.clone(), move |snapshot: RenderSnapshot| {
                let (ref expr, ref data) = snapshot;
                render_from_snapshot(expr, data, &session, &rt, &interp)
            })
        },
        env,
    )
}

// ── Config from env vars ────────────────────────────────────────────────────

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

waterui_ffi::export!();
