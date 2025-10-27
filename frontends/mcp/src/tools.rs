use crate::server::HolonMcpServer;
use crate::types::*;
use holon::api::holon_service::HolonService;
use holon::api::repository::CoreOperations;
use holon::api::types::Traversal;
use holon::api::LoroBackend;
use holon::storage::types::StorageEntity;
use holon_api::{Block, Change, EntityUri, QueryLanguage, Value};
use holon_orgmode::org_renderer::OrgRenderer;
use rmcp::{handler::server::wrapper::Parameters, model::*, tool, tool_router};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use uuid::Uuid;

/// Extract context_id/context_parent_id from a generic params map and build QueryContext.
async fn extract_context_from_params(
    service: &HolonService,
    params: &HashMap<String, serde_json::Value>,
) -> Option<holon::api::backend_engine::QueryContext> {
    let context_id = params.get("context_id").and_then(|v| v.as_str());
    let context_parent_id = params.get("context_parent_id").and_then(|v| v.as_str());
    service.build_context(context_id, context_parent_id).await
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

fn format_display_tree(
    tree: &holon_frontend::view_model::ViewModel,
    format: &str,
) -> Result<String, rmcp::ErrorData> {
    match format {
        "json" => serde_json::to_string_pretty(tree).map_err(|e| {
            rmcp::ErrorData::internal_error(
                "serialization_failed",
                Some(serde_json::json!({"error": e.to_string()})),
            )
        }),
        _ => Ok(tree.pretty_print(0)),
    }
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

#[tool_router(router = tool_router_backend, vis = "pub(crate)")]
impl HolonMcpServer {
    #[tool(description = "Create a table with specified schema")]
    async fn create_table(
        &self,
        Parameters(params): Parameters<CreateTableParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use holon::api::holon_service::ColumnDef;

        let columns: Vec<ColumnDef> = params
            .columns
            .iter()
            .map(|col| ColumnDef {
                name: col.name.clone(),
                sql_type: col.sql_type.clone(),
                primary_key: col.primary_key,
                default: col.default.clone(),
            })
            .collect();

        self.service()
            .create_table(&params.table_name, &columns)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to create table '{}': {}", params.table_name, e),
                    None,
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
        let holon_rows: Vec<HashMap<String, Value>> = params
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(k, v)| (k.clone(), json_to_holon_value(v.clone())))
                    .collect()
            })
            .collect();

        let count = self
            .service()
            .insert_data(&params.table_name, &holon_rows)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to insert into '{}': {}", params.table_name, e),
                    None,
                )
            })?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} rows inserted",
            count
        ))]))
    }

    #[tool(
        description = "Create a new entity type at runtime. Pass type_definition as a JSON object: {name, fields: [{name, sql_type, primary_key?, nullable?, indexed?}], primary_key?, graph_label?, id_references?}. Creates the extension table, registers in TypeRegistry and GQL graph."
    )]
    async fn create_entity_type(
        &self,
        Parameters(params): Parameters<CreateEntityTypeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let type_def: holon_api::TypeDefinition = serde_json::from_value(params.type_definition)
            .map_err(|e| {
                rmcp::ErrorData::invalid_params(format!("Invalid TypeDefinition: {e}"), None)
            })?;

        let name = type_def.name.clone();

        // Register in TypeRegistry (validates computed field expressions)
        if let Some(ref registry) = self.type_registry {
            registry.register(type_def.clone()).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to register type '{}': {e}", name),
                    None,
                )
            })?;
        }

        // Create extension table via DynamicSchemaModule
        if !type_def.fields.is_empty() {
            use holon::storage::SchemaModule;
            let module =
                holon::storage::dynamic_schema_module::DynamicSchemaModule::new(type_def.clone());
            let db_handle = self.engine().db_handle();
            module.ensure_schema(&db_handle).await.map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to create table for '{}': {e}", name),
                    None,
                )
            })?;
            db_handle
                .mark_available(module.provides())
                .await
                .map_err(|e| {
                    rmcp::ErrorData::internal_error(
                        format!("Failed to mark resources for '{}': {e}", name),
                        None,
                    )
                })?;
        }

        // Register in GQL graph for query support
        self.engine().register_entity_type(type_def);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Entity type '{}' created successfully",
            name
        ))]))
    }

    #[tool(description = "Drop a table")]
    async fn drop_table(
        &self,
        Parameters(params): Parameters<DropTableParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.service()
            .drop_table(&params.table_name)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to drop table '{}': {}", params.table_name, e),
                    None,
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
        let context = self
            .service()
            .build_context(
                params.context_id.as_deref(),
                params.context_parent_id.as_deref(),
            )
            .await;

        let mut holon_params = HashMap::new();
        for (k, v) in &params.params {
            holon_params.insert(k.clone(), json_to_holon_value(v.clone()));
        }

        let language = params
            .language
            .parse::<QueryLanguage>()
            .map_err(|e| rmcp::ErrorData::invalid_params(format!("Invalid language: {e}"), None))?;

        let query_result = self
            .service()
            .execute_query(&params.query, language, holon_params, context)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Query failed: {}", e),
                    Some(serde_json::json!({"query": params.query, "language": params.language})),
                )
            })?;

        let duration_ms = query_result.duration.as_secs_f64() * 1000.0;
        let include_profile = params.include_profile.unwrap_or(false);

        let json_rows: Vec<HashMap<String, serde_json::Value>> = query_result
            .rows
            .iter()
            .map(|row| {
                let mut json_row: HashMap<String, serde_json::Value> = row
                    .iter()
                    .map(|(k, v)| (k.clone(), holon_to_json_value(v)))
                    .collect();

                if include_profile {
                    let profile = self.engine().profile_resolver().resolve(row);
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
        let context = extract_context_from_params(self.service(), &params.params).await;

        let mut holon_params = HashMap::new();
        for (k, v) in &params.params {
            holon_params.insert(k.clone(), json_to_holon_value(v.clone()));
        }

        let language = params
            .language
            .parse::<QueryLanguage>()
            .map_err(|e| rmcp::ErrorData::invalid_params(format!("Invalid language: {e}"), None))?;

        let mut stream = self
            .service()
            .query_and_watch(&params.query, language, holon_params, context)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Watch query failed: {}", e),
                    Some(serde_json::json!({"query": params.query, "language": params.language})),
                )
            })?;

        // Collect initial data from the first batch (Change::Created items)
        let mut initial_rows: Vec<HashMap<String, holon_api::Value>> = Vec::new();
        if let Some(first_batch) = stream.next().await {
            for row_change in first_batch.inner.items {
                if let holon_api::Change::Created { data, .. } = row_change.change {
                    initial_rows.push(data);
                }
            }
        }

        let json_initial_data: Vec<HashMap<String, serde_json::Value>> = initial_rows
            .iter()
            .map(|row| {
                row.iter()
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
        let storage_entity = json_map_to_storage_entity(params.params);

        let response = self
            .service()
            .execute_operation(&params.entity_name, &params.operation, storage_entity)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!(
                        "Operation '{}' on '{}' failed: {}",
                        params.operation, params.entity_name, e
                    ),
                    None,
                )
            })?;

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
        let ops = self
            .service()
            .available_operations(&params.entity_name)
            .await;

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
        let result = self.service().undo().await;

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
        let result = self.service().redo().await;

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
        let available = self.service().can_undo().await;
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
        let available = self.service().can_redo().await;
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
        let rank_result = self.service().rank_tasks().await.map_err(|e| {
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

        let query_result = self
            .service()
            .execute_raw_sql(&params.sql, holon_params)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Raw SQL execution failed: {}", e),
                    Some(serde_json::json!({"sql": params.sql})),
                )
            })?;

        let duration_ms = query_result.duration.as_secs_f64() * 1000.0;

        let json_rows: Vec<HashMap<String, serde_json::Value>> = query_result
            .rows
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
        let language = params
            .language
            .parse::<QueryLanguage>()
            .map_err(|e| rmcp::ErrorData::invalid_params(format!("Invalid language: {e}"), None))?;

        let compiled_sql = self
            .service()
            .compile_query(&params.query, language)
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
        description = "List all tables, views and materialized views in the database. Returns name, type, and SQL definition (for views/matviews)."
    )]
    async fn list_tables(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let listing = self.service().list_tables().await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Failed to list tables: {}", e), None)
        })?;

        let to_json =
            |entries: &[holon::api::holon_service::TableEntry]| -> Vec<serde_json::Value> {
                entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "name": e.name,
                            "definition": e.definition,
                        })
                    })
                    .collect()
            };

        let tables = to_json(&listing.tables);
        let views = to_json(&listing.views);
        let matviews = to_json(&listing.materialized_views);

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

    #[tool(
        description = "List available slash commands (operations) for a block. Returns operation names, display names, and entity names. Use execute_command to run one."
    )]
    async fn list_commands(
        &self,
        Parameters(params): Parameters<ListCommandsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let block_uri = EntityUri::parse(&params.block_id).map_err(|e| {
            rmcp::ErrorData::invalid_params(
                format!("Invalid block_id '{}': {}", params.block_id, e),
                None,
            )
        })?;

        let filter = params.filter.as_deref().unwrap_or("");

        let block_result = self
            .service()
            .execute_raw_sql(
                "SELECT * FROM block WHERE id = $1",
                HashMap::from([("1".to_string(), Value::String(block_uri.to_string()))]),
            )
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to look up block '{}': {}", params.block_id, e),
                    None,
                )
            })?;

        let mut context_params: HashMap<String, Value> = HashMap::new();
        if let Some(row) = block_result.rows.first() {
            for (k, v) in row {
                context_params.insert(k.clone(), v.clone());
            }
        }

        let profile = block_result
            .rows
            .first()
            .map(|row| self.engine().profile_resolver().resolve(row));

        let entity_name = profile
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "blocks".to_string());

        let ops = self.service().available_operations(&entity_name).await;
        let wirings: Vec<holon_api::render_types::OperationWiring> = ops
            .into_iter()
            .map(|d| holon_api::render_types::OperationWiring {
                modified_param: String::new(),
                descriptor: d,
            })
            .collect();

        let commands = holon_frontend::command_provider::CommandProvider::build_command_items(
            &wirings,
            &context_params,
            filter,
        );

        let result: Vec<serde_json::Value> = commands
            .iter()
            .map(|item| {
                serde_json::json!({
                    "name": item.id,
                    "display_name": item.label,
                    "entity_name": entity_name,
                })
            })
            .collect();

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
        description = "Execute a slash command (operation) on a block by name. Use list_commands first to discover available commands."
    )]
    async fn execute_command(
        &self,
        Parameters(params): Parameters<ExecuteCommandParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let mut storage_entity = json_map_to_storage_entity(params.params);
        storage_entity
            .entry("id".to_string())
            .or_insert_with(|| holon_api::Value::String(params.block_id.clone()));

        let response = self
            .service()
            .execute_operation(&params.entity_name, &params.command_name, storage_entity)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!(
                        "Command '{}' on '{}' for block '{}' failed: {}",
                        params.command_name, params.entity_name, params.block_id, e
                    ),
                    None,
                )
            })?;

        let content = match response {
            Some(value) => Content::text(value.to_json_string()),
            None => Content::text(format!(
                "Command '{}' executed successfully on block '{}'",
                params.command_name, params.block_id
            )),
        };

        Ok(CallToolResult::success(vec![content]))
    }
}

