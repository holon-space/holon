pub mod di;
pub mod entity_view_registry;
pub mod geometry;
#[cfg(debug_assertions)]
pub mod inspector;
#[cfg(feature = "mobile")]
pub mod mobile;
pub mod navigation_state;
pub mod reactive_vm_poc;
pub mod render;
pub mod share_ui;

pub mod user_driver;
pub mod views;

use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{Enter, MoveDown, MoveUp};
use holon_api::EntityName;
use holon_frontend::input::{InputAction, WidgetInput};
use holon_frontend::navigation::{Boundary, CursorHint, NavDirection};
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::theme::ThemeRegistry;
use holon_frontend::view_model::ViewModel;
use holon_frontend::{FrontendSession, ReactiveViewModel, RenderContext};

use entity_view_registry::{FocusRegistry, LocalEntityScope};
use geometry::BoundsRegistry;
use navigation_state::NavigationState;
use render::builders::GpuiRenderContext;

// Re-export the shared interpret function for DI wiring
pub use holon_frontend::reactive::make_interpret_fn;

// ── AppModel: Entity-based reactive state ──────────────────────────────────

/// Reactive model backed by `ReactiveEngine`.
///
/// The root layout is watched via `engine.watch(root_uri)`. Sub-blocks
/// (LiveBlockView, LiveQueryView) each have their own independent streams.
/// `rebuild()` is only called for the root — sub-blocks update independently.
struct AppModel {
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    focus: FocusRegistry,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    /// The reactive root tree. LiveBlock nodes are placeholders.
    /// Wrapped in Arc so it can be shared with the InputRouter.
    root_vm: Arc<ReactiveViewModel>,
    /// Static snapshot for rendering (produced from root_vm on each update).
    view_model: ViewModel,
    shadow_ctx: RenderContext,
    show_settings: bool,
    show_widget_gallery: bool,
    /// Per-window share/accept UI state (modals, toasts, quarantines).
    share_ui: Entity<share_ui::ShareUiState>,
    /// Root-level ReactiveShell entities (sidebars, main panel), keyed by block_id.
    root_live_blocks: std::collections::HashMap<String, Entity<views::ReactiveShell>>,
    /// Buffered cursor position from the cursor signal, consumed after root layout
    /// reconciliation when the target editor entity exists.
    pending_cursor: Option<(String, i64)>,
    /// Handle to the root layout's ReactiveView, extracted from `root_vm` each
    /// time it's rebuilt. Used by the viewport observer to push container-query
    /// space updates into the root on window resize / keyboard toggle without
    /// triggering a full tree rebuild. Present iff the current root is a
    /// Reactive variant (i.e. a streaming container like `columns`).
    root_view: Option<Arc<holon_frontend::ReactiveView>>,
}

/// Extract the root `ReactiveView` from a `ReactiveViewModel`, if its top
/// node is a `Reactive` variant. Used to plumb viewport updates into the
/// root's `space` Mutable.
fn root_reactive_view(rvm: &ReactiveViewModel) -> Option<Arc<holon_frontend::ReactiveView>> {
    rvm.collection.clone()
}

/// Convert GPUI window dimensions into the frontend's `ViewportInfo`.
/// `size` is logical pixels; `scale` is the device pixel ratio.
fn viewport_info_from_window(
    size: gpui::Size<gpui::Pixels>,
    scale: f32,
) -> holon_frontend::reactive::ViewportInfo {
    holon_frontend::reactive::ViewportInfo {
        width_px: f32::from(size.width),
        height_px: f32::from(size.height),
        scale_factor: scale,
    }
}

/// Convert a `ViewportInfo` to the `AvailableSpace` the root ReactiveView
/// uses to kick off its container-query cascade.
fn viewport_to_available_space(
    info: holon_frontend::reactive::ViewportInfo,
) -> holon_frontend::AvailableSpace {
    holon_frontend::AvailableSpace {
        width_px: info.width_px,
        height_px: info.height_px,
        width_physical_px: info.width_px * info.scale_factor,
        height_physical_px: info.height_px * info.scale_factor,
        scale_factor: info.scale_factor,
    }
}

impl AppModel {
    /// Re-read the root layout's current state and reconcile Entity instances.
    fn rebuild(&mut self, cx: &mut gpui::Context<Self>) {
        let root_uri = holon_api::root_layout_block_uri();
        self.root_vm = Arc::new(self.engine.snapshot_reactive(&root_uri));
        self.view_model = self
            .root_vm
            .snapshot_resolved(&|bid| self.engine.snapshot(bid));

        self.shadow_ctx = RenderContext::default();

        self.root_view = root_reactive_view(&self.root_vm);
        // Re-seed the root's container-query allocation from the current
        // viewport. On first call this is a no-op (viewport is None); on
        // subsequent rebuilds (e.g. after a data-driven root signal fire)
        // this keeps the new root in sync with the user's current window.
        if let (Some(view), Some(vp)) = (&self.root_view, self.engine.ui_state().viewport()) {
            view.set_space(Some(viewport_to_available_space(vp)));
        }

        self.reconcile_root_live_blocks(cx);

        self.view_model =
            resolved_view_model(&self.root_vm, &self.engine, &self.root_live_blocks, cx);
        self.nav.set_root(self.root_vm.clone(), &self.focus);
    }

    /// Push a fresh viewport into `UiState` and the root ReactiveView.
    ///
    /// This is the single entry point for all viewport-change events:
    /// window resize on desktop, keyboard show/hide on mobile, orientation
    /// change, split-screen. It does NOT trigger a tree rebuild — instead
    /// it pushes new values into reactive signals, and the flat driver's
    /// space-reactive subscription rebuilds only the subtrees whose
    /// computed space actually changed.
    fn apply_viewport(&self, info: holon_frontend::reactive::ViewportInfo) {
        self.engine.ui_state().set_viewport(info);
        if let Some(view) = &self.root_view {
            view.set_space(Some(viewport_to_available_space(info)));
        }
    }

    /// Walk the root reactive tree to find LiveBlock nodes and create/GC their entities.
    fn reconcile_root_live_blocks(&mut self, cx: &mut gpui::Context<Self>) {
        let mut needed = std::collections::HashSet::new();
        collect_root_live_blocks(&self.root_vm, &mut needed);

        for block_id in &needed {
            if !self.root_live_blocks.contains_key(block_id) {
                let uri = holon_api::EntityUri::from_raw(block_id);
                let services: Arc<dyn BuilderServices> = self.engine.clone();
                let live_block = services.watch_live(&uri, services.clone());
                let render_ctx = RenderContext::default();
                let focus = self.focus.clone();
                let nav = self.nav.clone();
                let b = self.bounds_registry.clone();
                let bid = block_id.clone();
                let entity = cx.new(|cx| {
                    views::ReactiveShell::new_for_block(
                        bid, render_ctx, services, live_block, focus, nav, b, cx,
                    )
                });
                self.root_live_blocks.insert(block_id.clone(), entity);
            }
        }

        let stale: Vec<String> = self
            .root_live_blocks
            .keys()
            .filter(|k| !needed.contains(k.as_str()))
            .cloned()
            .collect();
        for k in &stale {
            self.root_live_blocks.remove(k);
        }
    }
}

