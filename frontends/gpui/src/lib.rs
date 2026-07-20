//! @c4 container
//! @c4 layer UI
//! Pattern: MVVM View
//!
//! GPUI desktop frontend (primary; mobile via gpui-mobile feature) — the MVVM
//! **View** layer; its render functions build native GPUI widgets from
//! holon-frontend's `ReactiveViewModel`.

#![recursion_limit = "1024"]

pub mod breadcrumb;
pub mod di;
pub mod entity_view_registry;
pub mod geometry;
#[cfg(debug_assertions)]
pub mod inspector;
#[cfg(feature = "mobile")]
pub mod mobile;
pub mod navigation_state;
#[cfg(debug_assertions)]
pub mod oracles_ui;
pub mod reactive_vm_poc;
pub mod render;
pub mod reset;
pub mod search_ui;
pub mod share_ui;

pub mod user_driver;
pub mod views;
pub mod window_state;

use std::sync::Arc;

use entity_view_registry::LocalEntityScope;
use geometry::BoundsRegistry;
use gpui::prelude::*;
use gpui::*;
use holon_api::EntityName;
use holon_frontend::FrontendSession;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::input::InputAction;
use holon_frontend::input::WidgetInput;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
// Re-export the shared interpret function for DI wiring
pub use holon_frontend::reactive::make_interpret_fn;
use holon_frontend::theme::ThemeRegistry;
use holon_frontend::view_model::ViewModel;
use navigation_state::NavigationState;
use render::builders::GpuiRenderContext;

/// Half-spread (in HSLA lightness, 0.0–1.0) of the subtle root-background
/// gradient painted in `render`. The top of the window is lightened by this
/// amount and the bottom darkened by it, both derived from the active theme's
/// `background` token so the fade tracks light/dark themes automatically.
/// Keep it small — this is a hint of depth, not a visible band. Raise it for a
/// more pronounced fade; set it to `0.0` to restore a flat fill.
const BG_GRADIENT_LIGHTNESS_SPREAD: f32 = 0.015;

// ── Android icon-glyph substitutes ──────────────────────────────────────────
//
// UI-chrome icons that are *monochrome* Unicode symbols DejaVu Sans covers
// (☰ ◧ ⚙, chevrons, checkboxes, arrows, …) render on Android via the DejaVu
// Sans coverage font we embed and register in
// `mobile::register_android_icon_fonts` — cosmic-text's per-glyph resolution
// picks up their glyphs from it.
//
// Two other classes render as tofu on Android and need a substitute:
//   • color emoji (🎨 🔗 🔍 🔎 ⛔ 🗑) — Android's on-device NotoColorEmoji uses
//     COLR v1 outlines that gpui-mobile's swash rasteriser cannot render, and
//     no CBDT emoji font ships in the APK; DejaVu has no astral-plane emoji.
//   • monochrome symbols DejaVu simply lacks (⧉ U+29C9) — same mechanism.
// On Android each is swapped for a DejaVu-covered symbol; mac/iOS use their own
// system text systems and keep the original glyph.
//
// INVARIANT (enforced by the icon-font coverage tests in this crate — see
// `icon_font_tests` here and the co-located sweeps in `render::builders::
// op_button` and `render::builders::icon`): every substitute here is present in
// the embedded DejaVu Sans cmap, and every source glyph is genuinely absent
// from it. When you add an icon glyph the DejaVu font can't render, add a row
// here (and route the glyph through `icon()`), or it will tofu on Android.
// Referenced by the Android `icon()`/`substitute_glyph` and by the tests; on
// desktop non-test builds those paths don't compile, so it reads as dead there.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const ICON_SUBSTITUTES: &[(&str, &str)] = &[
    ("🎨", "▦"), // widget gallery  → square with crosshatch (grid/gallery)
    ("🔗", "⚭"), // accept ticket   → interlocked rings (link)
    ("🔍", "⚲"), // inspector       → magnifier-like symbol
    ("🔎", "⚲"), // search field    → magnifier-like symbol
    ("⛔", "⊘"), // degraded banner → circled slash (blocked)
    ("🗑", "⌦"),  // delete op       → erase-to-the-right (delete)
    ("⧉", "❐"),  // embed op        → shadowed square (overlay/embed)
];

/// Monochrome glyphs the embedded DejaVu Sans genuinely cannot render on
/// Android for which no acceptable DejaVu substitute exists — DejaVu ships no
/// padlock glyph at all. These are only reachable if a layout names the `lock`
/// / `unlock` semantic icon (`render::builders::icon`); no current layout does
/// (the widget gallery uses lucide SVG names that fall through to `•`). Listed
/// here so the coverage tests record the gap loudly instead of it re-escaping
/// silently. If one becomes reachable, ship an SVG icon or a lock-capable
/// coverage font rather than a misleading substitute.
#[allow(dead_code)] // read only by the icon-font coverage tests
pub(crate) const KNOWN_ANDROID_GLYPH_GAPS: &[&str] = &["🔒", "🔓"];

/// Every non-ASCII icon glyph rendered from an *inline literal* (not from a
/// name→glyph table like `op_button`/`icon`). Kept here as the single place the
/// coverage test sweeps inline literals — the hand-maintained list that missing
/// an entry is the one drift risk, so each entry names its source site.
#[allow(dead_code)] // read only by the icon-font coverage tests
pub(crate) const INLINE_UI_GLYPHS: &[&str] = &[
    "☰",  // lib.rs left-sidebar toggle
    "◧",  // lib.rs right-sidebar toggle
    "⚙",  // lib.rs settings gear
    "🎨", // lib.rs widget-gallery toggle
    "🔗", // lib.rs accept-ticket toggle
    "🔎", // lib.rs search field
    "🔍", // inspector.rs
    "✕",  // lib.rs / share_ui.rs / oracles_ui.rs close/dismiss
    "⚠",  // share_ui.rs degraded banner
    "↻",  // share_ui.rs rehydration banner
    "⛔", // share_ui.rs blocked banner
    "▸",  // collapsible.rs collapsed chevron
    "▾",  // collapsible.rs expanded chevron
    "▼",  // expand_toggle.rs / reactive_vm_poc.rs expanded
    "▶",  // expand_toggle.rs / reactive_vm_poc.rs collapsed
    "◉",  // checkbox.rs checked
    "○",  // checkbox.rs unchecked
];

/// Apply the Android icon substitution table to a glyph (see
/// [`ICON_SUBSTITUTES`]). Non-`cfg`-gated so the host-side coverage tests can
/// exercise the exact mapping the Android `icon()` uses.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn substitute_glyph(glyph: &'static str) -> &'static str {
    let mut i = 0;
    while i < ICON_SUBSTITUTES.len() {
        if ICON_SUBSTITUTES[i].0 == glyph {
            return ICON_SUBSTITUTES[i].1;
        }
        i += 1;
    }
    glyph
}

