use std::sync::Arc;

use async_trait::async_trait;
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
use tokio::sync::RwLock;
use tracing::info;

use holon::storage::turso::TursoBackend;

/// Persistent OAuth credential store backed by Turso/SQLite.
///
/// Stores `StoredCredentials` (client_id + token response) as JSON in a
/// `mcp_oauth_credentials` table, keyed by MCP server URI.
#[derive(Clone)]
pub struct TursoCredentialStore {
    backend: Arc<RwLock<TursoBackend>>,
    server_uri: String,
}

impl TursoCredentialStore {
    pub fn new(backend: Arc<RwLock<TursoBackend>>, server_uri: String) -> Self {
        Self {
            backend,
            server_uri,
        }
    }

    pub async fn initialize_table(&self) -> anyhow::Result<()> {
        let sql = r#"
            CREATE TABLE IF NOT EXISTS mcp_oauth_credentials (
                server_uri TEXT PRIMARY KEY,
                credentials_json TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            )
        "#;

        let backend = self.backend.read().await;
        backend
            .execute_ddl(sql)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create mcp_oauth_credentials table: {e}"))?;

        info!("[TursoCredentialStore] mcp_oauth_credentials table initialized");
        Ok(())
    }
}

#[async_trait]
impl CredentialStore for TursoCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let backend = self.backend.read().await;

        let sql =
            "SELECT credentials_json FROM mcp_oauth_credentials WHERE server_uri = $server_uri";
        let mut params = std::collections::HashMap::new();
        params.insert(
            "server_uri".to_string(),
            holon_api::Value::String(self.server_uri.clone()),
        );

        let results = backend
            .execute_sql(sql, params)
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to query credentials: {e}")))?;

        if let Some(row) = results.into_iter().next() {
            if let Some(holon_api::Value::String(json)) = row.get("credentials_json") {
                let creds: StoredCredentials = serde_json::from_str(json).map_err(|e| {
                    AuthError::InternalError(format!("Failed to deserialize credentials: {e}"))
                })?;
                return Ok(Some(creds));
            }
        }

        Ok(None)
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let json = serde_json::to_string(&credentials).map_err(|e| {
            AuthError::InternalError(format!("Failed to serialize credentials: {e}"))
        })?;

        let sql = r#"
            INSERT INTO mcp_oauth_credentials (server_uri, credentials_json, updated_at)
            VALUES (?, ?, datetime('now'))
            ON CONFLICT(server_uri) DO UPDATE SET
                credentials_json = excluded.credentials_json,
                updated_at = excluded.updated_at
        "#;

        let backend = self.backend.read().await;
        backend
            .execute_via_actor(
                sql,
                vec![
                    turso::Value::Text(self.server_uri.clone()),
                    turso::Value::Text(json),
                ],
            )
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to save credentials: {e}")))?;

        info!(
            "[TursoCredentialStore] Saved credentials for server '{}'",
            self.server_uri
        );
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let sql = "DELETE FROM mcp_oauth_credentials WHERE server_uri = ?";

        let backend = self.backend.read().await;
        backend
            .execute_via_actor(sql, vec![turso::Value::Text(self.server_uri.clone())])
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to clear credentials: {e}")))?;

        info!(
            "[TursoCredentialStore] Cleared credentials for server '{}'",
            self.server_uri
        );
        Ok(())
    }
}
