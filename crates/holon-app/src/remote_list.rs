//! The transport half of a remote-list connection: the peer a round talks to,
//! and the local rows it decides against.
//!
//! Everything here is parameterised by [`CompiledListSync`], so a second peer
//! adds a sidecar and no Rust. The half that is NOT here — reconcile, the round
//! and the intents — lives in `crates/holon-connections`, which is in the wasm
//! graphs; this module is the part that must not be, because it reaches a
//! socket and a database.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use anyhow::Result;
use async_trait::async_trait;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_connections::CacheBuster;
use holon_connections::CommitAck;
use holon_connections::CommitBatch;
use holon_connections::CompiledListSync;
use holon_connections::ConfiguredList;
use holon_connections::ListSnapshot;
use holon_connections::LocalRow;
use holon_connections::LocalRowReader;
use holon_connections::RemoteListPeer;
use holon_core::TypeWriteAuthority;
use holon_mcp_client::McpIntegration;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::mcp_call_surface::extract_tool_response;
use holon_mcp_client::rest_transport::RESPONSE_VERSION_KEY;
use holon_profiles::TypeRegistry;
use rmcp::model::CallToolRequestParam;

/// What Holon calls itself on a peer's commits. Not a secret; the peer uses it
/// for its own bookkeeping only.
pub fn device_id() -> &'static str {
    "holon"
}

/// A list peer reached over the `utcp:` connection its sidecar declares.
///
/// Both legs are sidecar calls and both MAPPINGS are sidecar filters, so the
/// endpoint shape, both envelopes and the peer's JSON vocabulary all live in
/// YAML. What stays here is the ORDER — and the rule that only a fully mapped
/// body becomes a [`ListSnapshot`], because absence inside one is read as a
/// deletion.
pub struct RestListPeer {
    surface: Arc<dyn McpCallSurface>,
    compiled: Arc<CompiledListSync>,
    device_id: String,
    /// The newest version this peer observed, echoed on the next read. Zero
    /// until the first pull.
    last_version: AtomicI64,
}

impl RestListPeer {
    pub fn new(
        surface: Arc<dyn McpCallSurface>,
        compiled: Arc<CompiledListSync>,
        device_id: impl Into<String>,
    ) -> Self {
        Self {
            surface,
            compiled,
            device_id: device_id.into(),
            last_version: AtomicI64::new(0),
        }
    }

    async fn call(
        &self,
        name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        let result = self
            .surface
            .call_tool(CallToolRequestParam {
                name: Cow::Owned(name.to_string()),
                arguments: Some(args),
            })
            .await
            .map_err(|e| anyhow::anyhow!("list peer: call '{name}': {e}"))?;
        let value = extract_tool_response(&result)
            .with_context(|| format!("list peer: reading the '{name}' response"))?;
        value
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("list peer: call '{name}' answered a non-object body"))
    }

    fn observe(&self, version: i64) {
        self.last_version.fetch_max(version, Ordering::Relaxed);
    }
}

#[async_trait]
impl RemoteListPeer for RestListPeer {
    async fn pull(&self) -> Result<ListSnapshot> {
        let spec = self.compiled.spec();
        let seen = self.last_version.load(Ordering::Relaxed);
        let args = pull_arguments(spec, seen, &self.device_id);

        let response = self.call(&spec.pull_tool, args).await?;
        let rows = self
            .surface
            .map_response(&spec.pull_tool, &serde_json::Value::Object(response))
            .with_context(|| format!("list peer: mapping the '{}' response", spec.pull_tool))?;
        // The fetch time is stamped where the body is known to have mapped: it
        // becomes the watermark, and a watermark from a failed fetch would
        // license a later absence-as-deletion.
        let snapshot =
            ListSnapshot::from_rows(&self.compiled, &rows, chrono::Utc::now().to_rfc3339())?;
        self.observe(snapshot.version());
        Ok(snapshot)
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<CommitAck> {
        let spec = self.compiled.spec();
        let stream = batch.to_row_stream(spec)?;
        let args = self
            .surface
            .map_request(&spec.commit_tool, &stream)
            .with_context(|| format!("list peer: mapping the '{}' request", spec.commit_tool))?;
        let response = self.call(&spec.commit_tool, args).await?;
        let version = response
            .get(RESPONSE_VERSION_KEY)
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "list peer: the '{}' response carries no whole-number version under \
                     '{RESPONSE_VERSION_KEY}'; the sidecar's response_version_path is what puts \
                     it there",
                    spec.commit_tool
                )
            })?;
        self.observe(version);
        Ok(CommitAck { version })
    }
}

