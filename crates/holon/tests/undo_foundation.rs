//! Engine-level tests for the undo/redo foundation
//! (`DispatchingOperationEngine`): provenance gating, loud classification,
//! staleness policy, and persistence — over an in-memory stub provider,
//! fake state reader, and fake store.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon::api::OperationDispatcher;
use holon::api::operation_engine::DispatchingOperationEngine;
use holon::api::operation_engine::OperationEngine;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::OperationDescriptor;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result as DatasourceResult;
use holon_core::UndoStateReader;
use holon_core::UndoStore;
use holon_core::storage::types::StorageEntity;
use holon_core::traits::FieldDelta;
use holon_core::traits::UndoAction;

const BLOCK_ID: &str = "block:b1";
const FIELD: &str = "content";
const OLD: &str = "old";
const NEW: &str = "new";

/// Stub provider on entity "block":
/// - `edit`: reversible OLD→NEW content edit (inverse = `set_field` back to
///   OLD)
/// - `set_field`: replay target, irreversible (replays never re-push)
/// - `noundo`: returns an `Undeclared` classification (must be a loud error)
struct StubProvider {
    /// (op_name, value-param) log of every dispatched execution.
    log: Arc<Mutex<Vec<(String, Option<String>)>>>,
}

impl StubProvider {
    fn new() -> (Self, Arc<Mutex<Vec<(String, Option<String>)>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        (Self { log: log.clone() }, log)
    }

    fn descriptor(&self, name: &str) -> OperationDescriptor {
        OperationDescriptor {
            entity_name: EntityName::new("block"),
            name: name.to_string(),
            ..Default::default()
        }
    }
}

#[async_trait]
impl OperationProvider for StubProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![
            self.descriptor("edit"),
            self.descriptor("set_field"),
            self.descriptor("noundo"),
            self.descriptor("noop_forward"),
            self.descriptor("edit_vacuous_inverse"),
            self.descriptor("vacuous_replay"),
        ]
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        let value = params
            .get("value")
            .and_then(|v| v.as_string())
            .map(String::from);
        self.log.lock().unwrap().push((op_name.to_string(), value));

        match op_name {
            "edit" => {
                let changes = vec![FieldDelta::new(
                    BLOCK_ID,
                    FIELD,
                    Value::String(OLD.to_string()),
                    Value::String(NEW.to_string()),
                )];
                let mut inv_params = HashMap::new();
                inv_params.insert("id".to_string(), Value::String(BLOCK_ID.to_string()));
                inv_params.insert("field".to_string(), Value::String(FIELD.to_string()));
                inv_params.insert("value".to_string(), Value::String(OLD.to_string()));
                let inverse =
                    holon_api::Operation::new("block", "set_field", "Restore content", inv_params);
                Ok(OperationResult::new(changes, inverse))
            }
            "set_field" => Ok(OperationResult::irreversible(Vec::new())),
            "noundo" => Ok(OperationResult::from_undo(UndoAction::Undeclared)),
            // A reversible-but-INERT write: reports a delta whose old==new (an
            // identical-content set_field). Classified `Undo`, so a naive push
            // would journal it — the engine must skip it as provably vacuous.
            "noop_forward" => {
                let changes = vec![FieldDelta::new(
                    BLOCK_ID,
                    FIELD,
                    Value::String(OLD.to_string()),
                    Value::String(OLD.to_string()),
                )];
                let mut inv_params = HashMap::new();
                inv_params.insert("id".to_string(), Value::String(BLOCK_ID.to_string()));
                inv_params.insert("field".to_string(), Value::String(FIELD.to_string()));
                inv_params.insert("value".to_string(), Value::String(OLD.to_string()));
                let inverse =
                    holon_api::Operation::new("block", "set_field", "Restore content", inv_params);
                Ok(OperationResult::new(changes, inverse))
            }
            // A genuine forward edit (OLD→NEW, so it IS journaled) whose inverse
            // replays into `vacuous_replay` — modelling an entry whose undo turns
            // out to change nothing (the state already matched the inverse's
            // target by undo time).
            "edit_vacuous_inverse" => {
                let changes = vec![FieldDelta::new(
                    BLOCK_ID,
                    FIELD,
                    Value::String(OLD.to_string()),
                    Value::String(NEW.to_string()),
                )];
                let mut inv_params = HashMap::new();
                inv_params.insert("id".to_string(), Value::String(BLOCK_ID.to_string()));
                inv_params.insert("field".to_string(), Value::String(FIELD.to_string()));
                inv_params.insert("value".to_string(), Value::String(OLD.to_string()));
                let inverse = holon_api::Operation::new(
                    "block",
                    "vacuous_replay",
                    "No-op restore",
                    inv_params,
                );
                Ok(OperationResult::new(changes, inverse))
            }
            // Replay target that reports a vacuous delta (old==new) — the inverse
            // whose replay proves the undo changed nothing.
            "vacuous_replay" => {
                let changes = vec![FieldDelta::new(
                    BLOCK_ID,
                    FIELD,
                    Value::String(OLD.to_string()),
                    Value::String(OLD.to_string()),
                )];
                Ok(OperationResult::declared_irreversible(
                    changes,
                    "replay target",
                ))
            }
            other => Err(format!("StubProvider: unknown op {other}").into()),
        }
    }
}

