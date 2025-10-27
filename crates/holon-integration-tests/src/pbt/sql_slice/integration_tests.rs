//! Cross-cutting tests of the SQL slice. The headline is the §6 payoff again:
//! the **same** shared catalog the memory and Loro slices run now validates a
//! real Turso `BackendEngine` (the production storage + IVM matview layer) —
//! every `SutBackend` block-tree invariant lights up over the SQL realization by
//! capability presence, no catalog change, and the `SutSqlProjection`-bound
//! invariants (e.g. `inv-block-content/sql`) select on top. The
//! per-invariant catch triads live with their invariant in
//! `super::super::composed::invariants`, fixture-driven.

use crate::pbt::composed::fixtures::*;
use crate::pbt::composed::subsystem_seed::{assert_ref_seeded, run_with_seeded_ref, seed_ref};
use crate::pbt::sql_slice::builders::{new_sql_engine, sql_wide};
use crate::pbt::sql_slice::components::SqlProjectionComponent;

/// The §6 payoff, SQL edition: a composed `CapMap` over a real Turso
/// `BackendEngine` selects the `SutBackend`-only structural invariants by
/// capability presence and runs them over the SQL realization — no reactive
/// engine, no `min_sut`, no `E2ESut`.
#[tokio::test(flavor = "multi_thread")]
async fn sql_slice_runs_structural_block_invariants_over_turso() {
    let engine = new_sql_engine().await;
    let driver = SqlProjectionComponent::new(engine.clone());
    let root = uri("block:sql-r");
    let text_child = uri("block:sql-t");
    let source_child = uri("block:sql-s");
    driver
        .create_block(&root, &EntityUri::no_parent(), "parent")
        .await;
    driver.create_block(&text_child, &root, "child").await;
    // A source block with a language, to exercise inv-source-language-iff-source.
    driver
        .create_source_block(&source_child, &root, "rust", "fn x() {}")
        .await;

    let sut = sql_wide(engine);
    let ref_ = CapMap::new();

    let report = run_selected(&composed_invariant_catalog(), &sut, &ref_).await;

    // With no reference wired, only the three `SutBackend`-only structural
    // invariants run (`no-orphan-blocks` became ref-free when its CDC-lag gate
    // was removed); everything ref-comparing (and the Loro invariants) is
    // deselected — disclosed, not faked.
    let mut ran = report.ran_ids();
    ran.sort_unstable();
    assert_eq!(
        ran,
        [
            "inv-no-orphan-blocks",
            "inv-no-parent-cycles",
            "inv-source-language-iff-source",
        ],
        "exactly the no-ref SutBackend invariants are cap-selected over Turso; deselected={:?}",
        report.deselected,
    );
    assert!(
        report.failures().is_empty(),
        "structural invariants must hold on a valid Turso store: {:?}",
        report.failures(),
    );
}

/// Wiring a reference selects the ref-comparing block-tree invariants **and**
/// the `SutSqlProjection`-backed `inv-block-content/sql` (the SQL
/// variant), all of which pass when the Turso store agrees with the reference.
#[tokio::test(flavor = "multi_thread")]
async fn sql_slice_runs_ref_comparison_over_turso() {
    let engine = new_sql_engine().await;
    let driver = SqlProjectionComponent::new(engine.clone());
    let root = uri("block:sql-root");
    let child = uri("block:sql-child");
    driver
        .create_block(&root, &EntityUri::no_parent(), "root")
        .await;
    driver.create_block(&child, &root, "child").await;

    let blocks = vec![
        Block::new_text(root.clone(), EntityUri::no_parent(), "root"),
        Block::new_text(child.clone(), root.clone(), "child"),
    ];
    let sut = sql_wide(engine);
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
        "inv-block-content/block_raw",
        "inv-block-parent/block_raw",
        // The SQL-projection variant — selected only because this slice
        // provides `SutSqlProjection`.
        "inv-block-content/sql",
    ] {
        assert!(
            report.ran_ids().contains(&id),
            "wiring the reference must select {id} over Turso; ran={:?}",
            report.ran_ids(),
        );
    }
    assert!(
        report.failures().is_empty(),
        "the Turso store matches the reference, so all selected invariants pass: {:?}",
        report.failures(),
    );
}
