use std::marker::PhantomData;
use std::pin::Pin;

use async_trait::async_trait;
use holon_api::ApiError;
use holon_api::BatchMetadata;
use holon_api::CHANGE_ORIGIN_COLUMN;
use holon_api::Change;
use holon_api::ChangeOrigin;
use holon_api::DynamicEntity;
use holon_api::StreamPosition;
use holon_api::SyncTokenUpdate;
use holon_api::WithMetadata;
use holon_api::streaming::ChangeNotifications;
use holon_core::DataSource;
use holon_core::MaybeSendSync;
use holon_core::UndoAction;
use holon_core::storage::types::StorageEntity;
use tokio::sync::broadcast;
use tokio_stream::Stream;
use tracing;

use super::traits::IntoEntity;
use super::traits::Predicate;
use super::traits::Queryable;
use super::traits::Result;
use super::traits::TryFromEntity;
use super::traits::TypeDefinition;
use super::traits::value_to_turso;
use crate::storage::DbHandle;

/// A queryable cache backed by DbHandle (SQLite via database actor).
///
/// QueryableCache receives data exclusively through change streams
/// (`ingest_stream`, `apply_batch`) and provides read access via
/// `DataSource<T>` and `Queryable<T>`.
///
/// Operations (CRUD, Task, Block) are handled by separate operation structs
/// that hold a reference to the cache for lookups.
pub struct QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    db_handle: DbHandle,
    type_def: TypeDefinition,
    _phantom: PhantomData<T>,
}

