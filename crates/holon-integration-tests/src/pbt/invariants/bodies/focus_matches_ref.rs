//! Phase 7 — `inv-focus-matches-ref`.
//!
//! Inline body lives at `sut.rs:6709–6773`.
//!
//! Checks that the reactive/frontend engine's global `focused_block`
//! matches the reference model's global focus after every focus-changing
//! transition. Skipped when:
//! - No frontend engine is installed (SqlOnly mode).
//! - The reference model has no global focus yet.
//! - An editor is open in the reference model (editor focus is the source
//!   of truth while editing; engine focus may lag).
//!
//! The comparison bridges the ref-model's synthetic URI (e.g. `block:ref-doc-0`)
//! to the engine's resolved UUID via `SutDriver::resolve_ref_block_id`.
//!
//! Status: functional.

use holon_pbt_core::capabilities::{RefEditorMirror, RefGlobalFocus, SutDriver};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvFocusMatchesRef;

impl InvFocusMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-focus-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvFocusMatchesRef
where
    R: RefGlobalFocus + RefEditorMirror,
    S: SutDriver,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    fn mode(&self) -> RunMode {
        RunMode::Strict
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // Skipped when no global focus is set in the reference model.
        let Some(ref_focused) = ref_.global_focused_block() else {
            return InvariantResult::Ok;
        };
        // Skipped while an editor is open — engine focus may not have
        // updated yet relative to the click handler.
        if ref_.active_editor_block().is_some() {
            return InvariantResult::Ok;
        }

        let resolved_ref = sut.resolve_ref_block_id(&ref_focused);

        // Poll up to 1 s: chord ops (SplitBlock, JoinBlock) fire
        // editor_focus(new_block) as a follow-up that propagates through
        // SQL → watch_editor_cursor → window.focus → InputEvent::Focus →
        // set_focus. The new block's EditorView may not have mounted yet.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
        loop {
            let actual = sut.engine_focused_block().await;
            match &actual {
                None => {
                    // No frontend engine installed — skip (SqlOnly mode).
                    return InvariantResult::Ok;
                }
                Some(actual_id) => {
                    if actual_id == &resolved_ref {
                        return InvariantResult::Ok;
                    }
                    if std::time::Instant::now() >= deadline {
                        return InvariantResult::Fail(format!(
                            "[inv-focus-matches-ref] Global focus mismatch: \
                             reference model has {ref_focused} (resolved: {resolved_ref}), \
                             but engine.focused_block() has {actual_id} (polled 1s)"
                        ));
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}
