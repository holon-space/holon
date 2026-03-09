pub mod cdc;
pub mod geometry;
#[cfg(feature = "mobile")]
pub mod mobile;
mod render;
pub mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui::*;
use holon_frontend::theme::{ThemeColors, ThemeRegistry};
use holon_frontend::{FrontendSession, RenderContext};

use geometry::BoundsRegistry;
use state::AppState;

pub struct HolonApp {
    pub session: Arc<FrontendSession>,
    pub app_state: AppState,
    pub rt_handle: tokio::runtime::Handle,
    pub block_cache: holon_frontend::BlockRenderCache,
    pub bounds_registry: BoundsRegistry,
    pub theme: ThemeColors,
    pub show_settings: Arc<AtomicBool>,
}

impl Render for HolonApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.bounds_registry.clear();
        let widget_spec = self.app_state.widget_spec();
        let widget_states = Arc::new(self.session.ui_settings().widgets);
        let mut render_ctx = RenderContext {
            data_rows: Vec::new(),
            operations: Vec::new(),
            session: Arc::clone(&self.session),
            runtime_handle: self.rt_handle.clone(),
            depth: 0,
            query_depth: 0,
            is_screen_layout: false,
            ext: self.bounds_registry.clone(),
            block_cache: self.block_cache.clone(),
            widget_states,
        };
        render_ctx.is_screen_layout = true;

        let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
        let render_ctx = render_ctx.with_data_rows(data_rows);
        let interp = render::builders::create_interpreter();
        let root = interp.interpret(&widget_spec.render_expr, &render_ctx);

        let bg = geometry::rgba8_to_gpui(self.theme.background);
        let text = geometry::rgba8_to_gpui(self.theme.text_primary);

        let session_for_toggle = Arc::clone(&self.session);
        let registry_for_toggle = self.bounds_registry.clone();
        let border_color = geometry::rgba8_to_gpui(self.theme.border);

        let settings_overlay = if self.show_settings.load(Ordering::Relaxed) {
            let (render_expr, rows) = self.session.preferences_render_data();
            let settings_ctx = render_ctx.with_data_rows(rows);
            let settings_ui = interp.interpret(&render_expr, &settings_ctx);

            let overlay_bg = gpui::rgba(0x00000088);
            let panel_bg = bg;

            Some(
                div()
                    .id("settings-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(overlay_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("settings-panel")
                            .w(px(520.0))
                            .max_h(px(600.0))
                            .overflow_y_scroll()
                            .bg(panel_bg)
                            .rounded(px(12.0))
                            .border_1()
                            .border_color(border_color)
                            .p_4()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .pb_2()
                                    .border_b_1()
                                    .border_color(border_color)
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child("Settings"),
                                    )
                                    .child({
                                        let flag = Arc::clone(&self.show_settings);
                                        div()
                                            .id("settings-close")
                                            .cursor_pointer()
                                            .text_sm()
                                            .child("✕")
                                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                                flag.store(false, Ordering::Relaxed);
                                            })
                                    }),
                            )
                            .child(settings_ui),
                    ),
            )
        } else {
            None
        };

        let mut page = div()
            .size_full()
            .bg(bg)
            .text_color(text)
            .flex_col()
            .child(
                div()
                    .id("title-bar")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(36.0))
                    .px_3()
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .id("sidebar-toggle")
                                    .cursor_pointer()
                                    .text_sm()
                                    .child("☰")
                                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                        if let Some(bid) = registry_for_toggle.sidebar_block_id() {
                                            let ws = session_for_toggle.widget_state(&bid);
                                            session_for_toggle.set_widget_open(&bid, !ws.open);
                                        }
                                    }),
                            )
                            .child(div().text_sm().child("Holon")),
                    )
                    .child({
                        let flag = Arc::clone(&self.show_settings);
                        div()
                            .id("settings-gear")
                            .cursor_pointer()
                            .text_sm()
                            .child("⚙")
                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                let prev = flag.load(Ordering::Relaxed);
                                flag.store(!prev, Ordering::Relaxed);
                            })
                    }),
            )
            .child(div().flex_1().overflow_hidden().child(root));

        if let Some(overlay) = settings_overlay {
            page = page.child(overlay);
        }

        page
    }
}