impl<T> Clone for QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            db_handle: self.db_handle.clone(),
            type_def: self.type_def.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<T> QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    /// Borrow the underlying `DbHandle`. Used by readers that need to issue
    /// custom SQL (e.g. hydrating joins over edge-typed junction tables) on
    /// the same database actor as the cache itself.
    pub fn db_handle(&self) -> &DbHandle {
        &self.db_handle
    }

    /// Create QueryableCache with a DbHandle and type definition.
    pub async fn new(db_handle: DbHandle, type_def: TypeDefinition) -> Result<Self> {
        let cache = Self {
            db_handle,
            type_def,
            _phantom: PhantomData,
        };

        cache.initialize_schema().await?;
        Ok(cache)
    }

    /// Initialize the table schema.
    ///
    /// Uses execute_ddl to route through the database actor for serialization,
    /// preventing race conditions between DDL and DML/IVM operations.
    async fn initialize_schema(&self) -> Result<()> {
        let type_def = &self.type_def;
        let table_name = &type_def.name;

        tracing::debug!(
            "[QueryableCache] initialize_schema called for table '{}'",
            table_name
        );

        // If an object with this name already exists as a view (matview),
        // its schema is owned by a `SchemaModule` and the underlying base
        // table lives elsewhere (e.g. `block` matview → `block_raw`).
        // Running `CREATE TABLE IF NOT EXISTS block (...)` is a no-op here,
        // but `CREATE INDEX ... ON block (...)` fails because Turso doesn't
        // (yet) support indexes on matviews. Skip schema init entirely so
        // we don't trip on the index step.
        if self.master_kind(table_name).await?.as_deref() == Some("view") {
            tracing::debug!(
                "[QueryableCache] '{}' is a matview — schema owned by a SchemaModule; skipping CREATE TABLE/INDEX",
                table_name
            );
            return Ok(());
        }

        let create_table_sql = generate_create_table_sql_with_change_origin(type_def);
        tracing::debug!(
            "[QueryableCache] Executing CREATE TABLE for '{}' via execute_ddl...",
            table_name
        );

        self.db_handle
            .execute_ddl(&create_table_sql)
            .await
            .map_err(|e| format!("Failed to create table: {}", e))?;

        tracing::debug!(
            "[QueryableCache] CREATE TABLE completed for '{}'",
            table_name
        );

        let index_sqls = type_def.to_index_sql();
        tracing::debug!(
            "[QueryableCache] Creating {} indexes for '{}' via execute_ddl...",
            index_sqls.len(),
            table_name
        );

        for (i, index_sql) in index_sqls.iter().enumerate() {
            tracing::debug!(
                "[QueryableCache] Creating index {}/{} for '{}'...",
                i + 1,
                index_sqls.len(),
                table_name
            );
            self.db_handle
                .execute_ddl(index_sql)
                .await
                .map_err(|e| format!("Failed to create index: {}", e))?;
        }

        tracing::debug!(
            "[QueryableCache] initialize_schema completed for '{}'",
            table_name
        );

        Ok(())
    }

    /// Returns the `type` field from `sqlite_master` for the named object
    /// (`"table"`, `"view"`, etc.) or `None` if it doesn't exist.
    async fn master_kind(&self, name: &str) -> Result<Option<String>> {
        let escaped = name.replace('\'', "''");
        let sql = format!("SELECT type FROM sqlite_master WHERE name = '{escaped}' LIMIT 1");
        let rows = self
            .db_handle
            .query(&sql, std::collections::HashMap::new())
            .await
            .map_err(|e| format!("sqlite_master probe for '{name}': {e}"))?;
        Ok(rows
            .first()
            .and_then(|r| r.get("type"))
            .and_then(|v| v.as_string())
            .map(|s| s.to_string()))
    }

    pub async fn upsert_to_cache(&self, item: &T) -> Result<()> {
        self.upsert_to_cache_with_origin(item, None).await
    }

    pub async fn upsert_to_cache_with_origin(
        &self,
        item: &T,
        change_origin: Option<&ChangeOrigin>,
    ) -> Result<()> {
        let entity = item.to_entity();
        let type_def = self.type_def.clone();

        let mut columns = Vec::new();
        let mut placeholders = Vec::new();
        let mut values = Vec::new();

        for field in &type_def.fields {
            if let Some(value) = entity.fields.get(field.name.as_str()) {
                columns.push(field.name.clone());
                placeholders.push("?");
                values.push(value_to_turso(value));
            }
        }

        // Add _change_origin column for trace context propagation
        columns.push(CHANGE_ORIGIN_COLUMN.to_string());
        placeholders.push("?");
        let change_origin_json = change_origin
            .map(|co| co.to_json())
            .unwrap_or_else(|| "null".to_string());
        values.push(turso::Value::Text(change_origin_json));

        let id_field = type_def
            .fields
            .iter()
            .find(|f| f.primary_key)
            .map(|f| f.name.as_str())
            .expect("schema must have a primary_key field");

        let update_clause = columns
            .iter()
            .map(|c| format!("{} = excluded.{}", c, c))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT({}) DO UPDATE SET {}",
            type_def.name,
            columns.join(", "),
            placeholders.join(", "),
            id_field,
            update_clause
        );

        self.db_handle
            .execute(&sql, values)
            .await
            .map_err(|e| format!("Failed to execute upsert: {}", e))?;
        Ok(())
    }

    async fn get_from_cache(&self, id: &str) -> Result<Option<T>> {
        let type_def = self.type_def.clone();
        let id_field = type_def
            .fields
            .iter()
            .find(|f| f.primary_key)
            .map(|f| f.name.as_str())
            .expect("schema must have a primary_key field");

        let sql = format!(
            "SELECT * FROM {} WHERE {} = $id LIMIT 1",
            type_def.name, id_field
        );

        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), holon_api::Value::String(id.to_string()));

        let results = self
            .db_handle
            .query(&sql, params)
            .await
            .map_err(|e| format!("Failed to execute query: {}", e))?;

        if let Some(entity_map) = results.into_iter().next() {
            let mut entity = DynamicEntity::new(&type_def.name);
            for (key, value) in entity_map {
                entity.set(key, value);
            }
            T::from_entity(entity).map(Some)
        } else {
            Ok(None)
        }
    }

    pub async fn delete_from_cache(&self, id: &str) -> Result<()> {
        let type_def = self.type_def.clone();
        let id_field = type_def
            .fields
            .iter()
            .find(|f| f.primary_key)
            .map(|f| f.name.as_str())
            .expect("schema must have a primary_key field");

        let sql = format!("DELETE FROM {} WHERE {} = ?", type_def.name, id_field);
        let params = vec![turso::Value::Text(id.to_string())];

        self.db_handle
            .execute(&sql, params)
            .await
            .map_err(|e| format!("Failed to execute delete: {}", e))?;

        Ok(())
    }

    /// Clear all cached data from this table (DELETE FROM table)
    ///
    /// Used for full sync operations where all cached data should be pruned
    /// before re-syncing from the external system.
    /// Return all primary key values as `EntityUri`s, without deserializing
    /// full rows.
    pub async fn get_all_ids(&self) -> Result<Vec<holon_api::EntityUri>> {
        let id_field = self
            .type_def
            .fields
            .iter()
            .find(|f| f.primary_key)
            .map(|f| f.name.clone())
            .expect("schema must have a primary_key field");

        let sql = format!("SELECT {} FROM {}", id_field, self.type_def.name);
        let rows = self
            .db_handle
            .query_positional(&sql, vec![])
            .await
            .map_err(|e| format!("Failed to query IDs from {}: {}", self.type_def.name, e))?;

        Ok(rows
            .into_iter()
            .filter_map(|row| match row.get(id_field.as_str()) {
                // ALLOW(entity_uri_from_raw): id primary-key field from SQL row
                Some(holon_api::Value::String(s)) => Some(holon_api::EntityUri::from_raw(s)),
                Some(holon_api::Value::Integer(n)) => {
                    // ALLOW(entity_uri_from_raw): id primary-key field from SQL row
                    Some(holon_api::EntityUri::from_raw(&n.to_string()))
                }
                _ => None,
            })
            .collect())
    }

    pub async fn clear(&self) -> Result<()> {
        let type_def = self.type_def.clone();
        let table_name = &type_def.name;
        let sql = format!("DELETE FROM {}", table_name);

        self.db_handle
            .execute(&sql, vec![])
            .await
            .map_err(|e| format!("Failed to clear table {}: {}", table_name, e))?;

        tracing::info!("[QueryableCache] Cleared table '{}'", table_name);
        Ok(())
    }

    /// Wire up stream ingestion from a broadcast receiver (spawns background
    /// task)
    ///
    /// This method subscribes to a broadcast channel and updates the local
    /// cache as changes arrive from the provider. The background task runs
    /// until the stream is closed or the cache is dropped.
    /// ExternalServiceDiscovery
    pub fn ingest_stream(&self, rx: broadcast::Receiver<Vec<Change<T>>>)
    where
        T: Clone + Send + Sync + 'static,
    {
        let db_handle = self.db_handle.clone();
        let type_def = self.type_def.clone();
        let table_name = type_def.name.clone();
        let id_field = type_def
            .fields
            .iter()
            .find(|f| f.primary_key)
            .map(|f| f.name.clone())
            .expect("schema must have a primary_key field");

        // Spawn the ingestion task on the current runtime
        // IMPORTANT: This must be called from an async context on a runtime that stays
        // alive If called from a blocking thread with a temporary runtime, the
        // task will be dropped when that runtime is dropped. The caller should
        // ensure this is called from a persistent runtime.
        tokio::spawn(async move {
            let mut rx = rx;
            tracing::info!(
                "[QueryableCache] Started ingesting stream for table: {}",
                table_name
            );
            loop {
                match rx.recv().await {
                    Ok(changes) => {
                        let change_count = changes.len();
                        tracing::info!(
                            "[QueryableCache] Received {} changes for table: {}",
                            change_count,
                            table_name
                        );

                        // Create OpenTelemetry span for batch ingestion
                        let ingestion_span = tracing::span!(
                            tracing::Level::INFO,
                            "queryable_cache.ingest_batch",
                            "table_name" = %table_name,
                            "change_count" = change_count,
                        );
                        let _ingestion_guard = ingestion_span.enter();

                        // Process all changes in a single batch transaction
                        if let Err(e) = Self::apply_batch_to_cache(
                            &db_handle,
                            &type_def,
                            &table_name,
                            &id_field,
                            &changes,
                        )
                        .await
                        {
                            tracing::error!(
                                "[QueryableCache] Error ingesting batch into cache: {}",
                                e
                            );
                        } else {
                            tracing::debug!(
                                "[QueryableCache] Successfully ingested batch of {} changes for table: {}",
                                change_count,
                                table_name
                            );
                        }

                        // Log batch ingestion completion
                        tracing::info!(
                            "[QueryableCache] Completed ingesting {} changes for table: {}",
                            change_count,
                            table_name
                        );
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Stream lagged by {} messages, triggering resync", n);
                        // TODO: Trigger full resync
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("Change stream closed");
                        break;
                    }
                }
            }
        });
    }

    /// Wire up stream ingestion from a broadcast receiver with metadata (spawns
    /// background task)
    ///
    /// Applies a batch of changes directly to the cache (synchronous,
    /// blocking).
    ///
    /// This method is useful when you need to ensure ordering between different
    /// entity types (e.g., directories before files before headlines for
    /// referential integrity). Unlike `ingest_stream_with_metadata`, this
    /// method blocks until the batch is fully applied.
    pub async fn apply_batch(
        &self,
        changes: &[Change<T>],
        sync_token: Option<&SyncTokenUpdate>,
    ) -> Result<()>
    where
        T: Clone,
    {
        let type_def = self.type_def.clone();
        let table_name = type_def.name.clone();
        let id_field = type_def
            .fields
            .iter()
            .find(|f| f.primary_key)
            .map(|f| f.name.clone())
            .expect("schema must have a primary_key field");

        tracing::info!(
            "[QueryableCache] Applying batch of {} changes to table: {}",
            changes.len(),
            table_name
        );

        Self::apply_batch_to_cache_with_token(
            &self.db_handle,
            &type_def,
            &table_name,
            &id_field,
            changes,
            sync_token,
        )
        .await
    }

    /// This method subscribes to a broadcast channel that includes metadata
    /// (sync tokens) and updates the local cache as changes arrive from the
    /// provider. The sync token is saved atomically with the data changes
    /// in a single transaction.
    ///
    /// This method is preferred over `ingest_stream` when using providers that
    /// include sync tokens in their batch metadata (e.g.,
    /// TodoistSyncProvider).
    pub fn ingest_stream_with_metadata(
        &self,
        rx: broadcast::Receiver<WithMetadata<Vec<Change<T>>, BatchMetadata>>,
    ) where
        T: Clone + Send + Sync + 'static,
    {
        let db_handle = self.db_handle.clone();
        let type_def = self.type_def.clone();
        let table_name = type_def.name.clone();
        let id_field = type_def
            .fields
            .iter()
            .find(|f| f.primary_key)
            .map(|f| f.name.clone())
            .expect("schema must have a primary_key field");

        tokio::spawn(async move {
            let mut rx = rx;
            tracing::info!(
                "[QueryableCache] Started ingesting stream with metadata for table: {}",
                table_name
            );
            loop {
                match rx.recv().await {
                    Ok(batch_with_metadata) => {
                        let changes = &batch_with_metadata.inner;
                        let sync_token = batch_with_metadata.metadata.sync_token.clone();
                        let change_count = changes.len();

                        tracing::info!(
                            "[QueryableCache] Received {} changes for table: {} (sync_token: {})",
                            change_count,
                            table_name,
                            sync_token
                                .as_ref()
                                .map(|t| t.provider_name.as_str())
                                .unwrap_or("none")
                        );

                        let ingestion_span = tracing::span!(
                            tracing::Level::INFO,
                            "queryable_cache.ingest_batch_with_metadata",
                            "table_name" = %table_name,
                            "change_count" = change_count,
                            "has_sync_token" = sync_token.is_some(),
                        );
                        let _ingestion_guard = ingestion_span.enter();

                        // Process all changes AND sync token in a single atomic transaction
                        if let Err(e) = Self::apply_batch_to_cache_with_token(
                            &db_handle,
                            &type_def,
                            &table_name,
                            &id_field,
                            changes,
                            sync_token.as_ref(),
                        )
                        .await
                        {
                            tracing::error!(
                                "[QueryableCache] Error ingesting batch into cache: {}",
                                e
                            );
                        } else {
                            tracing::debug!(
                                "[QueryableCache] Successfully ingested batch of {} changes for table: {}",
                                change_count,
                                table_name
                            );
                        }

                        tracing::info!(
                            "[QueryableCache] Completed ingesting {} changes for table: {}",
                            change_count,
                            table_name
                        );
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Stream lagged by {} messages, triggering resync", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("Change stream closed");
                        break;
                    }
                }
            }
        });
    }

    // Helper method for applying a batch of changes to cache in a single
    // transaction This reduces database lock contention by processing all
    // changes atomically Includes retry logic with exponential backoff for
    // "database is locked" errors
    async fn apply_batch_to_cache(
        db_handle: &DbHandle,
        schema: &TypeDefinition,
        table_name: &str,
        id_field: &str,
        changes: &[Change<T>],
    ) -> Result<()>
    where
        T: IntoEntity + Clone,
    {
        if changes.is_empty() {
            return Ok(());
        }

        const MAX_RETRIES: u32 = 5;
        const INITIAL_DELAY_MS: u64 = 10;

        let mut attempt = 0;
        loop {
            attempt += 1;
            match Self::apply_batch_to_cache_inner(db_handle, schema, table_name, id_field, changes)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let error_str = e.to_string();
                    let is_retryable = error_str.contains("database is locked")
                        || error_str.contains("SQLITE_BUSY")
                        || error_str.contains("Database schema changed");

                    if is_retryable && attempt < MAX_RETRIES {
                        let delay_ms = INITIAL_DELAY_MS * (1 << (attempt - 1)); // Exponential backoff
                        tracing::warn!(
                            "[QueryableCache] Retryable error on attempt {}/{}: {}. Retrying in {}ms",
                            attempt,
                            MAX_RETRIES,
                            error_str,
                            delay_ms
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                        continue;
                    }

                    // Not a retryable error or max retries exceeded
                    return Err(e);
                }
            }
        }
    }

    // Helper method for applying a batch of changes + sync token in a single
    // transaction This ensures data and sync token are saved atomically,
    // preventing lock contention and ensuring consistency (no partial updates
    // on failure)
    async fn apply_batch_to_cache_with_token(
        db_handle: &DbHandle,
        schema: &TypeDefinition,
        table_name: &str,
        id_field: &str,
        changes: &[Change<T>],
        sync_token: Option<&SyncTokenUpdate>,
    ) -> Result<()>
    where
        T: IntoEntity + Clone,
    {
        // Allow empty changes if we have a sync token to save
        if changes.is_empty() && sync_token.is_none() {
            return Ok(());
        }

        const MAX_RETRIES: u32 = 5;
        const INITIAL_DELAY_MS: u64 = 10;

        let mut attempt = 0;
        loop {
            attempt += 1;
            match Self::apply_batch_to_cache_inner_with_token(
                db_handle, schema, table_name, id_field, changes, sync_token,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let error_str = e.to_string();
                    let is_retryable = error_str.contains("database is locked")
                        || error_str.contains("SQLITE_BUSY")
                        || error_str.contains("Database schema changed");

                    if is_retryable && attempt < MAX_RETRIES {
                        let delay_ms = INITIAL_DELAY_MS * (1 << (attempt - 1));
                        tracing::warn!(
                            "[QueryableCache] Retryable error on attempt {}/{}: {}. Retrying in {}ms",
                            attempt,
                            MAX_RETRIES,
                            error_str,
                            delay_ms
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                        continue;
                    }

                    return Err(e);
                }
            }
        }
    }

    // Inner implementation of batch application with sync token (called by retry
    // wrapper) Uses the database actor for batch transactions to ensure CDC and
    // serialization
    #[tracing::instrument(
        name = "atomic_transaction",
        skip(db_handle, changes, sync_token),
        fields(table = %table_name, changes = changes.len(), has_token = sync_token.is_some())
    )]
    async fn apply_batch_to_cache_inner_with_token(
        db_handle: &DbHandle,
        schema: &TypeDefinition,
        table_name: &str,
        id_field: &str,
        changes: &[Change<T>],
        sync_token: Option<&SyncTokenUpdate>,
    ) -> Result<()>
    where
        T: IntoEntity + Clone,
    {
        let statements =
            Self::build_batch_statements(schema, table_name, id_field, changes, sync_token);

        db_handle
            .transaction(statements)
            .await
            .map_err(|e| format!("Batch transaction failed: {}", e))?;

        tracing::debug!("[TX] Batch transaction completed successfully");
        Ok(())
    }

    /// Build batch statements for the actor transaction
    ///
    /// Returns a vector of (SQL, params) tuples ready for the actor's
    /// transaction method. The actor handles BEGIN/COMMIT automatically.
    fn build_batch_statements(
        schema: &TypeDefinition,
        table_name: &str,
        id_field: &str,
        changes: &[Change<T>],
        sync_token: Option<&SyncTokenUpdate>,
    ) -> Vec<(String, Vec<turso::Value>)>
    where
        T: IntoEntity + Clone,
    {
        let mut statements = Vec::with_capacity(changes.len() + 1);

        // Build SQL templates
        let columns: Vec<String> = schema
            .fields
            .iter()
            .map(|f| f.name.clone())
            .chain(std::iter::once(CHANGE_ORIGIN_COLUMN.to_string()))
            .collect();
        let placeholders: Vec<&str> = (0..columns.len()).map(|_| "?").collect();
        let update_clause = columns
            .iter()
            .map(|c| format!("{} = excluded.{}", c, c))
            .collect::<Vec<_>>()
            .join(", ");

        // Created events use INSERT OR IGNORE: if a row with the same id OR any
        // other UNIQUE constraint (e.g., parent_id+name for documents) already exists,
        // the insert is silently skipped. This handles the race where a document
        // block is re-created with a new UUID while the old one still exists.
        let insert_ignore_sql = format!(
            "INSERT OR IGNORE INTO {} ({}) VALUES ({})",
            table_name,
            columns.join(", "),
            placeholders.join(", "),
        );
        let upsert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT({}) DO UPDATE SET {}",
            table_name,
            columns.join(", "),
            placeholders.join(", "),
            id_field,
            update_clause
        );
        let delete_sql = format!("DELETE FROM {} WHERE {} = ?", table_name, id_field);

        // Build statements for each change
        for change in changes {
            match change {
                Change::Created { data, origin } => {
                    let entity = data.to_entity();
                    let mut values: Vec<turso::Value> = Vec::with_capacity(columns.len());
                    for field in &schema.fields {
                        let turso_value = entity
                            .fields
                            .get(field.name.as_str())
                            .map(value_to_turso)
                            .unwrap_or(turso::Value::Null);
                        values.push(turso_value);
                    }
                    values.push(turso::Value::Text(origin.to_json()));
                    if table_name == "block" {
                        let id_val = entity
                            .fields
                            .get("id")
                            .and_then(|v| v.as_string())
                            .unwrap_or("")
                            .to_string();
                        let content_val = entity
                            .fields
                            .get("content")
                            .and_then(|v| v.as_string())
                            .unwrap_or("")
                            .to_string();
                        tracing::trace!(
                            "[CACHE_APPLY_TRACE] INSERT OR IGNORE block id={} content={:?}",
                            id_val,
                            content_val
                        );
                    }
                    statements.push((insert_ignore_sql.clone(), values));
                }
                Change::Updated { data, origin, .. } => {
                    let entity = data.to_entity();
                    let mut values: Vec<turso::Value> = Vec::with_capacity(columns.len());
                    for field in &schema.fields {
                        let turso_value = entity
                            .fields
                            .get(field.name.as_str())
                            .map(value_to_turso)
                            .unwrap_or(turso::Value::Null);
                        values.push(turso_value);
                    }
                    values.push(turso::Value::Text(origin.to_json()));
                    if table_name == "block" {
                        let id_val = entity
                            .fields
                            .get("id")
                            .and_then(|v| v.as_string())
                            .unwrap_or("")
                            .to_string();
                        let content_val = entity
                            .fields
                            .get("content")
                            .and_then(|v| v.as_string())
                            .unwrap_or("")
                            .to_string();
                        let props_val = entity
                            .fields
                            .get("properties")
                            .map(|v| format!("{:?}", v))
                            .unwrap_or_else(|| "<none>".to_string());
                        tracing::trace!(
                            "[CACHE_APPLY_TRACE] UPSERT block id={} content={:?} properties={} origin={:?}",
                            id_val,
                            content_val,
                            props_val,
                            origin
                        );
                    }
                    statements.push((upsert_sql.clone(), values));
                }
                Change::Deleted { id, .. } => {
                    statements.push((delete_sql.clone(), vec![turso::Value::Text(id.to_string())]));
                }
                Change::FieldsChanged { .. } => {
                    // TODO: Implement partial field update
                    // For now, skip (same as the connection-based impl)
                }
            }
        }

        // Add sync token save if provided
        if let Some(token) = sync_token {
            let token_str = match &token.position {
                StreamPosition::Beginning => "*".to_string(),
                StreamPosition::Version(bytes) => {
                    String::from_utf8(bytes.clone()).unwrap_or_else(|_| "*".to_string())
                }
            };

            let sql = r#"
                INSERT INTO sync_states (provider_name, sync_token, updated_at)
                VALUES (?, ?, datetime('now'))
                ON CONFLICT(provider_name) DO UPDATE SET
                    sync_token = excluded.sync_token,
                    updated_at = excluded.updated_at
            "#
            .to_string();

            statements.push((
                sql,
                vec![
                    turso::Value::Text(token.provider_name.clone()),
                    turso::Value::Text(token_str),
                ],
            ));
        }

        statements
    }
}