/// Resolve the root reactive tree into a static ViewModel, bottom-up.
///
/// Each LiveBlock is resolved by reading its LiveBlockView entity's current
/// reactive tree and resolving it recursively. Falls back to get_block_data
/// for blocks whose view hasn't rendered yet.
fn resolved_view_model(
    root_vm: &ReactiveViewModel,
    engine: &ReactiveEngine,
    root_live_blocks: &std::collections::HashMap<String, Entity<views::ReactiveShell>>,
    cx: &App,
) -> ViewModel {
    let services: &dyn BuilderServices = engine;
    root_vm.snapshot_resolved(&|block_id| resolve_block(block_id, root_live_blocks, services, cx))
}

/// Resolve a single live_block by reading its LiveBlockView's reactive tree.
/// Recurses for nested live_blocks via snapshot_resolved.
fn resolve_block(
    block_id: &holon_api::EntityUri,
    root_live_blocks: &std::collections::HashMap<String, Entity<views::ReactiveShell>>,
    services: &dyn BuilderServices,
    cx: &App,
) -> ViewModel {
    let key = block_id.to_string();
    if let Some(entity) = root_live_blocks.get(&key) {
        return entity.read(cx).resolve_snapshot(cx);
    }
    let (render_expr, data_rows) = services.get_block_data(block_id);
    holon_frontend::interpret_pure(&render_expr, &data_rows, services).snapshot()
}

/// Walk a reactive tree to collect all LiveBlock block_ids at any depth.
/// Stops at LiveBlock nodes (they manage their own subtrees).
fn collect_root_live_blocks(node: &ReactiveViewModel, ids: &mut std::collections::HashSet<String>) {
    if node.widget_name().as_deref() == Some("live_block") {
        if let Some(block_id) = node.prop_str("block_id") {
            ids.insert(block_id.to_string());
        }
    } else {
        views::reactive_shell::for_each_child(node, |child| collect_root_live_blocks(child, ids));
    }
}

// ── Modal overlay helpers ──────────────────────────────────────────────────

fn interpret_and_render(
    render_expr: &holon_api::render_types::RenderExpr,
    rows: Vec<std::sync::Arc<std::collections::HashMap<String, holon_api::Value>>>,
    gpui_ctx: &GpuiRenderContext,
) -> impl IntoElement {
    let ctx = gpui_ctx.ctx.with_data_rows(rows);
    let rvm = gpui_ctx.services().interpret(render_expr, &ctx);
    let inner_ctx = gpui_ctx.with_gpui(|window, cx| {
        GpuiRenderContext::new(
            ctx,
            gpui_ctx.services.clone(),
            gpui_ctx.bounds_registry.clone(),
            LocalEntityScope::new(),
            gpui_ctx.focus.clone(),
            window,
            cx,
        )
    });
    render::builders::render(&rvm, &inner_ctx)
}

fn modal_overlay(
    id: &str,
    title: &str,
    content: impl IntoElement,
    panel_bg: Hsla,
    border_color: Hsla,
    model: Entity<AppModel>,
    field: fn(&mut AppModel) -> &mut bool,
) -> Stateful<Div> {
    let overlay_bg = gpui::rgba(0x00000088);
    div()
        .id(SharedString::from(format!("{id}-overlay")))
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
                .id(SharedString::from(format!("{id}-panel")))
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
                            *field(m) = false;
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
                                .child(title.to_string()),
                        )
                        .child({
                            let model = model.clone();
                            div()
                                .id(SharedString::from(format!("{id}-close")))
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded(px(4.0))
                                .hover(|s| s.bg(gpui::rgba(0xffffff18)))
                                .child("✕")
                                .on_click(move |_, _, cx| {
                                    model.update(cx, |m, cx| {
                                        *field(m) = false;
                                        cx.notify();
                                    });
                                })
                        }),
                )
                .child(content),
        )
}

// ── HolonApp: GPUI view ────────────────────────────────────────────────────

pub struct HolonApp {
    pub session: Arc<FrontendSession>,
    pub rt_handle: tokio::runtime::Handle,
    app_model: Entity<AppModel>,
    focus: FocusRegistry,
    nav: NavigationState,
    pub bounds_registry: BoundsRegistry,
    /// Persistent entity cache for the root render (survives across frames).
    entity_cache: entity_view_registry::EntityCache,
    /// Top safe area inset in logical pixels (status bar on mobile, 0 on desktop).
    pub safe_area_top: f32,
    /// Bottom safe area inset in logical pixels (home indicator on mobile, 0 on desktop).
    pub safe_area_bottom: f32,
    /// Entity ID of the currently focused editor. Written on every focus change
    /// (cross-block nav, mouse click). Read by PBT to verify navigation.
    focused_element_id: Arc<std::sync::RwLock<Option<String>>>,
    /// Share/accept UI state. Shared with `AppModel.share_ui` — lives here too
    /// so the render pass can build overlays without a double-read through
    /// `app_model.read(cx).share_ui.read(cx)`.
    pub share_ui: Entity<share_ui::ShareUiState>,
}

