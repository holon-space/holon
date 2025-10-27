//! SQL-based operation provider for blocks
//!
//! Provides direct SQL access to block operations, bypassing the Loro CRDT layer.
//! Used when OrgMode is enabled but Loro is disabled, or by any component that
//! needs to write blocks directly to the database.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::datasource::{OperationProvider, OperationResult, Result};
use crate::storage::sql_utils::value_to_sql_literal;
use crate::storage::turso::DbHandle;
use crate::storage::types::StorageEntity;
use crate::sync::event_bus::{AggregateType, Event, EventBus, EventKind, EventOrigin};
use holon_api::{OperationDescriptor, OperationParam, TypeHint, Value};

/// Known columns in the blocks table that can be used directly in SQL.
/// Any param key not in this set gets packed into the `properties` JSON column.
/// Known columns in the blocks table (must match schema in schema_modules.rs).
const BLOCKS_KNOWN_COLUMNS: &[&str] = &[
    "id",
    "parent_id",
    "document_id",
    "depth",
    "sort_key",
    "content",
    "content_type",
    "source_language",
    "source_name",
    "properties",
    "collapsed",
    "completed",
    "block_type",
    "created_at",
    "updated_at",
    "_change_origin",
];

/// A prepared operation: SQL statements and events, ready for execution.
struct PreparedOp {
    sql_statements: Vec<String>,
    events: Vec<Event>,
}

/// SQL-based operation provider that writes directly to a Turso table.
///
/// Uses the DbHandle actor to execute SQL, ensuring all writes go through
/// the connection that has CDC callbacks registered. This is critical for
/// materialized view change detection and real-time UI updates.
pub struct SqlOperationProvider {
    db_handle: DbHandle,
    table_name: String,
    entity_name: String,
    entity_short_name: String,
    known_columns: HashSet<String>,
    event_bus: Option<Arc<dyn EventBus>>,
}

impl SqlOperationProvider {
    pub fn new(
        db_handle: DbHandle,
        table_name: String,
        entity_name: String,
        entity_short_name: String,
    ) -> Self {
        let known_columns = BLOCKS_KNOWN_COLUMNS.iter().map(|s| s.to_string()).collect();
        Self {
            db_handle,
            table_name,
            entity_name,
            entity_short_name,
            known_columns,
            event_bus: None,
        }
    }

    pub fn with_event_bus(
        db_handle: DbHandle,
        table_name: String,
        entity_name: String,
        entity_short_name: String,
        event_bus: Arc<dyn EventBus>,
    ) -> Self {
        let known_columns = BLOCKS_KNOWN_COLUMNS.iter().map(|s| s.to_string()).collect();
        Self {
            db_handle,
            table_name,
            entity_name,
            entity_short_name,
            known_columns,
            event_bus: Some(event_bus),
        }
    }

    fn aggregate_type(&self) -> AggregateType {
        AggregateType::parse(&self.entity_short_name)
    }

    async fn publish_event(
        &self,
        event_kind: EventKind,
        aggregate_id: &str,
        payload: HashMap<String, serde_json::Value>,
    ) {
        if let Some(ref bus) = self.event_bus {
            let event = Event::new(
                event_kind,
                self.aggregate_type(),
                aggregate_id,
                EventOrigin::Other("sql".to_string()),
                payload,
            );
            if let Err(e) = bus.publish(event, None).await {
                tracing::warn!(
                    "[SqlOperationProvider] Failed to publish {}: {}",
                    event_kind,
                    e
                );
            }
        }
    }

    /// Walk up the parent chain from a block to find its document URI.
    /// Returns the `doc:...` URI of the owning document, or None if
    /// the block doesn't exist or has no document ancestor.
    async fn find_document_uri(&self, block_id: &str) -> Option<String> {
        use holon_api::EntityUri;

        let mut current_id = block_id.to_string();
        // Limit iterations to prevent infinite loops from circular parent refs
        for _ in 0..50 {
            let sql = format!(
                "SELECT parent_id FROM {} WHERE id = '{}'",
                self.table_name,
                current_id.replace('\'', "''")
            );
            let rows = self
                .db_handle
                .query(&sql, HashMap::new())
                .await
                .expect("database query in find_document_uri must succeed");
            let parent_id = rows.first()?.get("parent_id")?.as_string()?.to_string();

            if EntityUri::parse(&parent_id)
                .map(|u| u.is_doc())
                .unwrap_or(false)
            {
                return Some(parent_id);
            }
            current_id = parent_id;
        }
        None
    }

