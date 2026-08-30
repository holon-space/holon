mod columns;
pub(crate) mod prelude;
pub mod style;
mod table;

// Re-export tree_item collapse helper for use by ReactiveShell.
// Re-export the sidebar drag-resize machinery so the root view (`lib.rs`) can
// mount the full-window capture overlay while a drag is in progress.
// Accordion flow-panel split — ONE implementation, TWO call sites: columns.rs
// (shell-less compositions) and the per-block ReactiveShell arm (production,
// where the main panel is wrapped in a live_block so columns.rs sees no
// column).
pub(crate) use column::has_pinned_child;
pub(crate) use column::render_accordion_split;
pub(crate) use column::slot_pinned_container;
pub(crate) use drawer::SidebarResizeState;
pub(crate) use drawer::drag_sidebar_to;
pub(crate) use drawer::finalize_sidebar_resize;
pub(crate) use tree_item::collapse_state as tree_item_collapse_state;
// Scan-weight contract between a parent's disclosure and a leaf's bullet —
// asserted by the sidebar disclosure affordance test.
pub use tree_item::{DISCLOSURE_WEIGHT, LEAF_BULLET_WEIGHT};
pub(crate) use view_mode_switcher::render_accordion_split_slot;

// The shared switch geometry — a painted control, not a widget of its own, so
// it is skipped by the registry below and used by name.
pub(crate) mod switch_track;
pub(crate) use switch_track::switch_track;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude, columns, style, switch_track, table],
    node_dispatch: AnyElement,
    context: GpuiRenderContext,
    transform: crate::render::builders::tag_node(ctx, __name, node, __inner),
);

/// Wrapper applied to every builder's output via the `transform:` template in
/// `builder_registry!`. Wraps the element in a `TransparentTracker` so its
/// final bounds get recorded into `ctx.bounds_registry` keyed by
/// `"{widget}#{seq}"`. Layout-transparent — uses the child's own `LayoutId`,
/// no wrapper style.
///
/// This is the single call site where every widget gets observability;
/// debug_selector-style mechanisms, metrics, and tracing hooks all live here.
pub(crate) fn tag<E: gpui::IntoElement>(
    ctx: &GpuiRenderContext,
    name: &'static str,
    el: E,
) -> AnyElement {
    tag_with_entity_id(ctx, name, None, el)
}

/// The `transform:` template above — `tag()` plus the identity of the node
/// whose builder is being wrapped. That identity is what lets `describe_ui`
/// report a node's OWN rect: every node of a row's chain (`tree_item` >
/// `column` > `selectable` > `rendered_text`) renders the same entity, so an
/// entity-wide join can only hand them all one sibling's box.
pub(crate) fn tag_node<E: gpui::IntoElement>(
    ctx: &GpuiRenderContext,
    name: &'static str,
    node: &holon_frontend::reactive_view_model::ReactiveViewModel,
    el: E,
) -> AnyElement {
    let seq = ctx.bounds_registry.next_seq();
    let id = format!("{name}#{seq}");
    crate::geometry::TransparentTracker::new(
        id,
        name,
        ctx.bounds_registry.clone(),
        el.into_any_element(),
    )
    .with_vm_node(node.row_id().as_deref())
    .into_any_element()
}

/// Like `tag()`, but binds an `entity_id` on the tracker so PBT generators
/// and other region-scoped consumers can locate this subtree via
/// `BoundsRegistry::find_by_entity_id(...)`. Used by `live_block` to expose
/// its block URI (e.g. `block:default-left-sidebar`) — `tag()` is unaware of
/// block ids because the registry macro doesn't pass the node.
pub(crate) fn tag_with_entity_id<E: gpui::IntoElement>(
    ctx: &GpuiRenderContext,
    name: &'static str,
    entity_id: Option<&str>,
    el: E,
) -> AnyElement {
    let seq = ctx.bounds_registry.next_seq();
    let id = format!("{name}#{seq}");
    let mut tracker = crate::geometry::TransparentTracker::new(
        id,
        name,
        ctx.bounds_registry.clone(),
        el.into_any_element(),
    );
    if let Some(eid) = entity_id {
        tracker = tracker.with_entity_id(eid);
    }
    tracker.into_any_element()
}