/// Map a UI-chrome icon glyph to a platform-renderable form.
///
/// On Android, glyphs DejaVu can't render are swapped for DejaVu-covered
/// substitutes (see [`ICON_SUBSTITUTES`]); everything else passes through. On
/// every other platform this is the identity.
#[cfg(target_os = "android")]
pub(crate) fn icon(glyph: &'static str) -> &'static str {
    substitute_glyph(glyph)
}

/// See the Android variant. Identity on non-Android platforms.
#[cfg(not(target_os = "android"))]
pub(crate) fn icon(glyph: &'static str) -> &'static str {
    glyph
}

/// Shared host-side coverage helper for the icon-font tests. Asserts that
/// `glyph`, after the Android substitution, is renderable by the embedded
/// DejaVu Sans font — i.e. every non-ASCII char resolves to a glyph — OR that
/// it is a documented [`KNOWN_ANDROID_GLYPH_GAPS`] entry. `source` names the
/// call site for a legible failure.
#[cfg(test)]
pub(crate) fn assert_icon_renderable_on_android(glyph: &'static str, source: &str) {
    const DEJAVU_SANS: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
    let face = ttf_parser::Face::parse(DEJAVU_SANS, 0).expect("embedded DejaVu Sans must parse");
    if KNOWN_ANDROID_GLYPH_GAPS.contains(&glyph) {
        return;
    }
    let effective = substitute_glyph(glyph);
    for ch in effective.chars() {
        assert!(
            ch.is_ascii() || face.glyph_index(ch).is_some(),
            "{source}: icon {glyph:?} → Android-effective {effective:?} has char U+{:04X} that \
             DejaVu Sans cannot render and no substitute covers (add a row to ICON_SUBSTITUTES \
             or KNOWN_ANDROID_GLYPH_GAPS)",
            ch as u32
        );
    }
}

// ── Global undo/redo actions ────────────────────────────────────────────────
//
// `gpui_component::input::{Undo, Redo}` (bound to cmd-z / cmd-shift-z inside
// `InputState`, see gpui-component's `input/state.rs`) are scoped to the
// "Input" key context, so they never resolve at all while no editor is
// focused — that's the dogfooded "No handler matched the key chord" bug.
// These two actions are bound with `context: None` below (`launch_holon_
// window_impl`) so cmd-z/cmd-shift-z always resolves to *something*
// dispatchable, regardless of focus. The page-level capture_action handlers
// in `HolonApp::render` intercept both these and the `Input` actions (in the
// capture phase, before `InputState`'s own bubble-phase text-undo can run)
// and route both to the engine-level `FrontendSession::undo`/`redo`.
actions!(holon_gpui, [TriggerUndo, TriggerRedo, OpenSearch]);

// "Turn into page" (engine-synthetic `convert_block_to_page`, Option B) as an
// editor keybinding, sitting beside indent/outdent (`IndentInline`/
// `OutdentInline`) which are likewise editor-context actions intercepted by
// `EditorView`'s capture handlers. Bound in the "Input" context so it fires
// only while a block editor is focused; `EditorView::render` supplies the
// per-row `capture_action` that dispatches the op for the focused block.
actions!(holon_gpui, [TurnIntoPage]);

// ── AppModel: Entity-based reactive state ──────────────────────────────────

/// Reactive model backed by `ReactiveEngine`.
///
/// The root layout is watched via `engine.watch(root_uri)`. Sub-blocks
/// (block- and query-backed ReactiveShells) each have their own independent
/// streams. `rebuild()` is only called for the root — sub-blocks update
/// independently.
struct AppModel {
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
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
    /// Root-level ReactiveShell entities (sidebars, main panel), keyed by
    /// block_id.
    root_live_blocks: std::collections::HashMap<String, Entity<views::ReactiveShell>>,
    /// Handle to the root layout's ReactiveView, extracted from `root_vm` each
    /// time it's rebuilt. Used by the viewport observer to push container-query
    /// space updates into the root on window resize / keyboard toggle without
    /// triggering a full tree rebuild. Present iff the current root is a
    /// Reactive variant (i.e. a streaming container like `columns`).
    root_view: Option<Arc<holon_frontend::ReactiveView>>,
    /// Last observed navigation focus. Used to auto-close overlay-mode
    /// (phone) drawers when navigation focus changes — see
    /// [`AppModel::close_overlay_drawers_on_nav`].
    last_focused_block: Option<holon_api::EntityUri>,
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
        self.nav.set_root(self.root_vm.clone());
    }

    /// When navigation focus changes, auto-close any OPEN overlay-mode drawers
    /// (the phone left/right sidebars). Shrink-mode (desktop) sidebars are left
    /// alone — keeping them open after navigation is the correct desktop UX.
    /// Gated on [`DrawerMode::Overlay`], NOT `cfg(feature = "mobile")`, so a
    /// narrow desktop window (which also gets overlay drawers via `if_space`)
    /// behaves identically. Returns true iff at least one drawer was closed.
    ///
    /// Note: keyed on a *change* of focus, so re-selecting the already-focused
    /// page does not re-close the drawer (an accepted edge case).
    fn close_overlay_drawers_on_nav(&mut self) -> bool {
        let focused = self.engine.ui_state().focused_block();
        if focused == self.last_focused_block {
            return false;
        }
        self.last_focused_block = focused;
        let mut closed = false;
        for (bid, mode) in self.view_model.collect_drawers() {
            if matches!(mode, holon_frontend::view_model::DrawerMode::Overlay)
                && self.session.drawer_open(&bid, mode)
            {
                self.session.set_widget_open(&bid, false);
                closed = true;
            }
        }
        closed
    }

    /// Re-point this window's root view at a *different* SUT — a fresh
    /// `session` + `engine` — without re-opening the GPUI window. Used by the
    /// windowed capture-minimizer (`RebindHandle`) to reuse one live window
    /// across many ddmin candidates instead of one process per candidate.
    ///
    /// Drops the per-block `ReactiveShell` entities (their `watch_live`
    /// subscriptions are bound to the *old* engine) and rebuilds the whole root
    /// from the new engine via [`AppModel::rebuild`], which recreates them
    /// against `engine`'s `watch_live` and re-resolves the view model. The
    /// caller must `apply_viewport` + `cx.notify` afterwards (it holds the
    /// `Window`); see [`RebindHandle::rebind`].
    fn rebind(
        &mut self,
        session: Arc<FrontendSession>,
        engine: Arc<ReactiveEngine>,
        viewport: holon_frontend::reactive::ViewportInfo,
        cx: &mut gpui::Context<Self>,
    ) {
        self.session = session;
        self.engine = engine;
        // Seed the new engine's viewport *before* the snapshot so the root
        // `if_space(...)` picks the correct breakpoint on the first rebuilt
        // frame — otherwise the first paint lands on the narrow (overlay)
        // branch and the sidebar overlaps the main panel until the next fire.
        self.engine.ui_state().set_viewport(viewport);
        // Entities (LiveBlockView/ReactiveShell) carry subscriptions to the old
        // engine — drop them so `rebuild` re-creates them against the new one.
        self.root_live_blocks.clear();
        self.rebuild(cx);
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

    /// Walk the root reactive tree to find LiveBlock nodes and create/GC their
    /// entities.
    fn reconcile_root_live_blocks(&mut self, cx: &mut gpui::Context<Self>) {
        let mut needed = std::collections::HashSet::new();
        collect_root_live_blocks(&self.root_vm, &mut needed);

        for block_id in &needed {
            if !self.root_live_blocks.contains_key(block_id) {
                // block_id string from LiveBlock nodes in the root ViewModel tree
                // ALLOW(entity_uri_from_raw): block_id from LiveBlock nodes (boundary)
                let uri = holon_api::EntityUri::from_raw(block_id);
                let services: Arc<dyn BuilderServices> = self.engine.clone();
                let live_block = services.watch_live(&uri, services.clone());
                let render_ctx = RenderContext::default();
                let nav = self.nav.clone();
                let b = self.bounds_registry.clone();
                let bid = block_id.clone();
                let ancestors = entity_view_registry::LiveBlockAncestors::new();
                let entity = cx.new(|cx| {
                    views::ReactiveShell::new_for_block(
                        bid, render_ctx, services, live_block, nav, b, ancestors, cx,
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
            gpui_ctx.nav.clone(),
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
        // Inset so the panel keeps a margin on narrow (phone) viewports; the
        // panel is `w_full` capped at 640px, so this padding is what stops it
        // from touching the screen edges on mobile.
        .p(px(16.0))
        .child(
            div()
                .id(SharedString::from(format!("{id}-panel")))
                .w_full()
                .max_w(px(640.0))
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
    nav: NavigationState,
    pub bounds_registry: BoundsRegistry,
    /// Persistent entity cache for the root render (survives across frames).
    entity_cache: entity_view_registry::EntityCache,
    /// Top safe area inset in logical pixels (status bar on mobile, 0 on
    /// desktop).
    pub safe_area_top: f32,
    /// Bottom safe area inset in logical pixels (home indicator on mobile, 0 on
    /// desktop).
    pub safe_area_bottom: f32,
    /// Share/accept UI state. Shared with `AppModel.share_ui` — lives here too
    /// so the render pass can build overlays without a double-read through
    /// `app_model.read(cx).share_ui.read(cx)`.
    pub share_ui: Entity<share_ui::ShareUiState>,
    /// User-facing search modal (quick-open + full-text content), cmd-K.
    pub search_ui: Entity<search_ui::SearchUiState>,
    /// Page-ancestor breadcrumb for the focused page.
    pub breadcrumb: Entity<breadcrumb::BreadcrumbState>,
    /// Last focus the breadcrumb was resolved for; a change re-resolves it.
    last_breadcrumb_focus: Option<holon_api::EntityUri>,
    /// Live-oracle violations (debug builds): mirrors the global
    /// `holon_oracles` status; rendered as an impossible-to-miss top banner.
    #[cfg(debug_assertions)]
    pub oracle_ui: Entity<oracles_ui::OracleUiState>,
    /// Name of the theme currently applied to the `gpui_component` global.
    /// Compared against the session's selected theme on every render so a
    /// theme change (settings dropdown, or any other path) re-applies live.
    applied_theme: String,
    /// Last-observed `UiState::main_nav_generation`. When it advances, a page
    /// navigation landed in the main region — the main panel's scroll is reset
    /// to the top so the new page opens above the fold (LogSeq parity, dogfood
    /// #5 row 146). Same-page block clicks move `focused_block` but NOT this
    /// counter, so they leave the scroll position untouched.
    last_main_nav_gen: u64,
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
        // Live theme application. The gpui_component `Theme` global is seeded
        // once at launch; the settings dropdown only persists the pref and
        // calls `window.refresh()`. Re-apply here whenever the selected theme
        // differs from what's applied, so a theme switch repaints immediately
        // (and the correct theme is applied on the first frame). Must run
        // before `cx.theme()` is read below.
        let desired_theme = self
            .session
            .ui_settings()
            .theme
            .clone()
            .unwrap_or_else(|| "holonLight".to_string());
        if desired_theme != self.applied_theme {
            apply_holon_theme(&self.session, cx);
            self.applied_theme = desired_theme;
        }
        // Reset main-panel scroll to the top when a page navigation lands in the
        // main region (LogSeq parity, dogfood #5 row 146). `main_nav_generation`
        // advances only on `navigation.focus`(region=main)/`go_home`, NOT on
        // same-page block clicks, so mid-page editing keeps its scroll offset.
        let main_nav_gen = self
            .app_model
            .read(cx)
            .engine
            .ui_state()
            .main_nav_generation();
        if main_nav_gen != self.last_main_nav_gen {
            self.last_main_nav_gen = main_nav_gen;
            if let Some(main_panel) = self
                .app_model
                .read(cx)
                .root_live_blocks
                .get("block:default-main-panel")
                .cloned()
            {
                scroll_reactive_shell_tree_to_top(&main_panel, cx);
            }
        }
        // Re-resolve the page-ancestor breadcrumb whenever the focused block
        // changes. The trail is fetched async (matview-backed) and pumped back
        // into `self.breadcrumb`.
        {
            let focused = self.app_model.read(cx).engine.ui_state().focused_block();
            if focused != self.last_breadcrumb_focus {
                self.last_breadcrumb_focus = focused.clone();
                match focused {
                    Some(block_id) => {
                        let generation = self.breadcrumb.update(cx, |s, _| {
                            s.block_id = Some(block_id.clone());
                            s.error = None;
                            s.generation = s.generation.wrapping_add(1);
                            s.generation
                        });
                        let session = self.session.clone();
                        let rt_handle = self.rt_handle.clone();
                        let state = self.breadcrumb.clone();
                        let wh = window.window_handle();
                        let async_cx = cx.to_async();
                        breadcrumb::resolve_breadcrumb(
                            block_id, generation, session, rt_handle, state, wh, &async_cx,
                        );
                    }
                    None => {
                        self.breadcrumb.update(cx, |s, _| {
                            s.block_id = None;
                            s.segments.clear();
                            s.error = None;
                        });
                    }
                }
            }
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
            let l = LocalEntityScope::new().with_cache(self.entity_cache.clone());
            // Pre-populate the entity cache with root live_block entities.
            // This way the live_block builder finds them in get_or_create and
            // doesn't call watch_live + cx.new() during the render pass.
            for (bid, entity) in &root_refs {
                let key = crate::entity_view_registry::CacheKey::LiveBlock(bid.clone());
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
            self.nav.clone(),
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

        // Root window fill. A flat single color reads as dull; instead paint a
        // very subtle top→bottom luminance sweep DERIVED from the active theme's
        // background token (hue/saturation preserved, so it tracks light and
        // dark themes and any accent tint automatically). The spread is tiny
        // (`BG_GRADIENT_LIGHTNESS_SPREAD`) — a hint of depth, never enough to
        // change text contrast. Glass mode keeps its flat translucent fill.
        let page_background: gpui::Background = if glass {
            bg.into()
        } else {
            let base = theme.background;
            let hi = gpui::Hsla {
                l: (base.l + BG_GRADIENT_LIGHTNESS_SPREAD).min(1.0),
                ..base
            };
            let lo = gpui::Hsla {
                l: (base.l - BG_GRADIENT_LIGHTNESS_SPREAD).max(0.0),
                ..base
            };
            gpui::linear_gradient(
                160.0,
                gpui::linear_color_stop(hi, 0.0),
                gpui::linear_color_stop(lo, 1.0),
            )
        };
        let text = theme.foreground;

        // Drawer (id, mode) pairs from static snapshot (simpler than walking
        // reactive tree). Mode is needed so the toggle reads the same mode-aware
        // default open state the drawer renders with.
        let drawers = view_model.collect_drawers();
        let left_drawer = drawers.first().cloned();
        let right_drawer = if drawers.len() > 1 {
            drawers.last().cloned()
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
        let search_ui_for_btn = self.search_ui.clone();

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
                                if let Some((ref bid, mode)) = left_drawer {
                                    left_model.update(cx, |m, cx| {
                                        let current = m.session.drawer_open(bid, mode);
                                        m.session.set_widget_open(bid, !current);
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
                                if let Some((ref bid, mode)) = right_drawer {
                                    right_model.update(cx, |m, cx| {
                                        let current = m.session.drawer_open(bid, mode);
                                        m.session.set_widget_open(bid, !current);
                                        m.rebuild(cx);
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("search-open")
                            .cursor_pointer()
                            .text_size(px(15.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(gpui::rgba(0x00000010)))
                            .child(icon("🔎"))
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                search_ui_for_btn.update(cx, |s, cx| {
                                    s.open(window, cx);
                                    cx.emit(search_ui::NotifySearchUi);
                                    cx.notify();
                                });
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
                    // Widget Gallery is a dev tool — demoted off the toolbar
                    // outside debug builds (matches the Inspector's gating).
                    .when(cfg!(debug_assertions), |this| {
                        this.child(
                            div()
                                .id("gallery-toggle")
                                .cursor_pointer()
                                .text_size(px(15.0))
                                .px(px(6.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(gpui::rgba(0x00000010)))
                                .child(icon("🎨"))
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    gallery_model.update(cx, |m, cx| {
                                        m.show_widget_gallery = !m.show_widget_gallery;
                                        cx.notify();
                                    });
                                }),
                        )
                    })
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
                            .child(icon("🔗"))
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
                    .when(
                        cfg!(all(debug_assertions, not(feature = "mobile"))),
                        |this| {
                            this.child(
                                div()
                                    .id("inspector-toggle")
                                    .cursor_pointer()
                                    .text_size(px(15.0))
                                    .px(px(6.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(gpui::rgba(0x00000010)))
                                    // 🐞 not 🔎 — the magnifier now opens the
                                    // user search modal; this stays the debug
                                    // inspector (debug builds only).
                                    .child(icon("🐞"))
                                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                        #[cfg(debug_assertions)]
                                        window.toggle_inspector(cx);
                                        #[cfg(not(debug_assertions))]
                                        {
                                            let _ = (window, cx);
                                        }
                                    }),
                            )
                        },
                    ),
            );

        // Page-level chord pump. Cross-block navigation (MoveUp/MoveDown
        // across editor boundaries) and the mouse-up focus mirror live
        // inside `EditorView` (`views/editor_view.rs::handle_cross_block_nav`,
        // `InputEvent::Focus`). The page-level handler below only fires
        // for chords that aren't consumed by an inner editor.
        let content = {
            let nav = self.nav.clone();
            let (session, rt_handle) = {
                let m = self.app_model.read(cx);
                (m.session.clone(), m.rt_handle.clone())
            };
            let window_handle = window.window_handle();
            div()
                .size_full()
                .flex_1()
                .flex()
                .flex_col()
                .overflow_hidden()
                // Engine-level undo/redo. Two action types land here:
                // `gpui_component::input::{Undo, Redo}` resolve while an
                // editor has focus (their "Input"-context binding); our own
                // `TriggerUndo`/`TriggerRedo` (bound with `context: None` in
                // `launch_holon_window_impl`) resolve everywhere else. Both
                // are captured here — top-down, before `InputState`'s own
                // bubble-phase local-text-undo handler — and `stop_propagation`d
                // so the engine op is the only thing that runs; the editor's
                // own undo/redo history is not a substrate we want to diverge
                // from the engine's operation-log undo stack.
                .capture_action({
                    let session = session.clone();
                    let rt_handle = rt_handle.clone();
                    let share_ui = self.share_ui.clone();
                    move |_: &gpui_component::input::Undo, _window, cx: &mut App| {
                        let async_cx = cx.to_async();
                        share_ui::dispatch_undo(
                            session.clone(),
                            rt_handle.clone(),
                            share_ui.clone(),
                            window_handle,
                            &async_cx,
                        );
                        cx.stop_propagation();
                    }
                })
                .capture_action({
                    let session = session.clone();
                    let rt_handle = rt_handle.clone();
                    let share_ui = self.share_ui.clone();
                    move |_: &gpui_component::input::Redo, _window, cx: &mut App| {
                        let async_cx = cx.to_async();
                        share_ui::dispatch_redo(
                            session.clone(),
                            rt_handle.clone(),
                            share_ui.clone(),
                            window_handle,
                            &async_cx,
                        );
                        cx.stop_propagation();
                    }
                })
                .capture_action({
                    let session = session.clone();
                    let rt_handle = rt_handle.clone();
                    let share_ui = self.share_ui.clone();
                    move |_: &TriggerUndo, _window, cx: &mut App| {
                        let async_cx = cx.to_async();
                        share_ui::dispatch_undo(
                            session.clone(),
                            rt_handle.clone(),
                            share_ui.clone(),
                            window_handle,
                            &async_cx,
                        );
                        cx.stop_propagation();
                    }
                })
                .capture_action({
                    let session = session.clone();
                    let rt_handle = rt_handle.clone();
                    let share_ui = self.share_ui.clone();
                    move |_: &TriggerRedo, _window, cx: &mut App| {
                        let async_cx = cx.to_async();
                        share_ui::dispatch_redo(
                            session.clone(),
                            rt_handle.clone(),
                            share_ui.clone(),
                            window_handle,
                            &async_cx,
                        );
                        cx.stop_propagation();
                    }
                })
                .on_key_down({
                    let nav = nav.clone();
                    let session = session.clone();
                    let rt_handle = rt_handle.clone();
                    let services: Arc<dyn BuilderServices> = self.app_model.read(cx).engine.clone();
                    move |event: &gpui::KeyDownEvent, _window, cx: &mut App| {
                        let keys = keystroke_to_keys(&event.keystroke);
                        if keys.is_empty() {
                            return;
                        }
                        // The page-level chord pump targets the
                        // last-focused block — `EditorView::InputEvent::Focus`
                        // mirrors GPUI focus into `services.focused_block()`,
                        // so this is the source of truth without scanning
                        // any registry.
                        let Some(focused_uri) = services.focused_block() else {
                            tracing::debug!("[on_key_down] No focused editor for keys: {keys:?}");
                            return;
                        };
                        let input = WidgetInput::KeyChord { keys: keys.clone() };
                        let action = nav.bubble_input(&focused_uri, &input);
                        tracing::debug!(
                            "[on_key_down] keys={keys:?} focused={focused_uri} action={action:?}"
                        );
                        if let Some(InputAction::ExecuteOperation {
                            entity_name,
                            operation,
                            entity_id,
                        }) = action
                        {
                            let mut params = std::collections::HashMap::new();
                            params.insert(
                                "id".into(),
                                holon_api::Value::String(entity_id.to_string()),
                            );
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
                .child(root)
        };

        let search_theme = search_ui::SearchTheme {
            bg,
            border: border_color,
            fg: text,
            muted_fg: theme.muted_foreground,
            selected_bg: theme.accent,
            selected_fg: theme.accent_foreground,
        };

        // Page-ancestor breadcrumb bar, between the title bar and the content.
        // Full width for v1 (disclosed) — a slim path-back strip under the top
        // bar; clicking a segment navigates via the shared chokepoint.
        let breadcrumb_bar = breadcrumb::render_breadcrumb_bar(
            self.breadcrumb.read(cx),
            services.clone(),
            search_theme,
            16.0,
        );

        let mut page = div()
            .size_full()
            .bg(page_background)
            .text_color(text)
            .flex_col()
            .pt(px(self.safe_area_top))
            .pb(px(self.safe_area_bottom))
            .child(title_bar);
        if let Some(bar) = breadcrumb_bar {
            page = page.child(bar);
        }
        page = page.child(content);

        if let Some(overlay) = settings_overlay {
            page = page.child(overlay);
        }
        if let Some(overlay) = gallery_overlay {
            page = page.child(overlay);
        }

        // User search modal (quick-open + full-text content), cmd-K / 🔎.
        if let Some(overlay) = search_ui::render_search_overlay(
            self.search_ui.read(cx),
            self.search_ui.clone(),
            services.clone(),
            search_theme,
        ) {
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
            let pending_store = cx
                .try_global::<share_ui::PendingWritesGlobal>()
                .map(|g| g.0.clone());
            let share_state_read = self.share_ui.read(cx);
            let overlays = share_ui::render_overlays(
                share_state_read,
                share_state_entity,
                self.session.clone(),
                engine,
                self.rt_handle.clone(),
                wh,
                async_cx,
                pending_store,
                overlay_theme,
            );
            for ov in overlays {
                page = page.child(ov);
            }
        }

        // Live-oracle violation banner (debug builds) — rendered LAST so it
        // sits on top of everything: a violation must be impossible to miss.
        #[cfg(debug_assertions)]
        {
            let oracle_state_read = self.oracle_ui.read(cx);
            if let Some(banner) =
                oracles_ui::render_banner(oracle_state_read, self.oracle_ui.clone())
            {
                page = page.child(banner);
            }
        }

        page.into_any_element()
    }
}

/// Launch a Holon window, creating a new `BoundsRegistry` from the session's
/// theme.
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

/// Launch a Holon window with a pre-created `ReactiveEngine` and
/// `BoundsRegistry`.
///
/// Used by the GPUI PBT test: reuses the PBT's DI-resolved ReactiveEngine so
/// all watch_ui tasks and CDC subscriptions share the same tokio runtime.
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

/// Shared cell holding the window's *currently bound* engine. The interaction
/// pump reads it per command so scroll-into-view targets the rebound engine,
/// not the one captured when the window opened. [`RebindHandle::rebind`] writes
/// it.
type LiveEngine = std::sync::Arc<std::sync::RwLock<Arc<ReactiveEngine>>>;

/// A handle to a live Holon window whose root view can be *re-pointed* at a
/// different SUT (`session` + `engine`) without re-opening the window. Returned
/// by [`launch_holon_window_rebindable`]; used by the windowed
/// capture-minimizer to reuse one window across many ddmin candidates instead
/// of one process each.
pub struct RebindHandle {
    window: AnyWindowHandle,
    app_model: Entity<AppModel>,
    /// `HolonApp`'s persistent render cache. Its panel `ReactiveShell`s are
    /// keyed by *static* panel ids, so the render's `or_insert_with` would
    /// keep the previous engine's shells; rebind clears it so fresh shells
    /// are built.
    entity_cache: entity_view_registry::EntityCache,
    /// The live-engine cell the interaction pump reads (see [`LiveEngine`]).
    live_engine: LiveEngine,
}

impl RebindHandle {
    pub fn window(&self) -> AnyWindowHandle {
        self.window
    }

    /// Re-point the window at `session` + `engine`, re-seed the viewport from
    /// the window's current size, and request a repaint. Must run on the
    /// GPUI main thread (holds `cx: &mut App`).
    pub fn rebind(&self, session: Arc<FrontendSession>, engine: Arc<ReactiveEngine>, cx: &mut App) {
        // Re-point the pump's engine and drop the stale panel shells *before* the
        // render so scroll-into-view and the rebuilt tree both see the new engine.
        *self.live_engine.write().unwrap() = engine.clone();
        self.entity_cache.write().unwrap().clear();

        let app_model = self.app_model.clone();
        let window = self.window;
        let _ = cx.update_window(window, |_, win, cx| {
            let vp = viewport_info_from_window(win.viewport_size(), win.scale_factor());
            app_model.update(cx, |m, cx| {
                m.rebind(session, engine.clone(), vp, cx);
                cx.notify();
            });
            // The root-layout signal pump is bound to the *old* engine's stream;
            // re-point it at the new engine so viewport / structural changes drive
            // the rebound window (and the `if_space` breakpoint re-evaluates).
            spawn_root_layout_signal(app_model.clone(), engine, window, cx);
        });
    }
}

/// Like [`launch_holon_window_with_title`] but returns a [`RebindHandle`] so
/// the caller can re-point the window at fresh SUTs over its lifetime. `None`
/// if the window failed to open.
pub fn launch_holon_window_rebindable(
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
    rt_handle: tokio::runtime::Handle,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    debug: Option<Arc<holon_mcp::server::DebugServices>>,
    title: &str,
    cx: &mut App,
) -> Option<RebindHandle> {
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
    )
    .map(
        |(window, app_model, entity_cache, live_engine)| RebindHandle {
            window,
            app_model,
            entity_cache,
            live_engine,
        },
    )
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

/// Spawn the window's root-layout signal pump on `engine`: every fire (a
/// structural `render_expr` change, or a `ui_generation` bump from
/// `set_viewport`) rebuilds `root_vm`, reconciles the root live blocks, and
/// re-renders — this is what lets the root `if_space(...)` re-pick its
/// breakpoint (sidebar-beside-main vs overlay) when the viewport changes.
///
/// Factored out so [`RebindHandle::rebind`] can re-point it at a *new* engine:
/// the loop is bound to one engine's signal stream and can't swap it, so rebind
/// spawns a fresh loop. Stale loops (on a prior engine) self-suppress via the
/// `Arc::ptr_eq` guard — they no-op once `app_model.engine` is a different Arc
/// — so a late fire from an old engine can't clobber the freshly-bound window.
fn spawn_root_layout_signal(
    app_model: Entity<AppModel>,
    engine: Arc<ReactiveEngine>,
    wh: AnyWindowHandle,
    cx: &mut App,
) {
    let root_uri = holon_api::root_layout_block_uri();
    let root_signal = engine.watch_signal(&root_uri);
    cx.spawn(async move |cx| {
        use futures_signals::signal::SignalExt;
        root_signal
            .for_each(move |rvm| {
                let _ = cx.update_window(wh, |_, _, cx| {
                    app_model.update(cx, |m, cx| {
                        // Only the loop bound to the currently-active engine drives
                        // the window; stale loops (prior rebinds) no-op.
                        if !Arc::ptr_eq(&m.engine, &engine) {
                            return;
                        }
                        m.root_vm = Arc::new(rvm);
                        m.reconcile_root_live_blocks(cx);
                        m.view_model =
                            resolved_view_model(&m.root_vm, &m.engine, &m.root_live_blocks, cx);
                        // Auto-close phone overlay sidebars when navigation
                        // focus changed (e.g. a page tap in the drawer), then
                        // re-resolve so the closed state renders this frame.
                        if m.close_overlay_drawers_on_nav() {
                            m.reconcile_root_live_blocks(cx);
                            m.view_model =
                                resolved_view_model(&m.root_vm, &m.engine, &m.root_live_blocks, cx);
                        }
                        m.nav.set_root(m.root_vm.clone());
                        cx.notify();
                    });
                });
                async {}
            })
            .await;
    })
    .detach();
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
) -> Option<(
    AnyWindowHandle,
    Entity<AppModel>,
    entity_view_registry::EntityCache,
    LiveEngine,
)> {
    gpui_component::init(cx);

    // Context-free undo/redo bindings — see the `actions!(holon_gpui, ...)`
    // comment above for why `gpui_component::input::{Undo, Redo}` alone
    // (context "Input") can't cover the no-editor-focused case.
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-z", TriggerUndo, None),
        KeyBinding::new("cmd-shift-z", TriggerRedo, None),
        // Quick-open / search — cmd-K (free chord; cmd-P is unbound too but
        // cmd-K matches the VS Code / Linear / Slack command-palette idiom).
        KeyBinding::new("cmd-k", OpenSearch, None),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-z", TriggerUndo, None),
        KeyBinding::new("ctrl-y", TriggerRedo, None),
        KeyBinding::new("ctrl-k", OpenSearch, None),
    ]);

    // "Turn into page" — bound in the editor's "Input" context (same context
    // gpui_component binds Tab/Shift-Tab -> IndentInline/OutdentInline in), so
    // it only fires while a block editor is focused. `EditorView`'s per-row
    // `capture_action(&TurnIntoPage)` handles it for the focused block.
    #[cfg(target_os = "macos")]
    cx.bind_keys([KeyBinding::new("cmd-shift-p", TurnIntoPage, Some("Input"))]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([KeyBinding::new("ctrl-shift-p", TurnIntoPage, Some("Input"))]);

    #[cfg(debug_assertions)]
    inspector::init(cx);

    apply_holon_theme(&session, cx);

    let session_clone = Arc::clone(&session);
    let handle_clone = rt_handle.clone();

    let model_entity: Arc<std::sync::OnceLock<Entity<AppModel>>> =
        Arc::new(std::sync::OnceLock::new());
    let model_slot = model_entity.clone();

    // Slot to carry the search-UI entity out of the window-creation closure so
    // the cmd-K `OpenSearch` action handler (registered app-level, after the
    // window exists) can open + focus it.
    let search_entity_slot: Arc<std::sync::OnceLock<Entity<search_ui::SearchUiState>>> =
        Arc::new(std::sync::OnceLock::new());
    let search_entity_slot_for_window = search_entity_slot.clone();

    // Slot to carry the oracle-UI entity out of the window-creation closure
    // so the status bridge (needs the window handle) can be wired after.
    #[cfg(debug_assertions)]
    let oracle_entity_slot: Arc<std::sync::OnceLock<Entity<oracles_ui::OracleUiState>>> =
        Arc::new(std::sync::OnceLock::new());
    #[cfg(debug_assertions)]
    let oracle_entity_slot_for_window = oracle_entity_slot.clone();

    let glass = session_clone
        .ui_settings()
        .glass_background
        .unwrap_or(false);
    let env_bounds = std::env::var("HOLON_INITIAL_WINDOW_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            // ALLOW(ok): dev env-var override; malformed value falls back to default bounds
            let w: f32 = w.trim().parse().ok()?;
            // ALLOW(ok): dev env-var override; malformed value falls back to default bounds
            let h: f32 = h.trim().parse().ok()?;
            Some(gpui::Bounds {
                origin: gpui::point(px(100.0), px(100.0)),
                size: gpui::size(px(w), px(h)),
            })
        });
    // Restore persisted window bounds for the real production window only. PBT /
    // custom-title windows run against temp config dirs and set their own bounds,
    // and the dev HOLON_INITIAL_WINDOW_SIZE override always wins when present.
    let persisted_bounds = if custom_title.is_none() && env_bounds.is_none() {
        window_state::load(session.config_dir()).map(|state| {
            let displays = window_state::connected_displays(cx);
            tracing::info!(
                mode = ?state.mode,
                displays = displays.len(),
                "restoring persisted window bounds"
            );
            state.to_window_bounds(&displays)
        })
    } else {
        None
    };
    let restored_window_bounds =
        persisted_bounds.or_else(|| env_bounds.map(gpui::WindowBounds::Windowed));
    // Config dir to persist window state into — `None` for PBT / custom-title
    // windows so they never write over the user's real window_state.json.
    let persist_config_dir: Option<std::path::PathBuf> = custom_title
        .is_none()
        .then(|| session.config_dir().to_path_buf());
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
        window_bounds: restored_window_bounds,
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
    //
    // Android: NEVER block here. gpui-mobile runs `finish_launching`
    // (and therefore this whole function) on the event-loop thread — the
    // one that must keep pumping to present frames. `fg_executor.block_on`
    // on the foreground executor's own thread is a re-entrancy wedge: the
    // loop stops pumping, the first frame is never presented, and the app
    // shows a permanent black screen. Skip the pre-warm and open with the
    // loading state; the tokio root-layout signal drives the first real
    // repaint asynchronously once the event loop is running.
    #[cfg(not(target_os = "android"))]
    if let Some(ref engine) = existing_engine {
        use futures::StreamExt;
        use futures::future::Either;
        use futures::future::select;
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

    // Pre-create the root entity cache outside the window-creation closure
    // so we can share its `Arc<RwLock<_>>` with `setup_interaction_pump`
    // (which needs to walk the cache hierarchy for scroll-into-view).
    // `HolonApp::entity_cache` clones this same Arc, so both ends observe
    // the same map.
    let entity_cache: entity_view_registry::EntityCache = Default::default();
    let entity_cache_for_view = entity_cache.clone();
    let window_result = cx.open_window(window_options, move |window, cx| {
        tracing::debug!("[GPUI] Inside open_window callback — building root view");
        let close_persist_dir = persist_config_dir.clone();
        window.on_window_should_close(cx, move |window, cx| {
            // Final save on clean shutdown — captures the last position/size
            // even if it changed within the resize debounce window.
            if let Some(dir) = close_persist_dir.as_deref() {
                let state = window_state::PersistedWindowState::from_window(window, cx);
                if let Err(e) = window_state::save(dir, &state) {
                    tracing::warn!(error = %e, "persisting window state on close failed");
                }
            }
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
                services_slot.clone(),
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
            nav.set_block_resolver(std::sync::Arc::new(
                move |block_id: &holon_api::EntityUri| {
                    Some(std::sync::Arc::new(
                        engine_for_resolver.snapshot_reactive(block_id),
                    ))
                },
            ));
        }

        let shadow_ctx = RenderContext::default();

        let initial_root_view = root_reactive_view(&root_vm);
        let share_ui_entity = cx.new(|_cx| share_ui::ShareUiState::new());
        let search_ui_entity = cx.new(|cx| search_ui::SearchUiState::new(window, cx));
        search_entity_slot_for_window
            .set(search_ui_entity.clone())
            .ok();
        let breadcrumb_entity = cx.new(|_cx| breadcrumb::BreadcrumbState::default());
        #[cfg(debug_assertions)]
        let oracle_ui_entity = cx.new(|_cx| oracles_ui::OracleUiState::default());
        #[cfg(debug_assertions)]
        oracle_entity_slot_for_window
            .set(oracle_ui_entity.clone())
            .ok();
        let app_model = cx.new(|cx| {
            let mut model = AppModel {
                session: Arc::clone(&session_clone),
                engine: engine.clone(),
                rt_handle: handle_clone.clone(),
                nav: nav.clone(),
                bounds_registry: bounds_registry.clone(),
                root_vm: Arc::new(root_vm),
                view_model,
                shadow_ctx,
                show_settings: false,
                show_widget_gallery: false,
                share_ui: share_ui_entity.clone(),
                root_live_blocks: std::collections::HashMap::new(),
                root_view: initial_root_view,
                last_focused_block: None,
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
        let app_model_for_view = app_model.clone();
        let bounds_persist_dir = persist_config_dir.clone();
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
            let mut last_bounds_save: Option<std::time::Instant> = None;
            cx.observe_window_bounds(window, move |_this, window, cx| {
                let vp = viewport_info_from_window(window.viewport_size(), window.scale_factor());
                app_model_for_view.update(cx, |m, _cx| m.apply_viewport(vp));
                // Debounced persistence: this fires per pixel during a drag /
                // resize, so throttle disk writes. The on-close handler covers
                // whatever change lands inside the trailing window.
                if let Some(dir) = bounds_persist_dir.as_deref() {
                    let now = std::time::Instant::now();
                    let due = last_bounds_save
                        .map(|t| now.duration_since(t) >= std::time::Duration::from_millis(800))
                        .unwrap_or(true);
                    if due {
                        last_bounds_save = Some(now);
                        let state = window_state::PersistedWindowState::from_window(window, cx);
                        if let Err(e) = window_state::save(dir, &state) {
                            tracing::warn!(error = %e, "persisting window state failed");
                        }
                    }
                }
            })
            .detach();

            // Re-render the HolonApp whenever ShareUiState emits NotifyShareUi.
            // Without this the share/accept/quarantine modals would not appear
            // until the next unrelated render pass.
            cx.subscribe(
                &share_ui_entity,
                move |_, _, _: &share_ui::NotifyShareUi, cx| {
                    cx.notify();
                },
            )
            .detach();

            // Re-render + re-search whenever the search modal state changes.
            cx.subscribe(
                &search_ui_entity,
                move |_, _, _: &search_ui::NotifySearchUi, cx| {
                    cx.notify();
                },
            )
            .detach();
            cx.subscribe(
                &breadcrumb_entity,
                move |_, _, _: &breadcrumb::NotifyBreadcrumb, cx| {
                    cx.notify();
                },
            )
            .detach();

            // Search input → async query. Each keystroke bumps `generation`
            // (stale-response guard) and kicks off `run_search`, whose result
            // pumps back into the search entity.
            let search_input = search_ui_entity.read(cx).input.clone();
            cx.subscribe_in(
                &search_input,
                window,
                move |this: &mut HolonApp,
                      _input,
                      event: &gpui_component::input::InputEvent,
                      window,
                      cx| {
                    if matches!(event, gpui_component::input::InputEvent::Change) {
                        let query = this.search_ui.read(cx).input.read(cx).value().to_string();
                        let generation = this.search_ui.update(cx, |s, _| {
                            s.query = query.clone();
                            s.generation = s.generation.wrapping_add(1);
                            s.generation
                        });
                        let session = this.session.clone();
                        let rt_handle = this.rt_handle.clone();
                        let state = this.search_ui.clone();
                        let wh = window.window_handle();
                        let async_cx = cx.to_async();
                        search_ui::run_search(
                            query, generation, session, rt_handle, state, wh, &async_cx,
                        );
                    }
                },
            )
            .detach();

            // Re-render whenever the live-oracle status changes, so a
            // violation banner appears the moment an oracle fires.
            #[cfg(debug_assertions)]
            cx.subscribe(
                &oracle_ui_entity,
                move |_, _, _: &oracles_ui::NotifyOracleUi, cx| {
                    cx.notify();
                },
            )
            .detach();

            HolonApp {
                session: session_clone,
                rt_handle: handle_clone,
                app_model,
                nav,
                bounds_registry,
                entity_cache: entity_cache_for_view,
                safe_area_top: 0.0,
                safe_area_bottom: 0.0,
                share_ui: share_ui_entity,
                search_ui: search_ui_entity,
                breadcrumb: breadcrumb_entity,
                last_breadcrumb_focus: None,
                #[cfg(debug_assertions)]
                oracle_ui: oracle_ui_entity.clone(),
                applied_theme: String::new(),
                last_main_nav_gen: 0,
            }
        });
        let any_view: AnyView = view.into();
        cx.new(|cx| gpui_component::Root::new(any_view, window, cx))
    });
    let window_handle = match window_result {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("[GPUI] cx.open_window failed: {e:?}");
            return None;
        }
    };
    tracing::debug!("[GPUI] Window opened, starting reactive stream...");

    // HOLON_GPUI_FORCE_ACTIVE=1 — make the window key/active at startup.
    // Background-launched test runs usually get a NON-key window; runs whose
    // window IS key (the realistic foreground-user condition) showed a
    // deterministic split-at-wrong-caret divergence (2026-06-11). This flag
    // pins the activation state so that condition reproduces on demand.
    if std::env::var("HOLON_GPUI_FORCE_ACTIVE").as_deref() == Ok("1") {
        cx.activate(true);
        let _ = window_handle.update(cx, |_, window, _| window.activate_window());
    }

    let app_model = model_entity.get().unwrap().clone();
    let wh: AnyWindowHandle = window_handle.into();

    // App-level handlers for TriggerUndo/TriggerRedo. The page-level
    // `capture_action` handlers in `HolonApp::render` only sit on the action
    // dispatch path while some element inside the content div has window
    // focus; with NO focus (fresh boot, focus cleared by Escape) the dispatch
    // path is just the window root, so element listeners never see the
    // action and cmd-z used to fall through to "No handler matched the key
    // chord". Global `cx.on_action` handlers run at the end of the bubble
    // phase precisely when no element consumed the action. `stop_propagation`
    // also means that on a rebind (a second `launch_holon_window_impl` in the
    // same App) only the newest registration runs — global listeners are
    // invoked newest-first and the break on `!propagate_event` skips stale
    // ones pointing at the previous window.
    {
        let session_for_undo = app_model.read(cx).session.clone();
        let rt_for_undo = rt_handle.clone();
        let share_ui_for_undo = app_model.read(cx).share_ui.clone();
        cx.on_action(move |_: &TriggerUndo, cx: &mut App| {
            let async_cx = cx.to_async();
            share_ui::dispatch_undo(
                session_for_undo.clone(),
                rt_for_undo.clone(),
                share_ui_for_undo.clone(),
                wh,
                &async_cx,
            );
            cx.stop_propagation();
        });
        let session_for_redo = app_model.read(cx).session.clone();
        let rt_for_redo = rt_handle.clone();
        let share_ui_for_redo = app_model.read(cx).share_ui.clone();
        cx.on_action(move |_: &TriggerRedo, cx: &mut App| {
            let async_cx = cx.to_async();
            share_ui::dispatch_redo(
                session_for_redo.clone(),
                rt_for_redo.clone(),
                share_ui_for_redo.clone(),
                wh,
                &async_cx,
            );
            cx.stop_propagation();
        });
    }

    // App-level cmd-K handler: open + focus the search modal. Registered
    // globally (like undo/redo) so it fires regardless of which element holds
    // focus. Needs the window to focus the input, so it hops through
    // `update_window`.
    if let Some(search_entity) = search_entity_slot.get().cloned() {
        cx.on_action(move |_: &OpenSearch, cx: &mut App| {
            let search_entity = search_entity.clone();
            let _ = cx.update_window(wh, move |_, window, cx| {
                search_entity.update(cx, |s, cx| {
                    s.open(window, cx);
                    cx.emit(search_ui::NotifySearchUi);
                    cx.notify();
                });
            });
            cx.stop_propagation();
        });
    }

    // Root layout signal — structural changes only (render_expr).
    // Does NOT react to ui_generation (focus/view_mode) — the root
    // layout is a static columns container whose structure doesn't
    // depend on which block is focused. This avoids the full
    // HolonApp re-render cascade (269 EditorView renders) on every
    // arrow key press.
    let engine = app_model.read(cx).engine.clone();
    // The live-engine cell the interaction pump reads; rebind re-points it.
    let live_engine: LiveEngine = std::sync::Arc::new(std::sync::RwLock::new(engine.clone()));

    if let Some(ref debug) = debug {
        let async_cx = cx.to_async();
        setup_interaction_pump(
            debug,
            window_handle.into(),
            &async_cx,
            bounds_registry_for_pump,
            live_engine.clone(),
            entity_cache.clone(),
        );
    }

    // Wire the live-oracle status bridge (debug builds): global OracleStatus
    // changes → OracleUiState entity → top banner. The runner itself is
    // spawned in main.rs (plain tokio, no GPUI needed); this only wires the
    // surfacing. Gated on the same env switch as the runner.
    #[cfg(debug_assertions)]
    if holon_oracles::OracleMode::from_env().enabled() {
        let async_cx = cx.to_async();
        let oracle_ui_entity = oracle_entity_slot
            .get()
            .expect("oracle entity slot must be populated by the window closure")
            .clone();
        oracles_ui::spawn_oracle_bridge(
            &rt_handle,
            oracle_ui_entity,
            window_handle.into(),
            &async_cx,
        );
    }

    // Install the DegradedToastSink global so any view (e.g. a failed
    // slash-command) can surface a toast without plumbing the ShareUiState
    // entity through every builder. Unconditional — the share_ui entity always
    // exists, independent of the (optional) iroh share backend.
    {
        let toast_ui_entity = app_model.read(cx).share_ui.clone();
        let toast_window_handle: AnyWindowHandle = window_handle.into();
        cx.set_global(share_ui::DegradedToastSink::new(
            move |toast, cx: &mut App| {
                let _ = toast_window_handle.update(cx, |_, _window, cx| {
                    toast_ui_entity.update(cx, |s, cx| {
                        s.push_toast(toast);
                        cx.emit(share_ui::NotifyShareUi);
                        cx.notify();
                    });
                });
            },
        ));
    }

    // Wire the pending connector-write bus bridge (leases/read-write ruling,
    // increment 4c). The shared store is installed as a GPUI global in `main.rs`
    // from the DI-resolved handle; when MCP integrations are absent the global
    // is missing and no bridge is spawned (no once_only writes are possible).
    if let Some(pending) = cx.try_global::<share_ui::PendingWritesGlobal>().cloned() {
        let async_cx = cx.to_async();
        let pending_ui_entity = app_model.read(cx).share_ui.clone();
        share_ui::spawn_pending_writes_bridge(
            pending.0.clone(),
            rt_handle.clone(),
            pending_ui_entity,
            window_handle.into(),
            &async_cx,
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
    spawn_root_layout_signal(app_model.clone(), engine, wh, cx);

    // (The top-level `editor_cursor → focused_block` bridge was removed in
    // ADR 0010. Split/join focus now flows in-process from the op response to
    // `focused_block`; window focus follows that signal. There is no longer
    // any `editor_cursor` CDC to bridge.)

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
    Some((
        window_handle.into(),
        model_entity.get().unwrap().clone(),
        entity_cache,
        live_engine,
    ))
}

/// Return the set of widget names this GPUI frontend supports.
pub fn render_supported_widgets() -> std::collections::HashSet<String> {
    let mut widgets: std::collections::HashSet<String> = render::builders::builder_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    // Collection layouts are handled via ReactiveShell, not individual GPUI
    // builders. They must be in the supported set so profile variant filtering
    // doesn't drop them.
    for name in ["table", "tree", "list", "outline", "columns"] {
        widgets.insert(name.to_string());
    }
    widgets
}

pub fn is_theme_dark(session: &FrontendSession) -> bool {
    load_theme_def(session).is_dark
}

/// Dispatch an interaction event into the GPUI window.
///
/// Uses `dispatch_keystroke` for key events (which calls `dispatch_input` on
/// the input handler for text insertion) and `dispatch_event` for mouse events.
/// Wire up the MCP → GPUI interaction channel.
///
/// Creates a sync channel, registers the sender on `DebugServices`, and spawns
/// an async pump that forwards `InteractionCommand`s to the GPUI window.
pub fn setup_interaction_pump(
    debug: &std::sync::Arc<holon_mcp::server::DebugServices>,
    window_handle: AnyWindowHandle,
    cx: &gpui::AsyncApp,
    bounds_registry: BoundsRegistry,
    engine: LiveEngine,
    entity_cache: entity_view_registry::EntityCache,
) {
    let (tx, mut rx) = futures::channel::mpsc::channel::<holon_mcp::server::InteractionCommand>(16);
    debug.interaction_tx.set(tx.clone()).ok();

    // Install the channel-based `UserDriver` so MCP tools can dispatch
    // UI mutations through the same pipeline as click/key/scroll. The MCP-facing
    // driver binds the *current* engine at install time (re-pointing it on rebind
    // is out of scope — MCP isn't driven during minimization).
    // Flush-on-read: the MCP driver reads geometry from a window that may have
    // gone idle after its last paint (iOS) — see `FlushOnReadGeometry`.
    let geometry: Arc<dyn holon_frontend::geometry::GeometryProvider> =
        Arc::new(geometry::FlushOnReadGeometry(bounds_registry));
    let driver: Arc<dyn holon_frontend::user_driver::UserDriver> = Arc::new(
        user_driver::GpuiUserDriver::new(tx, geometry, engine.read().unwrap().clone()),
    );
    debug.user_driver.set(driver).ok();

    let engine_for_pump = engine;
    let entity_cache_for_pump = entity_cache;
    cx.spawn({
        async move |cx| {
            use futures::StreamExt;
            while let Some(cmd) = rx.next().await {
                if let holon_mcp::server::InteractionEvent::CaptureScreenshot = &cmd.event {
                    // Capture the last rendered frame off the swapchain via the
                    // platform's `render_to_image` (offscreen wgpu readback on
                    // Android). Fail loud: a render/readback error is surfaced in
                    // `detail`, never a blank image.
                    let captured =
                        cx.update_window(window_handle, |_, window, _cx| window.render_to_image());
                    let response = match captured {
                        Ok(Ok(img)) => holon_mcp::server::InteractionResponse {
                            handled: true,
                            detail: None,
                            screenshot: Some(holon_mcp::server::CapturedImage {
                                width: img.width(),
                                height: img.height(),
                                rgba: img.into_raw(),
                            }),
                        },
                        Ok(Err(e)) => holon_mcp::server::InteractionResponse {
                            handled: false,
                            detail: Some(format!("render_to_image failed: {e:#}")),
                            screenshot: None,
                        },
                        Err(e) => holon_mcp::server::InteractionResponse {
                            handled: false,
                            detail: Some(format!("window update failed during capture: {e}")),
                            screenshot: None,
                        },
                    };
                    cmd.response_tx.send(response).ok();
                    continue;
                }
                let result = cx.update_window(window_handle, |_, window, cx| {
                    use holon_mcp::server::InteractionEvent;
                    match &cmd.event {
                        InteractionEvent::ScrollEntityIntoView { entity_id } => {
                            // Read the *live* engine: after a rebind the window is
                            // bound to a new engine, and scroll-into-view must look
                            // up the entity in it, not the one captured at install.
                            let cur = engine_for_pump.read().unwrap().clone();
                            let scrolled = scroll_entity_into_view(
                                entity_id,
                                &cur,
                                &entity_cache_for_pump,
                                window,
                                cx,
                            );
                            // Same occluded-window rationale as the input
                            // branch below: force a frame so the scroll's
                            // effect (and any pending notify) reaches the
                            // committed BoundsRegistry.
                            window.refresh();
                            scrolled.map(|s| (s, None))
                        }
                        InteractionEvent::ScrollList { entity_id, dy, .. } => {
                            // Drive the target panel's `ListState::scroll_by`
                            // directly — the reliable path a synthetic
                            // `ScrollWheel` can't take (hover-gate no-op).
                            let scrolled =
                                scroll_list_by(entity_id, *dy, &entity_cache_for_pump, cx);
                            window.refresh();
                            match scrolled {
                                Ok(true) => Ok((true, None)),
                                Ok(false) => Ok((
                                    false,
                                    Some(format!(
                                        "scroll: no scrollable list reached for {entity_id:?} — \
                                         the entity is neither a rendered `block:default-*` panel \
                                         with a virtualized list nor a block inside one"
                                    )),
                                )),
                                Err(detail) => Err(detail),
                            }
                        }
                        _ => {
                            // Synthetic key events route through the window's
                            // focus tree: when this window is not the key
                            // window (foreground user activity or another
                            // test window de-keyed it), the focused editor is
                            // blurred and keystrokes are silently dropped or
                            // misrouted. Detect at dispatch time so the log
                            // and any unconsumed-key failure self-identify as
                            // key-focus contamination instead of masquerading
                            // as a UI bug.
                            let detail = if !window.is_window_active() {
                                // NOT an input-delivery problem: dispatch_keystroke
                                // never gates on key status (verified in gpui
                                // source + empirically, 2026-06-11). The risk is
                                // the deactivation BLUR: gpui re-renders with an
                                // empty focus path, so a mid-typing blur re-seeds
                                // the caret (displayed-text rotation face). Data
                                // loss is prevented by commit-on-authority-move.
                                let msg = format!(
                                    "[interaction-pump] WINDOW-INACTIVE while dispatching {:?} — \
                                     input IS delivered, but deactivation blur may have re-seeded \
                                     the caret mid-typing (caret-position faces possible; data \
                                     loss is not)",
                                    cmd.event
                                );
                                eprintln!("{msg}");
                                Some(msg)
                            } else {
                                None
                            };
                            let handled = dispatch_interaction(&cmd.event, window, cx);
                            // Force a frame after every synthetic input so the
                            // BoundsRegistry commit-notify fires even when the
                            // window is occluded — test wait-loops pace on
                            // committed frames, and an idle window would
                            // otherwise degrade every wait to its timeout cap.
                            window.refresh();
                            Ok((handled, detail))
                        }
                    }
                });
                let response = match result {
                    Ok(Ok((handled, detail))) => holon_mcp::server::InteractionResponse {
                        handled,
                        detail,
                        screenshot: None,
                    },
                    Ok(Err(detail)) => holon_mcp::server::InteractionResponse {
                        handled: false,
                        detail: Some(detail),
                        screenshot: None,
                    },
                    Err(e) => holon_mcp::server::InteractionResponse {
                        handled: false,
                        detail: Some(e.to_string()),
                        screenshot: None,
                    },
                };
                cmd.response_tx.send(response).ok();
            }
        }
    })
    .detach();
}

/// Scroll a virtualized list shell so the named `entity_id` becomes
/// visible. Returns `Ok(true)` when scrolled, `Ok(false)` if not in any
/// virtualized list (caller should keep polling bounds and rely on the
/// timeout as the authoritative failure signal), or `Err(detail)` if the
/// lookup couldn't be performed.
///
/// Approach: for each `block:default-*` panel, walk the panel shell's
/// local `entity_cache` for cached list-mode `ReactiveShell`s and ask
/// each one directly for the entity's row index
/// ([`ReactiveShell::visible_index_of`]); on a hit, call
/// `list_state.scroll_to_reveal_item(ix); cx.notify();`.
///
/// The shells are queried directly — NOT looked up via
/// `CacheKey::ReactiveShell(view.stable_cache_key())` of a fresh
/// `engine.snapshot_reactive` tree. A fresh snapshot builds new
/// `ReactiveView` instances whose cache keys never match the rendered
/// shell's, so the old lookup silently missed for the main panel and
/// rows below the viewport stayed unreachable (2026-06-11 missing-row
/// root cause: virtualized list + broken scroll-to-reveal lookup).
///
/// All collection panels virtualize (the Main panel's block-mode shell
/// falls through to the same `ReactiveShell` + `gpui::list` path as the
/// sidebars), so off-viewport rows have no bounds until scrolled to —
/// this function is the only way to give them any.
fn scroll_entity_into_view(
    entity_id: &str, // MCP boundary — parsed below, fail-loud
    _engine: &Arc<ReactiveEngine>, /* ALLOW(unused_param): kept for signature stability of the
                      * interaction pump */
    entity_cache: &entity_view_registry::EntityCache,
    _window: &mut Window, /* ALLOW(unused_param): signature parity with other scroll helpers in
                           * this module */
    cx: &mut App,
) -> Result<bool, String> {
    use crate::entity_view_registry::CacheKey;
    use crate::views::ReactiveShell;

    let entity_uri = holon_api::EntityUri::parse(entity_id)
        .map_err(|e| format!("scroll_entity_into_view: {entity_id:?} is not an EntityUri: {e}"))?;
    for panel_id in [
        "block:default-left-sidebar",
        "block:default-main-panel",
        "block:default-right-sidebar",
    ] {
        // The list-mode `ReactiveShell`s live in the panel shell's local
        // `entity_cache`, NOT the top-level cache.
        // Walk: top-level → panel shell → its `entity_cache` → list shells.
        let panel_shell: Option<gpui::Entity<ReactiveShell>> = {
            let cache = entity_cache.read().unwrap();
            cache
                .get(&CacheKey::LiveBlock(panel_id.to_string()))
                .and_then(|any| any.clone().downcast::<ReactiveShell>().ok()) // ALLOW(ok): downcast Err means the cached Any wasn't a ReactiveShell — treat as cache miss and fall through to the next panel
        };
        let Some(panel_shell) = panel_shell else {
            // Panel not rendered as a live_block yet. Continue scanning
            // other panels rather than reporting an error.
            eprintln!(
                "[scroll_entity_into_view] {panel_id}: no LiveBlock panel shell in top-level cache"
            );
            continue;
        };
        let panel_cache = panel_shell.read(cx).entity_cache_clone();
        let list_shells: Vec<gpui::Entity<ReactiveShell>> = {
            let cache = panel_cache.read().unwrap();
            cache
                .values()
                // ALLOW(filter_map_ok): downcast Err just means this cache entry isn't a
                // ReactiveShell — skipping is the semantics, not error hiding
                .filter_map(|any| any.clone().downcast::<ReactiveShell>().ok()) // ALLOW(ok): see filter_map_ok above
                .collect()
        };
        for list_shell in list_shells {
            let Some(ix) = list_shell.read(cx).visible_index_of(&entity_uri) else {
                continue;
            };
            list_shell.update(cx, |shell, cx| {
                let state = shell.list_state_handle();
                let before = state.logical_scroll_top();
                state.scroll_to_reveal_item(ix);
                eprintln!(
                    "[scroll-reveal] {entity_id} ix={ix} scroll_top_before={:?} (snap-back \
                     diagnosis: compare consecutive reveals for the same entity — a repeating \
                     `before` offset means something resets the viewport between frames)",
                    before
                );
                cx.notify();
            });
            return Ok(true);
        }
    }
    Ok(false)
}

/// Scroll a virtualized panel list by a pixel `dy`, driving its
/// `ListState::scroll_by` directly. Counterpart to [`scroll_entity_into_view`]
/// (which reveals a specific row); this applies a relative wheel/trackpad-style
/// delta without going through the platform `ScrollWheel` path, which no-ops
/// for a synthetic off-cursor event (gpui's `should_handle_scroll` hover gate).
///
/// `entity_id` is resolved against the same panel walk as
/// `scroll_entity_into_view`: a `block:default-*` panel scrolls its primary
/// (first) list-mode shell; any other block scrolls the list shell that
/// contains it (`visible_index_of` hit). Returns `Ok(false)` when no scrollable
/// list matches — the caller turns that into a loud error rather than a silent
/// success.
///
/// `pub` for the fail-loud regression test (`tests/mcp_scroll_fail_loud.rs`),
/// which exercises the unreachable-target → `Ok(false)` contract.
/// Reset a panel shell and every list-mode shell nested under it back to the
/// top. Used on cross-page navigation into the main region so the new page
/// opens above the fold (LogSeq parity). `panel_shell` is the block-mode shell
/// (its own `list_state` is a no-op); the scrollable state lives in the nested
/// list shells cached under it — the same descent `scroll_list_by` uses.
fn scroll_reactive_shell_tree_to_top(
    panel_shell: &gpui::Entity<views::ReactiveShell>,
    cx: &mut App,
) {
    use crate::views::ReactiveShell;
    let panel_cache = panel_shell.read(cx).entity_cache_clone();
    let list_shells: Vec<gpui::Entity<ReactiveShell>> = {
        let cache = panel_cache.read().unwrap();
        cache
            .values()
            .filter_map(|any| any.clone().downcast::<ReactiveShell>().ok()) // ALLOW(filter_map_ok): non-ReactiveShell entries are skipped, not errors — ALLOW(ok)
            .collect()
    };
    for list_shell in list_shells {
        list_shell.update(cx, |shell, cx| {
            shell.scroll_to_top();
            cx.notify();
        });
    }
    panel_shell.update(cx, |shell, cx| {
        shell.scroll_to_top();
        cx.notify();
    });
}

pub fn scroll_list_by(
    entity_id: &str,
    dy: f32,
    entity_cache: &entity_view_registry::EntityCache,
    cx: &mut App,
) -> Result<bool, String> {
    use crate::entity_view_registry::CacheKey;
    use crate::views::ReactiveShell;

    let entity_uri = holon_api::EntityUri::parse(entity_id)
        .map_err(|e| format!("scroll_list_by: {entity_id:?} is not an EntityUri: {e}"))?;
    for panel_id in [
        "block:default-left-sidebar",
        "block:default-main-panel",
        "block:default-right-sidebar",
    ] {
        let panel_shell: Option<gpui::Entity<ReactiveShell>> = {
            let cache = entity_cache.read().unwrap();
            cache
                .get(&CacheKey::LiveBlock(panel_id.to_string()))
                .and_then(|any| any.clone().downcast::<ReactiveShell>().ok()) // ALLOW(ok): downcast Err = cached Any wasn't a ReactiveShell; treat as miss
        };
        let Some(panel_shell) = panel_shell else {
            continue;
        };
        // Targeting the panel itself scrolls its primary list; targeting a
        // block scrolls whichever of the panel's list shells contains it.
        let target_is_panel = entity_id == panel_id;
        let panel_cache = panel_shell.read(cx).entity_cache_clone();
        let list_shells: Vec<gpui::Entity<ReactiveShell>> = {
            let cache = panel_cache.read().unwrap();
            cache
                .values()
                // ALLOW(filter_map_ok): non-ReactiveShell entries are skipped, not errors.
                // ALLOW(ok): a downcast miss is a type mismatch to skip, not a swallowed error
                .filter_map(|any| any.clone().downcast::<ReactiveShell>().ok())
                .collect()
        };
        for list_shell in list_shells {
            let matches =
                target_is_panel || list_shell.read(cx).visible_index_of(&entity_uri).is_some();
            if !matches {
                continue;
            }
            list_shell.update(cx, |shell, cx| {
                shell.list_state_handle().scroll_by(gpui::px(dy));
                cx.notify();
            });
            return Ok(true);
        }
    }
    Ok(false)
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
        InteractionEvent::InsertText { text } => dispatch_insert_text(text, window, cx),
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
/// MouseClick produces both MouseDown + MouseUp (GPUI needs both for click
/// handlers).
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
        InteractionEvent::ScrollEntityIntoView { .. }
        | InteractionEvent::ScrollList { .. }
        | InteractionEvent::InsertText { .. }
        | InteractionEvent::CaptureScreenshot => {
            // Handled directly by the interaction pump's match arms
            // (`scroll_entity_into_view` / `scroll_list_by` / `dispatch_insert_text`),
            // not by synthesizing a platform input. Returning an empty vec keeps
            // this fn's callers (which iterate inputs) a no-op for these
            // variants.
            vec![]
        }
    }
}

/// Deliver text the way a soft keyboard's `insertText:` does: bypass the GPUI
/// keymap and commit the string straight into the focused editor's input
/// handler. This mirrors `gpui-mobile`'s `IosWindow::handle_text_input` so the
/// harness can exercise the soft-keyboard input path that `type_text`'s
/// `KeyDown` route cannot reach.
///
/// A soft `Return` arrives as `"\n"`/`"\r"`/`"\r\n"`; the real soft keyboard
/// (post-fix) translates it into an `enter` action rather than inserting a
/// literal newline, so we do the same here — otherwise driving a soft Return
/// through this path would split nothing.
///
/// MOBILE-FIDELITY NOTE: on the `mobile` build this still routes through GPUI's
/// public `Window` API (`dispatch_keystroke` with a `key_char`), which reaches
/// the SAME `EntityInputHandler::replace_text_in_range` that
/// `IosWindow::handle_text_input` ultimately calls — it does NOT traverse the
/// Objective-C `insertText:` FFI glue itself. Covering that final hop requires
/// a `gpui-mobile` fork addition: a `pub fn gpui_mobile::insert_text(&str)`
/// that reaches `ios::ffi::IOS_WINDOW_LIST` and calls a `&str` variant of the
/// (currently `pub(crate)`) `IosWindow::handle_text_input`. Until that hook
/// lands, this rung covers the editor-side behavior (the class of the escaped
/// soft-Return bug) but not the fork's own FFI translation.
fn dispatch_insert_text(text: &str, window: &mut Window, cx: &mut App) -> bool {
    #[cfg(feature = "mobile")]
    tracing::warn!(
        "insert_text on the mobile build routes through GPUI's Window input handler, NOT the \
         Objective-C insertText: FFI (IosWindow::handle_text_input) — that final hop needs a \
         gpui_mobile::insert_text fork hook (see dispatch_insert_text doc comment)"
    );

    if matches!(text, "\n" | "\r" | "\r\n") {
        // Soft Return → `enter` action, mirroring handle_text_input's
        // Return-translation so the editor's Enter capture (split_block) fires.
        let ks = gpui::Keystroke {
            modifiers: gpui::Modifiers::default(),
            key: "enter".to_string(),
            key_char: None,
        };
        return window.dispatch_keystroke(ks, cx);
    }

    // Commit the text through the focused element's input handler. A keystroke
    // carrying `key_char` drives GPUI's `input_handler.dispatch_input` →
    // `replace_text_in_range` — the same editor entry point the soft keyboard
    // reaches — without matching the keymap character-by-character.
    let ks = gpui::Keystroke {
        modifiers: gpui::Modifiers::default(),
        key: text.to_string(),
        key_char: Some(text.to_string()),
    };
    window.dispatch_keystroke(ks, cx)
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
    // Default must match the preferences schema default ("holonLight",
    // preferences.rs) so the settings UI and the renderer agree on a fresh
    // install (no `ui.theme` set) — otherwise the modal shows Light while the
    // renderer applies Dark.
    let name = ui.theme.as_deref().unwrap_or("holonLight");
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

// ── Icon-font coverage tests ─────────────────────────────────────────────────
//
// Guard the Android icon fix. Every icon glyph the app renders must be
// Android-renderable: covered directly by the embedded DejaVu Sans font, or
// swapped for a covered glyph via `ICON_SUBSTITUTES`, or a documented
// `KNOWN_ANDROID_GLYPH_GAPS` entry. The name→glyph tables
// (`op_button::OP_ICONS`, `icon::ICON_CHARS`) are swept by co-located tests in
// those modules via `assert_icon_renderable_on_android`; here we sweep the
// inline literals (`INLINE_UI_GLYPHS`) and check the substitution table's own
// invariants. These run host-side (parsing only the embedded font bytes), so
// `cargo test -p holon-gpui` on macOS/Linux catches a truncated/wrong font
// asset or an unrenderable/unnecessary substitute before it ever reaches a
// device.
#[cfg(test)]
mod icon_font_tests {
    use ttf_parser::Face;

    use super::ICON_SUBSTITUTES;
    use super::INLINE_UI_GLYPHS;

    const DEJAVU_SANS: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");

    fn face() -> Face<'static> {
        Face::parse(DEJAVU_SANS, 0).expect("embedded DejaVu Sans must parse")
    }

    /// Every inline UI glyph literal (toolbar, chevrons, checkboxes, banners)
    /// must render on Android — covered by DejaVu directly or
    /// substitution-routed.
    #[test]
    fn inline_ui_glyphs_render_on_android() {
        for glyph in INLINE_UI_GLYPHS {
            super::assert_icon_renderable_on_android(glyph, "INLINE_UI_GLYPHS");
        }
    }

    /// Every substitute must be a glyph DejaVu actually has, and the source
    /// glyph it replaces must NOT be in DejaVu — otherwise the substitution is
    /// either broken (tofu substitute) or unnecessary (source was covered).
    #[test]
    fn substitutes_are_covered_and_needed() {
        let face = face();
        for (from, sub) in ICON_SUBSTITUTES {
            let sub_char = sub.chars().next().expect("substitute is non-empty");
            assert!(
                face.glyph_index(sub_char).is_some(),
                "substitute {sub:?} (U+{:04X}) for {from:?} not covered by DejaVu Sans",
                sub_char as u32
            );
            let from_char = from.chars().next().expect("source glyph is non-empty");
            assert!(
                face.glyph_index(from_char).is_none(),
                "source glyph {from:?} (U+{:04X}) IS covered by DejaVu Sans — substitution unnecessary",
                from_char as u32
            );
        }
    }
}