impl Render for HolonApp {
    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "frontend.render",
        fields(component = "root")
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.bounds_registry.begin_pass();
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            self.safe_area_top = crate::mobile::safe_area_top_px();
            self.safe_area_bottom = crate::mobile::safe_area_bottom_px();
        }
        let (view_model, shadow_ctx, services, show_settings, show_widget_gallery) = {
            let model = self.app_model.read(cx);
            let services: Arc<dyn BuilderServices> = model.engine.clone();
            (
                model.view_model.clone(),
                model.shadow_ctx.clone(),
                services,
                model.show_settings,
                model.show_widget_gallery,
            )
        };

        // Editor reconciliation is now handled by each LiveBlockView in its render().
        // Shadow index is built in the signal callback.

        let local = {
            let root_refs = self.app_model.read(cx).root_live_blocks.clone();
            let mut l = LocalEntityScope::new().with_cache(self.entity_cache.clone());
            // Pre-populate the entity cache with root live_block entities.
            // This way the live_block builder finds them in get_or_create and
            // doesn't call watch_live + cx.new() during the render pass.
            for (bid, entity) in &root_refs {
                let key = format!("live-block-{bid}");
                l.entity_cache
                    .write()
                    .unwrap()
                    .entry(key)
                    .or_insert_with(|| entity.clone().into_any());
            }
            l
        };
        let gpui_ctx = GpuiRenderContext::new(
            shadow_ctx,
            services.clone(),
            self.bounds_registry.clone(),
            local,
            self.focus.clone(),
            window,
            cx,
        );
        // Render from the reactive tree — dispatches on widget_name()
        let root = {
            let model = self.app_model.read(cx);
            #[cfg(feature = "hot-reload")]
            {
                subsecond::call(|| render::builders::render(&model.root_vm, &gpui_ctx))
            }
            #[cfg(not(feature = "hot-reload"))]
            {
                render::builders::render(&model.root_vm, &gpui_ctx)
            }
        };

        let theme = {
            use gpui_component::theme::ActiveTheme;
            cx.theme().colors
        };
        let glass = self.session.ui_settings().glass_background.unwrap_or(false);
        let bg = if glass {
            gpui::Hsla {
                a: 0.7,
                ..theme.background
            }
        } else {
            theme.background
        };
        let text = theme.foreground;

        // Drawer IDs from static snapshot (simpler than walking reactive tree)
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
            let content = interpret_and_render(&render_expr, rows, &gpui_ctx);
            Some(modal_overlay(
                "settings",
                "Settings",
                content,
                bg,
                border_color,
                self.app_model.clone(),
                |m| &mut m.show_settings,
            ))
        } else {
            None
        };

        let gallery_overlay = if show_widget_gallery {
            let (render_expr, rows) = self.session.widget_gallery_render_data();
            let content = interpret_and_render(&render_expr, rows, &gpui_ctx);
            Some(modal_overlay(
                "gallery",
                "Widget Gallery",
                content,
                bg,
                border_color,
                self.app_model.clone(),
                |m| &mut m.show_widget_gallery,
            ))
        } else {
            None
        };

        let traffic_light_pad = if cfg!(target_os = "macos") && !cfg!(feature = "mobile") {
            px(80.0)
        } else {
            px(12.0)
        };

        let left_model = self.app_model.clone();
        let right_model = self.app_model.clone();
        let settings_model = self.app_model.clone();
        let gallery_model = self.app_model.clone();

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
                    )
                    .child(
                        div()
                            .id("gallery-toggle")
                            .cursor_pointer()
                            .text_size(px(15.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(gpui::rgba(0x00000010)))
                            .child("🎨")
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                gallery_model.update(cx, |m, cx| {
                                    m.show_widget_gallery = !m.show_widget_gallery;
                                    cx.notify();
                                });
                            }),
                    )
                    .child({
                        let share_state = self.share_ui.clone();
                        div()
                            .id("accept-ticket-toggle")
                            .cursor_pointer()
                            .text_size(px(15.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(gpui::rgba(0x00000010)))
                            .child("🔗")
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                share_state.update(cx, |s, cx| {
                                    if s.show_accept_modal {
                                        s.close_accept();
                                    } else {
                                        s.open_accept();
                                    }
                                    cx.emit(share_ui::NotifyShareUi);
                                    cx.notify();
                                });
                            })
                    })
                    .when(cfg!(debug_assertions), |this| {
                        this.child(
                            div()
                                .id("inspector-toggle")
                                .cursor_pointer()
                                .text_size(px(15.0))
                                .px(px(6.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(gpui::rgba(0x00000010)))
                                .child("🔎")
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    #[cfg(debug_assertions)]
                                    window.toggle_inspector(cx);
                                    #[cfg(not(debug_assertions))]
                                    {
                                        let _ = (window, cx);
                                    }
                                }),
                        )
                    }),
            );

        // Wrap content area with cross-block navigation handlers.
        // When an InputState propagates MoveUp/MoveDown (cursor at boundary),
        // these handlers bubble the input through the ShadowDom to find the
        // next block and transfer focus + cursor position.
        let content = {
            let bounds = self.bounds_registry.clone();
            let focus = self.focus.clone();
            let nav = self.nav.clone();
            let (engine, session, rt_handle) = {
                let m = self.app_model.read(cx);
                (m.engine.clone(), m.session.clone(), m.rt_handle.clone())
            };
            div()
                .size_full()
                .flex_1()
                .flex()
                .flex_col()
                .overflow_hidden()
                .on_mouse_up(MouseButton::Left, {
                    let focus = focus.clone();
                    let focused_eid = self.focused_element_id.clone();
                    move |_, window, cx| {
                        if let Some(row_id) = focus.focused_editor_row_id(window, cx) {
                            *focused_eid.write().unwrap() = Some(row_id);
                        }
                    }
                })
                .on_key_down({
                    let nav = nav.clone();
                    let focus = focus.clone();
                    let session = session.clone();
                    let rt_handle = rt_handle.clone();
                    move |event: &gpui::KeyDownEvent, window, cx: &mut App| {
                        let keys = keystroke_to_keys(&event.keystroke);
                        if keys.is_empty() {
                            return;
                        }
                        let Some(focused_id) = focus.focused_editor_row_id(window, cx) else {
                            tracing::debug!("[on_key_down] No focused editor for keys: {keys:?}");
                            return;
                        };
                        let input = WidgetInput::KeyChord { keys: keys.clone() };
                        let action = nav.bubble_input(&focused_id, &input);
                        tracing::debug!(
                            "[on_key_down] keys={keys:?} focused={focused_id} action={action:?}"
                        );
                        if let Some(InputAction::ExecuteOperation {
                            entity_name,
                            operation,
                            entity_id,
                        }) = action
                        {
                            let mut params = std::collections::HashMap::new();
                            params.insert("id".into(), holon_api::Value::String(entity_id));
                            holon_frontend::operations::dispatch_operation(
                                &rt_handle,
                                &session,
                                &EntityName::new(entity_name),
                                operation.name,
                                params,
                            );
                            cx.stop_propagation();
                        }
                    }
                })
                .on_action({
                    let nav = nav.clone();
                    let focus = focus.clone();
                    let engine = engine.clone();
                    let focused_eid = self.focused_element_id.clone();
                    move |_: &MoveUp, window, cx: &mut App| {
                        handle_cross_block_nav(
                            &nav,
                            &focus,
                            &engine,
                            &focused_eid,
                            NavDirection::Up,
                            Boundary::Top,
                            window,
                            cx,
                        );
                    }
                })
                .on_action({
                    let nav = nav.clone();
                    let focus = focus.clone();
                    let engine = engine.clone();
                    let focused_eid = self.focused_element_id.clone();
                    move |_: &MoveDown, window, cx: &mut App| {
                        handle_cross_block_nav(
                            &nav,
                            &focus,
                            &engine,
                            &focused_eid,
                            NavDirection::Down,
                            Boundary::Bottom,
                            window,
                            cx,
                        );
                    }
                })
                .on_action({
                    let nav = nav.clone();
                    let focus = focus.clone();
                    let session = session.clone();
                    let rt_handle = rt_handle.clone();
                    move |_: &Enter, window, cx: &mut App| {
                        let Some(focused_id) = focus.focused_editor_row_id(window, cx) else {
                            return;
                        };
                        let cursor_byte = focus.focused_cursor_byte(&focused_id, cx);

                        let input = WidgetInput::chord(&[holon_api::input_types::Key::Enter]);
                        if let Some(InputAction::ExecuteOperation {
                            entity_name,
                            operation,
                            entity_id,
                        }) = nav.bubble_input(&focused_id, &input)
                        {
                            let mut params = std::collections::HashMap::new();
                            params.insert("id".into(), holon_api::Value::String(entity_id));
                            // Always include cursor position — split_block uses it,
                            // other operations ignore it.
                            params.insert(
                                "position".into(),
                                holon_api::Value::Integer(cursor_byte as i64),
                            );
                            holon_frontend::operations::dispatch_operation(
                                &rt_handle,
                                &session,
                                &EntityName::new(entity_name),
                                operation.name,
                                params,
                            );
                        }
                    }
                })
                .child(root)
        };

        let mut page = div()
            .size_full()
            .bg(bg)
            .text_color(text)
            .flex_col()
            .pt(px(self.safe_area_top))
            .pb(px(self.safe_area_bottom))
            .child(title_bar)
            .child(content);

        if let Some(overlay) = settings_overlay {
            page = page.child(overlay);
        }
        if let Some(overlay) = gallery_overlay {
            page = page.child(overlay);
        }

        // Share/accept/quarantine modals and toast stack. These live in a
        // separate Entity so async tokio events (degraded bus, ticket
        // responses) can update UI without going through the reactive
        // engine, and the main app's subscribe(share_ui) triggers re-render.
        {
            let share_state_entity = self.share_ui.clone();
            let engine = self.app_model.read(cx).engine.clone();
            let overlay_theme = share_ui::OverlayTheme {
                bg,
                border: border_color,
                fg: text,
                muted_fg: theme.muted_foreground,
            };
            let async_cx = cx.to_async();
            let wh = window.window_handle();
            let share_state_read = self.share_ui.read(cx);
            let overlays = share_ui::render_overlays(
                share_state_read,
                share_state_entity,
                self.session.clone(),
                engine,
                self.rt_handle.clone(),
                wh,
                async_cx,
                overlay_theme,
            );
            for ov in overlays {
                page = page.child(ov);
            }
        }

        page.into_any_element()
    }
}

