use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::Change;
use holon_api::ChangeOrigin;
use holon_api::DynamicEntity;
use holon_api::EntityUri;
use holon_api::StreamPosition;
use holon_api::Value;
use holon_core::EntityCache;
use holon_core::MatviewHook;
use holon_core::Result;
use holon_core::SyncTokenStore;
use holon_core::SyncableProvider;
use holon_turso::turso::DbHandle;
use rmcp::RoleClient;
use rmcp::model::SubscribeRequestParam;
use rmcp::service::Peer;
use tracing::Instrument;
use tracing::debug;
use tracing::info;
use tracing::info_span;
use tracing::warn;

use crate::entity_mirror::EntityMirror;
use crate::mcp_sidecar::McpSidecar;
use crate::mcp_sync_strategy::SyncStrategy;
use crate::mcp_sync_strategy::expand_uri_template;
use crate::mcp_sync_strategy::json_value_to_holon_value;
use crate::mcp_sync_strategy::match_uri_template;

/// Scheme-prefix a record id value. Single source of truth for id prefixing so
/// `record_to_entity` and `record_id` can never diverge (a divergence makes the
/// full-sync diff delete + recreate every row on every sync).
fn prefixed_id(scheme: &str, value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => Some(format!("{scheme}:{raw}")),
        Value::Integer(n) => Some(format!("{scheme}:{n}")),
        _ => None,
    }
}

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

/// Convert a fetched JSON record into a `DynamicEntity`, prefixing the ID
/// column with the entity's URI scheme.
fn record_to_entity(
    entity_name: &str,
    id_col: &str,
    scheme: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> DynamicEntity {
    let mut entity = DynamicEntity::new(entity_name);
    for (key, json_val) in obj {
        let value = json_value_to_holon_value(json_val);
        if key == id_col {
            match prefixed_id(scheme, &value) {
                Some(prefixed) => entity.set(key.as_str(), Value::String(prefixed)),
                None => entity.set(key.as_str(), value),
            }
        } else {
            entity.set(key.as_str(), value);
        }
    }
    entity
}

/// Extract the prefixed ID from a fetched JSON record as an `EntityUri`.
fn record_id(
    id_col: &str,
    scheme: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<EntityUri> {
    let val = obj.get(id_col)?;
    let raw = prefixed_id(scheme, &json_value_to_holon_value(val))?;
    // ALLOW(entity_uri_from_raw): remote MCP JSON record id at sync boundary
    Some(EntityUri::from_raw(&raw))
}

/// Full-sync diff: seed the mirror once, then diff the freshly fetched records
/// against the engine-owned mirror (never re-reading the `DatabaseActor`),
/// apply the resulting `Change` batch transactionally, and write the same batch
/// through to the mirror so it stays consistent with the cache table.
async fn apply_full_sync(
    entity_name: &str,
    id_col: &str,
    scheme: &str,
    records: &[serde_json::Map<String, serde_json::Value>],
    mirror: &EntityMirror,
    cache: &dyn EntityCache<DynamicEntity>,
) -> Result<usize> {
    // Lazily seed ONCE from the cache; every later full sync diffs the mirror.
    if !mirror.is_seeded() {
        let rows = cache.get_all().await?;
        mirror.seed(rows);
    }

    let snapshot = mirror.snapshot();
    let existing_by_id: HashMap<EntityUri, &DynamicEntity> = snapshot
        .iter()
        .filter_map(|e| {
            let id_str = match e.get(id_col) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Integer(n)) => n.to_string(),
                _ => return None,
            };
            // ALLOW(entity_uri_from_raw): mirror row id column, already scheme-prefixed
            Some((EntityUri::from_raw(&id_str), e.as_ref()))
        })
        .collect();
    let existing_ids: HashSet<EntityUri> = existing_by_id.keys().cloned().collect();

    let fetched_ids: HashSet<EntityUri> = records
        .iter()
        .filter_map(|obj| record_id(id_col, scheme, obj))
        .collect();

    let new_ids_count = fetched_ids.difference(&existing_ids).count();
    let overlapping_count = fetched_ids.len() - new_ids_count;
    let removed_ids: Vec<EntityUri> = existing_ids.difference(&fetched_ids).cloned().collect();

    let mut changes: Vec<Change<DynamicEntity>> = Vec::new();
    let mut updated_count = 0usize;

    for obj in records {
        let Some(id) = record_id(id_col, scheme, obj) else {
            continue;
        };
        let entity = record_to_entity(entity_name, id_col, scheme, obj);
        match existing_by_id.get(&id) {
            None => {
                changes.push(Change::Created {
                    data: entity,
                    origin: ChangeOrigin::local_with_current_span(),
                });
            }
            Some(cached) if !fetched_matches_cached(&entity, cached) => {
                updated_count += 1;
                changes.push(Change::Updated {
                    id: id.to_string(),
                    data: entity,
                    origin: ChangeOrigin::local_with_current_span(),
                });
            }
            Some(_) => {}
        }
    }

    let new_count = changes
        .iter()
        .filter(|c| matches!(c, Change::Created { .. }))
        .count();

    for id in &removed_ids {
        changes.push(Change::Deleted {
            id: id.to_string(),
            origin: ChangeOrigin::local_with_current_span(),
        });
    }

    // Sequence: apply_batch (transactional) → on Ok write the SAME batch through
    // to the mirror. On Err the mirror is left untouched (matches the rollback).
    let applied = changes.len();
    if !changes.is_empty() {
        cache.apply_batch(&changes, None).await?;
        mirror.apply(&changes);
    }

    info!(
        entity = entity_name,
        new = new_count,
        updated = updated_count,
        removed = removed_ids.len(),
        unchanged = overlapping_count.saturating_sub(updated_count),
        "sync_entity: full sync diff"
    );

    // Debug-only divergence guard: the mirror must match the real cache after a
    // full sync. A mismatch means an external writer (e.g. a full-resync
    // `clear_cache`) emptied the table without the engine resetting the mirror.
    // Release builds rely on the mirror-consistency PBT instead.
    #[cfg(debug_assertions)]
    {
        let cache_id_count = cache.get_all_ids().await?.len();
        let mirror_len = mirror.len();
        assert_eq!(
            mirror_len, cache_id_count,
            "mirror divergence for '{entity_name}': mirror has {mirror_len} rows, cache has \
             {cache_id_count} — was the cache cleared without resetting the mirror?"
        );
    }

    Ok(applied)
}

