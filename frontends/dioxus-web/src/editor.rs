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
use holon_frontend::OperationIntent;
use holon_frontend::editor_view_model::EditorKey;
use holon_frontend::editor_view_model::structural_block_action;

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
            onfocusin: {
                let eid = entity_id.clone();
                move |_| {
                    // A direct click landed DOM focus here. Mirror it into
                    // the worker's in-memory focus authority (ADR 0010) so
                    // variant selection + focus_chain follow. Skipped when
                    // the focus originated FROM the worker (worker_focus
                    // already records it).
                    if worker_focus::note_local_focus(&eid) {
                        let Some(bridge) = BRIDGE.with(|cell| cell.borrow().clone()) else {
                            tracing::error!("[EditorCell] BRIDGE missing on focusin {eid}");
                            return;
                        };
                        let eid = eid.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Err(e) = bridge
                                .call("engineSetFocus", [eid.clone().into(), wasm_bindgen::JsValue::NULL])
                                .await
                            {
                                tracing::error!("[EditorCell] engineSetFocus({eid}) failed: {e}");
                            }
                        });
                    }
                }
            },
            onfocusout: {
                let eid = entity_id.clone();
                let prop_content = content.clone();
                move |_| {
                    let flushed = flush_pending_now(&eid);
                    // Focus moved away with nothing pending (e.g. a split
                    // moved it to the new block): the element may hold text
                    // this render already superseded — `sync_dom_to_prop`
                    // skipped it while it was focused, and no later render
                    // re-runs the effect. Reconcile now. When a flush WAS
                    // dispatched, the prop is older than the user's text;
                    // leave the DOM alone until the update echoes back.
                    if !flushed {
                        sync_dom_to_prop(&eid, &prop_content);
                    }
                }
            },
            onkeydown: {
                let eid = entity_id.clone();
                let prop_content = content.clone();
                move |evt: KeyboardEvent| {
                    if handle_caret_nav(&evt, &eid) {
                        return;
                    }
                    handle_structural_key(&evt, &eid, &prop_content);
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
/// - `current == content`: nothing to do. Drop any stale pending cursor restore
///   so a later trigger doesn't misfire.
/// - `current != content` and the element is **focused**: the user is actively
///   editing. Their local state wins; skip the overwrite and drop any stale
///   pending restore.
/// - `current != content` and the element is **not focused**: safe to
///   overwrite. Apply an enqueued restore for this entity (this is how the
///   cursor survives a structural re-render / remount).
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

/// Cross-block caret navigation (GPUI parity: MoveUp/MoveDown propagate to
/// the outline when the editor doesn't consume them). ArrowUp with a
/// collapsed caret at offset 0 focuses the previous block (caret at end —
/// the worker's default seed); ArrowDown with the caret at the end of the
/// text focuses the next block (caret 0). "Previous/next" is DOM document
/// order over every focusable cell (`editor-cell` + `rendered-text`), which
/// matches the visual outline order. Returns true when the event was
/// consumed by a focus jump.
fn handle_caret_nav(evt: &KeyboardEvent, entity_id: &str) -> bool {
    let dir: i32 = match evt.key() {
        Key::ArrowUp => -1,
        Key::ArrowDown => 1,
        _ => return false,
    };
    let m = evt.modifiers();
    if m.shift() || m.alt() || m.ctrl() || m.meta() {
        return false;
    }
    let Some(saved) = cursor::save().filter(|s| s.entity_id == entity_id && s.anchor == s.focus)
    else {
        return false;
    };
    let live_text = cursor::find_element(entity_id)
        .and_then(|el| el.text_content())
        .unwrap_or_default();
    let at_boundary = if dir < 0 {
        saved.focus == 0
    } else {
        saved.focus as usize == live_text.encode_utf16().count()
    };
    if !at_boundary {
        return false;
    }

    let ring = focus_ring();
    let Some(idx) = ring.iter().position(|id| id == entity_id) else {
        return false;
    };
    let target_idx = idx as i32 + dir;
    if target_idx < 0 || target_idx as usize >= ring.len() {
        return false;
    }
    let target = ring[target_idx as usize].clone();
    evt.prevent_default();

    let Some(bridge) = BRIDGE.with(|cell| cell.borrow().clone()) else {
        tracing::error!("[EditorCell] BRIDGE missing on caret nav from {entity_id}");
        return true;
    };
    // Caret seed: end-of-text default (NULL) when moving up, 0 when moving
    // down — mirrors how a caret visually "passes through" the boundary.
    let caret: wasm_bindgen::JsValue = if dir < 0 {
        wasm_bindgen::JsValue::NULL
    } else {
        0u32.into()
    };
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = bridge
            .call("engineSetFocus", [target.clone().into(), caret])
            .await
        {
            tracing::error!("[EditorCell] caret-nav engineSetFocus({target}) failed: {e}");
        }
    });
    true
}

