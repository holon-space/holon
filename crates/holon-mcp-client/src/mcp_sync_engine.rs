use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::RoleClient;
use rmcp::model::SubscribeRequestParam;
use rmcp::service::Peer;
use tracing::{Instrument, debug, info, info_span, warn};

use holon::core::datasource::{
    DataSource, Result, StreamPosition, SyncTokenStore, SyncableProvider,
};
use holon::core::queryable_cache::QueryableCache;
use holon::storage::DbHandle;
use holon::sync::MatviewHook;
use holon_api::{Change, ChangeOrigin, DynamicEntity, EntityUri, Value};

use crate::mcp_sidecar::McpSidecar;
use crate::mcp_sync_strategy::{
    SyncStrategy, expand_uri_template, json_value_to_holon_value, match_uri_template,
};

/// Compare a freshly-fetched entity against a cached one.
///
/// Uses the fetched entity's fields as the canonical set — any field present in
/// `fetched` must exist and match in `cached`. Extra fields in `cached`
/// (e.g. `_change_origin`) are ignored.
fn fetched_matches_cached(fetched: &DynamicEntity, cached: &DynamicEntity) -> bool {
    for (k, v) in &fetched.fields {
        match cached.fields.get(k) {
            Some(cv) if cv == v => {}
            _ => return false,
        }
    }
    true
}

/// Describes an FDW-backed vtable entity that should be refreshed on resource notifications.
pub struct VtableSubscription {
    /// URI template, e.g. `"claude-history://sessions/{session_id}/messages"`
    pub uri_template: String,
    /// FDW table name, e.g. `"cc_message_fdw"`
    pub fdw_table: String,
    /// Dynamic param names that appear in the template, e.g. `["session_id"]`
    pub param_columns: Vec<String>,
}

/// Generic MCP sync engine that pulls data from any MCP server into local cache tables.
///
/// Uses `SyncStrategy` to abstract over tool-based and resource-based fetching.
/// Also handles vtable-backed entities via FDW cache refresh on notifications.
pub struct McpSyncEngine {
    peer: Peer<RoleClient>,
    strategies: HashMap<String, Box<dyn SyncStrategy>>,
    caches: HashMap<String, Arc<QueryableCache<DynamicEntity>>>,
    token_store: Arc<dyn SyncTokenStore>,
    provider_name: String,
    /// Reverse lookup: subscribe URI → entity name (for sync-based entities)
    uri_to_entity: HashMap<String, String>,
    /// Sidecar config — provides entity_prefix, id_column, etc.
    sidecar: McpSidecar,
    /// FDW-backed vtable entities refreshed via URI template matching.
    vtable_subscriptions: Vec<VtableSubscription>,
    /// Database handle for executing FDW cache refresh queries.
    db_handle: Option<DbHandle>,
}

impl McpSyncEngine {
    pub fn new(
        peer: Peer<RoleClient>,
        strategies: HashMap<String, Box<dyn SyncStrategy>>,
        caches: HashMap<String, Arc<QueryableCache<DynamicEntity>>>,
        token_store: Arc<dyn SyncTokenStore>,
        provider_name: String,
        sidecar: McpSidecar,
        vtable_subscriptions: Vec<VtableSubscription>,
        db_handle: Option<DbHandle>,
    ) -> Self {
        let uri_to_entity: HashMap<String, String> = strategies
            .iter()
            .filter_map(|(name, strategy)| {
                strategy
                    .subscribe_uri()
                    .map(|uri| (uri.to_string(), name.clone()))
            })
            .collect();

        Self {
            peer,
            strategies,
            caches,
            token_store,
            provider_name,
            uri_to_entity,
            sidecar,
            vtable_subscriptions,
            db_handle,
        }
    }

    /// Convert a fetched JSON record into a `DynamicEntity`, prefixing the ID column
    /// with the entity's URI scheme.
    fn record_to_entity(
        &self,
        entity_name: &str,
        id_col: &str,
        scheme: &str,
        obj: &serde_json::Map<String, serde_json::Value>,
    ) -> DynamicEntity {
        let mut entity = DynamicEntity::new(entity_name);
        for (key, json_val) in obj {
            let value = json_value_to_holon_value(json_val);
            if key == id_col {
                if let Value::String(ref raw) = value {
                    entity.set(key, Value::String(format!("{scheme}:{raw}")));
                } else {
                    entity.set(key, value);
                }
            } else {
                entity.set(key, value);
            }
        }
        entity
    }