/// In-memory (entity, field) → value map standing in for the projection table.
struct FakeReader {
    values: Mutex<HashMap<(String, String), Value>>,
}

impl FakeReader {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            values: Mutex::new(HashMap::new()),
        })
    }

    fn set(&self, entity_id: &str, field: &str, value: &str) {
        self.values.lock().unwrap().insert(
            (entity_id.to_string(), field.to_string()),
            Value::String(value.to_string()),
        );
    }
}

#[async_trait]
impl UndoStateReader for FakeReader {
    async fn field_value(&self, entity_id: &str, field: &str) -> anyhow::Result<Option<Value>> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(&(entity_id.to_string(), field.to_string()))
            .cloned())
    }
}

/// In-memory single-slot snapshot store (the fake `undo_log`).
struct FakeStore {
    snapshot: Mutex<Option<(String, i64)>>,
}

impl FakeStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            snapshot: Mutex::new(None),
        })
    }

    fn has_snapshot(&self) -> bool {
        self.snapshot.lock().unwrap().is_some()
    }
}

#[async_trait]
impl UndoStore for FakeStore {
    async fn load(&self) -> anyhow::Result<Option<String>> {
        Ok(self
            .snapshot
            .lock()
            .unwrap()
            .as_ref()
            .map(|(json, _)| json.clone()))
    }

    async fn save(&self, state_json: &str, seq: i64) -> anyhow::Result<()> {
        *self.snapshot.lock().unwrap() = Some((state_json.to_string(), seq));
        Ok(())
    }
}

struct Fixture {
    engine: DispatchingOperationEngine,
    dispatcher: Arc<OperationDispatcher>,
    reader: Arc<FakeReader>,
    store: Arc<FakeStore>,
    log: Arc<Mutex<Vec<(String, Option<String>)>>>,
}

async fn fixture() -> Fixture {
    let (provider, log) = StubProvider::new();
    let dispatcher = Arc::new(OperationDispatcher::new(vec![
        Arc::new(provider) as Arc<dyn OperationProvider>
    ]));
    let reader = FakeReader::new();
    let store = FakeStore::new();
    let engine = DispatchingOperationEngine::new_persistent(
        dispatcher.clone(),
        reader.clone(),
        store.clone(),
    )
    .await
    .expect("fresh persistent engine");
    Fixture {
        engine,
        dispatcher,
        reader,
        store,
        log,
    }
}

fn edit_params() -> StorageEntity {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(BLOCK_ID.to_string()));
    params
}

async fn execute_edit(engine: &DispatchingOperationEngine, origin: OpOrigin) {
    engine
        .execute_operation(&EntityName::new("block"), "edit", edit_params(), origin)
        .await
        .expect("edit dispatch");
}

/// Count how many times the replay target (`set_field`) was dispatched.
fn replay_count(log: &Mutex<Vec<(String, Option<String>)>>) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(op, _)| op == "set_field")
        .count()
}

