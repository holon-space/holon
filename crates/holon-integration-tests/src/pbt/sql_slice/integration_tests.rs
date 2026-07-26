//! Cross-cutting tests of the SQL slice. The headline is the §6 payoff again:
//! the **same** shared catalog the memory and Loro slices run now validates a
//! real Turso `BackendEngine` (the production storage + IVM matview layer) —
//! every `SutBackend` block-tree invariant lights up over the SQL realization
//! by capability presence, no catalog change, and the `SutSqlProjection`-bound
//! invariants (e.g. `inv-block-content/sql`) select on top. The
//! per-invariant catch triads live with their invariant in
//! `super::super::composed::invariants`, fixture-driven.

use crate::pbt::composed::fixtures::*;
use crate::pbt::composed::subsystem_seed::assert_ref_seeded;
use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
use crate::pbt::composed::subsystem_seed::seed_ref;
use crate::pbt::sql_slice::builders::new_sql_engine;
use crate::pbt::sql_slice::builders::new_sql_engine_with_structural_ops;
use crate::pbt::sql_slice::builders::sql_wide;
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
            "inv-mark-bounds-within-content",
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

/// `split_block` must PARTITION the origin block's inline marks across the
/// split point — the dogfood 2026-07-20 headline data-loss. Before the fix,
/// split wrote only `content`: the retained block kept STALE out-of-bounds
/// marks and the split-off block got NULL marks, so a `[[link]]` crossing the
/// cut vanished on both sides (and the stale left span was the exact
/// `scalar_range_to_bytes` crash condition).
///
/// This drives the PRODUCTION SqlOnly split path end-to-end over a real Turso
/// engine — `SqlBlockOperations::split_block` (the fixed default impl) over the
/// CRUD `SqlOperationProvider` — and asserts LINK PRESERVATION, so it stands on
/// its own regardless of any global mark-bounds invariant. Cases:
///  1. link entirely left of the split → stays on the retained block, in
///     bounds;
///  2. link straddling the split → degrades to plain text on both sides;
///  3. link entirely right of the split → moves to the new block, rebased.
#[tokio::test(flavor = "multi_thread")]
async fn split_block_partitions_link_marks_over_turso() {
    use holon_api::EntityRef;
    use holon_api::InlineMark;
    use holon_api::MarkSpan;
    use holon_api::OpOrigin;
    use holon_api::StorageEntity;
    use holon_api::Value;
    use holon_api::marks_from_json;
    use holon_api::marks_to_json;

    let link = |name: &str| InlineMark::Link {
        target: EntityRef::Name {
            name: name.to_string(),
        },
        label: name.to_string(),
    };

    // Read back a block's (content, marks) from the `block_raw` base table.
    async fn read_block(
        engine: &holon::api::backend_engine::BackendEngine,
        id: &str,
    ) -> (String, Vec<MarkSpan>) {
        let rows = engine
            .db_handle()
            .query(
                &format!(
                    "SELECT content, marks FROM block_raw WHERE id = '{}'",
                    id.replace('\'', "''")
                ),
                std::collections::HashMap::new(),
            )
            .await
            .expect("query block_raw");
        let row = rows
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("block {id} not found in block_raw"));
        let content = row
            .get("content")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();
        // `marks` is a `#[jsonb]` column: CDC delivers it as `Value::Json`
        // (or `Value::String` when empty/absent), never a plain string.
        let marks = match row.get("marks") {
            Some(Value::Json(s)) | Some(Value::String(s)) if !s.is_empty() => {
                marks_from_json(s).expect("marks JSON parses")
            }
            _ => Vec::new(),
        };
        (content, marks)
    }

    // Dispatch one production `block` operation over the engine.
    async fn op(
        engine: &holon::api::backend_engine::BackendEngine,
        name: &str,
        params: holon_api::StorageEntity,
    ) {
        let entity = holon_api::EntityName::new("block");
        engine
            .execute_operation(&entity, name, params, OpOrigin::User)
            .await
            .unwrap_or_else(|e| panic!("block/{name} failed: {e}"));
    }

    // One full create → split → read-back cycle. `content` is the stripped
    // label; `mark` covers `[mark_start, mark_end)`; the block is split at byte
    // `split_pos`. Returns the resulting (origin, new-block) (content, marks).
    async fn split_case(
        content: &str,
        mark: MarkSpan,
        split_pos: i64,
    ) -> ((String, Vec<MarkSpan>), (String, Vec<MarkSpan>)) {
        let engine = new_sql_engine_with_structural_ops().await;
        let root = "block:mk-root";
        let child = "block:mk-child";

        // Parent (a plain block, not a Page — split refuses Pages).
        let mut rp: StorageEntity = StorageEntity::new();
        rp.insert("id".into(), Value::String(root.into()));
        rp.insert("parent_id".into(), Value::Null);
        rp.insert("content".into(), Value::String("Root".into()));
        rp.insert("content_type".into(), holon_api::ContentType::Text.into());
        op(&engine, "create", rp).await;

        // Child carrying the link mark.
        let mut cp: StorageEntity = StorageEntity::new();
        cp.insert("id".into(), Value::String(child.into()));
        cp.insert("parent_id".into(), Value::String(root.into()));
        cp.insert("content".into(), Value::String(content.into()));
        cp.insert("content_type".into(), holon_api::ContentType::Text.into());
        cp.insert(
            "marks".into(),
            Value::String(marks_to_json(std::slice::from_ref(&mark))),
        );
        op(&engine, "create", cp).await;

        // Split.
        let mut sp: StorageEntity = StorageEntity::new();
        sp.insert("id".into(), Value::String(child.into()));
        sp.insert("position".into(), Value::Integer(split_pos));
        op(&engine, "split_block", sp).await;

        // The new block is the only child of root that is not the origin.
        let sibling_rows = engine
            .db_handle()
            .query(
                "SELECT id FROM block_raw WHERE parent_id = 'block:mk-root' AND id != \
                 'block:mk-child'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("query siblings");
        let new_id = sibling_rows
            .into_iter()
            .next()
            .and_then(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
            .expect("split created a new sibling block");

        let origin = read_block(&engine, child).await;
        let new_block = read_block(&engine, &new_id).await;
        (origin, new_block)
    }

    // Case 1 — link entirely LEFT of the split (dogfood repro shape).
    // "Owner is Ada Lovelace and reviewer", link over "Ada Lovelace" = [9,21);
    // split after "and" (byte 25). Link stays on the retained block in bounds;
    // the split-off "reviewer" carries no marks.
    let (origin, new_block) = split_case(
        "Owner is Ada Lovelace and reviewer",
        MarkSpan::new(9, 21, link("Ada Lovelace")),
        25,
    )
    .await;
    assert_eq!(origin.0, "Owner is Ada Lovelace and");
    assert_eq!(
        origin.1,
        vec![MarkSpan::new(9, 21, link("Ada Lovelace"))],
        "the link must survive on the retained block, in bounds"
    );
    assert_eq!(new_block.0, "reviewer");
    assert!(
        new_block.1.is_empty(),
        "the split-off block has no link, got {:?}",
        new_block.1
    );

    // Case 2 — link entirely RIGHT of the split. "see Ada Lovelace", link
    // [4,16); split after "see " (byte 4). Buggy code left the origin span
    // [4,16) dangling out of bounds over "see" AND gave the new block NULL
    // marks, LOSING the link. Fixed: origin bare, link moves right rebased.
    let (origin, new_block) = split_case(
        "see Ada Lovelace",
        MarkSpan::new(4, 16, link("Ada Lovelace")),
        4,
    )
    .await;
    assert_eq!(origin.0, "see");
    assert!(
        origin.1.is_empty(),
        "retained block must not keep a dangling out-of-bounds mark, got {:?}",
        origin.1
    );
    assert_eq!(new_block.0, "Ada Lovelace");
    assert_eq!(
        new_block.1,
        vec![MarkSpan::new(0, 12, link("Ada Lovelace"))],
        "the link must move to the new block, rebased to [0,12)"
    );

    // Case 3 — split INSIDE the link → degrade to plain text on BOTH sides.
    // "Ada Lovelace", link [0,12); split after "Ada L" (byte 5).
    let (origin, new_block) = split_case(
        "Ada Lovelace",
        MarkSpan::new(0, 12, link("Ada Lovelace")),
        5,
    )
    .await;
    assert_eq!(origin.0, "Ada L");
    assert!(
        origin.1.is_empty(),
        "a straddled link degrades to plain text on the left, got {:?}",
        origin.1
    );
    assert_eq!(new_block.0, "ovelace");
    assert!(
        new_block.1.is_empty(),
        "a straddled link degrades to plain text on the right, got {:?}",
        new_block.1
    );
}
