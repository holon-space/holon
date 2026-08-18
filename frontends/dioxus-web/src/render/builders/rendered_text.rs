use holon_api::EntityName;
use holon_api::EntityRef;
use holon_api::StyleFlags;
use holon_api::Value;
use holon_api::link_parser::LinkTargetClassifier;
use holon_frontend::link_segments::LinkClickAction;
use holon_frontend::link_segments::StyledSegment;
use holon_frontend::link_segments::link_click_action;
use holon_frontend::link_segments::marks_of;
use holon_frontend::link_segments::nav_focus;
use holon_frontend::link_segments::styled_link_segments;
use holon_frontend::link_segments::wants_styled_render;
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
/// carry any mark, the content is split into styled/link runs
/// (`holon_frontend::link_segments`): link runs are clickable and route
/// through the shared `link_click_action`, and every run paints its own
/// bold/italic/underline/strike/code attributes.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::RenderedText { content, .. } = &node.kind else {
        return rsx! {};
    };
    let content = content.clone();
    let row_id = node.row_id();
    let marks = marks_of(&node.entity);

    if wants_styled_render(&content, &marks) {
        let segments = styled_link_segments(&content, &marks);
        rsx! { StyledTextNode { segments, row_id } }
    } else {
        rsx! { RenderedTextNode { content, row_id } }
    }
}

#[component]
fn RenderedTextNode(content: String, row_id: Option<String>) -> Element {
    // Prefer the node's own row id (GPUI parity); an enclosing live_block's
    // EntityContext covers nodes whose interpretation dropped it.
    let entity_id = row_id.or_else(|| try_consume_context::<EntityContext>().map(|c| c.0));

    // An empty block paints nothing (ruling D5B-8.a); `min-height` is what
    // keeps its row clickable.
    let display = content.clone();
    let style = "white-space: pre-wrap; word-break: break-word; min-height: 1.4em; padding: 1px \
                 2px; cursor: text;";

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

/// Content with marks. Plain runs behave like `RenderedTextNode`
/// (click-to-focus) and carry their style attributes; link runs are visually
/// distinct and clickable — routed by the shared `link_click_action`.
#[component]
fn StyledTextNode(segments: Vec<StyledSegment>, row_id: Option<String>) -> Element {
    let entity_id = row_id.or_else(|| try_consume_context::<EntityContext>().map(|c| c.0));
    let dom_entity_id = entity_id.clone().unwrap_or_default();

    let style = "white-space: pre-wrap; word-break: break-word; min-height: 1.4em; \
                 padding: 1px 2px; cursor: text;";

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
                    None => rsx! {
                        span { key: "{i}", style: "{run_style(&seg.flags)}", "{seg.text}" }
                    },
                    Some(EntityRef::External { url }) => {
                        rsx! {
                            a {
                                key: "{i}",
                                "data-role": "link",
                                href: "{url}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                style: "{link_run_style(&seg.flags)}",
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
                                style: "{link_run_style(&seg.flags)}",
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

/// CSS for one run's paint attributes. The mark→attribute mapping itself lives
/// in `holon_api::mark_style_flags`; this only spells the flags as CSS.
fn run_style(flags: &StyleFlags) -> String {
    let mut css = String::new();
    if flags.bold {
        css.push_str("font-weight: 600;");
    }
    if flags.italic {
        css.push_str("font-style: italic;");
    }
    let mut decorations: Vec<&str> = Vec::new();
    if flags.underline {
        decorations.push("underline");
    }
    if flags.strikethrough {
        decorations.push("line-through");
    }
    if !decorations.is_empty() {
        css.push_str(&format!("text-decoration: {};", decorations.join(" ")));
    }
    if flags.background {
        css.push_str(
            "background: rgba(128,128,128,0.16); border-radius: 3px; padding: 0 3px; \
             font-family: ui-monospace, SFMono-Regular, Menlo, monospace;",
        );
    }
    css
}

/// Link runs add only the affordance. The underline comes from `run_style`,
/// because `mark_style_flags` already gives a `Link` mark `underline` — adding
/// a second `text-decoration` here would be overridden by that one anyway, and
/// would drop the line-through on a struck-out link.
fn link_run_style(flags: &StyleFlags) -> String {
    format!("color: #2f6feb; cursor: pointer;{}", run_style(flags))
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

/// Click on a link run targeting an entity. The verb is decided by the shared
/// `link_click_action`, so the web applies the same two gates as GPUI: an
/// unregistered scheme must never mint a page, and a registered-but-viewless
/// scheme (`tag:`, `person:`) must not navigate the main region to an empty
/// panel.
///
/// The classifier carries only `BUILT_IN_LINK_SCHEMES` — the profile registry
/// lives in the worker. That costs nothing here: navigation additionally
/// requires the `block` scheme, which is built in, so every target this
/// frontend can navigate to is one a full registry would classify identically.
fn follow_internal_link(target: &EntityRef) {
    let intent = match link_click_action(Some(target), &LinkTargetClassifier::default()) {
        LinkClickAction::Navigate(uri) => nav_focus(uri),
        LinkClickAction::FollowDangling(name) => {
            // GAP vs GPUI: GPUI's `follow_dangling_link` creates the page AND
            // navigates to the fresh leaf in one gesture, threading the create
            // op's response (the new page id) into a `navigation.focus`. The
            // worker exposes no such response-threading export today, and the
            // dispatch lane (`engineDispatchIntents`) is fire-and-forget, so we
            // dispatch the create+heal op only. The link heals on the next
            // reprojection and the second click navigates. Same-gesture
            // navigation needs an `engine_follow_dangling_link` worker export
            // — see BugFunnel.
            OperationIntent::new(
                EntityName::new("block"),
                "create_page_from_link".to_string(),
                [("target".to_string(), Value::String(name))]
                    .into_iter()
                    .collect(),
            )
        }
        // A caret seed needs a hit-tested byte offset, which this frontend's
        // click path does not produce; the enclosing div's click already
        // focuses the block, so an inert link run is the honest outcome.
        // `OpenUrl` never reaches here — external targets take the anchor arm.
        LinkClickAction::SeedCaret | LinkClickAction::OpenUrl(_) => return,
    };
    dispatch_chain(vec![intent_to_wire(&intent)]);
}
