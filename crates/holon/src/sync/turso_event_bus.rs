//! Turso-based EventBus implementation
//!
//! Uses Turso CDC (Change Data Capture) for event subscription.
//!
//! Watermark and consumer_position are backed by a materialized view
//! (`mv_events_watermark`) so CDC delivers push-based updates; the
//! trait methods read from in-process signals (no SQL round-trip).

use async_trait::async_trait;
use futures_signals::signal::Mutable;
use futures_signals::signal_map::MutableBTreeMap;
use serde_json;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tracing;

use crate::storage::DbHandle;
use crate::storage::types::{Result, StorageError};
use crate::sync::event_bus::{
    Event, EventBus, EventFilter, EventId, EventOrigin, EventStatus, EventStream,
};
use holon_api::Value;

const WATERMARK_VIEW: &str = "mv_events_watermark";

/// Watermark state backed by CDC on `mv_events_watermark`.
#[derive(Clone)]
pub struct WatermarkState {
    pub global: Mutable<i64>,
    pub by_consumer: MutableBTreeMap<String, i64>,
}

impl WatermarkState {
    /// Start the CDC listener and bootstrap current values from SQL.
    ///
    /// Call after `TursoEventBus::init_schema()` so the matview exists.
    pub async fn start(db_handle: &DbHandle) -> Result<Self> {
        let state = Self {
            global: Mutable::new(0),
            by_consumer: MutableBTreeMap::new(),
        };

        // Subscribe to CDC _before_ bootstrap so nothing is missed.
        let mut cdc_stream = db_handle.row_changes();

        // Bootstrap: single query to seed current values.
        let bootstrap_sql = "SELECT \
            MAX(created_at) AS global_ts, \
            MAX(CASE WHEN processed_by_loro = 1 THEN created_at END) AS loro_ts, \
            MAX(CASE WHEN processed_by_org  = 1 THEN created_at END) AS org_ts, \
            MAX(CASE WHEN processed_by_cache = 1 THEN created_at END) AS cache_ts \
            FROM events";
        if let Ok(rows) = db_handle.query(bootstrap_sql, HashMap::new()).await {
            if let Some(row) = rows.into_iter().next() {
                let read_i64 = |key: &str| -> i64 {
                    row.get(key)
                        .and_then(|v| match v {
                            Value::Integer(i) => Some(*i),
                            _ => None,
                        })
                        .unwrap_or(0)
                };
                *state.global.lock_mut() = read_i64("global_ts");
                let mut by_consumer = state.by_consumer.lock_mut();
                for (consumer, col) in [
                    ("loro", "loro_ts"),
                    ("org", "org_ts"),
                    ("cache", "cache_ts"),
                ] {
                    let ts = read_i64(col);
                    if ts > 0 {
                        by_consumer.insert_cloned(consumer.to_string(), ts);
                    }
                }
            }
        }

        // Spawn background task that applies CDC increments.
        let state_clone = state.clone();
        crate::util::spawn_actor(async move {
            while let Some(batch) = cdc_stream.next().await {
                for rc in &batch.inner.items {
                    if rc.relation_name != WATERMARK_VIEW {
                        continue;
                    }
                    state_clone.apply_cdc(&rc.change);
                }
            }
            tracing::debug!("[WatermarkState] CDC stream closed");
        });

        Ok(state)
    }

    fn bump_global(&self, ts: i64) {
        let mut g = self.global.lock_mut();
        if ts > *g {
            *g = ts;
        }
    }

    fn bump_consumer(&self, consumer: &str, ts: i64) {
        let mut map = self.by_consumer.lock_mut();
        let cur = map.get(consumer).copied().unwrap_or(0);
        if ts > cur {
            map.insert_cloned(consumer.to_string(), ts);
        }
    }

    fn apply_cdc(&self, change: &crate::storage::turso::ChangeData) {
        use crate::storage::turso::ChangeData;
        match change {
            ChangeData::Created { data, .. } | ChangeData::Updated { data, .. } => {
                let ts = data
                    .get("created_at")
                    .and_then(|v| match v {
                        Value::Integer(i) => Some(*i),
                        _ => None,
                    })
                    .unwrap_or(0);
                if ts > 0 {
                    self.bump_global(ts);
                }
                for (consumer, col) in [
                    ("loro", "processed_by_loro"),
                    ("org", "processed_by_org"),
                    ("cache", "processed_by_cache"),
                ] {
                    let is_processed = data
                        .get(col)
                        .and_then(|v| match v {
                            Value::Integer(i) => Some(*i == 1),
                            _ => None,
                        })
                        .unwrap_or(false);
                    if is_processed {
                        self.bump_consumer(consumer, ts);
                    }
                }
            }
            ChangeData::Deleted { .. } | ChangeData::FieldsChanged { .. } => {}
        }
    }
}

