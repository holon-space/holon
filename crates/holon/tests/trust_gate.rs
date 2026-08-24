//! Engine-level tests for the C5 trust gate (`DispatchingOperationEngine`):
//! sub-threshold origins are COERCED into proposal emissions under
//! `block:proposals` (never reaching canonical state), confirmation is an
//! ordinary trusted-origin intent that promotes the wrapped op with dual
//! provenance, rejection retracts, and trusted origins pass through untouched.
//!
//! The harness mirrors `undo_foundation.rs`: a stub `block` provider over an
//! in-memory store that mimics the prod "unknown fields pack into properties
//! JSON" path, shared with a `UndoStateReader` so the engine's read seam sees
//! exactly what the provider wrote.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon::api::OperationDispatcher;
use holon::api::operation_engine::DispatchingOperationEngine;
use holon::api::operation_engine::OperationEngine;
use holon_api::ACCEPT_PROPOSAL_OP;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::OperationDescriptor;
use holon_api::PROPOSAL_PROPERTY;
use holon_api::PROPOSALS_ROOT_ID;
use holon_api::PROPOSED_BY_PROPERTY;
use holon_api::PROVENANCE_PROPERTY;
use holon_api::ProposalRecord;
use holon_api::ProposalStatus;
use holon_api::REJECT_PROPOSAL_OP;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result as DatasourceResult;
use holon_core::UndoStateReader;
use holon_core::storage::types::StorageEntity;
use holon_profiles::trust::TrustPolicy;

/// Params keys the stub treats as block columns; everything else packs into
/// the `properties` object (the prod unknown-fields path).
const COLUMNS: &[&str] = &["id", "parent_id", "content"];

/// id → (columns, properties-object)
type BlockTable = HashMap<String, (HashMap<String, Value>, HashMap<String, Value>)>;

#[derive(Default)]
struct BlockStore {
    blocks: Mutex<BlockTable>,
    /// Every (op_name, id) the provider executed, in order.
    log: Mutex<Vec<(String, String)>>,
}

impl BlockStore {
    fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.blocks.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    fn properties(&self, id: &str) -> Option<HashMap<String, Value>> {
        self.blocks
            .lock()
            .unwrap()
            .get(id)
            .map(|(_, props)| props.clone())
    }

    fn column(&self, id: &str, column: &str) -> Option<Value> {
        self.blocks
            .lock()
            .unwrap()
            .get(id)
            .and_then(|(cols, _)| cols.get(column).cloned())
    }

    fn ops(&self) -> Vec<(String, String)> {
        self.log.lock().unwrap().clone()
    }
}

struct StubBlockProvider {
    store: Arc<BlockStore>,
}

impl StubBlockProvider {
    fn descriptor(&self, name: &str) -> OperationDescriptor {
        OperationDescriptor {
            entity_name: EntityName::new("block"),
            name: name.to_string(),
            entity_short_name: String::new(),
            id_column: "id".to_string(),
            display_name: String::new(),
            description: String::new(),
            required_params: vec![],
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }
    }

    fn split(params: &StorageEntity) -> (HashMap<String, Value>, HashMap<String, Value>) {
        let mut columns = HashMap::new();
        let mut properties = HashMap::new();
        for (k, v) in params {
            if COLUMNS.contains(&k.as_ref()) {
                columns.insert(k.to_string(), v.clone());
            } else {
                properties.insert(k.to_string(), v.clone());
            }
        }
        (columns, properties)
    }
}

#[async_trait]
impl OperationProvider for StubBlockProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![
            self.descriptor("create"),
            self.descriptor("update"),
            self.descriptor("set_field"),
        ]
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or("stub: missing id")?
            .to_string();
        self.store
            .log
            .lock()
            .unwrap()
            .push((op_name.to_string(), id.clone()));
        let (columns, properties) = Self::split(&params);
        let mut blocks = self.store.blocks.lock().unwrap();
        match op_name {
            "create" => {
                blocks.insert(id, (columns, properties));
            }
            "update" => {
                let entry = blocks
                    .get_mut(&id)
                    .ok_or_else(|| format!("stub: update of unknown block {id}"))?;
                entry.0.extend(columns);
                entry.1.extend(properties);
            }
            "set_field" => {
                let field = params
                    .get("field")
                    .and_then(|v| v.as_string())
                    .ok_or("stub: set_field missing 'field'")?
                    .to_string();
                let value = params
                    .get("value")
                    .cloned()
                    .ok_or("stub: set_field missing 'value'")?;
                let entry = blocks
                    .get_mut(&id)
                    .ok_or_else(|| format!("stub: set_field on unknown block {id}"))?;
                if COLUMNS.contains(&field.as_str()) {
                    entry.0.insert(field, value);
                } else {
                    entry.1.insert(field, value);
                }
            }
            other => return Err(format!("stub: unknown op {other}").into()),
        }
        Ok(OperationResult::irreversible(Vec::new()))
    }
}

