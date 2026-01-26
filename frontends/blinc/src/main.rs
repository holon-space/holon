mod cdc;
pub mod geometry;
mod render;
mod state;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use blinc_app::prelude::*;
use blinc_app::windowed::WindowedApp;
use blinc_theme::{ColorScheme, ColorToken, ThemeBundle, ThemeState};

use blinc_core::State;
use holon_frontend::theme::ThemeRegistry;
use holon_frontend::{FrontendConfig, FrontendSession};

use render::context::RenderContext;
use state::AppState;

fn main() -> Result<()> {
    #[cfg(feature = "chrome-trace")]
    let (_chrome_trace_guard, _chrome_trace_layer_set) = {
        use tracing_subscriber::layer::SubscriberExt;
        let (chrome_layer, guard) = holon_frontend::memory_monitor::chrome_trace::layer();
        let subscriber = tracing_subscriber::Registry::default()
            .with(chrome_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(true),
            )
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "holon_blinc=info,holon=info".into()),
            );
        tracing::subscriber::set_global_default(subscriber)
            .expect("Failed to set tracing subscriber");
        (guard, true)
    };

    #[cfg(not(feature = "chrome-trace"))]
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "holon_blinc=info,holon=info".into()),
        )
        .init();

    let config = parse_args()?;

    eprintln!(
        "Blinc frontend: db={}, orgmode={:?}, loro={}",
        config
            .db_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or("(temp)".into()),
        config.orgmode_root,
        config.loro_enabled
    );

    let runtime = tokio::runtime::Runtime::new()?;

    let (session, app_state) = runtime.block_on(async {
        let frontend_config = build_frontend_config(&config);
        tracing::info!("Starting Blinc frontend...");
        let session = Arc::new(FrontendSession::new(frontend_config).await?);

        // Start MCP server on port 8520 (same as Flutter)
        {
            let mcp_engine = session.engine().clone();
            let mcp_port: u16 = std::env::var("MCP_SERVER_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8520);
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
        let app_state = AppState::new(holon_api::widget_spec::WidgetSpec::from_rows(vec![]));

        let watch = session.watch_ui(root_id.clone(), None, true).await?;

        tracing::info!("watch_ui({root_id}) stream established");
        let cdc_state = app_state.clone_handle();
        tokio::spawn(cdc::ui_event_listener(watch, cdc_state));
        Ok::<_, anyhow::Error>((session, app_state))
    })?;

    // Keep the runtime alive in a background thread
    let rt_handle = runtime.handle().clone();
    let _runtime_guard = std::thread::spawn(move || {
        runtime.block_on(std::future::pending::<()>());
    });

    init_theme(&session);

    #[cfg(feature = "hot-reload")]
    {
        let patch_state = app_state.clone_handle();
        subsecond::register_handler(Arc::new(move || {
            patch_state.mark_dirty();
        }));
    }

    let block_cache =
        holon_frontend::BlockRenderCache::new(Arc::clone(&session), rt_handle.clone());

    Ok(WindowedApp::run(WindowConfig::default(), move |ctx| {
        let sidebar_open: State<bool> = ctx.use_state_keyed("sidebar_open", || true);
        let right_sidebar_open: State<bool> = ctx.use_state_keyed("right_sidebar_open", || true);
        let left_sidebar_block_id: State<Option<String>> =
            ctx.use_state_keyed("left_sidebar_block_id", || None);

        let widget_spec = app_state.widget_spec();
        let mut render_ctx = render::context::new_render_context(
            Arc::clone(&session),
            rt_handle.clone(),
            block_cache.clone(),
        );
        render_ctx.is_screen_layout = true;
        render_ctx.ext.sidebar_open = Some(sidebar_open.clone());
        render_ctx.ext.right_sidebar_open = Some(right_sidebar_open.clone());
        render_ctx.ext.left_sidebar_block_id = Some(left_sidebar_block_id.clone());

        let root = {
            #[cfg(feature = "hot-reload")]
            {
                subsecond::call(|| render_root(&widget_spec, &render_ctx))
            }
            #[cfg(not(feature = "hot-reload"))]
            {
                render_root(&widget_spec, &render_ctx)
            }
        };

        let theme = ThemeState::get();
        let title_bar = build_title_bar(
            &sidebar_open,
            &left_sidebar_block_id,
            Arc::clone(&session),
            theme,
        );

        div()
            .w(ctx.width)
            .h(ctx.height)
            .bg(theme.color(ColorToken::Background))
            .flex_col()
            .child(title_bar)
            .child(div().flex_1().overflow_clip().child(root))
    })?)
}

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
                eprintln!("Usage: holon-blinc [OPTIONS] [DATABASE_PATH]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --orgmode-root PATH  OrgMode root directory");
                eprintln!("  --loro               Enable Loro CRDT layer");
                eprintln!("  --help, -h           Show this help message");
                eprintln!();
                eprintln!("Environment variables:");
                eprintln!("  HOLON_DB_PATH          Database file path");
                eprintln!("  HOLON_ORGMODE_ROOT     OrgMode root directory");
                eprintln!("  HOLON_LORO_ENABLED     Enable Loro CRDT (1/true)");
                eprintln!();
                eprintln!("Examples:");
                eprintln!("  holon-blinc /path/to/db.db --orgmode-root /path/to/org/files");
                eprintln!("  HOLON_DB_PATH=./holon.db HOLON_ORGMODE_ROOT=./pkm holon-blinc");
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
    let tui_widgets: std::collections::HashSet<String> = render::builders::builder_names()
        .iter()
        .map(|s| String::from(*s))
        .collect();
    let ui_info = holon_api::UiInfo {
        available_widgets: tui_widgets,
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

/// Initialize the Blinc theme from the shared ThemeRegistry + UiSettings.
fn init_theme(session: &FrontendSession) {
    let registry = ThemeRegistry::load(user_themes_dir().as_deref());
    let ui = session.ui_settings();
    let theme_name = ui.theme.as_deref().unwrap_or("holonDark");

    if let Some(def) = registry.get(theme_name) {
        let scheme = if def.is_dark {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        };
        let color_tokens = def.colors.to_blinc_color_tokens();
        let theme = HolonTheme::new(theme_name, scheme, color_tokens);
        let bundle = ThemeBundle::new(theme_name, theme.clone(), theme);
        ThemeState::init(bundle, scheme);
    } else {
        tracing::warn!("Theme '{theme_name}' not found, using platform default");
        ThemeState::init_default();
    }
}

fn user_themes_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".config/holon/themes"))
}

