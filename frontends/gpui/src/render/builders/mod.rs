mod columns;
pub(crate) mod prelude;
pub mod style;

// Re-export tree_item collapse helper for use by ReactiveShell.
pub(crate) use tree_item::collapse_state as tree_item_collapse_state;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude, columns, style],
    node_dispatch: AnyElement,
    context: GpuiRenderContext,
    transform: crate::render::builders::tag(ctx, __name, __inner),
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
    let seq = ctx.bounds_registry.next_seq();
    let id = format!("{name}#{seq}");
    crate::geometry::TransparentTracker::new(
        id,
        name,
        ctx.bounds_registry.clone(),
        el.into_any_element(),
    )
    .into_any_element()
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
/// - `relative().size_full().flex().flex_col().overflow_hidden()` on the
///   outer div gives the region a definite height (the parent's) and clips
///   the inner list to that height so `gpui::uniform_list`'s wheel hitbox
///   participates in scroll event routing.
/// - `flex_1().min_h_0().w_full()` on the intermediate div is the critical
///   combination: without `min_h_0`, Taffy uses the content height of the
///   list (thousands of px) as the item's *minimum* and the list's viewport
///   then equals its content, so `scroll_max = 0` and the list looks frozen
///   even though it's "technically" scrollable. This was the April 2026
///   cascade bug — documented in `gpui_render_cascade_fix.md`.
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

use gpui::prelude::*;
use gpui::{div, AnyElement, Div};

use crate::entity_view_registry::{FocusRegistry, LocalEntityScope};
use crate::geometry::BoundsRegistry;
use crate::views::ReactiveShell;

/// Raw pointers to GPUI's Window and App, valid for the duration of a render pass.
/// Builders that need to create entities, register listeners, or interact with GPUI
/// state access these through `GpuiRenderContext::with_gpui()`.
struct GpuiHandle {
    window: *mut gpui::Window,
    cx: *mut gpui::App,
}

// Safety: GpuiHandle is only used on the main thread during a synchronous render pass.
// The pointers are valid for the lifetime of the GpuiRenderContext that contains them.
unsafe impl Send for GpuiHandle {}
unsafe impl Sync for GpuiHandle {}

/// GPUI-specific render context. Wraps the shared RenderContext with GPUI extensions.
pub struct GpuiRenderContext {
    pub ctx: holon_frontend::RenderContext,
    pub services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
    pub bounds_registry: BoundsRegistry,
    pub local: LocalEntityScope,
    pub focus: FocusRegistry,
    layout_style: futures_signals::signal::Mutable<style::LayoutStyle>,
    gpui: GpuiHandle,
}

impl GpuiRenderContext {
    pub fn new(
        ctx: holon_frontend::RenderContext,
        services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
        bounds_registry: BoundsRegistry,
        local: LocalEntityScope,
        focus: FocusRegistry,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Self {
        Self {
            ctx,
            services,
            bounds_registry,
            local,
            focus,
            layout_style: futures_signals::signal::Mutable::new(style::LayoutStyle::default()),
            gpui: GpuiHandle {
                window: window as *mut _,
                cx: cx as *mut _,
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

    // Collection-backed nodes are rendered via ReactiveShell
    if let Some(ref view) = node.collection {
        // Columns layout is rendered directly by the GPUI columns builder
        // (drawer animations, absolute positioning for panels). Other collections
        // go through ReactiveShell for VecDiff subscription.
        if matches!(
            view.layout(),
            Some(holon_frontend::CollectionVariant::Columns { .. })
        ) {
            return columns::render(node, ctx).into_any_element();
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
    let key = format!("rv-{:016x}", view.stable_cache_key());
    let view = view.clone();
    let render_ctx = ctx.ctx.clone();
    let services = ctx.services.clone();
    let focus = ctx.focus.clone();
    let bounds = ctx.bounds_registry.clone();
    let entity = ctx.local.get_or_create(&key, || {
        ctx.with_gpui(|_window, cx| {
            cx.new(|cx| {
                ReactiveShell::new_for_collection(view, render_ctx, services, focus, bounds, cx)
            })
            .into_any()
        })
    });
    entity.downcast().expect("cached entity type mismatch")
}

fn render_unsupported(name: &str, _ctx: &GpuiRenderContext) -> Div {
    div().child(format!("[unsupported: {name}]"))
}

/// Stable key for a live query, used to look up Entity<LiveQueryView> in the registry.
pub(crate) fn live_query_key(sql: &str, context_id: Option<&str>) -> String {
    use std::hash::{Hash, Hasher};
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
