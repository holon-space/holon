//! Event Bus trait and types for event sourcing
//!
//! The EventBus provides a unified interface for publishing and subscribing to events
//! across all systems (Loro, OrgMode, Todoist, etc.).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use ulid::Ulid;

use crate::storage::types::Result;

/// Tracks publish errors from event adapters.
///
/// This is useful for detecting "Database schema changed" errors that occur
/// during startup when DDL operations (like preload_views) race with event
/// publishing from sync adapters.
///
/// Register this in DI and share it across event adapters to track errors
/// without relying on log scraping.
#[derive(Clone, Default)]
pub struct PublishErrorTracker {
    /// Count of failed publish attempts
    error_count: Arc<AtomicUsize>,
    /// Count of successful publish attempts
    success_count: Arc<AtomicUsize>,
}

impl PublishErrorTracker {
    pub fn new() -> Self {
        Self {
            error_count: Arc::new(AtomicUsize::new(0)),
            success_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Record a publish error
    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Record a successful publish
    pub fn record_success(&self) {
        self.success_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Get the number of publish errors
    pub fn errors(&self) -> usize {
        self.error_count.load(Ordering::SeqCst)
    }

    /// Get the number of successful publishes
    pub fn successes(&self) -> usize {
        self.success_count.load(Ordering::SeqCst)
    }

    /// Returns true if any publish errors occurred
    pub fn has_errors(&self) -> bool {
        self.errors() > 0
    }

    /// Get total attempts (errors + successes)
    pub fn total_attempts(&self) -> usize {
        self.errors() + self.successes()
    }

    /// Reset counters (useful for tests)
    pub fn reset(&self) {
        self.error_count.store(0, Ordering::SeqCst);
        self.success_count.store(0, Ordering::SeqCst);
    }
}

/// Event ID (ULID for ordering + distribution)
pub type EventId = String;

/// Command ID (ULID for linking events to commands)
pub type CommandId = String;

/// Event status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventStatus {
    /// Event is speculative (offline, not yet confirmed)
    Speculative,
    /// Event is confirmed (persisted and valid)
    Confirmed,
    /// Event was rejected (conflict resolution, validation failure, etc.)
    Rejected,
}

impl EventStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventStatus::Speculative => "speculative",
            EventStatus::Confirmed => "confirmed",
            EventStatus::Rejected => "rejected",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "speculative" => Some(EventStatus::Speculative),
            "confirmed" => Some(EventStatus::Confirmed),
            "rejected" => Some(EventStatus::Rejected),
            _ => None,
        }
    }
}

/// Event origin (which system generated the event)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventOrigin {
    Loro,
    Org,
    Todoist,
    Ui,
    Other(String),
}

impl EventOrigin {
    pub fn as_str(&self) -> &str {
        match self {
            EventOrigin::Loro => "loro",
            EventOrigin::Org => "org",
            EventOrigin::Todoist => "todoist",
            EventOrigin::Ui => "ui",
            EventOrigin::Other(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "loro" => EventOrigin::Loro,
            "org" => EventOrigin::Org,
            "todoist" => EventOrigin::Todoist,
            "ui" => EventOrigin::Ui,
            other => EventOrigin::Other(other.to_string()),
        }
    }
}

/// What happened to an aggregate (closed set — new kinds require a code change).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    Created,
    Updated,
    Deleted,
    FieldsChanged,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Created => "created",
            EventKind::Updated => "updated",
            EventKind::Deleted => "deleted",
            EventKind::FieldsChanged => "fields_changed",
        }
    }

    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s {
            "created" => Ok(EventKind::Created),
            "updated" => Ok(EventKind::Updated),
            "deleted" => Ok(EventKind::Deleted),
            "fields_changed" => Ok(EventKind::FieldsChanged),
            other => Err(format!("Unknown event kind: '{}'", other)),
        }
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which entity type this event is about.
///
/// Known types are explicit variants; third-party integrations use `Custom(String)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AggregateType {
    Block,
    Task,
    Project,
    Directory,
    File,
    Custom(String),
}

impl AggregateType {
    pub fn as_str(&self) -> &str {
        match self {
            AggregateType::Block => "block",
            AggregateType::Task => "task",
            AggregateType::Project => "project",
            AggregateType::Directory => "directory",
            AggregateType::File => "file",
            AggregateType::Custom(s) => s.as_str(),
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "block" => AggregateType::Block,
            "task" => AggregateType::Task,
            "project" => AggregateType::Project,
            "directory" => AggregateType::Directory,
            "file" => AggregateType::File,
            other => AggregateType::Custom(other.to_string()),
        }
    }
}

impl std::fmt::Display for AggregateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Event payload key for routing events to document-specific handlers.
/// Contains the document URI that owns the affected block.
/// Stored as a top-level payload key (not inside `data`) to avoid corrupting
/// the block's actual `parent_id` when CacheEventSubscriber upserts from `data`.
pub const ROUTING_DOC_URI_KEY: &str = "_routing_doc_uri";

