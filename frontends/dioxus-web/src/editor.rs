//! `EditorCell` — an uncontrolled `contenteditable` Dioxus component with
//! path-based cursor preservation.
//!
//! # Why uncontrolled
//!
//! A naive `div { contenteditable: "true", "{content}" }` puts the content
//! in the VDOM as a text child. Dioxus diffs and overwrites the text node
//! on every re-render — which fights the browser's local contenteditable
//! state (cursor, IME composition, pending input events). The result is
//! flicker, cursor jumps, and lost keystrokes during fast typing.
//!
//! Instead, we render an empty `<div contenteditable>` and sync its
//! `textContent` imperatively from a `use_effect` that runs after each
//! render. When the element is focused we skip the overwrite entirely:
//! the user owns the local state until they blur. Remote updates while
//! focused are deferred until the next re-render that catches the cell
//! unfocused — acceptable in practice and the only sane default without
//! a full operational-transform layer.
//!
//! # Cursor preservation
//!
//! Structural re-renders (e.g. a parent list re-keys the cell's subtree)
//! unmount and remount `EditorCell`, which blurs the old element. To keep
//! the cursor where the user left it, `main.rs` calls [`cursor::save`]
//! before setting the ViewModel signal and [`cursor::enqueue_restore`]
//! with the result. After the effect's `set_text_content` runs on the
//! new element, [`cursor::apply_pending_if_matches`] focuses it and
//! re-applies the saved selection.
//!
//! Cursor offsets are flat UTF-16 indices over the element's concatenated
//! descendant text content (computed by a tree walker). This handles
//! multi-line content, multiple text nodes, and embedded spans — the
//! previous "first text node only" implementation silently broke on all
//! of those.
//!
//! # Update dispatch
//!
//! Keystrokes are debounced per entity before hitting the worker. Without
//! this, fast typing generates one `block.update` op per character, each
//! of which triggers a full ViewModel re-serialize. The debounce window
//! is [`DISPATCH_DEBOUNCE_MS`] — short enough to feel responsive, long
//! enough to coalesce a typing burst into one op.

use std::cell::RefCell;
use std::collections::HashMap;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;

use crate::BRIDGE;

/// Trailing-edge debounce window for `dispatch_content_update`, in ms.
/// Fast typists produce ~8 keystrokes / 50ms; this window coalesces a
/// typing burst into one worker op without making the UI feel lagged.
const DISPATCH_DEBOUNCE_MS: u32 = 50;

/// A single editable text cell. Renders as an empty `contenteditable`
/// div whose text is set imperatively by a `use_effect`.
///
/// `entity_id` must be a stable entity URI so cursor restoration can find
/// the element after a ViewModel re-render. An empty id is a bug — keystrokes
/// on such a cell are dropped with a loud log so misconfiguration surfaces.
#[component]
pub fn EditorCell(entity_id: String, content: String) -> Element {
    // Reconcile DOM text with the latest prop after each render.
    let id_for_effect = entity_id.clone();
    let content_for_effect = content.clone();
    use_effect(move || {
        sync_dom_to_prop(&id_for_effect, &content_for_effect);
    });

    rsx! {
        div {
            "data-entity-id": "{entity_id}",
            "data-role": "editor-cell",
            contenteditable: "true",
            style: "outline: none; white-space: pre-wrap; word-break: break-word; min-height: 1.4em; padding: 1px 2px;",
            oninput: {
                let eid = entity_id.clone();
                move |evt: Event<FormData>| {
                    let new_content = evt.data().value();
                    schedule_content_update(eid.clone(), new_content);
                }
            },
            onkeydown: move |evt: KeyboardEvent| {
                // Prevent Enter from inserting a <div> / <br> in Chrome.
                // Actual newline handling should go through an explicit
                // split-block operation (not yet wired).
                if evt.key() == Key::Enter && !evt.modifiers().shift() {
                    evt.prevent_default();
                }
            },
            // NOTE: no body. The content is set imperatively via
            // sync_dom_to_prop so Dioxus does not overwrite the text
            // node on every render — contenteditable would then lose
            // cursor and IME state.
        }
    }
}

/// Reconcile the contenteditable's `textContent` with the latest prop.
///
/// - `current == content`: nothing to do. Drop any stale pending cursor
///   restore so a later trigger doesn't misfire.
/// - `current != content` and the element is **focused**: the user is
///   actively editing. Their local state wins; skip the overwrite and
///   drop any stale pending restore.
/// - `current != content` and the element is **not focused**: safe to
///   overwrite. Apply an enqueued restore for this entity (this is how
///   the cursor survives a structural re-render / remount).
fn sync_dom_to_prop(entity_id: &str, content: &str) {
    let Some(el) = cursor::find_element(entity_id) else {
        return;
    };
    let current = el.text_content().unwrap_or_default();
    if current == content {
        cursor::drop_pending_if_matches(entity_id);
        return;
    }
    if cursor::is_element_focused(&el) {
        cursor::drop_pending_if_matches(entity_id);
        return;
    }
    el.set_text_content(Some(content));
    cursor::apply_pending_if_matches(entity_id);
}

