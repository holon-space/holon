//! Live probe of the Todoist `sync:` path (ToolSync → cursor), exercising the
//! exact production code: env-expanded static token, live HTTP connect, the
//! sidecar `into_strategy()`, `fetch_records()` (which now reads
//! `structured_content`), and a cursor round-trip to a second page.
//!
//! Usage:
//!   TODOIST_API_KEY=... cargo run -p holon-mcp-client --example
//! todoist_sync_probe -- \     docs/integrations/todoist.yaml

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use holon_api::StreamPosition;
use holon_core::SyncTokenStore;
use holon_mcp_client::AuthMode;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::connect_mcp;

struct MemTokenStore {
    tokens: tokio::sync::Mutex<HashMap<String, StreamPosition>>,
}
impl MemTokenStore {
    fn new() -> Self {
        Self {
            tokens: tokio::sync::Mutex::new(HashMap::new()),
        }
    }
}
#[async_trait::async_trait]
impl SyncTokenStore for MemTokenStore {
    async fn save_token(&self, key: &str, position: StreamPosition) -> holon_core::Result<()> {
        self.tokens.lock().await.insert(key.to_string(), position);
        Ok(())
    }
    async fn load_token(&self, key: &str) -> holon_core::Result<Option<StreamPosition>> {
        Ok(self.tokens.lock().await.get(key).cloned())
    }
    async fn clear_all_tokens(&self) -> holon_core::Result<()> {
        self.tokens.lock().await.clear();
        Ok(())
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let yaml_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "docs/integrations/todoist.yaml".to_string())
        .into();

    let yaml = std::fs::read_to_string(&yaml_path)
        .with_context(|| format!("read {}", yaml_path.display()))?;
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(&yaml).with_context(|| format!("parse {}", yaml_path.display()))?;

    // into_mcp_config performs the ${VAR} env-expansion on uri + static_token.
    let runtime_cfg = cfg.clone().into_mcp_config("todoist".into())?;
    let uri = match &runtime_cfg.transport {
        McpTransport::Http { uri } => uri.clone(),
        _ => bail!("todoist.yaml must use http transport"),
    };
    let token = match &runtime_cfg.auth_mode {
        AuthMode::StaticToken(t) => Some(t.clone()),
        AuthMode::None => None,
        other => bail!("unexpected auth mode: {other:?}"),
    };

    println!("# Todoist sync probe");
    println!("  uri:   {uri}");
    println!(
        "  token: {} (env-expanded)",
        token
            .as_deref()
            .map(mask)
            .unwrap_or_else(|| "<none>".into())
    );
    println!();

    let (peer, _service) = connect_mcp(&uri, token.as_deref())
        .await
        .with_context(|| format!("connect_mcp({uri})"))?;

    let store = MemTokenStore::new();
    let mut any_empty = false;

    for (entity_name, entity) in &cfg.entities {
        let Some(sync) = entity.sync.as_ref() else {
            continue;
        };
        let strategy = sync.into_strategy().context("into_strategy")?;
        let key = format!("todoist:{entity_name}");

        // ----- Page 1 (no cursor yet) -----
        let page1 = strategy
            .fetch_records(
                &peer as &dyn holon_mcp_client::mcp_call_surface::McpCallSurface,
                &store,
                &key,
            )
            .await
            .with_context(|| format!("fetch page 1 for {entity_name}"))?;
        let first_ids = sample_ids(&page1.records, entity.id_column.as_deref().unwrap_or("id"));
        println!(
            "{entity_name}: page1 = {} records | new_cursor = {} | sample = {:?}",
            page1.records.len(),
            page1
                .new_cursor
                .as_deref()
                .map(mask)
                .unwrap_or_else(|| "<none>".into()),
            first_ids,
        );
        if page1.records.is_empty() {
            any_empty = true;
        }

        // ----- Page 2 (feed cursor back through the token store) -----
        if let Some(cursor) = page1.new_cursor {
            store
                .save_token(&key, StreamPosition::Version(cursor.into_bytes()))
                .await
                .ok();
            let page2 = strategy
                .fetch_records(
                    &peer as &dyn holon_mcp_client::mcp_call_surface::McpCallSurface,
                    &store,
                    &key,
                )
                .await
                .with_context(|| format!("fetch page 2 for {entity_name}"))?;
            let second_ids =
                sample_ids(&page2.records, entity.id_column.as_deref().unwrap_or("id"));
            let advanced = second_ids.iter().all(|id| !first_ids.contains(id));
            println!(
                "{entity_name}: page2 = {} records | cursor advanced (no overlap) = {advanced} | \
                 sample = {:?}",
                page2.records.len(),
                second_ids,
            );
        } else {
            println!("{entity_name}: single page (no cursor returned)");
        }
        println!();
    }

    if any_empty {
        bail!("at least one entity returned 0 records on page 1");
    }
    println!("OK — Todoist sync path works end-to-end.");
    Ok(())
}

fn sample_ids(records: &[serde_json::Map<String, serde_json::Value>], id_col: &str) -> Vec<String> {
    records
        .iter()
        .take(3)
        .filter_map(|r| r.get(id_col).and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

/// Mask a secret/cursor for log output — show only length and last 4 chars.
fn mask(s: &str) -> String {
    let n = s.len();
    let tail: String = s
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("len={n}…{tail}")
}