impl<T> QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    async fn apply_batch_to_cache_inner(
        db_handle: &DbHandle,
        schema: &TypeDefinition,
        table_name: &str,
        id_field: &str,
        changes: &[Change<T>],
    ) -> Result<()>
    where
        T: IntoEntity + Clone,
    {
        let statements = Self::build_batch_statements(schema, table_name, id_field, changes, None);

        db_handle
            .transaction(statements)
            .await
            .map_err(|e| format!("Batch transaction failed: {}", e))?;

        Ok(())
    }

    /// Raw rows for an optional `WHERE` clause, every column preserved —
    /// including columns the domain type `T` does not deserialize (e.g. the
    /// internal `sort_key` ordering column, see ADR 0005). `where_sql` is a
    /// trusted, code-authored fragment; bind user values through `params`.
    /// No `ORDER BY` is applied (Turso IVM matviews ignore it); callers that
    /// need order use [`query_ordered`](Self::query_ordered).
    pub async fn query_raw(
        &self,
        where_sql: &str,
        params: Vec<holon_api::Value>,
    ) -> Result<Vec<StorageEntity>> {
        let sql = if where_sql.is_empty() {
            format!("SELECT * FROM {}", self.type_def.name)
        } else {
            format!("SELECT * FROM {} WHERE {}", self.type_def.name, where_sql)
        };
        let params: Vec<turso::Value> = params.iter().map(value_to_turso).collect();
        self.db_handle
            .query_positional(&sql, params)
            .await
            .map_err(|e| format!("query_raw failed: {}", e).into())
    }

    /// Read rows (optionally `WHERE`-filtered) and return the domain entities
    /// in the order given by `order_cols`, comparing the raw column values
    /// lexicographically. This is the single place the "Turso IVM cannot
    /// `ORDER BY`" limitation is absorbed (ADR 0005, Phase 8): we sort
    /// wrapper-side on the raw rows — which still carry the internal ordering
    /// columns — before converting to `T`, so the domain entity never has to
    /// carry an ordering encoding and downstream readers can trust the order.
    pub async fn query_ordered(
        &self,
        where_sql: &str,
        params: Vec<holon_api::Value>,
        order_cols: &[&str],
    ) -> Result<Vec<T>> {
        let mut rows = self.query_raw(where_sql, params).await?;
        rows.sort_by(|a, b| {
            for col in order_cols {
                let av = a.get(*col).and_then(|v| v.as_string()).unwrap_or("");
                let bv = b.get(*col).and_then(|v| v.as_string()).unwrap_or("");
                match av.cmp(bv) {
                    std::cmp::Ordering::Equal => continue,
                    ord => return ord,
                }
            }
            std::cmp::Ordering::Equal
        });
        let mut results = Vec::with_capacity(rows.len());
        for storage_entity in rows {
            let entity = DynamicEntity {
                type_name: self.type_def.name.clone(),
                fields: storage_entity,
            };
            let item = T::from_entity(entity).map_err(|e| {
                format!(
                    "[QueryableCache::query_ordered] from_entity failed for type {}: {}",
                    self.type_def.name, e
                )
            })?;
            results.push(item);
        }
        Ok(results)
    }
}

