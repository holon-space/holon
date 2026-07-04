//! Regression (BugFunnel dogfood #4): undo/redo of a `create` must recreate the
//! SAME block id, not re-mint a fresh uuid.
//!
//! The engine builds the stored forward (redo) op from the ORIGINAL params. For
//! an interactive create the caller omits `id` and the provider mints one, so a
//! naive redo re-ran `create` with no id → a NEW uuid, dangling every ref/link
//! that targeted the original. The engine now grafts the minted id
//! (authoritative on the create's `delete{id}` inverse) onto the redo op.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon::api::OperationDispatcher;
use holon::api::operation_engine::DispatchingOperationEngine;
use holon::api::operation_engine::OperationEngine;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::OperationDescriptor;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result as DatasourceResult;
use holon_core::UndoStateReader;
use holon_core::UndoStore;
use holon_core::storage::types::StorageEntity;
use holon_core::traits::FieldDelta;

/// Dispatched-op log: (op name, id param), shared with the test for assertions.
type OpLog = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A `block` provider that mints a fresh, DISTINCT id on every id-less `create`
/// (so a re-mint is observable). Records the `id` param of each dispatched
/// `create`/`delete` so the test can assert redo reused the original id.
struct MintingProvider {
    next: AtomicUsize,
    log: OpLog,
}

impl MintingProvider {
    fn new() -> (Self, OpLog) {
        let log = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                next: AtomicUsize::new(1),
                log: log.clone(),
            },
            log,
        )
    }
}

#[async_trait]
impl OperationProvider for MintingProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        ["create", "delete"]
            .into_iter()
            .map(|name| OperationDescriptor {
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
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            })
            .collect()
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        let id_param = params
            .get("id")
            .and_then(|v| v.as_string())
            .map(String::from);
        self.log
            .lock()
            .unwrap()
            .push((op_name.to_string(), id_param.clone()));

        match op_name {
            "create" => {
                // Mint a fresh, distinct id whenever the caller omits one.
                let id = id_param.unwrap_or_else(|| {
                    let n = self.next.fetch_add(1, Ordering::SeqCst);
                    format!("block:minted-{n}")
                });
                let mut inv = HashMap::new();
                inv.insert("id".to_string(), Value::String(id.clone()));
                let inverse = holon_api::Operation::new("block", "delete", "delete", inv);
                Ok(OperationResult::new(
                    vec![FieldDelta::new(
                        id.clone(),
                        "id",
                        Value::Null,
                        Value::String(id.clone()),
                    )],
                    inverse,
                ))
            }
            // `delete` restores the pre-create (absent) state; its own inverse
            // re-creates the same id so redo can round-trip.
            "delete" => Ok(OperationResult::irreversible(Vec::new())),
            other => Err(format!("MintingProvider: unknown op {other}").into()),
        }
    }
}

/// A reader that reports the block row as present iff a `create` with that id
/// has run more recently than a `delete` — enough to keep the precondition
/// happy across undo/redo in this test (the create fingerprints the `id`
/// column).
struct PresenceReader {
    present: Mutex<HashMap<String, bool>>,
}

impl PresenceReader {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            present: Mutex::new(HashMap::new()),
        })
    }

    fn set_present(&self, id: &str, present: bool) {
        self.present.lock().unwrap().insert(id.to_string(), present);
    }
}

#[async_trait]
impl UndoStateReader for PresenceReader {
    async fn field_value(&self, entity_id: &str, field: &str) -> anyhow::Result<Option<Value>> {
        // The create's fingerprint is on the `id` column: present → Some(id),
        // absent → None.
        if field == "id"
            && *self
                .present
                .lock()
                .unwrap()
                .get(entity_id)
                .unwrap_or(&false)
        {
            Ok(Some(Value::String(entity_id.to_string())))
        } else {
            Ok(None)
        }
    }
}

struct NoopStore;

#[async_trait]
impl UndoStore for NoopStore {
    async fn load(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    async fn save(&self, _: &str, _: i64) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn redo_of_create_recreates_the_same_id() {
    let (provider, log) = MintingProvider::new();
    let dispatcher = Arc::new(OperationDispatcher::new(vec![
        Arc::new(provider) as Arc<dyn OperationProvider>
    ]));
    let reader = PresenceReader::new();
    let engine = DispatchingOperationEngine::new_persistent(
        dispatcher.clone(),
        reader.clone(),
        Arc::new(NoopStore),
    )
    .await
    .expect("engine");

    // Create with NO id (interactive create). Provider mints one.
    let mut params: StorageEntity = HashMap::new();
    params.insert("parent_id".into(), Value::String("block:root".into()));
    params.insert("content".into(), Value::String("hello".into()));
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::User)
        .await
        .expect("create");

    // Recover the minted id from the first create dispatch's inverse target: it
    // is the id the provider logged on the (id-less) create, i.e. the mint.
    let minted = {
        let l = log.lock().unwrap();
        // create was dispatched with id=None; the mint is whatever the delete
        // inverse now carries — reconstruct it from the next-counter: first mint
        // is `block:minted-1`.
        assert_eq!(l[0], ("create".to_string(), None));
        "block:minted-1".to_string()
    };
    reader.set_present(&minted, true);

    // Undo → delete{minted}.
    engine.undo().await.expect("undo");
    reader.set_present(&minted, false);
    {
        let l = log.lock().unwrap();
        assert_eq!(
            l.last(),
            Some(&("delete".to_string(), Some(minted.clone()))),
            "undo must delete the minted id"
        );
    }

    // Redo → create MUST reuse the minted id (not re-mint a fresh one).
    engine.redo().await.expect("redo");
    let creates: Vec<Option<String>> = log
        .lock()
        .unwrap()
        .iter()
        .filter(|(op, _)| op == "create")
        .map(|(_, id)| id.clone())
        .collect();
    assert_eq!(
        creates,
        vec![None, Some(minted.clone())],
        "redo-of-create must carry the originally-minted id, not re-mint a new uuid"
    );
}
