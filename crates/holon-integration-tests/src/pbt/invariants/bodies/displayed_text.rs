//! Phase 7 — `inv-displayed-text` (DEFERRED).
//!
//! Inline body lives at `sut.rs:6775–6885`.
//!
//! # Why deferred
//!
//! The body requires:
//! - `self.frontend_geometry` — private `BoundsRegistry` handle (geometry elements
//!   with `displayed_text`, `entity_id`, `widget_type` per element)
//! - `self.doc_uri_map` / `self.resolve_uri` — private URI reverse map for split blocks
//! - `ref_state.active_editor.in_memory_content` — ref-side in-memory editor text
//! - `ref_state.block_state.blocks` — ref block content lookup
//! - `crate::pbt::panic_diag::diagnose_displayed_text` — internal diagnostic helper
//!
//! The capability surface would need a new `SutLayout::displayed_texts()` method
//! returning `Vec<(entity_id, widget_type, displayed_text)>` plus a `RefEditorMirror`
//! + `RefBlockTree` ref-side side. The `ref_state.active_editor` field is not yet in
//! any `Ref*` capability trait. Wire when those gaps are closed.

pub struct InvDisplayedText;

impl InvDisplayedText {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-displayed-text");
}

// `Invariant` impl intentionally omitted — body migration deferred.
