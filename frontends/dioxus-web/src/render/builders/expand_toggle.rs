use holon_api::render_types::OperationWiring;
use holon_api::types::EntityName;
use holon_frontend::expand_toggle::expand_toggle_effects;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;
use crate::render::SNAPSHOT_SEQ;

/// Chevron expand/collapse. The gate's authority is the worker's ViewModel —
/// a click sends both legs
/// `holon_frontend::expand_toggle::expand_toggle_effects` decides, so collapse
/// is undoable, provenance-tagged, syncs, and the lazy content materialises
/// into the next snapshot.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::ExpandToggle {
        target_id,
        expanded,
        content_deferred,
        children,
    } = &node.kind
    else {
        return rsx! {};
    };
    rsx! {
        ExpandToggleNode {
            key: "{target_id}",
            target_id: target_id.clone(),
            snapshot_expanded: *expanded,
            content_deferred: *content_deferred,
            children_vm: children.items.clone(),
            operations: node.operations.clone(),
            entity_name: node.entity_name(),
            row_id: node.row_id(),
        }
    }
}

#[component]
fn ExpandToggleNode(
    target_id: String,
    snapshot_expanded: bool,
    content_deferred: bool,
    children_vm: Vec<ViewModel>,
    operations: Vec<OperationWiring>,
    entity_name: Option<EntityName>,
    row_id: Option<String>,
) -> Element {
    // Optimism, bounded: `(snapshot sequence at click, value asked for)`. The
    // glyph must move on the click — the authoritative flip costs a worker
    // round trip, the whole interaction budget — but the prediction is valid
    // ONLY while the sequence still reads what it read at click time. The next
    // delivery ends it whatever it says, so an external fold, an undo, or a
    // rejected write all leave the worker's value on screen rather than the
    // page's stale guess.
    let mut prediction = use_signal(|| None::<(u64, bool)>);
    let mut failure = use_signal(|| None::<String>);
    let seq = SNAPSHOT_SEQ();
    let open = match prediction() {
        Some((at, wanted)) if at == seq => wanted,
        _ => snapshot_expanded,
    };
    let chevron = if open { "▾" } else { "▸" };
    rsx! {
        div {
            "data-role": "expand-toggle",
            "data-target-id": "{target_id}",
            "data-content-deferred": "{content_deferred}",
            span {
                style: "cursor: pointer; user-select: none; color: #888; display: inline-block; width: 1em;",
                onclick: move |_| {
                    // Read the CURRENT effective state, not the state this
                    // element rendered against, so a second click before the
                    // snapshot lands toggles back the way GPUI's does.
                    let want = !match prediction() {
                        Some((at, wanted)) if at == seq => wanted,
                        _ => snapshot_expanded,
                    };
                    prediction.set(Some((seq, want)));
                    failure.set(None);
                    crate::editor::dispatch_expand_toggle(
                        expand_toggle_effects(
                            &target_id,
                            want,
                            &operations,
                            entity_name.as_ref(),
                            row_id.as_deref(),
                        ),
                        move |err| {
                            prediction.set(None);
                            failure.set(Some(err));
                        },
                    );
                },
                "{chevron}"
            }
            if let Some(err) = failure() {
                span {
                    "data-role": "expand-toggle-error",
                    style: "color: #d9534f; cursor: help; user-select: none;",
                    title: "{err}",
                    "⚠"
                }
            }
            if open {
                div { style: "padding-left: 12px;",
                    for (key, child) in keyed_children(&children_vm) {
                        RenderNode { key: "{key}", node: child.clone() }
                    }
                }
            }
        }
    }
}
