//! Per-row "a local edit reached this buffer" epoch.
//!
//! An async gesture that will overwrite an editor buffer it captured BEFORE a
//! round trip (the undo/redo re-seed) must know whether the user typed into
//! that row meanwhile. Sampling this epoch on both sides of the round trip
//! answers exactly that question, and nothing else: it carries no text, no
//! ordering across rows, and no persistence semantics.
//!
//! It is stamped by the single keystroke sink
//! ([`crate::editor_view_model::EditorViewModel::apply_local_edit`]) UPSTREAM
//! of the cell/no-cell fork, so it reports a local edit in every storage mode.
//! That is why it is not [`holon_api::write_seq`]: a `WriteSeq` is allocated
//! only where a SQL content write is issued, so in the Loro arm — the shipped
//! wiring — no keystroke moves it at all.
//!
//! Row-scoped rather than process-global: a gesture must refuse for edits to
//! ITS row, and an unrelated row's keystroke is not a reason to leave a stale
//! buffer on this one.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

use holon_api::EntityUri;

/// How many local edits a row's editor had committed when sampled. Comparable
/// only against another sample of the SAME row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalEditEpoch(u64);

static EPOCHS: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Record that a local edit landed in `row`'s editor buffer.
pub fn note(row: &EntityUri) {
    let mut epochs = EPOCHS.lock().unwrap();
    *epochs.entry(row.as_str().to_string()).or_insert(0) += 1;
}

/// Sample `row`'s epoch without advancing it.
pub fn current(row: &EntityUri) -> LocalEditEpoch {
    LocalEditEpoch(
        EPOCHS
            .lock()
            .unwrap()
            .get(row.as_str())
            .copied()
            .unwrap_or(0),
    )
}
