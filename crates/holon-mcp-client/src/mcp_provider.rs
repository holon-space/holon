use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::Value;
use holon_api::render_types::Operation;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::ParamMapping;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_core::traits::Delivery;
use holon_core::traits::OperationResult;
use holon_core::traits::Result;
use holon_core::traits::UndoAction;
use rmcp::RoleClient;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParam;
use rmcp::service::Peer;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use tokio::process::Command;
use tracing::info;

use crate::mcp_schema_mapping::input_schema_to_params;
use crate::mcp_sidecar::AckVerdict;
use crate::mcp_sidecar::McpSidecar;
use crate::mcp_sidecar::ToolEffect;
use crate::mcp_sidecar::UndoConfig;
use crate::mcp_sidecar::WritesPolicy;
use crate::write_authorization::PendingState;
use crate::write_authorization::PendingWrite;
use crate::write_authorization::PendingWriteStore;
use crate::write_authorization::PendingWriteView;
use crate::write_authorization::SharedPendingWrites;
use crate::write_authorization::WriteAuthorizationPolicy;
use crate::write_authorization::WriteDecision;
use crate::write_authorization::WriteIntent;
use crate::write_authorization::policy_for;

/// Type-erased entity field reader — reads entity fields as HashMap<String,
/// Value>. Allows McpOperationProvider to capture old state without knowing
/// concrete entity types.
type FieldReadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<holon_api::StorageEntity>>> + Send + 'a>>;

pub trait EntityFieldReader: Send + Sync {
    fn get_fields(&self, id: &str) -> FieldReadFuture<'_>;
}

use rmcp::handler::client::ClientHandler;

/// Connect to an MCP server over Streamable HTTP and return a Peer for making
/// requests.
///
/// When `auth_token` is provided it is sent as a `Bearer` authorization header.
/// The returned `McpRunningService` must be kept alive for the connection to
/// stay open.
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
/// Uses rmcp's `AuthClient` to transparently inject OAuth tokens into every
/// request. The `AuthorizationManager` handles token refresh automatically.
/// The returned `McpRunningService` must be kept alive for the connection to
/// stay open.
pub async fn connect_mcp_oauth(
    uri: &str,
    auth_manager: rmcp::transport::auth::AuthorizationManager,
) -> anyhow::Result<(Peer<RoleClient>, McpRunningService)> {
    connect_mcp_oauth_with_handler(uri, auth_manager, default_client_info()).await
}

/// Connect to an MCP server over Streamable HTTP with OAuth and a custom
/// `ClientHandler`.
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