#[tool_router(router = tool_router_ui, vis = "pub(crate)")]
impl HolonMcpServer {
    #[tool(
        description = "List all loaded Loro documents with their file paths and UUID→path alias mappings. Requires Loro to be enabled."
    )]
    async fn list_loro_documents(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let store = self.debug.loro_doc_store.get().ok_or_else(|| {
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
            .engine()
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

        let doc_uri =
            EntityUri::parse(&params.doc_id).unwrap_or_else(|_| EntityUri::block(&params.doc_id));
        let rendered = OrgRenderer::render_blocks(&blocks, &file_path, &doc_uri);

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
        let block_uri = EntityUri::parse(&params.block_id).map_err(|e| {
            rmcp::ErrorData::invalid_params(
                format!("Invalid block_id '{}': {}", params.block_id, e),
                None,
            )
        })?;

        let svc = self.builder_services.clone().ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "describe_ui requires a running frontend (builder_services not registered)",
                None,
            )
        })?;

        // Ensure the watcher is running and wait for the first Structure event.
        // get_block_data starts a watcher if needed; await_ready returns
        // immediately if already loaded.
        let block_id = block_uri.clone();
        let svc_ready = svc.clone();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            svc_ready.await_ready(&block_id),
        )
        .await
        .ok(); // ALLOW(ok): timeout non-fatal — render whatever we have

        let display_tree = tokio::task::spawn_blocking(move || svc.snapshot_resolved(&block_id))
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Shadow interpretation panicked: {e}"),
                    None,
                )
            })?;

        let output = format_display_tree(&display_tree, &params.format)?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        description = "Capture a screenshot of a running Holon frontend window. Returns the screenshot as a PNG image. Works with GPUI (window title 'Holon') and Blinc frontends. Optionally specify a window_title to match a specific frontend."
    )]
    #[allow(unused_variables)]
    async fn screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        #[cfg(not(target_os = "macos"))]
        return Err(rmcp::ErrorData::internal_error(
            "Screenshot capture is only available on macOS",
            None,
        ));

        #[cfg(target_os = "macos")]
        {
            // xcap window enumeration is blocking — run on a blocking thread
            let window_title = params.window_title;
            let png_bytes =
                tokio::task::spawn_blocking(move || capture_window_as_png(window_title.as_deref()))
                    .await
                    .map_err(|e| {
                        rmcp::ErrorData::internal_error(
                            format!("Screenshot task panicked: {e}"),
                            None,
                        )
                    })?
                    .map_err(|e| rmcp::ErrorData::internal_error(e, None))?;

            let b64 =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_bytes);

            Ok(CallToolResult::success(vec![Content::image(
                b64,
                "image/png",
            )]))
        } // cfg(target_os = "macos")
    }

    #[tool(
        description = "Inspect the GPUI cross-block navigation state and entity registries. Shows the shadow index tree (widget hierarchy with navigators and entity IDs), registered editor inputs, and all live entity view registries (blocks, editors, live queries, collections, render blocks). Use this to debug navigation issues or understand which GPUI entities are currently alive."
    )]
    async fn describe_navigation(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let state = self.debug.navigation_state.read().unwrap();
        let output = format!(
            "{}\nEditor inputs: {} entries\n{}\n\n── Entity View Registries ──\n{}",
            state.shadow_index_description,
            state.editor_input_ids.len(),
            state
                .editor_input_ids
                .iter()
                .map(|id| format!("  {id}"))
                .collect::<Vec<_>>()
                .join("\n"),
            state.entity_registry_description,
        );
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    // ── UI interaction tools (semantic level) ──────────────────────────

    #[tool(
        description = "Simulate arrow-key navigation between blocks. Reads the shared shadow index to find the next focusable block in the given direction. Returns the target block_id and cursor placement."
    )]
    async fn send_navigation(
        &self,
        Parameters(params): Parameters<SendNavigationParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use holon_frontend::input::WidgetInput;
        use holon_frontend::navigation::{Boundary, CursorHint, NavDirection};

        let direction = match params.direction.to_lowercase().as_str() {
            "up" => NavDirection::Up,
            "down" => NavDirection::Down,
            "left" => NavDirection::Left,
            "right" => NavDirection::Right,
            other => {
                return Err(rmcp::ErrorData::invalid_params(
                    format!(
                        "Invalid direction '{other}', expected 'up', 'down', 'left', or 'right'"
                    ),
                    None,
                ))
            }
        };

        let boundary = match direction {
            NavDirection::Up | NavDirection::Left => Boundary::Top,
            NavDirection::Down | NavDirection::Right => Boundary::Bottom,
        };

        let hint = CursorHint {
            column: params.cursor_column.unwrap_or(0),
            boundary,
        };
        let input = WidgetInput::Navigate { direction, hint };

        let shadow = self.debug.shadow_index.read().unwrap();
        let index = shadow.as_ref().ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "Shadow index not yet built (no GPUI render has occurred)",
                None,
            )
        })?;

        match index.bubble_input(&params.from_entity_id, &input) {
            Some(holon_frontend::input::InputAction::Focus {
                block_id,
                placement,
            }) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "target_block_id": block_id,
                    "placement": format!("{:?}", placement),
                })
                .to_string(),
            )])),
            Some(other) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "action": format!("{:?}", other),
                })
                .to_string(),
            )])),
            None => Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "result": "at_boundary",
                    "detail": "No navigation target found (cursor is at the edge)"
                })
                .to_string(),
            )])),
        }
    }

    #[tool(
        description = "Simulate a keyboard shortcut (key chord) at a specific entity. The chord bubbles up through the shadow index tree, matching against bound operations. If a match is found, the operation is executed."
    )]
    async fn send_key_chord(
        &self,
        Parameters(params): Parameters<SendKeyChordParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use holon_frontend::input::WidgetInput;

        let keys: std::collections::BTreeSet<holon_frontend::input::Key> = params
            .keys
            .iter()
            .map(|s| parse_key(s))
            .collect::<Result<_, _>>()
            .map_err(|e| rmcp::ErrorData::invalid_params(e, None))?;

        let input = WidgetInput::KeyChord { keys };

        let action = {
            let shadow = self.debug.shadow_index.read().unwrap();
            let index = shadow.as_ref().ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    "Shadow index not yet built (no GPUI render has occurred)",
                    None,
                )
            })?;
            index.bubble_input(&params.entity_id, &input)
        };

        match action {
            Some(holon_frontend::input::InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                let mut op_params = HashMap::new();
                op_params.insert(
                    "id".to_string(),
                    holon_api::Value::String(entity_id.clone()),
                );

                let response = self
                    .engine()
                    .execute_operation(&entity_name, &operation.name, op_params)
                    .await
                    .map_err(|e| {
                        rmcp::ErrorData::internal_error(
                            format!(
                                "Key chord matched operation '{}.{}' but execution failed: {}",
                                entity_name, operation.name, e
                            ),
                            None,
                        )
                    })?;

                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({
                        "matched_operation": format!("{}.{}", entity_name, operation.name),
                        "entity_id": entity_id,
                        "result": response.map(|v| v.to_json_string()),
                    })
                    .to_string(),
                )]))
            }
            Some(holon_frontend::input::InputAction::Focus {
                block_id,
                placement,
            }) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "action": "focus",
                    "target_block_id": block_id,
                    "placement": format!("{:?}", placement),
                })
                .to_string(),
            )])),
            Some(holon_frontend::input::InputAction::Handled) => {
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({"action": "handled"}).to_string(),
                )]))
            }
            None => Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "action": "none",
                    "detail": "No handler matched the key chord"
                })
                .to_string(),
            )])),
        }
    }

    // ── UI interaction tools (raw input level) ─────────────────────────

    #[tool(
        description = "Send a mouse click at pixel coordinates in the GPUI window. Use describe_ui with format='json' to find element positions. Dispatches MouseDown+MouseUp events."
    )]
    async fn click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let tx = self.debug.interaction_tx.get().ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "No GPUI window connected (interaction channel not set up)",
                None,
            )
        })?;

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        tx.clone()
            .try_send(crate::server::InteractionCommand {
                event: crate::server::InteractionEvent::MouseClick {
                    position: (params.x, params.y),
                    button: params.button.clone(),
                    modifiers: params.modifiers.clone(),
                },
                response_tx: resp_tx,
            })
            .map_err(|_| {
                rmcp::ErrorData::internal_error("GPUI interaction channel disconnected", None)
            })?;

        resp_rx.await.map_err(|_| {
            rmcp::ErrorData::internal_error("GPUI did not respond to click event", None)
        })?;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({
                "clicked": [params.x, params.y],
                "button": params.button,
            })
            .to_string(),
        )]))
    }

    #[tool(description = "Turn the scroll wheel at a point in the GPUI window. \
                       `dx`/`dy` are line-based deltas (positive dy = down, \
                       positive dx = right). Pass `entity_id` to scroll at \
                       the center of a rendered block; otherwise provide \
                       `x`/`y` pixel coordinates. Dispatched through the \
                       same UserDriver channel as click/type_text, so it \
                       works off-screen and does not move the host cursor.")]
    async fn scroll(
        &self,
        Parameters(params): Parameters<ScrollParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let driver = self.debug.user_driver.get().ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "No UserDriver installed — the frontend has not registered one yet",
                None,
            )
        })?;

        match &params.entity_id {
            Some(entity_id) => {
                driver
                    .scroll_entity(entity_id, params.dx, params.dy)
                    .await
                    .map_err(|e| {
                        rmcp::ErrorData::internal_error(format!("scroll_entity failed: {e}"), None)
                    })?;
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({
                        "scrolled_entity": entity_id,
                        "delta": [params.dx, params.dy],
                    })
                    .to_string(),
                )]))
            }
            None => {
                driver
                    .scroll_at(params.x, params.y, params.dx, params.dy)
                    .await
                    .map_err(|e| {
                        rmcp::ErrorData::internal_error(format!("scroll_at failed: {e}"), None)
                    })?;
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({
                        "scrolled_at": [params.x, params.y],
                        "delta": [params.dx, params.dy],
                    })
                    .to_string(),
                )]))
            }
        }
    }

    #[tool(
        description = "Send keystrokes to the GPUI window. For special keys use names like 'enter', 'tab', 'escape', 'backspace', 'up', 'down', etc. For regular text, each character is sent as a separate keystroke."
    )]
    async fn type_text(
        &self,
        Parameters(params): Parameters<TypeTextParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let tx = self.debug.interaction_tx.get().ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "No GPUI window connected (interaction channel not set up)",
                None,
            )
        })?;

        let keystrokes: Vec<String> = if is_special_key(&params.text) {
            vec![params.text.clone()]
        } else {
            params.text.chars().map(|c| c.to_string()).collect()
        };

        for key in &keystrokes {
            let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
            tx.clone()
                .try_send(crate::server::InteractionCommand {
                    event: crate::server::InteractionEvent::KeyDown {
                        keystroke: key.clone(),
                        modifiers: params.modifiers.clone(),
                    },
                    response_tx: resp_tx,
                })
                .map_err(|_| {
                    rmcp::ErrorData::internal_error("GPUI interaction channel disconnected", None)
                })?;

            resp_rx.await.map_err(|_| {
                rmcp::ErrorData::internal_error("GPUI did not respond to key event", None)
            })?;
        }

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({
                "keystrokes_sent": keystrokes.len(),
            })
            .to_string(),
        )]))
    }
}

