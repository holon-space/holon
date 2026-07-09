use super::prelude::*;
use holon_frontend::reactive_view_model::ReactiveViewModel;

/// Hover-reveal container. `children[0]` (the trigger) renders always;
/// `children[1..]` (the content) render only while the row is hovered.
///
/// Hover is per-render-slot view state: the node's own `Mutable<bool>` (never
/// a `Cell`, never a registry keyed by id), so two same-id rows in different
/// panes hover independently. `.on_hover` flips it and `window.refresh()`
/// re-runs this builder — the same idiom `expand_toggle` uses for its gate.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let hovered = node
        .hovered
        .as_ref()
        .expect("on_hover requires hovered state");
    let is_hovered = hovered.get();

    let hovered_handle = hovered.clone();
    let el_id = format!("on-hover-{:p}", node as *const ReactiveViewModel);

    let mut container = div()
        .id(hashed_id(&el_id))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .on_hover(move |is_over, window, _cx| {
            if hovered_handle.get() != *is_over {
                hovered_handle.set(*is_over);
                window.refresh();
            }
        });

    let mut children = node.children.iter();
    if let Some(trigger) = children.next() {
        container = container.child(super::render(trigger, ctx));
    }
    if is_hovered {
        for content in children {
            container = container.child(super::render(content, ctx));
        }
    }

    container.into_any_element()
}
