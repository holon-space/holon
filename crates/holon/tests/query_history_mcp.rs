//! MCP `query_history` integration: an op dispatched through the same
//! `HolonService` facade the embedded MCP server uses (Agent origin) lands in
//! the C2b `block_history` relation, and `HolonService::query_history` — the
//! method the `query_history` tool wraps — reads it back with provenance
//! (VisionGapAnalysis C2b, ADR 0024 P8).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon::api::holon_service::HolonService;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::DbHandle;
use holon::testing::e2e_test_helpers::E2ETestContext;
use holon_api::EntityName;
use holon_api::HistoryQuery;
use holon_api::HistoryQueryArgs;
use holon_api::OpOrigin;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::FieldDelta;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result as DatasourceResult;
use holon_turso::schema_modules::HistorySchemaModule;

/// A minimal block provider whose `set_field` reports a `FieldDelta`, so the
/// dispatch chokepoint records a history event (the e2e scaffold's own provider
/// returns no deltas and would record nothing).
struct DeltaProvider {
    db_handle: DbHandle,
    entity_name: EntityName,
}

#[async_trait]
impl OperationProvider for DeltaProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        // The engine's startup check requires the block CRUD trio to be present.
        ["set_field", "create", "delete"]
            .into_iter()
            .map(|name| OperationDescriptor {
                entity_name: self.entity_name.clone(),
                entity_short_name: "block".to_string(),
                name: name.to_string(),
                display_name: name.to_string(),
                description: format!("{name} on block"),
                ..Default::default()
            })
            .collect()
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "missing 'id'".to_string())?
            .to_string();
        match op_name {
            "set_field" => {
                let field = params
                    .get("field")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "missing 'field'".to_string())?
                    .to_string();
                let value = params.get("value").cloned().unwrap_or(Value::Null);
                let sql = format!(
                    "UPDATE block_raw SET {field} = '{}' WHERE id = '{}'",
                    value.as_string().unwrap_or_default().replace('\'', "''"),
                    id.replace('\'', "''")
                );
                self.db_handle
                    .execute(&sql, vec![])
                    .await
                    .map_err(|e| format!("update failed: {e}"))?;
                Ok(OperationResult::irreversible(vec![FieldDelta::new(
                    id,
                    field,
                    Value::Null,
                    value,
                )]))
            }
            "create" => {
                let sql = format!(
                    "INSERT OR IGNORE INTO block_raw (id, parent_id, content) VALUES ('{}', \
                     'sentinel:no_parent', '')",
                    id.replace('\'', "''")
                );
                self.db_handle
                    .execute(&sql, vec![])
                    .await
                    .map_err(|e| format!("insert failed: {e}"))?;
                Ok(OperationResult::irreversible(Vec::new()))
            }
            "delete" => {
                let sql = format!(
                    "DELETE FROM block_raw WHERE id = '{}'",
                    id.replace('\'', "''")
                );
                self.db_handle
                    .execute(&sql, vec![])
                    .await
                    .map_err(|e| format!("delete failed: {e}"))?;
                Ok(OperationResult::irreversible(Vec::new()))
            }
            other => Err(format!("unknown op: {other}").into()),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn query_history_sees_a_dispatched_op() {
    let ctx = E2ETestContext::with_providers(|module| {
        module.with_operation_provider_factory(|backend| {
            let db_handle =
                tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
            Arc::new(DeltaProvider {
                db_handle,
                entity_name: EntityName::new("block"),
            })
        })
    })
    .await
    .expect("build engine with a delta-reporting block provider");

    let db = ctx.engine().db_handle();
    db.execute(
        "INSERT OR IGNORE INTO block_raw (id, parent_id, content) VALUES ('block:a', \
         'sentinel:no_parent', 'todo')",
        vec![],
    )
    .await
    .expect("seed row");
    // Guarantee the C2b relation exists (boot-owned; ensured here so the test is
    // robust to lazy resolution). Idempotent drop+recreate on the empty table.
    HistorySchemaModule
        .ensure_schema(db)
        .await
        .expect("history schema");

    // Drive the op through the Agent-origin facade the MCP server uses.
    let service = HolonService::new_with_origin(
        ctx.engine().clone(),
        OpOrigin::Agent {
            session_id: "sess-mcp".to_string(),
            tool_call_id: "call-1".to_string(),
        },
    );
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:a".to_string()));
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert("value".into(), Value::String("doing".to_string()));
    service
        .execute_operation(&EntityName::new("block"), "set_field", params)
        .await
        .expect("dispatch set_field");

    // The tool path: parse args → HistoryQuery → query_history.
    let args = HistoryQueryArgs {
        block_id: Some("block:a".to_string()),
        ..Default::default()
    };
    let events = service
        .query_history(&args.into_query())
        .await
        .expect("query_history");
    assert_eq!(
        events.len(),
        1,
        "the dispatched op is in history: {events:?}"
    );
    let e = &events[0];
    assert_eq!(e.block_id, "block:a");
    assert_eq!(e.field.as_deref(), Some("content"));
    assert_eq!(e.new_value.as_deref(), Some("doing"));
    assert_eq!(e.origin, "agent", "provenance carried the agent origin");
    assert_eq!(e.session_id.as_deref(), Some("sess-mcp"));
    assert_eq!(e.tool_call_id.as_deref(), Some("call-1"));

    // count=true path (the tool's count option).
    let count = service
        .count_history(&HistoryQuery::for_block("block:a"))
        .await
        .expect("count_history");
    assert_eq!(count, 1);
}
