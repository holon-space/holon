use std::time::Duration;

use gpui::AnimationExt;

use super::prelude::*;
use holon_frontend::view_model::{LazyChildren, NodeKind};

const SIDEBAR_WIDTH: f32 = 260.0;
const SLIDE_DURATION: Duration = Duration::from_millis(150);

pub fn render(gap: &f32, children: &LazyChildren, ctx: &GpuiRenderContext) -> Div {
    let has_drawers = children.items.iter().any(|n| matches!(n.kind, NodeKind::Drawer { .. }));
    if !has_drawers {
        let mut container = div().flex().flex_row().gap(px(*gap));
        for child in render_children(children, ctx) {
            container = container.child(child);
        }
        return container;
    }

    let mut container = div().flex().flex_row().items_stretch().flex_1().w_full();

    let items = &children.items;
    let first_drawer = items.iter().position(|n| matches!(n.kind, NodeKind::Drawer { .. }));
    let last_drawer = items.iter().rposition(|n| matches!(n.kind, NodeKind::Drawer { .. }));

    for (i, node) in items.iter().enumerate() {
        let is_first_drawer = Some(i) == first_drawer;
        let is_last_drawer = Some(i) == last_drawer && first_drawer != last_drawer;

        if let NodeKind::Drawer { ref block_id, ref child } = node.kind {
            if is_first_drawer || is_last_drawer {
                let is_open = ctx
                    .widget_states()
                    .get(block_id.as_str())
                    .map_or(true, |ws| ws.open);

                let rendered = super::render(child, ctx);
                let inner = div()
                    .id(ElementId::Name(block_id.clone().into()))
                    .h_full()
                    .overflow_y_scroll()
                    .bg(tc(ctx, |t| t.sidebar))
                    .w(px(SIDEBAR_WIDTH))
                    .min_w(px(SIDEBAR_WIDTH))
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_sm()
                    .child(rendered);
                let inner = if is_first_drawer {
                    inner.border_r_1().border_color(tc(ctx, |t| t.border))
                } else {
                    inner.border_l_1().border_color(tc(ctx, |t| t.border))
                };

                let (anim_id, target_width) = if is_open {
                    (format!("{}-open", block_id), SIDEBAR_WIDTH)
                } else {
                    (format!("{}-close", block_id), 0.0)
                };
                let start_width = if is_open { 0.0 } else { SIDEBAR_WIDTH };

                let clip = div()
                    .h_full()
                    .overflow_hidden()
                    .flex_shrink_0()
                    .w(px(target_width))
                    .child(inner);

                container = container.child(clip.with_animation(
                    ElementId::Name(anim_id.into()),
                    gpui::Animation::new(SLIDE_DURATION).with_easing(gpui::ease_in_out),
                    move |el, progress| {
                        let w = start_width + (target_width - start_width) * progress;
                        el.w(px(w))
                    },
                ));
            } else {
                // Non-first/last drawer — render as normal panel
                let rendered = super::render(child, ctx);
                container = container.child(
                    div()
                        .id(ElementId::Name(block_id.clone().into()))
                        .flex_1()
                        .overflow_y_scroll()
                        .p_2()
                        .child(rendered),
                );
            }
        } else {
            let scroll_id = node.entity.get("id").and_then(|v| v.as_string()).unwrap_or("panel");
            let rendered = super::render(node, ctx);
            container = container.child(
                div()
                    .id(ElementId::Name(scroll_id.to_string().into()))
                    .flex_1()
                    .overflow_y_scroll()
                    .px(px(32.0))
                    .py(px(12.0))
                    .child(rendered),
            );
        }
    }

    container
}