/// Reader over the same store: `properties` renders as a structured object,
/// any column as its stored value — mirroring what the SQL reader hands back.
struct StoreReader {
    store: Arc<BlockStore>,
}

#[async_trait]
impl UndoStateReader for StoreReader {
    async fn field_value(&self, entity_id: &str, field: &str) -> anyhow::Result<Option<Value>> {
        if field == "properties" {
            return Ok(self.store.properties(entity_id).map(Value::Object));
        }
        Ok(self.store.column(entity_id, field))
    }
}

const AGENT_PROPOSE_POLICY: &str = "rules:\n  - origin: agent\n    decision: propose\n";

struct Fixture {
    engine: DispatchingOperationEngine,
    store: Arc<BlockStore>,
}

fn fixture(policy_yaml: &str) -> Fixture {
    let store = Arc::new(BlockStore::default());
    let provider = StubBlockProvider {
        store: store.clone(),
    };
    let dispatcher = Arc::new(OperationDispatcher::new(vec![
        Arc::new(provider) as Arc<dyn OperationProvider>
    ]));
    let policy = TrustPolicy::parse_yaml(policy_yaml).expect("test policy parses");
    // `new` + reader-less would skip the idempotence read seam; wire the
    // reader through the same store the provider writes.
    let engine = DispatchingOperationEngine::new(dispatcher)
        .with_trust_policy(Arc::new(policy))
        .with_state_reader(Arc::new(StoreReader {
            store: store.clone(),
        }));
    Fixture { engine, store }
}

fn agent() -> OpOrigin {
    OpOrigin::Agent {
        session_id: "sess-1".to_string(),
        tool_call_id: "call-1".to_string(),
    }
}

