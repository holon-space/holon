//! A source-line write claims only the block it writes: typing in one block
//! never waits behind a write on another.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::SOURCE_TEXT_FIELD;
use holon_api::SourceKeystroke;
use holon_api::Value;
use holon_api::admission::AdmissionCensus;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const PAGE: &str = "block:page";
const HELD: &str = "block:held";
const TYPED: &str = "block:typed";

async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread runtime.
                    tokio::runtime::Handle::current()
                        .block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with the SqlOnly block providers")
}

fn params(pairs: &[(&str, &str)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), Value::String((*v).to_string())))
        .collect()
}

async fn seed(engine: &BackendEngine) {
    for (id, parent) in [(PAGE, "sentinel:no_parent"), (HELD, PAGE), (TYPED, PAGE)] {
        engine
            .execute_operation(
                &EntityName::new("block"),
                "create",
                params(&[("id", id), ("parent_id", parent), ("content", id)]),
                OpOrigin::Sync,
            )
            .await
            .unwrap_or_else(|e| panic!("seed {id}: {e:#}"));
    }
}

/// Park a user's `set_field` on [`HELD`] inside its claim.
async fn hold_a_write(engine: &Arc<BackendEngine>) -> tokio::task::JoinHandle<()> {
    engine.dispatch_hold().hold_next("block", "set_field");
    let held = tokio::spawn(engine.execute_operation(
        &EntityName::new("block"),
        "set_field",
        params(&[("id", HELD), ("field", "content"), ("value", "held")]),
        OpOrigin::User,
    ));
    let parked = tokio::time::timeout(Duration::from_secs(10), async {
        while engine.dispatch_hold().parked_runs("block", "set_field") == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(parked.is_ok(), "the held set_field never reached the hold");
    tokio::spawn(async move {
        held.await
            .expect("the held write's task")
            .expect("the held write lands once released");
    })
}

async fn content(engine: &BackendEngine, id: &str) -> String {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT content FROM {BLOCK_WRITE_TABLE} WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .expect("read the block");
    rows[0]["content"]
        .as_string()
        .expect("content is text")
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_does_not_wait_behind_a_held_write_on_another_block() {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold_a_write(&engine).await;

    let keystroke = engine.commit_keystroke(SourceKeystroke {
        id: TYPED.into(),
        source: "typed while another block is written".into(),
        write_seq: None,
    });
    assert_eq!(
        engine.admission().census(),
        AdmissionCensus {
            released: 2,
            waiting: 0
        },
        "the keystroke on {TYPED} waits behind the held write on {HELD}"
    );
    tokio::time::timeout(Duration::from_secs(10), keystroke)
        .await
        .expect("the keystroke finishes while the other write is held")
        .expect("the keystroke lands");
    assert_eq!(
        content(&engine, TYPED).await,
        "typed while another block is written"
    );

    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the held write");
    held.await.expect("the held write completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_line_write_does_not_wait_behind_a_held_write_on_another_block() {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold_a_write(&engine).await;

    let write = engine.execute_operation(
        &EntityName::new("block"),
        "set_field",
        params(&[
            ("id", TYPED),
            ("field", SOURCE_TEXT_FIELD),
            ("value", "TODO written while another block is written"),
        ]),
        OpOrigin::User,
    );
    assert_eq!(
        engine.admission().census(),
        AdmissionCensus {
            released: 2,
            waiting: 0
        },
        "the source-line write on {TYPED} waits behind the held write on {HELD}"
    );
    tokio::time::timeout(Duration::from_secs(10), write)
        .await
        .expect("the source-line write finishes while the other write is held")
        .expect("the source-line write lands");

    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the held write");
    held.await.expect("the held write completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_waits_behind_a_held_write_on_its_own_block() {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold_a_write(&engine).await;

    let keystroke = tokio::spawn(engine.commit_keystroke(SourceKeystroke {
        id: HELD.into(),
        source: "typed after the held write".into(),
        write_seq: None,
    }));
    assert_eq!(
        engine.admission().census(),
        AdmissionCensus {
            released: 1,
            waiting: 1
        }
    );
    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the held write");
    held.await.expect("the held write completes");
    keystroke
        .await
        .expect("the keystroke's task")
        .expect("the keystroke lands after the held write");
    assert_eq!(content(&engine, HELD).await, "typed after the held write");
}
