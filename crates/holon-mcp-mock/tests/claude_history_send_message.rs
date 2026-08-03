//! E2E coverage of the `send_message` WRITE tool declared in the SHIPPED
//! sidecar `docs/integrations/claude-history.yaml`, driven against the mock MCP
//! server (`MOCK_MCP_SCENARIO=live_fleet_send`) — NEVER the real provider
//! binary: a real dispatch would message a live human-owned agent session.
//!
//! Sending is irreversible and has no natural dedup key, so the declaration is
//! `effect: once_only` under `once_only: confirm_manually`. These tests assert
//! the two safety properties that buys: a send never fires unattended, and one
//! intent never fires twice.
//!
//! The mock's `applied_count` is cumulative per connection, so every assertion
//! about "how many effects fired" is read off the counter rather than inferred.

use std::collections::HashMap;
use std::sync::Arc;

use holon::di::DbHandleCacheFactory;
use holon_api::EntityName;
use holon_api::StreamPosition;
use holon_api::Value;
use holon_core::Delivery;
use holon_core::OperationProvider;
use holon_core::SyncTokenStore;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpConnectionResult;
use holon_mcp_client::McpIntegration;
use holon_mcp_client::PendingOAuthFlows;
use holon_mcp_client::PendingState;
use holon_mcp_client::SyncGate;
use holon_mcp_client::build_mcp_integration;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock-mcp-server");

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

async fn setup_db() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
}

async fn connect_shipped_sidecar(db: &DbHandle) -> McpIntegration {
    connect_scenario(db, "live_fleet_send").await
}

async fn connect_scenario(db: &DbHandle, scenario: &str) -> McpIntegration {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/integrations/claude-history.yaml"
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
        .insert("MOCK_MCP_SCENARIO".to_string(), scenario.to_string());

    let mcp_config = cfg
        .into_mcp_config("claude-history".to_string())
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

fn send_params(session_id: &str, message: &str) -> holon_api::StorageEntity {
    let mut params = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(session_id.to_string()));
    params.insert("message".into(), Value::String(message.to_string()));
    params
}

fn applied_count(result: &holon_core::OperationResult) -> i64 {
    let resp = result.response.as_ref().expect("send response present");
    match resp {
        Value::Object(m) => match m.get("applied_count") {
            Some(Value::Integer(n)) => *n,
            other => panic!("applied_count missing/non-integer: {other:?}"),
        },
        other => panic!("expected object response, got {other:?}"),
    }
}

const SESSION: &str = "session";

/// (a) A send is QUEUED, not fired. The caller gets the disclosed
/// "queued for confirmation" error, the intent sits in the pending-writes queue
/// awaiting approval, and the remote saw no call at all — proven below by the
/// approved dispatch reporting `applied_count == 1`, which it could not do had
/// the first attempt fired.
#[tokio::test(flavor = "multi_thread")]
async fn send_message_is_queued_for_confirmation_not_dispatched() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    let provider = &integration.operation_provider;

    let err = provider
        .execute_operation(
            &EntityName::from(SESSION),
            "send_message",
            send_params("s-bg-1", "ping"),
        )
        .await
        .expect_err("confirm_manually must queue a send, never fire it unattended");
    let msg = err.to_string();
    assert!(
        msg.contains("queued for confirmation") && msg.contains("confirm_manually"),
        "the error must disclose the pending-confirmation gate, got: {msg}"
    );

    let pending = provider.pending_writes();
    assert_eq!(
        pending.len(),
        1,
        "exactly one queued intent, got {pending:?}"
    );
    assert_eq!(pending[0].tool, "send_message");
    assert_eq!(pending[0].state, PendingState::AwaitingConfirmation);

    let approved = provider
        .approve(&pending[0].intent_key)
        .await
        .expect("approval re-dispatches the stored call");
    assert_eq!(
        applied_count(&approved),
        1,
        "the approved dispatch is the FIRST remote effect — the queued attempt did not fire"
    );
}