/// Launch a Holon window, creating a new `BoundsRegistry` from the session's theme.
pub fn launch_holon_window(
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    cx: &mut App,
) -> BoundsRegistry {
    let bounds_registry = BoundsRegistry::new();
    let nav = NavigationState::new();
    launch_holon_window_with_registry(session, rt_handle, nav, bounds_registry.clone(), cx);
    bounds_registry
}

/// Launch a Holon window with a pre-created `ReactiveEngine`.
///
/// The engine is shared with the MCP server so `describe_ui` returns real data.
pub fn launch_holon_window_with_engine(
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    debug: Arc<holon_mcp::server::DebugServices>,
    rt_handle: tokio::runtime::Handle,
    cx: &mut App,
) -> BoundsRegistry {
    launch_holon_window_with_engine_and_share(session, engine, debug, None, rt_handle, cx)
}

/// Variant of `launch_holon_window_with_engine` that also wires the
/// subtree-share UI's degraded-bus bridge. `share_backend` is resolved from
/// the DI injector at top-level (see `main.rs`) and is `None` when the
/// `iroh-sync` feature is disabled.
pub fn launch_holon_window_with_engine_and_share(
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    debug: Arc<holon_mcp::server::DebugServices>,
    share_backend: Option<Arc<holon::sync::loro_share_backend::LoroShareBackend>>,
    rt_handle: tokio::runtime::Handle,
    cx: &mut App,
) -> BoundsRegistry {
    let bounds_registry = BoundsRegistry::new();
    let mut nav = NavigationState::with_input_router(debug.input_router.clone());
    nav.set_navigation_debug(debug.navigation_state.clone());
    launch_holon_window_impl(
        session,
        Some(engine),
        Some(debug),
        share_backend,
        rt_handle,
        nav,
        bounds_registry.clone(),
        None,
        cx,
    );
    bounds_registry
}

/// Launch a Holon window with a pre-created `ReactiveEngine` and `BoundsRegistry`.
///
/// Used by the GPUI PBT test: reuses the PBT's DI-resolved ReactiveEngine so all
/// watch_ui tasks and CDC subscriptions share the same tokio runtime.
/// Launch a GPUI window with a custom title (used by PBT to avoid xcap
/// capturing the real Holon window when both are open).
pub fn launch_holon_window_with_title(
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    debug: Option<Arc<holon_mcp::server::DebugServices>>,
    title: &str,
    cx: &mut App,
) {
    launch_holon_window_impl(
        session,
        Some(engine),
        debug,
        None,
        rt_handle,
        nav,
        bounds_registry,
        Some(title.to_string()),
        cx,
    );
}

pub fn launch_holon_window_with_engine_and_registry(
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    cx: &mut App,
) {
    launch_holon_window_impl(
        session,
        Some(engine),
        None,
        None,
        rt_handle,
        nav,
        bounds_registry,
        None,
        cx,
    );
}

/// Launch a Holon window using a pre-created `BoundsRegistry`.
pub fn launch_holon_window_with_registry(
    session: Arc<FrontendSession>,
    rt_handle: tokio::runtime::Handle,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    cx: &mut App,
) {
    launch_holon_window_impl(
        session,
        None,
        None,
        None,
        rt_handle,
        nav,
        bounds_registry,
        None,
        cx,
    );
}