/// The storage-agnostic cache seam over this Turso-backed cache — providers
/// consume `dyn EntityCache` / `dyn CacheFactory` (holon-core) instead of
/// naming `QueryableCache`; delegates to the inherent methods.
#[async_trait]
impl<T> holon_core::EntityCache<T> for QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Clone + Send + Sync + 'static,
{
    async fn get_all_ids(&self) -> Result<Vec<holon_api::EntityUri>> {
        QueryableCache::get_all_ids(self).await
    }

    async fn apply_batch(
        &self,
        changes: &[Change<T>],
        sync_token: Option<&SyncTokenUpdate>,
    ) -> Result<()> {
        QueryableCache::apply_batch(self, changes, sync_token).await
    }

    async fn upsert(&self, item: &T) -> Result<()> {
        QueryableCache::upsert_to_cache(self, item).await
    }

    async fn delete(&self, id: &str) -> Result<()> {
        QueryableCache::delete_from_cache(self, id).await
    }
}

#[async_trait]
impl<T> DataSource<T> for QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    async fn get_all(&self) -> Result<Vec<T>> {
        let type_def = self.type_def.clone();
        let sql = format!("SELECT * FROM {}", type_def.name);

        let rows = self
            .db_handle
            .query_positional(&sql, vec![])
            .await
            .map_err(|e| format!("Failed to execute query: {}", e))?;