/// Every focusable block cell in DOM document order: mounted editors plus
/// read-only `rendered_text` rows (which mount an editor once focused).
fn focus_ring() -> Vec<String> {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return Vec::new();
    };
    let Ok(list) = doc.query_selector_all(
        "[data-role=\"editor-cell\"][data-entity-id], \
         [data-role=\"rendered-text\"][data-entity-id]",
    ) else {
        tracing::error!("[EditorCell] focus_ring selector failed");
        return Vec::new();
    };
    let mut out = Vec::with_capacity(list.length() as usize);
    for i in 0..list.length() {
        use wasm_bindgen::JsCast;
        let Some(node) = list.item(i) else { continue };
        let Some(el) = node.dyn_ref::<web_sys::Element>() else {
            continue;
        };
        let Some(id) = el.get_attribute("data-entity-id") else {
            continue;
        };
        if !id.is_empty()
            && !holon_frontend::row_origin::RowOrigin::from_id(&id).is_creation_placeholder()
        {
            out.push(id);
        }
    }
    out
}

/// Route Enter / Backspace-at-0 / Tab / Shift+Tab through the shared
/// `structural_block_action` decision table (the same one GPUI's
/// `editor_view` capture handlers use), chained behind a pending-content
/// commit: structural ops are commit points (docs/Architecture/UI.md),
/// and the chain guarantees the flush lands before the structural op.
fn handle_structural_key(evt: &KeyboardEvent, entity_id: &str, prop_content: &str) {
    let shift = evt.modifiers().shift();
    let key = match evt.key() {
        Key::Enter if !shift => {
            // Always prevent the browser's <div>/<br> insertion, even if
            // the caret can't be read — a mangled DOM is worse than a
            // dropped split.
            evt.prevent_default();
            EditorKey::Enter
        }
        Key::Tab => {
            // The browser default would move focus out of the editor.
            evt.prevent_default();
            if shift {
                EditorKey::BackTab
            } else {
                EditorKey::Tab
            }
        }
        Key::Backspace if !shift => EditorKey::Backspace,
        _ => return,
    };

    let caret = cursor::save().filter(|s| s.entity_id == entity_id);
    let caret_collapsed_at = caret
        .as_ref()
        .filter(|s| s.anchor == s.focus)
        .map(|s| s.focus as usize);

    // Backspace is only structural with a collapsed caret at offset 0 —
    // anything else keeps the browser's default text deletion.
    if key == EditorKey::Backspace && caret_collapsed_at != Some(0) {
        return;
    }

    // Live DOM text: strictly newer than both the prop and any pending
    // debounced update. Used for the commit flush AND the split position.
    let live_text = cursor::find_element(entity_id)
        .and_then(|el| el.text_content())
        .unwrap_or_else(|| prop_content.to_string());

    let cursor_byte = match key {
        EditorKey::Enter => {
            let Some(utf16_off) = caret_collapsed_at else {
                tracing::error!(
                    "[EditorCell] Enter in {entity_id} without a collapsed caret — split dropped"
                );
                return;
            };
            utf16_to_byte_offset(&live_text, utf16_off)
        }
        _ => 0,
    };

    let Some(intent) = structural_block_action(key, entity_id, cursor_byte) else {
        return;
    };
    if key == EditorKey::Backspace {
        evt.prevent_default();
    }

    let mut chain = Vec::new();
    if let Some(flush) = take_pending_commit(entity_id, prop_content, &live_text) {
        chain.push(flush);
    }
    chain.push(intent_to_wire(&intent));
    dispatch_chain(chain);
}