/// Minimal blinc Theme impl backed by our shared ThemeColors.
#[derive(Clone, Debug)]
struct HolonTheme {
    name: String,
    scheme: ColorScheme,
    colors: blinc_theme::ColorTokens,
}

impl HolonTheme {
    fn new(name: &str, scheme: ColorScheme, colors: blinc_theme::ColorTokens) -> Self {
        Self {
            name: name.to_string(),
            scheme,
            colors,
        }
    }
}

impl blinc_theme::Theme for HolonTheme {
    fn name(&self) -> &str {
        &self.name
    }
    fn color_scheme(&self) -> ColorScheme {
        self.scheme
    }
    fn colors(&self) -> &blinc_theme::ColorTokens {
        &self.colors
    }
    fn typography(&self) -> &blinc_theme::TypographyTokens {
        static DEFAULT: std::sync::LazyLock<blinc_theme::TypographyTokens> =
            std::sync::LazyLock::new(Default::default);
        &DEFAULT
    }
    fn spacing(&self) -> &blinc_theme::SpacingTokens {
        static DEFAULT: std::sync::LazyLock<blinc_theme::SpacingTokens> =
            std::sync::LazyLock::new(Default::default);
        &DEFAULT
    }
    fn radii(&self) -> &blinc_theme::RadiusTokens {
        static DEFAULT: std::sync::LazyLock<blinc_theme::RadiusTokens> =
            std::sync::LazyLock::new(Default::default);
        &DEFAULT
    }
    fn shadows(&self) -> &blinc_theme::ShadowTokens {
        static DEFAULT: std::sync::LazyLock<blinc_theme::ShadowTokens> =
            std::sync::LazyLock::new(Default::default);
        &DEFAULT
    }
    fn animations(&self) -> &blinc_theme::AnimationTokens {
        static DEFAULT: std::sync::LazyLock<blinc_theme::AnimationTokens> =
            std::sync::LazyLock::new(Default::default);
        &DEFAULT
    }
}

fn render_root(widget_spec: &holon_api::widget_spec::WidgetSpec, ctx: &RenderContext) -> Div {
    let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
    let render_ctx = ctx.with_data_rows(data_rows);
    render::interpreter::interpret(&widget_spec.render_expr, &render_ctx)
}

const TITLE_BAR_HEIGHT: f32 = 32.0;

const HAMBURGER_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="4" x2="20" y1="12" y2="12"/><line x1="4" x2="20" y1="6" y2="6"/><line x1="4" x2="20" y1="18" y2="18"/></svg>"#;

const HAMBURGER_OPEN_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="4" x2="20" y1="12" y2="12"/><line x1="4" x2="14" y1="6" y2="6"/><line x1="4" x2="14" y1="18" y2="18"/></svg>"#;

fn build_title_bar(
    sidebar_open: &State<bool>,
    sidebar_block_id: &State<Option<String>>,
    session: Arc<FrontendSession>,
    theme: &ThemeState,
) -> Div {
    let is_open = sidebar_open.get();
    let icon = if is_open {
        HAMBURGER_OPEN_SVG
    } else {
        HAMBURGER_SVG
    };

    let sidebar_state = sidebar_open.clone();
    let block_id = sidebar_block_id.get();

    div()
        .flex_row()
        .items_center()
        .w_full()
        .h(TITLE_BAR_HEIGHT)
        .bg(theme.color(ColorToken::Background))
        .border_bottom(1.0, theme.color(ColorToken::Border))
        .child(
            div()
                .flex_row()
                .items_center()
                .justify_center()
                .w(TITLE_BAR_HEIGHT)
                .h(TITLE_BAR_HEIGHT)
                .cursor(blinc_layout::element::CursorStyle::Pointer)
                .child(
                    svg(icon)
                        .size(18.0, 18.0)
                        .color(theme.color(ColorToken::TextSecondary)),
                )
                .on_click(move |_| {
                    let new_open = !sidebar_state.get();
                    sidebar_state.update_rebuild(|_| new_open);
                    if let Some(ref bid) = block_id {
                        session.set_widget_open(bid, new_open);
                    }
                }),
        )
        .child(
            text("Holon")
                .size(14.0)
                .color(theme.color(ColorToken::TextPrimary)),
        )
}
