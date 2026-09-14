//! The operation boundary accepts exactly ONE block-id form, and an undo of a
//! write through it applies.
//!
//! Org files on disk store bare ids and the org parser adds the scheme; a
//! caller reaching the dispatcher is past that parse. So the dispatcher parses
//! every entity reference once and refuses a bare one, and the precondition
//! reader — which keys on the scheme-qualified id `block_raw` rows carry — can
//! never be handed a spelling it does not key on.

#[path = "undo_precondition_id_scheme/harness.rs"]
mod harness;

use std::collections::HashMap;

use harness::PROBE_CHILD;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::UndoOutcome;
use holon_api::Value;

/// The ingested block's projected content: the headline plus its body.
const ORIGINAL: &str = "A child block\nSome text so the write-back fold has a document to render.";
const TYPED: &str = "set by the user";

async fn projected_content(engine: &holon::api::BackendEngine, bare: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT content FROM block_raw WHERE id = 'block:{bare}'"),
            HashMap::new(),
        )
        .await
        .expect("read the projected content of the probe block");
    rows.first()
        .and_then(|r| r.get("content"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

/// Block until the SQL projection of the probe block reads `want`. The Loro →
/// `block_raw` projection is asynchronous, so a read taken straight after a
/// write legitimately still shows the prior value; every assertion in this
/// suite is about the undo path, not about that latency.
async fn await_projected(engine: &holon::api::BackendEngine, bare: &str, want: &str) {
    for _ in 0..200 {
        if projected_content(engine, bare).await.as_deref() == Some(want) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!(
        "the projection never reached {want:?} for {bare} (last read: {:?})",
        projected_content(engine, bare).await
    );
}

async fn dispatch_set_content(
    engine: &holon::api::BackendEngine,
    id: &str,
    value: &str,
) -> anyhow::Result<holon_api::OpOutcome> {
    let mut params: holon_api::StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert("value".into(), Value::String(value.to_string()));
    engine
        .execute_operation(
            &EntityName::from("block"),
            "set_field",
            params,
            OpOrigin::User,
        )
        .await
}

async fn set_content(engine: &holon::api::BackendEngine, id: &str, value: &str) {
    dispatch_set_content(engine, id, value)
        .await
        .unwrap_or_else(|e| {
            panic!("a user set_field on {id} through the production dispatcher: {e:#}")
        });
}

/// The pin. A `set_field` addressed by the block's BARE id is refused at the
/// operation boundary, naming the parameter and the offending value — and the
/// block it named keeps its content.
#[tokio::test(flavor = "multi_thread")]
async fn a_bare_block_id_is_refused_at_the_operation_boundary() {
    let booted = harness::boot_a_working_session().await;
    let engine = booted.engine.clone();

    // Vacuity guard: without the ingested block projected, the gesture below
    // would name an absent block and the refusal could be about that instead.
    await_projected(&engine, PROBE_CHILD, ORIGINAL).await;

    let refusal = dispatch_set_content(&engine, PROBE_CHILD, TYPED)
        .await
        .expect_err("a bare block id must be refused at the operation boundary");
    let expected = holon_api::UnschemedEntityReference {
        param: "id".to_string(),
        operation: "block/set_field".to_string(),
        value: PROBE_CHILD.to_string(),
        expected_scheme: "block".to_string(),
    }
    .to_string();
    assert!(
        format!("{refusal:#}").contains(&expected),
        "the refusal must name the parameter and the value.\n expected: {expected}\n got: \
         {refusal:#}"
    );
    assert_eq!(
        projected_content(&engine, PROBE_CHILD).await.as_deref(),
        Some(ORIGINAL),
        "the refused write must leave the block's content alone"
    );
}

/// Control: the SAME gesture addressed by the scheme-qualified id lands and is
/// undoable — what isolates the id scheme as the discriminator rather than the
/// wiring, the projection, or the journal.
#[tokio::test(flavor = "multi_thread")]
async fn control_undo_restores_content_written_through_a_qualified_block_id() {
    let booted = harness::boot_a_working_session().await;
    let engine = booted.engine.clone();

    await_projected(&engine, PROBE_CHILD, ORIGINAL).await;

    set_content(&engine, &format!("block:{PROBE_CHILD}"), TYPED).await;
    await_projected(&engine, PROBE_CHILD, TYPED).await;

    let outcome = booted.engine.undo().await.expect("undo must not refuse");
    assert!(
        matches!(outcome, UndoOutcome::Applied),
        "undo of a qualified-id set_field must apply; got {outcome:?}"
    );
    await_projected(&engine, PROBE_CHILD, ORIGINAL).await;
}
