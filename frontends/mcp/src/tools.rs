use crate::server::HolonMcpServer;
use crate::types::*;
use holon::api::holon_service::HolonService;
use holon::api::repository::CoreOperations;
use holon::api::types::Traversal;
use holon::storage::BLOCK_READ_TABLE;
use holon_api::{Block, Change, EntityName, EntityUri, QueryLanguage, Value};
use holon_core::storage::types::StorageEntity;
use holon_loro::LoroBackend;
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
) -> Option<holon_api::QueryContext> {
    let context_id = params.get("context_id").and_then(|v| v.as_str());
    let context_parent_id = params.get("context_parent_id").and_then(|v| v.as_str());
    service.build_context(context_id, context_parent_id).await
}

// Helper function to convert serde_json::Value to holon_api::Value
fn json_to_holon_value(v: serde_json::Value) -> Value {
    Value::from_json_value(v)
}

/// A SUT retired by `reset_vault` (Phase 1 Option A, plan F). Holds its Arcs +
/// temp dirs so nothing Drops: the retired engine's watchers/consolidator idle
/// against still-existing but abandoned fresh paths. `_`-prefixed because the
/// point is to KEEP them alive, not read them.
#[cfg(debug_assertions)]
struct RetiredSut {
    _session: Arc<holon_frontend::FrontendSession>,
    _engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    _backend: Arc<holon::api::backend_engine::BackendEngine>,
    _tempdirs: Box<dyn std::any::Any + Send>,
}

/// Process-wide retirement list. Grows by exactly one per `reset_vault`; a hard
/// cap (checked in the tool) refuses further resets rather than leaking
/// unboundedly.
#[cfg(debug_assertions)]
static RETIRED: std::sync::Mutex<Vec<RetiredSut>> = std::sync::Mutex::new(Vec::new());

#[cfg(debug_assertions)]
fn retired_len() -> usize {
    RETIRED.lock().expect("RETIRED poisoned").len()
}

#[cfg(debug_assertions)]
fn push_retired(sut: RetiredSut) {
    let mut r = RETIRED.lock().expect("RETIRED poisoned");
    r.push(sut);
    tracing::warn!(
        "reset_vault: {} retired engine(s) held (leaked-but-inert on abandoned temp paths)",
        r.len()
    );
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
/// Resolve the calling agent's id from a tool param or `HOLON_AGENT_ID`.
fn resolve_agent_id(param: Option<String>) -> Result<String, rmcp::ErrorData> {
    let id = param
        .or_else(|| std::env::var("HOLON_AGENT_ID").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "agent_id required: pass `agent_id` param or set HOLON_AGENT_ID env",
                None,
            )
        })?;
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(rmcp::ErrorData::invalid_params(
            format!("invalid agent_id {id:?} — must match [A-Za-z0-9._-]+"),
            None,
        ));
    }
    Ok(id)
}

/// Accept bare slugs (`now-query`) as well as fully-qualified ids
/// (`block:now-query`) — the org file stores the bare form.
fn ensure_block_prefix(s: &str) -> String {
    if s.starts_with("block:") {
        s.to_string()
    } else {
        format!("block:{s}")
    }
}

/// Build a filesystem-safe slug from a task id (lowercase alphanumeric + hyphen).
fn slugify_for_devlog(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.chars().take(40).collect()
}

/// Run a single `set_field` op through the standard pipeline so the
/// org renderer + Loro sync see the write.
async fn set_field(
    service: &HolonService,
    id: &str,
    field: &str,
    value: Value,
) -> Result<(), rmcp::ErrorData> {
    let mut storage: StorageEntity = HashMap::new();
    storage.insert("id".into(), Value::String(id.to_string()));
    storage.insert("field".into(), Value::String(field.to_string()));
    storage.insert("value".into(), value);
    service
        .execute_operation(&EntityName::new("block"), "set_field", storage)
        .await
        .map_err(|e| {
            rmcp::ErrorData::internal_error(format!("set_field({field}) on {id} failed: {e}"), None)
        })?;
    Ok(())
}