/// What a pull carries, for the connection's own `query` template to place.
///
/// `version` and `device_id` are there for every list. The freshness argument
/// is there only when the connection asks for one: a cache-buster is one
/// peer's concern, and sending it unasked would put that peer's vocabulary in
/// every other connection's request.
pub fn pull_arguments(
    spec: &holon_connections::ListSyncSpec,
    version: i64,
    device_id: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut args = serde_json::Map::new();
    args.insert("version".into(), serde_json::json!(version));
    args.insert("device_id".into(), serde_json::json!(device_id));
    // Where it IS declared it is load-bearing, not decoration: the write leg
    // re-reads to confirm its own commit landed, and a cached body there
    // reports that write as missing and gets it sent twice.
    match spec.cache_buster {
        CacheBuster::None => {}
        CacheBuster::EpochMillis => {
            args.insert(
                "nocache".into(),
                serde_json::json!(chrono::Utc::now().timestamp_millis()),
            );
        }
    }
    args
}

/// The local rows a round decides against, read from the declared type's write
/// authority: a projection of it lags the previous round's writes, and a row it
/// does not show yet would be created a second time. Read-only by design: the
/// writes go back through the dispatcher as follow-up operations.
pub struct AuthorityRows {
    authority: Arc<dyn TypeWriteAuthority>,
    type_def: TypeDefinition,
}

impl AuthorityRows {
    pub fn new(authority: Arc<dyn TypeWriteAuthority>, type_def: TypeDefinition) -> Self {
        Self {
            authority,
            type_def,
        }
    }
}

#[async_trait]
impl LocalRowReader for AuthorityRows {
    async fn load(&self) -> Result<Vec<LocalRow>> {
        let name = &self.type_def.name;
        let rows = self
            .authority
            .read_all(&self.type_def)
            .await
            .map_err(|e| anyhow::anyhow!("reading the local '{name}' rows: {e}"))?;
        rows.into_iter()
            .map(|row| {
                let columns: std::collections::BTreeMap<String, Value> =
                    row.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
                let id = match columns.get("id") {
                    Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
                    other => anyhow::bail!(
                        "a '{name}' row carries no usable `id` (got {other:?}); the primary key \
                         is what every follow-up operation addresses it by"
                    ),
                };
                Ok(LocalRow { id, columns })
            })
            .collect()
    }
}

/// The list connection `name`'s sidecar declares, served by its connected
/// `integration`.
///
/// The refusal is a STRING, not an error to propagate: a sidecar this build
/// cannot serve must not take the integration down, and the text names the
/// connection and the reason. A spec whose entity the registry does not
/// declare, or whose type declares no soft deletion, is refused here rather
/// than producing a wrong round later.
pub fn configured_list(
    name: &str,
    spec: holon_connections::ListSyncSpec,
    integration: &McpIntegration,
    authority: Arc<dyn TypeWriteAuthority>,
    types: &TypeRegistry,
) -> std::result::Result<ConfiguredList, String> {
    let entity = spec.entity.clone();
    let declared = types.get(&entity).ok_or_else(|| {
        format!(
            "connection '{name}' declares `list_sync.entity` as '{entity}', which the type \
             registry does not declare; the entity is what the mirror's rows are and what every \
             write back goes through"
        )
    })?;
    let compiled = CompiledListSync::compile(name, spec, &declared)
        .map_err(|e| format!("connection '{name}': {e:#}"))?;
    Ok(ConfiguredList {
        connection: name.to_string(),
        compiled: compiled.clone(),
        peer: Arc::new(RestListPeer::new(
            integration.sync_engine.call_surface(),
            compiled.clone(),
            device_id(),
        )),
        rows: Arc::new(AuthorityRows::new(authority, declared)),
        device_id: device_id().to_string(),
    })
}
