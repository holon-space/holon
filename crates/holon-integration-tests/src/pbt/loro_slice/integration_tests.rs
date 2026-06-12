//! Cross-cutting tests of the Loro slice. The headline is the §6 payoff: the
//! **same** shared catalog the memory slice runs now validates a real
//! `LoroBackend` CRDT — every `SutBackend` block-tree invariant lights up over
//! Loro by capability presence, no catalog change. The per-invariant catch
//! triads (incl. the Loro-specific ones) live with their invariant in
//! `super::super::composed::invariants`, fixture-driven.

use holon::api::LoroBackend;
use holon_api::repository::{CoreOperations, Lifecycle};

use crate::pbt::composed::fixtures::*;
use crate::pbt::composed::subsystem_seed::{assert_ref_seeded, run_with_seeded_ref, seed_ref};
use crate::pbt::loro_slice::builders::loro_wide;

async fn new_loro(doc: &str) -> LoroBackend {
    LoroBackend::create_new(doc.to_string())
        .await
        .expect("create_new LoroBackend")
}

/// The §6 payoff, Loro edition: a composed `CapMap` over a real `LoroBackend`
/// selects the `SutBackend`-only structural invariants by capability presence
/// and runs them over the live CRDT tree — no Turso, no `min_sut`, no E2ESut.
#[tokio::test]
async fn loro_slice_runs_structural_block_invariants_over_loro() {
    let backend = new_loro("loro-slice").await;
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

    let sut = loro_wide(backend);
    let ref_ = CapMap::new();

    let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

    // With no reference wired, the no-ref invariants run: the two `SutBackend`-
    // only structural ones **plus** `inv-loro-no-errors` (the Loro component
    // also provides `SutLoroLog`). Ref-comparing invariants are deselected.
    let mut ran = report.ran_ids();
    ran.sort_unstable();
    assert_eq!(
        ran,
        [
            "inv-loro-no-errors",
            "inv-no-parent-cycles",
            "inv-source-language-iff-source",
        ],
        "exactly the no-ref invariants are cap-selected over Loro; deselected={:?}",
        report.deselected,
    );
    assert!(
        report.failures().is_empty(),
        "structural invariants must hold on a valid Loro tree: {:?}",
        report.failures(),
    );
}

/// Wiring a `RefBackend`/`RefBlockTree` reference selects the ref-comparing
/// block-tree invariants, which pass when the Loro store agrees with the
/// reference field-for-field — the convergence that proves the catalog is
/// storage-agnostic.
#[tokio::test]
async fn loro_slice_runs_ref_comparison_over_loro() {
    let backend = new_loro("loro-ref-slice").await;
    // Seed a two-level tree, capturing the *returned* ids so the reference is
    // built from exactly the ids Loro will report back (content stays intent).
    let root = backend
        .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
        .await
        .expect("create root");
    let child = backend
        .create_block(root.id.clone(), BlockContent::text("child"), None)
        .await
        .expect("create child");

    let blocks = vec![
        Block::new_text(root.id.clone(), EntityUri::no_parent(), "root"),
        Block::new_text(child.id.clone(), root.id.clone(), "child"),
    ];
    let sut = loro_wide(backend);
    let expected_ids: Vec<_> = blocks.iter().map(|b| b.id.clone()).collect();
    let ref_state = seed_ref(blocks);
    assert_ref_seeded(&ref_state, &expected_ids);

    let report = run_with_seeded_ref(
        &composed_invariant_catalog(),
        &sut,
        crate::pbt::reference_state::Resolved::identity(ref_state),
    )
    .await;

    for id in [
        "inv-blocks-match-ref/block_raw",
        "inv-no-orphan-blocks",
        "inv-block-content-matches-ref/block_raw",
        "inv-block-parent-matches-ref/block_raw",
    ] {
        assert!(
            report.ran_ids().contains(&id),
            "wiring the reference must select {id} over Loro; ran={:?}",
            report.ran_ids(),
        );
    }
    assert!(
        report.failures().is_empty(),
        "the Loro store matches the reference, so all selected invariants pass: {:?}",
        report.failures(),
    );
}
