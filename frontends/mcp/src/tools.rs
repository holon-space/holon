use crate::server::HolonMcpServer;
use crate::types::*;
use holon::api::backend_engine::{BackendEngine, QueryContext};
use holon::api::repository::CoreOperations;
use holon::api::types::Traversal;
use holon::api::LoroBackend;
use holon::storage::types::StorageEntity;
use holon_api::{Block, Change, EntityUri, QueryLanguage, Value};
use holon_orgmode::org_renderer::OrgRenderer;
use rmcp::{handler::server::wrapper::Parameters, model::*, tool, tool_router};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use uuid::Uuid;

/// Build a QueryContext from explicit context_id/context_parent_id values.
/// Also looks up the block path so `from descendants` works correctly.
async fn build_context(
    engine: &BackendEngine,
    context_id: Option<&str>,
    context_parent_id: Option<&str>,
) -> Option<QueryContext> {
    match context_id {
        Some(id) => {
            let path = engine
                .blocks()
                .lookup_block_path(id)
                .await
                .unwrap_or_else(|_| format!("/{}", id));
            Some(QueryContext::for_block_with_path(
                EntityUri::from_raw(id),
                context_parent_id.map(|s| EntityUri::from_raw(s)),
                path,
            ))
        }
        None => None,
    }
}

/// Extract context_id/context_parent_id from a generic params map and build QueryContext.
async fn extract_context_from_params(
    engine: &BackendEngine,
    params: &HashMap<String, serde_json::Value>,
) -> Option<QueryContext> {
    let context_id = params.get("context_id").and_then(|v| v.as_str());
    let context_parent_id = params.get("context_parent_id").and_then(|v| v.as_str());
    build_context(engine, context_id, context_parent_id).await
}

// Helper function to convert serde_json::Value to holon_api::Value
fn json_to_holon_value(v: serde_json::Value) -> Value {
    Value::from_json_value(v)
}

// Helper function to convert holon_api::Value to serde_json::Value
fn holon_to_json_value(v: &Value) -> serde_json::Value {
    match v {
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Integer(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Value::Number(
            serde_json::Number::from_f64(*f).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::DateTime(s) => serde_json::Value::String(s.clone()),
        Value::Json(s) => serde_json::from_str(s).unwrap_or(serde_json::Value::String(s.clone())),
        Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(holon_to_json_value).collect())
        }
        Value::Object(obj) => {
            let mut map = serde_json::Map::new();
            for (k, v) in obj {
                map.insert(k.clone(), holon_to_json_value(v));
            }
            serde_json::Value::Object(map)
        }
        Value::Null => serde_json::Value::Null,
    }
}

// Helper function to convert HashMap<String, serde_json::Value> to StorageEntity
fn json_map_to_storage_entity(map: HashMap<String, serde_json::Value>) -> StorageEntity {
    map.into_iter()
        .map(|(k, v)| (k, json_to_holon_value(v)))
        .collect()
}