        let mut results = Vec::with_capacity(rows.len());
        for storage_entity in rows {
            let entity = DynamicEntity {
                type_name: type_def.name.clone(),
                fields: storage_entity,
            };
            let item = T::from_entity(entity).map_err(|e| {
                format!(
                    "[QueryableCache::get_all] from_entity failed for type {}: {}",
                    type_def.name, e
                )
            })?;
            results.push(item);
        }

        Ok(results)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<T>> {
        self.get_from_cache(id).await
    }
}

#[async_trait]
impl<T> Queryable<T> for QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    async fn query<P>(&self, predicate: P) -> Result<Vec<T>>
    where
        P: Predicate<T> + Send + 'static,
    {
        if let Some(sql_pred) = predicate.to_sql() {
            let type_def = self.type_def.clone();
            let sql = format!("SELECT * FROM {} WHERE {}", type_def.name, sql_pred.sql);

            let params: Vec<turso::Value> = sql_pred.params.iter().map(value_to_turso).collect();

            let rows = self
                .db_handle
                .query_positional(&sql, params)
                .await
                .map_err(|e| format!("Failed to execute query: {}", e))?;

            let mut results = Vec::with_capacity(rows.len());
            for storage_entity in rows {
                let entity = DynamicEntity {
                    type_name: type_def.name.clone(),
                    fields: storage_entity,
                };
                let item = T::from_entity(entity).map_err(|e| {
                    format!(
                        "[QueryableCache::query] from_entity failed for type {}: {}",
                        type_def.name, e
                    )
                })?;
                results.push(item);
            }

            return Ok(results);
        }

        // Fall back to in-memory filtering if no SQL predicate
        let all_items = self.get_all().await?;
        Ok(all_items
            .into_iter()
            .filter(|item| predicate.test(item))
            .collect())
    }
}