#[tokio::test]
async fn rule_origin_never_enters_undo_stack() {
    let fx = fixture().await;

    execute_edit(
        &fx.engine,
        OpOrigin::Rule {
            transition_id: "rule:daily-journal".to_string(),
        },
    )
    .await;
    assert!(
        !fx.engine.can_undo().await,
        "rule-fired op must not enter the user undo stack"
    );

    execute_edit(&fx.engine, OpOrigin::Sync).await;
    execute_edit(&fx.engine, OpOrigin::Ingest).await;
    assert!(
        !fx.engine.can_undo().await,
        "sync/ingest ops must not enter the user undo stack"
    );

    execute_edit(&fx.engine, OpOrigin::User).await;
    assert!(
        fx.engine.can_undo().await,
        "user op with an inverse must push an undo entry"
    );
}

#[tokio::test]
async fn undeclared_classification_is_loud_error() {
    let fx = fixture().await;

    let result = fx
        .engine
        .execute_operation(
            &EntityName::new("block"),
            "noundo",
            edit_params(),
            OpOrigin::User,
        )
        .await;
    let err = result.expect_err("an Undeclared undo classification must be a loud error");
    assert!(
        err.to_string().contains("Undeclared"),
        "error must name the missing classification, got: {err}"
    );
    assert!(!fx.engine.can_undo().await);
}

#[tokio::test]
async fn stale_undo_dropped_loudly_without_replaying() {
    let fx = fixture().await;
    fx.reader.set(BLOCK_ID, FIELD, NEW);
    execute_edit(&fx.engine, OpOrigin::User).await;

    // Someone mutates the block underneath the undo entry.
    fx.reader.set(BLOCK_ID, FIELD, "tampered-by-someone-else");

    let outcome = fx.engine.undo().await.expect("undo call");
    match outcome {
        UndoOutcome::StaleDropped { reason } => {
            assert!(
                reason.contains(BLOCK_ID) && reason.contains(FIELD),
                "reason must name the divergent field, got: {reason}"
            );
        }
        other => panic!("expected StaleDropped, got {other:?}"),
    }
    assert_eq!(
        replay_count(&fx.log),
        0,
        "a stale entry's inverse must NOT be dispatched"
    );
    assert!(
        !fx.engine.can_undo().await,
        "the stale entry is dropped, not skipped-and-kept"
    );
    assert!(
        !fx.engine.can_redo().await,
        "a dropped entry must not become redoable"
    );
}

#[tokio::test]
async fn undo_applies_and_redo_staleness_is_symmetric() {
    let fx = fixture().await;
    fx.reader.set(BLOCK_ID, FIELD, NEW);
    execute_edit(&fx.engine, OpOrigin::User).await;

    let outcome = fx.engine.undo().await.expect("undo call");
    assert_eq!(outcome, UndoOutcome::Applied);
    assert_eq!(replay_count(&fx.log), 1, "inverse dispatched exactly once");
    let last = fx.log.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        last,
        ("set_field".to_string(), Some(OLD.to_string())),
        "inverse must restore the OLD value"
    );
    assert!(fx.engine.can_redo().await);

    // Redo precondition expects the post-inverse (OLD) state; tamper it.
    fx.reader.set(BLOCK_ID, FIELD, "tampered");
    let outcome = fx.engine.redo().await.expect("redo call");
    assert!(
        matches!(outcome, UndoOutcome::StaleDropped { .. }),
        "redo over tampered state must drop loudly, got {outcome:?}"
    );
    assert!(!fx.engine.can_redo().await);
}

#[tokio::test]
async fn redo_reapplies_forward_ops() {
    let fx = fixture().await;
    fx.reader.set(BLOCK_ID, FIELD, NEW);
    execute_edit(&fx.engine, OpOrigin::User).await;
    assert_eq!(fx.engine.undo().await.unwrap(), UndoOutcome::Applied);

    // Simulate the projection having applied the inverse.
    fx.reader.set(BLOCK_ID, FIELD, OLD);
    assert_eq!(fx.engine.redo().await.unwrap(), UndoOutcome::Applied);
    let last = fx.log.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.0, "edit", "redo must replay the stored forward op");
    assert!(fx.engine.can_undo().await, "redone entry is undoable again");
}