    fn value_to_sql(value: &Value) -> String {
        value_to_sql_literal(value)
    }

    fn quote_identifier(name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }

    /// Separate params into known SQL columns and extra properties.
    /// Extra properties get merged into the `properties` JSON column.
    /// If params already contains a `properties` field, its JSON content is
    /// merged with the extra properties.
    fn partition_params(
        &self,
        params: &StorageEntity,
    ) -> (
        Vec<(String, String)>,
        std::collections::HashMap<String, Value>,
    ) {
        let mut sql_fields = Vec::new();
        let mut extra_props = std::collections::HashMap::new();
        let mut existing_properties_json: Option<String> = None;

        for (key, value) in params.iter() {
            if key == "properties" {
                // Capture existing properties JSON to merge with extras later
                if let Some(s) = value.as_string() {
                    existing_properties_json = Some(s.to_string());
                }
            } else if self.known_columns.contains(key.as_str()) {
                sql_fields.push((key.clone(), Self::value_to_sql(value)));
            } else {
                extra_props.insert(key.clone(), value.clone());
            }
        }

        // Merge existing properties JSON into extra_props
        if let Some(json_str) = existing_properties_json {
            if let Ok(map) = serde_json::from_str::<
                std::collections::HashMap<String, serde_json::Value>,
            >(&json_str)
            {
                for (k, v) in map {
                    if !extra_props.contains_key(&k) {
                        let value = match v {
                            serde_json::Value::String(s) => Value::String(s),
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    Value::Integer(i)
                                } else {
                                    Value::Float(n.as_f64().unwrap_or(0.0))
                                }
                            }
                            serde_json::Value::Bool(b) => Value::Boolean(b),
                            _ => Value::String(v.to_string()),
                        };
                        extra_props.insert(k, value);
                    }
                }
            }
        }

        (sql_fields, extra_props)
    }

    /// Execute a prepared operation: run SQL statements and publish events.
    async fn execute_prepared(&self, prepared: PreparedOp) -> Result<()> {
        for sql in &prepared.sql_statements {
            self.db_handle
                .execute(sql, vec![])
                .await
                .map_err(|e| format!("Failed to execute SQL: {}", e))?;
        }
        for event in prepared.events {
            if let Some(ref bus) = self.event_bus {
                if let Err(e) = bus.publish(event, None).await {
                    tracing::warn!("[SqlOperationProvider] Failed to publish event: {}", e);
                }
            }
        }
        Ok(())
    }

    fn make_event(
        &self,
        kind: EventKind,
        aggregate_id: &str,
        payload: HashMap<String, serde_json::Value>,
    ) -> Event {
        Event::new(
            kind,
            self.aggregate_type(),
            aggregate_id,
            EventOrigin::Other("sql".to_string()),
            payload,
        )
    }

    /// Build SQL + event for a create operation without executing.
    fn prepare_create(&self, params: &StorageEntity) -> PreparedOp {
        let (mut sql_fields, extra_props) = self.partition_params(params);

        if !extra_props.is_empty() {
            let props_json = serde_json::to_string(
                &extra_props
                    .into_iter()
                    .map(|(k, v)| {
                        let json_val: serde_json::Value = match v {
                            Value::String(s) => serde_json::Value::String(s),
                            Value::Integer(i) => serde_json::json!(i),
                            Value::Float(f) => serde_json::json!(f),
                            Value::Boolean(b) => serde_json::json!(b),
                            _ => serde_json::Value::String(format!("{:?}", v)),
                        };
                        (k, json_val)
                    })
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            )
            .unwrap_or_else(|_| "{}".to_string());
            sql_fields.push((
                "properties".to_string(),
                format!("'{}'", props_json.replace('\'', "''")),
            ));
        }

        let columns: Vec<_> = sql_fields
            .iter()
            .map(|(k, _)| Self::quote_identifier(k))
            .collect();
        let values: Vec<_> = sql_fields.iter().map(|(_, v)| v.clone()).collect();

        let sql = format!(
            "INSERT OR REPLACE INTO {} ({}) VALUES ({})",
            self.table_name,
            columns.join(", "),
            values.join(", ")
        );

        let aggregate_id = params
            .get("id")
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let payload = self.build_event_payload(params);
        let event = self.make_event(EventKind::Created, &aggregate_id, payload);

        PreparedOp {
            sql_statements: vec![sql],
            events: vec![event],
        }
    }

    /// Build SQL + event for an update operation without executing.
    /// Returns None if there are no fields to update.
    fn prepare_update(&self, params: &StorageEntity) -> Option<PreparedOp> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("SqlOperationProvider::prepare_update: missing 'id' parameter");

        let (sql_fields, extra_props) = self.partition_params(params);

        let mut set_clauses: Vec<String> = sql_fields
            .iter()
            .filter(|(k, _)| k != "id")
            .map(|(k, v)| format!("{} = {}", Self::quote_identifier(k), v))
            .collect();

        if !extra_props.is_empty() {
            let props_json = serde_json::to_string(
                &extra_props
                    .into_iter()
                    .map(|(k, v)| {
                        let json_val: serde_json::Value = match v {
                            Value::String(s) => serde_json::Value::String(s),
                            Value::Integer(i) => serde_json::json!(i),
                            Value::Float(f) => serde_json::json!(f),
                            Value::Boolean(b) => serde_json::json!(b),
                            _ => serde_json::Value::String(format!("{:?}", v)),
                        };
                        (k, json_val)
                    })
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            )
            .unwrap_or_else(|_| "{}".to_string());
            set_clauses.push(format!(
                "\"properties\" = '{}'",
                props_json.replace('\'', "''")
            ));
        }

        if set_clauses.is_empty() {
            return None;
        }

        let sql = format!(
            "UPDATE {} SET {} WHERE id = '{}'",
            self.table_name,
            set_clauses.join(", "),
            id.replace('\'', "''")
        );

        let payload = self.build_event_payload(params);
        let event = self.make_event(EventKind::Updated, &id, payload);

        Some(PreparedOp {
            sql_statements: vec![sql],
            events: vec![event],
        })
    }

    /// Build SQL + events for a delete operation (with cascade) without executing.
    /// Requires async because cascade discovery queries the DB.
    async fn prepare_delete(&self, params: &StorageEntity) -> Result<PreparedOp> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "Missing 'id' parameter".to_string())?;

        let doc_uri = self.find_document_uri(&id).await;

        let mut queue = vec![id.to_string()];
        let mut all_ids = Vec::new();
        while let Some(parent) = queue.pop() {
            let children_sql = format!(
                "SELECT id FROM {} WHERE parent_id = '{}'",
                self.table_name,
                parent.replace('\'', "''")
            );
            let children: Vec<String> = self
                .db_handle
                .query(&children_sql, HashMap::new())
                .await
                .map_err(|e| format!("Failed to query children: {}", e))?
                .into_iter()
                .filter_map(|row| {
                    row.get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                })
                .collect();
            queue.extend(children.iter().cloned());
            all_ids.extend(children);
        }

        let delete_payload = match &doc_uri {
            Some(uri) => {
                let mut p = HashMap::new();
                let mut data = serde_json::Map::new();
                data.insert(
                    "parent_id".to_string(),
                    serde_json::Value::String(uri.clone()),
                );
                p.insert("data".to_string(), serde_json::Value::Object(data));
                p
            }
            None => HashMap::new(),
        };

        let mut sql_statements = Vec::new();
        let mut events = Vec::new();

        // Delete descendants bottom-up
        for desc_id in all_ids.iter().rev() {
            sql_statements.push(format!(
                "DELETE FROM {} WHERE id = '{}'",
                self.table_name,
                desc_id.replace('\'', "''")
            ));
            events.push(self.make_event(EventKind::Deleted, desc_id, delete_payload.clone()));
        }

        // Delete the target block itself
        sql_statements.push(format!(
            "DELETE FROM {} WHERE id = '{}'",
            self.table_name,
            id.replace('\'', "''")
        ));
        events.push(self.make_event(EventKind::Deleted, &id, delete_payload));

        Ok(PreparedOp {
            sql_statements,
            events,
        })
    }

    /// Build an event payload from params that CacheEventSubscriber can deserialize as Block.
    ///
    /// The problem: `params` is flat — extra properties like `collapse-to` and `column-order`
    /// are top-level keys. But `Block` expects them nested under a `properties` key.
    /// Without this restructuring, `serde_json::from_value::<Block>` drops unknown keys
    /// and `properties` gets `{}` via `#[serde(default)]`, causing INSERT OR REPLACE
    /// in QueryableCache to overwrite the correct SQL data with empty properties.
    fn build_event_payload(&self, params: &StorageEntity) -> HashMap<String, serde_json::Value> {
        let mut payload = HashMap::new();

        let mut data_map = serde_json::Map::new();
        let mut props_map = serde_json::Map::new();

        for (key, value) in params.iter() {
            let json_val = match value {
                Value::String(s) => serde_json::Value::String(s.clone()),
                Value::Integer(i) => serde_json::json!(i),
                Value::Float(f) => serde_json::json!(f),
                Value::Boolean(b) => serde_json::json!(b),
                Value::Null => serde_json::Value::Null,
                other => serde_json::Value::String(format!("{:?}", other)),
            };

            if key == "properties" {
                // Existing properties JSON — merge into props_map
                if let Some(s) = value.as_string() {
                    if let Ok(map) =
                        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&s)
                    {
                        for (k, v) in map {
                            props_map.insert(k, v);
                        }
                    }
                }
            } else if self.known_columns.contains(key.as_str()) {
                data_map.insert(key.clone(), json_val);
            } else {
                // Extra property — nest under `properties`
                props_map.insert(key.clone(), json_val);
            }
        }

        if !props_map.is_empty() {
            data_map.insert(
                "properties".to_string(),
                serde_json::Value::Object(props_map),
            );
        }

        payload.insert("data".to_string(), serde_json::Value::Object(data_map));
        payload
    }
}

