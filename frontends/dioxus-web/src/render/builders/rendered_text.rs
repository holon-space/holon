use holon_frontend::view_model::ViewKind;

use super::prelude::*;
use crate::render::EntityContext;

/// Read-only sibling of `editable_text` (mirrors GPUI
/// `render/builders/rendered_text.rs`). A click calls `engineSetFocus`
/// (ADR 0010: focus is pure in-memory worker state), which flips the
/// `is_focused` variant so the next snapshot swaps in `editable_text`;
/// `worker_focus::apply` then moves DOM focus into the mounted editor.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::RenderedText { content, .. } = &node.kind else {
        return rsx! {};
    };
    let content = content.clone();
    let row_id = node.row_id();
    rsx! { RenderedTextNode { content, row_id } }
}

#[component]
fn RenderedTextNode(content: String, row_id: Option<String>) -> Element {
    // Prefer the node's own row id (GPUI parity); an enclosing live_block's
    // EntityContext covers nodes whose interpretation dropped it.
    let entity_id = row_id.or_else(|| try_consume_context::<EntityContext>().map(|c| c.0));

    let empty = content.is_empty();
    // Mirror `editable_text`'s empty-placeholder hint so unfocused empty
    // blocks still read as clickable instead of "nothing here".
    let display = if empty {
        "Type here to add a new block".to_string()
    } else {
        content.clone()
    };
    let style = if empty {
        "white-space: pre-wrap; word-break: break-word; min-height: 1.4em; padding: 1px 2px; \
         cursor: text; color: rgba(128,128,128,0.5);"
    } else {
        "white-space: pre-wrap; word-break: break-word; min-height: 1.4em; padding: 1px 2px; \
         cursor: text;"
    };

    rsx! {
        div {
            "data-role": "rendered-text",
            style: "{style}",
            onclick: move |_| {
                let Some(eid) = entity_id.clone() else {
                    tracing::error!(
                        "[rendered_text] click without row_id or EntityContext — focus dropped"
                    );
                    return;
                };
                let Some(bridge) = crate::BRIDGE.with(|b| b.borrow().clone()) else {
                    tracing::error!("[rendered_text] BRIDGE missing on click {eid}");
                    return;
                };
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = bridge
                        .call(
                            "engineSetFocus",
                            [eid.clone().into(), wasm_bindgen::JsValue::NULL],
                        )
                        .await
                    {
                        tracing::error!("[rendered_text] engineSetFocus({eid}) failed: {e}");
                    }
                });
            },
            "{display}"
        }
    }
}