/// (b) One intent never fires twice. After the approved dispatch, neither a
/// re-approval nor an identical re-send reaches the remote; a genuinely
/// different send then reports `applied_count == 2`, proving the refused
/// attempts contributed no effect.
#[tokio::test(flavor = "multi_thread")]
async fn approved_send_never_dispatches_twice() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    let provider = &integration.operation_provider;

    provider
        .execute_operation(
            &EntityName::from(SESSION),
            "send_message",
            send_params("s-bg-1", "ping"),
        )
        .await
        .expect_err("first attempt is queued");
    let key = provider.pending_writes()[0].intent_key.clone();

    let approved = provider.approve(&key).await.expect("first approve fires");
    assert_eq!(applied_count(&approved), 1);
    assert_eq!(
        provider.pending_store().state_of(&key),
        Some(PendingState::Sent)
    );

    let second_approve = provider
        .approve(&key)
        .await
        .expect_err("a second approval of a sent intent must be refused");
    assert!(
        second_approve.to_string().contains("already approved"),
        "the refusal must be disclosed, got: {second_approve}"
    );

    let resend = provider
        .execute_operation(
            &EntityName::from(SESSION),
            "send_message",
            send_params("s-bg-1", "ping"),
        )
        .await
        .expect_err("an identical re-send must not fire the effect again");
    assert!(
        resend.to_string().contains("send_message"),
        "the refusal must name the tool, got: {resend}"
    );
    assert_eq!(
        provider.pending_store().state_of(&key),
        Some(PendingState::Sent),
        "the sent intent stays sent — no second dispatch was taken"
    );

    // A distinct intent is the probe: it reports the SECOND effect, so neither
    // refused attempt above reached the remote.
    let other = provider
        .execute_operation(
            &EntityName::from(SESSION),
            "send_message",
            send_params("s-bg-1", "a different message"),
        )
        .await
        .expect_err("a distinct intent is itself queued for confirmation");
    assert!(other.to_string().contains("queued for confirmation"));
    let other_key = provider
        .pending_writes()
        .into_iter()
        .find(|w| w.state == PendingState::AwaitingConfirmation)
        .expect("the distinct intent is awaiting confirmation")
        .intent_key;
    let second_effect = provider.approve(&other_key).await.expect("approve fires");
    assert_eq!(
        applied_count(&second_effect),
        2,
        "exactly two remote effects total: one per approved intent"
    );
}

/// (c) HEADLINE. An ack the provider itself labels UNPROVEN
/// (`{"outcome":"unconfirmed"}`) is a *successful* MCP response, but it is not
/// delivery. Reading it as delivery is the "silently degrades to look fine"
/// failure: Holon would show a sent message that may never have reached the
/// session. The intent must land in `OutcomeUnknown` — the state whose whole
/// contract is "disclosed, never auto-retried".
#[tokio::test(flavor = "multi_thread")]
async fn an_unproven_ack_is_not_recorded_as_delivered() {
    let db = setup_db().await;
    let integration = connect_scenario(&db, "live_fleet_send_unconfirmed").await;
    let provider = &integration.operation_provider;

    provider
        .execute_operation(
            &EntityName::from(SESSION),
            "send_message",
            send_params("s-bg-1", "ping"),
        )
        .await
        .expect_err("first attempt is queued for confirmation");
    let key = provider.pending_writes()[0].intent_key.clone();

    let approved = provider
        .approve(&key)
        .await
        .expect("an unproven ack is still a successful call, not a dispatch error");
    assert_eq!(
        applied_count(&approved),
        1,
        "precondition: the remote call happened exactly once"
    );

    // What the UI is told: the verdict rides on the dispatch result, so a
    // compose box cannot read `Ok` and assume the message landed.
    let Delivery::Unproven { detail } = &approved.delivery else {
        panic!(
            "an unproven ack must reach the caller as `Unproven`; got {:?}",
            approved.delivery
        )
    };
    assert!(
        detail.contains("unconfirmed"),
        "the caller-facing detail must carry the provider's own outcome word; got: {detail}"
    );

    let state = provider
        .pending_store()
        .state_of(&key)
        .expect("the approved intent is tracked");
    let PendingState::OutcomeUnknown { detail } = &state else {
        panic!("an ack of `outcome: unconfirmed` must NOT be recorded as delivered; got {state:?}")
    };
    assert!(
        detail.contains("unconfirmed"),
        "the disclosure must carry the provider\'s own outcome word; got: {detail}"
    );
}

/// (d) The counterpart: an ack the provider labels PROVEN (`delivered`) IS
/// delivery. Without this the fix could over-correct into never claiming
/// success, which is the same lie in the other direction.
#[tokio::test(flavor = "multi_thread")]
async fn a_proven_ack_is_recorded_as_delivered() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    let provider = &integration.operation_provider;

    provider
        .execute_operation(
            &EntityName::from(SESSION),
            "send_message",
            send_params("s-bg-1", "ping"),
        )
        .await
        .expect_err("first attempt is queued for confirmation");
    let key = provider.pending_writes()[0].intent_key.clone();
    let approved = provider.approve(&key).await.expect("approve fires");

    assert_eq!(
        approved.delivery,
        Delivery::Proven,
        "a proven ack must reach the caller as `Proven`"
    );
    assert_eq!(
        provider.pending_store().state_of(&key),
        Some(PendingState::Sent),
        "an ack of `outcome: delivered` proves the message landed"
    );
}