/// Shared implementation for launching a Holon window.
///
/// If `existing_engine` is `Some`, reuses it (shared with MCP server).
/// Otherwise creates a fresh `ReactiveEngine` inside the window callback.
/// `share_backend` is resolved from the DI injector in `main.rs`; pass
/// `None` to skip wiring the degraded-bus bridge (PBT / mobile paths).
fn launch_holon_window_impl(
    session: Arc<FrontendSession>,
    existing_engine: Option<Arc<ReactiveEngine>>,
    debug: Option<Arc<holon_mcp::server::DebugServices>>,
    share_backend: Option<Arc<holon::sync::loro_share_backend::LoroShareBackend>>,
    rt_handle: tokio::runtime::Handle,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    custom_title: Option<String>,
    cx: &mut App,
) {
    gpui_component::init(cx);

    #[cfg(debug_assertions)]
    inspector::init(cx);

    apply_holon_theme(&session, cx);

    let session_clone = Arc::clone(&session);
    let handle_clone = rt_handle.clone();

    let model_entity: Arc<std::sync::OnceLock<Entity<AppModel>>> =
        Arc::new(std::sync::OnceLock::new());
    let model_slot = model_entity.clone();

    let glass = session_clone
        .ui_settings()
        .glass_background
        .unwrap_or(false);
    let initial_bounds = std::env::var("HOLON_INITIAL_WINDOW_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            let w: f32 = w.trim().parse().ok()?;
            let h: f32 = h.trim().parse().ok()?;
            Some(gpui::Bounds {
                origin: gpui::point(px(100.0), px(100.0)),
                size: gpui::size(px(w), px(h)),
            })
        });
    let window_options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(
                custom_title
                    .clone()
                    .unwrap_or_else(|| "Holon".to_string())
                    .into(),
            ),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(9.0), px(9.0))),
        }),
        window_background: if glass {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Opaque
        },
        window_bounds: initial_bounds.map(gpui::WindowBounds::Windowed),
        ..Default::default()
    };

    // Pre-warm the root layout watcher: start the tokio watcher task and
    // wait for its first event to transition render_expr from Loading to
    // the real expression. Without this wait, the window opens with an
    // empty view and the signal's first fire may be Loading — by the time
    // the real event arrives on tokio, the GPUI subscription may have
    // already gone quiet, causing BoundsRegistry to stay empty.
    //
    // Only pre-warm when we were given an existing_engine (PBT / MCP
    // desktop case) — otherwise the engine doesn't exist yet and has to
    // be created inside open_window's callback. The pre-warm is driven
    // synchronously on gpui's background executor so that the call path
    // stays on the main thread and the outer cx.spawn wrapper (which
    // breaks on iOS) can be avoided.
    if let Some(ref engine) = existing_engine {
        use futures::future::{select, Either};
        use futures::StreamExt;
        use futures_signals::signal::SignalExt;
        let root_uri = holon_api::root_layout_block_uri();
        let signal = engine.watch_data_signal(&root_uri);
        let fg_executor = cx.foreground_executor().clone();
        let bg_executor = cx.background_executor().clone();
        let prewarm_max = std::time::Duration::from_secs(10);
        fg_executor.block_on(async move {
            let mut stream = signal.to_stream();
            let prewarm_start = std::time::Instant::now();
            loop {
                let elapsed = prewarm_start.elapsed();
                if elapsed >= prewarm_max {
                    eprintln!("[GPUI] pre-warm timeout — window will open with loading state");
                    break;
                }
                let timeout = bg_executor.timer(prewarm_max - elapsed);
                let next_fut = stream.next();
                match select(Box::pin(next_fut), Box::pin(timeout)).await {
                    Either::Left((Some(rvm), _)) => {
                        if rvm.widget_name().as_deref() != Some("loading") {
                            eprintln!(
                                "[GPUI] pre-warm: root signal fired with real data after {:?}",
                                prewarm_start.elapsed()
                            );
                            break;
                        }
                    }
                    Either::Left((None, _)) => {
                        eprintln!("[GPUI] pre-warm: signal stream ended");
                        break;
                    }
                    Either::Right(_) => {
                        eprintln!("[GPUI] pre-warm timeout — window will open with loading state");
                        break;
                    }
                }
            }
        });
    }

    tracing::debug!("[GPUI] About to call cx.open_window...");
    let bounds_registry_for_pump = bounds_registry.clone();
    let window_result = cx.open_window(window_options, |window, cx| {
        tracing::debug!("[GPUI] Inside open_window callback — building root view");
        window.on_window_should_close(cx, |_window, cx| {
            cx.quit();
            true
        });

        let engine = if let Some(engine) = existing_engine {
            engine
        } else {
            // Break circular dependency: engine needs interpret_fn, which needs
            // services (= the engine). Use OnceLock for deferred init.
            let services_slot: Arc<std::sync::OnceLock<Arc<dyn BuilderServices>>> =
                Arc::new(std::sync::OnceLock::new());

            let engine = Arc::new(ReactiveEngine::new(
                Arc::clone(&session_clone),
                handle_clone.clone(),
                Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
                make_interpret_fn(services_slot.clone()),
            ));

            let services: Arc<dyn BuilderServices> = engine.clone();
            services_slot.set(services).ok();
            engine
        };

        let root_uri = holon_api::root_layout_block_uri();
        let root_vm = engine.snapshot_reactive(&root_uri);
        let view_model = root_vm.snapshot_resolved(&|bid| engine.snapshot(bid));

        // Install the block resolver so `nav.bubble_input` can cross
        // `live_block` boundaries. Without this, chord ops (Tab/Shift+Tab/
        // Enter/Alt+Up/Alt+Down) from a focused editor inside a live_block
        // silently no-op — the router walks past the empty slot and never
        // finds the entity. The resolver returns the latest snapshot of the
        // nested block's tree on demand.
        {
            let engine_for_resolver = engine.clone();
            nav.set_block_resolver(std::sync::Arc::new(move |block_id: &str| {
                let uri = holon_api::EntityUri::from_raw(block_id);
                Some(std::sync::Arc::new(
                    engine_for_resolver.snapshot_reactive(&uri),
                ))
            }));
        }

        let shadow_ctx = RenderContext::default();

        let focus = FocusRegistry::new();
        let initial_root_view = root_reactive_view(&root_vm);
        let share_ui_entity = cx.new(|_cx| share_ui::ShareUiState::new());
        let app_model = cx.new(|cx| {
            let mut model = AppModel {
                session: Arc::clone(&session_clone),
                engine: engine.clone(),
                rt_handle: handle_clone.clone(),
                focus: focus.clone(),
                nav: nav.clone(),
                bounds_registry: bounds_registry.clone(),
                root_vm: Arc::new(root_vm),
                view_model,
                shadow_ctx,
                show_settings: false,
                show_widget_gallery: false,
                share_ui: share_ui_entity.clone(),
                root_live_blocks: std::collections::HashMap::new(),
                pending_cursor: None,
                root_view: initial_root_view,
            };
            // Initial reconciliation — create root LiveBlockView entities.
            // Each LiveBlockView manages its own child entities (editors, live queries).
            model.reconcile_root_live_blocks(cx);
            // Seed the initial viewport: push the window's current logical size
            // and scale factor into UiState and the root ReactiveView's space
            // Mutable, kicking off the container-query cascade before the first
            // frame is painted.
            let initial_vp =
                viewport_info_from_window(window.viewport_size(), window.scale_factor());
            model.apply_viewport(initial_vp);
            model
        });
        model_slot.set(app_model.clone()).ok();

        let focused_element_id = debug
            .as_ref()
            .map(|d| d.focused_element_id.clone())
            .unwrap_or_default();
        let app_model_for_view = app_model.clone();
        let view = cx.new(|cx| {
            cx.observe(&app_model, |_this, _model, cx| cx.notify())
                .detach();
            // Install window-bounds observer: every window resize, keyboard
            // show/hide, orientation change, or safe-area change fires this
            // callback. It recomputes `ViewportInfo` and pushes it through
            // `AppModel::apply_viewport`, which updates `UiState.viewport`
            // and the root ReactiveView's `space` Mutable. The reactive
            // cascade rebuilds only affected subtrees — no full rebuild,
            // transient widget state is preserved in untouched branches.
            cx.observe_window_bounds(window, move |_this, window, cx| {
                let vp = viewport_info_from_window(window.viewport_size(), window.scale_factor());
                app_model_for_view.update(cx, |m, _cx| m.apply_viewport(vp));
            })
            .detach();

            // Re-render the HolonApp whenever ShareUiState emits NotifyShareUi.
            // Without this the share/accept/quarantine modals would not appear
            // until the next unrelated render pass.
            cx.subscribe(
                &share_ui_entity,
                move |_this, _entity, _ev: &share_ui::NotifyShareUi, cx| {
                    cx.notify();
                },
            )
            .detach();

            HolonApp {
                session: session_clone,
                rt_handle: handle_clone,
                app_model,
                focus,
                nav,
                bounds_registry,
                entity_cache: Default::default(),
                safe_area_top: 0.0,
                safe_area_bottom: 0.0,
                focused_element_id,
                share_ui: share_ui_entity,
            }
        });
        let any_view: AnyView = view.into();
        cx.new(|cx| gpui_component::Root::new(any_view, window, cx))
    });
    let window_handle = match window_result {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("[GPUI] cx.open_window failed: {e:?}");
            return;
        }
    };
    tracing::debug!("[GPUI] Window opened, starting reactive stream...");

    let app_model = model_entity.get().unwrap().clone();
    let wh: AnyWindowHandle = window_handle.into();

    // Root layout signal — structural changes only (render_expr).
    // Does NOT react to ui_generation (focus/view_mode) — the root
    // layout is a static columns container whose structure doesn't
    // depend on which block is focused. This avoids the full
    // HolonApp re-render cascade (269 EditorView renders) on every
    // arrow key press.
    let root_uri = holon_api::root_layout_block_uri();
    let engine = app_model.read(cx).engine.clone();

    if let Some(ref debug) = debug {
        let async_cx = cx.to_async();
        setup_interaction_pump(
            debug,
            window_handle.into(),
            &async_cx,
            bounds_registry_for_pump,
            engine.clone(),
        );
    }

    // Wire the share-subtree degraded-bus bridge + ShareTrigger global. If
    // `share_backend` is `None` (iroh-sync disabled or PBT) no bridge is
    // spawned and ShareTrigger is not installed — the share context menu
    // silently no-ops with a warning.
    if let Some(backend) = share_backend {
        let async_cx = cx.to_async();
        let share_ui_entity = app_model.read(cx).share_ui.clone();
        share_ui::spawn_degraded_bus_bridge(
            backend,
            rt_handle.clone(),
            share_ui_entity.clone(),
            window_handle.into(),
            &async_cx,
        );

        // Install the ShareTrigger global so block right-click handlers can
        // dispatch `share_subtree` without plumbing session/rt_handle/async_cx
        // through every intermediate builder.
        let session_for_trigger = app_model.read(cx).session.clone();
        let rt_handle_for_trigger = rt_handle.clone();
        let window_handle_for_trigger: AnyWindowHandle = window_handle.into();
        cx.set_global(share_ui::ShareTrigger::new(
            move |block_id, cx: &mut App| {
                let async_cx = cx.to_async();
                share_ui::dispatch_share(
                    session_for_trigger.clone(),
                    rt_handle_for_trigger.clone(),
                    share_ui_entity.clone(),
                    window_handle_for_trigger,
                    &async_cx,
                    block_id,
                );
            },
        ));
    }
    // Use watch_signal (ui_generation-aware) so viewport changes bumping
    // `ui_generation` via `UiState::set_viewport` re-fire the root. This
    // lets the root `if_space(...)` re-pick its breakpoint branch when the
    // window resizes. Focus changes do NOT bump ui_generation so they
    // don't cascade here.
    let root_signal = engine.watch_signal(&root_uri);

    cx.spawn({
        let app_model = app_model.clone();
        async move |cx| {
            use futures_signals::signal::SignalExt;
            root_signal
                .for_each(|rvm| {
                    tracing::debug!("[root-signal] fired");
                    let _ = cx.update_window(wh, |_, window, cx| {
                        app_model.update(cx, |m, cx| {
                            m.root_vm = Arc::new(rvm);

                            // Reconcile root LiveBlockView entities.
                            // Each LiveBlockView manages its own child entities.
                            m.reconcile_root_live_blocks(cx);

                            // Resolve view_model + update input router.
                            m.view_model =
                                resolved_view_model(&m.root_vm, &m.engine, &m.root_live_blocks, cx);
                            m.nav.set_root(m.root_vm.clone(), &m.focus);

                            // Consume buffered cursor position now that editors may exist.
                            if let Some((block_id, cursor_offset)) = m.pending_cursor.take() {
                                if let Some(input) = m.focus.editor_inputs.get(&block_id) {
                                    use gpui_component::RopeExt;
                                    window.focus(&input.focus_handle(cx), cx);
                                    let pos = input
                                        .read(cx)
                                        .text()
                                        .offset_to_position(cursor_offset as usize);
                                    input.update(cx, |state, cx| {
                                        state.set_cursor_position(pos, window, cx);
                                    });
                                } else {
                                    // Editor not found — drop the pending cursor.
                                    // Re-buffering would cause an infinite render loop
                                    // (cx.notify below → re-render → re-buffer → notify).
                                    tracing::debug!(
                                        "[pending_cursor] Editor not found for {block_id}, dropping"
                                    );
                                }
                            }

                            cx.notify();
                        });
                    });
                    async {}
                })
                .await;
        }
    })
    .detach();

    // Editor cursor signal — transfers GPUI focus when editor_cursor changes.
    let cursor_signal = engine.watch_editor_cursor();
    cx.spawn({
        let app_model = app_model.clone();
        async move |cx| {
            use futures_signals::signal::SignalExt;
            cursor_signal
                .for_each(|cursor| {
                    tracing::debug!("[cursor-signal] fired: {cursor:?}");
                    if let Some((block_id, cursor_offset)) = cursor {
                        let _ = cx.update_window(wh, |_, window, cx| {
                            let already_focused = app_model
                                .read(cx)
                                .focus
                                .focused_editor_row_id(window, cx)
                                .map_or(false, |focused| focused == block_id);
                            if already_focused {
                                return;
                            }

                            app_model.update(cx, |m, _cx| {
                                if let Some(input) = m.focus.editor_inputs.get(&block_id) {
                                    use gpui_component::RopeExt;
                                    window.focus(&input.focus_handle(_cx), _cx);
                                    let pos = input
                                        .read(_cx)
                                        .text()
                                        .offset_to_position(cursor_offset as usize);
                                    input.update(_cx, |state, cx| {
                                        state.set_cursor_position(pos, window, cx);
                                    });
                                } else {
                                    m.pending_cursor = Some((block_id, cursor_offset));
                                }
                            });
                        });
                    }
                    async {}
                })
                .await;
        }
    })
    .detach();

    // iOS/Android keyboard height observer.
    //
    // gpui_mobile::keyboard_height() is updated by platform notifications
    // (UIKeyboardWillShow/Hide on iOS). GPUI's `force_render` re-paints the
    // window but skips `render()` on views that aren't dirty. This poller
    // detects keyboard height changes and marks the HolonApp view dirty so
    // the next draw picks up the new safe_area_bottom_px().
    #[cfg(feature = "mobile")]
    cx.spawn({
        let app_model = app_model.clone();
        async move |cx| {
            use std::sync::atomic::Ordering;
            let mut last_bits = gpui_mobile::KEYBOARD_HEIGHT_BITS.load(Ordering::Relaxed);
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let bits = gpui_mobile::KEYBOARD_HEIGHT_BITS.load(Ordering::Relaxed);
                if bits != last_bits {
                    last_bits = bits;
                    let _ = cx.update_window(wh, |_, window, cx| {
                        let vp = viewport_info_from_window(
                            window.viewport_size(),
                            window.scale_factor(),
                        );
                        app_model.update(cx, |m, _cx| m.apply_viewport(vp));
                    });
                }
            }
        }
    })
    .detach();

    tracing::debug!("[GPUI] Reactive engine running");
}