// Helper function to expose tool_router
pub(crate) fn get_tool_router() -> rmcp::handler::server::router::tool::ToolRouter<HolonMcpServer> {
    HolonMcpServer::tool_router()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_to_holon_string() {
        let v = json_to_holon_value(serde_json::json!("hello"));
        assert_eq!(v, Value::String("hello".into()));
    }

    #[test]
    fn json_to_holon_integer() {
        let v = json_to_holon_value(serde_json::json!(42));
        assert_eq!(v, Value::Integer(42));
    }

    #[test]
    fn json_to_holon_float() {
        let v = json_to_holon_value(serde_json::json!(3.14));
        assert_eq!(v, Value::Float(3.14));
    }

    #[test]
    fn json_to_holon_bool() {
        let v = json_to_holon_value(serde_json::json!(true));
        assert_eq!(v, Value::Boolean(true));
    }

    #[test]
    fn json_to_holon_null() {
        let v = json_to_holon_value(serde_json::json!(null));
        assert_eq!(v, Value::Null);
    }

    #[test]
    fn json_to_holon_array() {
        let v = json_to_holon_value(serde_json::json!([1, "two"]));
        match v {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 2);
                assert_eq!(arr[0], Value::Integer(1));
                assert_eq!(arr[1], Value::String("two".into()));
            }
            _ => panic!("expected Array"),
        }
    }

    #[test]
    fn json_to_holon_object() {
        let v = json_to_holon_value(serde_json::json!({"key": "value"}));
        match v {
            Value::Object(map) => {
                assert_eq!(map.get("key").unwrap(), &Value::String("value".into()));
            }
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn holon_to_json_string() {
        let v = holon_to_json_value(&Value::String("hello".into()));
        assert_eq!(v, serde_json::json!("hello"));
    }

    #[test]
    fn holon_to_json_integer() {
        let v = holon_to_json_value(&Value::Integer(42));
        assert_eq!(v, serde_json::json!(42));
    }

    #[test]
    fn holon_to_json_float() {
        let v = holon_to_json_value(&Value::Float(3.14));
        assert_eq!(v, serde_json::json!(3.14));
    }

    #[test]
    fn holon_to_json_bool() {
        let v = holon_to_json_value(&Value::Boolean(false));
        assert_eq!(v, serde_json::json!(false));
    }

    #[test]
    fn holon_to_json_null() {
        let v = holon_to_json_value(&Value::Null);
        assert_eq!(v, serde_json::Value::Null);
    }

    #[test]
    fn holon_to_json_datetime() {
        let v = holon_to_json_value(&Value::DateTime("2024-01-01T00:00:00Z".into()));
        assert_eq!(v, serde_json::json!("2024-01-01T00:00:00Z"));
    }

    #[test]
    fn holon_to_json_valid_json_string_is_parsed() {
        let v = holon_to_json_value(&Value::Json(r#"{"nested": true}"#.into()));
        assert_eq!(v, serde_json::json!({"nested": true}));
    }

    #[test]
    fn holon_to_json_invalid_json_falls_back_to_string() {
        let v = holon_to_json_value(&Value::Json("not json".into()));
        assert_eq!(v, serde_json::json!("not json"));
    }

    #[test]
    fn holon_to_json_array() {
        let v = holon_to_json_value(&Value::Array(vec![
            Value::Integer(1),
            Value::String("two".into()),
        ]));
        assert_eq!(v, serde_json::json!([1, "two"]));
    }

    #[test]
    fn holon_to_json_object() {
        let mut map = HashMap::new();
        map.insert("k".into(), Value::Boolean(true));
        let v = holon_to_json_value(&Value::Object(map));
        assert_eq!(v, serde_json::json!({"k": true}));
    }

    #[test]
    fn roundtrip_json_to_holon_to_json() {
        let original = serde_json::json!({
            "name": "test",
            "count": 42,
            "active": true,
            "tags": ["a", "b"],
            "meta": null
        });
        let holon = json_to_holon_value(original.clone());
        let back = holon_to_json_value(&holon);
        assert_eq!(original, back);
    }

    #[test]
    fn json_map_to_storage_entity_converts_all_fields() {
        let mut map = HashMap::new();
        map.insert("id".into(), serde_json::json!("block-1"));
        map.insert("priority".into(), serde_json::json!(3));
        let entity = json_map_to_storage_entity(map);
        assert_eq!(entity.get("id").unwrap(), &Value::String("block-1".into()));
        assert_eq!(entity.get("priority").unwrap(), &Value::Integer(3));
    }
}

#[tool_router]
impl HolonMcpServer {
    #[tool(description = "Create a table with specified schema")]
    async fn create_table(
        &self,
        Parameters(params): Parameters<CreateTableParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let mut sql_parts = vec![
            "CREATE TABLE IF NOT EXISTS".to_string(),
            params.table_name.clone(),
            "(".to_string(),
        ];
        let mut column_defs = Vec::new();

        for col in &params.columns {
            let mut col_def = format!("{} {}", col.name, col.sql_type);
            if col.primary_key {
                col_def.push_str(" PRIMARY KEY");
            }
            if let Some(ref default) = col.default {
                col_def.push_str(&format!(" DEFAULT {}", default));
            }
            column_defs.push(col_def);
        }

        sql_parts.push(column_defs.join(", "));
        sql_parts.push(")".to_string());
        let sql = sql_parts.join(" ");

        self.engine
            .execute_query(sql.clone(), HashMap::new(), None)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to create table '{}': {}", params.table_name, e),
                    Some(serde_json::json!({"sql": sql})),
                )
            })?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Table '{}' created successfully",
            params.table_name
        ))]))
    }

    #[tool(description = "Insert rows into a table")]
    async fn insert_data(
        &self,
        Parameters(params): Parameters<InsertDataParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if params.rows.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "0 rows inserted".to_string(),
            )]));
        }

        // Get column names from first row
        let columns: Vec<String> = params.rows[0].keys().cloned().collect();
        let placeholders: Vec<String> = (0..columns.len()).map(|i| format!("${}", i + 1)).collect();

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            params.table_name,
            columns.join(", "),
            placeholders.join(", ")
        );

        let mut row_count = 0;
        for row in &params.rows {
            let mut values = HashMap::new();
            for (i, col) in columns.iter().enumerate() {
                if let Some(val) = row.get(col) {
                    values.insert(format!("{}", i + 1), json_to_holon_value(val.clone()));
                }
            }

            self.engine
                .execute_query(sql.clone(), values, None)
                .await
                .map_err(|e| {
                    rmcp::ErrorData::internal_error(
                        format!("Failed to insert into '{}': {}", params.table_name, e),
                        Some(serde_json::json!({"sql": sql, "row_index": row_count})),
                    )
                })?;
            row_count += 1;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} rows inserted",
            row_count
        ))]))
    }

    #[tool(description = "Drop a table")]
    async fn drop_table(
        &self,
        Parameters(params): Parameters<DropTableParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let sql = format!("DROP TABLE IF EXISTS {}", params.table_name);

        self.engine
            .execute_query(sql.clone(), HashMap::new(), None)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to drop table '{}': {}", params.table_name, e),
                    Some(serde_json::json!({"sql": sql})),
                )
            })?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Table '{}' dropped successfully",
            params.table_name
        ))]))
    }

    #[tool(
        description = "Execute a query in PRQL, GQL, or SQL and return results. Set language to 'prql', 'gql', or 'sql'. This uses a very similar mechanism as the UI does and adds information about widget specs, operations and profiles. Use this if you need to debug backend -> UI interaction."
    )]
    async fn execute_query(
        &self,
        Parameters(params): Parameters<ExecuteQueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let context = build_context(
            &self.engine,
            params.context_id.as_deref(),
            params.context_parent_id.as_deref(),
        )
        .await;

        let mut holon_params = HashMap::new();
        for (k, v) in &params.params {
            holon_params.insert(k.clone(), json_to_holon_value(v.clone()));
        }

        let t0 = Instant::now();

        let sql = self
            .engine
            .compile_to_sql(
                &params.query,
                params.language.parse::<QueryLanguage>().map_err(|e| {
                    rmcp::ErrorData::invalid_params(format!("Invalid language: {e}"), None)
                })?,
            )
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Query compilation failed: {}", e),
                    Some(serde_json::json!({"query": params.query, "language": params.language})),
                )
            })?;

        let rows = self
            .engine
            .execute_query(sql.clone(), holon_params, context)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Query execution failed: {}", e),
                    Some(serde_json::json!({"query": params.query, "language": params.language, "sql": sql})),
                )
            })?;

        let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let include_profile = params.include_profile.unwrap_or(false);

        let json_rows: Vec<HashMap<String, serde_json::Value>> = rows
            .iter()
            .map(|row| {
                let mut json_row: HashMap<String, serde_json::Value> = row
                    .iter()
                    .map(|(k, v)| (k.clone(), holon_to_json_value(v)))
                    .collect();

                if include_profile {
                    let profile = self
                        .engine
                        .profile_resolver()
                        .resolve(row, &holon::entity_profile::ProfileContext::default());
                    json_row.insert(
                        "_profile".to_string(),
                        serde_json::json!({
                            "name": profile.name,
                            "render": format!("{:?}", profile.render),
                            "operations": profile.operations.iter()
                                .map(|op| format!("{}.{}", op.entity_name, op.name))
                                .collect::<Vec<_>>(),
                        }),
                    );
                }

                json_row
            })
            .collect();

        let result = QueryResult {
            rows: json_rows.clone(),
            row_count: json_rows.len(),
            duration_ms: Some(duration_ms),
        };

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "Start watching a query for CDC changes. Supports prql, gql, and sql languages."
    )]
    async fn watch_query(
        &self,
        Parameters(params): Parameters<WatchQueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let context = extract_context_from_params(&self.engine, &params.params).await;

        let mut holon_params = HashMap::new();
        for (k, v) in &params.params {
            holon_params.insert(k.clone(), json_to_holon_value(v.clone()));
        }

        let sql = self
            .engine
            .compile_to_sql(
                &params.query,
                params.language.parse::<QueryLanguage>().map_err(|e| {
                    rmcp::ErrorData::invalid_params(format!("Invalid language: {e}"), None)
                })?,
            )
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Query compilation failed: {}", e),
                    Some(serde_json::json!({"query": params.query, "language": params.language})),
                )
            })?;

        let (widget_spec, stream) = self
            .engine
            .query_and_watch(sql, holon_params, context)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Watch query failed: {}", e),
                    Some(serde_json::json!({"query": params.query, "language": params.language})),
                )
            })?;

        // Convert initial data to JSON
        let json_initial_data: Vec<HashMap<String, serde_json::Value>> = widget_spec
            .data
            .iter()
            .map(|row| {
                row.data
                    .iter()
                    .map(|(k, v): (&String, &holon_api::Value)| (k.clone(), holon_to_json_value(v)))
                    .collect()
            })
            .collect();

        // Generate watch ID
        let watch_id = Uuid::new_v4().to_string();

        // Create pending changes buffer
        let pending_changes = Arc::new(Mutex::new(Vec::<RowChangeJson>::new()));
        let pending_changes_clone = pending_changes.clone();

        // Spawn background task to collect changes
        let task_handle = tokio::spawn(async move {
            let mut stream = stream;
            while let Some(batch) = stream.next().await {
                let mut changes = pending_changes_clone.lock().await;
                for row_change in batch.inner.items {
                    let change: &holon_api::Change<HashMap<String, holon_api::Value>> =
                        &row_change.change;
                    let change_json = RowChangeJson {
                        change_type: match change {
                            Change::Created { .. } => "Created".to_string(),
                            Change::Updated { .. } => "Updated".to_string(),
                            Change::Deleted { .. } => "Deleted".to_string(),
                            Change::FieldsChanged { .. } => "Updated".to_string(),
                        },
                        entity_id: match change {
                            Change::Created { data, .. } => data
                                .get("id")
                                .and_then(|v: &holon_api::Value| v.as_string_owned()),
                            Change::Updated { id, .. } => Some(id.clone()),
                            Change::Deleted { id, .. } => Some(id.clone()),
                            Change::FieldsChanged { entity_id, .. } => Some(entity_id.clone()),
                        },
                        data: match change {
                            Change::Created { data, .. } => Some(
                                data.iter()
                                    .map(|(k, v): (&String, &holon_api::Value)| {
                                        (k.clone(), holon_to_json_value(v))
                                    })
                                    .collect(),
                            ),
                            Change::Updated { data, .. } => Some(
                                data.iter()
                                    .map(|(k, v): (&String, &holon_api::Value)| {
                                        (k.clone(), holon_to_json_value(v))
                                    })
                                    .collect(),
                            ),
                            Change::Deleted { .. } => None,
                            Change::FieldsChanged { fields, .. } => {
                                // Convert fields vec to a map
                                let mut map = HashMap::new();
                                for (field_name, _old_val, new_val) in fields {
                                    map.insert(field_name.clone(), holon_to_json_value(&new_val));
                                }
                                Some(map)
                            }
                        },
                    };
                    changes.push(change_json);
                }
            }
        });

        // Store watch state
        let mut watches = self.watches.lock().await;
        watches.insert(
            watch_id.clone(),
            crate::server::WatchState {
                pending_changes,
                _task_handle: task_handle,
            },
        );

        let handle = WatchHandle {
            watch_id: watch_id.clone(),
            initial_data: json_initial_data,
        };

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&handle).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(description = "Poll for accumulated CDC changes")]
    async fn poll_changes(
        &self,
        Parameters(params): Parameters<PollChangesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let mut watches = self.watches.lock().await;

        let watch_state = watches.get_mut(&params.watch_id).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "watch_not_found",
                Some(serde_json::json!({"watch_id": params.watch_id})),
            )
        })?;

        let mut changes = watch_state.pending_changes.lock().await;
        let result = changes.drain(..).collect::<Vec<_>>();

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(description = "Stop watching a query")]
    async fn stop_watch(
        &self,
        Parameters(params): Parameters<StopWatchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let mut watches = self.watches.lock().await;

        watches.remove(&params.watch_id).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "watch_not_found",
                Some(serde_json::json!({"watch_id": params.watch_id})),
            )
        })?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Watch '{}' stopped successfully",
            params.watch_id
        ))]))
    }

    #[tool(
        description = "Execute an operation on an entity. Use list_operations first to discover available operations and their required parameters"
    )]
    async fn execute_operation(
        &self,
        Parameters(params): Parameters<ExecuteOperationParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Convert params to StorageEntity
        let storage_entity = json_map_to_storage_entity(params.params);

        // Execute operation
        let response = self.engine.execute_operation(&params.entity_name, &params.operation, storage_entity)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(
                format!("Operation '{}' on '{}' failed: {}", params.operation, params.entity_name, e),
                Some(serde_json::json!({"entity": params.entity_name, "operation": params.operation}))
            ))?;

        let content = match response {
            Some(value) => Content::text(value.to_json_string()),
            None => Content::text(format!(
                "Operation '{}' on entity '{}' executed successfully",
                params.operation, params.entity_name
            )),
        };

        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "List available operations for an entity. Returns operation names, required parameters, and descriptions. Common entities: blocks, directories, documents"
    )]
    async fn list_operations(
        &self,
        Parameters(params): Parameters<ListOperationsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let ops = self.engine.available_operations(&params.entity_name).await;

        // Convert OperationDescriptor to JSON
        let json_ops: Vec<serde_json::Value> = ops
            .iter()
            .map(|op| {
                serde_json::json!({
                    "entity_name": op.entity_name,
                    "entity_short_name": op.entity_short_name,
                    "id_column": op.id_column,
                    "name": op.name,
                    "display_name": op.display_name,
                    "description": op.description,
                    "required_params": op.required_params.iter().map(|p| serde_json::json!({
                        "name": p.name,
                        "type_hint": format!("{:?}", p.type_hint),
                        "description": p.description,
                    })).collect::<Vec<_>>(),
                    "affected_fields": op.affected_fields,
                })
            })
            .collect();

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&json_ops).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(description = "Undo the last operation")]
    async fn undo(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let result = self.engine.undo().await;

        match result {
            Ok(success) => {
                let undo_result = UndoRedoResult {
                    success,
                    message: if success {
                        "Operation undone successfully".to_string()
                    } else {
                        "Nothing to undo".to_string()
                    },
                };
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&undo_result).map_err(|e| {
                        rmcp::ErrorData::internal_error(
                            "serialization_failed",
                            Some(serde_json::json!({"error": e.to_string()})),
                        )
                    })?,
                )]))
            }
            Err(e) => Err(rmcp::ErrorData::internal_error(
                format!("Undo failed: {}", e),
                None,
            )),
        }
    }

    #[tool(description = "Redo the last undone operation")]
    async fn redo(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let result = self.engine.redo().await;

        match result {
            Ok(success) => {
                let redo_result = UndoRedoResult {
                    success,
                    message: if success {
                        "Operation redone successfully".to_string()
                    } else {
                        "Nothing to redo".to_string()
                    },
                };
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&redo_result).map_err(|e| {
                        rmcp::ErrorData::internal_error(
                            "serialization_failed",
                            Some(serde_json::json!({"error": e.to_string()})),
                        )
                    })?,
                )]))
            }
            Err(e) => Err(rmcp::ErrorData::internal_error(
                format!("Redo failed: {}", e),
                None,
            )),
        }
    }

    #[tool(description = "Check if undo is available")]
    async fn can_undo(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let available = self.engine.can_undo().await;
        let result = CanUndoRedoResult { available };

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(description = "Check if redo is available")]
    async fn can_redo(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let available = self.engine.can_redo().await;
        let result = CanUndoRedoResult { available };

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "Rank active tasks using WSJF (Weighted Shortest Job First). Returns tasks ordered by value-per-minute: highest priority and shortest duration tasks rank first. Uses a Petri Net model where task dependencies (depends_on property) block dependent tasks until prerequisites are complete."
    )]
    async fn rank_tasks(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let rank_result = self.engine.blocks().rank_tasks().await.map_err(|e| {
            rmcp::ErrorData::internal_error(
                "rank_tasks_failed",
                Some(serde_json::json!({"error": e.to_string()})),
            )
        })?;

        let tasks: Vec<RankedTaskJson> = rank_result
            .ranked
            .into_iter()
            .enumerate()
            .map(|(i, rt)| RankedTaskJson {
                rank: i + 1,
                block_id: rt.block_id,
                label: rt.label,
                delta_obj: rt.delta_obj,
                delta_per_minute: rt.delta_per_minute,
                duration_minutes: rt.duration_minutes,
            })
            .collect();

        let result = RankTasksResult {
            tasks,
            mental_slots: MentalSlotsJson {
                occupied: rank_result.mental_slots.occupied,
                capacity: rank_result.mental_slots.capacity,
            },
        };

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "Execute raw SQL directly against Turso, bypassing all query compilation (PRQL/GQL) and SQL transforms. Use this for Turso-specific queries, pragmas, or when you need to avoid the holon query pipeline."
    )]
    async fn execute_raw_sql(
        &self,
        Parameters(params): Parameters<ExecuteRawSqlParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let mut holon_params = HashMap::new();
        for (k, v) in &params.params {
            holon_params.insert(k.clone(), json_to_holon_value(v.clone()));
        }

        let t0 = Instant::now();

        let rows = self
            .engine
            .db_handle()
            .query(&params.sql, holon_params)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Raw SQL execution failed: {}", e),
                    Some(serde_json::json!({"sql": params.sql})),
                )
            })?;

        let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let json_rows: Vec<HashMap<String, serde_json::Value>> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(k, v)| (k.clone(), holon_to_json_value(v)))
                    .collect()
            })
            .collect();

        let result = QueryResult {
            rows: json_rows.clone(),
            row_count: json_rows.len(),
            duration_ms: Some(duration_ms),
        };

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    // --- Debug / inspection tools ---

    #[tool(
        description = "Compile a PRQL/GQL/SQL query to final SQL without executing. Shows what the query engine actually runs."
    )]
    async fn compile_query(
        &self,
        Parameters(params): Parameters<CompileQueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let compiled_sql = self
            .engine
            .compile_to_sql(
                &params.query,
                params.language.parse::<QueryLanguage>().map_err(|e| {
                    rmcp::ErrorData::invalid_params(format!("Invalid language: {e}"), None)
                })?,
            )
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Query compilation failed: {}", e),
                    Some(serde_json::json!({"query": params.query, "language": params.language})),
                )
            })?;

        let result = CompileQueryResult {
            compiled_sql,
            render_spec: None,
        };

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "List all loaded Loro documents with their file paths and UUID→path alias mappings. Requires Loro to be enabled."
    )]
    async fn list_loro_documents(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let store = self.debug.loro_doc_store.as_ref().ok_or_else(|| {
            rmcp::ErrorData::internal_error("Loro is not enabled in this session", None)
        })?;

        let store_read = store.read().await;
        let docs = store_read.iter().await;
        let aliases = store_read.get_all_aliases().await;

        let doc_list: Vec<serde_json::Value> = docs
            .iter()
            .map(|(path, doc)| {
                serde_json::json!({
                    "file_path": path.to_string(),
                    "doc_id": doc.doc_id(),
                })
            })
            .collect();

        let alias_list: Vec<serde_json::Value> = aliases
            .iter()
            .map(|(uuid, path)| {
                serde_json::json!({
                    "alias": uuid,
                    "file_path": path.display().to_string(),
                })
            })
            .collect();

        let result = serde_json::json!({
            "documents": doc_list,
            "aliases": alias_list,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "Get blocks directly from a Loro CRDT document (bypassing SQL). Takes doc_id which can be a UUID or file path. Returns all blocks as JSON."
    )]
    async fn inspect_loro_blocks(
        &self,
        Parameters(params): Parameters<InspectLoroBlocksParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let blocks = self.get_loro_blocks(&params.doc_id).await?;

        let json_blocks: Vec<serde_json::Value> = blocks
            .iter()
            .map(|b| serde_json::to_value(b).unwrap_or_default())
            .collect();

        let result = serde_json::json!({
            "doc_id": params.doc_id,
            "block_count": json_blocks.len(),
            "blocks": json_blocks,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "Compare blocks in Loro CRDT vs blocks in SQL for a document. Shows mismatches: only-in-loro, only-in-sql, and field differences."
    )]
    async fn diff_loro_sql(
        &self,
        Parameters(params): Parameters<DiffLoroSqlParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let loro_blocks = self.get_loro_blocks(&params.doc_id).await?;

        // Build a map of Loro blocks by ID
        let loro_map: HashMap<String, &Block> =
            loro_blocks.iter().map(|b| (b.id.to_string(), b)).collect();

        // Get the document URI for SQL query
        let doc_uri = self.resolve_doc_uri(&params.doc_id).await?;

        // Query SQL for blocks belonging to this document
        let sql = "SELECT * FROM block WHERE parent_id LIKE $doc_uri_prefix";
        let mut query_params = HashMap::new();
        query_params.insert(
            "doc_uri_prefix".to_string(),
            Value::String(format!("{}%", doc_uri)),
        );
        let sql_rows = self
            .engine
            .execute_query(sql.to_string(), query_params, None)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("SQL query failed: {}", e),
                    Some(serde_json::json!({"sql": sql})),
                )
            })?;

        // Build SQL block map by ID
        let mut sql_map: HashMap<String, &HashMap<String, Value>> = HashMap::new();
        for row in &sql_rows {
            if let Some(Value::String(id)) = row.get("id") {
                sql_map.insert(id.clone(), row);
            }
        }

        // Compare
        let mut only_in_loro = Vec::new();
        let mut only_in_sql = Vec::new();
        let mut mismatches = Vec::new();

        for (id, loro_block) in &loro_map {
            if let Some(sql_row) = sql_map.get(id) {
                let mut diffs = Vec::new();
                // Compare key fields
                for field in &["content", "parent_id", "content_type", "task_state"] {
                    let loro_val = match *field {
                        "content" => loro_block.content.clone(),
                        "parent_id" => loro_block.parent_id.to_string(),
                        "content_type" => loro_block.content_type.to_string(),
                        _ => continue,
                    };
                    if let Some(sql_val) = sql_row.get(*field) {
                        let sql_str = match sql_val {
                            Value::String(s) => s.clone(),
                            other => format!("{:?}", other),
                        };
                        if loro_val != sql_str {
                            diffs.push(serde_json::json!({
                                "field": field,
                                "loro": loro_val,
                                "sql": sql_str,
                            }));
                        }
                    }
                }
                if !diffs.is_empty() {
                    mismatches.push(serde_json::json!({
                        "block_id": id,
                        "diffs": diffs,
                    }));
                }
            } else {
                only_in_loro.push(serde_json::json!({
                    "block_id": id,
                    "content": loro_block.content,
                    "parent_id": loro_block.parent_id,
                }));
            }
        }

        for id in sql_map.keys() {
            if !loro_map.contains_key(id) {
                let row = &sql_map[id];
                only_in_sql.push(serde_json::json!({
                    "block_id": id,
                    "content": row.get("content").map(|v| format!("{:?}", v)),
                    "parent_id": row.get("parent_id").map(|v| format!("{:?}", v)),
                }));
            }
        }

        let result = serde_json::json!({
            "doc_id": params.doc_id,
            "loro_block_count": loro_blocks.len(),
            "sql_block_count": sql_rows.len(),
            "only_in_loro": only_in_loro,
            "only_in_sql": only_in_sql,
            "field_mismatches": mismatches,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "Read raw org file content from disk for a document. Resolves doc_id (UUID or file path) to a file path via aliases."
    )]
    async fn read_org_file(
        &self,
        Parameters(params): Parameters<ReadOrgFileParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let file_path = self.resolve_to_file_path(&params.doc_id).await?;

        let content = tokio::fs::read_to_string(&file_path).await.map_err(|e| {
            rmcp::ErrorData::internal_error(
                format!("Failed to read file '{}': {}", file_path.display(), e),
                None,
            )
        })?;

        let result = serde_json::json!({
            "doc_id": params.doc_id,
            "file_path": file_path.display().to_string(),
            "content": content,
            "byte_length": content.len(),
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "Render org text from current Loro block state (what OrgRenderer would write to disk). Compare with read_org_file to spot sync mismatches."
    )]
    async fn render_org_from_blocks(
        &self,
        Parameters(params): Parameters<RenderOrgParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let blocks = self.get_loro_blocks(&params.doc_id).await?;
        let file_path = self
            .resolve_to_file_path(&params.doc_id)
            .await
            .unwrap_or_else(|_| std::path::PathBuf::from("unknown.org"));

        let rendered = OrgRenderer::render_blocks(&blocks, &file_path, &params.doc_id);

        let result = serde_json::json!({
            "doc_id": params.doc_id,
            "file_path": file_path.display().to_string(),
            "rendered_org": rendered,
            "block_count": blocks.len(),
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }

    #[tool(
        description = "Render a block's UI as a structural tree. Returns what an LLM agent would 'see': widget hierarchy, entity IDs, labels, and nesting. Use format 'text' for readable output or 'json' for structured data."
    )]
    async fn describe_ui(
        &self,
        Parameters(params): Parameters<DescribeUiParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (widget_spec, _stream) = self
            .engine
            .blocks()
            .render_block(&params.block_id, None, false)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to render block '{}': {}", params.block_id, e),
                    None,
                )
            })?;

        let engine = self.engine.clone();
        let render_expr = widget_spec.render_expr.clone();
        let data_rows: Vec<HashMap<String, Value>> =
            widget_spec.data.iter().map(|r| r.data.clone()).collect();

        // Run shadow interpretation on a blocking thread because nested
        // block_ref builders use `handle.block_on()` which panics inside tokio.
        let display_tree = tokio::task::spawn_blocking(move || {
            let ctx = holon_frontend::RenderContext::headless(engine);
            let ctx = ctx.with_data_rows(data_rows);
            let shadow = holon_frontend::create_shadow_interpreter();
            shadow.interpret(&render_expr, &ctx)
        })
        .await
        .map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Shadow interpretation panicked: {e}"), None)
        })?;

        let output = match params.format.as_str() {
            "json" => serde_json::to_string_pretty(&display_tree).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
            _ => display_tree.pretty_print(0),
        };

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        description = "List all tables, views and materialized views in the database. Returns name, type, and SQL definition (for views/matviews)."
    )]
    async fn list_tables(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let sql = r#"
            SELECT
                name,
                type,
                sql AS definition
            FROM sqlite_master
            WHERE type IN ('table', 'view')
              AND name NOT LIKE 'sqlite_%'
              AND name NOT LIKE '_litestream_%'
            ORDER BY type, name
        "#;

        let rows = self
            .engine
            .db_handle()
            .query(sql, HashMap::new())
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Failed to list tables: {}", e), None)
            })?;

        let mut tables = Vec::new();
        let mut views = Vec::new();

        for row in &rows {
            let name = match row.get("name") {
                Some(Value::String(s)) => s.clone(),
                _ => continue,
            };
            let obj_type = match row.get("type") {
                Some(Value::String(s)) => s.clone(),
                _ => continue,
            };
            let definition = match row.get("definition") {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            };

            let entry = serde_json::json!({
                "name": name,
                "definition": definition,
            });

            match obj_type.as_str() {
                "table" => tables.push(entry),
                "view" => views.push(entry),
                _ => {}
            }
        }

        // Turso materialized views live in a separate pragma
        let matview_rows = self
            .engine
            .db_handle()
            .query("PRAGMA materialized_views", HashMap::new())
            .await
            .unwrap_or_default();

        let mut matviews: Vec<serde_json::Value> = Vec::new();
        for row in &matview_rows {
            let name = match row.get("name") {
                Some(Value::String(s)) => s.clone(),
                _ => continue,
            };
            let definition = match row.get("sql") {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            };
            matviews.push(serde_json::json!({
                "name": name,
                "definition": definition,
            }));
        }

        let result = serde_json::json!({
            "tables": tables,
            "views": views,
            "materialized_views": matviews,
            "summary": {
                "table_count": tables.len(),
                "view_count": views.len(),
                "matview_count": matviews.len(),
            }
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?,
        )]))
    }
}

