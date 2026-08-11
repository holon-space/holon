//! D1 (#63): the `sort_key TEXT NOT NULL DEFAULT 'A0'` column default must
//! never be what a SqlOnly block create lands on.
//!
//! These tests drive the PRODUCTION composition, not a convenient seam. In
//! SqlOnly mode `block.create` is served by [`OrderedBlockCrud`] wrapping a
//! bare `SqlOperationProvider`, with `SqlBlockOperations` as the order owner —
//! the `crud_authority: None` arm of `turso_seams.rs`. `SqlBlockOperations`
//! itself advertises only the STRUCTURAL ops, so a refusal placed on its
//! `create` is never reached by a dispatched `block.create`; that mistake is
//! exactly what this file exists to catch.
//!
//! The mode split is structural rather than a runtime check: under Loro
//! authority this decorator is NOT installed (`turso_seams.rs` takes the
//! `Some(authority)` arm and block CRUD lands in the Loro doc), and the SQL row
//! is written keyless by the projection on purpose. `OrderedBlockCrud` is
//! therefore SqlOnly by construction, and the undecorated writer must keep
//! accepting keyless creates.

use std::collections::HashMap;
use std::sync::Arc;

use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::Value;
use holon_api::block::Block;
use holon_app::ordered_block_crud::OrderedBlockCrud;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;

/// The `turso_seams.rs` `crud_authority: None` composition, verbatim:
/// `OrderedBlockCrud::new(sql_ops, SqlBlockOperations::new(sql_ops, cache),
/// sql_ops)`. Returns the decorator (what production dispatches `block.create`
/// to) and the bare provider underneath (what Loro mode's projection writes
/// through).
async fn sqlonly_block_crud() -> (
    TursoBackend,
    DbHandle,
    OrderedBlockCrud,
    Arc<SqlOperationProvider>,
) {
    let (backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("FK pragma");
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("core schema");
    BlockSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block schema");

    let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
        handle.clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
    ));
    let mut block_raw_type_def = Block::type_definition();
    block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
    let cache = Arc::new(
        QueryableCache::<Block>::new(handle.clone(), block_raw_type_def)
            .await
            .expect("block_raw cache"),
    );
    let order_owner = Arc::new(SqlBlockOperations::new(sql_ops.clone(), cache));
    let provider = OrderedBlockCrud::new(
        sql_ops.clone() as Arc<dyn OperationProvider>,
        order_owner,
        sql_ops.clone(),
    );
    (backend, handle, provider, sql_ops)
}

fn create_params(id: &str) -> StorageEntity {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String("root-shaped".to_string()));
    params
}