/// Read the canonical `assigned-to` value for a block straight from `block_raw`.
async fn read_assigned_to(
    engine: &Arc<holon::api::backend_engine::BackendEngine>,
    id: &str,
) -> Result<Option<String>, rmcp::ErrorData> {
    let sql =
        "SELECT json_extract(properties, '$.assigned-to') AS assigned_to FROM block_raw WHERE id = $id"
            .to_string();
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(id.to_string()));
    let rows = engine.execute_query(sql, params, None).await.map_err(|e| {
        rmcp::ErrorData::internal_error(format!("read assigned-to failed: {e}"), None)
    })?;
    Ok(rows.into_iter().next().and_then(|row| {
        row.get("assigned_to").and_then(|v| match v {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
    }))
}

fn json_map_to_storage_entity(map: HashMap<String, serde_json::Value>) -> StorageEntity {
    map.into_iter()
        .map(|(k, v)| (std::sync::Arc::from(k.as_str()), json_to_holon_value(v)))
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
            let engine = self.engine();
            let db_handle = engine.db_handle();
            module.ensure_schema(db_handle).await.map_err(|e| {
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
        self.finalize_query_response(&query_result.rows, Some(duration_ms), include_profile)
    }

    #[tool(
        description = "Execute the query stored in a source block by block_id. Looks up the block's `content` (the query) and `source_language` (one of holon_prql / holon_gql / holon_sql), then dispatches through the same path as `execute_query`. Use this to run live source-block queries (e.g. the Now.org `now-query::src::0`) without copy-pasting the SQL. `params`, `context_id`, `context_parent_id`, `render`, `include_profile`, and `language` (override) all mirror `execute_query`."
    )]
    async fn execute_source_block(
        &self,
        Parameters(params): Parameters<ExecuteSourceBlockParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // ALLOW(entity_uri_from_raw): MCP tool param ExecuteSourceBlockParams.block_id
        let block_id = EntityUri::from_raw(&params.block_id).to_string();
        let lookup_sql = format!(
            "SELECT content, source_language FROM block_raw WHERE id = '{}'",
            block_id.replace('\'', "''")
        );
        let lookup = self
            .service()
            .execute_raw_sql(&lookup_sql, HashMap::new())
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to look up source block '{}': {}", block_id, e),
                    None,
                )
            })?;
        let row = lookup.rows.into_iter().next().ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                format!("No block found with id '{}'", block_id),
                Some(serde_json::json!({"block_id": block_id})),
            )
        })?;
        let query = row
            .get("content")
            .and_then(|v| v.as_string())
            .ok_or_else(|| {
                rmcp::ErrorData::invalid_params(
                    format!("Block '{}' has no content", block_id),
                    None,
                )
            })?
            .to_string();
        let stored_language = row
            .get("source_language")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string());
        let language_str = params.language.or(stored_language).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                format!(
                    "Block '{}' has no source_language; pass `language` explicitly",
                    block_id
                ),
                None,
            )
        })?;
        let language = language_str
            .parse::<QueryLanguage>()
            .map_err(|e| rmcp::ErrorData::invalid_params(format!("Invalid language: {e}"), None))?;

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

        let query_result = self
            .service()
            .execute_query(&query, language, holon_params, context)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Query failed: {}", e),
                    Some(serde_json::json!({"block_id": block_id, "language": language_str})),
                )
            })?;

        let duration_ms = query_result.duration.as_secs_f64() * 1000.0;
        let include_profile = params.include_profile.unwrap_or(false);
        self.finalize_query_response(&query_result.rows, Some(duration_ms), include_profile)
    }

    #[tool(
        description = "Start watching a query for CDC changes. Supports prql, gql, and sql languages."
    )]
    async fn watch_query(
        &self,
        Parameters(params): Parameters<WatchQueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let context = extract_context_from_params(&self.service(), &params.params).await;

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
        let mut initial_rows: Vec<holon_api::StorageEntity> = Vec::new();
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
                    .map(|(k, v)| (k.to_string(), holon_to_json_value(v)))
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
                    let change: &holon_api::Change<holon_api::StorageEntity> = &row_change.change;
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
                                    .map(|(k, v)| (k.to_string(), holon_to_json_value(v)))
                                    .collect(),
                            ),
                            Change::Updated { data, .. } => Some(
                                data.iter()
                                    .map(|(k, v)| (k.to_string(), holon_to_json_value(v)))
                                    .collect(),
                            ),
                            Change::Deleted { .. } => None,
                            Change::FieldsChanged { fields, .. } => {
                                // Convert fields vec to a map
                                let mut map = HashMap::new();
                                for (field_name, _old_val, new_val) in fields {
                                    map.insert(field_name.clone(), holon_to_json_value(new_val));
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
                task_handle,
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

        let state = watches.remove(&params.watch_id).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "watch_not_found",
                Some(serde_json::json!({"watch_id": params.watch_id})),
            )
        })?;
        state.task_handle.abort();

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
            .execute_operation(
                &EntityName::new(&params.entity_name),
                &params.operation,
                storage_entity,
            )
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    // `{:#}` renders the FULL anyhow chain (every `.context`
                    // layer down to the typed source, e.g. `ParentNotFound`),
                    // not just the outermost message.
                    format!(
                        "Operation '{}' on '{}' failed: {:#}",
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
        self.finalize_query_response(&query_result.rows, Some(duration_ms), false)
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
                &format!("SELECT * FROM {BLOCK_READ_TABLE} WHERE id = $1"),
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
                context_params.insert(k.to_string(), v.clone());
            }
        }

        let profile = block_result.rows.first().map(|row| {
            let row_string_keyed: HashMap<String, Value> = row
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect();
            self.engine().profile_resolver().resolve(&row_string_keyed)
        });

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
            .entry("id".into())
            .or_insert_with(|| holon_api::Value::String(params.block_id.clone()));

        let response = self
            .service()
            .execute_operation(
                &EntityName::new(&params.entity_name),
                &params.command_name,
                storage_entity,
            )
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

    #[tool(
        description = "Return ranked Now-snapshot tasks visible to the calling agent. Mirrors the `now-query::src::0` block but adds two filters: tasks must be unclaimed OR already assigned to this agent (`assigned-to` property), and any `task_state IN ('TODO','DOING')` is allowed (so an agent re-discovers in-flight work). agent_id falls back to env HOLON_AGENT_ID. Tasks already claimed by the caller sort first."
    )]
    async fn now_for_agent(
        &self,
        Parameters(params): Parameters<NowForAgentParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let agent_id = resolve_agent_id(params.agent_id)?;
        let limit = params.limit.unwrap_or(10).clamp(1, 100);
        let sql = format!(
            "SELECT b.* \
             FROM block b \
             WHERE json_extract(b.properties, '$.task_state') IN ('TODO', 'DOING') \
               AND json_extract(b.properties, '$.gate') = 'G1' \
               AND ( \
                 json_extract(b.properties, '$.assigned-to') IS NULL \
                 OR json_extract(b.properties, '$.assigned-to') = $agent_id \
               ) \
               AND NOT EXISTS ( \
                 SELECT 1 FROM block_requires br \
                 JOIN block bl ON bl.id = br.required_id \
                 WHERE br.block_id = b.id \
                   AND COALESCE(json_extract(bl.properties, '$.task_state'), '') <> 'DONE' \
               ) \
               AND ( \
                 EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'agent') \
                 OR NOT EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'human-only') \
               ) \
             ORDER BY \
               CASE WHEN json_extract(b.properties, '$.assigned-to') = $agent_id THEN 0 ELSE 1 END, \
               json_extract(b.properties, '$.priority'), \
               json_extract(b.properties, '$.effort'), \
               b.id \
             LIMIT {limit}"
        );
        let mut q_params = HashMap::new();
        q_params.insert("agent_id".to_string(), Value::String(agent_id.clone()));

        let rows = self
            .engine()
            .execute_query(sql, q_params, None)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("now_for_agent query failed: {e}"),
                    Some(serde_json::json!({"agent_id": agent_id})),
                )
            })?;

        self.finalize_query_response(&rows, None, false)
    }

    #[tool(
        description = "Best-effort claim of a task for the calling agent. Reads current `assigned-to`; refuses if already held by another agent. Otherwise sets `assigned-to`, `claimed-at`, `claimed-from` (worktree path), and flips `task_state` to DOING. Sleeps 1s and re-reads to detect lost races inside the file-watcher debounce window (~500ms). Returns `{claimed: bool, assigned_to, was: <prior>}`."
    )]
    async fn claim_task(
        &self,
        Parameters(params): Parameters<ClaimTaskParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let agent_id = resolve_agent_id(params.agent_id)?;
        let task_id = ensure_block_prefix(&params.task_id);

        let current = read_assigned_to(&self.engine(), &task_id).await?;
        if let Some(other) = &current {
            if other != &agent_id {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({
                        "claimed": false,
                        "task_id": task_id,
                        "agent_id": agent_id,
                        "was": other,
                        "reason": "already-claimed-by-other",
                    })
                    .to_string(),
                )]));
            }
        }

        let now_iso = chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
            .expect("now within range")
            .to_rfc3339();
        let worktree = std::env::current_dir()
            .ok() // ALLOW(ok): best-effort metadata; missing CWD shouldn't block claim
            .map(|p| p.display().to_string());

        set_field(
            &self.service(),
            &task_id,
            "assigned-to",
            Value::String(agent_id.clone()),
        )
        .await?;
        set_field(
            &self.service(),
            &task_id,
            "claimed-at",
            Value::String(now_iso.clone()),
        )
        .await?;
        if let Some(wt) = &worktree {
            set_field(
                &self.service(),
                &task_id,
                "claimed-from",
                Value::String(wt.clone()),
            )
            .await?;
        }
        set_field(
            &self.service(),
            &task_id,
            "task_state",
            Value::String("DOING".to_string()),
        )
        .await?;

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let final_assignee = read_assigned_to(&self.engine(), &task_id).await?;
        let claimed = final_assignee.as_deref() == Some(&agent_id);

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({
                "claimed": claimed,
                "task_id": task_id,
                "agent_id": agent_id,
                "assigned_to": final_assignee,
                "claimed_at": now_iso,
                "claimed_from": worktree,
                "reason": if claimed { "ok" } else { "lost-race" },
            })
            .to_string(),
        )]))
    }

    #[tool(
        description = "Append a new TODO block as a child of an existing block. Mints a UUID for the new id (or uses `id` if supplied), defaults task_state to TODO and gate to G1 so the new task is visible to `now_for_agent` immediately. Extra `properties` are merged into the JSON properties bucket. Detects id collision via `block.create`'s response (Some(existing_id) means INSERT OR IGNORE no-op). NOTE: `tags` and `requires` params are reserved for a follow-up — set them via separate operations for now. Returns the new task's id."
    )]
    async fn add_subtask(
        &self,
        Parameters(params): Parameters<AddSubtaskParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let parent_id = ensure_block_prefix(&params.parent_id);

        let new_id_bare = params
            .id
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let new_id = ensure_block_prefix(&new_id_bare);

        let title = params.title.trim();
        if title.is_empty() {
            return Err(rmcp::ErrorData::invalid_params(
                "title must be non-empty",
                None,
            ));
        }
        let content = match params
            .body
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(body) => format!("{title}\n{body}"),
            None => title.to_string(),
        };

        let task_state = params
            .task_state
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "TODO".to_string());
        let gate = params
            .gate
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "G1".to_string());

        let mut storage: StorageEntity = HashMap::new();
        storage.insert("id".into(), Value::String(new_id.clone()));
        storage.insert("parent_id".into(), Value::String(parent_id.clone()));
        storage.insert("content".into(), Value::String(content.clone()));
        storage.insert("content_type".into(), Value::String("text".to_string()));
        storage.insert("task_state".into(), Value::String(task_state.clone()));
        storage.insert("gate".into(), Value::String(gate.clone()));
        // ID property mirrors the bare id so org-rendered :PROPERTIES: blocks stay round-trip stable.
        storage.insert("ID".into(), Value::String(new_id_bare.clone()));
        for (k, v) in params.properties.into_iter() {
            storage.insert(std::sync::Arc::from(k.as_str()), json_to_holon_value(v));
        }

        // block.create returns response = None on successful INSERT,
        // Some(existing_id) when INSERT OR IGNORE no-op'd on a primary-key collision.
        let response = self
            .service()
            .execute_operation(&EntityName::new("block"), "create", storage)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("block.create failed: {e}"), None)
            })?;
        if let Some(existing) = response {
            return Err(rmcp::ErrorData::invalid_params(
                format!(
                    "id collision: a block with id {new_id:?} already exists ({existing:?}) — pass a different `id` or omit it to mint a UUID"
                ),
                None,
            ));
        }

        let tags_warning = params.tags.as_ref().filter(|v| !v.is_empty()).map(|v| {
            format!(
                "tags ignored ({} supplied) — set via separate operation",
                v.len()
            )
        });
        let requires_warning = params.requires.as_ref().filter(|v| !v.is_empty()).map(|v| {
            format!(
                "requires ignored ({} supplied) — set via separate operation",
                v.len()
            )
        });

        let warnings: Vec<String> = [tags_warning, requires_warning]
            .into_iter()
            .flatten()
            .collect();
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({
                "task_id": new_id,
                "id_bare": new_id_bare,
                "parent_id": parent_id,
                "task_state": task_state,
                "gate": gate,
                "warnings": warnings,
            })
            .to_string(),
        )]))
    }

    #[tool(
        description = "Mark a claimed task DONE and append a devlog file at <cwd>/devlog/YYYY-MM-DD-HHMMSS-<agent-id>-<slug>.md with the supplied summary (and optional commit_sha). Sets `task_state=DONE` and `completed-at` (UTC RFC3339) via the standard operation pipeline. Errors if `<cwd>/devlog/` does not exist (run holon-mcp from a repo checkout that has it). Returns the devlog path."
    )]
    async fn complete_task(
        &self,
        Parameters(params): Parameters<CompleteTaskParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let agent_id = resolve_agent_id(params.agent_id)?;
        let task_id = ensure_block_prefix(&params.task_id);

        let cwd = std::env::current_dir().map_err(|e| {
            rmcp::ErrorData::internal_error(format!("current_dir failed: {e}"), None)
        })?;
        let devlog_dir = cwd.join("devlog");
        if !devlog_dir.is_dir() {
            return Err(rmcp::ErrorData::invalid_params(
                format!(
                    "devlog dir not found at {} — run holon-mcp from a repo checkout that contains devlog/",
                    devlog_dir.display()
                ),
                None,
            ));
        }

        let completed_iso = chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
            .expect("now within range")
            .to_rfc3339();
        set_field(
            &self.service(),
            &task_id,
            "task_state",
            Value::String("DONE".to_string()),
        )
        .await?;
        set_field(
            &self.service(),
            &task_id,
            "completed-at",
            Value::String(completed_iso.clone()),
        )
        .await?;

        let datestamp = chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
            .expect("now within range")
            .format("%Y-%m-%d-%H%M%S")
            .to_string();
        let slug = slugify_for_devlog(&params.task_id);
        let filename = if slug.is_empty() {
            format!("{datestamp}-{agent_id}.md")
        } else {
            format!("{datestamp}-{agent_id}-{slug}.md")
        };
        let path = devlog_dir.join(&filename);

        let mut body = format!(
            "# {}\n\n- **Agent:** `{}`\n- **Task:** `{}`\n- **Completed:** {}\n",
            params.task_id, agent_id, task_id, completed_iso
        );
        if let Some(sha) = &params.commit_sha {
            body.push_str(&format!("- **Commit:** `{sha}`\n"));
        }
        body.push_str(&format!("\n## Summary\n\n{}\n", params.summary.trim()));

        std::fs::write(&path, &body).map_err(|e| {
            rmcp::ErrorData::internal_error(
                format!("write devlog {} failed: {e}", path.display()),
                None,
            )
        })?;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({
                "task_id": task_id,
                "agent_id": agent_id,
                "task_state": "DONE",
                "completed_at": completed_iso,
                "devlog": path.display().to_string(),
            })
            .to_string(),
        )]))
    }
}

