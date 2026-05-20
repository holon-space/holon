//! `inv-focus-matches-ref`.
//!
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

use holon_pbt_core::capabilities::{EngineFocus, RefEditorMirror, RefGlobalFocus, SutDriver};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use crate::pbt::retry::retry_until_ok;

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

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let Some(ref_focused) = ref_.global_focused_block() else {
            return InvariantResult::Skipped("no global focus in reference model".into());
        };
        // Engine focus may not have updated yet relative to the click handler.
        if ref_.active_editor_block().is_some() {
            return InvariantResult::Skipped(
                "editor open — editor focus is source of truth".into(),
            );
        }

        let resolved_ref = sut.resolve_ref_block_id(&ref_focused);

        // No frontend engine (SqlOnly headless): focus is unobservable —
        // a visible Skip, not a silent Ok. Checked once up front; engine
        // presence cannot change mid-poll.
        if sut.engine_focused_block().await == EngineFocus::NoEngine {
            return InvariantResult::Skipped("no frontend engine (SqlOnly)".into());
        }

        // Poll up to 1 s: chord ops (SplitBlock, JoinBlock) move focus to the
        // new block via their op response, applied in-process (ADR 0010);
        // window focus then follows the `focused_block` signal. The new
        // block's EditorView may not have mounted yet. `Unfocused` keeps
        // polling and FAILS on timeout — the ref expects focus here, so a
        // focus-less engine is the steal-back/lost-focus bug, not a skip.
        let result = retry_until_ok(
            std::time::Duration::from_millis(1000),
            std::time::Duration::from_millis(20),
            async || match sut.engine_focused_block().await {
                EngineFocus::NoEngine => {
                    unreachable!("engine presence cannot change mid-poll")
                }
                EngineFocus::Focused(actual_id) if actual_id == resolved_ref => Ok(()),
                EngineFocus::Focused(actual_id) => Err(Some(actual_id)),
                EngineFocus::Unfocused => Err(None),
            },
        )
        .await;
        match result {
            Ok(()) => InvariantResult::Ok,
            Err(Some(actual_id)) => InvariantResult::Fail(format!(
                "[inv-focus-matches-ref] Global focus mismatch: \
                 reference model has {ref_focused} (resolved: {resolved_ref}), \
                 but engine.focused_block() has {actual_id} (polled 1s)"
            )),
            Err(None) => InvariantResult::Fail(format!(
                "[inv-focus-matches-ref] Global focus LOST: \
                 reference model has {ref_focused} (resolved: {resolved_ref}), \
                 but engine.focused_block() is None (polled 1s)"
            )),
        }
    }
}