    /// Extract the prefixed ID from a fetched JSON record as an `EntityUri`.
    fn record_id(
        &self,
        id_col: &str,
        scheme: &str,
        obj: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<EntityUri> {
        let val = obj.get(id_col)?;
        let raw = match json_value_to_holon_value(val) {
            Value::String(raw) => format!("{scheme}:{raw}"),
            Value::Integer(n) => format!("{scheme}:{n}"),
            _ => return None,
        };
        Some(EntityUri::from_raw(&raw))
    }

    /// Sync a single entity using its strategy.
    ///
    /// For incremental sync (cursor present), all fetched records are appended.
    /// For full sync (no cursor), diffs against the cache to only insert new records,
    /// delete removed records, and skip unchanged ones.
    async fn sync_entity(
        &self,
        entity_name: &str,
        strategy: &dyn SyncStrategy,
        cache: &QueryableCache<DynamicEntity>,
    ) -> Result<()> {
        let span = info_span!("sync_entity", entity = entity_name, provider = %self.provider_name);
        self.sync_entity_inner(entity_name, strategy, cache)
            .instrument(span)
            .await
    }

    async fn sync_entity_inner(
        &self,
        entity_name: &str,
        strategy: &dyn SyncStrategy,
        cache: &QueryableCache<DynamicEntity>,
    ) -> Result<()> {
        let token_key = format!("{}.{}", self.provider_name, entity_name);

        let fetch_result = strategy
            .fetch_records(&self.peer, self.token_store.as_ref(), &token_key)
            .await
            .map_err(|e| format!("sync_entity '{entity_name}': {e}"))?;

        info!(
            records = fetch_result.records.len(),
            entity = entity_name,
            "sync_entity: fetched records"
        );

        let id_col = self.sidecar.id_column(entity_name);
        let entity_type = self.sidecar.prefixed_name(entity_name);
        let scheme = entity_type.as_str();

        if fetch_result.new_cursor.is_some() {
            // Incremental sync — server already filtered to new/changed records
            let changes: Vec<Change<DynamicEntity>> = fetch_result
                .records
                .iter()
                .map(|obj| Change::Created {
                    data: self.record_to_entity(entity_name, &id_col, scheme, obj),
                    origin: ChangeOrigin::local_with_current_span(),
                })
                .collect();

            if !changes.is_empty() {
                cache.apply_batch(&changes, None).await?;
            }

            let new_cursor = fetch_result.new_cursor.unwrap();
            self.token_store
                .save_token(
                    &token_key,
                    StreamPosition::Version(new_cursor.as_bytes().to_vec()),
                )
                .await?;
            debug!("[McpSyncEngine] Saved cursor for {entity_name}: {new_cursor}");

            info!(
                entity = entity_name,
                records = changes.len(),
                "sync_entity: incremental sync applied"
            );
        } else {
            // Full sync — diff against existing cache to minimise writes.
            // First pass: lightweight ID-only query to detect the common append-only case.
            let existing_ids: HashSet<EntityUri> = cache.get_all_ids().await?.into_iter().collect();

            let fetched_ids: HashSet<EntityUri> = fetch_result
                .records
                .iter()
                .filter_map(|obj| self.record_id(&id_col, scheme, obj))
                .collect();

            let new_ids: HashSet<&EntityUri> = fetched_ids.difference(&existing_ids).collect();
            let removed_ids: HashSet<&EntityUri> = existing_ids.difference(&fetched_ids).collect();
            let overlapping_count = fetched_ids.len() - new_ids.len();

            // If there are overlapping IDs, we need full rows to detect field-level changes.
            // If it's purely append + delete (no overlap), skip the expensive get_all.
            let (mut changes, updated_count) = if overlapping_count > 0 {
                let existing: Vec<DynamicEntity> = cache.get_all().await?;
                let existing_by_id: HashMap<EntityUri, &DynamicEntity> = existing
                    .iter()
                    .filter_map(|e| {
                        let id_str = match e.get(&id_col) {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Integer(n)) => n.to_string(),
                            _ => return None,
                        };
                        Some((EntityUri::from_raw(&id_str), e))
                    })
                    .collect();

                let mut changes: Vec<Change<DynamicEntity>> = Vec::new();
                let mut updated = 0usize;

                for obj in &fetch_result.records {
                    let Some(id) = self.record_id(&id_col, scheme, obj) else {
                        continue;
                    };
                    let entity = self.record_to_entity(entity_name, &id_col, scheme, obj);
                    match existing_by_id.get(&id) {
                        None => {
                            changes.push(Change::Created {
                                data: entity,
                                origin: ChangeOrigin::local_with_current_span(),
                            });
                        }
                        Some(cached) if !fetched_matches_cached(&entity, cached) => {
                            updated += 1;
                            changes.push(Change::Updated {
                                id: id.to_string(),
                                data: entity,
                                origin: ChangeOrigin::local_with_current_span(),
                            });
                        }
                        Some(_) => {}
                    }
                }
                (changes, updated)
            } else {
                // Pure append — all fetched records are new
                let changes: Vec<Change<DynamicEntity>> = fetch_result
                    .records
                    .iter()
                    .map(|obj| Change::Created {
                        data: self.record_to_entity(entity_name, &id_col, scheme, obj),
                        origin: ChangeOrigin::local_with_current_span(),
                    })
                    .collect();
                (changes, 0)
            };

            for id in &removed_ids {
                changes.push(Change::Deleted {
                    id: id.to_string(),
                    origin: ChangeOrigin::local_with_current_span(),
                });
            }

            let new_count = changes
                .iter()
                .filter(|c| matches!(c, Change::Created { .. }))
                .count();

            if !changes.is_empty() {
                cache.apply_batch(&changes, None).await?;
            }

            info!(
                entity = entity_name,
                new = new_count,
                updated = updated_count,
                removed = removed_ids.len(),
                unchanged = overlapping_count.saturating_sub(updated_count),
                "sync_entity: full sync diff"
            );
        }

        Ok(())
    }