/// Debug-only per-case reset tool, isolated in its own `#[tool_router]` impl
/// so the whole router (method + macro-generated registration) compiles out
/// together in release — rmcp's `#[tool_router]` does not honour a per-method
/// `#[cfg]`, so a gated tool inside a shared router leaves a dangling
/// `reset_vault_tool_attr` reference and breaks the release build.
#[cfg(debug_assertions)]
#[tool_router(router = tool_router_reset, vis = "pub(crate)")]
impl HolonMcpServer {
    /// Per-case, in-process reset (Phase 1 Option A). Builds a FRESH seeded
    /// engine+session on fresh temp paths, swaps the live MCP backend cell so
    /// every subsequent tool call reads the new engine, then rebinds the live
    /// window onto it — keeping ONE window and ONE MCP server. Returns the new
    /// `block_raw` id-set (a fail-loud self-check that the reset actually
    /// re-seeded).
    ///
    /// GATED: compiled only in debug builds AND requires `HOLON_MCP_ALLOW_RESET`
    /// to be set, so a shipped release can never swap a user's vault over MCP.
    #[tool(
        description = "TEST-ONLY per-case reset: boot a fresh seeded vault and rebind the running window onto it in place (no relaunch, no 2nd MCP server). Requires HOLON_MCP_ALLOW_RESET. Returns the new block_raw id-set."
    )]
    async fn reset_vault(
        &self,
        Parameters(params): Parameters<ResetVaultParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if std::env::var("HOLON_MCP_ALLOW_RESET").is_err() {
            return Err(rmcp::ErrorData::internal_error(
                "reset_vault is disabled — set HOLON_MCP_ALLOW_RESET=1 to enable the \
                 in-process vault reset (test harness only)",
                None,
            ));
        }

        // Retirement-list cap (plan F): refuse rather than leak unboundedly;
        // the caller falls back to the Option B' cold relaunch past this.
        const RESET_CAP: usize = 20;
        if retired_len() >= RESET_CAP {
            return Err(rmcp::ErrorData::internal_error(
                format!(
                    "reset_vault retirement cap reached ({RESET_CAP} retired engines); \
                     fall back to a cold `ios_reset_sut.sh` relaunch"
                ),
                None,
            ));
        }

        let builder = self.debug.reset_builder.get().cloned().ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "reset_vault requires a frontend reset builder (no window/pump wired)",
                None,
            )
        })?;
        let reset_tx = self.debug.reset_tx.get().cloned().ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "reset_vault requires a frontend reset pump (reset_tx not installed)",
                None,
            )
        })?;

        // 1. Build the fresh SUT on the tokio side (NOT the GPUI main thread).
        let files: Vec<(String, String)> = params
            .files
            .into_iter()
            .map(|f| (f.name, f.content))
            .collect();
        let out = builder(files).await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("reset_vault build failed: {e}"), None)
        })?;

        // 2. Swap the live MCP backend cell BEFORE rebinding, so any concurrent
        //    read already sees the fresh engine (plan C2).
        {
            let mut cell = self.backend.write().expect("backend cell poisoned");
            cell.engine = Some(out.backend.clone());
            let bs: Arc<dyn holon_frontend::reactive::BuilderServices> = out.engine.clone();
            cell.builder_services = Some(bs);
        }

        // Swap the debug convergence/mirror handles in the SAME breath, so
        // `await_quiescence` / `debug_pbt_snapshot` read the fresh session's
        // Loro sync controller / org idle signal / CDC mirror rather than the
        // retired engine's (a stale read would silently answer wrong — fail
        // that failure mode by swapping alongside the backend cell).
        {
            let mut cell = self
                .debug
                .live_debug
                .write()
                .expect("live_debug cell poisoned");
            *cell = out.live_debug.clone();
        }

        // 3. Rebind the live window (main thread) and await the ack.
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        reset_tx
            .clone()
            .try_send(crate::server::ResetRequest {
                session: out.session.clone(),
                engine: out.engine.clone(),
                ack: ack_tx,
            })
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("reset_vault could not reach the reset pump: {e}"),
                    None,
                )
            })?;
        // Fail loud rather than hang forever if the main-thread pump stalls.
        tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx)
            .await
            .map_err(|_| {
                rmcp::ErrorData::internal_error(
                    "reset_vault timed out (30s) waiting for the main-thread rebind pump",
                    None,
                )
            })?
            .map_err(|_| {
                rmcp::ErrorData::internal_error(
                    "reset_vault pump dropped the ack (window gone?)",
                    None,
                )
            })?
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("reset_vault rebind failed: {e}"), None)
            })?;

        // 4. Retire the SUT: keep its Arcs + temp dirs alive-but-inert so no
        //    Drop runs (plan F). Growth is observable via `RETIRED.len()`.
        push_retired(RetiredSut {
            _session: out.session,
            _engine: out.engine,
            _backend: out.backend.clone(),
            _tempdirs: out.retire,
        });

        // 5. Fail-loud self-check: read the fresh engine's block_raw id-set.
        let probe = self
            .service()
            .execute_raw_sql("SELECT id FROM block_raw ORDER BY id", HashMap::new())
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("reset_vault self-check query failed: {e}"),
                    None,
                )
            })?;
        let ids: Vec<String> = probe
            .rows
            .iter()
            .filter_map(|row| row.get("id").map(|v| format!("{v:?}")))
            .collect();

        let result = serde_json::json!({
            "reset": true,
            "block_raw_count": ids.len(),
            "block_raw_ids": ids,
            "retired_engines": retired_len(),
        });
        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }
}