/// Production layout chain for a scrollable list region.
///
/// Both the real reactive-collection rendering path (see
/// `builders::render` where it wraps the `ReactiveShell` `AnyView`) **and**
/// the fast-UI test fixtures for scroll (`tests/support::ScrollableListView`)
/// call this so the layout idiom stays in one place. Any change to the
/// wrapper chain is therefore picked up automatically by the scroll tests.
///
/// Why this exact chain:
/// - `relative().size_full().flex().flex_col().overflow_hidden()` on the outer
///   div gives the region a definite height (the parent's) and clips the inner
///   list to that height so `gpui::uniform_list`'s wheel hitbox participates in
///   scroll event routing.
/// - `flex_1().min_h_0().w_full()` on the intermediate div is the critical
///   combination: without `min_h_0`, Taffy uses the content height of the list
///   (thousands of px) as the item's *minimum* and the list's viewport then
///   equals its content, so `scroll_max = 0` and the list looks frozen even
///   though it's "technically" scrollable. This was the April 2026 cascade bug
///   — documented in `gpui_render_cascade_fix.md`.
pub fn scrollable_list_wrapper<E: gpui::IntoElement>(
    inner: E,
    shell_id: impl Into<gpui::ElementId>,
) -> AnyElement {
    gpui::div()
        .id(shell_id)
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(gpui::div().flex_1().min_h_0().w_full().child(inner))
        .into_any_element()
}

use gpui::AnyElement;
use gpui::Div;
use gpui::div;
use gpui::prelude::*;

use crate::entity_view_registry::LocalEntityScope;
use crate::geometry::BoundsRegistry;
use crate::navigation_state::NavigationState;
use crate::views::ReactiveShell;

/// Raw pointers to GPUI's Window and App, valid for the duration of a render
/// pass. Builders that need to create entities, register listeners, or interact
/// with GPUI state access these through `GpuiRenderContext::with_gpui()`.
struct GpuiHandle {
    window: *mut gpui::Window,
    cx: *mut gpui::App,
}

// Safety: GpuiHandle is only used on the main thread during a synchronous
// render pass. The pointers are valid for the lifetime of the GpuiRenderContext
// that contains them.
unsafe impl Send for GpuiHandle {}
unsafe impl Sync for GpuiHandle {}

/// GPUI-specific render context. Wraps the shared RenderContext with GPUI
/// extensions.
pub struct GpuiRenderContext {
    pub ctx: holon_frontend::RenderContext,
    pub services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
    pub bounds_registry: BoundsRegistry,
    pub local: LocalEntityScope,
    pub nav: NavigationState,
    /// Chain of `live_block` block ids being rendered up the tree at the
    /// frame this context represents. Consulted by the `live_block` builder
    /// to refuse cyclic creation (A→B→A). Each `ReactiveShell` extends
    /// this with its own `block_id` before constructing the context, and
    /// each lazy `live_block` create-closure captures the current chain so
    /// the new shell's own renders see the right ancestor set.
    pub live_block_ancestors: crate::entity_view_registry::LiveBlockAncestors,
    /// The layout slot the element being built will land in. A `live_block` /
    /// `live_query` builder hands this to the `ReactiveShell` it creates, which
    /// then knows whether it may claim `size_full` and own a scroll viewport.
    ///
    /// Defaults to `Panel` — the definite-height slot a window root, a panel
    /// wrapper, or a layout fixture provides. Only the contexts that build ONE
    /// ROW of a collection (`RenderEntityView`, the virtualized list's per-row
    /// context) declare `Nested`, because only there is the parent height
    /// indefinite.
    pub placement: crate::views::reactive_shell::ShellPlacement,
    layout_style: futures_signals::signal::Mutable<style::LayoutStyle>,
    gpui: GpuiHandle,
}

