pub mod entity_registry;
pub mod geometry;
#[cfg(feature = "mobile")]
pub mod mobile;
mod render;
pub mod state;
pub mod views;

use std::sync::Arc;

use gpui::*;
use holon_frontend::theme::ThemeRegistry;
use holon_frontend::view_model::ViewModel;
use holon_frontend::{FrontendSession, RenderContext, RenderPipeline};

use geometry::BoundsRegistry;
use render::builders::GpuiRenderContext;
use state::AppState;

// ── AppModel: Entity-based reactive state ──────────────────────────────────

/// Reactive model that replaces the old `AtomicBool` dirty flags.
///
/// Holds pre-computed display state (ViewModel + RenderContext). Updated by
/// CDC loops via `Entity::update()` + `cx.notify()`. `HolonApp` observes
/// this model and re-renders when it changes.
struct AppModel {
    session: Arc<FrontendSession>,
    app_state: AppState,
    rt_handle: tokio::runtime::Handle,
    block_watch: holon_frontend::BlockWatchRegistry,
    view_model: ViewModel,
    shadow_ctx: RenderContext,
    show_settings: bool,
    entity_registry: entity_registry::EntityRegistry,
}

impl AppModel {
    /// Re-read the latest WidgetSpec, re-interpret the shadow tree, and
    /// reconcile Entity instances (create new BlockRefViews, remove stale ones).
    fn rebuild(&mut self, cx: &mut gpui::Context<Self>) {
        let widget_spec = self.app_state.widget_spec();
        let pipeline = Arc::new(RenderPipeline {
            session: Arc::clone(&self.session),
            runtime_handle: self.rt_handle.clone(),
            block_watch: self.block_watch.clone(),
            widget_states: Arc::new(self.session.ui_settings().widgets),
        });
        let shadow_ctx =
            RenderContext::from_pipeline(pipeline.clone()).with_data_rows(widget_spec.data.clone());
        let interp = holon_frontend::create_shadow_interpreter();
        self.view_model = interp.interpret(&widget_spec.render_expr, &shadow_ctx);
        self.shadow_ctx = shadow_ctx;

        self.entity_registry.set_pipeline(pipeline);
        self.entity_registry.reconcile_blocks(&self.view_model, cx);
        self.entity_registry
            .reconcile_live_queries(&self.view_model, cx);
    }
}

// ── HolonApp: GPUI view ────────────────────────────────────────────────────

pub struct HolonApp {
    pub session: Arc<FrontendSession>,
    pub rt_handle: tokio::runtime::Handle,
    app_model: Entity<AppModel>,
    pub bounds_registry: BoundsRegistry,
    /// Top safe area inset in logical pixels (status bar on mobile, 0 on desktop).
    pub safe_area_top: f32,
}