    /// Subscribe to resource update notifications for all sync + vtable entities.
    pub async fn subscribe_all(&self) -> anyhow::Result<()> {
        for (uri, entity_name) in &self.uri_to_entity {
            info!(
                "[McpSyncEngine] Subscribing to '{}' for entity '{}'",
                uri, entity_name
            );
            self.peer
                .subscribe(SubscribeRequestParam { uri: uri.clone() })
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to subscribe to '{uri}' for '{entity_name}': {e}")
                })?;
        }

        // Subscribe to vtable resource templates that have no dynamic params
        // (fully static URIs). For templates with dynamic params, we rely on
        // MCP servers broadcasting notifications for all URIs under the scheme.
        for sub in &self.vtable_subscriptions {
            if sub.param_columns.is_empty() {
                info!(
                    "[McpSyncEngine] Subscribing to vtable resource '{}'",
                    sub.uri_template
                );
                self.peer
                    .subscribe(SubscribeRequestParam {
                        uri: sub.uri_template.clone(),
                    })
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to subscribe to vtable '{}': {e}", sub.uri_template)
                    })?;
            } else {
                info!(
                    "[McpSyncEngine] Vtable '{}' has dynamic params {:?} — relying on broadcast notifications",
                    sub.fdw_table, sub.param_columns
                );
            }
        }

        Ok(())
    }

    /// Re-sync a single entity identified by its subscription URI.
    /// Tries the sync path first (exact URI match), then the vtable path (template match).
    pub async fn resync_by_uri(&self, uri: &str) -> anyhow::Result<()> {
        // Try sync path (exact URI match)
        if let Some(entity_name) = self.uri_to_entity.get(uri) {
            let strategy = self
                .strategies
                .get(entity_name)
                .ok_or_else(|| anyhow::anyhow!("No strategy for entity '{entity_name}'"))?;

            let cache = self
                .caches
                .get(entity_name)
                .ok_or_else(|| anyhow::anyhow!("No cache for entity '{entity_name}'"))?;

            info!(entity = %entity_name, %uri, "resync_by_uri: starting");

            return self
                .sync_entity(entity_name, strategy.as_ref(), cache)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"));
        }

        // Try vtable path (template match)
        if self.resync_vtable_by_uri(uri).await? {
            return Ok(());
        }

        debug!("[McpSyncEngine] No handler for resource URI '{uri}' — ignoring notification");
        Ok(())
    }

    /// Refresh an FDW-backed cache table by matching the URI against vtable templates.
    /// Returns `true` if a template matched and the refresh was attempted.
    async fn resync_vtable_by_uri(&self, uri: &str) -> anyhow::Result<bool> {
        let db_handle = match &self.db_handle {
            Some(h) => h,
            None => return Ok(false),
        };

        for sub in &self.vtable_subscriptions {
            if let Some(params) = match_uri_template(&sub.uri_template, uri) {
                // Build WHERE clause from extracted params
                let where_clauses: Vec<String> = sub
                    .param_columns
                    .iter()
                    .filter_map(|col| {
                        params.get(col).map(|val| {
                            let escaped = val.replace('\'', "''");
                            format!("{col} = '{escaped}'")
                        })
                    })
                    .collect();

                let sql = if where_clauses.is_empty() {
                    format!("SELECT * FROM {}", sub.fdw_table)
                } else {
                    format!(
                        "SELECT * FROM {} WHERE {}",
                        sub.fdw_table,
                        where_clauses.join(" AND ")
                    )
                };

                info!(
                    "[McpSyncEngine] Refreshing vtable cache via: {}",
                    &sql[..sql.len().min(200)]
                );

                let rows = db_handle.query(&sql, HashMap::new()).await.map_err(|e| {
                    anyhow::anyhow!(
                        "[McpSyncEngine] Vtable refresh failed for '{}': {e}",
                        sub.fdw_table
                    )
                })?;
                info!(
                    "[McpSyncEngine] Vtable refresh: {} rows written through from '{}'",
                    rows.len(),
                    sub.fdw_table
                );
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Sync all entities. Convenience wrapper around the SyncableProvider trait.
    pub async fn sync_all(&self) -> anyhow::Result<()> {
        self.sync(StreamPosition::Beginning)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Check if any entities have subscription URIs (sync or vtable).
    pub fn has_subscriptions(&self) -> bool {
        !self.uri_to_entity.is_empty() || !self.vtable_subscriptions.is_empty()
    }

    /// Access the sidecar config.
    pub fn sidecar(&self) -> &McpSidecar {
        &self.sidecar
    }

    /// Subscribe to a specific resource URI for change notifications.
    async fn subscribe_to_resource(&self, uri: &str) {
        match self
            .peer
            .subscribe(SubscribeRequestParam {
                uri: uri.to_string(),
            })
            .await
        {
            Ok(_) => info!("[McpSyncEngine] Subscribed to '{uri}'"),
            Err(e) => warn!("[McpSyncEngine] Failed to subscribe to '{uri}': {e}"),
        }
    }
}

#[async_trait]
impl MatviewHook for McpSyncEngine {
    async fn on_fdw_primed(&self, cache_table: &str, _fdw_sql: &str) {
        // Find the vtable subscription for this cache table.
        // The FDW table name is "{cache_table}_fdw", so match by stripping the suffix.
        let fdw_table = format!("{cache_table}_fdw");
        let sub = match self
            .vtable_subscriptions
            .iter()
            .find(|s| s.fdw_table == fdw_table)
        {
            Some(s) => s,
            None => return,
        };

        // If the template has no dynamic params, it's already subscribed via subscribe_all.
        if sub.param_columns.is_empty() {
            return;
        }

        // Extract param values from the FDW SQL WHERE clause to reconstruct the concrete URI.
        // Parse simple "column = 'value'" patterns from the SQL.
        let mut params = HashMap::new();
        for col in &sub.param_columns {
            let pattern = format!("{col} = '");
            if let Some(start) = _fdw_sql.find(&pattern) {
                let value_start = start + pattern.len();
                if let Some(end) = _fdw_sql[value_start..].find('\'') {
                    params.insert(
                        col.clone(),
                        _fdw_sql[value_start..value_start + end].to_string(),
                    );
                }
            }
        }

        if params.len() != sub.param_columns.len() {
            debug!("[McpSyncEngine] Could not extract all params from FDW SQL for subscription");
            return;
        }

        match expand_uri_template(&sub.uri_template, &params) {
            Ok(concrete_uri) => {
                self.subscribe_to_resource(&concrete_uri).await;
            }
            Err(e) => {
                warn!("[McpSyncEngine] Failed to expand URI template for subscription: {e}");
            }
        }
    }
}

#[async_trait]
impl SyncableProvider for McpSyncEngine {
    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    async fn sync(&self, _position: StreamPosition) -> Result<StreamPosition> {
        let span = info_span!("mcp_full_sync", provider = %self.provider_name);
        async {
            info!("mcp_full_sync: starting");

            for (entity_name, strategy) in &self.strategies {
                let cache = match self.caches.get(entity_name) {
                    Some(c) => c,
                    None => {
                        warn!(entity = %entity_name, "mcp_full_sync: no cache, skipping");
                        continue;
                    }
                };

                self.sync_entity(entity_name, strategy.as_ref(), cache)
                    .await?;
            }

            info!("mcp_full_sync: complete");
            Ok(StreamPosition::Beginning)
        }
        .instrument(span)
        .await
    }
}