/// Cancel the entity's pending debounced update and, if the live DOM text
/// diverged from the last-rendered prop, return a `block.update` wire
/// intent carrying it. MUST run before any structural dispatch for the
/// same entity — a stale debounce firing after a split would resurrect
/// the pre-split text into the truncated block.
fn take_pending_commit(
    entity_id: &str,
    prop_content: &str,
    live_text: &str,
) -> Option<serde_json::Value> {
    let had_pending = PENDING_DISPATCH
        .with(|cell| cell.borrow_mut().remove(entity_id))
        .is_some();
    if !had_pending && live_text == prop_content {
        return None;
    }
    Some(update_wire(entity_id, live_text))
}

/// Flush the pending debounced update for `entity_id` immediately with the
/// element's live text. Called on blur so the 50 ms window can't race a
/// subsequent snapshot into clobbering the user's final keystrokes.
/// Returns whether an update was dispatched.
fn flush_pending_now(entity_id: &str) -> bool {
    let Some(_pending) = PENDING_DISPATCH.with(|cell| cell.borrow_mut().remove(entity_id)) else {
        return false;
    };
    let Some(text) = cursor::find_element(entity_id).and_then(|el| el.text_content()) else {
        tracing::error!("[EditorCell] blur flush: element for {entity_id} gone — update dropped");
        return false;
    };
    dispatch_chain(vec![update_wire(entity_id, &text)]);
    true
}

pub(crate) fn update_wire(entity_id: &str, content: &str) -> serde_json::Value {
    // `set_field`, not `update`: the block OperationProvider registered by
    // EventInfraModule (`SqlBlockOperations`) exposes set_field / create /
    // delete + structural ops — it has NO "update" op, and a dispatched
    // "update" dies as UnknownOperation inside the worker. Same wire GPUI's
    // editor and state_toggle produce.
    serde_json::json!({
        "entity": "block",
        "op": "set_field",
        "params": { "id": entity_id, "field": "content", "value": content },
    })
}

pub(crate) fn intent_to_wire(intent: &OperationIntent) -> serde_json::Value {
    serde_json::json!({
        "entity": intent.entity_name.to_string(),
        "op": intent.op_name,
        "params": intent.params,
    })
}

/// Send an ordered intent chain through `engineDispatchIntents` — the
/// single write lane. All content + structural writes go through here so
/// the worker's `dispatch_intent_chain` serializes them; mixing lanes
/// with `engineExecuteOperation` (block_on) could interleave a spawned
/// chain with a blocking op in unspecified order.
pub(crate) fn dispatch_chain(intents: Vec<serde_json::Value>) {
    let Some(bridge) = BRIDGE.with(|cell| cell.borrow().clone()) else {
        tracing::error!("[EditorCell] BRIDGE not initialized — dropping intent chain");
        return;
    };
    let json = serde_json::Value::Array(intents).to_string();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = bridge.call("engineDispatchIntents", [json.into()]).await {
            tracing::error!("[EditorCell] engineDispatchIntents failed: {e}");
        }
    });
}

/// Convert a flat UTF-16 offset (DOM Selection units) into a byte offset
/// into `s` (what `split_block`'s `position` param expects). Past-end
/// offsets clamp to `s.len()`.
fn utf16_to_byte_offset(s: &str, utf16_off: usize) -> usize {
    let mut u16_seen = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        if u16_seen >= utf16_off {
            return byte_idx;
        }
        u16_seen += ch.len_utf16();
    }
    s.len()
}

