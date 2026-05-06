use super::prelude::*;
use holon_api::ClickModifiers;
use holon_frontend::operations::OperationIntent;
use holon_frontend::ReactiveViewModel;
use std::collections::HashMap;

/// Handles "selectable" interaction wrapper.
///
/// Pre-resolves every click-bound intent on the node into a
/// `HashMap<ClickModifiers, OperationIntent>` and looks the active modifier
/// set up on each mouse-down. Adding a new modifier (alt-click, cmd-click)
/// is a one-line wiring in the shadow `selectable.rs` plus a profile YAML
/// entry — no changes here. Any modifier-click `stop_propagation`s so the
/// row-level click handler doesn't also fire (LogSeq-style "pin to sidebar
/// without focusing").
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let child = node.children.first().expect("selectable requires a child");
    let child_el = super::render(child, ctx);

    let click_intents: HashMap<ClickModifiers, OperationIntent> = node
        .operations
        .iter()
        .filter_map(|ow| {
            ow.descriptor.click_modifiers().map(|m| {
                (
                    m,
                    OperationIntent::new(
                        ow.descriptor.entity_name.clone(),
                        ow.descriptor.name.clone(),
                        ow.descriptor.bound_params.clone(),
                    ),
                )
            })
        })
        .collect();
    if click_intents.is_empty() {
        return child_el;
    }

    let row_id = node.row_id();
    let el_id = format!("selectable-{}", row_id.as_deref().unwrap_or("unknown"));
    let services = ctx.services.clone();
    let el_id_log = el_id.clone();

    let inner = div()
        .child(
            div()
                .id(hashed_id(&el_id))
                .cursor_pointer()
                .child(child_el)
                .on_mouse_down(gpui::MouseButton::Left, move |event, _, cx| {
                    let modifiers = ClickModifiers {
                        shift: event.modifiers.shift,
                        alt: event.modifiers.alt,
                        cmd: event.modifiers.platform,
                        ctrl: event.modifiers.control,
                    };
                    if let Some(intent) = click_intents.get(&modifiers) {
                        tracing::debug!(
                            "[selectable] CLICKED ({:?}): el_id={}, action={}.{}",
                            modifiers,
                            el_id_log,
                            intent.entity_name,
                            intent.op_name
                        );
                        if !modifiers.is_none() {
                            cx.stop_propagation();
                        }
                        services.dispatch_intent(intent.clone());
                    }
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
        None,
    )
    .into_any_element()
}