/// Launch a Holon window, creating a new `BoundsRegistry` from the session's theme.
pub fn launch_holon_window(
    session: Arc<FrontendSession>,
    app_state: AppState,
    watch_handle: holon_api::streaming::WatchHandle,
    rt_handle: tokio::runtime::Handle,
    cx: &mut App,
) -> BoundsRegistry {
    let theme = load_theme_colors(&session);
    let bounds_registry = BoundsRegistry::new(theme);
    launch_holon_window_with_registry(
        session,
        app_state,
        watch_handle,
        rt_handle,
        bounds_registry.clone(),
        cx,
    );
    bounds_registry
}

/// Launch a Holon window using a pre-created `BoundsRegistry`.
///
/// Use this when you need to share the same registry across threads (e.g. PBT tests
/// where the GeometryDriver reads bounds recorded during GPUI render passes).
pub fn launch_holon_window_with_registry(
    session: Arc<FrontendSession>,
    app_state: AppState,
    watch_handle: holon_api::streaming::WatchHandle,
    rt_handle: tokio::runtime::Handle,
    bounds_registry: BoundsRegistry,
    cx: &mut App,
) {
    let theme = bounds_registry.theme().clone();
    let session_clone = Arc::clone(&session);
    let handle_clone = rt_handle.clone();
    let cdc_state = app_state.clone_handle();

    cx.spawn(async move |cx| {
        let view_entity: Arc<std::sync::OnceLock<Entity<HolonApp>>> =
            Arc::new(std::sync::OnceLock::new());
        let view_slot = view_entity.clone();

        let window_options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Holon".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        tracing::info!("Opening GPUI window...");
        cx.open_window(window_options, |window, cx| {
            let block_cache = holon_frontend::BlockRenderCache::new(
                Arc::clone(&session_clone),
                handle_clone.clone(),
            );
            let view = cx.new(|_cx| HolonApp {
                session: session_clone,
                app_state,
                rt_handle: handle_clone,
                block_cache,
                bounds_registry,
                theme,
                show_settings: Arc::new(AtomicBool::new(false)),
            });
            view_slot.set(view.clone()).ok();
            view
        })?;
        tracing::info!("GPUI window opened successfully");

        let view = view_entity.get().unwrap().clone();

        // Periodic refresh for matview population after startup
        let refresh_view = view.clone();
        cx.spawn({
            let cx_handle = cx.clone();
            async move |_| {
                for _ in 0..10 {
                    smol::Timer::after(std::time::Duration::from_secs(2)).await;
                    let _ = cx_handle.update(|cx| refresh_view.update(cx, |_, cx| cx.notify()));
                }
            }
        })
        .detach();

        let mut watch_handle = watch_handle;
        while let Some(event) = watch_handle.recv().await {
            if cdc::apply_event(&cdc_state, event) {
                let _ = cx.update(|cx| view.update(cx, |_, cx| cx.notify()));
            }
        }

        Ok::<_, anyhow::Error>(())
    })
    .detach();
}

/// Return the set of widget names this GPUI frontend supports.
pub fn render_supported_widgets() -> std::collections::HashSet<String> {
    render::builders::create_interpreter().supported_widgets()
}

pub fn load_theme_colors(session: &FrontendSession) -> ThemeColors {
    let user_dir = std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".config/holon/themes"));
    let registry = ThemeRegistry::load(user_dir.as_deref());
    let ui = session.ui_settings();
    let name = ui.theme.as_deref().unwrap_or("holonDark");
    registry
        .get(name)
        .map(|def| def.colors.clone())
        .unwrap_or_else(|| {
            tracing::warn!("Theme '{name}' not found, using holonDark");
            registry
                .get("holonDark")
                .expect("holonDark builtin missing")
                .colors
                .clone()
        })
}
