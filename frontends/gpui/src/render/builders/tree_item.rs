use super::prelude::*;
use holon_frontend::view_model::LazyChildren;

const INDENT_PX: f32 = 24.0;
const CHEVRON_HIT_SIZE: f32 = 20.0;
const BULLET_SIZE: f32 = 6.0;
const ITEM_MIN_HEIGHT: f32 = 28.0;

/// Extract a stable ID from the first child's entity data for collapse state tracking.
/// Walks into wrapper nodes (RenderBlock, LiveQuery) to find the actual entity with an "id".
fn node_id(children: &LazyChildren) -> Option<String> {
    use holon_frontend::view_model::NodeKind;
    let mut vm = children.items.first()?;
    loop {
        if let Some(id) = vm.entity.get("id").and_then(|v| v.as_string()) {
            return Some(id.to_string());
        }
        // Unwrap single-child wrappers
        match &vm.kind {
            NodeKind::RenderBlock { content } => vm = content,
            NodeKind::LiveQuery { content, .. } => vm = content,
            _ => {
                // Try first child of any container
                if let Some(first) = vm.children().first() {
                    vm = first;
                } else {
                    return None;
                }
            }
        }
    }
}

fn bullet_dot(ctx: &GpuiRenderContext) -> Div {
    div()
        .flex_shrink_0()
        .w(px(CHEVRON_HIT_SIZE))
        .h(px(ITEM_MIN_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(BULLET_SIZE))
                .h(px(BULLET_SIZE))
                .rounded(px(BULLET_SIZE / 2.0))
                .bg(tc(ctx, |t| t.muted_foreground)),
        )
}

fn collapse_chevron(
    collapsed: bool,
    el_id: String,
    registry: crate::geometry::BoundsRegistry,
    ctx: &GpuiRenderContext,
) -> gpui::Stateful<Div> {
    let toggle_id = el_id.clone();
    let chevron = if collapsed {
        "\u{25B6}" // ▶ right-pointing triangle
    } else {
        "\u{25BC}" // ▼ down-pointing triangle
    };
    let color = tc(ctx, |t| t.muted_foreground);

    div()
        .id(ElementId::Name(format!("tree-toggle-{el_id}").into()))
        .cursor_pointer()
        .flex_shrink_0()
        .w(px(CHEVRON_HIT_SIZE))
        .h(px(CHEVRON_HIT_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.0))
        .text_color(color)
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, _| {
            registry.toggle_tree_item_collapsed(toggle_id.clone());
            window.refresh();
        })
        .child(chevron.to_string())
}

pub fn render(children: &LazyChildren, ctx: &GpuiRenderContext) -> Div {
    let has_children = children.items.len() > 1;
    let id = node_id(children);
    let collapsed = id
        .as_ref()
        .map(|id| ctx.bounds_registry.is_tree_item_collapsed(id))
        .unwrap_or(false);

    let mut container = div().flex_col();

    let first_rendered = children
        .items
        .first()
        .map(|child| super::render(child, ctx));

    if let Some(node) = first_rendered {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .min_h(px(ITEM_MIN_HEIGHT));

        if has_children {
            let el_id = id.clone().unwrap_or_else(|| "tree-toggle".to_string());
            let registry = ctx.bounds_registry.clone();
            row = row.child(collapse_chevron(collapsed, el_id, registry, ctx));
        } else {
            row = row.child(bullet_dot(ctx));
        }

        row = row.child(div().flex_1().overflow_hidden().child(node));
        container = container.child(row);
    }

    if has_children && !collapsed {
        let mut indented = div().flex_col().pl(px(INDENT_PX));
        for child in children.items.iter().skip(1) {
            indented = indented.child(super::render(child, ctx));
        }
        container = container.child(indented);
    }

    container
}
