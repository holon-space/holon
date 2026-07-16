//! C2 INC 2: the `automations_journal` matview fires CDC when a rule-origin
//! history row lands, and IVM maintains the grouped counts O(delta).
//!
//! `automations_journal` groups `block_history` by `(origin, transition_id,
//! day)` and counts (ADR 0024 P8). This test proves the reactivity promise (F1a
//! — "the Watcher notices the 7th postponement immediately"): subscribe to the
//! matview's CDC stream, append a rule-origin history row, and assert a
//! `Change` arrives keyed on `automations_journal`. The O(delta) assertion
//! follows the `derived_field_matview.rs` precedent — a second row in the same
//! group emits ONE update batch (not a re-emission of every group), and a row
//! in a new group emits a single new Created.
//!
//! History rows are appended with direct SQL matching the shape
//! `TursoHistoryStore::record` writes (origin `rule`, non-null `op_group`); the
//! engine dispatch → record_history → block_history chain is covered by the
//! keystone correspondence oracle.

use std::collections::HashMap;
use std::time::Duration;

use holon_api::BatchWithMetadata;
use holon_api::streaming::Change;
use holon_core::storage::StorageEntity;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::HistorySchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::RowChange;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast::Receiver;

const JOURNAL: &str = "automations_journal";

async fn setup() -> (DbHandle, Receiver<BatchWithMetadata<RowChange>>) {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend); // keep the actor alive for the test
    HistorySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block_history schema");
    reconcile_named_view(
        &handle,
        JOURNAL,
        include_str!("../sql/schema/automations_journal_matview.sql"),
    )
    .await
    .expect("automations_journal matview DDL must succeed (no hang)");

    // Subscribe AFTER the matview exists but BEFORE any writes — broadcast only
    // delivers to subscribers present at send time.
    let cdc_rx = handle.subscribe_row_changes();
    (handle, cdc_rx)
}

/// Append one history row (the shape `TursoHistoryStore::record` writes).
async fn record(handle: &DbHandle, seq: i64, origin: &str, transition: &str, day: &str) {
    handle
        .execute(
            "INSERT INTO block_history (seq, entity_name, block_id, op_name, origin, \
             transition_id, at_millis, day, op_group) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                turso::Value::Integer(seq),
                turso::Value::Text("block".into()),
                turso::Value::Text(format!("blk-{seq}")),
                turso::Value::Text("set_field".into()),
                turso::Value::Text(origin.into()),
                turso::Value::Text(transition.into()),
                turso::Value::Integer(seq * 1000),
                turso::Value::Text(day.into()),
                turso::Value::Integer(seq),
            ],
        )
        .await
        .expect("append history row");
}

/// Drain all pending CDC changes for the `automations_journal` relation,
/// waiting briefly for the matview maintenance task to publish.
async fn drain_journal_changes(
    cdc_rx: &mut Receiver<BatchWithMetadata<RowChange>>,
) -> Vec<Change<StorageEntity>> {
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut out = Vec::new();
    while let Ok(batch) = cdc_rx.try_recv() {
        for change in batch.inner.items {
            if change.relation_name == JOURNAL {
                out.push(change.change);
            }
        }
    }
    out
}

async fn read_counts(handle: &DbHandle) -> Vec<(String, String, String, i64)> {
    let rows = handle
        .query(
            "SELECT origin, transition_id, day, effect_count FROM automations_journal \
             ORDER BY origin, transition_id, day",
            HashMap::new(),
        )
        .await
        .expect("query automations_journal");
    rows.iter()
        .map(|r| {
            let get = |k: &str| match r.get(k) {
                Some(holon_api::Value::String(s)) => s.clone(),
                other => panic!("{k}: unexpected {other:?}"),
            };
            let count = match r.get("effect_count") {
                Some(holon_api::Value::Integer(i)) => *i,
                other => panic!("effect_count: unexpected {other:?}"),
            };
            (get("origin"), get("transition_id"), get("day"), count)
        })
        .collect()
}

#[tokio::test]
async fn rule_origin_history_row_fires_journal_cdc_and_maintains_delta() {
    let (handle, mut cdc_rx) = setup().await;

    // A rule-origin op's history row lands -> the journal group is created and
    // CDC fires (the rule-firing trigger shape F1a promises).
    record(&handle, 1, "rule", "delegate-work", "2026-07-16").await;
    let first = drain_journal_changes(&mut cdc_rx).await;
    assert!(
        first.iter().any(|c| matches!(c, Change::Created { .. })),
        "first rule-origin history row must fire a Created on {JOURNAL}; saw {first:?}",
    );
    assert_eq!(
        read_counts(&handle).await,
        vec![(
            "rule".into(),
            "delegate-work".into(),
            "2026-07-16".into(),
            1
        )],
    );

    // O(delta): a second effect in the SAME group emits CDC for that group only
    // (the count 1->2 update), not a re-emission of the whole view.
    record(&handle, 2, "rule", "delegate-work", "2026-07-16").await;
    let second = drain_journal_changes(&mut cdc_rx).await;
    assert!(
        !second.is_empty(),
        "same-group append must emit a maintenance change on {JOURNAL}; saw none",
    );
    assert_eq!(
        second.len(),
        1,
        "O(delta): only the touched group changes, not every group; saw {second:?}",
    );
    assert_eq!(
        read_counts(&handle).await,
        vec![(
            "rule".into(),
            "delegate-work".into(),
            "2026-07-16".into(),
            2
        )],
    );

    // A distinct day is a new group — one new Created, first group untouched.
    record(&handle, 3, "rule", "delegate-work", "2026-07-17").await;
    let third = drain_journal_changes(&mut cdc_rx).await;
    assert_eq!(
        third.len(),
        1,
        "new group emits exactly one change; saw {third:?}",
    );
    assert_eq!(
        read_counts(&handle).await,
        vec![
            (
                "rule".into(),
                "delegate-work".into(),
                "2026-07-16".into(),
                2
            ),
            (
                "rule".into(),
                "delegate-work".into(),
                "2026-07-17".into(),
                1
            ),
        ],
    );
}