// Implement ChangeNotifications<StorageEntity> via TursoBackend
// TODO: Option A - Each QueryableCache filters by table name
// This is inefficient when multiple caches share the same backend (all receive
// all events). Consider optimizing to Option B (table-specific subscriptions)
// in the future.
#[async_trait]
impl<T> ChangeNotifications<StorageEntity> for QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    async fn watch_changes_since(
        &self,
        _: StreamPosition,
    ) -> Pin<Box<dyn Stream<Item = std::result::Result<Vec<Change<StorageEntity>>, ApiError>> + Send>>
    {
        // IMPORTANT: No auto-sync here - caller must sync first
        // This allows offline startup without sync attempts

        let type_def = self.type_def.clone();
        let table_name = type_def.name.clone();

        // Subscribe to CDC broadcast and bridge to mpsc to fit the Stream trait.
        // ALLOW(compatibility): the trait the bridge targets is named Stream; this
        // is plain English usage, not a back-compat shim.
        let mut broadcast_rx = self.db_handle.subscribe_row_changes();
        let (tx, rx) = tokio::sync::mpsc::channel(1024);
        tokio::spawn(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(batch) => {
                        if tx.send(batch).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            "[QueryableCache] CDC subscriber lagged by {} messages, continuing",
                            n
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let row_change_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        // TODO: Option A - Filter stream for this table and convert RowChange to
        // Change<StorageEntity> This is inefficient when multiple
        // QueryableCache instances share the same backend. Consider optimizing
        // to Option B (table-specific subscriptions) in the future.
        use holon_api::BatchWithMetadata;
        use tokio_stream::StreamExt;

        use crate::storage::turso::ChangeData;
        use crate::storage::turso::RowChange;

        let table_name_clone = table_name.clone();

        // Filter batches by relation_name in metadata, then flatten to individual
        // RowChanges Use futures::stream::StreamExt for flat_map which has
        // better trait implementations
        let filtered_stream = row_change_stream
            .filter_map(move |batch: BatchWithMetadata<RowChange>| {
                // Filter by relation_name in metadata
                if batch.metadata.relation_name != table_name_clone {
                    return None;
                }

                // Log CDC event emission with OpenTelemetry
                let change_count = batch.inner.items.len();
                let relation_name = batch.metadata.relation_name.clone();
                let trace_context = batch.metadata.trace_context.clone();

                // Count change types
                let mut created_count = 0;
                let mut updated_count = 0;
                let mut deleted_count = 0;
                for row_change in &batch.inner.items {
                    match &row_change.change {
                        ChangeData::Created { .. } => created_count += 1,
                        ChangeData::Updated { .. } => updated_count += 1,
                        ChangeData::Deleted { .. } => deleted_count += 1,
                        ChangeData::FieldsChanged { .. } => updated_count += 1, // Count as update
                    }
                }

                // Create OpenTelemetry span for CDC emission
                let cdc_span = tracing::span!(
                    tracing::Level::INFO,
                    "queryable_cache.cdc_emission",
                    "relation_name" = %relation_name,
                    "change_count" = change_count,
                    "created_count" = created_count,
                    "updated_count" = updated_count,
                    "deleted_count" = deleted_count,
                );
                let _cdc_guard = cdc_span.enter();

                if let Some(ref trace_ctx) = trace_context {
                    // Use tracing macros instead of record() for string values
                    tracing::debug!("trace_id={}, span_id={}", trace_ctx.trace_id, trace_ctx.span_id);
                }

                tracing::info!(
                    "[QueryableCache] Emitting CDC batch: relation={}, changes={} (created={}, updated={}, deleted={})",
                    relation_name,
                    change_count,
                    created_count,
                    updated_count,
                    deleted_count
                );

                // Convert batch items into individual RowChanges and process them
                let mut results = Vec::new();
                for row_change in batch.inner.items {
                    // Convert RowChange to Change<StorageEntity>
                    // StorageEntity is HashMap<String, Value>, so we can use data directly
                    let result = match row_change.change {
                        ChangeData::Created { data, origin } => {
                            Change::Created {
                                data, // data is already HashMap<String, Value> = StorageEntity
                                origin,
                            }
                        }
                        ChangeData::Updated {
                            id: _rowid,
                            data,
                            origin,
                        } => {
                            // Extract entity ID from data, not ROWID
                            let entity_id = data
                                .get("id")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string())
                                .expect("CDC change data must have an 'id' field");
                            Change::Updated {
                                id: entity_id,
                                data, // data is already HashMap<String, Value> = StorageEntity
                                origin,
                            }
                        }
                        ChangeData::Deleted { id: _rowid, origin } => {
                            // TODO: For deletes, we need the entity ID, not ROWID
                            // This is a limitation - we may need to track entity_id separately
                            // For now, use a placeholder - proper fix requires enhancing CDC system
                            Change::Deleted {
                                id: format!("rowid_{}", _rowid), // Placeholder - not ideal
                                origin,
                            }
                        }
                        ChangeData::FieldsChanged { entity_id, fields, origin } => {
                            Change::FieldsChanged {
                                entity_id,
                                fields,
                                origin,
                            }
                        }
                    };
                    results.push(result);
                }
                Some(Ok(results))
            });

        Box::pin(filtered_stream)
    }

    async fn get_current_version(&self) -> std::result::Result<Vec<u8>, ApiError> {
        // Return empty version vector for now
        // Could be enhanced to track sync tokens
        Ok(vec![])
    }
}