thread_local! {
    /// Per-entity pending debounced dispatch. Overwriting a value drops
    /// the previous `Timeout`, which cancels the pending callback.
    static PENDING_DISPATCH: RefCell<HashMap<String, Timeout>> =
        RefCell::new(HashMap::new());
}

/// Schedule a trailing-edge debounced `block.update` for `entity_id`.
///
/// Called from every keystroke. The previous pending dispatch for the
/// same entity is cancelled by overwriting its `Timeout` in the map —
/// gloo's `Timeout` cancels on drop — so a burst of N keystrokes results
/// in exactly one worker op carrying the final content.
fn schedule_content_update(entity_id: String, content: String) {
    if entity_id.is_empty() {
        tracing::error!("[EditorCell] dropped keystroke: empty entity_id — missing EntityContext?");
        return;
    }

    let eid_for_closure = entity_id.clone();
    let timeout = Timeout::new(DISPATCH_DEBOUNCE_MS, move || {
        PENDING_DISPATCH.with(|cell| {
            cell.borrow_mut().remove(&eid_for_closure);
        });
        dispatch_content_update_now(eid_for_closure.clone(), content.clone());
    });

    PENDING_DISPATCH.with(|cell| {
        cell.borrow_mut().insert(entity_id, timeout);
    });
}

/// Fire the actual `engineExecuteOperation` RPC. Used only by the
/// debounced scheduler above — do not call directly from event handlers.
fn dispatch_content_update_now(entity_id: String, content: String) {
    let bridge = BRIDGE.with(|cell| cell.borrow().clone());
    let Some(bridge) = bridge else {
        tracing::error!("[EditorCell] BRIDGE not initialized — dropping update");
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        let params = serde_json::json!({
            "id": entity_id,
            "content": content,
        });
        if let Err(e) = bridge
            .call(
                "engineExecuteOperation",
                ["block".into(), "update".into(), params.to_string().into()],
            )
            .await
        {
            tracing::error!("[EditorCell] execute_operation failed: {e}");
        }
    });
}

/// Cursor save/restore for `contenteditable` elements using a flat
/// UTF-16 offset scheme over the descendant text-node sequence.
///
/// `anchor` and `focus` are counted across all descendant text nodes in
/// document order, matching what `Selection` reports locally but reduced
/// to a pair of integers that survives re-render and can be re-inflated
/// via a second tree walk. This supports multi-line and multi-text-node
/// content; the previous implementation silently failed on both.
pub mod cursor {
    use std::cell::RefCell;

    use wasm_bindgen::JsCast;
    use web_sys::{window, Element, HtmlElement, Node};

    /// A saved cursor position for a specific entity. Offsets are flat
    /// UTF-16 code units from the start of the element's concatenated
    /// descendant text content.
    pub struct SavedCursor {
        pub entity_id: String,
        pub anchor: u32,
        pub focus: u32,
    }

    /// Look up an editable element by its `data-entity-id` attribute.
    pub fn find_element(entity_id: &str) -> Option<Element> {
        let selector = format!("[data-entity-id=\"{entity_id}\"]");
        window()?.document()?.query_selector(&selector).ok()?
    }

    /// True if `el` is the document's `activeElement`.
    pub fn is_element_focused(el: &Element) -> bool {
        let Some(doc) = window().and_then(|w| w.document()) else {
            return false;
        };
        let Some(active) = doc.active_element() else {
            return false;
        };
        let active_node: &Node = active.as_ref();
        let el_node: &Node = el.as_ref();
        active_node.is_same_node(Some(el_node))
    }

    /// Walk `root`'s descendant text nodes in document order. Recursion
    /// is bounded by the editable subtree depth — contenteditable cells
    /// are shallow in practice, so a stack-safe iterative walker would
    /// be overkill.
    fn collect_text_nodes(root: &Node, out: &mut Vec<Node>) {
        let mut child_opt = root.first_child();
        while let Some(child) = child_opt {
            let next = child.next_sibling();
            let ty = child.node_type();
            if ty == Node::TEXT_NODE {
                out.push(child);
            } else if ty == Node::ELEMENT_NODE {
                collect_text_nodes(&child, out);
            }
            child_opt = next;
        }
    }

    /// UTF-16 code-unit count. Matches what DOM `Selection` offsets use,
    /// unlike Rust's byte-oriented `str::len`.
    fn utf16_len(s: &str) -> u32 {
        s.encode_utf16().count() as u32
    }

    /// Convert a `(node, local_offset)` DOM position inside `el` into a
    /// flat UTF-16 offset from the start of `el`'s text content.
    /// Returns `None` if `node` is not a descendant text node of `el`.
    fn flatten_offset(el: &Element, node: &Node, local_off: u32) -> Option<u32> {
        let el_node: &Node = el.as_ref();
        if node.is_same_node(Some(el_node)) {
            // Selection set at the element itself — treat as offset 0.
            return Some(0);
        }
        let mut nodes = Vec::new();
        collect_text_nodes(el_node, &mut nodes);
        let mut acc = 0u32;
        for n in &nodes {
            if n.is_same_node(Some(node)) {
                return Some(acc + local_off);
            }
            acc += utf16_len(&n.text_content().unwrap_or_default());
        }
        None
    }

