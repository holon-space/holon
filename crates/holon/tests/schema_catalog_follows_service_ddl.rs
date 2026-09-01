//! Contract: the schema catalog follows DDL issued by the agent-facing service
//! helpers.
//!
//! `HolonService::create_table` and `drop_table` build DDL and send it through
//! `execute_query`, not `execute_ddl` — they are the MCP `create_table` /
//! `drop_table` tools. A catalog fed only by the DDL-typed methods would keep
//! declaring a dropped table's columns and deny a created table's, and the
//! `_change_origin` rewriter would then emit SQL the engine refuses.

use std::sync::Arc;

use holon::api::HolonService;
use holon::api::holon_service::ColumnDef;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon_turso::sql_parser::ChangeOriginInjector;
use holon_turso::sql_parser::SqlTransformer;
use holon_turso::sql_parser::apply_sql_transforms;

fn origin_column() -> ColumnDef {
    ColumnDef {
        name: "_change_origin".to_string(),
        sql_type: "TEXT".to_string(),
        primary_key: false,
        default: None,
    }
}

fn id_column() -> ColumnDef {
    ColumnDef {
        name: "id".to_string(),
        sql_type: "TEXT".to_string(),
        primary_key: true,
        default: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_table_and_drop_table_move_the_catalog() {
    let engine = create_test_engine_with_providers(":memory:".into(), |module| module)
        .await
        .expect("test engine");
    let catalog = engine.db_handle().schema_catalog();
    let service = HolonService::new(Arc::clone(&engine));

    service
        .create_table("gizmo", &[id_column(), origin_column()])
        .await
        .expect("create_table");

    assert!(
        catalog.declares_column("gizmo", "_change_origin"),
        "a table the service really created must be in the catalog"
    );

    let transformers: Vec<Box<dyn SqlTransformer>> =
        vec![Box::new(ChangeOriginInjector::new(Arc::clone(&catalog)))];
    let rewritten = apply_sql_transforms("SELECT id FROM gizmo", &transformers);
    assert!(
        rewritten.contains("gizmo._change_origin"),
        "the column the service declared must be projected: {rewritten}"
    );
    engine
        .db_handle()
        .query_positional(&rewritten, vec![])
        .await
        .expect("the rewritten SQL must run against the engine");

    service.drop_table("gizmo").await.expect("drop_table");

    assert!(
        !catalog.declares_column("gizmo", "_change_origin"),
        "a table the service really dropped must leave the catalog"
    );
}