/// Byte → flat UTF-16 offset (inverse of [`utf16_to_byte_offset`]) for
/// placing the caret from a worker caret seed. Past-end clamps.
fn byte_to_utf16_offset(s: &str, byte_off: usize) -> u32 {
    s.char_indices()
        .take_while(|(i, _)| *i < byte_off)
        .map(|(_, c)| c.len_utf16() as u32)
        .sum()
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

/// Fire the debounced content update through the single
/// `engineDispatchIntents` write lane. Used only by the debounced
/// scheduler above — do not call directly from event handlers.
fn dispatch_content_update_now(entity_id: String, content: String) {
    dispatch_chain(vec![update_wire(&entity_id, &content)]);
}

/// Worker-authoritative editor focus (ADR 0010: the worker's in-memory
/// `UiState.focused_block` is the single authority; the page's DOM focus
/// is a projection of it).
///
/// Every `WatchEnvelope` carries `{focused_block, caret_offset}` read
/// atomically with the interpretation. `apply` moves DOM focus to match:
/// the newly-focused block's editor may only mount in the re-render the
/// same envelope triggers, so application retries briefly until the
/// element exists. Moving DOM focus FIRST also un-focuses the previously
/// focused element, so `sync_dom_to_prop`'s "focused element keeps its
/// local text" rule can't pin a stale (e.g. pre-split) text.
pub mod worker_focus {
    use std::cell::RefCell;

    use wasm_bindgen::JsCast;

    thread_local! {
        /// Last focus applied or locally noted. `apply` no-ops when the
        /// envelope agrees, so user caret movement isn't disturbed by
        /// every snapshot.
        static CURRENT: RefCell<Option<String>> = const { RefCell::new(None) };
        /// Bumped per `apply` change so a stale retry loop aborts instead
        /// of stealing focus back.
        static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    /// Record a locally-initiated focus (direct click on an editor cell).
    /// Returns `true` when this is news — the caller then informs the
    /// worker via `engineSetFocus`; `false` when it merely echoes the
    /// worker's own focus application.
    pub fn note_local_focus(entity_id: &str) -> bool {
        CURRENT.with(|c| {
            let mut cur = c.borrow_mut();
            if cur.as_deref() == Some(entity_id) {
                return false;
            }
            *cur = Some(entity_id.to_string());
            true
        })
    }

    /// Apply the worker's focus state from a `WatchEnvelope`. `caret_byte`
    /// is the worker's one-shot caret seed (byte offset into the block's
    /// content); `None` defaults to end-of-text, matching GPUI's mount rule.
    pub fn apply(focused: Option<String>, caret_byte: Option<usize>) {
        let changed = CURRENT.with(|c| {
            let mut cur = c.borrow_mut();
            if *cur == focused {
                return false;
            }
            *cur = focused.clone();
            true
        });
        if !changed {
            return;
        }
        let generation = GENERATION.with(|g| {
            g.set(g.get() + 1);
            g.get()
        });

        let Some(target) = focused else {
            // Worker cleared focus: blur whichever editor cell holds it.
            if let Some(active) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.active_element())
            {
                if active.get_attribute("data-role").as_deref() == Some("editor-cell") {
                    if let Some(html) = active.dyn_ref::<web_sys::HtmlElement>() {
                        let _ = html.blur();
                    }
                }
            }
            return;
        };

        wasm_bindgen_futures::spawn_local(async move {
            // The editable variant for `target` renders in the re-render
            // this same envelope triggered; poll briefly for it to mount.
            for _ in 0..30u32 {
                if GENERATION.with(|g| g.get()) != generation {
                    return; // superseded by a newer focus change
                }
                if let Some(el) = super::cursor::find_element(&target) {
                    if super::cursor::is_element_focused(&el) {
                        return; // e.g. local click that we merely echo
                    }
                    let text = el.text_content().unwrap_or_default();
                    let utf16 = match caret_byte {
                        Some(b) => super::byte_to_utf16_offset(&text, b),
                        None => text.encode_utf16().count() as u32,
                    };
                    super::cursor::focus_and_place(&el, utf16);
                    return;
                }
                gloo_timers::future::TimeoutFuture::new(16).await;
            }
            tracing::error!(
                "[worker_focus] editor element for {target} never mounted — focus not applied"
            );
        });
    }
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
    use web_sys::Element;
    use web_sys::HtmlElement;
    use web_sys::Node;
    use web_sys::window;

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
        // Scoped to editor cells: rendered_text rows now carry
        // data-entity-id too (for focus_ring), and this lookup must never
        // hand sync_dom_to_prop a Dioxus-owned rendered-text div.
        let selector = format!("[data-role=\"editor-cell\"][data-entity-id=\"{entity_id}\"]");
        window()?.document()?.query_selector(&selector).ok()? // ALLOW(ok): DOM selector miss → None
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

        let sel = win.get_selection().ok()??; // ALLOW(ok): Selection API JsValue error → None
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

    /// Focus `el` and place a collapsed caret at flat UTF-16 offset
    /// `offset`. Used by [`super::worker_focus`] to project the worker's
    /// focus authority onto the DOM.
    pub(crate) fn focus_and_place(el: &Element, offset: u32) {
        restore_flat(el, offset, offset);
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
