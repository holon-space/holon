use async_trait::async_trait;
use holon_macros::Entity;
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
use serde::{Deserialize, Serialize};
use tracing::info;

use holon::core::datasource::DataSource;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::DbHandle;

/// Entity for the mcp_oauth_credentials table.
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "mcp_oauth_credentials", api_crate = "holon_api")]
pub struct OAuthCredential {
    #[primary_key]
    pub server_uri: String,
    pub credentials_json: String,
    pub updated_at: String,
}

/// Persistent OAuth credential store backed by QueryableCache.
///
/// Each instance is scoped to a single MCP server URI, but the underlying
/// cache holds credentials for all servers.
#[derive(Clone)]
pub struct TursoCredentialStore {
    cache: QueryableCache<OAuthCredential>,
    server_uri: String,
}

impl TursoCredentialStore {
    /// Create a credential store for a specific server.
    ///
    /// The table is created automatically by QueryableCache::for_entity().
    pub async fn new(db_handle: DbHandle, server_uri: String) -> anyhow::Result<Self> {
        let cache = QueryableCache::<OAuthCredential>::for_entity(db_handle)
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to initialize mcp_oauth_credentials table: {e}")
            })?;

        info!("[TursoCredentialStore] mcp_oauth_credentials table initialized");
        Ok(Self { cache, server_uri })
    }
}

fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[async_trait]
impl CredentialStore for TursoCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let credential: Option<OAuthCredential> =
            DataSource::get_by_id(&self.cache, &self.server_uri)
                .await
                .map_err(|e| {
                    AuthError::InternalError(format!("Failed to query credentials: {e}"))
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
            .upsert_to_cache(&credential)
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
            .delete_from_cache(&self.server_uri)
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to clear credentials: {e}")))?;

        info!(
            "[TursoCredentialStore] Cleared credentials for server '{}'",
            self.server_uri
        );
        Ok(())
    }
}
