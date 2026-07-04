//! Cross-cutting tests of the memory slice — the ones that assert over *several*
//! invariants at once (selection counts, multi-invariant positives) and the
//! sequence proptests that turn the slice from bug-*catching* into bug-*finding*.
//! Single-invariant catch/positive/deselection triads live with their invariant
//! in `super::invariants::*`.

use std::sync::Arc;

use holon::api::MemoryBackend;
use holon_api::repository::{CoreOperations, Lifecycle};
use holon_pbt_core::capabilities::{RefEditorMirrorMut, SutEditorMirrorWrite};
use holon_pbt_core::composition::CapProvider;
use proptest::prelude::*;

use crate::pbt::composed::fixtures::*;
use crate::pbt::composed::subsystem_seed::{
    assert_ref_seeded, run_with_seeded_ref, seed_ref, seed_ref_with_editor,
};
use crate::pbt::memory_slice::builders::{memory_wide, memory_wide_with_editor};
use crate::pbt::memory_slice::components::{InMemEditorComponent, MemoryBackendComponent};

/// The vertical proof: a composed `CapMap` slice selects the two `SutBackend`-
/// only structural invariants by capability presence and runs them over a real
/// `MemoryBackend` tree — no Turso, no `min_sut`, no E2ESut.
#[tokio::test]
async fn memory_slice_runs_structural_block_invariants_without_turso() {
    let backend = MemoryBackend::create_new("mem-slice".to_string())
        .await
        .expect("create_new memory backend");
    // A small valid tree: text child + source child under a virtual root, so
    // the invariants traverse real data rather than an empty store.
    let root = EntityUri::no_parent();
    let parent = backend
        .create_block(root, BlockContent::text("parent"), None)
        .await
        .expect("create parent");
    backend
        .create_block(parent.id.clone(), BlockContent::text("child"), None)
        .await
        .expect("create text child");
    backend
        .create_block(
            parent.id.clone(),
            BlockContent::source("rust", "fn x() {}"),
            None,
        )
        .await
        .expect("create source child");

    let sut = memory_wide(backend);
    // These invariants ignore the reference, so an empty ref CapMap suffices.
    let ref_ = CapMap::new();

    let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

    // The two `SutBackend`-only invariants run; the ref-comparing
    // `blocks-match-ref/block_raw` is *deselected* (no `RefBackend` wired),
    // disclosed rather than faked — the §2 negative containment in action.
    assert_eq!(
        report.ran_ids().len(),
        2,
        "only the SutBackend invariants are cap-selected; ran={:?} deselected={:?}",
        report.ran_ids(),
        report.deselected,
    );
    assert!(
        report
            .deselected
            .iter()
            .any(|id| id.0 == "inv-blocks-match-ref/block_raw"),
        "the ref-comparing invariant must be deselected when no RefBackend is wired; \
         deselected={:?}",
        report.deselected,
    );
    assert!(
        report.failures().is_empty(),
        "structural invariants must hold on a valid memory tree: {:?}",
        report.failures(),
    );
}

/// The Ref side composes (§5.1): wiring a `RefBackend` cap selects both the
/// `blocks-match-ref/block_raw` and `no-orphan` invariants, which pass when the
/// SUT's `block_raw` matches the reference's non-seed blocks field-for-field.
#[tokio::test]
async fn memory_slice_runs_ref_comparison_when_ref_is_wired() {
    let blocks = vec![
        Block::new_text(uri("local://r"), EntityUri::no_parent(), "root"),
        Block::new_text(uri("local://c"), uri("local://r"), "child"),
    ];
    let sut = fixture_slice(blocks.clone());
    let expected_ids: Vec<_> = blocks.iter().map(|b| b.id.clone()).collect();
    let ref_state = seed_ref(blocks);
    assert_ref_seeded(&ref_state, &expected_ids);

    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(ref_state),
    )
    .await;

    for id in ["inv-blocks-match-ref/block_raw", "inv-no-orphan-blocks"] {
        assert!(
            report.ran_ids().contains(&id),
            "wiring RefBackend must select {id}; ran={:?}",
            report.ran_ids(),
        );
    }
    assert!(
        report.failures().is_empty(),
        "block_raw matches the reference and the tree is well-formed, so all pass: {:?}",
        report.failures(),
    );
}

