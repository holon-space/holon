//! The D3.a seam, driven through the real CDC callback.
//!
//! One commit window routinely carries several writers, and `process_cdc_event`
//! is the ONE place that decides what happens to them: the batch is attributed
//! to the first `_change_origin` it sees and LINKS the rest. Every other test
//! of that ruling synthesizes a `BatchMetadata` and hands it to `LiveData`,
//! which never runs this function — so the production half was verified by
//! reading only (task #15 verification, caveat 1).
//!
//! Here two rows with two different origins land in ONE transaction and the
//! assertion reads the metadata the callback actually produced.

use std::time::Duration;

use holon_api::BatchWithMetadata;
use holon_api::ChangeOrigin;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::RowChange;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast::Receiver;

const VIEW: &str = "origin_probe_view";

/// A `_change_origin` payload naming one interaction, in the exact shape the
/// batch writer stamps (`sql_operation_provider`'s `local_with_current_span`).
fn origin(trace: &str, span: &str) -> String {
    ChangeOrigin::local_with_trace(Some(trace.to_string()), Some(span.to_string())).to_json()
}

async fn setup() -> (DbHandle, Receiver<BatchWithMetadata<RowChange>>) {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend); // keep the actor alive for the test
    handle
        .execute_ddl(
            "CREATE TABLE origin_probe (id TEXT PRIMARY KEY, content TEXT, _change_origin TEXT)",
        )
        .await
        .expect("base table");
    reconcile_named_view(
        &handle,
        VIEW,
        "SELECT id, content, _change_origin FROM origin_probe",
    )
    .await
    .expect("probe matview");
    // Subscribe after the view exists but before any write — broadcast delivers
    // only to subscribers present at send time.
    let rx = handle.subscribe_row_changes();
    (handle, rx)
}

/// The one batch this relation emitted. More than one means the two writes did
/// not share a commit window, which would make the test vacuous rather than
/// green — so that is a failure, not a filter.
async fn sole_batch(
    rx: &mut Receiver<BatchWithMetadata<RowChange>>,
) -> BatchWithMetadata<RowChange> {
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut batches = Vec::new();
    while let Ok(batch) = rx.try_recv() {
        if batch.metadata.relation_name == VIEW {
            batches.push(batch);
        }
    }
    assert_eq!(
        batches.len(),
        1,
        "the two rows must reach CDC as ONE consolidated batch for the \
         attribute-first-link-the-rest rule to have anything to decide; got {} batches with \
         sizes {:?}",
        batches.len(),
        batches
            .iter()
            .map(|b| b.inner.items.len())
            .collect::<Vec<_>>()
    );
    batches.pop().expect("checked non-empty")
}

#[tokio::test]
async fn a_two_writer_commit_parents_the_first_and_links_the_second() {
    let (handle, mut rx) = setup().await;

    handle
        .transaction(vec![
            (
                "INSERT INTO origin_probe (id, content, _change_origin) VALUES (?, ?, ?)"
                    .to_string(),
                vec![
                    turso::Value::Text("a".into()),
                    turso::Value::Text("first".into()),
                    turso::Value::Text(origin(
                        "4bf92f3577b34da6a3ce929d0e0e4736",
                        "00f067aa0ba902b7",
                    )),
                ],
            ),
            (
                "INSERT INTO origin_probe (id, content, _change_origin) VALUES (?, ?, ?)"
                    .to_string(),
                vec![
                    turso::Value::Text("b".into()),
                    turso::Value::Text("second".into()),
                    turso::Value::Text(origin(
                        "4bf92f3577b34da6a3ce929d0e0e4737",
                        "00f067aa0ba902b8",
                    )),
                ],
            ),
        ])
        .await
        .expect("one commit carrying two writers");

    let batch = sole_batch(&mut rx).await;
    let parent = batch
        .metadata
        .trace_context
        .as_ref()
        .expect("a batch whose rows carry `_change_origin` must be attributed");
    assert_eq!(
        parent.span_id, "00f067aa0ba902b7",
        "the FIRST writer parents"
    );
    assert_eq!(
        batch
            .metadata
            .linked_contexts
            .iter()
            .map(|c| c.span_id.as_str())
            .collect::<Vec<_>>(),
        vec!["00f067aa0ba902b8"],
        "the writer past the first must be LINKED, not dropped — dropping it is how the \
         largest redundancy class became unattributable"
    );
}

