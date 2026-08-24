//! One undo gesture holds one lock (task #47).
//!
//! An external write must not land between the staleness check and the replay,
//! nor between two inverses of one composite entry. Each rung drives a REAL
//! competing write on a second task through the same engine, so the only thing
//! that can serialize it is the engine's own stripe lock.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

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
use tokio::sync::oneshot;

const FIELD: &str = "content";
const OLD: &str = "old";
const NEW: &str = "new";
const COMPETING: &str = "competing";

/// How long a rung waits for the competing write before giving up on it.
///
/// This is a DEADLOCK ESCAPE, not a race window, and both directions are
/// deterministic rather than timing-dependent:
///
/// - Before the fix nothing holds the target's stripe while the window is open,
///   so the competing write acquires immediately and signals; the wait ends on
///   the signal, never on the clock, however slow the machine is.
/// - After the fix the stripe is held by the very task that is waiting, so the
///   competing write CANNOT complete until that task releases — no scheduling
///   order makes the signal arrive, and the wait always ends on the clock.
///
/// Each rung asserts which of the two happened (`completed_in_window`), so a
/// green run proves the lock did the work instead of merely agreeing with the
/// final value.
const RACE_ESCAPE: Duration = Duration::from_secs(5);

/// Upper bound on a whole gesture. A stripe re-locked by the task that already
/// holds it would hang forever; this turns that into a named failure.
const GESTURE_ESCAPE: Duration = Duration::from_secs(30);

/// The stripe an id hashes to, mirroring `EntityWriteLocks::stripe_of`
/// (private to the engine). Only used to assert that the ids the self-deadlock
/// rung picks really do collide — if the engine's hashing changes, that rung
/// tells us rather than silently testing nothing.
fn stripe_of(entity_name: &str, id: &str) -> usize {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;

    let mut hasher = DefaultHasher::new();
    entity_name.hash(&mut hasher);
    id.hash(&mut hasher);
    (hasher.finish() % 64) as usize
}

/// Block id → current content. Shared by the provider (which writes it) and the
/// reader (which verifies preconditions against it).
type Store = Arc<Mutex<HashMap<String, String>>>;

fn content_of(store: &Store, id: &str) -> String {
    store
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .unwrap_or_else(|| OLD.to_string())
}

/// Fires ONE competing write, on a real second task, the first time the engine
/// reaches the chosen point of the gesture.
struct Race {
    engine: OnceLock<Weak<DispatchingOperationEngine>>,
    /// The test arms this immediately before `undo()`, so incidental reads
    /// during the forward edits can never consume the one-shot.
    armed: AtomicBool,
    fired: AtomicBool,
    /// Whether the competing write finished inside `RACE_ESCAPE`.
    completed_in_window: AtomicBool,
    /// The id whose observation opens the window.
    trigger_id: String,
    /// The id the competing write targets.
    target_id: String,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Race {
    fn new(trigger_id: &str, target_id: &str) -> Arc<Self> {
        Arc::new(Self {
            engine: OnceLock::new(),
            armed: AtomicBool::new(false),
            fired: AtomicBool::new(false),
            completed_in_window: AtomicBool::new(false),
            trigger_id: trigger_id.to_string(),
            target_id: target_id.to_string(),
            handle: Mutex::new(None),
        })
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    async fn maybe_fire(&self, seen_id: &str) {
        if !self.armed.load(Ordering::SeqCst) || seen_id != self.trigger_id {
            return;
        }
        if self.fired.swap(true, Ordering::SeqCst) {
            return;
        }

        let engine = self
            .engine
            .get()
            .expect("race engine wired")
            .upgrade()
            .expect("engine alive");
        let target = self.target_id.clone();
        let (tx, rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            engine
                .execute_operation(
                    &EntityName::new("block"),
                    "set_field",
                    set_field_params(&target, COMPETING),
                    OpOrigin::User,
                )
                .await
                .expect("competing write dispatch");
            let _ = tx.send(());
        });
        *self.handle.lock().unwrap() = Some(handle);

        if tokio::time::timeout(RACE_ESCAPE, rx).await.is_ok() {
            self.completed_in_window.store(true, Ordering::SeqCst);
        }
    }

    /// Wait for the competing write to land. After the fix it is still queued
    /// on the stripe when `undo()` returns, so every assertion about final
    /// content must come after this.
    async fn settle(&self) {
        let handle = self.handle.lock().unwrap().take().expect("race fired");
        handle.await.expect("competing write task");
    }
}

fn set_field_params(id: &str, value: &str) -> StorageEntity {
    let mut p = StorageEntity::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("field".into(), Value::String(FIELD.to_string()));
    p.insert("value".into(), Value::String(value.to_string()));
    p
}

/// Provider on entity "block":
/// - `edit`: reversible content write (inverse = `set_field` back to the value
///   it found), the op the rungs journal.
/// - `set_field`: the replay target AND the competing write's op.
struct StubProvider {
    store: Store,
    /// Fires between two inverses of a composite: the window a per-op lock
    /// leaves open but a per-gesture hold closes. `None` for the rungs whose
    /// window is the staleness check instead.
    race: Option<Arc<Race>>,
}

impl StubProvider {
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
}

#[async_trait]
impl OperationProvider for StubProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![self.descriptor("edit"), self.descriptor("set_field")]
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
            .expect("stub op carries an id")
            .to_string();
        let value = params
            .get("value")
            .and_then(|v| v.as_string())
            .expect("stub op carries a value")
            .to_string();
        let old = content_of(&self.store, &id);
        self.store.lock().unwrap().insert(id.clone(), value.clone());