    /// Locate the `(text_node, local_offset)` matching a flat offset.
    /// Past-end offsets clamp to the end of the last text node; `None`
    /// means the element has no text nodes at all.
    fn locate_offset(el: &Element, flat: u32) -> Option<(Node, u32)> {
        let el_node: &Node = el.as_ref();
        let mut nodes = Vec::new();
        collect_text_nodes(el_node, &mut nodes);
        if nodes.is_empty() {
            return None;
        }
        let mut acc = 0u32;
        for n in &nodes {
            let len = utf16_len(&n.text_content().unwrap_or_default());
            if acc + len >= flat {
                return Some((n.clone(), flat - acc));
            }
            acc += len;
        }
        let last = nodes.last().unwrap().clone();
        let len = utf16_len(&last.text_content().unwrap_or_default());
        Some((last, len))
    }

    /// Save the cursor for the currently focused editable element.
    /// Returns `None` if no editor cell is focused, or if the selection
    /// is pointing outside the element's descendant tree (shouldn't
    /// happen in practice).
    pub fn save() -> Option<SavedCursor> {
        let win = window()?;
        let doc = win.document()?;
        let active = doc.active_element()?;
        let entity_id = active.get_attribute("data-entity-id")?;

        let sel = win.get_selection().ok()??;
        let anchor_node = sel.anchor_node()?;
        let focus_node = sel.focus_node()?;

        let anchor = flatten_offset(&active, &anchor_node, sel.anchor_offset())?;
        let focus = flatten_offset(&active, &focus_node, sel.focus_offset())?;

        Some(SavedCursor {
            entity_id,
            anchor,
            focus,
        })
    }

    /// Apply a flat saved selection to `el`. Focuses the element first
    /// so the selection is visible and inputs land there.
    fn restore_flat(el: &Element, anchor: u32, focus: u32) {
        let Some(win) = window() else { return };
        let Ok(Some(sel)) = win.get_selection() else {
            return;
        };

        if let Some(html) = el.dyn_ref::<HtmlElement>() {
            let _ = html.focus();
        }

        let el_node: &Node = el.as_ref();

        // Empty editable (first mount, or content was cleared): collapse
        // the selection at element start. set_base_and_extent on an
        // element node with offset 0 is valid and places the caret at
        // the beginning.
        if el_node.first_child().is_none() {
            let _ = sel.set_base_and_extent(el_node, 0, el_node, 0);
            return;
        }

        let Some((anchor_node, local_anchor)) = locate_offset(el, anchor) else {
            return;
        };
        let Some((focus_node, local_focus)) = locate_offset(el, focus) else {
            return;
        };

        // set_base_and_extent handles reversed ranges (focus before
        // anchor) natively, unlike Range::set_start / set_end which
        // require ordered positions.
        let _ = sel.set_base_and_extent(&anchor_node, local_anchor, &focus_node, local_focus);
    }

    /// Enqueue a restore to be applied after the next render.
    ///
    /// Single-slot: enqueuing a second time before the previous one is
    /// consumed drops the older restore. Only one editor can hold focus
    /// at a time so a single slot is sufficient.
    pub fn enqueue_restore(saved: SavedCursor) {
        PENDING.with(|cell| *cell.borrow_mut() = Some(saved));
    }

    /// If a pending restore matches `entity_id`, take it out of the slot
    /// and apply it. Called from `EditorCell`'s effect right after
    /// overwriting `textContent` on a non-focused element.
    pub fn apply_pending_if_matches(entity_id: &str) {
        // Take the pending slot atomically so we don't hold the borrow
        // across DOM mutation (restore_flat reaches into web-sys).
        let taken = PENDING.with(|cell| {
            let mut opt = cell.borrow_mut();
            if opt.as_ref().map(|s| s.entity_id.as_str()) == Some(entity_id) {
                opt.take()
            } else {
                None
            }
        });
        if let Some(saved) = taken {
            if let Some(el) = find_element(&saved.entity_id) {
                restore_flat(&el, saved.anchor, saved.focus);
            }
        }
    }

    /// Drop a pending restore whose entity matches — used when the
    /// element is focused and already has the correct cursor, so any
    /// enqueued restore would snap the user backwards.
    pub fn drop_pending_if_matches(entity_id: &str) {
        PENDING.with(|cell| {
            let mut opt = cell.borrow_mut();
            if opt.as_ref().map(|s| s.entity_id.as_str()) == Some(entity_id) {
                *opt = None;
            }
        });
    }

    thread_local! {
        static PENDING: RefCell<Option<SavedCursor>> = const { RefCell::new(None) };
    }
}
