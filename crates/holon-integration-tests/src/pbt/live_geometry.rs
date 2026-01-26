//! Process-wide bridge for "what is currently rendered on screen".
//!
//! `ReferenceStateMachine::transitions(state)` is a static-ish hook that
//! gets only `&Self::State`. The atomic editor primitives (FocusEditableText
//! etc.) need to filter their candidate set to *blocks that are actually
//! mounted in the live GPUI tree* — without that filter, the generator
//! happily proposes blocks the ref model knows about (CDC lag, ghost
//! matview rows from inv10i, peer-pending) but the SUT can't click.
//!
//! `gpui_ui_pbt::main` installs an `Arc<dyn GeometryProvider>` once at
//! window-up time. The PBT thread reads it from the precondition and
//! generator. Headless runs leave it unset; callers fall back to ref-state
//! filtering (the existing pre-atomic-editor behavior).

use std::sync::{Arc, OnceLock};

use holon_frontend::geometry::GeometryProvider;

static LIVE_GEOMETRY: OnceLock<Arc<dyn GeometryProvider>> = OnceLock::new();

/// Install the live geometry source. Must be called once, before the PBT
/// generator runs the first post-startup `transitions()` step. Calling
/// twice is a no-op (OnceLock semantics).
pub fn install(provider: Arc<dyn GeometryProvider>) {
    let _ = LIVE_GEOMETRY.set(provider);
}

/// Snapshot of `entity_id`s of every element currently rendered with
/// `has_content == true`. Returns `None` when no live geometry is
/// installed (headless runs).
pub fn rendered_entity_ids() -> Option<std::collections::HashSet<String>> {
    let provider = LIVE_GEOMETRY.get()?;
    Some(
        provider
            .all_elements()
            .into_iter()
            .filter_map(|(_, info)| {
                if info.has_content {
                    info.entity_id
                } else {
                    None
                }
            })
            .collect(),
    )
}

/// Whether `entity_id` is currently rendered (regardless of has_content).
/// Used by the precondition: a block is focusable only if it has bounds
/// in the live tree.
pub fn is_entity_rendered(entity_id: &str) -> bool {
    let Some(provider) = LIVE_GEOMETRY.get() else {
        return false;
    };
    provider
        .all_elements()
        .iter()
        .any(|(_, info)| info.entity_id.as_deref() == Some(entity_id))
}