/// The same origin twice is one interaction, not two: a link to the batch's own
/// parent would claim a second writer that never existed.
#[tokio::test]
async fn one_writer_writing_twice_produces_no_links() {
    let (handle, mut rx) = setup().await;
    let same = origin("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7");

    handle
        .transaction(vec![
            (
                "INSERT INTO origin_probe (id, content, _change_origin) VALUES (?, ?, ?)"
                    .to_string(),
                vec![
                    turso::Value::Text("a".into()),
                    turso::Value::Text("first".into()),
                    turso::Value::Text(same.clone()),
                ],
            ),
            (
                "INSERT INTO origin_probe (id, content, _change_origin) VALUES (?, ?, ?)"
                    .to_string(),
                vec![
                    turso::Value::Text("b".into()),
                    turso::Value::Text("second".into()),
                    turso::Value::Text(same),
                ],
            ),
        ])
        .await
        .expect("one commit, one writer, two rows");

    let batch = sole_batch(&mut rx).await;
    assert_eq!(
        batch
            .metadata
            .trace_context
            .as_ref()
            .map(|c| c.span_id.as_str()),
        Some("00f067aa0ba902b7")
    );
    assert!(
        batch.metadata.linked_contexts.is_empty(),
        "one interaction must produce zero links; got {:?}",
        batch.metadata.linked_contexts
    );
}

/// A projection that drops `_change_origin` cannot attribute its batches — the
/// mechanism behind every unparented `live_data.apply_batch` (task #27). Pinned
/// so the day a mirror's SELECT list gains the column, this test says so.
#[tokio::test]
async fn a_projection_without_the_origin_column_attributes_nothing() {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
        .execute_ddl(
            "CREATE TABLE origin_probe (id TEXT PRIMARY KEY, content TEXT, _change_origin TEXT)",
        )
        .await
        .expect("base table");
    reconcile_named_view(&handle, VIEW, "SELECT id, content FROM origin_probe")
        .await
        .expect("probe matview without the origin column");
    let mut rx = handle.subscribe_row_changes();

    handle
        .execute(
            "INSERT INTO origin_probe (id, content, _change_origin) VALUES (?, ?, ?)",
            vec![
                turso::Value::Text("a".into()),
                turso::Value::Text("first".into()),
                turso::Value::Text(origin(
                    "4bf92f3577b34da6a3ce929d0e0e4736",
                    "00f067aa0ba902b7",
                )),
            ],
        )
        .await
        .expect("attributed write");

    let batch = sole_batch(&mut rx).await;
    assert!(
        batch.metadata.trace_context.is_none() && batch.metadata.linked_contexts.is_empty(),
        "the row carried an origin but the projection does not select it, so the batch has \
         none — got parent {:?} links {:?}",
        batch.metadata.trace_context,
        batch.metadata.linked_contexts
    );
}

const TRACE_A: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const SPAN_A: &str = "00f067aa0ba902b7";
const TRACE_B: &str = "4bf92f3577b34da6a3ce929d0e0e4737";
const SPAN_B: &str = "00f067aa0ba902b8";

/// Row `a` written under trace A in its own commit; that commit's batch is
/// consumed.
async fn row_written_under_a(handle: &DbHandle, rx: &mut Receiver<BatchWithMetadata<RowChange>>) {
    handle
        .execute(
            "INSERT INTO origin_probe (id, content, _change_origin) VALUES (?, ?, ?)",
            vec![
                turso::Value::Text("a".into()),
                turso::Value::Text("first".into()),
                turso::Value::Text(origin(TRACE_A, SPAN_A)),
            ],
        )
        .await
        .expect("write under A");
    sole_batch(rx).await;
}

/// The view delivers an update as DELETE(old row) + INSERT(new row). The old
/// row still names A; the update is B's alone.
#[tokio::test]
async fn an_update_is_attributed_to_its_own_writer_not_the_rows_previous_one() {
    let (handle, mut rx) = setup().await;
    row_written_under_a(&handle, &mut rx).await;

    handle
        .execute(
            "UPDATE origin_probe SET content = ?, _change_origin = ? WHERE id = 'a'",
            vec![
                turso::Value::Text("second".into()),
                turso::Value::Text(origin(TRACE_B, SPAN_B)),
            ],
        )
        .await
        .expect("update under B");

    let batch = sole_batch(&mut rx).await;
    let parent = batch.metadata.trace_context.as_ref().expect("attributed");
    assert_eq!(
        (parent.trace_id.as_str(), parent.span_id.as_str()),
        (TRACE_B, SPAN_B)
    );
    assert!(
        batch.metadata.linked_contexts.is_empty(),
        "the previous writer A is no writer of this batch; got links {:?}",
        batch.metadata.linked_contexts
    );
}

/// A plain DELETE stamps no origin, so the deleted row's last writer is the
/// only origin its CDC event carries.
#[tokio::test]
async fn a_delete_without_an_insert_names_the_deleted_rows_last_writer() {
    let (handle, mut rx) = setup().await;
    row_written_under_a(&handle, &mut rx).await;

    handle
        .execute("DELETE FROM origin_probe WHERE id = 'a'", vec![])
        .await
        .expect("delete");

    let batch = sole_batch(&mut rx).await;
    let parent = batch.metadata.trace_context.as_ref().expect("attributed");
    assert_eq!(
        (parent.trace_id.as_str(), parent.span_id.as_str()),
        (TRACE_A, SPAN_A)
    );
    assert!(batch.metadata.linked_contexts.is_empty());
}