/// Return the set of widget names this GPUI frontend supports.
pub fn render_supported_widgets() -> std::collections::HashSet<String> {
    let mut widgets: std::collections::HashSet<String> = render::builders::builder_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    // Collection layouts are handled via ReactiveShell, not individual GPUI builders.
    // They must be in the supported set so profile variant filtering doesn't drop them.
    for name in ["table", "tree", "list", "outline", "columns"] {
        widgets.insert(name.to_string());
    }
    widgets
}

pub fn is_theme_dark(session: &FrontendSession) -> bool {
    load_theme_def(session).is_dark
}

/// Handle a MoveUp/MoveDown action that bubbled up from an InputState at its boundary.
/// Uses the ShadowDom to find the next block and transfers focus + cursor position.
/// Updates `UiState` so variant predicates (e.g., `is_focused`) react to the change.
#[tracing::instrument(level = "debug", skip_all, fields(?direction))]
fn handle_cross_block_nav(
    nav: &NavigationState,
    focus: &FocusRegistry,
    engine: &ReactiveEngine,
    focused_element_id: &Arc<std::sync::RwLock<Option<String>>>,
    direction: NavDirection,
    boundary: Boundary,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(focused_id) = focus.focused_editor_row_id(window, cx) else {
        tracing::debug!(
            "cross_block_nav: no focused editor found (editors={}, has_root={})",
            focus.editor_inputs.len(),
            nav.has_root()
        );
        return;
    };

    let column = focus.focused_cursor_column(&focused_id, cx);
    let hint = CursorHint { column, boundary };
    let input = WidgetInput::Navigate { direction, hint };

    match nav.bubble_input(&focused_id, &input) {
        Some(InputAction::Focus {
            block_id,
            placement,
        }) => {
            if let Some(target_input) = focus.editor_inputs.get(&block_id) {
                let text = target_input.read(cx).value().to_string();
                let offset = holon_frontend::navigation::placement_to_offset(&text, placement);
                let focus_handle = target_input.read(cx).focus_handle(cx).clone();
                target_input.update(cx, |state, cx| {
                    state.set_cursor_offset(offset, cx);
                });
                window.focus(&focus_handle, cx);

                *focused_element_id.write().unwrap() = Some(block_id.clone());
                engine
                    .ui_state()
                    .set_focus(Some(holon_api::EntityUri::from_raw(&block_id)));
            } else {
                tracing::debug!(
                    "cross_block_nav: bubble_input returned block_id={block_id} but no editor_input found for it (editors={})",
                    focus.editor_inputs.len()
                );
            }
        }
        Some(other) => {
            tracing::debug!("cross_block_nav: bubble_input returned non-Focus action: {other:?}");
        }
        None => {
            tracing::debug!(
                "cross_block_nav: bubble_input returned None for focused_id={focused_id}, direction={direction:?} (router={})",
                nav.describe()
            );
        }
    }
}

