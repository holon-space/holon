//! Database-backed sync token store implementation
//!
//! This module provides a SyncTokenStore implementation that persists sync
//! tokens to a SQLite database using a QueryableCache-backed `sync_states`
//! table.

use async_trait::async_trait;
use holon_api::StreamPosition;
use holon_core::DataSource;
use holon_core::Result;
use holon_core::SyncTokenStore;
use holon_macros::Entity;
use serde::Deserialize;
use serde::Serialize;
use tracing::debug;
use tracing::info;

use crate::core::queryable_cache::QueryableCache;
use crate::storage::DbHandle;

/// Entity for the sync_states table.
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "sync_states", api_crate = "holon_api")]
pub struct SyncState {
    #[primary_key]
    pub provider_name: String,
    pub sync_token: String,
    pub updated_at: String,
}

/// Database-backed sync token store
///
/// Stores sync tokens in the sync_states table via QueryableCache.
pub struct DatabaseSyncTokenStore {
    cache: QueryableCache<SyncState>,
}

impl DatabaseSyncTokenStore {
    /// Create a new DatabaseSyncTokenStore backed by QueryableCache.
    ///
    /// The table is created automatically by QueryableCache::for_entity().
    pub async fn new(db_handle: DbHandle) -> Result<Self> {
        let cache = QueryableCache::new(db_handle, SyncState::type_definition()).await?;
        Ok(Self { cache })
    }

    /// Clear all sync tokens from the database
    pub async fn clear_all_tokens(&self) -> Result<()> {
        self.cache.clear().await?;
        info!("[DatabaseSyncTokenStore] Cleared all sync tokens");
        Ok(())
    }
}

/// The wall-clock format every `updated_at` column in this schema uses.
/// Shared so a second table cannot drift into a different one.
pub fn now_utc() -> String {
    chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
        .expect("now within range")
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

#[async_trait]
impl SyncTokenStore for DatabaseSyncTokenStore {
    async fn load_token(&self, provider_name: &str) -> Result<Option<StreamPosition>> {
        debug!(
            "[DatabaseSyncTokenStore] load_token called for provider '{}'",
            provider_name
        );

        let state: Option<SyncState> = DataSource::get_by_id(&self.cache, provider_name).await?;

        match state {
            Some(s) => {
                debug!(
                    "[DatabaseSyncTokenStore] Loaded sync token for provider '{}': {}",
                    provider_name, s.sync_token
                );
                let position = if s.sync_token == "*" {
                    StreamPosition::Beginning
                } else {
                    StreamPosition::Version(s.sync_token.as_bytes().to_vec())
                };
                Ok(Some(position))
            }
            None => {
                debug!(
                    "[DatabaseSyncTokenStore] No sync token found for provider '{}'",
                    provider_name
                );
                Ok(None)
            }
        }
    }

    async fn save_token(&self, provider_name: &str, position: StreamPosition) -> Result<()> {
        debug!(
            "[DatabaseSyncTokenStore] save_token called for provider '{}'",
            provider_name
        );

        let token_str = match position {
            StreamPosition::Beginning => "*".to_string(),
            StreamPosition::Version(bytes) => {
                String::from_utf8(bytes).unwrap_or_else(|_| "*".to_string())
            }
        };

        let state = SyncState {
            provider_name: provider_name.to_string(),
            sync_token: token_str.clone(),
            updated_at: now_utc(),
        };

        self.cache.upsert_to_cache(&state).await?;

        info!(
            "[DatabaseSyncTokenStore] Saved sync token for provider '{}': {}",
            provider_name, token_str
        );
        Ok(())
    }

    async fn clear_all_tokens(&self) -> Result<()> {
        DatabaseSyncTokenStore::clear_all_tokens(self).await
    }
}
