use super::prelude::*;

use crate::entity_view_registry::CollapsibleView;

const TEXT_SECONDARY: u32 = 0x9D9D95FF;
const TOOL_BG: u32 = 0x3D3D38FF;
const TOOL_BORDER: u32 = 0x4A4A44FF;
const DETAIL_BG: u32 = 0x1E1E1CFF;

fn c(hex: u32) -> Hsla {
    gpui::rgba(hex).into()
}

impl gpui::Render for CollapsibleView {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let chevron = if self.collapsed { "▸" } else { "▾" };

        let header_row = div()
            .id("collapsible-toggle")
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(8.0))
            .cursor_pointer()
            .hover(|s| s.bg(c(0x45453FFF)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.collapsed = !this.collapsed;
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(c(TEXT_SECONDARY))
                    .child(chevron),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .child(self.icon_text.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.0))
                    .text_color(c(TEXT_SECONDARY))
                    .child(self.header_text.clone()),
            );

        let mut tool_div = div()
            .w_full()
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .bg(c(TOOL_BG))
            .border_1()
            .border_color(c(TOOL_BORDER))
            .child(header_row);

        if !self.collapsed && !self.detail_text.is_empty() {
            tool_div = tool_div.child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(c(TOOL_BORDER))
                    .bg(c(DETAIL_BG))
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_size(px(12.0))
                    .text_color(c(TEXT_SECONDARY))
                    .child(self.detail_text.clone()),
            );
        }

        div()
            .w_full()
            .flex_shrink_0()
            .py(px(2.0))
            .pl(px(38.0))
            .child(tool_div)
    }
}

pub fn render(node: &holon_frontend::reactive_view_model::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let header_text = node.prop_str("header").unwrap_or_else(|| "".to_string());
    let icon_text = node.prop_str("icon").unwrap_or_else(|| "".to_string());
    let children = &node.children;

    let detail_text: String = children.iter().filter_map(|child| {
        child.prop_str("content").map(|s| s.to_string())
    }).collect::<Vec<_>>().join("\n");

    let cache_key = crate::entity_view_registry::CacheKey::Ephemeral(format!(
        "collapsible:{header_text}"
    ));
    let any_entity = ctx.local.get_or_create(cache_key, || {
        ctx.with_gpui(|_window, cx| {
            cx.new(|_cx| CollapsibleView {
                collapsed: true,
                header_text,
                icon_text,
                detail_text,
            })
            .into_any()
        })
    });
    let entity: gpui::Entity<CollapsibleView> =
        any_entity.downcast().expect("cached entity type mismatch");
    entity.into_any_element()
}