        let changes = vec![FieldDelta::new(
            &id,
            FIELD,
            Value::String(old.clone()),
            Value::String(value),
        )];

        match op_name {
            "edit" => {
                let mut inv = HashMap::new();
                inv.insert("id".to_string(), Value::String(id));
                inv.insert("field".to_string(), Value::String(FIELD.to_string()));
                inv.insert("value".to_string(), Value::String(old));
                Ok(OperationResult::new(
                    changes,
                    holon_api::Operation::new("block", "set_field", "Restore content", inv),
                ))
            }
            "set_field" => {
                if let Some(race) = &self.race {
                    race.maybe_fire(&id).await;
                }
                Ok(OperationResult::declared_irreversible(
                    changes,
                    "replay target",
                ))
            }
            other => Err(format!("StubProvider: unknown op {other}").into()),
        }
    }
}

/// Reads live content out of the same store the provider writes.
struct RacingReader {
    store: Store,
    /// Fires during the staleness check: the window between the check and the
    /// replay. `None` for the rungs whose window is between two inverses.
    race: Option<Arc<Race>>,
}

#[async_trait]
impl UndoStateReader for RacingReader {
    async fn field_value(&self, entity_id: &str, field: &str) -> anyhow::Result<Option<Value>> {
        if field != FIELD {
            return Ok(None);
        }
        // Snapshot BEFORE opening the window. The engine must decide staleness
        // against the state it saw when the window opened; returning the
        // post-race value would let the check catch the competing write by
        // accident and hide the very gap these rungs exist to prove.
        let seen = content_of(&self.store, entity_id);
        if let Some(race) = &self.race {
            race.maybe_fire(entity_id).await;
        }
        Ok(Some(Value::String(seen)))
    }
}

struct NoStore;