#[tool_router(router = tool_router_ui, vis = "pub(crate)")]
impl HolonMcpServer {
    /// Block-until-quiescent: the MCP-facing twin of the composed PBT's
    /// `converge_projections` combined-fixed-point settle. Waits — capped at
    /// `budget_ms` (default 30000) — until the CDC watermark, the Loro
    /// sync-controller frontier (vs the authority doc's oplog frontier), and the
    /// org idle tick are ALL simultaneously stable for one quiet floor, then
    /// reports the signals it actually watched. Budget exhaustion is an error
    /// naming the still-moving signal(s) — a non-converged wait is NEVER reported
    /// as success. Reads the swappable `live_debug` handles, so it follows a
    /// `reset_vault` onto the fresh session.
    #[cfg(debug_assertions)]
    #[tool(
        description = "TEST-ONLY: block until the live session reaches a combined fixed point (Turso CDC + Loro frontier + org idle tick all quiet for one floor), capped at budget_ms (default 30000). Errors naming the still-moving signal(s) if the budget is exhausted."
    )]
    async fn await_quiescence(
        &self,
        Parameters(params): Parameters<AwaitQuiescenceParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let budget = std::time::Duration::from_millis(params.budget_ms.unwrap_or(30_000));

        // Snapshot the swappable handles once: a concurrent `reset_vault` swaps
        // the cell, but a single quiescence wait converges the session that was
        // live when it began.
        let (loro_sync, loro_store, org_idle) = {
            let cell = self
                .debug
                .live_debug
                .read()
                .expect("live_debug cell poisoned");
            (
                cell.loro_sync_handle.clone(),
                cell.loro_doc_store.clone(),
                cell.org_idle_signal.clone(),
            )
        };

        // Wired-but-unreachable is a bug, not a skip: a Loro doc-store with no
        // sync controller can never be observed for convergence — fail loud
        // rather than silently dropping the Loro signal.
        if loro_store.is_some() && loro_sync.is_none() {
            return Err(rmcp::ErrorData::internal_error(
                "await_quiescence: a Loro doc-store is wired but its sync-controller handle is \
                 unreachable — cannot observe Loro convergence (half-wired/stale session)",
                None,
            ));
        }

        let engine = self.engine();
        let check_loro = loro_sync.is_some() && loro_store.is_some();

        let mut signals: Vec<&str> = vec!["cdc"];
        if check_loro {
            signals.push("loro");
        }
        if org_idle.is_some() {
            signals.push("org");
        }

        let start = tokio::time::Instant::now();
        let deadline = start + budget;
        let quiet = std::time::Duration::from_millis(50);
        let poll = std::time::Duration::from_millis(2);

        let mut last_cdc = engine.db_handle().cdc_emitted_watermark();
        let mut last_tick = org_idle.as_ref().map(|s| s.current_tick());
        let mut last_activity = tokio::time::Instant::now();
        let mut still_moving: Vec<&str> = Vec::new();

        loop {
            tokio::time::sleep(poll).await;
            let mut moving: Vec<&str> = Vec::new();

            // Loro FIRST (the projection that writes the reordered sort_key CDC
            // then fires): a frontier not yet caught up counts as activity.
            if let (Some(sync), Some(store)) = (&loro_sync, &loro_store) {
                let current = {
                    let guard = store.read().await;
                    guard.get_global_doc().await.map_err(|e| {
                        rmcp::ErrorData::internal_error(
                            format!("await_quiescence: live Loro global doc unreachable: {e}"),
                            None,
                        )
                    })?
                }
                .doc()
                .oplog_frontiers();
                if sync.last_synced_frontiers() != current {
                    moving.push("loro");
                }
            }

            let now_cdc = engine.db_handle().cdc_emitted_watermark();
            if now_cdc != last_cdc {
                last_cdc = now_cdc;
                moving.push("cdc");
            }

            if let Some(idle) = &org_idle {
                let now_tick = idle.current_tick();
                if last_tick != Some(now_tick) {
                    last_tick = Some(now_tick);
                    moving.push("org");
                }
            }

            let now = tokio::time::Instant::now();
            if moving.is_empty() {
                if now.duration_since(last_activity) >= quiet {
                    let lamport_height = self.live_lamport_height().await?;
                    let result = serde_json::json!({
                        "converged": true,
                        "waited_ms": start.elapsed().as_millis() as u64,
                        "lamport_height": lamport_height,
                        "signals": signals,
                    });
                    return Ok(CallToolResult::success(vec![Content::text(
                        result.to_string(),
                    )]));
                }
            } else {
                last_activity = now;
                still_moving = moving;
            }

            if now >= deadline {
                return Err(rmcp::ErrorData::internal_error(
                    format!(
                        "await_quiescence: budget {}ms exhausted before a combined fixed point; \
                         still-moving signal(s): {:?}",
                        budget.as_millis(),
                        still_moving
                    ),
                    None,
                ));
            }
        }
    }

    /// Capture the live PBT-facing snapshot: the CDC-driven `LiveData` block
    /// mirror (NOT a matview SQL read) + focus-roots, plus the Loro tree's error
    /// flag, lamport height, and per-parent child lists. The block source is the
    /// swappable `block_query_source` — fail loud (no SQL fallback) if it is
    /// unwired. Follows a `reset_vault` onto the fresh session.
    #[cfg(debug_assertions)]
    #[tool(
        description = "TEST-ONLY: snapshot the live CDC-mirrored blocks (LiveData, not matview SQL), focus-roots, and Loro tree state (had_errors, lamport_height, per-parent children). Errors if no block_query_source is wired."
    )]
    async fn debug_pbt_snapshot(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let (block_query_source, loro_sync, reactive_engine) = {
            let cell = self
                .debug
                .live_debug
                .read()
                .expect("live_debug cell poisoned");
            (
                cell.block_query_source.clone(),
                cell.loro_sync_handle.clone(),
                cell.reactive_engine.clone(),
            )
        };
        let block_query_source = block_query_source.ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "debug_pbt_snapshot requires a wired block_query_source (the live CDC mirror); \
                 there is no SQL fallback",
                None,
            )
        })?;
        // The cell is populated (block_query_source resolved), so the reactive
        // engine slot MUST be present too — a missing one is a boot/reset wiring
        // bug, not an honest "not wired". Fail loud rather than emit null.
        let reactive_engine = reactive_engine.ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                "debug_pbt_snapshot: live_debug cell is populated but reactive_engine is unset \
                 (boot/reset failed to wire the engine)",
                None,
            )
        })?;
        let focused_block =
            holon_frontend::reactive::BuilderServices::focused_block(&*reactive_engine)
                .map(|b| b.to_string());

        let snapshot = block_query_source.snapshot().await.map_err(|e| {
            rmcp::ErrorData::internal_error(
                format!("debug_pbt_snapshot: live block snapshot failed: {e}"),
                None,
            )
        })?;

        let live_blocks: Vec<serde_json::Value> = snapshot
            .iter_blocks()
            .map(|b| serde_json::to_value(holon_api::block::BlockWire::from(b)))
            .collect::<Result<_, _>>()
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("debug_pbt_snapshot: BlockWire serialization failed: {e}"),
                    None,
                )
            })?;

        let focus_roots: Vec<serde_json::Value> =
            holon_core::storage::BlockQuery::focus_roots(&snapshot)
                .into_iter()
                .map(|fr| serde_json::json!({"region": fr.region, "root_id": fr.root_id}))
                .collect();

        let loro_had_errors = loro_sync.map(|h| h.error_count() > 0).unwrap_or(false);

        let (lamport_height, loro_tree_children) = match self.live_loro_backend().await? {
            Some(backend) => {
                let height = backend.lamport_height().await.map_err(|e| {
                    rmcp::ErrorData::internal_error(
                        format!("debug_pbt_snapshot: live Loro lamport_height failed: {e}"),
                        None,
                    )
                })?;
                let children = self.live_loro_tree_children(&backend).await?;
                (Some(height), children)
            }
            None => (None, std::collections::BTreeMap::new()),
        };

        let result = serde_json::json!({
            "live_blocks": live_blocks,
            "focus_roots": focus_roots,
            "loro_had_errors": loro_had_errors,
            "lamport_height": lamport_height,
            "loro_tree_children": loro_tree_children,
            "focused_block": focused_block,
        });
        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    #[tool(
        description = "List all loaded Loro documents with their file paths and UUID→path alias mappings. Requires Loro to be enabled."
    )]
    async fn list_loro_documents(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let store = self.current_loro_doc_store().ok_or_else(|| {
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
            .map(|b| serde_json::to_value(holon_api::block::BlockWire::from(b)))
            .collect::<Result<_, _>>()
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    "serialization_failed",
                    Some(serde_json::json!({"error": e.to_string()})),
                )
            })?;

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

        // Fetch ALL blocks, then select the doc's full subtree by walking
        // parent chains in Rust. A `parent_id LIKE '<doc_uri>%'` filter only
        // finds direct children: nested blocks have another block's UUID as
        // parent_id and would be falsely reported as only_in_loro.
        // task_state lives inside the properties JSON, so surface it as a
        // column for the field comparison below.
        let sql = format!(
            "SELECT *, json_extract(properties, '$.task_state') AS task_state \
             FROM {BLOCK_READ_TABLE}"
        );
        let all_rows = self
            .engine()
            .execute_query(sql.to_string(), HashMap::new(), None)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("SQL query failed: {}", e),
                    Some(serde_json::json!({"sql": sql})),
                )
            })?;

        let mut rows_by_id: HashMap<String, &holon_api::StorageEntity> = HashMap::new();
        for row in &all_rows {
            if let Some(Value::String(id)) = row.get("id") {
                rows_by_id.insert(id.clone(), row);
            }
        }

        let parent_of = |id: &str| -> Option<String> {
            match rows_by_id.get(id)?.get("parent_id") {
                Some(Value::String(p)) => Some(p.clone()),
                _ => None,
            }
        };

        // Build SQL block map: every block whose parent chain reaches the doc.
        // The starts_with covers legacy hierarchical ids (old `LIKE '<doc_uri>%'`).
        let mut sql_map: HashMap<String, &holon_api::StorageEntity> = HashMap::new();
        for (id, row) in &rows_by_id {
            let mut current = id.clone();
            let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
            while let Some(parent) = parent_of(&current) {
                if parent.starts_with(&doc_uri) {
                    sql_map.insert(id.clone(), *row);
                    break;
                }
                if !visited.insert(parent.clone()) {
                    return Err(rmcp::ErrorData::internal_error(
                        format!("parent_id cycle detected at block '{}'", parent),
                        Some(serde_json::json!({"block_id": id})),
                    ));
                }
                current = parent;
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
                        "content" => Some(loro_block.content.clone()),
                        "parent_id" => Some(loro_block.parent_id.to_string()),
                        "content_type" => Some(loro_block.content_type.to_string()),
                        "task_state" => loro_block.get_property_str("task_state"),
                        _ => unreachable!("field '{}' listed but not extracted", field),
                    };
                    let sql_val = match sql_row.get(*field) {
                        None | Some(Value::Null) => None,
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(other) => Some(format!("{:?}", other)),
                    };
                    if loro_val != sql_val {
                        diffs.push(serde_json::json!({
                            "field": field,
                            "loro": loro_val,
                            "sql": sql_val,
                        }));
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
            "sql_block_count": sql_map.len(),
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

        let content = self
            .debug
            .org_filesystem()
            .read_to_string(&file_path)
            .await
            .map_err(|e| {
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

        let doc_uri = EntityUri::parse(&params.doc_id).map_err(|e| {
            rmcp::ErrorData::invalid_params(
                format!("invalid doc_id `{}`: {e}", params.doc_id),
                None,
            )
        })?;
        let rendered = OrgRenderer::render_entitys(&blocks, &file_path, &doc_uri);

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

        let svc = self.builder_services().ok_or_else(|| {
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
        description = "Inspect the GPUI cross-block navigation state. Shows the reactive tree (widget hierarchy with navigators and entity IDs) and the cached focus path (ancestor chain from root to focused entity, with operations and collection markers). Use this to debug navigation issues."
    )]
    async fn describe_navigation(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let state = self.debug.navigation_state.read().unwrap();
        let focus_path_desc = self.debug.input_router.describe_focus_path();
        let output = format!(
            "── Reactive Tree ──\n{}\n\n── Focus Path ──\n{}",
            state.tree_description, focus_path_desc,
        );
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    // ── UI interaction tools (semantic level) ──────────────────────────

    #[tool(
        description = "Simulate arrow-key navigation between blocks. Walks the reactive tree via focus-path to find the next focusable block in the given direction. Returns the target block_id and cursor placement."
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

        let from_entity_uri = holon_api::EntityUri::parse(&params.from_entity_id).map_err(|e| {
            rmcp::ErrorData::invalid_params(
                format!("from_entity_id is not a valid EntityUri: {e}"),
                None,
            )
        })?;
        match self
            .debug
            .input_router
            .bubble_input(&from_entity_uri, &input)
        {
            Some(holon_frontend::input::InputAction::Focus {
                block_id,
                placement,
            }) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "target_block_id": block_id.as_str(),
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
        description = "Simulate a keyboard shortcut (key chord) at a specific entity. The chord bubbles up through the reactive tree via focus-path, matching against bound operations. If a match is found, the operation is executed."
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

        let entity_uri = holon_api::EntityUri::parse(&params.entity_id).map_err(|e| {
            rmcp::ErrorData::invalid_params(
                format!("entity_id is not a valid EntityUri: {e}"),
                None,
            )
        })?;
        match self.debug.input_router.bubble_input(&entity_uri, &input) {
            Some(holon_frontend::input::InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                let mut op_params: holon_api::StorageEntity = HashMap::new();
                op_params.insert("id".into(), holon_api::Value::String(entity_id.to_string()));

                let entity_name_typed = EntityName::new(&entity_name);
                let response = self
                    .engine()
                    .execute_operation(&entity_name_typed, &operation.name, op_params)
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
                    "target_block_id": block_id.as_str(),
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
        description = "Click an element in the GPUI window. Prefer `entity_id` (a block id from \
                       describe_ui): the click is dispatched at the element's center via the same \
                       entity-addressed UserDriver path the E2E tests use — it resolves the \
                       element bounds, hit-tests the point, warns if a different element is on \
                       top, and survives scroll/relayout. Falls back to raw `x`/`y` pixel \
                       coordinates when `entity_id` is omitted. Dispatches MouseDown+MouseUp events."
    )]
    async fn click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if let Some(entity_id) = &params.entity_id {
            let driver = self.debug.user_driver.get().ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    "No UserDriver installed — the frontend has not registered one yet",
                    None,
                )
            })?;
            let entity_uri = holon_api::EntityUri::parse(entity_id).map_err(|e| {
                rmcp::ErrorData::invalid_params(
                    format!("entity_id is not a valid EntityUri: {e}"),
                    None,
                )
            })?;
            driver
                .click_entity(&entity_uri, &params.region)
                .await
                .map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("click_entity failed: {e}"), None)
                })?;
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "clicked_entity": entity_id,
                    "region": params.region,
                })
                .to_string(),
            )]));
        }

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
                let entity_uri = holon_api::EntityUri::parse(entity_id).map_err(|e| {
                    rmcp::ErrorData::invalid_params(
                        format!("entity_id is not a valid EntityUri: {e}"),
                        None,
                    )
                })?;
                driver
                    .scroll_entity(&entity_uri, params.dx, params.dy)
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
    /// Build the standard `QueryResult` JSON response from a set of holon rows,
    /// optionally annotating each row with profile metadata.
    fn finalize_query_response(
        &self,
        rows: &[holon_api::StorageEntity],
        duration_ms: Option<f64>,
        include_profile: bool,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let json_rows: Vec<HashMap<String, serde_json::Value>> = rows
            .iter()
            .map(|row| {
                let mut json_row: HashMap<String, serde_json::Value> = row
                    .iter()
                    .map(|(k, v)| (k.to_string(), holon_to_json_value(v)))
                    .collect();

                if include_profile {
                    let row_string_keyed: HashMap<String, Value> = row
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.clone()))
                        .collect();
                    let profile = self.engine().profile_resolver().resolve(&row_string_keyed);
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
            duration_ms,
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

    /// The current Loro doc store: the swappable `live_debug` cell when
    /// populated (mobile boot + every `reset_vault` swap), else the boot-time
    /// `OnceLock` (desktop paths that never reset). Tools MUST read through
    /// this, not `debug.loro_doc_store` directly — the `OnceLock` goes stale
    /// after a reset and would silently answer against the retired session.
    fn current_loro_doc_store(
        &self,
    ) -> Option<Arc<tokio::sync::RwLock<holon::sync::LoroDocumentStore>>> {
        let from_cell = self
            .debug
            .live_debug
            .read()
            .expect("live_debug cell poisoned")
            .loro_doc_store
            .clone();
        from_cell.or_else(|| self.debug.loro_doc_store.get().cloned())
    }

    /// Build a `LoroBackend` over the live, swappable global doc from the
    /// `live_debug` cell. `None` when Loro is not wired in this config; a wired
    /// store whose global doc is unreachable is an error, not `None`.
    #[cfg(debug_assertions)]
    async fn live_loro_backend(&self) -> Result<Option<LoroBackend>, rmcp::ErrorData> {
        let store = {
            let cell = self
                .debug
                .live_debug
                .read()
                .expect("live_debug cell poisoned");
            cell.loro_doc_store.clone()
        };
        match store {
            None => Ok(None),
            Some(store) => {
                let doc = {
                    let guard = store.read().await;
                    guard.get_global_doc().await.map_err(|e| {
                        rmcp::ErrorData::internal_error(
                            format!("live Loro global doc unreachable: {e}"),
                            None,
                        )
                    })?
                };
                Ok(Some(LoroBackend::from_document(doc)))
            }
        }
    }

    /// The live Loro doc's lamport height, or `None` when Loro is not wired.
    #[cfg(debug_assertions)]
    async fn live_lamport_height(&self) -> Result<Option<u32>, rmcp::ErrorData> {
        match self.live_loro_backend().await? {
            None => Ok(None),
            Some(backend) => {
                let height = backend.lamport_height().await.map_err(|e| {
                    rmcp::ErrorData::internal_error(
                        format!("live Loro lamport_height failed: {e}"),
                        None,
                    )
                })?;
                Ok(Some(height))
            }
        }
    }

    /// Per-parent child-id lists from the live Loro tree (only parents that have
    /// children are keyed), for cross-checking against the CDC-mirrored snapshot.
    #[cfg(debug_assertions)]
    async fn live_loro_tree_children(
        &self,
        backend: &LoroBackend,
    ) -> Result<std::collections::BTreeMap<String, Vec<String>>, rmcp::ErrorData> {
        let blocks = backend.get_all_blocks(Traversal::ALL).await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("live Loro get_all_blocks failed: {e}"), None)
        })?;
        let mut out = std::collections::BTreeMap::new();
        for block in &blocks {
            let parent = block.id.to_string();
            let children = backend.list_children(&parent).await.map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("live Loro list_children({parent}) failed: {e}"),
                    None,
                )
            })?;
            if !children.is_empty() {
                out.insert(parent, children);
            }
        }
        Ok(out)
    }

    /// Resolve a doc_id (UUID or file path) to blocks from Loro.
    async fn get_loro_blocks(&self, doc_id: &str) -> Result<Vec<Block>, rmcp::ErrorData> {
        let store = self.current_loro_doc_store().ok_or_else(|| {
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
        // ALLOW(entity_uri_from_raw): resolve_doc_uri raw MCP arg before sentinel check
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
            let fs = self.debug.org_filesystem();
            let path = std::path::PathBuf::from(doc_id);
            if fs.exists(&path) {
                return Ok(path);
            }
            // Try under orgmode_root
            if let Some(root) = self.debug.orgmode_root.get() {
                let full = root.join(doc_id);
                if fs.exists(&full) {
                    return Ok(full);
                }
            }
        }

        // Try to resolve via Loro aliases
        if let Some(store) = self.current_loro_doc_store() {
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
        let v = json_to_holon_value(serde_json::json!(2.5));
        assert_eq!(v, Value::Float(2.5));
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
        let v = holon_to_json_value(&Value::Float(2.5));
        assert_eq!(v, serde_json::json!(2.5));
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