/// Generate CREATE TABLE SQL with automatic `_change_origin` column
///
/// This wraps Schema's field definitions and adds the `_change_origin` column
/// for trace context propagation. The column stores JSON-serialized
/// `ChangeOrigin` which allows CDC callbacks to read trace context from each
/// row.
fn generate_create_table_sql_with_change_origin(type_def: &TypeDefinition) -> String {
    // Mirror `TypeDefinition::to_create_table_sql`: when multiple fields are
    // flagged `primary_key`, emit a table-level `PRIMARY KEY (a, b, …)`
    // clause and skip the inline form. SQLite otherwise rejects with
    // "table has more than one primary key".
    let pk_count = type_def.fields.iter().filter(|f| f.primary_key).count();
    let inline_pk = pk_count == 1;

    let mut columns = Vec::new();
    for field in &type_def.fields {
        let mut col = format!("{} {}", field.name, field.sql_type);
        if field.primary_key && inline_pk {
            col.push_str(" PRIMARY KEY");
        }
        if !field.nullable {
            col.push_str(" NOT NULL");
        }
        columns.push(col);
    }

    // Add _change_origin column for trace context propagation
    columns.push(format!("{} TEXT", CHANGE_ORIGIN_COLUMN));

    if pk_count >= 2 {
        let pk_cols: Vec<&str> = type_def
            .fields
            .iter()
            .filter(|f| f.primary_key)
            .map(|f| f.name.as_str())
            .collect();
        columns.push(format!("PRIMARY KEY ({})", pk_cols.join(", ")));
    }

    format!(
        "CREATE TABLE IF NOT EXISTS {} (\n  {}\n)",
        type_def.name,
        columns.join(",\n  ")
    )
}

