//! `inv-editor-text-matches-ref`.
//!
//! Headless active-block live-text check: the SUT's `MutableText` value for
//! the actively-edited block (the cell headless keystrokes mutate, read via
//! `SutEditorMirrorRead::editor_live_text`) must equal the reference's
//! `active_editor_text()`. Closes the gap left by
//! `inv-displayed-text/viewmodel` deliberately skipping the active block:
//! pre-blur, the live editor text is checked nowhere headless (the `/widget`
//! arm covering it is geometry-gated, GPUI only).
//!
//! Skipped when:
//! - No active editor in the reference model.
//! - The live text is unobservable (no frontend engine in SqlOnly headless,
//!   or no `MutableText` resolvable for the block yet) — disclosed via the
//!   capability's `Err`.
//!
//! Status: functional.

use holon_pbt_core::capabilities::{RefEditorMirror, SutEditorMirrorRead};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvEditorTextMatchesRef;

impl InvEditorTextMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-editor-text-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvEditorTextMatchesRef
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
        let ref_text = ref_
            .active_editor_text()
            .expect("ref invariant: active_editor_block() implies active_editor_text()");

        match sut.editor_live_text(&block_id) {
            Err(reason) => InvariantResult::Skipped(format!("live text unobservable: {reason}")),
            Ok(sut_text) if sut_text == ref_text => InvariantResult::Ok,
            Ok(sut_text) => InvariantResult::Fail(format!(
                "[inv-editor-text-matches-ref] Live editor text mismatch on {block_id}:\n  \
                 reference: {ref_text:?}\n  SUT MutableText: {sut_text:?}"
            )),
        }
    }
}
