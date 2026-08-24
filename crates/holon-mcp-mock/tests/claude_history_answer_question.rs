//! E2E coverage of the `pending_question` entity and the `answer_question`
//! WRITE tool declared in the SHIPPED sidecar
//! `assets/integrations/claude-history.yaml`, driven against the mock MCP
//! server (`MOCK_MCP_SCENARIO=live_fleet_answer`) — NEVER the real provider
//! binary: a real dispatch would answer a live human-owned agent session.
//!
//! Answering is irreversible and the provider exposes no idempotency
//! parameter, so the declaration is `effect: once_only` under
//! `once_only: confirm_manually`. An UNDECLARED tool would resolve to
//! `ToolEffect::Read` and fire unattended on the first click — that is the
//! property test (b) pins down.

use std::collections::HashMap;
use std::sync::Arc;

use holon::di::DbHandleCacheFactory;
use holon_api::EntityName;
use holon_api::StreamPosition;
use holon_api::Value;
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
const TABLE: &str = "cc_pending_question";
// The entity the connector ADVERTISES: `entity_prefix` applied, the form
// `EntityName` canonicalizes to. Dispatch selects a provider by its own
// descriptor's entity, so this — not the sidecar's internal key — is the only
// name `execute_operation` accepts.
const ENTITY: &str = "cc_pending_question";
/// The mirror stores the primary key scheme-qualified.
const ROW_ID: &str = "cc-pending-question:job-77:0:1a2b3c4d";
/// The provider's opaque `answer_question.question_id` — the URI path of
/// ROW_ID.
const QUESTION_ID: &str = "job-77:0:1a2b3c4d";

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
    cp.env.insert(
        "MOCK_MCP_SCENARIO".to_string(),
        "live_fleet_answer".to_string(),
    );

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

/// The arguments the provider declares: a `question_id` and the chosen labels
/// as an ARRAY. One clicked button is a one-element array.
fn answer_params(question_id: &str, labels: &[&str]) -> holon_api::StorageEntity {
    let mut params = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(question_id.to_string()));
    params.insert("question_id".into(), Value::String(question_id.to_string()));
    params.insert(
        "answers".into(),
        Value::Array(
            labels
                .iter()
                .map(|l| Value::String((*l).to_string()))
                .collect(),
        ),
    );
    params
}

fn recorded(result: &holon_core::OperationResult) -> String {
    let resp = result.response.as_ref().expect("answer response present");
    match resp {
        Value::Object(m) => match m.get("recorded") {
            Some(Value::String(s)) => s.clone(),
            other => panic!("`recorded` missing/non-string: {other:?}; full: {m:?}"),
        },
        other => panic!("expected object response, got {other:?}"),
    }
}

/// (a) The pending questions a live session is blocked on land in
/// `cc_pending_question`, options JSON intact — that column is what the UI
/// turns into one button per offered answer.
#[tokio::test(flavor = "multi_thread")]
async fn pending_questions_are_mirrored_with_their_options() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    integration.sync_engine.sync_all().await.expect("sync_all");

    let rows = db
        .query(
            &format!(
                "SELECT id, session_id, job_id, question_index, question, options, answerable \
                 FROM {TABLE} ORDER BY question_index"
            ),
            HashMap::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("query {TABLE}: {e}"));
    assert_eq!(rows.len(), 2, "both pending questions cached");

    let head = &rows[0];
    assert_eq!(
        head.get("id"),
        Some(&Value::String(ROW_ID.to_string())),
        "the mirror scheme-qualifies the primary key"
    );
    assert_eq!(
        holon_api::EntityUri::parse(ROW_ID)
            .expect("row id is an entity URI")
            .id(),
        QUESTION_ID,
        "unwrapping the scheme yields the id the tool accepts"
    );
    assert_eq!(head.get("answerable"), Some(&Value::Integer(1)));

    let Some(Value::String(options)) = head.get("options") else {
        panic!(
            "options must be cached as TEXT, got {:?}",
            head.get("options")
        )
    };
    let parsed: serde_json::Value =
        serde_json::from_str(options).unwrap_or_else(|e| panic!("options must be JSON: {e}"));
    let labels: Vec<&str> = parsed
        .as_array()
        .expect("options is an array")
        .iter()
        .map(|o| o["label"].as_str().expect("each option has a label"))
        .collect();
    assert_eq!(labels, vec!["Turso", "Plain SQLite"]);
}