/// Turso-based EventBus implementation
pub struct TursoEventBus {
    db_handle: DbHandle,
    watermark_state: WatermarkState,
}

impl TursoEventBus {
    pub fn new(db_handle: DbHandle, watermark_state: WatermarkState) -> Self {
        Self {
            db_handle,
            watermark_state,
        }
    }

    /// Reactive signal of the global watermark (max `created_at`).
    pub fn watermark_signal(&self) -> impl futures_signals::signal::Signal<Item = i64> {
        self.watermark_state.global.signal()
    }

    /// Run DDL to create events table, indexes, and watermark matview.
    ///
    /// Call once before constructing `TursoEventBus` and starting `WatermarkState`.
    pub async fn init_schema(db_handle: &DbHandle) -> Result<()> {
        for stmt in crate::storage::sql_statements(include_str!("../../sql/schema/events.sql")) {
            db_handle.execute_ddl(stmt).await.map_err(|e| {
                StorageError::DatabaseError(format!("Failed to execute events schema DDL: {}", e))
            })?;
        }

        db_handle
            .execute_ddl(include_str!("../../sql/schema/mv_events_watermark.sql"))
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to create {WATERMARK_VIEW}: {e}"))
            })?;

        tracing::info!("[TursoEventBus] Schema initialized");
        Ok(())
    }

    /// Parse a StorageEntity (query result row) into an Event.
    ///
    /// Direct queries return `payload` as a deserialized Value (Object/Array),
    /// but the CDC-based parser expects it as a JSON string. We normalize here.
    pub fn parse_event_row(row: &crate::storage::StorageEntity) -> Result<Event> {
        let mut data = row.clone();
        // Normalize payload: CDC delivers it as Value::String (JSON text),
        // but direct SQL queries deserialize it into Value::Object/Array.
        if let Some(val) = data.get("payload") {
            if !matches!(val, holon_api::Value::String(_)) {
                let json_str = serde_json::to_string(&val).map_err(|e| {
                    StorageError::SerializationError(format!("serialize payload: {e}"))
                })?;
                data.insert("payload".to_string(), holon_api::Value::String(json_str));
            }
        }
        Self::parse_row_change_to_event(&crate::storage::turso::ChangeData::Created {
            data,
            origin: holon_api::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        })
    }

    /// Parse a RowChange (Change<StorageEntity>) into an Event
    fn parse_row_change_to_event(change: &crate::storage::turso::ChangeData) -> Result<Event> {
        use crate::storage::turso::ChangeData;
        use holon_api::Value;

        match change {
            ChangeData::Created { data, .. } | ChangeData::Updated { data, .. } => {
                // Extract fields from StorageEntity
                let id = data
                    .get("id")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        StorageError::DatabaseError("Missing 'id' in event row".to_string())
                    })?;

                let event_type_str = data
                    .get("event_type")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        StorageError::DatabaseError("Missing 'event_type' in event row".to_string())
                    })?;

                let (aggregate_type, event_kind) = Event::parse_event_type_string(&event_type_str)
                    .map_err(|e| {
                        StorageError::DatabaseError(format!(
                            "Invalid event_type '{}': {}",
                            event_type_str, e
                        ))
                    })?;

                let aggregate_id = data
                    .get("aggregate_id")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        StorageError::DatabaseError(
                            "Missing 'aggregate_id' in event row".to_string(),
                        )
                    })?;

                let origin_str = data
                    .get("origin")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        StorageError::DatabaseError("Missing 'origin' in event row".to_string())
                    })?;

                let status_str = data
                    .get("status")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "confirmed".to_string());

                let payload_json = data
                    .get("payload")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        StorageError::DatabaseError("Missing 'payload' in event row".to_string())
                    })?;

                let payload: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&payload_json).map_err(|e| {
                        StorageError::SerializationError(format!(
                            "Failed to parse payload JSON: {}",
                            e
                        ))
                    })?;

                let trace_id = data.get("trace_id").and_then(|v| match v {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                });

                let command_id = data.get("command_id").and_then(|v| match v {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                });

                let created_at = data
                    .get("created_at")
                    .and_then(|v| match v {
                        Value::Integer(i) => Some(*i),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        StorageError::DatabaseError("Missing 'created_at' in event row".to_string())
                    })?;

                let speculative_id = data.get("speculative_id").and_then(|v| match v {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                });

                let rejection_reason = data.get("rejection_reason").and_then(|v| match v {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                });

                let origin = EventOrigin::from_str(&origin_str);
                let status = EventStatus::from_str(&status_str).unwrap_or_else(|| {
                    panic!("stored event status must be valid, got: '{}'", status_str)
                });

                Ok(Event {
                    id,
                    event_kind,
                    aggregate_type,
                    aggregate_id,
                    origin,
                    status,
                    payload,
                    trace_id,
                    command_id,
                    created_at,
                    speculative_id,
                    rejection_reason,
                })
            }
            ChangeData::Deleted { id, .. } => {
                // For deleted events, we can't reconstruct the full event
                // This shouldn't happen in practice (events table is append-only)
                Err(StorageError::DatabaseError(format!(
                    "Unexpected DELETE event for event ID: {}",
                    id
                )))
            }
            ChangeData::FieldsChanged { .. } => {
                // FieldsChanged is not used for events table (events are immutable)
                Err(StorageError::DatabaseError(
                    "Unexpected FieldsChanged event for events table".to_string(),
                ))
            }
        }
    }

    /// Check if an event matches the filter criteria
    fn event_matches_filter(event: &Event, filter: &EventFilter) -> bool {
        // Filter by origin
        if !filter.origins.is_empty() {
            if !filter
                .origins
                .iter()
                .any(|o| o.as_str() == event.origin.as_str())
            {
                return false;
            }
        }

        // Filter by status
        if !filter.statuses.is_empty() {
            if !filter.statuses.iter().any(|s| *s == event.status) {
                return false;
            }
        }

        // Filter by aggregate type
        if !filter.aggregate_types.is_empty() {
            if !filter
                .aggregate_types
                .iter()
                .any(|t| *t == event.aggregate_type)
            {
                return false;
            }
        }

        // Filter by timestamp
        if let Some(after_timestamp) = filter.after_timestamp {
            if event.created_at <= after_timestamp {
                return false;
            }
        }

        true
    }

    const INSERT_EVENT_SQL: &'static str = include_str!("../../sql/events/insert_event.sql");

    /// Convert an Event to SQL parameters
    fn event_to_params(event: &Event, payload_json: &str) -> Vec<turso::Value> {
        vec![
            turso::Value::Text(event.id.clone()),
            turso::Value::Text(event.event_type_string()),
            turso::Value::Text(event.aggregate_type.as_str().to_string()),
            turso::Value::Text(event.aggregate_id.clone()),
            turso::Value::Text(event.origin.as_str().to_string()),
            turso::Value::Text(event.status.as_str().to_string()),
            turso::Value::Text(payload_json.to_string()),
            event
                .trace_id
                .clone()
                .map(turso::Value::Text)
                .unwrap_or(turso::Value::Null),
            event
                .command_id
                .clone()
                .map(turso::Value::Text)
                .unwrap_or(turso::Value::Null),
            turso::Value::Integer(event.created_at),
            event
                .speculative_id
                .clone()
                .map(turso::Value::Text)
                .unwrap_or(turso::Value::Null),
            event
                .rejection_reason
                .clone()
                .map(turso::Value::Text)
                .unwrap_or(turso::Value::Null),
        ]
    }
}

