//! The transport half of a remote-list connection: the peer a round talks to,
//! and the local rows it decides against.
//!
//! Everything here is parameterised by [`CompiledListSync`], so a second peer
//! adds a sidecar and no Rust. The half that is NOT here — reconcile, the round
//! and the intents — lives in `crates/holon-connections`, which is in the wasm
//! graphs; this module is the part that must not be, because it reaches a
//! socket and a database.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use anyhow::Result;
use async_trait::async_trait;
use holon::storage::DbHandle;
use holon_api::Value;
use holon_connections::CacheBuster;
use holon_connections::CommitAck;
use holon_connections::CommitBatch;
use holon_connections::CompiledListSync;
use holon_connections::ConfiguredList;
use holon_connections::ConfiguredLists;
use holon_connections::ListSnapshot;
use holon_connections::LocalRow;
use holon_connections::LocalRowReader;
use holon_connections::RemoteListPeer;
use holon_mcp_client::integration_config::IntegrationFileConfig;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::mcp_call_surface::extract_tool_response;
use holon_mcp_client::rest_transport::RESPONSE_VERSION_KEY;
use holon_profiles::TypeRegistry;
use rmcp::model::CallToolRequestParam;

use crate::mcp_integrations::McpIntegrationRegistry;

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

/// A peer whose connection is declared but not connected.
///
/// It answers every call with the disclosure rather than being absent: an
/// operation that silently does not exist would leave the caller with
/// "no configured connection serves this" and nothing about WHY the one it
/// configured is not answering.
pub struct UnreachablePeer {
    connection: String,
    why: String,
}

#[async_trait]
impl RemoteListPeer for UnreachablePeer {
    async fn pull(&self) -> Result<ListSnapshot> {
        anyhow::bail!(
            "the '{}' connection is declared but not connected, so there is nothing to sync \
             with: {}",
            self.connection,
            self.why
        )
    }

    async fn commit(&self, _: &CommitBatch) -> Result<CommitAck> {
        anyhow::bail!(
            "the '{}' connection is declared but not connected: {}",
            self.connection,
            self.why
        )
    }
}

/// The local rows a round decides against, read from the table the sidecar
/// names. Read-only by design: the writes go back through the dispatcher as
/// follow-up operations.
pub struct SqlMirrorRows {
    db_handle: DbHandle,
    table: String,
}

impl SqlMirrorRows {
    pub fn new(db_handle: DbHandle, compiled: &CompiledListSync) -> Self {
        Self {
            db_handle,
            table: compiled.spec().table.clone(),
        }
    }
}

#[async_trait]
impl LocalRowReader for SqlMirrorRows {
    async fn load(&self) -> Result<Vec<LocalRow>> {
        // `SELECT *` rather than the declared columns: the table is the mirror
        // of the declared type and may carry engine columns (the overflow bag,
        // provenance) that no sidecar names. Which of them a row HAS is the
        // declared type's business, checked where a row is written.
        let sql = format!("SELECT * FROM {}", self.table);
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("reading the local mirror table '{}': {e}", self.table))?;
        rows.iter()
            .map(|row| {
                let columns: std::collections::BTreeMap<String, Value> = row
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect();
                let id = match columns.get("id") {
                    Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
                    other => anyhow::bail!(
                        "a '{}' row carries no usable `id` (got {other:?}); the mirror's primary \
                         key is what every follow-up operation addresses it by",
                        self.table
                    ),
                };
                Ok(LocalRow { id, columns })
            })
            .collect()
    }
}

/// Every list connection this build can reach, assembled from the sidecars it
/// loaded.
///
/// A sidecar that declares `holon.list_sync` gets one connection. Everything
/// that is not the transport is validated here, at boot: a spec whose entity
/// the registry does not declare, or whose type declares no soft deletion, is
/// carried back as a refusal rather than a wrong round later.
///
/// A connection whose spec cannot compile is NOT skipped silently: the boot
/// stays up (a bad sidecar must not take the engine down) and the refusal is
/// reported by name at the dispatch that finds no operation for it.
pub fn configured_lists(
    configs: &[(String, IntegrationFileConfig)],
    registry: &McpIntegrationRegistry,
    db_handle: &DbHandle,
    types: &TypeRegistry,
) -> ConfiguredLists {
    let mut lists = Vec::new();
    let mut refusals = Vec::new();
    for (name, config) in configs {
        let Some(spec) = config
            .holon
            .as_ref()
            .and_then(|holon| holon.list_sync.clone())
        else {
            continue;
        };
        match build_one(name, spec, registry, db_handle, types) {
            Ok(list) => lists.push(list),
            Err(why) => refusals.push(why),
        }
    }
    ConfiguredLists::new(lists, refusals)
}

/// The refusal is a STRING, not an error to propagate: a sidecar this build
/// cannot serve must not take the boot down, and the text is what the dispatch
/// that finds no operation reports by name.
fn build_one(
    name: &str,
    spec: holon_connections::ListSyncSpec,
    registry: &McpIntegrationRegistry,
    db_handle: &DbHandle,
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
    let peer: Arc<dyn RemoteListPeer> = match registry.by_name(name) {
        Some(integration) => Arc::new(RestListPeer::new(
            integration.sync_engine.call_surface(),
            compiled.clone(),
            device_id(),
        )),
        None => Arc::new(UnreachablePeer {
            connection: name.to_string(),
            why: NOT_CONNECTED_WHY.to_string(),
        }),
    };
    Ok(ConfiguredList {
        connection: name.to_string(),
        compiled: compiled.clone(),
        peer,
        rows: Arc::new(SqlMirrorRows::new(db_handle.clone(), &compiled)),
        device_id: device_id().to_string(),
    })
}

/// The disclosure a declared-but-unconnected list connection carries. Its boot
/// outcome is recorded on the `integration_state` mirror by
/// [`crate::mcp_integrations`]; a missing credential for its `utcp:` URL is the
/// usual cause.
const NOT_CONNECTED_WHY: &str = "it is declared but not connected, and its boot outcome was \
                                 disclosed on the degraded bus — a missing credential for its \
                                 `utcp:` URL is the usual cause";