/// (b) An answer is QUEUED, not fired. This is what the `effect: once_only`
/// declaration buys: without it the tool would classify as `Read` and the
/// first click would answer a human's live session unattended.
#[tokio::test(flavor = "multi_thread")]
async fn answer_is_queued_for_confirmation_not_dispatched() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    let provider = &integration.operation_provider;

    let err = provider
        .execute_operation(
            &EntityName::from(ENTITY),
            "answer_question",
            answer_params(QUESTION_ID, &["Turso"]),
        )
        .await
        .expect_err("confirm_manually must queue an answer, never fire it unattended");
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
    assert_eq!(pending[0].tool, "answer_question");
    assert_eq!(pending[0].state, PendingState::AwaitingConfirmation);

    let approved = provider
        .approve(&pending[0].intent_key)
        .await
        .expect("approval re-dispatches the stored call");
    assert_eq!(
        recorded(&approved),
        "Turso",
        "the approved dispatch is the FIRST remote effect and records the chosen label"
    );
}

/// (c) One answer never fires twice — a second approval of a sent intent is
/// refused, and the intent stays `Sent`.
#[tokio::test(flavor = "multi_thread")]
async fn approved_answer_never_dispatches_twice() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    let provider = &integration.operation_provider;

    provider
        .execute_operation(
            &EntityName::from(ENTITY),
            "answer_question",
            answer_params(QUESTION_ID, &["Turso"]),
        )
        .await
        .expect_err("first attempt is queued");
    let key = provider.pending_writes()[0].intent_key.clone();

    provider.approve(&key).await.expect("first approve fires");
    assert_eq!(
        provider.pending_store().state_of(&key),
        Some(PendingState::Sent)
    );

    let second = provider
        .approve(&key)
        .await
        .expect_err("a second approval of a sent intent must be refused");
    assert!(
        second.to_string().contains("already approved"),
        "the refusal must be disclosed, got: {second}"
    );
    assert_eq!(
        provider.pending_store().state_of(&key),
        Some(PendingState::Sent),
        "the sent intent stays sent — no second dispatch was taken"
    );
}

/// (d) The chosen labels ride under `answers`, as an ARRAY. A scalar under any
/// name is what the shipped build dispatched, and the provider refuses it — so
/// every click died at the binary while every test here passed.
#[tokio::test(flavor = "multi_thread")]
async fn a_scalar_label_is_refused_by_the_provider_contract() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    let provider = &integration.operation_provider;

    let mut params = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(QUESTION_ID.to_string()));
    params.insert("question_id".into(), Value::String(QUESTION_ID.to_string()));
    params.insert("label".into(), Value::String("Turso".to_string()));

    provider
        .execute_operation(&EntityName::from(ENTITY), "answer_question", params)
        .await
        .expect_err("queued for confirmation");
    let key = provider.pending_writes()[0].intent_key.clone();

    let err = provider
        .approve(&key)
        .await
        .expect_err("a scalar label must be refused at the provider");
    assert!(
        err.to_string()
            .contains("cannot answer: answers must be an array of option labels"),
        "the refusal must be the provider's own, got: {err}"
    );
}

/// (e) A multi-select answer is several labels in one array, and the provider
/// records them `", "`-joined — the join is what makes them parse as several
/// selections.
#[tokio::test(flavor = "multi_thread")]
async fn several_labels_are_recorded_comma_joined() {
    let db = setup_db().await;
    let integration = connect_shipped_sidecar(&db).await;
    let provider = &integration.operation_provider;

    provider
        .execute_operation(
            &EntityName::from(ENTITY),
            "answer_question",
            answer_params(QUESTION_ID, &["Turso", "Plain SQLite"]),
        )
        .await
        .expect_err("queued for confirmation");
    let key = provider.pending_writes()[0].intent_key.clone();

    let approved = provider.approve(&key).await.expect("approve fires");
    assert_eq!(recorded(&approved), "Turso, Plain SQLite");
}
