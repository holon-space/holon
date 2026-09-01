//! E2E coverage of the `live_session` entity declared in the SHIPPED sidecar
//! `assets/integrations/claude-history.yaml`, driven against the mock MCP
//! server (`MOCK_MCP_SCENARIO=live_fleet`) rather than the real provider
//! binary.
//!
//! The sidecar is loaded from the repo, so these tests fail if the declaration
//! is missing or its columns drift away from the provider's `LiveSession`.

use std::collections::HashMap;
use std::sync::Arc;

use holon::di::DbHandleCacheFactory;
use holon_api::StreamPosition;
use holon_api::Value;
use holon_core::SyncTokenStore;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpConnectionResult;
use holon_mcp_client::McpIntegration;
use holon_mcp_client::PendingOAuthFlows;
use holon_mcp_client::SyncGate;
use holon_mcp_client::build_mcp_integration;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock-mcp-server");
const TABLE: &str = "cc_live_session";

struct InMemorySyncTokenStore {
    tokens: tokio::sync::Mutex<HashMap<String, StreamPosition>>,
}

#[async_trait::async_trait]
impl SyncTokenStore for InMemorySyncTokenStore {
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

async fn connect_shipped_sidecar(db: &DbHandle) -> McpIntegration {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/integrations/claude-history.yaml"
    );
    let yaml = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut cfg: IntegrationFileConfig =
        serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("parse {path}: {e}"));
    let cp = cfg
        .transport
        .child_process
        .as_mut()
        .expect("claude-history sidecar must declare child_process transport");
    cp.command = MOCK_BIN.to_string();
    cp.env
        .insert("MOCK_MCP_SCENARIO".to_string(), "live_fleet".to_string());

    let mcp_config = cfg
        .into_mcp_config(
            "claude-history".to_string(),
            &holon_mcp_client::CredentialRoot::new("/tmp/holon-mcp-mock-config"),
        )
        .expect("sidecar into_mcp_config");
    let result = build_mcp_integration(
        mcp_config,
        db.clone(),
        Arc::new(DbHandleCacheFactory::new(db.clone())),
        Arc::new(InMemorySyncTokenStore {
            tokens: tokio::sync::Mutex::new(HashMap::new()),
        }),
        &PendingOAuthFlows::new(),
        SyncGate::opened(),
    )
    .await
    .expect("connect claude-history sidecar against the mock");
    match result {
        McpConnectionResult::Connected(integration) => integration,
        McpConnectionResult::NeedsAuth { provider_name, .. } => {
            panic!("unexpected NeedsAuth for '{provider_name}'")
        }
    }
}

async fn setup_db() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
}

/// Every cached live row as `session_id -> (kind, state, tempo, needs,
/// job_id)`.
async fn live_rows(db: &DbHandle) -> HashMap<String, Vec<Option<String>>> {
    let rows = db
        .query(
            &format!("SELECT session_id, kind, state, tempo, needs, job_id FROM {TABLE}"),
            HashMap::new(),
        )
        .await
        .expect("query cc_live_session");
    rows.iter()
        .map(|r| {
            let text = |c: &str| match r.get(c) {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Null) | None => None,
                other => panic!("column {c}: unexpected value {other:?}"),
            };
            let session_id = text("session_id").expect("session_id is never null");
            let cols = ["kind", "state", "tempo", "needs", "job_id"]
                .iter()
                .map(|c| text(c))
                .collect();
            (session_id, cols)
        })
        .collect()
}

/// (a) A running background session lands in `cc_live_session` with its
/// liveness columns mapped.
#[tokio::test(flavor = "multi_thread")]
async fn running_background_session_is_cached_with_liveness_columns() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    integration.sync_engine.sync_all().await.expect("sync_all");

    let rows = live_rows(&db).await;
    assert_eq!(
        rows.keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["s-bg-1".to_string(), "s-fg-1".to_string()].into(),
        "both live sessions cached"
    );
    assert_eq!(
        rows["s-bg-1"],
        vec![
            Some("bg".to_string()),
            Some("running".to_string()),
            Some("steady".to_string()),
            Some("nothing".to_string()),
            Some("job-77".to_string()),
        ],
        "background session's kind/state/tempo/needs/job_id"
    );
}

/// (b) MIRROR SEMANTICS. The provider drops a session from the listing once it
/// reaches a terminal state; the cache must drop it too, or a later feature
/// would target a dead session.
#[tokio::test(flavor = "multi_thread")]
async fn session_absent_from_a_later_listing_is_evicted() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    let engine = &integration.sync_engine;

    engine.sync_all().await.expect("first sync");
    assert!(
        live_rows(&db).await.contains_key("s-bg-1"),
        "background session present after the first listing"
    );

    engine.sync_all().await.expect("second sync");
    let rows = live_rows(&db).await;
    assert!(
        !rows.contains_key("s-bg-1"),
        "session missing from the second listing must be evicted, got {:?}",
        rows.keys().collect::<Vec<_>>()
    );
    assert!(
        rows.contains_key("s-fg-1"),
        "still-live session survives the mirror update"
    );
}
