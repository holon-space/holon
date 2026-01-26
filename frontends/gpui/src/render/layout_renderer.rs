//! Layout renderer registry — name → render fn.
//!
//! `mod.rs::render` looks up the layout renderer for a collection's
//! `LayoutSpec.name` and dispatches to it. Layouts that need GPUI-specific
//! treatment (board's lane grouping, columns' drawer animations) register
//! a custom impl here; everything else falls through to the default
//! `ReactiveShell` render path.
//!
//! The registry replaces the previous chain of `if name == "columns"` /
//! `if name == "board"` branches in the dispatch site. New layouts that
//! ship with their own GPUI render fn just call `register_layout_renderer`
//! at startup — adding one is a one-line change to startup wiring, not a
//! shared-infra patch.
//!
//! ## Why a per-frontend registry (not a shared one)
//!
//! `holon_frontend::collection_layout::LayoutRegistry` is the data-layer
//! registry — shape (Flat / Hierarchical) drives the streaming runtime,
//! gap drives default spacing. It's frontend-agnostic.
//!
//! This file is the GPUI-side renderer registry. Other frontends (Flutter,
//! Dioxus, …) will get their own when they grow the same pain. The shared
//! `LayoutSpec` stays the source of truth for shape/gap; the per-frontend
//! registries decide *how* to draw that shape.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use gpui::AnyElement;
use holon_frontend::reactive_view_model::ReactiveViewModel;

use super::builders::prelude::GpuiRenderContext;

/// A platform-specific render impl for a collection layout.
///
/// Returning `None` from `lookup_renderer` means "fall through to the
/// default `ReactiveShell` render" — most layouts (list, table, tree,
/// outline) live in that bucket. Custom impls (columns, board) handle
/// their own subscription / layout / dispatch.
pub trait LayoutRenderer: Send + Sync {
    fn render(&self, node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement;
}

/// Function-pointer convenience: `pub fn render(&node, &ctx) -> AnyElement`
/// can be registered without a wrapper struct.
impl<F> LayoutRenderer for F
where
    F: Fn(&ReactiveViewModel, &GpuiRenderContext) -> AnyElement + Send + Sync,
{
    fn render(&self, node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
        (self)(node, ctx)
    }
}

type Registry = HashMap<String, Arc<dyn LayoutRenderer>>;

static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();

fn registry() -> &'static RwLock<Registry> {
    REGISTRY.get_or_init(|| {
        let mut r = Registry::new();
        super::builders::register_builtin_layout_renderers(&mut r);
        RwLock::new(r)
    })
}

/// Register a layout renderer by name. Idempotent: re-registering an
/// existing name overwrites. Typically called at frontend startup.
pub fn register_layout_renderer(name: &str, renderer: Arc<dyn LayoutRenderer>) {
    registry()
        .write()
        .unwrap()
        .insert(name.to_string(), renderer);
}

/// Look up a renderer by layout name. `None` means "use the default
/// `ReactiveShell` render path."
pub fn lookup_renderer(name: &str) -> Option<Arc<dyn LayoutRenderer>> {
    registry().read().unwrap().get(name).cloned()
}

/// Helper for callers that already have a `LayoutSpec` reference.
pub fn renderer_for_spec(
    spec: &holon_frontend::collection_layout::LayoutSpec,
) -> Option<Arc<dyn LayoutRenderer>> {
    lookup_renderer(&spec.name)
}
