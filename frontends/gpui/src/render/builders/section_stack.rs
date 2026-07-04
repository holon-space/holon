use holon_frontend::ReactiveViewModel;
use holon_frontend::sticky_accordion as sa;

use super::prelude::*;

/// Stable geometry-registry key for this stack (so multiple stacks don't
/// collide on the container / section / footer ids).
fn stack_key(node: &ReactiveViewModel) -> String {
    node.prop_str("block_id")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "section-stack".to_string())
}

fn is_accordion(c: &ReactiveViewModel) -> bool {
    c.widget_name().as_deref() == Some("accordion")
}

fn placement_of(c: &ReactiveViewModel) -> Option<String> {
    c.prop_str("placement").map(|s| s.to_string())
}

/// Section-stack container (Inc C). A definite-height scroll region of
/// sections; an in-flow (`pinned:false`) accordion renders inline at its flow
/// position, a `sticky:true` accordion is lifted OUT of flow into an absolute
/// `.occlude()` footer overlay whose top follows the spike position law
/// (computed by `accordion::render_sticky` from OBSERVED bounds).
///
/// The outer `relative` container is tracked as
/// [`sa::SECTION_STACK_CONTAINER_WIDGET`] so the overlay + the PBT invariants
/// read its committed bounds as the definite viewport.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let key = stack_key(node);

    let mut scroller = div()
        .id(hashed_id(&format!("section-stack-scroller:{key}")))
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .overflow_y_scroll();

    let mut sticky_children: Vec<&ReactiveViewModel> = Vec::new();

    for (i, child) in node.children.iter().enumerate() {
        if is_accordion(child) && placement_of(child).as_deref() == Some("sticky") {
            // Lifted out of flow; rendered as an overlay AFTER the scroller.
            sticky_children.push(child);
            continue;
        }
        // Content sections and in-flow accordions render inline, tracked as
        // sections so the overlay position law can read `next_section_top`.
        let rendered = if is_accordion(child) && placement_of(child).as_deref() == Some("in_flow") {
            super::accordion::render_in_flow(child, ctx).into_any_element()
        } else {
            super::render(child, ctx)
        };
        let tracked = crate::geometry::TransparentTracker::new(
            format!("section:{key}:{i}"),
            sa::SECTION_WIDGET,
            ctx.bounds_registry.clone(),
            rendered,
        )
        .with_entity_id(format!("section:{key}:{i}"));
        scroller = scroller.child(tracked);
    }

    let mut container = div()
        .relative()
        .flex_1()
        .w_full()
        .flex()
        .flex_col()
        .child(scroller);

    for sticky in sticky_children {
        container = container.child(super::accordion::render_sticky(sticky, ctx, &key));
    }

    // Track the outer container as the definite viewport the overlay caps
    // against and the invariants read `viewport_bottom` from.
    let tracked = crate::geometry::TransparentTracker::new(
        format!("section-stack:{key}"),
        sa::SECTION_STACK_CONTAINER_WIDGET,
        ctx.bounds_registry.clone(),
        container.into_any_element(),
    )
    .with_entity_id(format!("section-stack:{key}"));
    div().flex_1().w_full().child(tracked)
}