fn create_params() -> StorageEntity {
    let mut params: StorageEntity = StorageEntity::new();
    params.insert("id".into(), Value::String("block:x".to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("block:journals".to_string()),
    );
    params.insert("content".into(), Value::String("hello".to_string()));
    params
}

async fn dispatch(
    engine: &DispatchingOperationEngine,
    op: &str,
    params: StorageEntity,
    origin: OpOrigin,
) -> anyhow::Result<Option<Value>> {
    engine
        .execute_operation(&EntityName::new("block"), op, params, origin)
        .await
        .map(|out| out.response)
}

fn response_field(response: &Option<Value>, key: &str) -> String {
    match response {
        Some(Value::Object(map)) => match map.get(key) {
            Some(Value::String(s)) => s.clone(),
            other => panic!("response field '{key}' not a string: {other:?}"),
        },
        other => panic!("expected object response, got {other:?}"),
    }
}

fn proposals_root() -> String {
    EntityUri::block(PROPOSALS_ROOT_ID).as_str().to_string()
}

/// The single proposal block id in the store (asserts exactly one exists).
fn sole_proposal_id(store: &BlockStore) -> String {
    let ids: Vec<String> = store
        .ids()
        .into_iter()
        .filter(|id| *id != proposals_root())
        .collect();
    assert_eq!(ids.len(), 1, "expected exactly one proposal block: {ids:?}");
    ids[0].clone()
}

fn proposal_record(store: &BlockStore, id: &str) -> ProposalRecord {
    let props = store.properties(id).expect("proposal block exists");
    ProposalRecord::from_value(props.get(PROPOSAL_PROPERTY).expect("_proposal property"))
        .expect("proposal record parses")
}

fn stamp_origin(props: &HashMap<String, Value>, key: &str) -> String {
    match props.get(key) {
        Some(Value::Object(map)) => match map.get("origin") {
            Some(Value::String(s)) => s.clone(),
            other => panic!("stamp '{key}' has no string origin: {other:?}"),
        },
        other => panic!("stamp '{key}' is not an object: {other:?}"),
    }
}

#[tokio::test]
async fn untrusted_create_coerces_to_proposal() {
    let fx = fixture(AGENT_PROPOSE_POLICY);

    let response = dispatch(&fx.engine, "create", create_params(), agent())
        .await
        .expect("coercion succeeds");

    assert_eq!(response_field(&response, "status"), "proposed");
    // Canonical target never created; only the proposals root + the proposal.
    assert!(
        !fx.store.ids().contains(&"block:x".to_string()),
        "canonical create must not reach the store: {:?}",
        fx.store.ids()
    );
    let proposal_id = sole_proposal_id(&fx.store);
    assert_eq!(response_field(&response, "proposal_id"), proposal_id);
    assert_eq!(
        fx.store.column(&proposal_id, "parent_id"),
        Some(Value::String(proposals_root())),
        "proposal lives under the proposal place"
    );

    let record = proposal_record(&fx.store, &proposal_id);
    assert_eq!(record.status, ProposalStatus::Pending);
    assert_eq!(record.op_name, "create");
    assert_eq!(
        record.params.get("content"),
        Some(&Value::String("hello".to_string()))
    );
    // Proposer provenance is the block's ordinary `_provenance` stamp.
    let props = fx.store.properties(&proposal_id).unwrap();
    assert_eq!(stamp_origin(&props, PROVENANCE_PROPERTY), "agent");
}

#[tokio::test]
async fn refire_converges_on_one_proposal() {
    let fx = fixture(AGENT_PROPOSE_POLICY);

    let first = dispatch(&fx.engine, "create", create_params(), agent())
        .await
        .unwrap();
    let second = dispatch(&fx.engine, "create", create_params(), agent())
        .await
        .unwrap();

    assert_eq!(
        response_field(&first, "proposal_id"),
        response_field(&second, "proposal_id"),
        "deterministic proposal id"
    );
    assert_eq!(response_field(&second, "status"), "already_proposed");
    sole_proposal_id(&fx.store);
    let creates = fx
        .store
        .ops()
        .iter()
        .filter(|(op, id)| op == "create" && *id != proposals_root())
        .count();
    assert_eq!(creates, 1, "re-fire must not re-create the proposal");
}

#[tokio::test]
async fn accept_promotes_with_identical_content_and_dual_provenance() {
    let fx = fixture(AGENT_PROPOSE_POLICY);
    dispatch(&fx.engine, "create", create_params(), agent())
        .await
        .unwrap();
    let proposal_id = sole_proposal_id(&fx.store);

    let mut accept_params: StorageEntity = StorageEntity::new();
    accept_params.insert("id".into(), Value::String(proposal_id.clone()));
    dispatch(
        &fx.engine,
        ACCEPT_PROPOSAL_OP,
        accept_params,
        OpOrigin::User,
    )
    .await
    .expect("accept succeeds");

    // Promoted with identical content under the id the proposer chose.
    assert_eq!(
        fx.store.column("block:x", "content"),
        Some(Value::String("hello".to_string()))
    );
    let promoted = fx.store.properties("block:x").unwrap();
    assert_eq!(
        stamp_origin(&promoted, PROVENANCE_PROPERTY),
        "user",
        "confirmer is the latest writer"
    );
    assert_eq!(
        stamp_origin(&promoted, PROPOSED_BY_PROPERTY),
        "agent",
        "proposer provenance survives promotion"
    );

    let record = proposal_record(&fx.store, &proposal_id);
    assert_eq!(record.status, ProposalStatus::Accepted);
    assert!(record.resolved_by.is_some(), "resolver stamp recorded");
}

#[tokio::test]
async fn reject_retracts_without_executing() {
    let fx = fixture(AGENT_PROPOSE_POLICY);
    dispatch(&fx.engine, "create", create_params(), agent())
        .await
        .unwrap();
    let proposal_id = sole_proposal_id(&fx.store);

    let mut reject_params: StorageEntity = StorageEntity::new();
    reject_params.insert("id".into(), Value::String(proposal_id.clone()));
    let response = dispatch(
        &fx.engine,
        REJECT_PROPOSAL_OP,
        reject_params,
        OpOrigin::User,
    )
    .await
    .expect("reject succeeds");

    assert_eq!(response_field(&response, "status"), "rejected");
    assert!(
        !fx.store.ids().contains(&"block:x".to_string()),
        "rejected op never executes"
    );
    let record = proposal_record(&fx.store, &proposal_id);
    assert_eq!(record.status, ProposalStatus::Rejected);
}

#[tokio::test]
async fn resolving_a_non_pending_proposal_fails_loud() {
    let fx = fixture(AGENT_PROPOSE_POLICY);
    dispatch(&fx.engine, "create", create_params(), agent())
        .await
        .unwrap();
    let proposal_id = sole_proposal_id(&fx.store);

    let mut params: StorageEntity = StorageEntity::new();
    params.insert("id".into(), Value::String(proposal_id.clone()));
    dispatch(
        &fx.engine,
        REJECT_PROPOSAL_OP,
        params.clone(),
        OpOrigin::User,
    )
    .await
    .unwrap();

    let err = dispatch(&fx.engine, ACCEPT_PROPOSAL_OP, params, OpOrigin::User)
        .await
        .expect_err("terminal proposal must not promote");
    assert!(err.to_string().contains("already rejected"), "got: {err:#}");
    assert!(!fx.store.ids().contains(&"block:x".to_string()));
}

#[tokio::test]
async fn trusted_origin_passes_through_untouched() {
    let fx = fixture(AGENT_PROPOSE_POLICY);

    dispatch(&fx.engine, "create", create_params(), OpOrigin::User)
        .await
        .expect("trusted create executes");

    assert_eq!(
        fx.store.ids(),
        vec!["block:x".to_string()],
        "no proposal artifacts for a trusted origin"
    );
    assert_eq!(
        fx.store.column("block:x", "content"),
        Some(Value::String("hello".to_string()))
    );
}

#[tokio::test]
async fn untrusted_set_field_coerces_and_promotes_on_accept() {
    let fx = fixture(AGENT_PROPOSE_POLICY);
    // Target block exists (created by a trusted origin).
    dispatch(&fx.engine, "create", create_params(), OpOrigin::User)
        .await
        .unwrap();

    let mut set_params: StorageEntity = StorageEntity::new();
    set_params.insert("id".into(), Value::String("block:x".to_string()));
    set_params.insert("field".into(), Value::String("content".to_string()));
    set_params.insert("value".into(), Value::String("edited".to_string()));
    dispatch(&fx.engine, "set_field", set_params, agent())
        .await
        .expect("coercion succeeds");

    assert_eq!(
        fx.store.column("block:x", "content"),
        Some(Value::String("hello".to_string())),
        "target untouched while pending"
    );

    let proposal_id = fx
        .store
        .ids()
        .into_iter()
        .find(|id| {
            fx.store
                .properties(id)
                .is_some_and(|p| p.contains_key(PROPOSAL_PROPERTY))
        })
        .expect("proposal exists");
    let mut accept_params: StorageEntity = StorageEntity::new();
    accept_params.insert("id".into(), Value::String(proposal_id));
    dispatch(
        &fx.engine,
        ACCEPT_PROPOSAL_OP,
        accept_params,
        OpOrigin::User,
    )
    .await
    .expect("accept succeeds");

    assert_eq!(
        fx.store.column("block:x", "content"),
        Some(Value::String("edited".to_string())),
        "accepted set_field applies"
    );
}

#[tokio::test]
async fn untrusted_accept_is_itself_coerced() {
    let fx = fixture(AGENT_PROPOSE_POLICY);
    dispatch(&fx.engine, "create", create_params(), agent())
        .await
        .unwrap();
    let proposal_id = sole_proposal_id(&fx.store);

    let mut accept_params: StorageEntity = StorageEntity::new();
    accept_params.insert("id".into(), Value::String(proposal_id.clone()));
    let response = dispatch(&fx.engine, ACCEPT_PROPOSAL_OP, accept_params, agent())
        .await
        .expect("coerced, not executed");

    assert_eq!(
        response_field(&response, "status"),
        "proposed",
        "an untrusted origin cannot self-accept — the accept becomes a proposal"
    );
    assert!(
        !fx.store.ids().contains(&"block:x".to_string()),
        "wrapped op still not executed"
    );
    assert_eq!(
        proposal_record(&fx.store, &proposal_id).status,
        ProposalStatus::Pending
    );
}
