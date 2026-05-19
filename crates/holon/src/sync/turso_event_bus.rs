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
    Consumer, Event, EventBus, EventFilter, EventId, EventOrigin, EventStatus, EventStream,
};
use holon_api::Value;

const GLOBAL_WATERMARK_VIEW: &str = "mv_events_global_watermark";
const ACKS_WATERMARK_VIEW: &str = "mv_event_acks_watermark";

/// Watermark state backed by CDC on the global and per-consumer ack matviews.
#[derive(Clone)]
pub struct WatermarkState {
    pub global: Mutable<i64>,
    pub by_consumer: MutableBTreeMap<String, i64>,
}

impl WatermarkState {
    /// Start the CDC listener and bootstrap current values from SQL.
    ///
    /// Call after `TursoEventBus::init_schema()` so the matviews exist.
    pub async fn start(db_handle: &DbHandle) -> Result<Self> {
        let state = Self {
            global: Mutable::new(0),
            by_consumer: MutableBTreeMap::new(),
        };

        // Subscribe to CDC _before_ bootstrap so nothing is missed.
        let mut cdc_stream = db_handle.row_changes();

        // Bootstrap global: max created_at across events.
        if let Ok(rows) = db_handle
            .query("SELECT MAX(created_at) AS ts FROM events", HashMap::new())
            .await
            && let Some(row) = rows.into_iter().next()
            && let Some(Value::Integer(ts)) = row.get("ts")
        {
            *state.global.lock_mut() = *ts;
        }

        // Bootstrap per-consumer: max acked_at per consumer from the ack table.
        if let Ok(rows) = db_handle
            .query(
                "SELECT consumer, MAX(acked_at) AS ts FROM event_acks GROUP BY consumer",
                HashMap::new(),
            )
            .await
        {
            let mut by_consumer = state.by_consumer.lock_mut();
            for row in rows {
                let consumer = match row.get("consumer") {
                    Some(Value::String(s)) => s.clone(),
                    _ => continue,
                };
                let ts = match row.get("ts") {
                    Some(Value::Integer(i)) => *i,
                    _ => continue,
                };
                if ts > 0 {
                    by_consumer.insert_cloned(consumer, ts);
                }
            }
        }

        // Spawn background task that applies CDC increments from both matviews.
        let state_clone = state.clone();
        crate::util::spawn_actor(async move {
            while let Some(batch) = cdc_stream.next().await {
                for rc in &batch.inner.items {
                    match rc.relation_name.as_str() {
                        GLOBAL_WATERMARK_VIEW => state_clone.apply_global_cdc(&rc.change),
                        ACKS_WATERMARK_VIEW => state_clone.apply_acks_cdc(&rc.change),
                        _ => {}
                    }
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

    fn apply_global_cdc(&self, change: &crate::storage::turso::ChangeData) {
        use crate::storage::turso::ChangeData;
        match change {
            ChangeData::Created { data, .. } | ChangeData::Updated { data, .. } => {
                if let Some(Value::Integer(ts)) = data.get("ts")
                    && *ts > 0
                {
                    self.bump_global(*ts);
                }
            }
            ChangeData::Deleted { .. } | ChangeData::FieldsChanged { .. } => {}
        }
    }

    /// Phase 4: wait until the watermark for `consumer` is at least `target`.
    /// Returns immediately if already satisfied. The current implementation
    /// polls the `Mutable<i64>` snapshot every 2 ms because
    /// `futures_signals::MutableBTreeMap` doesn't expose a per-key signal
    /// directly and the wrapper would need a fold over `SignalMap` deltas
    /// (an O(map_size) cost per emission). Polling a single `lock_ref`
    /// read is cheaper and matches the contract; bound this with
    /// `tokio::time::timeout` at the call site so a stuck consumer fails
    /// loud instead of hanging forever.
    pub async fn wait_for_consumer_ge(&self, consumer: &str, target: i64) {
        loop {
            let cur = self
                .by_consumer
                .lock_ref()
                .get(consumer)
                .copied()
                .unwrap_or(0);
            if cur >= target {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }

    fn apply_acks_cdc(&self, change: &crate::storage::turso::ChangeData) {
        use crate::storage::turso::ChangeData;
        match change {
            ChangeData::Created { data, .. } | ChangeData::Updated { data, .. } => {
                let consumer = match data.get("consumer") {
                    Some(Value::String(s)) => s.clone(),
                    _ => return,
                };
                if let Some(Value::Integer(ts)) = data.get("ts")
                    && *ts > 0
                {
                    self.bump_consumer(&consumer, *ts);
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
    /// Routes CDC batches to per-view subscribers via a single demux task,
    /// and owns matview lifecycle. Replaces the old pattern of every
    /// `subscribe()` call spawning its own broadcast filter task.
    matview_manager: std::sync::Arc<crate::sync::MatviewManager>,
}

impl TursoEventBus {
    pub fn new(
        db_handle: DbHandle,
        watermark_state: WatermarkState,
        matview_manager: std::sync::Arc<crate::sync::MatviewManager>,
    ) -> Self {
        Self {
            db_handle,
            watermark_state,
            matview_manager,
        }
    }

    /// Reactive signal of the global watermark (max `created_at`).
    pub fn watermark_signal(&self) -> impl futures_signals::signal::Signal<Item = i64> {
        self.watermark_state.global.signal()
    }

    /// Phase 4: wait until the ack watermark for `consumer` reaches `target`,
    /// bounded by `timeout_ms`. Returns `true` on caught-up, `false` on
    /// timeout. Replaces 10 ms `get_blocks().len() >= expected` polls with
    /// a 2 ms in-memory map check driven by CDC pushes.
    pub async fn wait_for_consumer_caught_up(
        &self,
        consumer: &str,
        target: i64,
        timeout_ms: u64,
    ) -> bool {
        let fut = self.watermark_state.wait_for_consumer_ge(consumer, target);
        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), fut).await {
            Ok(()) => true,
            Err(_) => false,
        }
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

        for stmt in
            crate::storage::sql_statements(include_str!("../../sql/schema/mv_events_watermark.sql"))
        {
            db_handle.execute_ddl(stmt).await.map_err(|e| {
                StorageError::DatabaseError(format!("Failed to create watermark matviews: {e}"))
            })?;
        }

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
        if let Some(val) = data.get("payload")
            && !matches!(val, holon_api::Value::String(_))
        {
            let json_str = serde_json::to_string(&val)
                .map_err(|e| StorageError::SerializationError(format!("serialize payload: {e}")))?;
            data.insert("payload".to_string(), holon_api::Value::String(json_str));
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

                let mut payload: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&payload_json).map_err(|e| {
                        StorageError::SerializationError(format!(
                            "Failed to parse payload JSON: {}",
                            e
                        ))
                    })?;

                // Lift the transport-key view of the typed positional intent
                // back into the [`Event::position_after_block_id`] field, then
                // strip the key from the payload so downstream consumers only
                // ever see the typed view.
                let position_after_block_id = payload
                    .remove(crate::sync::event_bus::POSITION_AFTER_BLOCK_ID_PAYLOAD_KEY)
                    .and_then(|v| match v {
                        serde_json::Value::String(s) if !s.is_empty() => Some(s),
                        _ => None,
                    });

                // Same round-trip for the typed document-routing intent.
                let routing_doc_uri = payload
                    .remove(crate::sync::event_bus::ROUTING_DOC_URI_PAYLOAD_KEY)
                    .and_then(|v| match v {
                        serde_json::Value::String(s) if !s.is_empty() => Some(s),
                        _ => None,
                    });

                let trace_id = data.get("trace_id").and_then(|v| match v {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                });

                let span_id = data.get("span_id").and_then(|v| match v {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                });

                let trace_flags = data
                    .get("trace_flags")
                    .and_then(|v| match v {
                        Value::Integer(i) => Some(*i as u8),
                        _ => None,
                    })
                    .unwrap_or(0);

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

                let origin = EventOrigin::parse_str(&origin_str);
                let status = EventStatus::parse_str(&status_str).unwrap_or_else(|| {
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
                    span_id,
                    trace_flags,
                    command_id,
                    created_at,
                    speculative_id,
                    rejection_reason,
                    position_after_block_id,
                    routing_doc_uri,
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
        if !filter.origins.is_empty()
            && !filter
                .origins
                .iter()
                .any(|o| o.as_str() == event.origin.as_str())
        {
            return false;
        }

        // Filter by status
        if !filter.statuses.is_empty() && !filter.statuses.contains(&event.status) {
            return false;
        }

        // Filter by aggregate type
        if !filter.aggregate_types.is_empty()
            && !filter.aggregate_types.contains(&event.aggregate_type)
        {
            return false;
        }

        // Filter by timestamp
        if let Some(after_timestamp) = filter.after_timestamp
            && event.created_at <= after_timestamp
        {
            return false;
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
                .span_id
                .clone()
                .map(turso::Value::Text)
                .unwrap_or(turso::Value::Null),
            turso::Value::Integer(event.trace_flags as i64),
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
        let mut event = event;
        if let Some(cmd_id) = command_id {
            event.command_id = Some(cmd_id);
        }

        // Flush the typed positional intent into the payload JSON under the
        // transport key. The events SQL table has a fixed column set; we
        // round-trip the typed field via the payload so that the CDC-stream
        // consumer can reconstruct it. See
        // `POSITION_AFTER_BLOCK_ID_PAYLOAD_KEY` for the contract.
        if let Some(ref after_id) = event.position_after_block_id {
            event.payload.insert(
                crate::sync::event_bus::POSITION_AFTER_BLOCK_ID_PAYLOAD_KEY.to_string(),
                serde_json::Value::String(after_id.clone()),
            );
        }
        // Same round-trip for the typed document-routing intent.
        if let Some(ref doc_uri) = event.routing_doc_uri {
            event.payload.insert(
                crate::sync::event_bus::ROUTING_DOC_URI_PAYLOAD_KEY.to_string(),
                serde_json::Value::String(doc_uri.clone()),
            );
        }

        let payload_json = serde_json::to_string(&event.payload).map_err(|e| {
            StorageError::SerializationError(format!("Failed to serialize payload: {}", e))
        })?;

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
            // Flush the typed positional intent into the payload before
            // serializing (same round-trip contract as `publish`). We clone
            // the payload locally rather than mutating the caller's Event,
            // since this is the only spot that needs the transport-key view.
            let mut payload = event.payload.clone();
            if let Some(ref after_id) = event.position_after_block_id {
                payload.insert(
                    crate::sync::event_bus::POSITION_AFTER_BLOCK_ID_PAYLOAD_KEY.to_string(),
                    serde_json::Value::String(after_id.clone()),
                );
            }
            if let Some(ref doc_uri) = event.routing_doc_uri {
                payload.insert(
                    crate::sync::event_bus::ROUTING_DOC_URI_PAYLOAD_KEY.to_string(),
                    serde_json::Value::String(doc_uri.clone()),
                );
            }
            let payload_json = serde_json::to_string(&payload).map_err(|e| {
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

    async fn subscribe(&self, filter: EventFilter, consumer: Consumer) -> Result<EventStream> {
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

        // Reconcile the named matview through MatviewManager's free function:
        // detects if a view with the same SELECT already exists (skipping DDL),
        // drops + recreates only if the SELECT changed.
        let select_sql = format!("SELECT * FROM events WHERE {}", where_clause);
        crate::sync::matview_manager::reconcile_named_view(
            &self.db_handle,
            &view_name,
            &select_sql,
        )
        .await
        .map_err(|e| {
            StorageError::DatabaseError(format!(
                "Failed to reconcile event matview {}: {}",
                view_name, e
            ))
        })?;

        // Register with the central CDC demux BEFORE querying replay rows.
        // The demux routes batches by `relation_name` to per-view subscribers
        // (matview_manager.rs:191), so this stream only sees events for OUR
        // view — no per-task filtering loop. Anything published between this
        // point and the SQL snapshot below will appear on both the demux
        // stream and the replay query — `delivered_event_ids` dedups the
        // overlap.
        let mut cdc_stream = self.matview_manager.subscribe_cdc(&view_name);
        tracing::debug!(
            "[TursoEventBus::subscribe] subscribed via MatviewManager demux for view: {}",
            view_name
        );

        // --- Replay: query unprocessed events for this consumer ----------------
        // Querying the base `events` table (not the matview) gives us a
        // deterministic snapshot of everything `consumer` hasn't acknowledged
        // yet. We mirror the matview's WHERE clause so replay matches what the
        // CDC stream will deliver going forward. "Unprocessed" = no row in
        // `event_acks` for this (event, consumer) pair.
        let mut replay_where = where_clauses.clone();
        replay_where.push(format!(
            "NOT EXISTS (SELECT 1 FROM event_acks a \
                          WHERE a.event_id = events.id AND a.consumer = '{}')",
            consumer.name()
        ));
        let replay_sql = format!(
            "SELECT events.* FROM events WHERE {} ORDER BY created_at, id",
            replay_where.join(" AND ")
        );
        tracing::debug!("[TursoEventBus::subscribe] replay SQL: {}", replay_sql);
        let replay_rows = self
            .db_handle
            .query(&replay_sql, std::collections::HashMap::new())
            .await?;

        let mut replay_events = Vec::with_capacity(replay_rows.len());
        let mut delivered_event_ids: std::collections::HashSet<EventId> =
            std::collections::HashSet::with_capacity(replay_rows.len());
        for row in &replay_rows {
            let event = TursoEventBus::parse_event_row(row)?;
            delivered_event_ids.insert(event.id.clone());
            replay_events.push(event);
        }
        tracing::info!(
            "[TursoEventBus::subscribe] consumer={} view={} replaying {} unprocessed events",
            consumer,
            view_name,
            replay_events.len()
        );

        let (tx, rx) = mpsc::channel(1024);
        let filter_clone = filter.clone();
        let view_name_clone = view_name.clone();

        // One task: drain the replay first, then forward broadcast events.
        // The replay sends fill the channel as the consumer drains it; broadcast
        // events arriving during replay sit in the broadcast buffer (capacity
        // 1024 — see crates/holon/src/storage/turso.rs) and are processed once
        // replay finishes. `delivered_event_ids` is pre-seeded so any event that
        // appears in both replay and broadcast is delivered exactly once.
        tokio::spawn(async move {
            for event in replay_events {
                if tx.send(event).await.is_err() {
                    tracing::debug!(
                        "[TursoEventBus::subscribe] receiver closed during replay for view={}",
                        view_name_clone
                    );
                    return;
                }
            }
            tracing::debug!(
                "[TursoEventBus::subscribe] replay drained, entering live loop for view: {}",
                view_name_clone
            );
            while let Some(batch) = cdc_stream.next().await {
                tracing::debug!(
                    "[TursoEventBus::subscribe] CDC received batch with {} items for view={}",
                    batch.items.len(),
                    view_name_clone
                );
                // The MatviewManager demux already routes by relation_name, so
                // every batch we see here is for our view — no per-item filter
                // needed.
                for row_change in &batch.items {
                    // Parse RowChange into Event
                    match TursoEventBus::parse_row_change_to_event(&row_change.change) {
                        Ok(event) => {
                            // Skip events we've already delivered (replay overlap or
                            // re-delivery when mark_processed updates the row)
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

    async fn consumer_position(&self, consumer: Consumer) -> Result<i64> {
        Ok(self
            .watermark_state
            .by_consumer
            .lock_ref()
            .get(consumer.name())
            .copied()
            .unwrap_or(0))
    }

    #[tracing::instrument(skip(self, event_id), fields(consumer = %consumer), name = "events.mark_processed")]
    async fn mark_processed(&self, event_id: &EventId, consumer: Consumer) -> Result<()> {
        // Insert-only into the side table — never mutates `events`, so the
        // `events_view_*` matviews don't see a delete+insert per ack.
        let now = chrono::Utc::now().timestamp_millis();
        let sql =
            "INSERT OR IGNORE INTO event_acks (event_id, consumer, acked_at) VALUES (?, ?, ?)";
        self.db_handle
            .execute(
                sql,
                vec![
                    turso::Value::Text(event_id.clone()),
                    turso::Value::Text(consumer.name().to_string()),
                    turso::Value::Integer(now),
                ],
            )
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to mark event as processed: {}", e))
            })?;

        Ok(())
    }

    #[tracing::instrument(skip(self, event_ids), fields(consumer = %consumer, batch_len = event_ids.len()), name = "events.mark_processed_batch")]
    async fn mark_processed_batch(&self, event_ids: &[EventId], consumer: Consumer) -> Result<()> {
        if event_ids.is_empty() {
            return Ok(());
        }
        // Single multi-VALUES INSERT — one statement, one matview re-eval,
        // instead of N round-trips through the DB actor.
        let now = chrono::Utc::now().timestamp_millis();
        let placeholders = std::iter::repeat_n("(?, ?, ?)", event_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "INSERT OR IGNORE INTO event_acks (event_id, consumer, acked_at) VALUES {}",
            placeholders
        );
        let consumer_name = consumer.name().to_string();
        let mut params: Vec<turso::Value> = Vec::with_capacity(event_ids.len() * 3);
        for id in event_ids {
            params.push(turso::Value::Text(id.clone()));
            params.push(turso::Value::Text(consumer_name.clone()));
            params.push(turso::Value::Integer(now));
        }
        self.db_handle.execute(&sql, params).await.map_err(|e| {
            StorageError::DatabaseError(format!(
                "Failed to mark {} events as processed: {}",
                event_ids.len(),
                e
            ))
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
            .map(turso::Value::Text)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::turso::TursoBackend;
    use crate::sync::event_bus::{AggregateType, Event, EventKind};
    use std::time::Duration;
    use tokio::time::timeout;

    async fn make_bus() -> TursoEventBus {
        let (_backend, db_handle) = TursoBackend::new_in_memory()
            .await
            .expect("create in-memory TursoBackend");
        TursoEventBus::init_schema(&db_handle)
            .await
            .expect("init events schema");
        let watermark_state = WatermarkState::start(&db_handle)
            .await
            .expect("start WatermarkState");
        let matview_manager = std::sync::Arc::new(crate::sync::MatviewManager::new(
            db_handle.clone(),
            std::sync::Arc::new(tokio::sync::Mutex::new(())),
        ));
        TursoEventBus::new(db_handle, watermark_state, matview_manager)
    }

    fn block_event(idx: u32) -> Event {
        let mut payload = HashMap::new();
        payload.insert("idx".to_string(), serde_json::Value::Number(idx.into()));
        Event::new(
            EventKind::Created,
            AggregateType::Block,
            format!("block-{idx}"),
            EventOrigin::Loro,
            payload,
        )
    }

    /// Subscribe-then-replay: a consumer that registers AFTER events were
    /// published must still receive every unprocessed event in created_at,id
    /// order.
    ///
    /// This is the regression test for the bootstrap race that caused PBT seed
    /// 8588447fcae7… to flake — `tokio::sync::broadcast::Sender::subscribe()`
    /// only delivers future messages, so any consumer that subscribes after a
    /// publish burst would silently miss those events.
    #[tokio::test]
    async fn subscribe_replays_unprocessed_events() {
        let bus = make_bus().await;

        let filter = EventFilter::new()
            .with_aggregate_type(AggregateType::Block)
            .with_status(EventStatus::Confirmed);

        // Publish 5 events BEFORE subscribing — these would all be lost without
        // replay.
        let mut published_ids = Vec::new();
        for i in 0..5 {
            let event = block_event(i);
            published_ids.push(event.id.clone());
            bus.publish(event, None).await.expect("publish");
        }

        // Subscribe AFTER the publishes.
        let mut stream = bus
            .subscribe(filter, Consumer::LORO)
            .await
            .expect("subscribe");

        // All 5 must arrive on the replay before any further publishes.
        let mut received = Vec::new();
        for _ in 0..5 {
            let event = timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("replay timeout — consumer did not catch up")
                .expect("stream closed prematurely");
            received.push(event.id);
        }

        // Replay ordering is `ORDER BY created_at, id`; we assert the *set*
        // matches because publish order isn't guaranteed to align with the
        // (created_at_ms, id) sort key when multiple publishes land in the
        // same millisecond.
        let mut received_sorted = received.clone();
        received_sorted.sort();
        let mut expected_sorted = published_ids.clone();
        expected_sorted.sort();
        assert_eq!(
            received_sorted, expected_sorted,
            "replay must yield every unprocessed event"
        );
    }

    /// After replay, live events must still arrive on the same stream — and
    /// every event published must reach the subscriber exactly once,
    /// regardless of which side of the subscribe boundary it landed on.
    #[tokio::test]
    async fn subscribe_combines_replay_and_live() {
        let bus = make_bus().await;
        let filter = EventFilter::new()
            .with_aggregate_type(AggregateType::Block)
            .with_status(EventStatus::Confirmed);

        // 2 events before subscribe (must come via replay), then 3 after (must
        // come via the live broadcast forwarder).
        let mut expected = std::collections::HashSet::new();
        for i in 0..2 {
            let event = block_event(i);
            expected.insert(event.id.clone());
            bus.publish(event, None).await.expect("publish");
        }

        let mut stream = bus
            .subscribe(filter, Consumer::LORO)
            .await
            .expect("subscribe");

        for i in 2..5 {
            let event = block_event(i);
            expected.insert(event.id.clone());
            bus.publish(event, None).await.expect("publish");
        }

        let mut received = std::collections::HashSet::new();
        for _ in 0..5 {
            let event = timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("event timeout")
                .expect("stream closed prematurely");
            received.insert(event.id);
        }

        assert_eq!(
            received, expected,
            "stream must deliver every event (replay + live) exactly once"
        );
    }

    /// Events the consumer has already marked processed must NOT be replayed.
    #[tokio::test]
    async fn subscribe_skips_already_processed() {
        let bus = make_bus().await;
        let filter = EventFilter::new()
            .with_aggregate_type(AggregateType::Block)
            .with_status(EventStatus::Confirmed);

        // Publish 3 events; mark the first one processed.
        let mut ids = Vec::new();
        for i in 0..3 {
            let event = block_event(i);
            ids.push(event.id.clone());
            bus.publish(event, None).await.expect("publish");
        }
        bus.mark_processed(&ids[0], Consumer::LORO)
            .await
            .expect("mark_processed");

        let mut stream = bus
            .subscribe(filter, Consumer::LORO)
            .await
            .expect("subscribe");

        // Only ids[1] and ids[2] should be replayed; ids[0] is already done.
        let mut received = std::collections::HashSet::new();
        for _ in 0..2 {
            let event = timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("replay timeout")
                .expect("stream closed prematurely");
            received.insert(event.id);
        }
        let expected: std::collections::HashSet<_> =
            [ids[1].clone(), ids[2].clone()].into_iter().collect();
        assert_eq!(received, expected);

        // No further events should come immediately.
        let extra = timeout(Duration::from_millis(100), stream.next()).await;
        assert!(
            extra.is_err(),
            "stream yielded an unexpected extra event: {extra:?}"
        );
    }
}
