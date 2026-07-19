//! Undo INVERSE COVERAGE WAVE 2 — exact inverses for the mark ops
//! (`apply_mark` / `remove_mark`) and for arbitrary-property `set_field`
//! (`set_due_date`, `set_priority`, generic org drawer keys).
//!
//! These inverses live in the **Loro** provider (`LoroBlockOperations`, the
//! default prod write authority — the SQL provider has no `MarkOperations`),
//! so wave 2 drives that provider directly rather than the SqlOnly engine
//! `undo_inverse_wave1` uses. Each test asserts undo∘op ≡ identity: the state
//! read back after replaying the returned inverse equals the pre-op state,
//! marks and properties included.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityName;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::CrudOperations;
use holon_core::DataSource;
use holon_core::MarkOperations;
use holon_core::OperationProvider;
use holon_core::TaskOperations;
use holon_core::UndoAction;
use holon_loro::LoroBlockOperations;
use holon_loro::LoroDocumentStore;
use tokio::sync::RwLock;

/// Build a Loro block provider over a fresh temp store holding one anchor block
/// `block:anchor` with plain text "a task" (marks `None`, no properties).
async fn ops_with_anchor() -> (LoroBlockOperations, tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(RwLock::new(LoroDocumentStore::new(dir.path().to_path_buf())));
    let ops = LoroBlockOperations::new(store);

    let mut fields: StorageEntity = HashMap::new();
    fields.insert("id".into(), Value::String("block:anchor".into()));
    fields.insert("parent_id".into(), Value::String("sentinel:no_parent".into()));
    fields.insert("content".into(), Value::String("a task".into()));
    let (id, _res) = ops.create(fields).await.expect("create anchor");
    (ops, dir, id)
}

/// Read the current block state through the public provider API.
async fn block(ops: &LoroBlockOperations, id: &str) -> Block {
    ops.get_by_id(id)
        .await
        .expect("get_by_id")
        .expect("anchor block present")
}

/// Replay a returned inverse through the same dispatcher the undo stack uses
/// (`execute_operation`), so it also crosses the intent boundary. Panics unless
/// the op is reversible — a `DeclaredIrreversible`/`Undeclared` here is a bug.
async fn replay(ops: &LoroBlockOperations, undo: &UndoAction) {
    let op = match undo {
        UndoAction::Undo(op) => op.clone(),
        other => panic!("expected a reversible inverse, got {other:?}"),
    };
    let params: StorageEntity = op
        .params
        .clone()
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect();
    ops.execute_operation(&EntityName::new("block"), &op.op_name, params)
        .await
        .expect("replay inverse");
}

fn bold(start: usize, end: usize) -> MarkSpan {
    MarkSpan::new(start, end, InlineMark::Bold)
}

fn mark_json(mark: &InlineMark) -> String {
    serde_json::to_string(mark).expect("InlineMark serialization is total")
}

// ---------------------------------------------------------------------------
// #4 — set_field(arbitrary property): exact inverse
// ---------------------------------------------------------------------------

/// A property that ALREADY had a value: the inverse restores the prior value
/// exactly (generic org drawer key routed through the properties map).
#[tokio::test(flavor = "multi_thread")]
async fn set_field_property_undo_restores_prior_value() {
    let (ops, _dir, id) = ops_with_anchor().await;

    ops.set_field(&id, "effort", Value::String("small".into()))
        .await
        .expect("seed effort");
    let result = ops
        .set_field(&id, "effort", Value::String("large".into()))
        .await
        .expect("overwrite effort");
    assert_eq!(
        block(&ops, &id).await.get_property("effort"),
        Some(Value::String("large".into())),
        "forward write took effect"
    );

    replay(&ops, &result.undo).await;
    assert_eq!(
        block(&ops, &id).await.get_property("effort"),
        Some(Value::String("small".into())),
        "undo restores the exact prior property value"
    );
}

/// A property that was PREVIOUSLY ABSENT: undo must REMOVE the key (leaving the
/// block genuinely property-free), NOT pin a null-valued key. Driven through
/// `set_priority`, which writes the `PRIORITY` property.
#[tokio::test(flavor = "multi_thread")]
async fn set_priority_first_time_undo_removes_property() {
    let (ops, _dir, id) = ops_with_anchor().await;
    assert_eq!(
        block(&ops, &id).await.get_property("PRIORITY"),
        None,
        "precondition: no prior PRIORITY"
    );

    let result = ops.set_priority(&id, 1).await.expect("set_priority");
    assert_eq!(
        block(&ops, &id).await.get_property("PRIORITY"),
        Some(Value::Integer(1)),
        "forward write took effect"
    );

    replay(&ops, &result.undo).await;
    assert_eq!(
        block(&ops, &id).await.get_property("PRIORITY"),
        None,
        "undo of a first-time property write REMOVES the key (not a null blob)"
    );
}