/// Trait for operation providers that have a QueryableCache
///
/// This trait provides the `clear_cache` operation that clears all cached data
/// for an entity type. It's used for full sync operations where all cached data
/// should be pruned before re-syncing from the external system.
///
/// The `#[operations_trait]` macro auto-generates the `clear_cache` operation
/// descriptor.
///
/// # Example
/// ```ignore
/// impl HasCache<OrgBlock> for OrgBlockOperations {
///     fn get_cache(&self) -> &QueryableCache<OrgBlock> {
///         &self.cache
///     }
/// }
/// ```
#[holon_macros::operations_trait]
#[async_trait]
pub trait HasCache<T>: MaybeSendSync
where
    T: IntoEntity + TryFromEntity + MaybeSendSync + 'static,
{
    /// Get reference to the underlying QueryableCache
    fn get_cache(&self) -> &QueryableCache<T>;

    /// Clear all cached data for this entity
    ///
    /// This method clears the cache table (DELETE FROM table) and is
    /// irreversible. Used for full sync operations.
    async fn clear_cache(&self) -> Result<UndoAction> {
        self.get_cache().clear().await?;
        Ok(UndoAction::DeclaredIrreversible(
            "clear_cache: full-sync cache clear is not undoable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use holon_api::Change;
    use holon_api::ChangeOrigin;
    use holon_api::Value;
    use tempfile::tempdir;

    use super::*;
    use crate::core::traits::FieldSchema;
    use crate::core::traits::SqlPredicate;

    #[derive(Debug, Clone, PartialEq)]
    struct TestTask {
        id: String,
        title: String,
        priority: i64,
    }

    impl holon_api::entity::IntoEntity for TestTask {
        fn to_entity(&self) -> DynamicEntity {
            DynamicEntity::new("TestTask")
                .with_field("id", self.id.clone())
                .with_field("title", self.title.clone())
                .with_field("priority", self.priority)
        }

        fn type_definition() -> TypeDefinition {
            TestTask::type_definition()
        }
    }

    impl holon_api::entity::TryFromEntity for TestTask {
        fn from_entity(entity: DynamicEntity) -> Result<Self> {
            Ok(TestTask {
                id: entity.get_string("id").ok_or("Missing id")?,
                title: entity.get_string("title").ok_or("Missing title")?,
                priority: entity.get_i64("priority").ok_or("Missing priority")?,
            })
        }
    }

    impl TestTask {
        fn type_definition() -> TypeDefinition {
            TypeDefinition::new(
                "test_tasks",
                vec![
                    FieldSchema::new("id", "TEXT").primary_key(),
                    FieldSchema::new("title", "TEXT"),
                    FieldSchema::new("priority", "INTEGER"),
                ],
            )
        }
    }

    struct PriorityPredicate {
        min: i64,
    }

    impl Predicate<TestTask> for PriorityPredicate {
        fn test(&self, item: &TestTask) -> bool {
            item.priority >= self.min
        }

        fn to_sql(&self) -> Option<SqlPredicate> {
            Some(SqlPredicate::new(
                "priority >= ?".to_string(),
                vec![Value::Integer(self.min)],
            ))
        }
    }

    /// Create a test DbHandle for testing QueryableCache.
    async fn create_test_db_handle() -> DbHandle {
        use crate::storage::turso::TursoBackend;

        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test_cache.db");

        let db = TursoBackend::open_database(&db_path).expect("Failed to open database");
        let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
        let (_backend, handle) =
            TursoBackend::new(db, cdc_tx).expect("Failed to create TursoBackend");

        // Keep temp_dir alive
        std::mem::forget(temp_dir);

        handle
    }

    #[tokio::test]
    async fn test_queryable_cache_creation() {
        let db_handle = create_test_db_handle().await;
        let _cache: QueryableCache<TestTask> =
            QueryableCache::new(db_handle, TestTask::type_definition())
                .await
                .unwrap();
    }

    #[tokio::test]
    async fn test_upsert_and_retrieve() {
        let db_handle = create_test_db_handle().await;
        let cache: QueryableCache<TestTask> =
            QueryableCache::new(db_handle, TestTask::type_definition())
                .await
                .unwrap();

        let task = TestTask {
            id: "1".to_string(),
            title: "Test Task".to_string(),
            priority: 5,
        };

        cache.upsert_to_cache(&task).await.unwrap();

        let retrieved = cache.get_by_id("1").await.unwrap();
        assert_eq!(retrieved, Some(task));
    }

    #[tokio::test]
    async fn test_apply_batch() {
        let db_handle = create_test_db_handle().await;
        let cache: QueryableCache<TestTask> =
            QueryableCache::new(db_handle, TestTask::type_definition())
                .await
                .unwrap();

        let task1 = TestTask {
            id: "1".to_string(),
            title: "Task 1".to_string(),
            priority: 3,
        };

        let task2 = TestTask {
            id: "2".to_string(),
            title: "Task 2".to_string(),
            priority: 7,
        };

        let changes = vec![
            Change::Created {
                data: task1.clone(),
                origin: ChangeOrigin::local_with_current_span(),
            },
            Change::Created {
                data: task2.clone(),
                origin: ChangeOrigin::local_with_current_span(),
            },
        ];

        cache.apply_batch(&changes, None).await.unwrap();

        let retrieved1 = cache.get_by_id("1").await.unwrap();
        assert_eq!(retrieved1, Some(task1));

        let retrieved2 = cache.get_by_id("2").await.unwrap();
        assert_eq!(retrieved2, Some(task2));
    }

    #[tokio::test]
    async fn test_get_all() {
        let db_handle = create_test_db_handle().await;
        let cache: QueryableCache<TestTask> =
            QueryableCache::new(db_handle, TestTask::type_definition())
                .await
                .unwrap();

        let task1 = TestTask {
            id: "1".to_string(),
            title: "Task 1".to_string(),
            priority: 3,
        };

        let task2 = TestTask {
            id: "2".to_string(),
            title: "Task 2".to_string(),
            priority: 7,
        };

        cache.upsert_to_cache(&task1).await.unwrap();
        cache.upsert_to_cache(&task2).await.unwrap();

        let all = cache.get_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_query_with_sql() {
        let db_handle = create_test_db_handle().await;
        let cache: QueryableCache<TestTask> =
            QueryableCache::new(db_handle, TestTask::type_definition())
                .await
                .unwrap();

        let task1 = TestTask {
            id: "1".to_string(),
            title: "Low Priority".to_string(),
            priority: 2,
        };

        let task2 = TestTask {
            id: "2".to_string(),
            title: "High Priority".to_string(),
            priority: 8,
        };

        cache.upsert_to_cache(&task1).await.unwrap();
        cache.upsert_to_cache(&task2).await.unwrap();

        let results = cache.query(PriorityPredicate { min: 5 }).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "High Priority");
    }

    #[tokio::test]
    async fn test_delete_from_cache() {
        let db_handle = create_test_db_handle().await;
        let cache: QueryableCache<TestTask> =
            QueryableCache::new(db_handle, TestTask::type_definition())
                .await
                .unwrap();

        let task = TestTask {
            id: "1".to_string(),
            title: "Original".to_string(),
            priority: 3,
        };

        cache.upsert_to_cache(&task).await.unwrap();

        let exists = cache.get_by_id("1").await.unwrap();
        assert!(exists.is_some());

        cache.delete_from_cache("1").await.unwrap();
        let deleted = cache.get_by_id("1").await.unwrap();
        assert!(deleted.is_none());
    }
}
