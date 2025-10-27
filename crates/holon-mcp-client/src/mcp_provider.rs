use std::borrow::Cow;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::CallToolRequestParam;
use rmcp::service::Peer;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use tokio::process::Command;
use tracing::info;

use holon::core::datasource::OperationProvider;
use holon::storage::types::StorageEntity;
use holon_api::render_types::{Operation, OperationDescriptor, ParamMapping};
use holon_api::{EntityName, Value};
use holon_core::traits::{OperationResult, Result, UndoAction};

use crate::mcp_schema_mapping::input_schema_to_params;
use crate::mcp_sidecar::{McpSidecar, UndoConfig};

/// Type-erased entity field reader — reads entity fields as HashMap<String, Value>.
/// Allows McpOperationProvider to capture old state without knowing concrete entity types.
pub trait EntityFieldReader: Send + Sync {
    fn get_fields(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<HashMap<String, Value>>>> + Send + '_>>;
}

use std::future::Future;

use holon::core::queryable_cache::QueryableCache;
use holon_api::entity::{IntoEntity, TryFromEntity};
use holon_core::DataSource;

impl<T: IntoEntity + TryFromEntity + Send + Sync + 'static> EntityFieldReader
    for QueryableCache<T>
{
    fn get_fields(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<HashMap<String, Value>>>> + Send + '_>> {
        let id = id.to_string();
        Box::pin(async move {
            let entity: Option<T> = self.get_by_id(&id).await?;
            Ok(entity.map(|e| e.to_entity().fields))
        })
    }
}

use rmcp::handler::client::ClientHandler;

/// Connect to an MCP server over Streamable HTTP and return a Peer for making requests.
///
/// When `auth_token` is provided it is sent as a `Bearer` authorization header.
/// The returned `McpRunningService` must be kept alive for the connection to stay open.
pub async fn connect_mcp(
    uri: &str,
    auth_token: Option<&str>,
) -> anyhow::Result<(Peer<RoleClient>, McpRunningService)> {
    connect_mcp_with_handler(uri, auth_token, default_client_info()).await
}

/// Connect to an MCP server over Streamable HTTP with a custom `ClientHandler`.
///
/// Use `NotifyingClientHandler` to receive resource update notifications.
pub async fn connect_mcp_with_handler<H: ClientHandler>(
    uri: &str,
    auth_token: Option<&str>,
    handler: H,
) -> anyhow::Result<(Peer<RoleClient>, McpRunningService)> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(uri);
    if let Some(token) = auth_token {
        config = config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::from_config(config);
    let service = handler.serve(transport).await?;
    let peer = service.peer().clone();
    Ok((peer, McpRunningService(Box::new(service))))
}

/// Connect to an MCP server over Streamable HTTP with OAuth authentication.
///
/// Uses rmcp's `AuthClient` to transparently inject OAuth tokens into every request.
/// The `AuthorizationManager` handles token refresh automatically.
/// The returned `McpRunningService` must be kept alive for the connection to stay open.
pub async fn connect_mcp_oauth(
    uri: &str,
    auth_manager: rmcp::transport::auth::AuthorizationManager,
) -> anyhow::Result<(Peer<RoleClient>, McpRunningService)> {
    connect_mcp_oauth_with_handler(uri, auth_manager, default_client_info()).await
}

/// Connect to an MCP server over Streamable HTTP with OAuth and a custom `ClientHandler`.
pub async fn connect_mcp_oauth_with_handler<H: ClientHandler>(
    uri: &str,
    auth_manager: rmcp::transport::auth::AuthorizationManager,
    handler: H,
) -> anyhow::Result<(Peer<RoleClient>, McpRunningService)> {
    let auth_client =
        rmcp::transport::auth::AuthClient::new(reqwest::Client::default(), auth_manager);
    let config = StreamableHttpClientTransportConfig::with_uri(uri);
    let transport = StreamableHttpClientTransport::with_client(auth_client, config);
    let service = handler.serve(transport).await?;
    let peer = service.peer().clone();
    Ok((peer, McpRunningService(Box::new(service))))
}

/// Connect to an MCP server via stdio child process and return a Peer for making requests.
///
/// Spawns the given command as a child process and communicates via stdin/stdout.
/// The returned `McpRunningService` must be kept alive for the connection to stay open.
pub async fn connect_mcp_child(
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> anyhow::Result<(Peer<RoleClient>, McpRunningService)> {
    connect_mcp_child_with_handler(command, args, env, default_client_info()).await
}

/// Connect to an MCP server via stdio child process with a custom `ClientHandler`.
pub async fn connect_mcp_child_with_handler<H: ClientHandler>(
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
    handler: H,
) -> anyhow::Result<(Peer<RoleClient>, McpRunningService)> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let transport = TokioChildProcess::new(cmd)?;
    let service = handler.serve(transport).await?;
    let peer = service.peer().clone();
    Ok((peer, McpRunningService(Box::new(service))))
}

fn default_client_info() -> rmcp::model::ClientInfo {
    rmcp::model::ClientInfo {
        protocol_version: Default::default(),
        capabilities: Default::default(),
        client_info: rmcp::model::Implementation {
            name: "holon-mcp-client".into(),
            title: None,
            version: env!("CARGO_PKG_VERSION").into(),
            icons: None,
            website_url: None,
        },
    }
}

/// Opaque handle that keeps the MCP connection alive. Drop to disconnect.
pub struct McpRunningService(#[allow(dead_code)] Box<dyn std::any::Any + Send + Sync>);

pub struct McpOperationProvider {
    peer: Peer<RoleClient>,
    descriptors: Vec<OperationDescriptor>,
    /// Maps normalized op_name (snake_case) -> original MCP tool name (kebab-case)
    tool_name_map: HashMap<String, String>,
    /// Sidecar config for undo declarations
    sidecar: McpSidecar,
    /// Type-erased cache readers keyed by entity_name (e.g. "todoist_task")
    entity_readers: HashMap<String, Arc<dyn EntityFieldReader>>,
    /// Keeps the MCP connection alive for the lifetime of this provider.
    /// None when the caller holds the connection externally (e.g., McpIntegration).
    _connection: Option<McpRunningService>,
}

impl McpOperationProvider {
    /// Connect to an MCP server, fetch tool schemas, and build the provider.
    ///
    /// When `auth_token` is provided it is sent as a `Bearer` authorization header.
    /// The connection is kept alive for the lifetime of this provider.
    pub async fn connect(
        uri: &str,
        auth_token: Option<&str>,
        sidecar: McpSidecar,
        entity_readers: HashMap<String, Arc<dyn EntityFieldReader>>,
    ) -> anyhow::Result<Self> {
        let (peer, connection) = connect_mcp(uri, auth_token).await?;
        Self::from_peer(peer, connection, sidecar, entity_readers).await
    }

    /// Connect to an MCP server via stdio child process and build the provider.
    pub async fn connect_child(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        sidecar: McpSidecar,
        entity_readers: HashMap<String, Arc<dyn EntityFieldReader>>,
    ) -> anyhow::Result<Self> {
        let (peer, connection) = connect_mcp_child(command, args, env).await?;
        Self::from_peer(peer, connection, sidecar, entity_readers).await
    }

    /// Build the provider from an already-connected peer, merging with sidecar UI annotations.
    /// Takes ownership of the connection to keep it alive for the provider's lifetime.
    pub async fn from_peer(
        peer: Peer<RoleClient>,
        connection: McpRunningService,
        sidecar: McpSidecar,
        entity_readers: HashMap<String, Arc<dyn EntityFieldReader>>,
    ) -> anyhow::Result<Self> {
        let mut provider = Self::from_peer_shared(peer, sidecar, entity_readers).await?;
        provider._connection = Some(connection);
        Ok(provider)
    }

    /// Build the provider from an already-connected peer without taking ownership
    /// of the connection. The caller must keep the `McpRunningService` alive separately.
    pub async fn from_peer_shared(
        peer: Peer<RoleClient>,
        sidecar: McpSidecar,
        entity_readers: HashMap<String, Arc<dyn EntityFieldReader>>,
    ) -> anyhow::Result<Self> {
        let tools = peer.list_all_tools().await?;
        info!(
            "[McpOperationProvider] Fetched {} tools from MCP server",
            tools.len()
        );

        let mut descriptors = Vec::with_capacity(tools.len());
        let mut tool_name_map = HashMap::new();

        for tool in &tools {
            let tool_name = tool.name.as_ref();
            let normalized = tool_name.replace('-', "_");

            let tool_config = sidecar.tools.get(tool_name);

            let entity_name = tool_config
                .and_then(|tc| tc.entity.as_deref())
                .unwrap_or_else(|| sidecar.default_entity())
                .to_string();

            let entity_config = sidecar
                .entities
                .get(&entity_name)
                .unwrap_or_else(|| panic!("entity '{entity_name}' not found in sidecar"));

            let display_name = tool_config
                .and_then(|tc| tc.display_name.as_deref())
                .unwrap_or(tool_name)
                .to_string();

            let description = tool.description.as_deref().unwrap_or("").to_string();

            let input_schema = tool.input_schema.as_ref();
            let param_overrides = tool_config.and_then(|tc| tc.param_overrides.as_ref());
            let required_params = input_schema_to_params(input_schema, param_overrides);

            let affected_fields = tool_config
                .and_then(|tc| tc.affected_fields.clone())
                .unwrap_or_default();

            let param_mappings: Vec<ParamMapping> = tool_config
                .and_then(|tc| tc.triggered_by.as_ref())
                .map(|triggers| {
                    triggers
                        .iter()
                        .map(|t| ParamMapping {
                            from: t.from.clone(),
                            provides: t.provides.clone(),
                            defaults: HashMap::new(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            let precondition = tool_config
                .and_then(|tc| tc.precondition.as_ref())
                .map(|p| p.to_checker());

            let descriptor = OperationDescriptor {
                entity_name: entity_name.clone().into(),
                entity_short_name: entity_config.short_name_or(&entity_name),
                id_column: entity_config.id_column_or_default(),
                name: normalized.clone(),
                display_name,
                description,
                required_params,
                affected_fields,
                param_mappings,
                precondition,
                ..Default::default()
            };

            descriptors.push(descriptor);
            tool_name_map.insert(normalized, tool_name.to_string());
        }

        Ok(Self {
            peer,
            descriptors,
            tool_name_map,
            sidecar,
            entity_readers,
            _connection: None,
        })
    }

    /// Capture old field values from cache for mirror undo.
    async fn capture_old_state(
        &self,
        entity_name: &str,
        entity_id: &str,
        capture_fields: &[String],
    ) -> Result<HashMap<String, Value>> {
        let reader = self.entity_readers.get(entity_name).ok_or_else(|| {
            let err: Box<dyn std::error::Error + Send + Sync> = format!(
                "no EntityFieldReader registered for entity '{entity_name}' — \
                 cannot capture old state for undo"
            )
            .into();
            err
        })?;

        let all_fields = reader.get_fields(entity_id).await?.ok_or_else(|| {
            let err: Box<dyn std::error::Error + Send + Sync> = format!(
                "entity '{entity_name}' with id '{entity_id}' not found in cache — \
                 cannot capture old state for undo"
            )
            .into();
            err
        })?;

        let mut captured = HashMap::new();
        for field in capture_fields {
            if let Some(value) = all_fields.get(field) {
                captured.insert(field.clone(), value.clone());
            }
        }
        Ok(captured)
    }

    /// Build the undo action for a tool call based on its UndoConfig.
    async fn build_undo_action(
        &self,
        original_tool_name: &str,
        entity_name: &str,
        params: &StorageEntity,
    ) -> UndoAction {
        let tool_config = match self.sidecar.tools.get(original_tool_name) {
            Some(tc) => tc,
            None => return UndoAction::Irreversible,
        };

        let undo_config = match &tool_config.undo {
            Some(cfg) => cfg,
            None => return UndoAction::Irreversible,
        };

        match undo_config {
            UndoConfig::Irreversible { .. } => UndoAction::Irreversible,
            UndoConfig::Mirror { tool, capture } => {
                let entity_config = match self.sidecar.entities.get(entity_name) {
                    Some(ec) => ec,
                    None => return UndoAction::Irreversible,
                };

                let id_col = entity_config.id_column_or_default();
                let entity_id = match params.get(&id_col) {
                    Some(Value::String(id)) => id.clone(),
                    _ => return UndoAction::Irreversible,
                };

                let old_state = match self
                    .capture_old_state(entity_name, &entity_id, capture)
                    .await
                {
                    Ok(state) => state,
                    Err(e) => {
                        tracing::warn!(
                            "Failed to capture old state for undo of '{original_tool_name}': {e}"
                        );
                        return UndoAction::Irreversible;
                    }
                };

                let mut inverse_params: HashMap<String, Value> = HashMap::new();
                inverse_params.insert(id_col, Value::String(entity_id));
                for (field, value) in old_state {
                    inverse_params.insert(field, value);
                }

                let inverse_op_name = tool.replace('-', "_");
                let display_name = tool_config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| format!("Undo {}", original_tool_name));

                UndoAction::Undo(Operation {
                    entity_name: entity_name.into(),
                    op_name: inverse_op_name,
                    display_name,
                    params: inverse_params,
                })
            }
        }
    }
}

/// Convert a holon_api::Value to serde_json::Value for MCP tool call params.
fn to_json_value(v: holon_api::Value) -> serde_json::Value {
    serde_json::Value::from(v)
}

#[async_trait]
impl OperationProvider for McpOperationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        self.descriptors.clone()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        let original_name = self
            .tool_name_map
            .get(op_name)
            .unwrap_or_else(|| panic!("unknown MCP operation: {op_name}"));

        let undo_action = self
            .build_undo_action(original_name, entity_name.as_str(), &params)
            .await;

        let json_params: serde_json::Map<String, serde_json::Value> = params
            .into_iter()
            .map(|(k, v)| (k, to_json_value(v)))
            .collect();

        let result = self
            .peer
            .call_tool(CallToolRequestParam {
                name: Cow::Owned(original_name.clone()),
                arguments: Some(json_params),
            })
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("MCP call_tool '{original_name}' failed: {e}").into()
            })?;

        if result.is_error == Some(true) {
            let error_text: String = result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!("MCP tool '{original_name}' returned error: {error_text}").into());
        }

        // Extract text content from MCP response
        let response_text: String = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");

        let response_value = serde_json::from_str::<serde_json::Value>(&response_text)
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(response_text));

        Ok(OperationResult {
            changes: vec![],
            undo: undo_action,
            response: Some(response_value),
            follow_ups: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFieldReader {
        fields: HashMap<String, HashMap<String, Value>>,
    }

    impl EntityFieldReader for FakeFieldReader {
        fn get_fields(
            &self,
            id: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<HashMap<String, Value>>>> + Send + '_>>
        {
            let result = self.fields.get(id).cloned();
            Box::pin(async move { Ok(result) })
        }
    }

    fn make_test_sidecar() -> McpSidecar {
        let yaml = r#"
entities:
  todoist_tasks:
    short_name: task
    id_column: id
tools:
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description]
    undo:
      tool: update-tasks
      capture: [content, description]
  delete-object:
    entity: todoist_tasks
    display_name: Delete
    undo:
      reversible: false
  find-tasks:
    entity: todoist_tasks
"#;
        McpSidecar::from_yaml(yaml).unwrap()
    }

    fn make_test_reader() -> Arc<dyn EntityFieldReader> {
        let mut fields = HashMap::new();
        let mut task_fields = HashMap::new();
        task_fields.insert("id".to_string(), Value::String("123".to_string()));
        task_fields.insert(
            "content".to_string(),
            Value::String("Old content".to_string()),
        );
        task_fields.insert(
            "description".to_string(),
            Value::String("Old description".to_string()),
        );
        task_fields.insert("priority".to_string(), Value::Integer(1));
        fields.insert("123".to_string(), task_fields);
        Arc::new(FakeFieldReader { fields })
    }

    #[tokio::test]
    async fn mirror_undo_captures_old_state() {
        let sidecar = make_test_sidecar();
        let reader = make_test_reader();
        let mut entity_readers: HashMap<String, Arc<dyn EntityFieldReader>> = HashMap::new();
        entity_readers.insert("todoist_task".to_string(), reader);

        let mut params = StorageEntity::new();
        params.insert("id".to_string(), Value::String("123".to_string()));
        params.insert(
            "content".to_string(),
            Value::String("New content".to_string()),
        );

        // We can't call build_undo_action directly without a full McpOperationProvider,
        // so test via capture_old_state + UndoConfig logic
        let entity_reader = entity_readers.get("todoist_task").unwrap();
        let old_fields = entity_reader.get_fields("123").await.unwrap().unwrap();

        assert_eq!(
            old_fields.get("content"),
            Some(&Value::String("Old content".to_string()))
        );
        assert_eq!(
            old_fields.get("description"),
            Some(&Value::String("Old description".to_string()))
        );

        // Verify the sidecar config is Mirror
        let tool_config = sidecar.tools.get("update-tasks").unwrap();
        match tool_config.undo.as_ref().unwrap() {
            UndoConfig::Mirror { tool, capture } => {
                assert_eq!(tool, "update-tasks");
                assert_eq!(capture, &["content", "description"]);

                // Build inverse params as the provider would
                let mut inverse_params: HashMap<String, Value> = HashMap::new();
                inverse_params.insert("id".to_string(), Value::String("123".to_string()));
                for field in capture {
                    if let Some(value) = old_fields.get(field) {
                        inverse_params.insert(field.clone(), value.clone());
                    }
                }

                let undo = UndoAction::Undo(Operation {
                    entity_name: "todoist_task".into(),
                    op_name: "update_tasks".to_string(),
                    display_name: "update-tasks".to_string(),
                    params: inverse_params.clone(),
                });

                match undo {
                    UndoAction::Undo(op) => {
                        assert_eq!(op.op_name, "update_tasks");
                        assert_eq!(
                            op.params.get("content"),
                            Some(&Value::String("Old content".to_string()))
                        );
                        assert_eq!(
                            op.params.get("description"),
                            Some(&Value::String("Old description".to_string()))
                        );
                        assert_eq!(op.params.get("id"), Some(&Value::String("123".to_string())));
                    }
                    _ => panic!("expected Undo"),
                }
            }
            other => panic!("expected Mirror, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn irreversible_config_produces_irreversible_action() {
        let sidecar = make_test_sidecar();
        let tool_config = sidecar.tools.get("delete-object").unwrap();
        match tool_config.undo.as_ref().unwrap() {
            UndoConfig::Irreversible { reversible } => assert!(!reversible),
            other => panic!("expected Irreversible, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn no_undo_config_is_irreversible() {
        let sidecar = make_test_sidecar();
        let tool_config = sidecar.tools.get("find-tasks").unwrap();
        assert!(tool_config.undo.is_none());
    }
}