#[async_trait]
impl OperationProvider for SqlOperationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                id_column: "id".to_string(),
                name: "set_field".to_string(),
                display_name: "Set Field".to_string(),
                description: format!("Set a field on {}", self.entity_short_name),
                required_params: vec![
                    OperationParam {
                        name: "id".to_string(),
                        type_hint: TypeHint::String,
                        description: "Entity ID".to_string(),
                    },
                    OperationParam {
                        name: "field".to_string(),
                        type_hint: TypeHint::String,
                        description: "Field name".to_string(),
                    },
                    OperationParam {
                        name: "value".to_string(),
                        type_hint: TypeHint::String,
                        description: "Field value".to_string(),
                    },
                ],
                affected_fields: vec![],
                param_mappings: vec![],
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                id_column: "id".to_string(),
                name: "create".to_string(),
                display_name: "Create".to_string(),
                description: format!("Create a new {}", self.entity_short_name),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                id_column: "id".to_string(),
                name: "update".to_string(),
                display_name: "Update".to_string(),
                description: format!("Update {}", self.entity_short_name),
                required_params: vec![OperationParam {
                    name: "id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Entity ID".to_string(),
                }],
                affected_fields: vec![],
                param_mappings: vec![],
                precondition: None,
            },
            OperationDescriptor {
                entity_name: self.entity_name.clone().into(),
                entity_short_name: self.entity_short_name.clone(),
                id_column: "id".to_string(),
                name: "delete".to_string(),
                display_name: "Delete".to_string(),
                description: format!("Delete {}", self.entity_short_name),
                required_params: vec![OperationParam {
                    name: "id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Entity ID".to_string(),
                }],
                affected_fields: vec![],
                param_mappings: vec![],
                precondition: None,
            },
        ]
    }

    async fn execute_operation(
        &self,
        entity_name: &str,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        assert_eq!(
            entity_name, self.entity_name,
            "SqlOperationProvider: expected entity '{}', got '{}'",
            self.entity_name, entity_name
        );

        match op_name {
            "set_field" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "Missing 'id' parameter".to_string())?;
                let field = params
                    .get("field")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| "Missing 'field' parameter".to_string())?;
                let value = params
                    .get("value")
                    .ok_or_else(|| "Missing 'value' parameter".to_string())?;

                let sql_value = Self::value_to_sql(value);

                let sql = if self.known_columns.contains(&*field) {
                    format!(
                        "UPDATE {} SET {} = {} WHERE id = '{}'",
                        self.table_name,
                        Self::quote_identifier(&field),
                        sql_value,
                        id.replace('\'', "''")
                    )
                } else {
                    format!(
                        "UPDATE {} SET properties = json_set(COALESCE(properties, '{{}}'), '$.{}', {}) WHERE id = '{}'",
                        self.table_name,
                        field.replace('\'', "''"),
                        sql_value,
                        id.replace('\'', "''")
                    )
                };
                self.db_handle
                    .execute(&sql, vec![])
                    .await
                    .map_err(|e| format!("Failed to execute SQL: {}", e))?;

                let mut payload = HashMap::new();
                let fields_json =
                    serde_json::to_value(vec![(&field, &Value::Null, value)]).unwrap_or_default();
                payload.insert("fields".to_string(), fields_json);
                self.publish_event(EventKind::FieldsChanged, &id, payload)
                    .await;

                Ok(OperationResult::irreversible(Vec::new()))
            }
            "create" => {
                let prepared = self.prepare_create(&params);
                self.execute_prepared(prepared).await?;
                Ok(OperationResult::irreversible(Vec::new()))
            }
            "update" => {
                if let Some(prepared) = self.prepare_update(&params) {
                    self.execute_prepared(prepared).await?;
                }
                Ok(OperationResult::irreversible(Vec::new()))
            }
            "delete" => {
                let prepared = self.prepare_delete(&params).await?;
                self.execute_prepared(prepared).await?;
                Ok(OperationResult::irreversible(Vec::new()))
            }
            _ => Err(format!("Unknown operation: {}", op_name).into()),
        }
    }

    /// Execute multiple operations in a single transaction with batch event publishing.
    ///
    /// All SQL statements are wrapped in one transaction (single CDC/IVM pass).
    /// All events are published in a single batch after the transaction commits.
    async fn execute_batch(
        &self,
        entity_name: &str,
        operations: Vec<(String, StorageEntity)>,
    ) -> Result<Vec<OperationResult>> {
        assert_eq!(
            entity_name, self.entity_name,
            "SqlOperationProvider: expected entity '{}', got '{}'",
            self.entity_name, entity_name
        );

        if operations.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 1: Prepare all operations (may involve async DB reads for delete cascade)
        let mut all_sql = Vec::new();
        let mut all_events = Vec::new();

        for (op_name, params) in &operations {
            let prepared = match op_name.as_str() {
                "create" => self.prepare_create(params),
                "update" => match self.prepare_update(params) {
                    Some(p) => p,
                    None => continue,
                },
                "delete" => self.prepare_delete(params).await?,
                other => return Err(format!("Unknown batch operation: {}", other).into()),
            };
            all_sql.extend(prepared.sql_statements.into_iter().map(|s| (s, vec![])));
            all_events.extend(prepared.events);
        }

        let count = operations.len();

        // Phase 2: Execute all SQL in a single transaction
        tracing::info!(
            "[SqlOperationProvider] Executing batch: {} operations, {} SQL statements",
            count,
            all_sql.len()
        );
        self.db_handle
            .transaction(all_sql)
            .await
            .map_err(|e| format!("Batch transaction failed: {}", e))?;

        // Phase 3: Publish all events in a single batch
        if let Some(ref bus) = self.event_bus {
            bus.publish_batch(all_events)
                .await
                .map_err(|e| format!("Batch event publish failed: {}", e))?;
        }

        Ok(vec![OperationResult::irreversible(Vec::new()); count])
    }
}
