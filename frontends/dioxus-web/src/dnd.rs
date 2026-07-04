//! Block drag & drop state for the web renderer.
//!
//! HTML5 drag events fire on different elements (`draggable` source,
//! `drop_zone` target), so the dragged block id is parked here between
//! `dragstart` and `drop`. thread_local for the same reason as `BRIDGE`:
//! wasm is single-threaded and the payload is !Send-adjacent page state.
//!
//! GPUI equivalent: `render/drag.rs` `DraggedBlock` carried by
//! `on_drag`/`on_drop`. The dispatched intent is identical
//! (`build_drop_intent` → `move_block`).

use std::cell::RefCell;

thread_local! {
    static DRAG_SOURCE: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn start_drag(block_id: String) {
    tracing::info!("[dnd] drag start: {block_id}");
    DRAG_SOURCE.with(|s| *s.borrow_mut() = Some(block_id));
}

pub fn clear_drag() {
    DRAG_SOURCE.with(|s| *s.borrow_mut() = None);
}

/// The block id currently being dragged, if any. Non-consuming: `dragend`
/// (which always fires on the source, drop or no drop) does the cleanup.
pub fn current_drag() -> Option<String> {
    DRAG_SOURCE.with(|s| s.borrow().clone())
}
