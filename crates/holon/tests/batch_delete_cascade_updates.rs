//! What a batch's post-commit "every UPDATEd row still exists" assertion
//! reports, and for which cause, driven through the PRODUCTION write path
//! (`SqlOperationProvider::execute_batch_with_origin`) against the REAL
//! `block_raw` schema.
//!
//! Three causes, three verdicts:
//! - the caller deleted the row itself → intended, silent;
//! - a delete in the batch cascaded over a row the batch also UPDATEs → the
//!   caller's tree and the sink disagree, and only a reseed settles it;
//! - the row is gone for a reason this batch cannot see → the sink lost it.
//!
//! The last two both Err, because both are recovered the same way, but the
//! message must name the right one — a lane hunting a projection stall reads
//! it as its first evidence.

use std::collections::HashMap;
use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::storage::schema_module::EdgeFieldDescriptor;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::Value;
use holon_core::BatchOp;
use holon_core::EventOrigin;
use holon_core::OperationProvider;
use holon_core::OriginTaggedWrites;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::LinkSchemaModule;

const ENTITY: &str = "block";
const TABLE: &str = "block_raw";
const ROOT: &str = "sentinel:no_parent";

fn tags_descriptor() -> EdgeFieldDescriptor {
    EdgeFieldDescriptor {
        entity: ENTITY.to_string(),
        field: "tags".to_string(),
        join_table: "block_tags".to_string(),
        source_col: "block_id".to_string(),
        target_col: "tag".to_string(),
    }
}

async fn setup() -> (DbHandle, Arc<SqlOperationProvider>) {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("core schema");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (
                block_id TEXT NOT NULL,
                tag TEXT NOT NULL,
                PRIMARY KEY (block_id, tag),
                FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE
            )",
        )
        .await
        .expect("block_tags table");
    LinkSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("link schema");
    let provider = Arc::new(SqlOperationProvider::with_edge_fields(
        handle.clone(),
        TABLE.to_string(),
        ENTITY.to_string(),
        ENTITY.to_string(),
        vec![tags_descriptor()],
    ));
    (handle, provider)
}

fn params(pairs: &[(&str, &str)]) -> holon_api::StorageEntity {
    let mut p: holon_api::StorageEntity = HashMap::new();
    for (k, v) in pairs {
        p.insert((*k).into(), Value::String((*v).to_string()));
    }
    p
}

/// `pp` → `c1` → `c2`, plus `o1` directly under the root.
async fn seed(provider: &SqlOperationProvider) {
    let entity: EntityName = ENTITY.to_string().into();
    for (id, parent, content) in [
        ("block:pp", ROOT, "PP"),
        ("block:c1", "block:pp", "C1"),
        ("block:c2", "block:c1", "C2"),
        ("block:o1", ROOT, "O1"),
    ] {
        provider
            .execute_operation(
                &entity,
                "create",
                params(&[("id", id), ("parent_id", parent), ("content", content)]),
            )
            .await
            .unwrap_or_else(|e| panic!("seed {id}: {e}"));
    }
}

async fn ids(handle: &DbHandle) -> Vec<String> {
    let mut rows: Vec<String> = handle
        .query("SELECT id FROM block_raw", HashMap::new())
        .await
        .expect("select ids")
        .into_iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
        .collect();
    rows.sort();
    rows
}

/// The batch edits `c2` and deletes its parent `c1`, naming `c2` in neither
/// the delete nor a reparent. Its two ops disagree about whether `c2` lives,
/// and the sink can only honour one of them — so the batch is reported, and
/// the diagnosis names the cascade rather than blaming the sink.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_of_a_block_the_same_batch_cascades_away_names_the_cascade() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed(&provider).await;

    let err = provider
        .execute_batch_with_origin(
            &entity,
            vec![
                BatchOp::data(
                    "update",
                    params(&[("id", "block:c2"), ("content", "C2 edited")]),
                ),
                BatchOp::data("delete", params(&[("id", "block:c1")])),
            ],
            EventOrigin::Org,
        )
        .await
        .expect_err("a batch that both edits and cascades over a row must be reported");
    let msg = err.to_string();
    assert!(
        msg.contains("block:c2") && msg.contains("delete cascade removed"),
        "the diagnosis must name the cascade and the row; got: {msg}"
    );
    assert!(
        !msg.contains("the sink lost"),
        "the sink lost nothing here, and saying so sends the reader hunting a \
         storage bug; got: {msg}"
    );
    assert_eq!(
        ids(&handle).await,
        vec!["block:o1", "block:pp", ROOT],
        "the commit stands: the cascade removed exactly the deleted subtree"
    );
}

