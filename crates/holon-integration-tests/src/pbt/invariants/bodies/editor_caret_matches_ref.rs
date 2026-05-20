//! `inv-editor-caret-matches-ref`.
//!
//! Compares the SUT's tracked editor caret byte against the reference
//! model's `active_editor_cursor()`. The ref has tracked this since the
//! editor mirror landed but nothing ever checked it — multiple caret root
//! causes in history (compute_text_delta overflow, split caret re-seed,
//! MoveCursor byte/keystroke conflation) would have been one-step
//! diagnoses with this in place.
//!
//! Skipped when:
//! - No active editor in the reference model.
//! - The SUT/driver medium can't observe a caret (GPUI `InputState`,
//!   no driver installed) — disclosed via the capability's `Err`.
//! - The SUT tracks no caret for the block yet (no keystroke since
//!   focus; the headless mirror initializes lazily on first keystroke).
//!
//! Status: functional.

use holon_pbt_core::capabilities::{RefEditorMirror, SutEditorMirrorRead};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvEditorCaretMatchesRef;

impl InvEditorCaretMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-editor-caret-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvEditorCaretMatchesRef
where
    R: RefEditorMirror,
    S: SutEditorMirrorRead,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let Some(block_id) = ref_.active_editor_block() else {
            return InvariantResult::Skipped("no active editor in reference model".into());
        };
        let ref_cursor = ref_
            .active_editor_cursor()
            .expect("ref invariant: active_editor_block() implies active_editor_cursor()");

        match sut.editor_caret_byte(&block_id) {
            Err(reason) => InvariantResult::Skipped(format!("caret unobservable: {reason}")),
            Ok(None) => InvariantResult::Skipped(format!(
                "SUT tracks no caret for {block_id} yet (no keystroke since focus)"
            )),
            Ok(Some(sut_cursor)) if sut_cursor == ref_cursor => InvariantResult::Ok,
            Ok(Some(sut_cursor)) => InvariantResult::Fail(format!(
                "[inv-editor-caret-matches-ref] Caret mismatch on {block_id}: \
                 reference model cursor_byte={ref_cursor}, SUT tracked caret={sut_cursor} \
                 (ref editor text: {:?})",
                ref_.active_editor_text()
            )),
        }
    }
}