/// Incremental (cursor) sync: the server already filtered to new/changed
/// records, so every record is a `Created`. Applies the batch to the cache and
/// — if the mirror is already seeded — writes the batch through so a later full
/// sync diffs correctly. If the mirror is not yet seeded, the eventual first
/// full sync seeds from the cache (which already holds these rows).
async fn apply_incremental(
    entity_name: &str,
    id_col: &str,
    scheme: &str,
    records: &[serde_json::Map<String, serde_json::Value>],
    mirror: &EntityMirror,
    cache: &dyn EntityCache<DynamicEntity>,
) -> Result<usize> {
    let changes: Vec<Change<DynamicEntity>> = records
        .iter()
        .map(|obj| Change::Created {
            data: record_to_entity(entity_name, id_col, scheme, obj),
            origin: ChangeOrigin::local_with_current_span(),
        })
        .collect();

    if !changes.is_empty() {
        cache.apply_batch(&changes, None).await?;
        if mirror.is_seeded() {
            mirror.apply(&changes);
        }
    }
    Ok(changes.len())
}

/// Describes an FDW-backed vtable entity that should be refreshed on resource
/// notifications.
pub struct VtableSubscription {
    /// URI template, e.g. `"claude-history://sessions/{session_id}/messages"`
    pub uri_template: String,
    /// FDW table name, e.g. `"cc_message_fdw"`
    pub fdw_table: String,
    /// Dynamic param names that appear in the template, e.g. `["session_id"]`
    pub param_columns: Vec<String>,
}