async fn read_sort_key(handle: &DbHandle, id: &str) -> Option<String> {
    let rows = handle
        .query(
            &format!(
                "SELECT sort_key FROM block_raw WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("read sort_key");
    rows.into_iter().next().map(|r| {
        r.get("sort_key")
            .and_then(|v| v.as_string())
            .expect("sort_key column")
            .to_string()
    })
}

async fn seed_parent(handle: &DbHandle, id: &str) {
    handle
        .execute(
            &format!(
                "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                 created_at, updated_at) VALUES ('{id}', 'sentinel:no_parent', 'V', 'parent', \
                 'text', 0, 0)"
            ),
            vec![],
        )
        .await
        .expect("seed parent");
}

fn assert_refusal_names_the_block(err: &str, id: &str) {
    assert!(
        err.contains("sort_key"),
        "the refusal must name the missing key; got: {err}"
    );
    assert!(
        err.contains(id),
        "the refusal must name the block it refused; got: {err}"
    );
}

/// THE rung this lane's first round missed: drives `block.create` through the
/// provider production actually dispatches to.
#[tokio::test(flavor = "multi_thread")]
async fn a_sqlonly_prod_path_create_without_a_sort_key_fails_loudly() {
    let (_backend, handle, provider, _sql) = sqlonly_block_crud().await;

    let outcome = provider
        .execute_operation(
            &EntityName::new("block"),
            "create",
            create_params("block:prodkeyless"),
        )
        .await;

    let Err(err) = outcome else {
        let landed = read_sort_key(&handle, "block:prodkeyless").await;
        panic!(
            "the SqlOnly production create path must refuse a block with no sort_key and no \
             parent_id; it succeeded and the row landed at sort_key {landed:?}"
        );
    };
    assert_refusal_names_the_block(&format!("{err}"), "block:prodkeyless");
    assert_eq!(
        read_sort_key(&handle, "block:prodkeyless").await,
        None,
        "a refused create must leave no row behind"
    );
}

/// Hazard 3: `""` is not a position — it collides in the keyspace exactly like
/// the `A0` sentinel, so a caller that supplies it is refused rather than
/// silently minted over.
#[tokio::test(flavor = "multi_thread")]
async fn a_sqlonly_create_with_an_empty_sort_key_is_refused() {
    let (_backend, handle, provider, _sql) = sqlonly_block_crud().await;

    let mut params = create_params("block:emptykey");
    params.insert("sort_key".into(), Value::String(String::new()));

    let Err(err) = provider
        .execute_operation(&EntityName::new("block"), "create", params)
        .await
    else {
        panic!(
            "an empty sort_key must be refused; it succeeded and landed at {:?}",
            read_sort_key(&handle, "block:emptykey").await
        );
    };
    assert_refusal_names_the_block(&format!("{err}"), "block:emptykey");
}

/// Hazard 4: a `Value::Null` key used to slip past a `contains_key` check and
/// die one layer down as `NOT NULL constraint failed: block_raw.sort_key` — a
/// message naming neither the block nor the remedy. It gets the same named
/// refusal as every other positionless create.
#[tokio::test(flavor = "multi_thread")]
async fn a_sqlonly_create_with_a_null_sort_key_is_refused() {
    let (_backend, _handle, provider, _sql) = sqlonly_block_crud().await;

    let mut params = create_params("block:nullkey");
    params.insert("sort_key".into(), Value::Null);

    let Err(err) = provider
        .execute_operation(&EntityName::new("block"), "create", params)
        .await
    else {
        panic!("a null sort_key must be refused, not deferred to the NOT NULL constraint");
    };
    assert_refusal_names_the_block(&format!("{err}"), "block:nullkey");
}

/// Guards against over-refusing: omitting `sort_key` is the LEGITIMATE
/// creation-slot contract when a parent is named ("type here" appends to the
/// end). That path must still mint, not refuse. GREEN before and after.
#[tokio::test(flavor = "multi_thread")]
async fn a_sqlonly_create_under_a_parent_still_mints_a_real_key() {
    let (_backend, handle, provider, _sql) = sqlonly_block_crud().await;
    seed_parent(&handle, "block:parent").await;

    let mut params = create_params("block:child");
    params.insert(
        "parent_id".into(),
        Value::String("block:parent".to_string()),
    );
    provider
        .execute_operation(&EntityName::new("block"), "create", params)
        .await
        .expect("a keyless create under a named parent must still be appended, not refused");

    let landed = read_sort_key(&handle, "block:child")
        .await
        .expect("the child row must exist");
    assert!(
        !landed.is_empty() && landed != "A0",
        "the appended child must carry a real minted key, not the sentinel; got {landed:?}"
    );
}

/// The mode-split anti-regression guard, GREEN before and after: the refusal
/// lives in the decorator, so the UNDECORATED writer — the bare
/// `SqlOperationProvider` that Loro mode's projection writes the SQL row
/// through, where the tree owns the fractional index and the key arrives later
/// — must keep accepting a keyless create.
#[tokio::test(flavor = "multi_thread")]
async fn the_undecorated_projection_writer_still_accepts_a_keyless_create() {
    let (_backend, handle, _provider, sql) = sqlonly_block_crud().await;

    sql.execute_operation(
        &EntityName::new("block"),
        "create",
        create_params("block:projected"),
    )
    .await
    .expect("the undecorated projection writer must still accept a keyless create");

    assert!(
        read_sort_key(&handle, "block:projected").await.is_some(),
        "the projected row must exist, awaiting the tree's key"
    );
}
