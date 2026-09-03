//! The shopping-list peer over the `utcp:` connection declared in
//! `assets/integrations/shopping.yaml`.
//!
//! Both legs are sidecar calls and both MAPPINGS are sidecar filters, so the
//! endpoint shape, the commit envelope, the version path and the peer's JSON
//! vocabulary all live in YAML. What stays here is the peer's own semantics —
//! that only a fully mapped body becomes a [`CompleteSnapshot`], and that a
//! commit's ack is read from the transport's provider-neutral version key.

use std::borrow::Cow;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use anyhow::Result;
use async_trait::async_trait;
use holon_kitchen::shopping::CompleteSnapshot;
use holon_kitchen::shopping_sync::CommitAck;
use holon_kitchen::shopping_sync::CommitBatch;
use holon_kitchen::shopping_sync::ShoppingPeer;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::rest_transport::RESPONSE_VERSION_KEY;
use rmcp::model::CallToolRequestParam;

/// The manual's tool that fetches one whole list.
pub const LIST_CALL: &str = "pull_list";
/// The manual's tool that commits a batch of commands.
pub const COMMIT_CALL: &str = "commit";

pub struct RestShoppingPeer {
    surface: std::sync::Arc<dyn McpCallSurface>,
    /// Stable per install, sent on every commit. Not a secret — the credential
    /// is the URL.
    device_id: String,
    /// The newest list version this peer has observed, echoed on the next read
    /// exactly as the captured client does. Zero until the first pull.
    last_version: AtomicI64,
}

impl RestShoppingPeer {
    /// The list is named by the manual's tool `url` — the whole share link,
    /// ending in the list id — so nothing here parses or carries a list id.
    pub fn new(surface: std::sync::Arc<dyn McpCallSurface>, device_id: impl Into<String>) -> Self {
        Self {
            surface,
            device_id: device_id.into(),
            last_version: AtomicI64::new(0),
        }
    }

    async fn call(
        &self,
        name: &'static str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        let result = self
            .surface
            .call_tool(CallToolRequestParam {
                name: Cow::Borrowed(name),
                arguments: Some(args),
            })
            .await
            .map_err(|e| anyhow::anyhow!("shopping peer: call '{name}': {e}"))?;
        let value = holon_mcp_client::mcp_call_surface::extract_tool_response(&result)
            .with_context(|| format!("shopping peer: reading the '{name}' response"))?;
        value.as_object().cloned().ok_or_else(|| {
            anyhow::anyhow!("shopping peer: call '{name}' answered a non-object body")
        })
    }

    /// Record the newest version seen, so the next read echoes it the way the
    /// captured client does.
    fn observe(&self, version: i64) {
        self.last_version.fetch_max(version, Ordering::Relaxed);
    }
}

/// Milliseconds since the epoch, the unit the captured `_nocache` carries.
fn epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[async_trait]
impl ShoppingPeer for RestShoppingPeer {
    async fn pull(&self) -> Result<CompleteSnapshot> {
        let seen = self.last_version.load(Ordering::Relaxed);
        let mut args = serde_json::Map::new();
        args.insert("oldVersion".into(), serde_json::json!(seen));
        args.insert("version".into(), serde_json::json!(seen));
        // The captured client's cache-buster, and not decoration: the write leg
        // re-reads to check its own commit landed, so a cached body there
        // reports the write as missing and gets it sent a second time.
        args.insert("nocache".into(), serde_json::json!(epoch_ms()));
        let response = self.call(LIST_CALL, args).await?;
        let rows = self
            .surface
            .map_response(LIST_CALL, &serde_json::Value::Object(response))?;
        // The fetch time is stamped here, where the body is known to have
        // mapped: it becomes the `last_seen_remote` watermark, and a watermark
        // from a fetch that failed would license a later absence-as-deletion.
        let snapshot = CompleteSnapshot::from_rows(&rows, chrono::Utc::now().to_rfc3339())?;
        self.observe(snapshot.version().list);
        Ok(snapshot)
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<CommitAck> {
        // The batch's device id is the reconciler's; this peer's own is what
        // the install is known by, so the batch carries it into the mapping
        // rather than the mapping reaching for a second source.
        let mut stream = batch.to_row_stream();
        stream["rows"][0]["row"]["device_id"] = serde_json::Value::String(self.device_id.clone());
        let args = self.surface.map_request(COMMIT_CALL, &stream)?;

        let response = self.call(COMMIT_CALL, args).await?;
        let version = response
            .get(RESPONSE_VERSION_KEY)
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "shopping peer: the commit response carries no whole-number version under \
                     '{RESPONSE_VERSION_KEY}'; the sidecar's response_version_path is what puts \
                     it there"
                )
            })?;
        // The peer versions the picked-items map separately and answers with
        // both; a response that omits the second one leaves it where it was.
        let picked_items_version = response
            .get("pickedItemsVersion")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(batch.old_picked_items_version);
        self.observe(version);
        Ok(CommitAck {
            version,
            picked_items_version,
        })
    }
}
