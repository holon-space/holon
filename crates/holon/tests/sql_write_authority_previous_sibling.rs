//! The SQL write authority's previous sibling follows sibling order: by
//! `sort_key`, ties broken by `id`, the root sentinel never a sibling.

use std::sync::Arc;

use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::core::sql_write_authority::SqlWriteAuthority;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::WriteAuthorityReads;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

#[tokio::test(flavor = "multi_thread")]
async fn the_previous_sibling_is_the_one_before_in_sort_key_then_id_order() {
    let engine = create_test_engine_with_providers(":memory:".into(), |module| {
        module.with_operation_provider_factory(|backend| {
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
    })
    .await
    .expect("test engine");
    let rows = [
        ("block:page", "sentinel:no_parent", "A0"),
        ("block:a", "block:page", "a1"),
        ("block:c", "block:page", "a2"),
        ("block:b", "block:page", "a2"),
        ("block:d", "block:page", "a3"),
    ];
    for (id, parent, sort_key) in rows {
        let params: StorageEntity = [
            ("id", id),
            ("parent_id", parent),
            ("content", id),
            ("sort_key", sort_key),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), Value::String(v.to_string())))
        .collect();
        engine
            .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
            .await
            .unwrap_or_else(|e| panic!("seed {id}: {e:#}"));
    }
    let authority = SqlWriteAuthority::new(engine.db_handle().clone());
    for (of, expected) in [
        ("block:page", None),
        ("block:a", None),
        ("block:b", Some("block:a")),
        ("block:c", Some("block:b")),
        ("block:d", Some("block:c")),
    ] {
        let of = EntityUri::from_raw(of);
        let near = authority
            .tagged_neighbourhood(&["decision"], std::slice::from_ref(&of), Some(&of))
            .await
            .expect("read")
            .expect("the SQL authority answers the read");
        assert_eq!(
            near.previous_sibling().map(EntityUri::as_str),
            expected,
            "previous sibling of {of}"
        );
    }
}