#[async_trait]
impl EventBus for TursoEventBus {
    async fn publish(&self, event: Event, command_id: Option<EventId>) -> Result<EventId> {
        let payload_json = serde_json::to_string(&event.payload).map_err(|e| {
            StorageError::SerializationError(format!("Failed to serialize payload: {}", e))
        })?;

        let mut event = event;
        if let Some(cmd_id) = command_id {
            event.command_id = Some(cmd_id);
        }

        let event_id = event.id.clone();
        let event_type_str = event.event_type_string();
        let params = Self::event_to_params(&event, &payload_json);

        self.db_handle
            .execute(Self::INSERT_EVENT_SQL, params)
            .await
            .map_err(|e| StorageError::DatabaseError(format!("Failed to insert event: {}", e)))?;

        tracing::debug!("[TursoEventBus] Published event: {}", event_id);
        tracing::debug!(
            "[TursoEventBus::publish] Published event id={}, type={}",
            event_id,
            event_type_str
        );
        Ok(event_id)
    }

    async fn publish_batch(&self, events: Vec<Event>) -> Result<Vec<EventId>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        tracing::debug!(
            "[TursoEventBus] Publishing batch of {} events",
            events.len()
        );

        let mut statements = Vec::with_capacity(events.len());
        let mut event_ids = Vec::with_capacity(events.len());