/// An event representing a fact that occurred in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Event ID (ULID)
    pub id: EventId,
    /// What happened (created, updated, deleted, fields_changed)
    pub event_kind: EventKind,
    /// Which entity type this event is about (block, task, project, …)
    pub aggregate_type: AggregateType,
    /// Aggregate ID (entity ID)
    pub aggregate_id: String,
    /// Origin system that generated this event
    pub origin: EventOrigin,
    /// Event status
    pub status: EventStatus,
    /// Event payload (JSON)
    pub payload: HashMap<String, serde_json::Value>,
    /// OpenTelemetry trace ID (optional)
    pub trace_id: Option<String>,
    /// Command ID that generated this event (for undo correlation)
    pub command_id: Option<CommandId>,
    /// Timestamp (Unix milliseconds)
    pub created_at: i64,
    /// Links confirmed event to original speculative event
    pub speculative_id: Option<EventId>,
    /// Rejection reason (if status = Rejected)
    pub rejection_reason: Option<String>,
}

impl Event {
    /// Legacy composite event type string for SQL storage (e.g., "block.created").
    pub fn event_type_string(&self) -> String {
        format!("{}.{}", self.aggregate_type, self.event_kind)
    }

    /// Parse a legacy composite event type string (e.g., "block.created") into parts.
    pub fn parse_event_type_string(
        s: &str,
    ) -> std::result::Result<(AggregateType, EventKind), String> {
        let dot = s
            .rfind('.')
            .ok_or_else(|| format!("Invalid event type (no dot): '{}'", s))?;
        let aggregate = AggregateType::parse(&s[..dot]);
        let kind = EventKind::parse(&s[dot + 1..])?;
        Ok((aggregate, kind))
    }
}

impl Event {
    /// Create a new event
    pub fn new(
        event_kind: EventKind,
        aggregate_type: AggregateType,
        aggregate_id: impl Into<String>,
        origin: EventOrigin,
        payload: HashMap<String, serde_json::Value>,
    ) -> Self {
        let id = Ulid::new().to_string();
        let created_at = chrono::Utc::now().timestamp_millis();

        Self {
            id,
            event_kind,
            aggregate_type,
            aggregate_id: aggregate_id.into(),
            origin,
            status: EventStatus::Confirmed,
            payload,
            trace_id: None,
            command_id: None,
            created_at,
            speculative_id: None,
            rejection_reason: None,
        }
    }

    /// Create a speculative event (for offline mode)
    pub fn new_speculative(
        event_kind: EventKind,
        aggregate_type: AggregateType,
        aggregate_id: impl Into<String>,
        origin: EventOrigin,
        payload: HashMap<String, serde_json::Value>,
        command_id: Option<CommandId>,
    ) -> Self {
        let mut event = Self::new(event_kind, aggregate_type, aggregate_id, origin, payload);
        event.status = EventStatus::Speculative;
        event.command_id = command_id;
        event
    }
}

/// Filter for subscribing to events
#[derive(Debug, Clone)]
pub struct EventFilter {
    /// Only events from these origins (empty = all origins)
    pub origins: Vec<EventOrigin>,
    /// Only events with these statuses (empty = all statuses)
    pub statuses: Vec<EventStatus>,
    /// Only events for these aggregate types (empty = all types)
    pub aggregate_types: Vec<AggregateType>,
    /// Only events after this timestamp (None = no lower bound)
    pub after_timestamp: Option<i64>,
}

impl EventFilter {
    pub fn new() -> Self {
        Self {
            origins: Vec::new(),
            statuses: Vec::new(),
            aggregate_types: Vec::new(),
            after_timestamp: None,
        }
    }

    /// Filter by origin (exclude events from this origin)
    ///
    /// Note: Origin filtering is primarily handled by the EventSubscriber trait's
    /// template method pattern (see Phase 2). This method is kept for API completeness
    /// but the actual filtering happens in the subscriber implementation.
    pub fn exclude_origin(self, _origin: EventOrigin) -> Self {
        // Origin filtering is handled by EventSubscriber template method pattern
        // This method is a no-op - kept for API completeness
        self
    }

    /// Filter by origin (include events from this origin)
    pub fn with_origin(mut self, origin: EventOrigin) -> Self {
        self.origins.push(origin);
        self
    }

    /// Filter by status
    pub fn with_status(mut self, status: EventStatus) -> Self {
        self.statuses.push(status);
        self
    }

    /// Filter by aggregate type
    pub fn with_aggregate_type(mut self, aggregate_type: AggregateType) -> Self {
        self.aggregate_types.push(aggregate_type);
        self
    }

    /// Filter events after timestamp
    pub fn after_timestamp(mut self, timestamp: i64) -> Self {
        self.after_timestamp = Some(timestamp);
        self
    }
}