// --- Helper methods for debug tools ---
impl HolonMcpServer {
    /// Resolve a doc_id (UUID or file path) to blocks from Loro.
    async fn get_loro_blocks(&self, doc_id: &str) -> Result<Vec<Block>, rmcp::ErrorData> {
        let store = self.debug.loro_doc_store.as_ref().ok_or_else(|| {
            rmcp::ErrorData::internal_error("Loro is not enabled in this session", None)
        })?;

        let store_read = store.read().await;
        let mut loro_doc = store_read.resolve_by_doc_id(doc_id).await;
        if loro_doc.is_none() {
            // Try as file path
            loro_doc = store_read.get(std::path::Path::new(doc_id)).await;
        }
        let loro_doc = loro_doc.ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                format!("Document '{}' not found in Loro store", doc_id),
                None,
            )
        })?;

        let backend = LoroBackend::from_document(loro_doc);
        backend.get_all_blocks(Traversal::ALL).await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Failed to read blocks from Loro: {}", e), None)
        })
    }

    /// Resolve a doc_id to its document URI (doc: prefix).
    async fn resolve_doc_uri(&self, doc_id: &str) -> Result<String, rmcp::ErrorData> {
        let uri = holon_api::EntityUri::from_raw(doc_id);
        if uri.is_doc() {
            return Ok(uri.to_string());
        }
        Ok(holon_api::EntityUri::doc(doc_id).to_string())
    }

    /// Resolve a doc_id (UUID or path) to a file path on disk.
    async fn resolve_to_file_path(
        &self,
        doc_id: &str,
    ) -> Result<std::path::PathBuf, rmcp::ErrorData> {
        // If it looks like a file path already, use it directly
        if doc_id.contains('/') || doc_id.ends_with(".org") {
            let path = std::path::PathBuf::from(doc_id);
            if path.exists() {
                return Ok(path);
            }
            // Try under orgmode_root
            if let Some(ref root) = self.debug.orgmode_root {
                let full = root.join(doc_id);
                if full.exists() {
                    return Ok(full);
                }
            }
        }

        // Try to resolve via Loro aliases
        if let Some(ref store) = self.debug.loro_doc_store {
            let store_read = store.read().await;
            if let Some(path) = store_read.resolve_alias_to_path(doc_id).await {
                return Ok(path);
            }
        }

        Err(rmcp::ErrorData::invalid_params(
            format!(
                "Cannot resolve '{}' to a file path. Provide a UUID with registered alias or a file path.",
                doc_id
            ),
            None,
        ))
    }
}