impl GpuiRenderContext {
    pub fn new(
        ctx: holon_frontend::RenderContext,
        services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
        bounds_registry: BoundsRegistry,
        local: LocalEntityScope,
        nav: NavigationState,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Self {
        Self {
            ctx,
            services,
            bounds_registry,
            local,
            nav,
            live_block_ancestors: crate::entity_view_registry::LiveBlockAncestors::new(),
            placement: crate::views::reactive_shell::ShellPlacement::Panel,
            layout_style: futures_signals::signal::Mutable::new(style::LayoutStyle::default()),
            gpui: GpuiHandle {
                window: window as *mut _,
                cx: cx as *mut _,
            },
        }
    }

    /// Replace the ancestor chain on this context. Used by `ReactiveShell`'s
    /// render entry-point to inject the chain captured at the shell's own
    /// creation time, extended with the shell's own `block_id`.
    pub fn with_live_block_ancestors(
        mut self,
        ancestors: crate::entity_view_registry::LiveBlockAncestors,
    ) -> Self {
        self.live_block_ancestors = ancestors;
        self
    }

    /// Declare the layout slot this context's elements land in. Re-emitted by
    /// every block-mode `ReactiveShell` from its own placement, and set to
    /// `Nested` by the two contexts that build one ROW of a collection.
    pub fn with_shell_placement(
        mut self,
        placement: crate::views::reactive_shell::ShellPlacement,
    ) -> Self {
        self.placement = placement;
        self
    }

    /// This context with `Nested` placement — for building a child of a
    /// content-sized container, where a shell that claims `size_full` plus
    /// `height: relative(1.0)` has no definite parent to resolve against and
    /// collapses to 0 px.
    pub(crate) fn nested(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
            services: self.services.clone(),
            bounds_registry: self.bounds_registry.clone(),
            local: self.local.clone(),
            nav: self.nav.clone(),
            live_block_ancestors: self.live_block_ancestors.clone(),
            placement: crate::views::reactive_shell::ShellPlacement::Nested,
            layout_style: self.layout_style.clone(),
            gpui: GpuiHandle {
                window: self.gpui.window,
                cx: self.gpui.cx,
            },
        }
    }

    pub fn with_layout_style(
        mut self,
        style: futures_signals::signal::Mutable<style::LayoutStyle>,
    ) -> Self {
        self.layout_style = style;
        self
    }

    pub fn style(&self) -> futures_signals::signal::MutableLockRef<'_, style::LayoutStyle> {
        self.layout_style.lock_ref()
    }

    pub fn layout_style_signal(
        &self,
    ) -> impl futures_signals::signal::Signal<Item = style::LayoutStyle> {
        self.layout_style.signal_cloned()
    }

    pub fn with_gpui<R>(&self, f: impl FnOnce(&mut gpui::Window, &mut gpui::App) -> R) -> R {
        unsafe { f(&mut *self.gpui.window, &mut *self.gpui.cx) }
    }

    pub fn services(&self) -> &dyn holon_frontend::reactive::BuilderServices {
        &*self.services
    }
}