        for event in &events {
            let payload_json = serde_json::to_string(&event.payload).map_err(|e| {
                StorageError::SerializationError(format!("Failed to serialize payload: {}", e))
            })?;
            let params = Self::event_to_params(event, &payload_json);
            event_ids.push(event.id.clone());
            statements.push((Self::INSERT_EVENT_SQL.to_string(), params));
        }

        self.db_handle.transaction(statements).await.map_err(|e| {
            StorageError::DatabaseError(format!("Failed to insert event batch: {}", e))
        })?;

        tracing::debug!(
            "[TursoEventBus] Published batch of {} events",
            event_ids.len()
        );
        Ok(event_ids)
    }

    async fn subscribe(&self, filter: EventFilter) -> Result<EventStream> {
        // Generate unique view name from filter (CDC only works with materialized views)
        // Include origin in view name to ensure different filters get different views
        let origin_suffix = filter
            .origins
            .first()
            .map(|o| format!("_{}", o.as_str()))
            .unwrap_or_default();
        let view_name = format!(
            "events_view_{}{}",
            filter
                .aggregate_types
                .first()
                .map(|t| t.as_str())
                .unwrap_or("all"),
            origin_suffix
        );

        // Build WHERE clause from filter
        // NOTE: Turso materialized views only support simple predicates: column = 'value' or column = column
        //       NOT supported: 1=1, IN(...), OR, etc.
        // For single values we use: column = 'value'
        // For multiple values we need multiple views or use the first value only
        let mut where_clauses = Vec::new();

        // Status filter - use first status only (Turso limitation)
        if let Some(status) = filter.statuses.first() {
            where_clauses.push(format!("status = '{}'", status.as_str()));
        }

        // Aggregate type filter - use first type only (Turso limitation)
        if let Some(agg_type) = filter.aggregate_types.first() {
            where_clauses.push(format!("aggregate_type = '{}'", agg_type.as_str()));
        }

        // Origin filter - use first origin only (Turso limitation)
        if let Some(origin) = filter.origins.first() {
            where_clauses.push(format!("origin = '{}'", origin.as_str()));
        }

        // If no filters, select all events
        let where_clause = if where_clauses.is_empty() {
            // Turso requires a WHERE clause for materialized views, use a tautology
            // that it can parse: id = id (column = column is supported)
            "id = id".to_string()
        } else {
            where_clauses.join(" AND ")
        };

        // Create materialized view for this subscription
        // Since view_name is deterministic (based on filter), we can reuse existing views
        let create_view_sql = format!(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS SELECT * FROM events WHERE {}",
            view_name, where_clause
        );

        // IMPORTANT: Create the materialized view on the ACTOR's connection (or write connection).
        // All DDL should go through the actor to prevent schema change errors.

        // Check if view already exists first (using execute_sql which routes through actor)
        let check_view_sql = format!(
            "SELECT name FROM sqlite_master WHERE type='view' AND name='{}'",
            view_name
        );
        let view_exists = match self
            .db_handle
            .query(&check_view_sql, std::collections::HashMap::new())
            .await
        {
            Ok(results) => !results.is_empty(),
            Err(_) => false,
        };

        if view_exists {
            tracing::debug!(
                "[TursoEventBus::subscribe] View {} already exists, reusing (skipping DDL)",
                view_name
            );
        } else {
            tracing::debug!(
                "[TursoEventBus::subscribe] Creating materialized view: {}",
                create_view_sql
            );
            // Use execute_ddl which routes through the actor when available
            self.db_handle.execute_ddl(&create_view_sql).await?;
            tracing::debug!("[TursoEventBus::subscribe] CREATE VIEW succeeded");
        }

        // Now set up CDC stream for watching the view (write connection already exists)

        let mut cdc_stream = self.db_handle.row_changes();
        tracing::debug!(
            "[TursoEventBus::subscribe] CDC stream established (connection managed by TursoBackend)"
        );

        let (tx, rx) = mpsc::channel(1024);
        let filter_clone = filter.clone();
        let view_name_clone = view_name.clone();

        // Spawn task to parse CDC events and apply filter
        // Track already-delivered event IDs to prevent re-delivery when mark_processed updates the row
        let mut delivered_event_ids = std::collections::HashSet::new();

        tokio::spawn(async move {
            tracing::debug!(
                "[TursoEventBus::subscribe] CDC listener task started for view: {}",
                view_name_clone
            );
            while let Some(batch) = cdc_stream.next().await {
                tracing::debug!(
                    "[TursoEventBus::subscribe] CDC received batch with {} items",
                    batch.items.len()
                );
                for row_change in &batch.items {
                    // Only process events from our materialized view
                    if row_change.relation_name != view_name_clone {
                        continue;
                    }

                    // Parse RowChange into Event
                    match TursoEventBus::parse_row_change_to_event(&row_change.change) {
                        Ok(event) => {
                            // Skip events we've already delivered (prevents re-delivery when mark_processed updates the row)
                            if delivered_event_ids.contains(&event.id) {
                                tracing::debug!(
                                    "[TursoEventBus::subscribe] DEDUP SKIP event={} type={}.{} view={}",
                                    event.id,
                                    event.aggregate_type,
                                    event.event_kind,
                                    view_name_clone
                                );
                                continue;
                            }

                            let matches =
                                TursoEventBus::event_matches_filter(&event, &filter_clone);
                            tracing::debug!(
                                "[TursoEventBus::subscribe] PARSED event={} type={}.{} matches={} view={}",
                                event.id,
                                event.aggregate_type,
                                event.event_kind,
                                matches,
                                view_name_clone
                            );
                            // Apply filter
                            if matches {
                                // Remember this event was delivered
                                delivered_event_ids.insert(event.id.clone());
                                if tx.send(event).await.is_err() {
                                    tracing::debug!(
                                        "[TursoEventBus] Event stream receiver closed for view={}",
                                        view_name_clone
                                    );
                                    break;
                                }
                                tracing::debug!(
                                    "[TursoEventBus::subscribe] SENT event to channel for view={}",
                                    view_name_clone
                                );
                            }
                        }
                        Err(e) => {
                            tracing::debug!(
                                "[TursoEventBus] PARSE FAILED for view={}: {}",
                                view_name_clone,
                                e
                            );
                        }
                    }
                }
            }
            tracing::info!("[TursoEventBus] CDC stream closed");
        });

        Ok(ReceiverStream::new(rx))
    }

    async fn watermark(&self) -> Result<i64> {
        Ok(self.watermark_state.global.get())
    }

    async fn consumer_position(&self, consumer: &str) -> Result<i64> {
        Ok(self
            .watermark_state
            .by_consumer
            .lock_ref()
            .get(consumer)
            .copied()
            .unwrap_or(0))
    }

    #[tracing::instrument(skip(self, event_id), fields(consumer = consumer), name = "events.mark_processed")]
    async fn mark_processed(&self, event_id: &EventId, consumer: &str) -> Result<()> {
        let column = match consumer {
            "loro" => "processed_by_loro",
            "org" => "processed_by_org",
            "cache" => "processed_by_cache",
            _ => {
                return Err(StorageError::DatabaseError(format!(
                    "Unknown consumer: {}",
                    consumer
                )));
            }
        };

        // Use execute_via_actor which routes through the database actor
        let sql = format!("UPDATE events SET {} = 1 WHERE id = ?", column);
        self.db_handle
            .execute(&sql, vec![turso::Value::Text(event_id.clone())])
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to mark event as processed: {}", e))
            })?;

        Ok(())
    }

    async fn update_status(
        &self,
        event_id: &EventId,
        status: EventStatus,
        rejection_reason: Option<String>,
    ) -> Result<()> {
        // Use execute_via_actor which routes through the database actor
        let rejection_reason_value = rejection_reason
            .map(|r| turso::Value::Text(r))
            .unwrap_or(turso::Value::Null);

        let sql = include_str!("../../sql/events/update_status.sql");
        self.db_handle
            .execute(
                sql,
                vec![
                    turso::Value::Text(status.as_str().to_string()),
                    rejection_reason_value,
                    turso::Value::Text(event_id.clone()),
                ],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to update event status: {}", e))
            })?;

        Ok(())
    }

    async fn link_speculative(
        &self,
        confirmed_event_id: &EventId,
        speculative_event_id: &EventId,
    ) -> Result<()> {
        // Use execute_via_actor which routes through the database actor
        let sql = include_str!("../../sql/events/link_speculative.sql");
        self.db_handle
            .execute(
                sql,
                vec![
                    turso::Value::Text(speculative_event_id.clone()),
                    turso::Value::Text(confirmed_event_id.clone()),
                ],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to link speculative event: {}", e))
            })?;

        Ok(())
    }
}