/// Negative containment (§5.2 / §2): the editor invariants are *deselected* —
/// disclosed, not faked — when no editor component is wired. A backend-only SUT
/// must not silently "pass" the editor checks.
#[tokio::test]
async fn memory_slice_editor_invariants_deselected_without_editor() {
    let blocks = vec![Block::new_text(
        uri("local://r"),
        EntityUri::no_parent(),
        "root",
    )];
    let sut = fixture_slice(blocks.clone());
    let expected_ids: Vec<_> = blocks.iter().map(|b| b.id.clone()).collect();
    let ref_state = seed_ref(blocks);
    assert_ref_seeded(&ref_state, &expected_ids);

    // The ref now carries `RefEditorMirror` (registered unconditionally), but
    // selection is an AND over SUT and ref caps — the backend-only SUT has no
    // editor read cap, so the editor invariants still deselect.
    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(ref_state),
    )
    .await;

    for id in [
        "inv-editor-text/mirror",
        "inv-editor-caret/mirror",
    ] {
        assert!(
            report.deselected.iter().any(|d| d.0 == id),
            "{id} must be deselected without an editor component; ran={:?} deselected={:?}",
            report.ran_ids(),
            report.deselected,
        );
    }
}

/// The §6 payoff: a *two-component* SUT (`MemoryBackend` + `InMemEditor`).
/// Wiring both the editor cap and a ref editor selects the editor invariants,
/// which pass when the production-parity SUT editor agrees with the independent
/// `Vec<char>` reference after an open + multi-byte type.
#[tokio::test]
async fn memory_slice_editor_text_and_caret_match_when_wired() {
    let block = uri("local://e");
    let backend = MemoryBackend::create_new("editor-slice".to_string())
        .await
        .expect("create_new memory backend");
    let (sut, editor) = memory_wide_with_editor(backend);

    // SUT editor: open at end-of-text, then type (incl. a multi-byte char).
    editor.open(block.clone(), "héllo".to_string());
    editor.type_chars(" wörld");

    // The real `ReferenceState` oracle driven through the same intent — both
    // sides share the `editor_caret` primitive (R2), so this guards the
    // editor↔mirror integration path, not the text primitive itself.
    let mut ref_state = seed_ref_with_editor(Vec::new(), block.clone(), "héllo");
    ref_state.type_chars(" wörld");

    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(ref_state),
    )
    .await;

    for id in [
        "inv-editor-text/mirror",
        "inv-editor-caret/mirror",
    ] {
        assert!(
            report.ran_ids().contains(&id),
            "wiring the editor + ref editor must select {id}; ran={:?}",
            report.ran_ids(),
        );
    }
    assert!(
        report.failures().is_empty(),
        "SUT editor agrees with the independent reference: {:?}",
        report.failures(),
    );
}

// ─── Editor op-sequence: a differential PBT across two UTF-8 editors ──────

#[derive(Clone, Debug)]
enum EditorOp {
    /// Type a short string (drawn from an alphabet with 1-, 2- and 4-byte
    /// codepoints, to stress byte-vs-codepoint handling).
    Type(String),
    DeleteBackward(usize),
    /// Move the caret to a byte offset (clamped to a boundary by both sides).
    MoveCursor(usize),
}

