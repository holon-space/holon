use async_trait::async_trait;
use holon_macros::Entity;
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
use serde::{Deserialize, Serialize};
use tracing::info;

use std::sync::Arc;

use holon_api::entity::{IntoEntity, TryFromEntity};
use holon_api::DynamicEntity;
use holon_core::{CacheFactory, DataSource, EntityCache};

/// Entity for the mcp_oauth_credentials table.
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "mcp_oauth_credentials", api_crate = "holon_api")]
pub struct OAuthCredential {
    #[primary_key]
    pub server_uri: String,
    pub credentials_json: String,
    pub updated_at: String,
}

/// Persistent OAuth credential store backed by the storage-agnostic
/// [`EntityCache`] seam (a `DynamicEntity` cache over the
/// `mcp_oauth_credentials` table; rows convert to/from [`OAuthCredential`]
/// at this boundary).
///
/// Each instance is scoped to a single MCP server URI, but the underlying
/// cache holds credentials for all servers.
#[derive(Clone)]
pub struct TursoCredentialStore {
    cache: Arc<dyn EntityCache<DynamicEntity>>,
    server_uri: String,
}

impl TursoCredentialStore {
    /// Create a credential store for a specific server.
    ///
    /// The backing table is created by the [`CacheFactory`].
    pub async fn new(
        cache_factory: &dyn CacheFactory,
        server_uri: String,
    ) -> anyhow::Result<Self> {
        let cache = cache_factory
            .create_dynamic_cache(OAuthCredential::type_definition())
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to initialize mcp_oauth_credentials table: {e}")
            })?;

        info!("[TursoCredentialStore] mcp_oauth_credentials table initialized");
        Ok(Self { cache, server_uri })
    }
}

fn now_utc() -> String {
    chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
        .expect("now within range")
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

#[async_trait]
impl CredentialStore for TursoCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let row: Option<DynamicEntity> = DataSource::get_by_id(&*self.cache, &self.server_uri)
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to query credentials: {e}")))?;
        let credential: Option<OAuthCredential> = row
            .map(OAuthCredential::from_entity)
            .transpose()
            .map_err(|e| {
                AuthError::InternalError(format!("Failed to decode stored credentials: {e}"))
            })?;

        match credential {
            Some(c) => {
                let creds: StoredCredentials =
                    serde_json::from_str(&c.credentials_json).map_err(|e| {
                        AuthError::InternalError(format!("Failed to deserialize credentials: {e}"))
                    })?;
                Ok(Some(creds))
            }
            None => Ok(None),
        }
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let json = serde_json::to_string(&credentials).map_err(|e| {
            AuthError::InternalError(format!("Failed to serialize credentials: {e}"))
        })?;

        let credential = OAuthCredential {
            server_uri: self.server_uri.clone(),
            credentials_json: json,
            updated_at: now_utc(),
        };

        self.cache
            .upsert(&credential.to_entity())
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to save credentials: {e}")))?;

        info!(
            "[TursoCredentialStore] Saved credentials for server '{}'",
            self.server_uri
        );
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.cache
            .delete(&self.server_uri)
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to clear credentials: {e}")))?;

        info!(
            "[TursoCredentialStore] Cleared credentials for server '{}'",
            self.server_uri
        );
        Ok(())
    }
}
