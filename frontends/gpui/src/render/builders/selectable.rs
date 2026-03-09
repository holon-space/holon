use super::prelude::*;
use holon_frontend::ReactiveViewModel;

/// Handles "selectable" interaction wrapper.
///
/// Asks the node for its click intent (sourced from `node.operations` by the
/// shadow builder) and dispatches it on mouse-down. No row-data side-channel,
/// no parsing in the click handler — the node fully describes what should
/// happen on click.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let child = node.children.first().expect("selectable requires a child");
    let child_el = super::render(child, ctx);

    let Some(intent) = node.click_intent() else {
        return child_el;
    };

    let row_id = node.row_id();
    let el_id = format!("selectable-{}", row_id.as_deref().unwrap_or("unknown"));
    let services = ctx.services.clone();

    let action_name_log = format!("{}.{}", intent.entity_name, intent.op_name);
    let el_id_log = el_id.clone();
    let inner = div()
        .child(
            div()
                .id(hashed_id(&el_id))
                .cursor_pointer()
                .child(child_el)
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                    tracing::debug!(
                        "[selectable] CLICKED: el_id={}, action={}",
                        el_id_log,
                        action_name_log
                    );
                    services.dispatch_intent(intent.clone());
                }),
        )
        .into_any_element();
    crate::geometry::tracked(
        el_id,
        inner,
        &ctx.bounds_registry,
        "selectable",
        row_id.as_deref(),
        true,
    )
    .into_any_element()
}