#[tokio::test]
async fn empty_stack_reports_empty() {
    let fx = fixture().await;
    assert_eq!(fx.engine.undo().await.unwrap(), UndoOutcome::Empty);
    assert_eq!(fx.engine.redo().await.unwrap(), UndoOutcome::Empty);
}

#[tokio::test]
async fn persistence_survives_reload() {
    let fx = fixture().await;
    fx.reader.set(BLOCK_ID, FIELD, NEW);
    execute_edit(&fx.engine, OpOrigin::User).await;
    assert!(fx.store.has_snapshot(), "push must persist a snapshot");

    // "Restart": a fresh engine over the same dispatcher/reader/store.
    let engine2 = DispatchingOperationEngine::new_persistent(
        fx.dispatcher.clone(),
        fx.reader.clone(),
        fx.store.clone(),
    )
    .await
    .expect("reloaded engine");
    assert!(
        engine2.can_undo().await,
        "undo history must survive a restart"
    );

    let outcome = engine2.undo().await.expect("undo after reload");
    assert_eq!(outcome, UndoOutcome::Applied);
    let last = fx.log.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        last,
        ("set_field".to_string(), Some(OLD.to_string())),
        "reloaded entry must replay the stored inverse"
    );
}

#[tokio::test]
async fn stale_after_reload_drops_loudly() {
    let fx = fixture().await;
    fx.reader.set(BLOCK_ID, FIELD, NEW);
    execute_edit(&fx.engine, OpOrigin::User).await;

    // Mutate underneath between "restart"s.
    fx.reader.set(BLOCK_ID, FIELD, "changed-while-down");

    let engine2 = DispatchingOperationEngine::new_persistent(
        fx.dispatcher.clone(),
        fx.reader.clone(),
        fx.store.clone(),
    )
    .await
    .expect("reloaded engine");
    let outcome = engine2.undo().await.expect("undo after reload");
    assert!(
        matches!(outcome, UndoOutcome::StaleDropped { .. }),
        "stale persisted entry must drop loudly, got {outcome:?}"
    );
    assert_eq!(replay_count(&fx.log), 0);
    assert!(!engine2.can_undo().await);
}

/// BugFunnel 2026-07-13 undo row, part 2 — a provably-vacuous forward write (a
/// reported delta whose `old_value == new_value`, e.g. an identical-content
/// `set_field`) must NOT enter the undo log. Journaling it poisons the stack
/// with an entry whose undo is itself a no-op, so consecutive undo presses get
/// eaten while a real target underneath stays unreachable.
#[tokio::test]
async fn vacuous_forward_write_is_not_journaled() {
    let fx = fixture().await;
    fx.engine
        .execute_operation(
            &EntityName::new("block"),
            "noop_forward",
            edit_params(),
            OpOrigin::User,
        )
        .await
        .expect("noop_forward dispatch");
    assert!(
        !fx.engine.can_undo().await,
        "an identical-content (vacuous) write must not push an undo entry"
    );
}

/// BugFunnel 2026-07-13 undo row, part 1 — fail-loud contract. An undo whose
/// inverse replay produces NO observable change (every delta vacuous) must
/// report `NoChange`, never `Applied`: the caller (MCP result / UI toast) must
/// not claim success for a press that changed nothing.
#[tokio::test]
async fn undo_with_vacuous_inverse_reports_no_change() {
    let fx = fixture().await;
    // Forward is a genuine OLD→NEW edit, so it IS journaled; its stored inverse
    // replays into `vacuous_replay`. Satisfy the forward precondition so the
    // entry is not dropped as stale before we reach the replay.
    fx.reader.set(BLOCK_ID, FIELD, NEW);
    fx.engine
        .execute_operation(
            &EntityName::new("block"),
            "edit_vacuous_inverse",
            edit_params(),
            OpOrigin::User,
        )
        .await
        .expect("edit_vacuous_inverse dispatch");
    assert!(
        fx.engine.can_undo().await,
        "genuine forward edit is journaled"
    );

    let outcome = fx.engine.undo().await.expect("undo call");
    assert_eq!(
        outcome,
        UndoOutcome::NoChange,
        "an undo whose inverse changed nothing must report NoChange, not Applied"
    );
    // The poison entry is consumed, not left to eat the next press.
    assert!(!fx.engine.can_undo().await);
}
