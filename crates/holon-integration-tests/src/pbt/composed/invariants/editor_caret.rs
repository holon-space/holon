//! `inv-editor-caret-matches-ref` wired into the memory slice — the second-
//! component editor invariant: `SutEditorMirrorRead` + `RefEditorMirror`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefEditorMirror, SutEditorMirrorRead};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::editor_caret_matches_ref::InvEditorCaretMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvEditorCaretMatchesRef,
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

    /// Catch (doc §6 gate): a SUT editor whose tracked byte caret is off by one
    /// relative to the reference (the `MoveCursor` byte/keystroke-conflation bug
    /// class). Text agrees, so only `inv-editor-caret-matches-ref` fires.
    #[tokio::test]
    async fn memory_slice_editor_catches_caret_divergence() {
        let block = uri("local://e");
        let sut = buggy_editor_map(BuggyEditor {
            block: block.clone(),
            text: "hello".to_string(),
            caret: 4,
        });
        // The real oracle opens the editor at end-of-text (caret = len = 5); the
        // buggy SUT reports caret 4. Text agrees on "hello".
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
                .any(|(id, _)| *id == "inv-editor-caret-matches-ref"),
            "the caret divergence must be caught; failures={failures:?}",
        );
        assert!(
            !failures
                .iter()
                .any(|(id, _)| *id == "inv-editor-text-matches-ref"),
            "text agrees, so only the caret invariant fires; failures={failures:?}",
        );
    }
}