/// Reversed op order reaches the same end state, so it must reach the same
/// diagnosis: the batch carries no ordering contract between an update and a
/// delete of some other block.
#[tokio::test(flavor = "multi_thread")]
async fn the_diagnosis_does_not_depend_on_where_the_delete_sits_in_the_batch() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed(&provider).await;

    let err = provider
        .execute_batch_with_origin(
            &entity,
            vec![
                BatchOp::data("delete", params(&[("id", "block:c1")])),
                BatchOp::data(
                    "update",
                    params(&[("id", "block:c2"), ("content", "C2 edited")]),
                ),
            ],
            EventOrigin::Org,
        )
        .await
        .expect_err("delete-before-update of the same subtree is the same contradiction");
    let msg = err.to_string();
    assert!(
        msg.contains("block:c2") && msg.contains("delete cascade removed"),
        "op order must not change which cause is reported; got: {msg}"
    );
    assert_eq!(ids(&handle).await, vec!["block:o1", "block:pp", ROOT]);
}

/// A block the cascade sweeps up AND the batch names in its own `delete`.
/// The caller asked for it to go, so there is no disagreement to report.
#[tokio::test(flavor = "multi_thread")]
async fn a_block_both_cascaded_and_explicitly_deleted_is_not_reported() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed(&provider).await;

    let result = provider
        .execute_batch_with_origin(
            &entity,
            vec![
                BatchOp::data(
                    "update",
                    params(&[("id", "block:c2"), ("content", "C2 edited")]),
                ),
                BatchOp::data("delete", params(&[("id", "block:c2")])),
                BatchOp::data("delete", params(&[("id", "block:c1")])),
            ],
            EventOrigin::Org,
        )
        .await;

    assert!(
        result.is_ok(),
        "a block the caller itself deleted is gone by intent; got: {:?}",
        result.err().map(|e| e.to_string())
    );
    assert_eq!(ids(&handle).await, vec!["block:o1", "block:pp", ROOT]);
}

/// A row outside the deleted subtree is untouched by any of this: its edit
/// lands and the batch is silent.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_outside_the_cascade_is_still_asserted_and_still_applied() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed(&provider).await;

    provider
        .execute_batch_with_origin(
            &entity,
            vec![
                BatchOp::data(
                    "update",
                    params(&[("id", "block:o1"), ("content", "O1 edited")]),
                ),
                BatchOp::data("delete", params(&[("id", "block:c1")])),
            ],
            EventOrigin::Org,
        )
        .await
        .expect("an edit outside the deleted subtree is a correct batch");

    assert_eq!(ids(&handle).await, vec!["block:o1", "block:pp", ROOT]);
    let content: Vec<String> = handle
        .query(
            "SELECT content FROM block_raw WHERE id = 'block:o1'",
            HashMap::new(),
        )
        .await
        .expect("select o1")
        .into_iter()
        .filter_map(|r| {
            r.get("content")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(content, vec!["O1 edited".to_string()]);
}

/// The shape the postcondition exists for: the caller UPDATEs a row that is
/// gone from the sink and that THIS batch never touched. SQL would grant that
/// UPDATE silently against zero rows, so the batch must fail loudly instead —
/// and here the sink really did lose it, so the message says so.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_of_a_row_no_op_of_this_batch_removed_is_reported_loudly() {
    let (_handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed(&provider).await;

    provider
        .execute_batch_with_origin(
            &entity,
            vec![BatchOp::data("delete", params(&[("id", "block:c1")]))],
            EventOrigin::Org,
        )
        .await
        .expect("the delete batch itself is correct");

    let err = provider
        .execute_batch_with_origin(
            &entity,
            vec![BatchOp::data(
                "update",
                params(&[("id", "block:c2"), ("content", "C2 edited")]),
            )],
            EventOrigin::Org,
        )
        .await
        .expect_err("an UPDATE of a row this batch never removed must be loud");
    let msg = err.to_string();
    assert!(
        msg.contains("block:c2") && msg.contains("the sink lost"),
        "a row no op of this batch removed is a sink loss, and the message must \
         say that; got: {msg}"
    );
}