impl std::ops::Deref for GpuiRenderContext {
    type Target = holon_frontend::RenderContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

/// Render a ReactiveViewModel tree into a GPUI AnyElement.
///
/// Collection nodes (those with `node.collection`) are rendered via a
/// `ReactiveShell` entity, cached in `EntityCache`.
#[tracing::instrument(level = "trace", skip_all)]
pub fn render(
    node: &holon_frontend::reactive_view_model::ReactiveViewModel,
    ctx: &GpuiRenderContext,
) -> AnyElement {
    // Empty node — no widget_name
    let widget_name = node.widget_name();
    if widget_name.as_deref() == Some("empty") || widget_name.is_none() {
        return div().into_any_element();
    }

    // Collection-backed nodes: dispatch through the layout-renderer
    // registry. Layouts that need GPUI-specific treatment (columns'
    // drawer animations, board's lane grouping) register a custom impl;
    // everything else falls through to the default `ReactiveShell` path.
    if let Some(ref view) = node.collection {
        if let Some(layout) = view.layout() {
            if let Some(renderer) = crate::render::layout_renderer::lookup_renderer(layout.name()) {
                return renderer.render(node, ctx);
            }
        }

        // Under a `Nested` placement there is no definite height for the
        // virtualized `gpui::list` to measure against — `scrollable_list_wrapper`'s
        // `size_full` chain resolves to 0 and the list paints nothing. Render
        // eagerly at content height instead, the same firewall
        // `column::eager_collection_div` provides for content-sized columns.
        if ctx.placement == crate::views::ShellPlacement::Nested {
            return tag(
                ctx,
                "reactive_shell",
                column::eager_collection_div(view, ctx),
            );
        }

        let entity = get_or_create_reactive_shell(view, ctx);
        // The scrollable wrapper chain is shared with fast-UI scroll
        // fixtures via `scrollable_list_wrapper` — see its docs for why
        // this exact combination of `size_full` / `flex_1` / `min_h_0`
        // is load-bearing. Any change here must keep tests in
        // `layout_scroll.rs` green.
        //
        // Wrapped in `tag()` so the shell's outer bounds end up in
        // `BoundsRegistry`. Without this, a collection with 0
        // items produces no tracked widgets and layout invariants see
        // it as "nothing rendered" — indistinguishable from a real
        // broken-render regression.
        let shell_key = format!("reactive-shell-{:p}", std::sync::Arc::as_ptr(view));
        return tag(
            ctx,
            "reactive_shell",
            scrollable_list_wrapper(gpui::AnyView::from(entity), prelude::hashed_id(&shell_key)),
        );
    }
    render_node(node, ctx)
}

/// Look up or create a `ReactiveShell` entity for a ReactiveView.
fn get_or_create_reactive_shell(
    view: &std::sync::Arc<holon_frontend::reactive_view::ReactiveView>,
    ctx: &GpuiRenderContext,
) -> gpui::Entity<ReactiveShell> {
    // Use the view's stable cache key instead of the Arc pointer. When the
    // parent block's interpreted tree is rebuilt (e.g. on a structural
    // change or view-mode switch), a new `Arc<ReactiveView>` is allocated
    // but it wraps the same data source and item template — keying on the
    // pointer would cause a fresh entity (and fresh ListState) on every
    // rebuild, losing scroll position and re-running all row measurements.
    let key = crate::entity_view_registry::CacheKey::ReactiveShell(view.stable_cache_key());
    let view = view.clone();
    let render_ctx = ctx.ctx.clone();
    let services = ctx.services.clone();
    let nav = ctx.nav.clone();
    let bounds = ctx.bounds_registry.clone();
    // Capture the parent's ancestor chain at creation time. Collections
    // don't add their own id (they have none), but we propagate the chain
    // so descendant `live_block` builders inside the collection's items
    // can refuse cycles correctly.
    let ancestors = ctx.live_block_ancestors.clone();
    ctx.local.get_or_create_typed(key, || {
        ctx.with_gpui(|_window, cx| {
            cx.new(|cx| {
                ReactiveShell::new_for_collection(
                    view, render_ctx, services, nav, bounds, ancestors, cx,
                )
            })
        })
    })
}

fn render_unsupported(name: &str, _: &GpuiRenderContext) -> Div {
    div().child(format!("[unsupported: {name}]"))
}

/// Stable key for a live query, used to look up its Entity<ReactiveShell> in
/// the registry.
pub(crate) fn live_query_key(sql: &str, context_id: Option<&str>) -> String {
    use std::hash::Hash;
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sql.hash(&mut hasher);
    context_id.hash(&mut hasher);
    format!("lq-{:x}", hasher.finish())
}

/// Render a Vec of children into AnyElements.
#[tracing::instrument(level = "trace", skip_all)]
pub(crate) fn render_children(
    children: &[std::sync::Arc<holon_frontend::reactive_view_model::ReactiveViewModel>],
    ctx: &GpuiRenderContext,
) -> Vec<AnyElement> {
    children.iter().map(|child| render(child, ctx)).collect()
}

/// Register the layout renderers shipped with the GPUI frontend. Called
/// the first time the registry is touched (see `layout_renderer::registry`).
///
/// Adding a new layout that needs a custom render fn is a one-line append
/// here — no shared-infra changes. Layouts that work with the default
/// `ReactiveShell` flat/tree path don't need an entry at all; they fall
/// through to the default in `mod.rs::render`.
pub(crate) fn register_builtin_layout_renderers(
    registry: &mut std::collections::HashMap<
        String,
        std::sync::Arc<dyn crate::render::layout_renderer::LayoutRenderer>,
    >,
) {
    use gpui::IntoElement;
    use holon_frontend::reactive_view_model::ReactiveViewModel;

    // `columns::render` returns a `Div`; wrap so the registry sees an
    // `AnyElement` like every other renderer.
    fn columns_render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
        columns::render(node, ctx).into_any_element()
    }

    registry.insert(
        "columns".to_string(),
        std::sync::Arc::new(
            columns_render as fn(&ReactiveViewModel, &GpuiRenderContext) -> AnyElement,
        ),
    );
    registry.insert(
        "board".to_string(),
        std::sync::Arc::new(
            board::render as fn(&ReactiveViewModel, &GpuiRenderContext) -> AnyElement,
        ),
    );
    // `table_columnar` is deliberately a SEPARATE layout name from `table`.
    // Dispatch here is by layout name, so registering this under `table` would
    // route a bare `table` — `live_query`'s default item template — through the
    // columnar renderer too. Keeping the names apart is what makes "bare-table
    // behaviour is untouched" a structural fact rather than a runtime guard.
    // Collapse the two only with a proof of equivalence.
    registry.insert(
        "table_columnar".to_string(),
        std::sync::Arc::new(
            table::render as fn(&ReactiveViewModel, &GpuiRenderContext) -> AnyElement,
        ),
    );
}