impl Default for EventFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a `Change<T>` into an `Event`.
///
/// This is the single canonical implementation of the Change→Event mapping,
/// shared by all event adapters (Org, Todoist, Loro, etc.).
pub fn change_to_event<T: serde::Serialize>(
    change: &holon_api::streaming::Change<T>,
    aggregate_type: AggregateType,
    origin: EventOrigin,
    extract_id: impl Fn(&T) -> String,
) -> Result<Event> {
    use holon_api::streaming::{Change, ChangeOrigin};

    let (event_kind, aggregate_id, payload_map, trace_id) =
        match change {
            Change::Created { data, origin } => {
                let payload = serde_json::to_value(data).map_err(|e| {
                    crate::storage::types::StorageError::SerializationError(format!(
                        "Failed to serialize: {}",
                        e
                    ))
                })?;
                let mut payload_map = HashMap::new();
                payload_map.insert("data".to_string(), payload);
                payload_map.insert(
                    "change_type".to_string(),
                    serde_json::Value::String("created".to_string()),
                );
                let trace_id = match origin {
                    ChangeOrigin::Local { trace_id, .. }
                    | ChangeOrigin::Remote { trace_id, .. } => trace_id.clone(),
                };
                (EventKind::Created, extract_id(data), payload_map, trace_id)
            }
            Change::Updated { id, data, origin } => {
                let payload = serde_json::to_value(data).map_err(|e| {
                    crate::storage::types::StorageError::SerializationError(format!(
                        "Failed to serialize: {}",
                        e
                    ))
                })?;
                let mut payload_map = HashMap::new();
                payload_map.insert("data".to_string(), payload);
                payload_map.insert(
                    "change_type".to_string(),
                    serde_json::Value::String("updated".to_string()),
                );
                let trace_id = match origin {
                    ChangeOrigin::Local { trace_id, .. }
                    | ChangeOrigin::Remote { trace_id, .. } => trace_id.clone(),
                };
                (EventKind::Updated, id.clone(), payload_map, trace_id)
            }
            Change::Deleted { id, origin } => {
                let mut payload_map = HashMap::new();
                payload_map.insert(
                    "change_type".to_string(),
                    serde_json::Value::String("deleted".to_string()),
                );
                let trace_id = match origin {
                    ChangeOrigin::Local { trace_id, .. }
                    | ChangeOrigin::Remote { trace_id, .. } => trace_id.clone(),
                };
                (EventKind::Deleted, id.clone(), payload_map, trace_id)
            }
            Change::FieldsChanged {
                entity_id,
                fields,
                origin,
            } => {
                let fields_json = serde_json::to_value(fields).map_err(|e| {
                    crate::storage::types::StorageError::SerializationError(format!(
                        "Failed to serialize fields: {}",
                        e
                    ))
                })?;
                let mut payload_map = HashMap::new();
                payload_map.insert("fields".to_string(), fields_json);
                payload_map.insert(
                    "change_type".to_string(),
                    serde_json::Value::String("fields_changed".to_string()),
                );
                let trace_id = match origin {
                    ChangeOrigin::Local { trace_id, .. }
                    | ChangeOrigin::Remote { trace_id, .. } => trace_id.clone(),
                };
                (
                    EventKind::FieldsChanged,
                    entity_id.clone(),
                    payload_map,
                    trace_id,
                )
            }
        };

    let mut event = Event::new(
        event_kind,
        aggregate_type,
        aggregate_id,
        origin,
        payload_map,
    );
    event.trace_id = trace_id;
    Ok(event)
}

/// Stream of events (type alias for now, will be implemented as async stream)
pub type EventStream = tokio_stream::wrappers::ReceiverStream<Event>;

/// Event Bus trait for publishing and subscribing to events
#[async_trait]
pub trait EventBus: Send + Sync {
    /// Publish an event to the event bus
    ///
    /// Returns the event ID (which may differ from the input event ID if the bus generates it)
    async fn publish(&self, event: Event, command_id: Option<CommandId>) -> Result<EventId>;

    /// Publish multiple events in a single transaction
    ///
    /// This is more efficient than calling `publish` multiple times and ensures
    /// all events are inserted atomically. IVM (Incremental View Maintenance)
    /// only triggers once at the end of the transaction.
    ///
    /// Returns the event IDs of all published events.
    async fn publish_batch(&self, events: Vec<Event>) -> Result<Vec<EventId>> {
        // Default implementation: publish one by one
        // Implementors should override for better performance
        let mut ids = Vec::with_capacity(events.len());
        for event in events {
            ids.push(self.publish(event, None).await?);
        }
        Ok(ids)
    }

    /// Subscribe to events matching the filter
    ///
    /// Returns a stream of events. The stream will continue until the subscription is dropped.
    async fn subscribe(&self, filter: EventFilter) -> Result<EventStream>;

    /// Mark an event as processed by a consumer
    ///
    /// This is used to track which systems have processed which events for cleanup.
    async fn mark_processed(&self, event_id: &EventId, consumer: &str) -> Result<()>;

    /// Update event status (e.g., speculative → confirmed)
    async fn update_status(
        &self,
        event_id: &EventId,
        status: EventStatus,
        rejection_reason: Option<String>,
    ) -> Result<()>;

    /// Link a confirmed event to its original speculative event
    async fn link_speculative(
        &self,
        confirmed_event_id: &EventId,
        speculative_event_id: &EventId,
    ) -> Result<()>;
}
