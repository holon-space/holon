use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::RoleClient;
use rmcp::model::SubscribeRequestParam;
use rmcp::service::Peer;
use tracing::{debug, info, warn};

use holon::core::datasource::{Result, StreamPosition, SyncTokenStore, SyncableProvider};
use holon::core::queryable_cache::QueryableCache;
use holon_api::{Change, ChangeOrigin, DynamicEntity};

use crate::mcp_sync_strategy::{SyncStrategy, json_value_to_holon_value};

/// Generic MCP sync engine that pulls data from any MCP server into local cache tables.
///
/// Uses `SyncStrategy` to abstract over tool-based and resource-based fetching.
pub struct McpSyncEngine {
    peer: Peer<RoleClient>,
    strategies: HashMap<String, Box<dyn SyncStrategy>>,
    caches: HashMap<String, Arc<QueryableCache<DynamicEntity>>>,
    token_store: Arc<dyn SyncTokenStore>,
    provider_name: String,
    /// Reverse lookup: subscribe URI → entity name
    uri_to_entity: HashMap<String, String>,
}

impl McpSyncEngine {
    pub fn new(
        peer: Peer<RoleClient>,
        strategies: HashMap<String, Box<dyn SyncStrategy>>,
        caches: HashMap<String, Arc<QueryableCache<DynamicEntity>>>,
        token_store: Arc<dyn SyncTokenStore>,
        provider_name: String,
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
        }
    }

    /// Sync a single entity using its strategy.
    async fn sync_entity(
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
            "[McpSyncEngine] Got {} records for entity '{}'",
            fetch_result.records.len(),
            entity_name
        );

        let changes: Vec<Change<DynamicEntity>> = fetch_result
            .records
            .iter()
            .map(|obj| {
                let mut entity = DynamicEntity::new(entity_name);
                for (key, json_val) in obj {
                    entity.set(key, json_value_to_holon_value(json_val));
                }
                Change::Created {
                    data: entity,
                    origin: ChangeOrigin::local_with_current_span(),
                }
            })
            .collect();

        // Full sync (no cursor) → clear before insert
        if fetch_result.new_cursor.is_none() && !changes.is_empty() {
            cache.clear().await?;
        }

        if !changes.is_empty() {
            cache.apply_batch(&changes, None).await?;
        }

        if let Some(new_cursor) = fetch_result.new_cursor {
            let token_key = format!("{}.{}", self.provider_name, entity_name);
            self.token_store
                .save_token(
                    &token_key,
                    StreamPosition::Version(new_cursor.as_bytes().to_vec()),
                )
                .await?;
            debug!("[McpSyncEngine] Saved cursor for {entity_name}: {new_cursor}");
        }

        info!(
            "[McpSyncEngine] Synced {} records for entity '{}'",
            changes.len(),
            entity_name
        );

        Ok(())
    }

    /// Subscribe to resource update notifications for all ResourceSync entities.
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
        Ok(())
    }

    /// Re-sync a single entity identified by its subscription URI.
    pub async fn resync_by_uri(&self, uri: &str) -> anyhow::Result<()> {
        let entity_name = self
            .uri_to_entity
            .get(uri)
            .ok_or_else(|| anyhow::anyhow!("No entity subscribed to URI '{uri}'"))?;

        let strategy = self
            .strategies
            .get(entity_name)
            .ok_or_else(|| anyhow::anyhow!("No strategy for entity '{entity_name}'"))?;

        let cache = self
            .caches
            .get(entity_name)
            .ok_or_else(|| anyhow::anyhow!("No cache for entity '{entity_name}'"))?;

        info!(
            "[McpSyncEngine] Re-syncing entity '{}' (URI: {})",
            entity_name, uri
        );

        self.sync_entity(entity_name, strategy.as_ref(), cache)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Sync all entities. Convenience wrapper around the SyncableProvider trait.
    pub async fn sync_all(&self) -> anyhow::Result<()> {
        self.sync(StreamPosition::Beginning)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Check if any entities have subscription URIs.
    pub fn has_subscriptions(&self) -> bool {
        !self.uri_to_entity.is_empty()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SyncableProvider for McpSyncEngine {
    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    async fn sync(&self, _position: StreamPosition) -> Result<StreamPosition> {
        info!(
            "[McpSyncEngine] Starting sync for provider '{}'",
            self.provider_name
        );

        for (entity_name, strategy) in &self.strategies {
            let cache = match self.caches.get(entity_name) {
                Some(c) => c,
                None => {
                    warn!(
                        "[McpSyncEngine] No cache for entity '{}', skipping",
                        entity_name
                    );
                    continue;
                }
            };

            self.sync_entity(entity_name, strategy.as_ref(), cache)
                .await?;
        }

        info!(
            "[McpSyncEngine] Sync complete for provider '{}'",
            self.provider_name
        );

        Ok(StreamPosition::Beginning)
    }
}
