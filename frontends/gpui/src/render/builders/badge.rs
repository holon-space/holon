use gpui::AnyElement;
use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let label = node.prop_str("label").unwrap_or_default();
    let el = div()
        .px(px(8.0))
        .py(px(2.0))
        .text_size(px(11.0))
        .text_color(tc(ctx, |t| t.accent))
        .child(label.clone())
        .into_any_element();

    // A badge is a leaf that paints words the user reads, so it exposes them
    // through `BoundsRegistry` the way `text` exposes its own. A badge whose
    // label is a literal has no row binding of its own, so it names the block
    // it marks through `#{block_id: col("id")}` — a mark nobody can attribute
    // to a block is not a mark anything can judge.
    let block_id = node.entity_id().map(|uri| uri.to_string());
    let el_id = format!("badge#{}", ctx.bounds_registry.next_seq());
    crate::geometry::tracked(
        el_id,
        el,
        &ctx.bounds_registry,
        "badge",
        block_id.as_deref(),
        !label.is_empty(),
        Some(std::sync::Arc::from(label)),
    )
    .into_any_element()
}