// --- Key parsing helpers ---

/// Parse a string key name into a holon_frontend Key enum.
fn parse_key(s: &str) -> Result<holon_frontend::input::Key, String> {
    use holon_frontend::input::Key;
    match s.to_lowercase().as_str() {
        "cmd" | "command" | "platform" => Ok(Key::Cmd),
        "ctrl" | "control" => Ok(Key::Ctrl),
        "alt" | "option" => Ok(Key::Alt),
        "shift" => Ok(Key::Shift),
        "up" => Ok(Key::Up),
        "down" => Ok(Key::Down),
        "left" => Ok(Key::Left),
        "right" => Ok(Key::Right),
        "home" => Ok(Key::Home),
        "end" => Ok(Key::End),
        "pageup" => Ok(Key::PageUp),
        "pagedown" => Ok(Key::PageDown),
        "tab" => Ok(Key::Tab),
        "enter" | "return" => Ok(Key::Enter),
        "backspace" => Ok(Key::Backspace),
        "delete" => Ok(Key::Delete),
        "escape" | "esc" => Ok(Key::Escape),
        "space" => Ok(Key::Space),
        s if s.len() == 1 => Ok(Key::Char(s.chars().next().unwrap())),
        s if s.starts_with('f') && s[1..].parse::<u8>().is_ok() => {
            Ok(Key::F(s[1..].parse::<u8>().unwrap()))
        }
        other => Err(format!("Unknown key: '{other}'")),
    }
}