/// Dispatch an interaction event into the GPUI window.
///
/// Uses `dispatch_keystroke` for key events (which calls `dispatch_input` on the
/// input handler for text insertion) and `dispatch_event` for mouse events.
/// Wire up the MCP → GPUI interaction channel.
///
/// Creates a sync channel, registers the sender on `DebugServices`, and spawns
/// an async pump that forwards `InteractionCommand`s to the GPUI window.
pub fn setup_interaction_pump(
    debug: &std::sync::Arc<holon_mcp::server::DebugServices>,
    window_handle: AnyWindowHandle,
    cx: &gpui::AsyncApp,
    bounds_registry: BoundsRegistry,
    engine: Arc<ReactiveEngine>,
) {
    let (tx, mut rx) = futures::channel::mpsc::channel::<holon_mcp::server::InteractionCommand>(16);
    debug.interaction_tx.set(tx.clone()).ok();

    // Install the channel-based `UserDriver` so MCP tools can dispatch
    // UI mutations through the same pipeline as click/key/scroll.
    let geometry: Arc<dyn holon_frontend::geometry::GeometryProvider> = Arc::new(bounds_registry);
    let driver: Arc<dyn holon_frontend::user_driver::UserDriver> =
        Arc::new(user_driver::GpuiUserDriver::new(tx, geometry, engine));
    debug.user_driver.set(driver).ok();

    cx.spawn({
        async move |cx| {
            use futures::StreamExt;
            while let Some(cmd) = rx.next().await {
                let result = cx.update_window(window_handle, |_, window, cx| {
                    dispatch_interaction(&cmd.event, window, cx)
                });
                let response = match result {
                    Ok(handled) => holon_mcp::server::InteractionResponse {
                        handled,
                        detail: None,
                    },
                    Err(e) => holon_mcp::server::InteractionResponse {
                        handled: false,
                        detail: Some(e.to_string()),
                    },
                };
                cmd.response_tx.send(response).ok();
            }
        }
    })
    .detach();
}

pub fn dispatch_interaction(
    event: &holon_mcp::server::InteractionEvent,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    use holon_mcp::server::InteractionEvent;
    match event {
        InteractionEvent::KeyDown { .. } | InteractionEvent::KeyUp { .. } => {
            let inputs = interaction_event_to_platform_inputs(event);
            for input in inputs {
                if let gpui::PlatformInput::KeyDown(key_down) = input {
                    if window.dispatch_keystroke(key_down.keystroke, cx) {
                        return true;
                    }
                } else {
                    let r = window.dispatch_event(input, cx);
                    if !r.propagate {
                        return true;
                    }
                }
            }
            false
        }
        _ => {
            let inputs = interaction_event_to_platform_inputs(event);
            let mut handled = false;
            for input in inputs {
                let r = window.dispatch_event(input, cx);
                if !r.propagate {
                    handled = true;
                }
            }
            handled
        }
    }
}

/// Convert an MCP InteractionEvent to one or more GPUI PlatformInputs.
/// MouseClick produces both MouseDown + MouseUp (GPUI needs both for click handlers).
pub fn interaction_event_to_platform_inputs(
    event: &holon_mcp::server::InteractionEvent,
) -> Vec<gpui::PlatformInput> {
    use holon_mcp::server::InteractionEvent;

    fn parse_modifiers(mods: &[String]) -> gpui::Modifiers {
        let mut m = gpui::Modifiers::default();
        for s in mods {
            match s.to_lowercase().as_str() {
                "cmd" | "command" | "platform" => m.platform = true,
                "ctrl" | "control" => m.control = true,
                "alt" | "option" => m.alt = true,
                "shift" => m.shift = true,
                "fn" | "function" => m.function = true,
                _ => {}
            }
        }
        m
    }

    fn parse_button(s: &str) -> gpui::MouseButton {
        match s.to_lowercase().as_str() {
            "right" => gpui::MouseButton::Right,
            "middle" => gpui::MouseButton::Middle,
            _ => gpui::MouseButton::Left,
        }
    }

    match event {
        InteractionEvent::MouseClick {
            position,
            button,
            modifiers,
        } => {
            let pos = gpui::point(gpui::px(position.0), gpui::px(position.1));
            let mods = parse_modifiers(modifiers);
            let btn = parse_button(button);
            vec![
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: btn,
                    position: pos,
                    modifiers: mods,
                    click_count: 1,
                    first_mouse: false,
                }),
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: btn,
                    position: pos,
                    modifiers: mods,
                    click_count: 1,
                }),
            ]
        }
        InteractionEvent::KeyDown {
            keystroke,
            modifiers,
        } => {
            let extra_mods = parse_modifiers(modifiers);
            // Build keystroke string in GPUI format: "ctrl-shift-x"
            let mut parts = Vec::new();
            if extra_mods.platform {
                parts.push("cmd");
            }
            if extra_mods.control {
                parts.push("ctrl");
            }
            if extra_mods.alt {
                parts.push("alt");
            }
            if extra_mods.shift {
                parts.push("shift");
            }
            if extra_mods.function {
                parts.push("fn");
            }
            parts.push(keystroke);
            let ks_str = parts.join("-");
            let ks = gpui::Keystroke::parse(&ks_str)
                .unwrap_or_else(|_| gpui::Keystroke {
                    modifiers: extra_mods,
                    key: keystroke.clone(),
                    key_char: None,
                })
                .with_simulated_ime();
            vec![gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: ks,
                is_held: false,
                prefer_character_input: false,
            })]
        }
        InteractionEvent::KeyUp {
            keystroke,
            modifiers,
        } => {
            let extra_mods = parse_modifiers(modifiers);
            let mut parts = Vec::new();
            if extra_mods.platform {
                parts.push("cmd");
            }
            if extra_mods.control {
                parts.push("ctrl");
            }
            if extra_mods.alt {
                parts.push("alt");
            }
            if extra_mods.shift {
                parts.push("shift");
            }
            parts.push(keystroke);
            let ks_str = parts.join("-");
            let ks = gpui::Keystroke::parse(&ks_str).unwrap_or_else(|_| gpui::Keystroke {
                modifiers: extra_mods,
                key: keystroke.clone(),
                key_char: None,
            });
            vec![gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                keystroke: ks,
            })]
        }
        InteractionEvent::MouseDown {
            position,
            button,
            modifiers,
        } => {
            let pos = gpui::point(gpui::px(position.0), gpui::px(position.1));
            let mods = parse_modifiers(modifiers);
            let btn = parse_button(button);
            vec![gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                button: btn,
                position: pos,
                modifiers: mods,
                click_count: 1,
                first_mouse: false,
            })]
        }
        InteractionEvent::MouseUp {
            position,
            button,
            modifiers,
        } => {
            let pos = gpui::point(gpui::px(position.0), gpui::px(position.1));
            let mods = parse_modifiers(modifiers);
            let btn = parse_button(button);
            vec![gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                button: btn,
                position: pos,
                modifiers: mods,
                click_count: 1,
            })]
        }
        InteractionEvent::MouseMove {
            position,
            pressed_button,
            modifiers,
        } => {
            let pos = gpui::point(gpui::px(position.0), gpui::px(position.1));
            let mods = parse_modifiers(modifiers);
            let pressed = pressed_button.as_deref().map(parse_button);
            vec![gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: pos,
                pressed_button: pressed,
                modifiers: mods,
            })]
        }
        InteractionEvent::ScrollWheel {
            position,
            delta,
            modifiers,
        } => {
            let pos = gpui::point(gpui::px(position.0), gpui::px(position.1));
            let mods = parse_modifiers(modifiers);
            let scroll_delta = gpui::ScrollDelta::Lines(gpui::point(delta.0, delta.1));
            vec![gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                position: pos,
                delta: scroll_delta,
                modifiers: mods,
                touch_phase: gpui::TouchPhase::default(),
            })]
        }
    }
}

