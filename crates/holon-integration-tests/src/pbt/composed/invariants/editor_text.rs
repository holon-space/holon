//! `inv-editor-text-matches-ref` wired into the memory slice — the second-
//! component editor invariant: `SutEditorMirrorRead` + `RefEditorMirror`.
//! `Strict`, but `Skip`s (not `Fail`s) when the ref has no active editor or the
//! SUT can't observe the live text — the body's own disclosed-skip contract.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefEditorMirror, SutEditorMirrorRead};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::editor_text_matches_ref::InvEditorTextMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvEditorTextMatchesRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutEditorMirrorRead>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefEditorMirror>()],
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::{run_with_seeded_ref, seed_ref_with_editor};

    /// Catch (doc §6 gate): a SUT editor whose `MutableText` lost a character
    /// relative to the reference. Reads the reference through the borrow-
    /// returning `RefEditorMirror::active_editor_text` via `CapMap::expect_ref`.
    #[tokio::test]
    async fn memory_slice_editor_catches_live_text_divergence() {
        let block = uri("local://e");
        let sut = buggy_editor_map(BuggyEditor {
            block: block.clone(),
            text: "helo".to_string(),
            caret: 4,
        });
        // The real oracle holds "hello" (caret = len = 5); the buggy SUT dropped
        // a char to "helo".
        let ref_state = seed_ref_with_editor(Vec::new(), block, "hello");

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-editor-text-matches-ref"),
            "the live-text divergence must be caught; failures={failures:?}",
        );
    }
}
