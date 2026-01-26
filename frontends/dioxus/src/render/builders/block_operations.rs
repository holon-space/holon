use std::sync::Arc;

use holon_frontend::FrontendSession;
use holon_frontend::operations::OperationIntent;
use holon_frontend::operations::find_ops_affecting;
use holon_frontend::view_model::ViewKind;

use super::dispatch::dispatch_intent;
use super::prelude::*;

/// Block fields whose operations a `block_operations` node exposes. Mirrors
/// `gpui/src/render/builders/block_operations.rs`.
const BLOCK_FIELDS: &[&str] = &["parent_id", "sort_key", "depth", "content"];

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::BlockOperations { .. } = &node.kind else {
        return rsx! {};
    };
    rsx! { BlockOperationsNode { node: node.clone() } }
}

/// A `[...]` affordance that dispatches the first block-mutating operation
/// wired to this node (indent / outdent / move / …) on click. Renders nothing
/// when no dispatchable operation is available — no fake disabled button.
#[component]
fn BlockOperationsNode(node: ViewModel) -> Element {
    // Hooks first (unconditional) so the early returns below can't reorder them.
    let session: Arc<FrontendSession> = use_context();
    let rt: tokio::runtime::Handle = use_context();

    let ops = find_ops_affecting(BLOCK_FIELDS, &node.operations);
    if ops.is_empty() {
        return rsx! {};
    }
    let Some(row_id) = node.row_id() else {
        return rsx! {};
    };
    let intent = OperationIntent::for_row(ops[0], &row_id, node.entity_name().as_ref());

    rsx! {
        span {
            style: "cursor: pointer; font-size: 0.75em; color: var(--text-muted); padding: 0 4px;",
            onmousedown: move |evt| {
                evt.stop_propagation();
                dispatch_intent(&rt, &session, intent.clone());
            },
            "[...]"
        }
    }
}
