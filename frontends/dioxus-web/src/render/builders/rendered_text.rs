use holon_api::EntityName;
use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Value;
use holon_api::marks_from_json;
use holon_frontend::link_segments::ContentSegment;
use holon_frontend::link_segments::link_content_segments;
use holon_frontend::operations::OperationIntent;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;
use crate::editor::dispatch_chain;
use crate::editor::intent_to_wire;
use crate::render::EntityContext;

/// Read-only sibling of `editable_text` (mirrors GPUI
/// `render/builders/rendered_text.rs`). A click on plain text calls
/// `engineSetFocus` (ADR 0010: focus is pure in-memory worker state), which
/// flips the `is_focused` variant so the next snapshot swaps in
/// `editable_text`; `worker_focus::apply` then moves DOM focus into the
/// mounted editor.
///
/// When the block's `marks` (read off `node.entity`, same source GPUI reads)
/// carry `InlineMark::Link` spans, the content is split into text/link runs
/// (`holon_frontend::link_segments`) and each link renders as a clickable
/// element that navigates the main region — GPUI parity for link rendering,
/// which this frontend previously dropped (rendered every block as plain
/// text).
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::RenderedText { content, .. } = &node.kind else {
        return rsx! {};
    };
    let content = content.clone();
    let row_id = node.row_id();
    let marks = link_marks(&node.entity);
    let has_links = marks
        .iter()
        .any(|m| matches!(m.mark, InlineMark::Link { .. }));

    if has_links && !content.is_empty() {
        let segments = link_content_segments(&content, &marks);
        rsx! { LinkedTextNode { segments, row_id } }
    } else {
        rsx! { RenderedTextNode { content, row_id } }
    }
}

/// Extract the block's inline marks from its entity row. Fail loud on
/// malformed JSON: stored `blocks.marks` must be valid (same contract GPUI's
/// `rendered_text` / `text` builders enforce).
fn link_marks(entity: &holon_api::DataRow) -> Vec<MarkSpan> {
    match entity.get("marks") {
        Some(Value::String(s)) | Some(Value::Json(s)) if !s.is_empty() && s != "[]" => {
            marks_from_json(s).expect("blocks.marks must be valid JSON")
        }
        _ => Vec::new(),
    }
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
        "Type here".to_string()
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

    let dom_entity_id = entity_id.clone().unwrap_or_default();
    rsx! {
        div {
            "data-role": "rendered-text",
            // Consumed by editor::focus_ring for cross-block caret nav.
            "data-entity-id": "{dom_entity_id}",
            style: "{style}",
            onclick: move |_| focus_block(entity_id.clone()),
            "{display}"
        }
    }
}

/// Content with at least one link mark. Plain runs behave like
/// `RenderedTextNode` (click-to-focus); link runs are visually distinct and
/// clickable — navigating the main region on click.
#[component]
fn LinkedTextNode(segments: Vec<ContentSegment>, row_id: Option<String>) -> Element {
    let entity_id = row_id.or_else(|| try_consume_context::<EntityContext>().map(|c| c.0));
    let dom_entity_id = entity_id.clone().unwrap_or_default();

    let style = "white-space: pre-wrap; word-break: break-word; min-height: 1.4em; \
                 padding: 1px 2px; cursor: text;";
    let link_style = "color: #2f6feb; text-decoration: underline; cursor: pointer;";

    rsx! {
        div {
            "data-role": "rendered-text",
            "data-entity-id": "{dom_entity_id}",
            style: "{style}",
            onclick: {
                let entity_id = entity_id.clone();
                move |_| focus_block(entity_id.clone())
            },
            for (i , seg) in segments.iter().enumerate() {
                match &seg.link_target {
                    None => rsx! { span { key: "{i}", "{seg.text}" } },
                    Some(EntityRef::External { url }) => {
                        rsx! {
                            a {
                                key: "{i}",
                                "data-role": "link",
                                href: "{url}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                style: "{link_style}",
                                onclick: move |evt| evt.stop_propagation(),
                                "{seg.text}"
                            }
                        }
                    }
                    Some(target) => {
                        let target = target.clone();
                        rsx! {
                            span {
                                key: "{i}",
                                "data-role": "link",
                                style: "{link_style}",
                                onclick: move |evt| {
                                    evt.stop_propagation();
                                    follow_internal_link(&target);
                                },
                                "{seg.text}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Click on a plain (non-link) run: request read→edit focus for the block via
/// the worker, matching `RenderedTextNode`.
fn focus_block(entity_id: Option<String>) {
    let Some(eid) = entity_id else {
        tracing::error!("[rendered_text] click without row_id or EntityContext — focus dropped");
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
}

/// Click on a link run targeting an entity. A target that names an entity
/// navigates the main region to it (GPUI parity: `navigation.focus`). `Name`
/// (dangling wiki-link) lazily creates+heals the page chain via
/// `block.create_page_from_link`; see the gap note below.
fn follow_internal_link(target: &EntityRef) {
    let intent = match target {
        // A colon-bearing target that names no entity (`Meeting/Notes:2026`)
        // has nothing to navigate to and must never mint a page, so the click
        // is inert — matching GPUI, which places the caret instead.
        EntityRef::Scheme { .. } => {
            let Some(uri) = target.entity_uri() else {
                return;
            };
            OperationIntent::new(
                EntityName::new("navigation"),
                "focus".to_string(),
                [
                    ("region".to_string(), Value::String("main".to_string())),
                    ("block_id".to_string(), Value::String(uri.to_string())),
                ]
                .into_iter()
                .collect(),
            )
        }
        EntityRef::Name { name } => {
            // GAP vs GPUI: GPUI's `follow_dangling_link` creates the page AND
            // navigates to the fresh leaf in one gesture, threading the create
            // op's response (the new page id) into a `navigation.focus`. The
            // worker exposes no such response-threading export today, and the
            // dispatch lane (`engineDispatchIntents`) is fire-and-forget, so we
            // dispatch the create+heal op only. The link heals on the next
            // reprojection (this arm becomes `Internal`) and the second click
            // navigates. Same-gesture navigation for dangling links needs an
            // `engine_follow_dangling_link` worker export — see BugFunnel.
            OperationIntent::new(
                EntityName::new("block"),
                "create_page_from_link".to_string(),
                [("target".to_string(), Value::String(name.clone()))]
                    .into_iter()
                    .collect(),
            )
        }
        // External handled by the anchor arm; never reaches here.
        EntityRef::External { .. } => return,
    };
    dispatch_chain(vec![intent_to_wire(&intent)]);
}