impl Render for HolonApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.bounds_registry.clear();
        self.bounds_registry.sync_theme(_cx);

        // Clone out of the model to release the immutable borrow on cx,
        // which ensure_input_states needs mutably.
        let (view_model, shadow_ctx, show_settings) = {
            let model = self.app_model.read(_cx);
            (
                model.view_model.clone(),
                model.shadow_ctx.clone(),
                model.show_settings,
            )
        };

        // Reconcile editor entities (requires Window for InputState creation).
        {
            let session = self.session.clone();
            let rt_handle = self.rt_handle.clone();
            let vm = view_model.clone();
            self.app_model.update(_cx, |m, cx| {
                m.entity_registry
                    .reconcile_editors(&vm, &session, &rt_handle, _window, cx);
            });
        }

        let gpui_ctx = GpuiRenderContext {
            ctx: shadow_ctx,
            bounds_registry: self.bounds_registry.clone(),
        };
        let root = render::builders::render(&view_model, &gpui_ctx);

        let theme = self.bounds_registry.theme();
        let glass = self.session.ui_settings().glass_background;
        let bg = if glass {
            gpui::Hsla {
                a: 0.7,
                ..theme.background
            }
        } else {
            theme.background
        };
        let text = theme.foreground;

        let drawer_ids = view_model.collect_drawer_ids();
        let left_drawer_id = drawer_ids.first().cloned();
        let right_drawer_id = if drawer_ids.len() > 1 {
            drawer_ids.last().cloned()
        } else {
            None
        };
        let border_color = theme.border;

        let settings_overlay = if show_settings {
            let (render_expr, rows) = self.session.preferences_render_data();
            let settings_ctx = gpui_ctx.ctx.with_data_rows(rows);
            let settings_interp = holon_frontend::create_shadow_interpreter();
            let settings_view_model = settings_interp.interpret(&render_expr, &settings_ctx);
            let settings_gpui_ctx = GpuiRenderContext {
                ctx: settings_ctx,
                bounds_registry: self.bounds_registry.clone(),
            };
            let settings_ui = render::builders::render(&settings_view_model, &settings_gpui_ctx);

            let overlay_bg = gpui::rgba(0x00000088);
            let panel_bg = bg;

            let model = self.app_model.clone();
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
                            .w(px(640.0))
                            .max_h(px(720.0))
                            .overflow_y_scroll()
                            .bg(panel_bg)
                            .rounded(px(12.0))
                            .border_1()
                            .border_color(border_color)
                            .shadow_lg()
                            .p(px(24.0))
                            .flex_col()
                            .gap_1()
                            .on_mouse_down_out({
                                let model = model.clone();
                                move |_, _window, cx| {
                                    model.update(cx, |m, cx| {
                                        m.show_settings = false;
                                        cx.notify();
                                    });
                                }
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .pb_3()
                                    .mb_2()
                                    .border_b_1()
                                    .border_color(border_color)
                                    .child(
                                        div()
                                            .text_size(px(18.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child("Settings"),
                                    )
                                    .child({
                                        let model = model.clone();
                                        div()
                                            .id("settings-close")
                                            .cursor_pointer()
                                            .px_2()
                                            .py_1()
                                            .rounded(px(4.0))
                                            .hover(|s| s.bg(gpui::rgba(0xffffff18)))
                                            .child("✕")
                                            .on_click(move |_, _, cx| {
                                                model.update(cx, |m, cx| {
                                                    m.show_settings = false;
                                                    cx.notify();
                                                });
                                            })
                                    }),
                            )
                            .child(settings_ui),
                    ),
            )
        } else {
            None
        };

        // On macOS the native titlebar is transparent — we render into it.
        // Left padding leaves room for the traffic-light buttons.
        let traffic_light_pad = if cfg!(target_os = "macos") && !cfg!(feature = "mobile") {
            px(80.0)
        } else {
            px(12.0)
        };

        let left_model = self.app_model.clone();
        let right_model = self.app_model.clone();
        let settings_model = self.app_model.clone();

        let title_bar = div()
            .id("title-bar")
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(38.0))
            .pl(traffic_light_pad)
            .pr(px(16.0))
            .border_b_1()
            .border_color(border_color)
            .on_mouse_down(MouseButton::Left, |ev, window, _cx| {
                if ev.click_count == 2 {
                    window.zoom_window();
                }
            })
            .on_mouse_move(|ev, window, _cx| {
                if ev.dragging() {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .id("sidebar-toggle")
                            .cursor_pointer()
                            .text_size(px(15.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(gpui::rgba(0x00000010)))
                            .child("☰")
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                if let Some(ref bid) = left_drawer_id {
                                    left_model.update(cx, |m, cx| {
                                        let ws = m.session.widget_state(bid);
                                        m.session.set_widget_open(bid, !ws.open);
                                        m.rebuild(cx);
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground)
                            .child("Holon"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .id("right-sidebar-toggle")
                            .cursor_pointer()
                            .text_size(px(15.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(gpui::rgba(0x00000010)))
                            .child("◧")
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                if let Some(ref bid) = right_drawer_id {
                                    right_model.update(cx, |m, cx| {
                                        let ws = m.session.widget_state(bid);
                                        m.session.set_widget_open(bid, !ws.open);
                                        m.rebuild(cx);
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("settings-gear")
                            .cursor_pointer()
                            .text_size(px(15.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(gpui::rgba(0x00000010)))
                            .child("⚙")
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                settings_model.update(cx, |m, cx| {
                                    m.show_settings = !m.show_settings;
                                    cx.notify();
                                });
                            }),
                    ),
            );

        let mut page = div()
            .size_full()
            .bg(bg)
            .text_color(text)
            .flex_col()
            .pt(px(self.safe_area_top))
            .child(title_bar)
            .child(
                div()
                    .size_full()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(root),
            );

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
    rt_handle: tokio::runtime::Handle,
    cx: &mut App,
) -> BoundsRegistry {
    let bounds_registry = BoundsRegistry::new();
    launch_holon_window_with_registry(session, app_state, rt_handle, bounds_registry.clone(), cx);
    bounds_registry
}

/// Launch a Holon window using a pre-created `BoundsRegistry`.
///
/// Use this when you need to share the same registry across threads (e.g. PBT tests
/// where the GeometryDriver reads bounds recorded during GPUI render passes).
pub fn launch_holon_window_with_registry(
    session: Arc<FrontendSession>,
    app_state: AppState,
    rt_handle: tokio::runtime::Handle,
    bounds_registry: BoundsRegistry,
    cx: &mut App,
) {
    gpui_component::init(cx);

    // Sync gpui-component theme with Holon's active theme
    let is_dark = is_theme_dark(&session);
    let mode = if is_dark {
        gpui_component::theme::ThemeMode::Dark
    } else {
        gpui_component::theme::ThemeMode::Light
    };
    gpui_component::theme::Theme::change(mode, None, cx);

    let session_clone = Arc::clone(&session);
    let handle_clone = rt_handle.clone();
    let mut cdc_state = app_state.clone();

    cx.spawn(async move |cx| {
        let view_entity: Arc<std::sync::OnceLock<Entity<HolonApp>>> =
            Arc::new(std::sync::OnceLock::new());
        let model_entity: Arc<std::sync::OnceLock<Entity<AppModel>>> =
            Arc::new(std::sync::OnceLock::new());
        let view_slot = view_entity.clone();
        let model_slot = model_entity.clone();

        let glass = session_clone.ui_settings().glass_background;
        let window_options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Holon".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(9.0), px(9.0))),
            }),
            window_background: if glass {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Opaque
            },
            ..Default::default()
        };
        eprintln!("[GPUI] Opening window...");
        let window_handle = cx.open_window(window_options, |window, cx| {
            let block_watch = holon_frontend::BlockWatchRegistry::new(
                Arc::clone(&session_clone),
                handle_clone.clone(),
            );

            // Build initial display state.
            let initial_spec = app_state.widget_spec();
            let pipeline = Arc::new(RenderPipeline {
                session: Arc::clone(&session_clone),
                runtime_handle: handle_clone.clone(),
                block_watch: block_watch.clone(),
                widget_states: Arc::new(session_clone.ui_settings().widgets),
            });
            let shadow_ctx =
                RenderContext::from_pipeline(pipeline).with_data_rows(initial_spec.data.clone());
            let interp = holon_frontend::create_shadow_interpreter();
            let view_model = interp.interpret(&initial_spec.render_expr, &shadow_ctx);

            let er_pipeline = Arc::new(RenderPipeline {
                session: Arc::clone(&session_clone),
                runtime_handle: handle_clone.clone(),
                block_watch: block_watch.clone(),
                widget_states: Arc::new(session_clone.ui_settings().widgets),
            });
            let er = entity_registry::EntityRegistry::new(er_pipeline, bounds_registry.clone());

            let app_model = cx.new(|_| AppModel {
                session: Arc::clone(&session_clone),
                app_state,
                rt_handle: handle_clone.clone(),
                block_watch,
                view_model,
                shadow_ctx,
                show_settings: false,
                entity_registry: er,
            });
            model_slot.set(app_model.clone()).ok();

            let view = cx.new(|cx| {
                cx.observe(&app_model, |_this, _model, cx| cx.notify())
                    .detach();
                HolonApp {
                    session: session_clone,
                    rt_handle: handle_clone,
                    app_model,
                    bounds_registry,
                    safe_area_top: 0.0,
                }
            });
            view_slot.set(view.clone()).ok();
            let any_view: AnyView = view.into();
            cx.new(|cx| gpui_component::Root::new(any_view, window, cx))
        })?;
        eprintln!("[GPUI] Window opened, starting CDC listener...");

        let app_model = model_entity.get().unwrap().clone();

        // Read BlockWatch subscription + tokio handle from model.
        let (block_watch_rx, tokio_handle) = cx.update(|cx| {
            let m = app_model.read(cx);
            (m.block_watch.subscribe(), m.rt_handle.clone())
        });

        let wh: AnyWindowHandle = window_handle.into();

        // Wire BlockWatchRegistry → GPUI via bounded channel (replaces 32ms poll).
        // The bounded(1) channel naturally coalesces: try_send on a full buffer
        // drops the duplicate, so rapid changes collapse into a single rebuild.
        let (notify_tx, notify_rx) = smol::channel::bounded::<()>(1);
        {
            let mut rx = block_watch_rx;
            tokio_handle.spawn(async move {
                loop {
                    if rx.recv().await.is_err() {
                        break;
                    }
                    while rx.try_recv().is_ok() {}
                    let _ = notify_tx.try_send(());
                }
            });
        }
        {
            let app_model = app_model.clone();
            cx.spawn({
                async move |cx| {
                    while notify_rx.recv().await.is_ok() {
                        let _ = cx.update_window(wh, |_, _window, cx| {
                            app_model.update(cx, |m, cx| {
                                m.rebuild(cx);
                                cx.notify();
                            });
                        });
                    }
                }
            })
            .detach();
        }

        // Reactive CDC loop: wait for structural changes, rebuild model.
        eprintln!("[GPUI] Entering reactive CDC loop...");
        while cdc_state.changed().await {
            let _ = cx.update_window(wh, |_, _window, cx| {
                app_model.update(cx, |m, cx| {
                    m.rebuild(cx);
                    cx.notify();
                });
            });
        }
        eprintln!("[GPUI] CDC stream ended");

        Ok::<_, anyhow::Error>(())
    })
    .detach();
}

/// Return the set of widget names this GPUI frontend supports.
pub fn render_supported_widgets() -> std::collections::HashSet<String> {
    render::builders::builder_names()
        .iter()
        .map(|s| s.to_string())
        .collect()
}

pub fn is_theme_dark(session: &FrontendSession) -> bool {
    load_theme_def(session).is_dark
}

fn load_theme_def(session: &FrontendSession) -> holon_frontend::theme::ThemeDef {
    let user_dir = std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".config/holon/themes"));
    let registry = ThemeRegistry::load(user_dir.as_deref());
    let ui = session.ui_settings();
    let name = ui.theme.as_deref().unwrap_or("holonDark");
    registry.get(name).cloned().unwrap_or_else(|| {
        tracing::warn!("Theme '{name}' not found, using holonDark");
        registry
            .get("holonDark")
            .expect("holonDark builtin missing")
            .clone()
    })
}
