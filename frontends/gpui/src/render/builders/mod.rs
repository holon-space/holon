pub(crate) mod operation_helpers;
mod prelude;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude, operation_helpers],
    node_dispatch: AnyElement,
    context: GpuiRenderContext,
    convert: .into_any_element()
);

use gpui::prelude::*;
use gpui::{div, AnyElement, Div};

use crate::geometry::BoundsRegistry;

/// GPUI-specific render context. Wraps the shared RenderContext with GPUI extensions.
pub struct GpuiRenderContext {
    pub ctx: holon_frontend::RenderContext,
    pub bounds_registry: BoundsRegistry,
}

impl std::ops::Deref for GpuiRenderContext {
    type Target = holon_frontend::RenderContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

/// Render a ViewModel tree into a GPUI AnyElement.
pub fn render(node: &holon_frontend::view_model::ViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    if matches!(node.kind, holon_frontend::view_model::NodeKind::Empty) {
        return div().into_any_element();
    }
    render_node(node, ctx)
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

/// Recursively render children of a LazyChildren into AnyElements.
pub(crate) fn render_children(
    children: &holon_frontend::view_model::LazyChildren,
    ctx: &GpuiRenderContext,
) -> Vec<AnyElement> {
    children
        .items
        .iter()
        .map(|child| render(child, ctx))
        .collect()
}