/// Check if a string is a special key name (not regular text).
fn is_special_key(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        "enter"
            | "return"
            | "tab"
            | "escape"
            | "esc"
            | "backspace"
            | "delete"
            | "space"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
    ) || (s.starts_with('f') && s.len() <= 3 && s[1..].parse::<u8>().is_ok())
}

// --- Screenshot capture ---
//
// Uses xcap fork (nightscape/xcap#feat/macos-offscreen-windows) which
// uses `OptionAll` instead of `OptionOnScreenOnly`, so windows on other
// macOS desktops/spaces are visible.

#[cfg(target_os = "macos")]
fn capture_window_as_png(window_title: Option<&str>) -> Result<Vec<u8>, String> {
    let windows = xcap::Window::all().map_err(|e| format!("Failed to enumerate windows: {e}"))?;

    let our_pid = std::process::id();

    let window = if let Some(title) = window_title {
        let needle = title.to_lowercase();
        windows.iter().find(|w| {
            let t = w.title().unwrap_or_default().to_lowercase();
            let a = w.app_name().unwrap_or_default().to_lowercase();
            t.contains(&needle) || a.contains(&needle)
        })
    } else {
        // Match by PID + title "Holon" to skip GPUI's invisible auxiliary windows.
        windows
            .iter()
            // ALLOW(ok): window queries — non-fatal
            .find(|w| w.pid().ok() == Some(our_pid) && w.title().unwrap_or_default() == "Holon")
    };

    let window = window.ok_or_else(|| {
        // ALLOW(filter_map_ok): OS window queries — errors are not actionable
        let available: Vec<String> = windows
            .iter()
            .filter_map(|w| {
                let title = w.title().ok()?; // ALLOW(ok): window query
                let app = w.app_name().ok().unwrap_or_default(); // ALLOW(ok): window query
                let pid = w.pid().ok().unwrap_or(0); // ALLOW(ok): window query
                let width = w.width().unwrap_or(0);
                let height = w.height().unwrap_or(0);
                Some(format!(
                    "{title:?} (app={app:?}, pid={pid}, {width}x{height})"
                ))
            })
            .collect();
        format!(
            "No window found (our pid={our_pid}, searched for {:?}). Available: {available:?}",
            window_title.unwrap_or("(own process, largest window)")
        )
    })?;

    let win_title = window.title().unwrap_or_default();
    let win_app = window.app_name().unwrap_or_default();
    let win_w = window.width().unwrap_or(0);
    let win_h = window.height().unwrap_or(0);
    let win_x = window.x().unwrap_or(0);
    let win_y = window.y().unwrap_or(0);
    let win_minimized = window.is_minimized().unwrap_or(false);

    let img = window.capture_image().map_err(|e| {
        format!(
            "capture_image failed: {e} (title={win_title:?}, app={win_app:?}, \
             size={win_w}x{win_h}, pos=({win_x},{win_y}), minimized={win_minimized})"
        )
    })?;

    let mut png_buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png_buf, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    Ok(png_buf.into_inner())
}

// --- Helper methods for debug tools ---
impl HolonMcpServer {
    /// Resolve a doc_id (UUID or file path) to blocks from Loro.
    async fn get_loro_blocks(&self, doc_id: &str) -> Result<Vec<Block>, rmcp::ErrorData> {
        let store = self.debug.loro_doc_store.get().ok_or_else(|| {
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

    /// Resolve a doc_id to its block URI.
    async fn resolve_doc_uri(&self, doc_id: &str) -> Result<String, rmcp::ErrorData> {
        let uri = holon_api::EntityUri::from_raw(doc_id);
        if uri.is_sentinel() {
            return Ok(uri.to_string());
        }
        Ok(holon_api::EntityUri::block(doc_id).to_string())
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
            if let Some(root) = self.debug.orgmode_root.get() {
                let full = root.join(doc_id);
                if full.exists() {
                    return Ok(full);
                }
            }
        }

        // Try to resolve via Loro aliases
        if let Some(store) = self.debug.loro_doc_store.get() {
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
