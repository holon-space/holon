//! A malformed entity reference reaching a write through the production
//! engine route — `BackendEngine::execute_operation`, whose own pre-dispatch
//! steps read the op's ids before the dispatcher does — is refused by name,
//! never a panic and never a silent write.

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::CrudOperations;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// The production SqlOnly block wiring: the CRUD authority plus the structural
/// provider.
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
                    let handle = tokio::runtime::Handle::current();
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread test
                    // runtime; block_in_place makes the bridge deadlock-free.
                    handle.block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with block provider")
}

/// An engine holding the root blocks `block:a` and `block:b`.
async fn engine_with_two_blocks() -> Arc<BackendEngine> {
    let engine = block_engine().await;
    for id in ["block:a", "block:b"] {
        let params: StorageEntity = HashMap::from([
            ("id".into(), Value::String(id.into())),
            ("content".into(), Value::String(id.into())),
            (
                "parent_id".into(),
                Value::String("sentinel:no_parent".into()),
            ),
        ]);
        engine
            .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
            .await
            .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
    }
    engine
}

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

fn text(s: &str) -> Value {
    Value::String(s.into())
}

/// Each case runs on its own engine in its own task, so a panic is reported
/// as that case's outcome instead of ending the table.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_id_through_the_engine_is_refused_by_name() {
    let cases: Vec<(&str, &str, StorageEntity)> = vec![
        ("indent", "id", params(&[("id", text("a b"))])),
        ("outdent", "id", params(&[("id", text("a b"))])),
        (
            "move_block",
            "id",
            params(&[("id", text("a b")), ("parent_id", text("block:a"))]),
        ),
        (
            "move_block",
            "parent_id",
            params(&[("id", text("block:b")), ("parent_id", text("a b"))]),
        ),
        (
            "set_field",
            "id",
            params(&[
                ("id", text("a b")),
                ("field", text("task_state")),
                ("value", text("TODO")),
            ]),
        ),
        (
            "set_field",
            "id",
            params(&[
                ("id", text("a b")),
                ("field", text("content")),
                ("value", text("TODO x")),
            ]),
        ),
        (
            "update",
            "id",
            params(&[("id", text("a b")), ("content", text("x"))]),
        ),
        ("cycle_task_state", "id", params(&[("id", text("a b"))])),
        (
            "merge_blocks",
            "canonical",
            params(&[("canonical", text("a b")), ("duplicate", text("block:b"))]),
        ),
        (
            "convert_block_to_page",
            "target",
            params(&[("target", text("a b"))]),
        ),
    ];

    let mut failures = Vec::new();
    for (op, param, case) in cases {
        let outcome = tokio::spawn(async move {
            let engine = engine_with_two_blocks().await;
            engine
                .execute_operation(&EntityName::new("block"), op, case, OpOrigin::User)
                .await
                .map(|_| ())
        })
        .await;
        match outcome {
            Err(join) => failures.push(format!("{op}({param} = \"a b\"): panicked: {join}")),
            Ok(Ok(())) => failures.push(format!("{op}({param} = \"a b\"): returned Ok")),
            Ok(Err(e)) => {
                let e = format!("{e:#}");
                if !(e.contains(&format!("parameter '{param}'")) && e.contains("\"a b\"")) {
                    failures.push(format!("{op}({param} = \"a b\"): unnamed refusal: {e}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The same edge write is refused by name under the Loro CRUD authority and
/// under SqlOnly, and the junction keeps nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_edge_target_is_refused_by_name_under_both_authorities() {
    let targets = Value::Array(vec![text("a b")]);
    let named = |e: &str| e.contains("edge field 'requires'") && e.contains("\"a b\"");

    let dir = tempfile::tempdir().expect("tempdir");
    let loro = holon_loro::LoroBlockOperations::new(Arc::new(tokio::sync::RwLock::new(
        holon_loro::LoroDocumentStore::new(dir.path().to_path_buf()),
    )));
    loro.create(params(&[
        ("id", text("block:a")),
        ("content", text("a")),
        ("parent_id", text("sentinel:no_parent")),
    ]))
    .await
    .expect("create block:a under Loro");
    let loro_err = loro
        .set_field("block:a", "requires", targets.clone())
        .await
        .expect_err("Loro set_field must refuse the target")
        .to_string();

    let engine = engine_with_two_blocks().await;
    let set_field = engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params(&[
                ("id", text("block:a")),
                ("field", text("requires")),
                ("value", targets.clone()),
            ]),
            OpOrigin::User,
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:#}"));
    let create = engine
        .execute_operation(
            &EntityName::new("block"),
            "create",
            params(&[
                ("id", text("block:c")),
                ("content", text("c")),
                ("parent_id", text("sentinel:no_parent")),
                ("requires", targets),
            ]),
            OpOrigin::User,
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:#}"));
    let junction = engine
        .db_handle()
        .query(
            "SELECT block_id, required_id FROM block_requires",
            HashMap::new(),
        )
        .await
        .expect("read the junction");

    assert!(named(&loro_err), "Loro: {loro_err}");
    assert!(
        matches!(&set_field, Err(e) if named(e)),
        "SqlOnly set_field: {set_field:?}"
    );
    assert!(
        matches!(&create, Err(e) if named(e)),
        "SqlOnly create: {create:?}"
    );
    assert!(junction.is_empty(), "junction rows: {junction:?}");
}