fn editor_op_strategy() -> impl Strategy<Value = EditorOp> {
    let glyph = proptest::sample::select(vec!["a", "b", " ", "é", "😀"]);
    prop_oneof![
        proptest::collection::vec(glyph, 1..3).prop_map(|v| EditorOp::Type(v.concat())),
        (0usize..3).prop_map(EditorOp::DeleteBackward),
        (0usize..16).prop_map(EditorOp::MoveCursor),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Drive a random editor op sequence against the production-parity SUT
    /// editor (`InMemEditorComponent`) and the real `ReferenceState` oracle in
    /// lockstep, checking the editor invariants after every op. With both sides
    /// on the shared `editor_caret` primitive (R2), this exercises the
    /// editor↔mirror integration path on multibyte op sequences — text-primitive
    /// correctness is owned by the `editor_caret` unit-PBT in `holon-frontend`.
    ///
    /// `ReferenceState` owns a `tokio::runtime::Runtime`; it is created and
    /// dropped in this **sync** proptest scope (not inside `block_on`), and each
    /// tick's catalog run goes through [`run_with_seeded_ref`] (which clones it
    /// and drops the clone off the executor) so no runtime ever drops on an
    /// async executor.
    #[test]
    fn memory_slice_editor_op_sequence_sut_matches_independent_ref(
        ops in proptest::collection::vec(editor_op_strategy(), 1..20),
    ) {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let block = uri("local://eop");
        let (sut, editor) = rt.block_on(async {
            let backend = MemoryBackend::create_new("editor-op-shell".to_string())
                .await
                .expect("create_new");
            memory_wide_with_editor(backend)
        });
        editor.open(block.clone(), "séed".to_string());
        let mut ref_state = seed_ref_with_editor(Vec::new(), block.clone(), "séed");
        let registry = composed_invariant_catalog();

        for (tick, op) in ops.iter().enumerate() {
            match op {
                EditorOp::Type(s) => {
                    editor.type_chars(s);
                    ref_state.type_chars(s);
                }
                EditorOp::DeleteBackward(n) => {
                    editor.delete_backward(*n);
                    ref_state.delete_backward(*n);
                }
                EditorOp::MoveCursor(b) => {
                    editor.move_cursor(*b);
                    ref_state.move_cursor(*b);
                }
            }
            let report =
                rt.block_on(run_with_seeded_ref(&registry, &sut, crate::pbt::reference_state::Resolved::identity(ref_state.clone())));

            let failures = report.failures();
            prop_assert!(
                failures.is_empty(),
                "tick {tick} after {op:?}: SUT editor diverged from the reference oracle; \
                 failures={failures:?}",
            );
        }
    }
}

// ─── E0: editor commit round-trip (editor text → real MemoryBackend) ──────
//
// The op-sequence proptest above checks the editor MIRROR against the ref. This
// closes the loop to STORAGE: `take_commit()` writes the live text through the
// production `MemoryBackend::update_block`, then `blocks-match-ref/block_raw`
// re-reads it from the store and cross-checks against the independent reference.
// A bug in the editor's text math OR in the commit path surfaces as a committed-
// content divergence in an already-wired invariant — no new invariant needed.

/// Apply-phase glue: commit the editor's pending text into the backing store,
/// exactly as a structural commit point would in production.
async fn commit_editor(editor: &InMemEditorComponent, backend: &MemoryBackend) {
    let (id, text) = editor
        .take_commit()
        .expect("commit_editor called with no active editor");
    backend
        .update_block(id.as_str(), BlockContent::text(text))
        .await
        .expect("commit update_block");
}

/// E0 positive: open an editor on a real block, type, commit, and confirm the
/// committed text reached the `MemoryBackend` — `blocks-match-ref/block_raw`
/// (re-reading the store) agrees with the reference's expected content.
#[tokio::test]
async fn memory_slice_editor_commit_flows_to_backend() {
    let backend = MemoryBackend::create_new("editor-commit".to_string())
        .await
        .expect("create_new");
    let blk = backend
        .create_block(EntityUri::no_parent(), BlockContent::text("initial"), None)
        .await
        .expect("create block");

    let editor = InMemEditorComponent::new(Arc::new(backend.clone()) as Arc<dyn CoreOperations>);
    editor.open(blk.id.clone(), "initial".to_string());
    editor.type_chars(" edited");
    commit_editor(&editor, &backend).await;

    let sut = memory_wide(backend.clone());
    let ref_block = Block::new_text(blk.id, EntityUri::no_parent(), "initial edited");
    let expected_ids = vec![ref_block.id.clone()];
    let ref_state = seed_ref(vec![ref_block]);
    assert_ref_seeded(&ref_state, &expected_ids);
    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(ref_state),
    )
    .await;

    assert!(
        report.ran_ids().contains(&"inv-blocks-match-ref/block_raw"),
        "blocks-match must be selected; ran={:?}",
        report.ran_ids(),
    );
    assert!(
        report.failures().is_empty(),
        "committed editor text must match the reference in the store: {:?}",
        report.failures(),
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// E0 bug-finding: type/delete a random sequence, commit to the real
    /// `MemoryBackend` through the production [`InProcEditorSut`] write cap after
    /// every op, and require `blocks-match-ref/block_raw` to confirm the stored
    /// content equals the `ReferenceState` oracle's committed content. Exercises
    /// the full editor-math → `take_commit` → normalize → `update_block` →
    /// `block_raw` read-back path. Both sides normalize via
    /// `normalize_content_for_org_roundtrip` (`InProcEditorSut::commit` and
    /// `ReferenceState::commit_active_editor_if_changed`), so committed content
    /// matches byte-for-byte. (Caret isn't stored, so this validates text; the
    /// mirror op-sequence test covers the caret.)
    #[test]
    fn memory_slice_editor_commit_roundtrip_matches_ref(
        ops in proptest::collection::vec(editor_op_strategy(), 1..20),
    ) {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let (caps, blk_id) = rt.block_on(async {
            let backend = Arc::new(
                MemoryBackend::create_new("editor-commit-rt".to_string())
                    .await
                    .expect("create_new"),
            );
            let blk = backend
                .create_block(EntityUri::no_parent(), BlockContent::text("séed"), None)
                .await
                .expect("create block");
            // The editor IS the write target now (Stage-1b collapse): it commits into
            // the SAME shared store the `SutBackend` cap reads.
            let editor = Arc::new(InMemEditorComponent::new(
                backend.clone() as Arc<dyn CoreOperations>
            ));
            editor.open(blk.id.clone(), "séed".to_string());
            let mut caps = CapMap::new();
            Arc::new(MemoryBackendComponent::new_shared(backend.clone())).register(&mut caps);
            editor.register(&mut caps);
            (caps, blk.id)
        });
        let mut ref_state = seed_ref_with_editor(
            vec![Block::new_text(blk_id.clone(), EntityUri::no_parent(), "séed")],
            blk_id,
            "séed",
        );
        let registry = composed_invariant_catalog();

        for (tick, op) in ops.iter().enumerate() {
            // Drive the editor write THROUGH the composed `CapMap` (the
            // `#[capmap_adapter]` forward to the hosted `SutEditorMirrorWrite`) — i.e.
            // the composed map IS an editor `SutTransitionTarget`, the E1 payoff.
            rt.block_on(async {
                match op {
                    EditorOp::Type(s) => caps.apply_type_chars(s).await,
                    EditorOp::DeleteBackward(n) => caps.apply_delete_backward(*n).await,
                    EditorOp::MoveCursor(b) => caps.apply_move_cursor(*b).await,
                }
            });
            match op {
                EditorOp::Type(s) => {
                    ref_state.type_chars(s);
                    ref_state.commit_active_editor_if_changed();
                }
                EditorOp::DeleteBackward(n) => {
                    ref_state.delete_backward(*n);
                    ref_state.commit_active_editor_if_changed();
                }
                // No commit — matches `apply_move_cursor`, which doesn't write.
                EditorOp::MoveCursor(b) => ref_state.move_cursor(*b),
            }
            let report =
                rt.block_on(run_with_seeded_ref(&registry, &caps, crate::pbt::reference_state::Resolved::identity(ref_state.clone())));

            let failures = report.failures();
            prop_assert!(
                failures.is_empty(),
                "tick {tick} after {op:?}: committed content diverged from the \
                 reference in the store; failures={failures:?}",
            );
        }
    }
}