/// `set_due_date` (writes the `DEADLINE` property) is invertible: overwriting an
/// existing deadline and undoing restores the prior deadline exactly.
#[tokio::test(flavor = "multi_thread")]
async fn set_due_date_undo_restores_prior_deadline() {
    let (ops, _dir, id) = ops_with_anchor().await;
    let d1 = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let d2 = chrono::DateTime::parse_from_rfc3339("2026-02-02T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    ops.set_due_date(&id, Some(d1)).await.expect("seed deadline");
    let before = block(&ops, &id).await.get_property("DEADLINE");
    assert!(before.is_some(), "seeded a DEADLINE");

    let result = ops.set_due_date(&id, Some(d2)).await.expect("overwrite date");
    assert_ne!(
        block(&ops, &id).await.get_property("DEADLINE"),
        before,
        "forward write changed the deadline"
    );

    replay(&ops, &result.undo).await;
    assert_eq!(
        block(&ops, &id).await.get_property("DEADLINE"),
        before,
        "undo restores the prior deadline exactly"
    );
}

// ---------------------------------------------------------------------------
// #3 — apply_mark / remove_mark: exact inverses (per-range prior-mark capture)
// ---------------------------------------------------------------------------

/// `apply_mark`'s inverse restores the EXACT prior mark set — proven against a
/// real state change. Seed a block Bold over [0,6); apply a DIFFERENT mark
/// (Italic) over an overlapping sub-range; undo must drop the Italic AND leave
/// the pre-existing Bold fully intact. The inverse shape must be the whole-set
/// `content=Object` restore, never a `remove_mark`.
#[tokio::test(flavor = "multi_thread")]
async fn apply_mark_undo_restores_exact_prior_marks() {
    let (ops, _dir, id) = ops_with_anchor().await;

    ops.set_field(
        &id,
        "marks",
        Value::String(holon_api::marks_to_json(&[bold(0, 6)])),
    )
    .await
    .expect("seed bold");
    let before = block(&ops, &id).await.marks;
    assert_eq!(before, Some(vec![bold(0, 6)]), "seeded a single Bold span");

    let result = ops
        .apply_mark(&id, 1, 3, mark_json(&InlineMark::Italic))
        .await
        .expect("apply_mark italic");
    assert_ne!(
        block(&ops, &id).await.marks,
        before,
        "forward apply_mark changed the mark set (added Italic)"
    );

    // Inverse must be the atomic whole-set restore, not a blind remove_mark.
    match &result.undo {
        UndoAction::Undo(op) => {
            assert_eq!(op.op_name, "set_field");
            assert_eq!(
                op.params.get("field").and_then(|v| v.as_string()),
                Some("content"),
                "inverse restores via the atomic content=Object path"
            );
            assert!(
                matches!(op.params.get("value"), Some(Value::Object(_))),
                "inverse value must be a rich Object payload (whole-set restore)"
            );
        }
        other => panic!("apply_mark must be reversible, got {other:?}"),
    }

    replay(&ops, &result.undo).await;
    assert_eq!(
        block(&ops, &id).await.marks,
        before,
        "undo restores the exact prior mark set (Italic gone, Bold intact)"
    );
}

/// The overlapping-marks trap the plan calls out: applying a mark of the SAME
/// key over a sub-range of an existing span, then undoing, must NOT punch a
/// hole in the pre-existing span (which a blind `remove_mark(range, key)`
/// inverse would). Bold[0,6), apply Bold[2,4), undo ⇒ Bold[0,6) intact.
#[tokio::test(flavor = "multi_thread")]
async fn apply_mark_same_key_overlap_undo_leaves_no_hole() {
    let (ops, _dir, id) = ops_with_anchor().await;

    ops.set_field(
        &id,
        "marks",
        Value::String(holon_api::marks_to_json(&[bold(0, 6)])),
    )
    .await
    .expect("seed bold");
    let before = block(&ops, &id).await.marks;
    assert_eq!(before, Some(vec![bold(0, 6)]));

    let result = ops
        .apply_mark(&id, 2, 4, mark_json(&InlineMark::Bold))
        .await
        .expect("apply_mark bold sub-range");

    replay(&ops, &result.undo).await;
    assert_eq!(
        block(&ops, &id).await.marks,
        before,
        "undo restores a single contiguous Bold span — no [0,2)+[4,6) hole"
    );
}

/// `remove_mark`'s inverse restores the removed span. Bold[0,6), remove Bold
/// over [2,4) (splits into [0,2)+[4,6)), undo ⇒ the full Bold[0,6) is back.
#[tokio::test(flavor = "multi_thread")]
async fn remove_mark_undo_restores_removed_span() {
    let (ops, _dir, id) = ops_with_anchor().await;

    ops.set_field(
        &id,
        "marks",
        Value::String(holon_api::marks_to_json(&[bold(0, 6)])),
    )
    .await
    .expect("seed bold");
    let before = block(&ops, &id).await.marks;
    assert_eq!(before, Some(vec![bold(0, 6)]));

    let result = ops
        .remove_mark(&id, 2, 4, InlineMark::Bold.loro_key().to_string())
        .await
        .expect("remove_mark");
    assert_ne!(
        block(&ops, &id).await.marks,
        before,
        "forward remove_mark changed the mark set"
    );

    replay(&ops, &result.undo).await;
    assert_eq!(
        block(&ops, &id).await.marks,
        before,
        "undo restores the removed Bold span exactly"
    );
}