#[async_trait]
impl UndoStore for NoStore {
    async fn load(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    async fn save(&self, _: &str, _: i64) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Build an engine whose reader and provider share `store`, wiring `race` into
/// whichever of the two the rung wants as its trigger point.
async fn engine_with(
    store: Store,
    reader_race: Option<Arc<Race>>,
    provider_race: Option<Arc<Race>>,
) -> Arc<DispatchingOperationEngine> {
    let dispatcher = Arc::new(OperationDispatcher::new(vec![Arc::new(StubProvider {
        store: store.clone(),
        race: provider_race,
    })
        as Arc<dyn OperationProvider>]));
    let reader = Arc::new(RacingReader {
        store,
        race: reader_race,
    });
    Arc::new(
        DispatchingOperationEngine::new_persistent(dispatcher, reader, Arc::new(NoStore))
            .await
            .expect("persistent engine"),
    )
}

/// An engine wired exactly the way `register_loro_operation_engine` wires the
/// Loro-only production session (`sync::loro_block_query_source`): `new()` plus
/// a degraded history store, and NO live-state reader.
fn readerless_prod_engine(store: Store) -> Arc<DispatchingOperationEngine> {
    let dispatcher =
        Arc::new(OperationDispatcher::new(vec![
            Arc::new(StubProvider { store, race: None }) as Arc<dyn OperationProvider>,
        ]));
    Arc::new(
        DispatchingOperationEngine::new(dispatcher)
            .with_history_store(Arc::new(holon::api::DegradedHistoryStore::new())),
    )
}

async fn edit(engine: &DispatchingOperationEngine, id: &str, value: &str) {
    let mut p = StorageEntity::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("value".into(), Value::String(value.to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "edit", p, OpOrigin::User)
        .await
        .expect("edit dispatch");
}

/// The staleness-check window: a competing write dispatched while `check_stale`
/// reads must not be overwritten by the replay that follows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_external_write_during_the_staleness_check_cannot_race_the_replay() {
    let id = "block:b1";
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    let race = Race::new(id, id);
    let engine = engine_with(store.clone(), Some(race.clone()), None).await;
    race.engine
        .set(Arc::downgrade(&engine))
        .expect("engine wired once");

    edit(&engine, id, NEW).await;
    assert_eq!(content_of(&store, id), NEW, "forward edit landed");

    race.arm();
    let outcome = tokio::time::timeout(GESTURE_ESCAPE, engine.undo())
        .await
        .expect("undo must not hang")
        .expect("undo dispatch");
    race.settle().await;

    assert!(
        !race.completed_in_window.load(Ordering::SeqCst),
        "the competing write completed INSIDE the check-to-replay window — the \
         gesture did not hold {id}'s stripe across its staleness check \
         (outcome was {outcome:?})"
    );
    assert_eq!(
        content_of(&store, id),
        COMPETING,
        "the competing write was serialized behind the whole undo gesture, so it \
         must be the surviving value; finding the inverse's value ({OLD}) means \
         the replay overwrote a write it never saw"
    );
}

/// The between-inverses window: a composite entry replays N inverses, and a
/// competing write to a LATER inverse's block must not land between them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_composite_undo_holds_its_locks_across_every_inverse() {
    let first = "block:b1";
    let second = "block:b2";
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    // Inverses replay in reverse order, so `second` is inverted first; the
    // window opens there and the competing write targets `first`, whose inverse
    // has not run yet.
    let race = Race::new(second, first);
    let engine = engine_with(store.clone(), None, Some(race.clone())).await;
    race.engine
        .set(Arc::downgrade(&engine))
        .expect("engine wired once");

    engine.begin_undo_group().await;
    edit(&engine, first, NEW).await;
    edit(&engine, second, NEW).await;
    engine.end_undo_group().await.expect("close undo group");

    race.arm();
    let outcome = tokio::time::timeout(GESTURE_ESCAPE, engine.undo())
        .await
        .expect("undo must not hang")
        .expect("undo dispatch");
    race.settle().await;

    assert!(
        !race.completed_in_window.load(Ordering::SeqCst),
        "the competing write completed BETWEEN two inverses of one composite \
         entry — the gesture released {first}'s stripe while it was still \
         replaying (outcome was {outcome:?})"
    );
    assert_eq!(
        content_of(&store, first),
        COMPETING,
        "the competing write must survive the whole composite gesture; finding \
         the inverse's value ({OLD}) means inverse 2 overwrote it"
    );
}

/// Two blocks of one composite entry that hash to the SAME stripe must not
/// deadlock the gesture against itself. A `tokio` mutex is not reentrant, so a
/// multi-guard hold that orders and dedupes by `(entity_name, id)` rather than
/// by stripe index re-locks a stripe it already holds and hangs forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_composite_undo_over_two_stripe_colliding_blocks_completes() {
    let a = "block:b0";
    let b = "block:b89";
    assert_eq!(
        stripe_of("block", a),
        stripe_of("block", b),
        "this rung is only meaningful while {a} and {b} collide; pick a new \
         colliding pair for the engine's current hashing"
    );

    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    let engine = engine_with(store.clone(), None, None).await;

    engine.begin_undo_group().await;
    edit(&engine, a, NEW).await;
    edit(&engine, b, NEW).await;
    engine.end_undo_group().await.expect("close undo group");

    tokio::time::timeout(GESTURE_ESCAPE, engine.undo())
        .await
        .expect(
            "a composite undo over two blocks sharing one stripe hung — the \
                 gesture's guards are ordered/deduped by key instead of by \
                 stripe index",
        )
        .expect("undo dispatch");

    assert_eq!(content_of(&store, a), OLD, "{a} restored");
    assert_eq!(content_of(&store, b), OLD, "{b} restored");
}

/// The Loro-only production wiring has no live-state reader, yet every
/// reversible op journals a non-empty precondition. Undo must still work there:
/// staleness goes UNVERIFIED (announced at WARN) rather than taking undo away.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_readerless_production_engine_can_still_undo() {
    let id = "block:b1";
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    let engine = readerless_prod_engine(store.clone());

    edit(&engine, id, NEW).await;
    assert_eq!(content_of(&store, id), NEW, "forward edit landed");

    let outcome = engine.undo().await.expect(
        "a reader-less engine must still undo — refusing here removes \
                 undo from every Loro-only session, because each reversible op \
                 journals a precondition unconditionally",
    );

    assert!(
        matches!(outcome, holon_api::UndoOutcome::Applied),
        "expected the inverse to apply, got {outcome:?}"
    );
    assert_eq!(
        content_of(&store, id),
        OLD,
        "the inverse must have restored the prior content"
    );
}