/// Apply holon's custom theme colors on top of gpui_component's base theme.
fn apply_holon_theme(session: &FrontendSession, cx: &mut App) {
    let theme_def = load_theme_def(session);
    let mode = if theme_def.is_dark {
        gpui_component::theme::ThemeMode::Dark
    } else {
        gpui_component::theme::ThemeMode::Light
    };
    gpui_component::theme::Theme::change(mode, None, cx);

    let c = &theme_def.colors;
    let theme = gpui_component::theme::Theme::global_mut(cx);
    theme.colors.primary = rgba8_to_hsla(c.primary);
    theme.colors.primary_hover = rgba8_to_hsla(darken(c.primary, 0.1));
    theme.colors.primary_active = rgba8_to_hsla(darken(c.primary, 0.2));
    theme.colors.primary_foreground = rgba8_to_hsla(c.background);
    theme.colors.foreground = rgba8_to_hsla(c.text_primary);
    theme.colors.muted_foreground = rgba8_to_hsla(c.text_secondary);
    theme.colors.background = rgba8_to_hsla(c.background);
    theme.colors.secondary = rgba8_to_hsla(c.background_secondary);
    theme.colors.secondary_foreground = rgba8_to_hsla(c.text_primary);
    theme.colors.sidebar = rgba8_to_hsla(c.sidebar_background);
    theme.colors.sidebar_foreground = rgba8_to_hsla(c.text_primary);
    theme.colors.sidebar_border = rgba8_to_hsla(c.border);
    theme.colors.border = rgba8_to_hsla(c.border);
    theme.colors.input = rgba8_to_hsla(c.border);
    theme.colors.ring = rgba8_to_hsla(c.border_focus);
    theme.colors.accent = rgba8_to_hsla(c.primary);
    theme.colors.accent_foreground = rgba8_to_hsla(c.text_primary);
    theme.colors.success = rgba8_to_hsla(c.success);
    theme.colors.success_foreground = rgba8_to_hsla(c.background);
    theme.colors.danger = rgba8_to_hsla(c.error);
    theme.colors.danger_foreground = rgba8_to_hsla(c.background);
    theme.colors.warning = rgba8_to_hsla(c.warning);
    theme.colors.link = rgba8_to_hsla(c.primary_light);
    theme.colors.popover = rgba8_to_hsla(c.background_secondary);
    theme.colors.popover_foreground = rgba8_to_hsla(c.text_primary);
    theme.colors.list = rgba8_to_hsla(c.background);
    theme.colors.list_hover = rgba8_to_hsla(c.background_secondary);
    theme.colors.table = rgba8_to_hsla(c.background);
    theme.colors.table_head = rgba8_to_hsla(c.background_secondary);
    theme.colors.tab_bar = rgba8_to_hsla(c.background_secondary);
    theme.colors.scrollbar_thumb = rgba8_to_hsla(c.text_tertiary);
}

fn rgba8_to_hsla(c: holon_frontend::theme::Rgba8) -> gpui::Hsla {
    gpui::rgba((c[0] as u32) << 24 | (c[1] as u32) << 16 | (c[2] as u32) << 8 | (c[3] as u32))
        .into()
}

fn darken(c: holon_frontend::theme::Rgba8, amount: f32) -> holon_frontend::theme::Rgba8 {
    [
        (c[0] as f32 * (1.0 - amount)) as u8,
        (c[1] as f32 * (1.0 - amount)) as u8,
        (c[2] as f32 * (1.0 - amount)) as u8,
        c[3],
    ]
}

fn load_theme_def(session: &FrontendSession) -> holon_frontend::theme::ThemeDef {
    let user_dir = std::env::var("HOME")
        .ok() // ALLOW(ok): non-critical env var
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

/// Translate a GPUI Keystroke into a set of holon Key values for KeyMap lookup.
fn keystroke_to_keys(ks: &gpui::Keystroke) -> std::collections::BTreeSet<holon_api::Key> {
    use holon_api::Key;
    let mut keys = std::collections::BTreeSet::new();
    if ks.modifiers.platform {
        keys.insert(Key::Cmd);
    }
    if ks.modifiers.control {
        keys.insert(Key::Ctrl);
    }
    if ks.modifiers.alt {
        keys.insert(Key::Alt);
    }
    if ks.modifiers.shift {
        keys.insert(Key::Shift);
    }
    match ks.key.as_str() {
        "enter" => {
            keys.insert(Key::Enter);
        }
        "backspace" => {
            keys.insert(Key::Backspace);
        }
        "delete" => {
            keys.insert(Key::Delete);
        }
        "escape" => {
            keys.insert(Key::Escape);
        }
        "tab" => {
            keys.insert(Key::Tab);
        }
        "space" => {
            keys.insert(Key::Space);
        }
        "up" => {
            keys.insert(Key::Up);
        }
        "down" => {
            keys.insert(Key::Down);
        }
        "left" => {
            keys.insert(Key::Left);
        }
        "right" => {
            keys.insert(Key::Right);
        }
        "home" => {
            keys.insert(Key::Home);
        }
        "end" => {
            keys.insert(Key::End);
        }
        "pageup" => {
            keys.insert(Key::PageUp);
        }
        "pagedown" => {
            keys.insert(Key::PageDown);
        }
        s if s.len() == 1 => {
            keys.insert(Key::Char(s.chars().next().unwrap().to_ascii_uppercase()));
        }
        s if s.starts_with('f') && s[1..].parse::<u8>().is_ok() => {
            keys.insert(Key::F(s[1..].parse().unwrap()));
        }
        _ => {}
    }
    keys
}