/// Generic MCP sync engine that pulls data from any MCP server into local cache
/// tables.
///
/// Uses `SyncStrategy` to abstract over tool-based and resource-based fetching.
/// Also handles vtable-backed entities via FDW cache refresh on notifications.
pub struct McpSyncEngine {
    peer: Peer<RoleClient>,
    strategies: HashMap<String, Box<dyn SyncStrategy>>,
    caches: HashMap<String, Arc<dyn EntityCache<DynamicEntity>>>,
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
    /// One write-through in-memory mirror per sync-strategy entity, replacing
    /// the per-fire `get_all_ids`/`get_all` DatabaseActor reads in the
    /// full-sync diff. Seeded lazily on the first full sync; reset on
    /// full-resync.
    mirrors: HashMap<String, Arc<EntityMirror>>,
}

impl McpSyncEngine {
    #[allow(clippy::too_many_arguments)] // wires up the full sync pipeline; each arg is a distinct subsystem
    pub fn new(
        peer: Peer<RoleClient>,
        strategies: HashMap<String, Box<dyn SyncStrategy>>,
        caches: HashMap<String, Arc<dyn EntityCache<DynamicEntity>>>,
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

        // One mirror per sync-strategy entity, keyed by entity name. Each carries
        // the entity's scheme (prefixed name) and id column so it can key rows
        // exactly as the sync diff does. All start unseeded — seeded lazily on
        // the first full sync.
        let mirrors: HashMap<String, Arc<EntityMirror>> = strategies
            .keys()
            .map(|name| {
                let entity_type = sidecar.prefixed_name(name).as_str().to_string();
                let id_column = sidecar.id_column(name);
                (
                    name.clone(),
                    Arc::new(EntityMirror::new(entity_type, id_column)),
                )
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
            mirrors,
        }
    }

    /// Reset every entity mirror to unseeded. Called at the start of a full
    /// sweep (`SyncableProvider::sync`), which runs after the `full_sync`
    /// operation clears the cache tables + tokens — the mirrors re-seed from
    /// the (now-empty) caches on the next per-entity diff.
    fn reset_all_mirrors(&self) {
        for mirror in self.mirrors.values() {
            mirror.reset();
        }
    }

    /// Sync a single entity using its strategy.
    ///
    /// For incremental sync (cursor present), all fetched records are appended.
    /// For full sync (no cursor), diffs against the cache to only insert new
    /// records, delete removed records, and skip unchanged ones.
    async fn sync_entity(
        &self,
        entity_name: &str,
        strategy: &dyn SyncStrategy,
        cache: &dyn EntityCache<DynamicEntity>,
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
        cache: &dyn EntityCache<DynamicEntity>,
    ) -> Result<()> {
        let token_key = format!("{}.{}", self.provider_name, entity_name);

        let fetch_result = strategy
            .fetch_records(
                &self.peer as &dyn crate::mcp_call_surface::McpCallSurface,
                self.token_store.as_ref(),
                &token_key,
            )
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

        // The engine-owned mirror replaces the per-fire `get_all_ids`/`get_all`
        // DatabaseActor reads: it is seeded once and kept consistent by
        // write-through after every committed batch (the engine is the sole
        // writer to this sync entity's cache table — enforced by the
        // sync-vs-`vtable.write_through` config check at connect).
        let mirror = self.mirrors.get(entity_name).ok_or_else(|| {
            format!("sync_entity '{entity_name}': no in-memory mirror (engine wiring bug)")
        })?;

        if let Some(new_cursor) = fetch_result.new_cursor.clone() {
            let applied = apply_incremental(
                entity_name,
                &id_col,
                scheme,
                &fetch_result.records,
                mirror,
                cache,
            )
            .await?;

            self.token_store
                .save_token(
                    &token_key,
                    StreamPosition::Version(new_cursor.as_bytes().to_vec()),
                )
                .await?;
            debug!("[McpSyncEngine] Saved cursor for {entity_name}: {new_cursor}");

            info!(
                entity = entity_name,
                records = applied,
                "sync_entity: incremental sync applied"
            );
        } else {
            apply_full_sync(
                entity_name,
                &id_col,
                scheme,
                &fetch_result.records,
                mirror,
                cache,
            )
            .await?;
        }

        Ok(())
    }

    /// Subscribe to resource update notifications for all sync + vtable
    /// entities.
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
                    "[McpSyncEngine] Vtable '{}' has dynamic params {:?} — relying on broadcast \
                     notifications",
                    sub.fdw_table, sub.param_columns
                );
            }
        }

        Ok(())
    }

    /// Re-sync a single entity identified by its subscription URI.
    /// Tries the sync path first (exact URI match), then the vtable path
    /// (template match).
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
                .sync_entity(entity_name, strategy.as_ref(), cache.as_ref())
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

    /// Refresh an FDW-backed cache table by matching the URI against vtable
    /// templates. Returns `true` if a template matched and the refresh was
    /// attempted.
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

    /// Sync one entity by name — the poll-tick path. Fails loudly if the
    /// entity has no sync strategy or cache (a poll tick for such an entity
    /// is a wiring bug, not a condition to paper over).
    pub async fn sync_entity_by_name(&self, entity_name: &str) -> anyhow::Result<()> {
        let strategy = self.strategies.get(entity_name).ok_or_else(|| {
            anyhow::anyhow!("poll tick for '{entity_name}': no sync strategy configured")
        })?;
        let cache = self
            .caches
            .get(entity_name)
            .ok_or_else(|| anyhow::anyhow!("poll tick for '{entity_name}': no cache configured"))?;
        self.sync_entity(entity_name, strategy.as_ref(), cache.as_ref())
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Sync all entities. Convenience wrapper around the SyncableProvider
    /// trait.
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

    /// Access the underlying MCP peer (e.g. to build additional typed
    /// sources like `McpClaudeSessionSource` over the same connection).
    pub fn peer(&self) -> &Peer<RoleClient> {
        &self.peer
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
    // ALLOW(unused_param): _fdw_sql required by trait shape — handler ignores it
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

        // If the template has no dynamic params, it's already subscribed via
        // subscribe_all.
        if sub.param_columns.is_empty() {
            return;
        }

        // Extract param values from the FDW SQL WHERE clause to reconstruct the
        // concrete URI. Parse simple "column = 'value'" patterns from the SQL.
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

    // ALLOW(unused_param): _position required by trait shape — full sync ignores
    // stream position
    async fn sync(&self, _position: StreamPosition) -> Result<StreamPosition> {
        let span = info_span!("mcp_full_sync", provider = %self.provider_name);
        async {
            info!("mcp_full_sync: starting");

            // Full sweep is the engine's full-resync entry point: it runs at the
            // initial boot sync and when the `full_sync` operation clears every
            // cache table + sync token before re-syncing. In the latter case the
            // tables were just emptied, so the mirrors must be reset here (they
            // re-seed from the now-empty cache on the first per-entity diff) —
            // this is how the engine is "informed" of the external clear without
            // a cross-crate wire from the operation dispatcher. Routine
            // notification/poll syncs never call `sync`, so their mirrors persist
            // and keep serving diffs with zero DatabaseActor reads.
            self.reset_all_mirrors();

            for (entity_name, strategy) in &self.strategies {
                let cache = match self.caches.get(entity_name) {
                    Some(c) => c,
                    None => {
                        warn!(entity = %entity_name, "mcp_full_sync: no cache, skipping");
                        continue;
                    }
                };

                self.sync_entity(entity_name, strategy.as_ref(), cache.as_ref())
                    .await?;
            }

            info!("mcp_full_sync: complete");
            Ok(StreamPosition::Beginning)
        }
        .instrument(span)
        .await
    }
}