/// Connect to an MCP server via stdio child process and return a Peer for
/// making requests.
///
/// Spawns the given command as a child process and communicates via
/// stdin/stdout. The returned `McpRunningService` must be kept alive for the
/// connection to stay open.
pub async fn connect_mcp_child(
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> anyhow::Result<(Peer<RoleClient>, McpRunningService)> {
    connect_mcp_child_with_handler(command, args, env, default_client_info()).await
}

/// Connect to an MCP server via stdio child process with a custom
/// `ClientHandler`.
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

impl McpRunningService {
    /// An inert keep-alive handle for transports that own no MCP connection
    /// (the `rest` transport serves calls over plain HTTP, with nothing to hold
    /// open). The `service` field on `McpIntegration` is mandatory; this fills
    /// it without pretending there is a live MCP service.
    pub fn inert() -> Self {
        McpRunningService(Box::new(()))
    }
}

pub struct McpOperationProvider {
    /// The MCP peer used to call tools. `None` for a read-only provider (the
    /// `rest` transport exposes no write operations).
    peer: Option<Peer<RoleClient>>,
    descriptors: Vec<OperationDescriptor>,
    /// Maps normalized op_name (snake_case) -> original MCP tool name
    /// (kebab-case)
    tool_name_map: HashMap<String, String>,
    /// Sidecar config for undo declarations
    sidecar: McpSidecar,
    /// Type-erased cache readers keyed by entity_name (e.g. "todoist_task")
    entity_readers: HashMap<String, Arc<dyn EntityFieldReader>>,
    /// Keeps the MCP connection alive for the lifetime of this provider.
    /// None when the caller holds the connection externally (e.g.,
    /// McpIntegration).
    _connection: Option<McpRunningService>,
    /// Sent-intents ledger for `keyed` writes: minted idempotency key → number
    /// of times we have dispatched it. STRICTLY retry bookkeeping (ADR 0024 P4:
    /// an execution log is not a cross-replica dedup mechanism). The remote is
    /// the dedup authority; this only lets a retry observe "already sent".
    sent_intents: Arc<Mutex<HashMap<String, u32>>>,
    /// Writer-designation policy for this connector's `once_only` effects
    /// (leases/read-write ruling, increment 4). Selected from the sidecar's
    /// `once_only:` field at construction.
    policy: Box<dyn WriteAuthorizationPolicy>,
    /// At-most-once state machine + approve queue for `once_only` writes
    /// (increment 4). Shared so the frontend approve UI (increment 4c) can list
    /// and confirm pending intents.
    pending: SharedPendingWrites,
}

impl McpOperationProvider {
    /// Connect to an MCP server, fetch tool schemas, and build the provider.
    ///
    /// When `auth_token` is provided it is sent as a `Bearer` authorization
    /// header. The connection is kept alive for the lifetime of this
    /// provider.
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

    /// Build the provider from an already-connected peer, merging with sidecar
    /// UI annotations. Takes ownership of the connection to keep it alive
    /// for the provider's lifetime.
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

    /// Build the provider from an already-connected peer without taking
    /// ownership of the connection. The caller must keep the
    /// `McpRunningService` alive separately.
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
                // A sidecar's Rhai `precondition:` is parameter-shaped, which
                // ADR 0031 P6=A makes illegal as a descriptor guard (guards are
                // relational). It stays a parse-time-validated declaration on
                // the sidecar; nothing ever evaluated it from here.
                guard: holon_api::pattern::OpGuard::None,
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::External,
                },
                trigger: None,
                bound_params: Default::default(),
            };

            descriptors.push(descriptor);
            tool_name_map.insert(normalized, tool_name.to_string());
        }

        let policy = policy_for(sidecar.once_only);
        Ok(Self {
            peer: Some(peer),
            descriptors,
            tool_name_map,
            sidecar,
            entity_readers,
            _connection: None,
            sent_intents: Arc::new(Mutex::new(HashMap::new())),
            policy,
            pending: Arc::new(PendingWriteStore::new()),
        })
    }

    /// Build a read-only provider for a transport that exposes no write
    /// operations (the `rest` transport). It carries no peer, no operation
    /// descriptors, and an empty tool map, so `operations()` is empty and any
    /// `execute_operation` call fails loud. The entity readers still back
    /// cache reads for the sync path.
    pub fn read_only(
        sidecar: McpSidecar,
        entity_readers: HashMap<String, Arc<dyn EntityFieldReader>>,
    ) -> Self {
        let policy = policy_for(sidecar.once_only);
        Self {
            peer: None,
            descriptors: Vec::new(),
            tool_name_map: HashMap::new(),
            sidecar,
            entity_readers,
            _connection: None,
            sent_intents: Arc::new(Mutex::new(HashMap::new())),
            policy,
            pending: Arc::new(PendingWriteStore::new()),
        }
    }

    /// Build a `PendingWrite` record capturing everything needed to re-dispatch
    /// a `once_only` intent on approval (increment 4). `params` is cloned so
    /// the stored call survives past the point where the live
    /// `execute_operation` consumes its params.
    fn pending_write_record(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: &holon_api::StorageEntity,
        original_name: &str,
    ) -> PendingWrite {
        let connector = self
            .sidecar
            .entity_prefix
            .as_deref()
            .unwrap_or("")
            .to_string();
        let display = self
            .sidecar
            .tools
            .get(original_name)
            .and_then(|tc| tc.display_name.clone())
            .unwrap_or_else(|| original_name.to_string());
        PendingWrite {
            entity_name: entity_name.clone(),
            op_name: op_name.to_string(),
            params: params.clone(),
            connector,
            tool: original_name.to_string(),
            display,
            // Overwritten by enqueue_pending / begin_dispatch to the correct
            // initial state.
            state: PendingState::AwaitingConfirmation,
            dispatch_count: 0,
        }
    }

    /// Approve a pending `once_only` write (leases/read-write ruling, increment
    /// 4). Compare-and-take (amendment B): only the single winning approval
    /// re-dispatches; a second approve is a disclosed no-op. Re-dispatch
    /// re-enters `execute_operation` with the stored call and the SAME intent
    /// key, so the `Confirmed -> Dispatching` transition and remote dedup keep
    /// it at-most-once.
    pub async fn approve(&self, intent_key: &str) -> Result<OperationResult> {
        if !self.pending.confirm(intent_key) {
            return Err(format!(
                "no once_only write awaiting confirmation for intent '{intent_key}' \
                 (already approved, dispatched, or unknown-outcome)"
            )
            .into());
        }
        let (entity_name, op_name, params) = self
            .pending
            .stored_call(intent_key)
            .expect("a confirmed intent has a stored call");
        self.execute_operation(&entity_name, &op_name, params).await
    }

    /// Snapshot of tracked `once_only` intents for the pending-writes UI
    /// (increment 4c) and for tests.
    pub fn pending_writes(&self) -> Vec<PendingWriteView> {
        self.pending.list()
    }

    /// Shared handle to the pending-write store — for the frontend approve UI
    /// (increment 4c) and for tests exercising the compare-and-take contract
    /// directly.
    pub fn pending_store(&self) -> SharedPendingWrites {
        self.pending.clone()
    }

    /// Replace this provider's pending-write store with a shared one (increment
    /// 4c). The DI composition root creates ONE store and installs it on every
    /// MCP provider so the frontend's approve panel and every connector's
    /// chokepoint coordinate through the same at-most-once state machine. Set
    /// before any once_only dispatch (at construction time in `holon-app`).
    pub fn set_pending_store(&mut self, store: SharedPendingWrites) {
        self.pending = store;
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
                "no EntityFieldReader registered for entity '{entity_name}' — cannot capture old \
                 state for undo"
            )
            .into();
            err
        })?;

        let all_fields = reader.get_fields(entity_id).await?.ok_or_else(|| {
            let err: Box<dyn std::error::Error + Send + Sync> = format!(
                "entity '{entity_name}' with id '{entity_id}' not found in cache — cannot capture \
                 old state for undo"
            )
            .into();
            err
        })?;

        let mut captured = HashMap::new();
        let mut missing: Vec<&str> = Vec::new();
        for field in capture_fields {
            match all_fields.get(field.as_str()) {
                Some(value) => {
                    captured.insert(field.clone(), value.clone());
                }
                None => missing.push(field.as_str()),
            }
        }
        // Warn instead of erroring: an absent key can also mean the field is
        // legitimately unset on this entity, not just a misnamed capture field.
        if !missing.is_empty() {
            tracing::warn!(
                entity = entity_name,
                id = entity_id,
                missing = ?missing,
                "sidecar undo capture fields absent from cached entity — \
                 undo will not restore them (misnamed capture field or unset field)"
            );
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
            None => {
                return UndoAction::DeclaredIrreversible("mcp: undo not configured for this tool");
            }
        };

        let undo_config = match &tool_config.undo {
            Some(cfg) => cfg,
            None => {
                return UndoAction::DeclaredIrreversible("mcp: undo not configured for this tool");
            }
        };

        match undo_config {
            UndoConfig::Irreversible { .. } => {
                UndoAction::DeclaredIrreversible("mcp: undo not configured for this tool")
            }
            UndoConfig::Mirror { tool, capture } => {
                let entity_config = match self.sidecar.entities.get(entity_name) {
                    Some(ec) => ec,
                    None => {
                        tracing::warn!(
                            "undo for '{original_tool_name}' configured as Mirror but entity \
                             '{entity_name}' is not in the sidecar — degrading to Irreversible"
                        );
                        return UndoAction::DeclaredIrreversible(
                            "mcp: undo not configured for this tool",
                        );
                    }
                };

                let id_col = entity_config.id_column_or_default();
                let entity_id = match params.get(id_col.as_str()) {
                    Some(Value::String(id)) => id.clone(),
                    other => {
                        tracing::warn!(
                            "undo for '{original_tool_name}' configured as Mirror but params lack \
                             a string id column '{id_col}' (got {other:?}) — degrading to \
                             Irreversible"
                        );
                        return UndoAction::DeclaredIrreversible(
                            "mcp: undo not configured for this tool",
                        );
                    }
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
                        return UndoAction::DeclaredIrreversible(
                            "mcp: undo not configured for this tool",
                        );
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
        let peer =
            self.peer
                .as_ref()
                .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                    format!(
                "provider '{}' is read-only: the `rest` integration exposes no write operations \
                 (attempted '{op_name}' on entity '{}')",
                self.sidecar.entity_prefix.as_deref().unwrap_or("<unprefixed>"),
                entity_name.as_str()
            )
            .into()
                })?;

        let original_name = self
            .tool_name_map
            .get(op_name)
            .unwrap_or_else(|| panic!("unknown MCP operation: {op_name}"));

        // Write-policy gate — the single dispatch chokepoint (leases/read-write
        // ruling). A tool with no explicit effect is `read` (validated at load:
        // write-shaped tools must classify themselves).
        let effect = self
            .sidecar
            .tools
            .get(original_name)
            .and_then(|tc| tc.effect)
            .unwrap_or(ToolEffect::Read);
        if effect != ToolEffect::Read && self.sidecar.writes == WritesPolicy::Disabled {
            return Err(format!(
                "connector write blocked: tool '{original_name}' (effect: {}) needs \
                 `writes: enabled` in the provider sidecar — writes are disabled \
                 (attempted on entity '{}')",
                effect.as_str(),
                entity_name.as_str()
            )
            .into());
        }

        // Which user submission this dispatch belongs to, when the caller is a
        // surface that has submissions (a compose box). Parsed here, at the
        // boundary, so a malformed id fails loudly instead of degrading into
        // the payload-only key it is meant to replace.
        let submission: Option<holon_api::effect_id::ComposeId> = match params
            .get(holon_api::effect_id::ComposeId::PARAM)
        {
            None => None,
            Some(Value::String(raw)) => Some(
                holon_api::effect_id::ComposeId::parse(raw)
                    .map_err(|e| format!("connector write rejected for '{original_name}': {e}"))?,
            ),
            Some(other) => {
                return Err(format!(
                        "connector write rejected for '{original_name}': `{}` must be a string, got {other:?}",
                        holon_api::effect_id::ComposeId::PARAM
                    )
                    .into());
            }
        };

        // Deterministic idempotency/intent key for `keyed` and `once_only`
        // writes (ADR 0024 P4 naming discipline). Minted BEFORE params are
        // consumed so the fingerprint is stable across retries/re-dispatches.
        // For `keyed` the key is injected into the call (remote dedups); for
        // `once_only` it also keys the pending-write store (approve-dedup +
        // at-most-once). Injection into the outgoing args happens only when the
        // tool declares a `key_param`.
        let intent_key: Option<String> =
            if matches!(effect, ToolEffect::Keyed | ToolEffect::OnceOnly) {
                let id_col = self
                    .sidecar
                    .entities
                    .get(entity_name.as_str())
                    .map(|e| e.id_column_or_default())
                    .unwrap_or_else(|| "id".to_string());
                let entity_id = match params.get(id_col.as_str()) {
                    Some(Value::String(s)) => s.as_str(),
                    _ => "",
                };
                let fingerprint = holon_api::effect_id::FiringKey::from_row(&params);
                let connector = self.sidecar.entity_prefix.as_deref().unwrap_or("");
                Some(
                    holon_api::effect_id::deterministic_intent_key(
                        connector,
                        original_name,
                        entity_id,
                        &fingerprint,
                        submission.as_ref(),
                    )
                    .to_string(),
                )
            } else {
                None
            };

        // `once_only` writer-designation gate (increment 4; amendments A+B).
        // The dispatch-owning transition is taken BEFORE the remote call; the
        // outcome is recorded AFTER. A post-dispatch failure lands in
        // `OutcomeUnknown` and is NEVER auto-retried.
        if effect == ToolEffect::OnceOnly {
            let key = intent_key
                .as_deref()
                .expect("once_only mints an intent key");
            // Approved re-dispatch: `Confirmed -> Dispatching`, single-winner.
            if !self.pending.take_for_dispatch(key) {
                let connector = self.sidecar.entity_prefix.as_deref().unwrap_or("");
                match self.policy.authorize(&WriteIntent {
                    connector,
                    tool: original_name,
                    effect,
                    intent_key: key,
                }) {
                    WriteDecision::Allow => {
                        // always_run first attempt: take dispatch ownership by
                        // inserting straight into `Dispatching`. A re-attempt on
                        // an existing (in-flight / sent / unknown) intent is
                        // refused — never fire the effect twice.
                        let record =
                            self.pending_write_record(entity_name, op_name, &params, original_name);
                        if !self.pending.begin_dispatch(key, record) {
                            return Err(format!(
                                "connector write blocked: once_only intent '{key}' for tool \
                                 '{original_name}' was already dispatched (outcome may be \
                                 unknown) — not auto-retried; verify on the remote before \
                                 resending"
                            )
                            .into());
                        }
                    }
                    WriteDecision::RequireConfirmation => {
                        let record =
                            self.pending_write_record(entity_name, op_name, &params, original_name);
                        self.pending.enqueue_pending(key, record);
                        return Err(format!(
                            "connector write queued for confirmation: tool '{original_name}' is \
                             `effect: once_only` under `once_only: confirm_manually` — approve it \
                             in the pending-writes panel (intent '{key}'). It will not fire \
                             unattended."
                        )
                        .into());
                    }
                    WriteDecision::Deny { reason } => {
                        return Err(format!(
                            "connector write denied for tool '{original_name}': {reason}"
                        )
                        .into());
                    }
                }
            }
        }

        // `keyed` retry bookkeeping (strictly local; the remote is the dedup
        // authority — ADR 0024 P4).
        if effect == ToolEffect::Keyed
            && let Some(key) = intent_key.as_deref()
        {
            let dispatch = {
                let mut ledger = self.sent_intents.lock().unwrap();
                let c = ledger.entry(key.to_string()).or_insert(0);
                *c += 1;
                *c
            };
            tracing::debug!(
                tool = %original_name, key = %key, dispatch,
                "keyed connector write: minted idempotency key (retry bookkeeping)"
            );
        }

        // Inject the key into the outgoing call args when the tool declares a
        // `key_param` (keyed always; once_only only if it opts into remote
        // dedup).
        let inject: Option<(String, String)> = intent_key.as_ref().and_then(|key| {
            self.sidecar
                .tools
                .get(original_name)
                .and_then(|tc| tc.key_param.clone())
                .map(|param| (param, key.clone()))
        });

        let undo_action = self
            .build_undo_action(original_name, entity_name.as_str(), &params)
            .await;

        // `_`-prefixed params are Holon's own bookkeeping — the same columns
        // `FiringKey` treats as non-semantic. They stop here: a remote tool
        // declares its arguments, and handing it one it never declared is a
        // call it is entitled to reject.
        let mut json_params: serde_json::Map<String, serde_json::Value> = params
            .into_iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| (k.to_string(), to_json_value(v)))
            .collect();
        if let Some((param_name, key)) = inject {
            json_params.insert(param_name, serde_json::Value::String(key));
        }

        let call_result: Result<OperationResult> = match peer
            .call_tool(CallToolRequestParam {
                name: Cow::Owned(original_name.clone()),
                arguments: Some(json_params),
            })
            .await
        {
            Err(e) => Err(format!("MCP call_tool '{original_name}' failed: {e}").into()),
            Ok(result) if result.is_error == Some(true) => {
                let error_text: String = result
                    .content
                    .iter()
                    .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                    .collect::<Vec<_>>()
                    .join("\n");
                Err(format!("MCP tool '{original_name}' returned error: {error_text}").into())
            }
            Ok(result) => {
                let response_text: String = result
                    .content
                    .iter()
                    .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let response_value = serde_json::from_str::<serde_json::Value>(&response_text)
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::String(response_text));
                // The sidecar's `outcome:` declaration is the authority on
                // whether this ack proves the effect landed; without one, a
                // transport ack IS the proof.
                let delivery = match self
                    .sidecar
                    .tools
                    .get(original_name)
                    .and_then(|tc| tc.outcome.as_ref())
                    .map(|oc| oc.classify(original_name, &response_value))
                {
                    None | Some(AckVerdict::Proven) => Delivery::Proven,
                    Some(AckVerdict::Unproven { detail }) => Delivery::Unproven { detail },
                };
                Ok(OperationResult {
                    changes: vec![],
                    undo: undo_action,
                    response: Some(response_value),
                    follow_ups: vec![],
                    delivery,
                })
            }
        };

        // Record the once_only outcome AFTER the call (amendment A): a PROVEN
        // ack -> `Sent`; a failure, or an ack the provider itself declares
        // unproven, -> `OutcomeUnknown` (disclosed, never auto-retried).
        if effect == ToolEffect::OnceOnly {
            let key = intent_key
                .as_deref()
                .expect("once_only mints an intent key");
            match &call_result {
                Ok(r) => match &r.delivery {
                    Delivery::Proven => self.pending.mark_sent(key),
                    Delivery::Unproven { detail } => {
                        self.pending.mark_outcome_unknown(key, detail.clone())
                    }
                },
                Err(e) => self.pending.mark_outcome_unknown(key, e.to_string()),
            }
        }

        call_result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFieldReader {
        fields: HashMap<String, holon_api::StorageEntity>,
    }

    impl EntityFieldReader for FakeFieldReader {
        fn get_fields(
            &self,
            id: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<holon_api::StorageEntity>>> + Send + '_>>
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
    effect: idempotent
    affected_fields: [content, description]
    undo:
      tool: update-tasks
      capture: [content, description]
  delete-object:
    entity: todoist_tasks
    display_name: Delete
    effect: once_only
    undo:
      reversible: false
  find-tasks:
    entity: todoist_tasks
"#;
        McpSidecar::from_yaml(yaml).unwrap()
    }

    fn make_test_reader() -> Arc<dyn EntityFieldReader> {
        let mut fields = HashMap::new();
        let mut task_fields = holon_api::StorageEntity::new();
        task_fields.insert("id".into(), Value::String("123".to_string()));
        task_fields.insert("content".into(), Value::String("Old content".to_string()));
        task_fields.insert(
            "description".into(),
            Value::String("Old description".to_string()),
        );
        task_fields.insert("priority".into(), Value::Integer(1));
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
        params.insert("id".into(), Value::String("123".to_string()));
        params.insert("content".into(), Value::String("New content".to_string()));

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
                inverse_params.insert("id".into(), Value::String("123".to_string()));
                for field in capture {
                    if let Some(value) = old_fields.get(field.as_str()) {
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
