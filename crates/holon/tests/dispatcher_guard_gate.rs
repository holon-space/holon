//! ADR 0031 Increment 3c — the dispatcher's declared-guard gate.
//!
//! A declared `#[require]` guard that nothing enforces is a lie. These tests
//! prove the gate refuses BEFORE the provider runs (guard-then-fire), that the
//! refusal quotes the developer's own literal, and — the trap — that it binds
//! the SUBJECT and not merely "somebody satisfies this".
//!
//! The world here is IN MEMORY, so the same tests also prove the `GuardWorld`
//! seam really hides the substrate from the dispatcher.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon::api::OperationDispatcher;
use holon::api::guard_world::GuardQuery;
use holon::api::guard_world::GuardSubject;
use holon::api::guard_world::GuardWorld;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::Value;
use holon_api::pattern::Binding;
use holon_api::pattern::InMemoryWorld;
use holon_api::pattern::OpGuard;
use holon_api::pattern::WorldBlock;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;

const ENTITY: &str = "block";
const GUARDED: &str = "set_priority";
const UNGUARDED: &str = "set_flag";
const GUARD_SRC: &str = "has_tag(\"Page\")";

// ─── Fixtures ─────────────────────────────────────────────────────────────

/// Counts every provider call, so a refusal that still fired the op is
/// distinguishable from a refusal that did not.
struct CountingProvider {
    ops: Vec<OperationDescriptor>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl OperationProvider for CountingProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        self.ops.clone()
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        _: &str,
        _: StorageEntity,
    ) -> Result<OperationResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(OperationResult::irreversible(Vec::new()))
    }
}

/// The reference evaluator behind the seam.
struct MemoryGuardWorld {
    world: InMemoryWorld,
}

#[async_trait]
impl GuardWorld for MemoryGuardWorld {
    async fn guard_holds(&self, query: &GuardQuery<'_>) -> Result<bool> {
        let result = query.guard().evaluate(&self.world);
        Ok(match query.subject() {
            GuardSubject::Block(id) => result.bindings.contains(&Binding::Block(id.clone())),
            GuardSubject::Clock => !result.bindings.is_empty(),
        })
    }
}

fn descriptor(op: &str, guard: OpGuard) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: ENTITY.into(),
        entity_short_name: ENTITY.to_string(),
        id_column: "id".to_string(),
        name: op.to_string(),
        display_name: op.to_string(),
        description: op.to_string(),
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
        guard,
    }
}

fn page(id: &str, tags: &[&str]) -> WorldBlock {
    WorldBlock {
        id: id.to_string(),
        name: id.to_string(),
        parent_id: None,
        properties: Default::default(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
    }
}

/// A dispatcher over one guarded and one unguarded op, evaluating against
/// `blocks`. Returns it alongside the provider's call counter.
fn harness(blocks: Vec<WorldBlock>) -> (OperationDispatcher, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(CountingProvider {
        ops: vec![
            descriptor(GUARDED, OpGuard::parse(GUARD_SRC).expect("guard parses")),
            descriptor(UNGUARDED, OpGuard::None),
        ],
        calls: calls.clone(),
    });
    let mut dispatcher = OperationDispatcher::new(vec![provider]);
    dispatcher.set_guard_world(Arc::new(MemoryGuardWorld {
        world: InMemoryWorld::new(blocks, "2026-08-10"),
    }));
    (dispatcher, calls)
}

async fn dispatch(
    dispatcher: &OperationDispatcher,
    op: &str,
    subject: &str,
) -> std::result::Result<OperationResult, String> {
    let mut params = StorageEntity::new();
    params.insert("id".into(), Value::String(subject.to_string()));
    dispatcher
        .execute_operation_with_input(
            &EntityName::new(ENTITY),
            op,
            params,
            holon::api::AuthoredInput::Verbatim,
        )
        .await
        .map_err(|e| e.to_string())
}

// ─── The three required assertions ────────────────────────────────────────

/// (1) A declared guard the world does not satisfy refuses, and the provider is
/// NEVER called — guard-then-fire, not fire-then-regret.
#[tokio::test]
async fn unsatisfied_guard_refuses_before_the_provider_runs() {
    let (dispatcher, calls) = harness(vec![page("b:plain", &[])]);
    let err = dispatch(&dispatcher, GUARDED, "b:plain")
        .await
        .expect_err("the guard does not hold, so the op must be refused");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the provider ran despite the refusal: {err}"
    );
}

/// (2) The refusal quotes the developer's own `#[require]` text.
#[tokio::test]
async fn refusal_names_the_guard_source_text() {
    let (dispatcher, _) = harness(vec![page("b:plain", &[])]);
    let err = dispatch(&dispatcher, GUARDED, "b:plain")
        .await
        .expect_err("refused");
    assert!(
        err.contains(GUARD_SRC),
        "refusal must quote the guard source {GUARD_SRC:?}, got: {err}"
    );
    assert!(
        err.contains(GUARDED),
        "refusal must name the operation, got: {err}"
    );
}

/// (3) A satisfied guard fires normally, and an `OpGuard::None` op is
/// unaffected by the gate.
#[tokio::test]
async fn satisfied_guard_fires_and_unguarded_ops_are_untouched() {
    let (dispatcher, calls) = harness(vec![page("b:page", &["Page"])]);
    dispatch(&dispatcher, GUARDED, "b:page")
        .await
        .expect("the guard holds, so the op must fire");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    dispatch(&dispatcher, UNGUARDED, "b:plain")
        .await
        .expect("an op declaring no guard is never gated");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

// ─── The subject-binding trap ─────────────────────────────────────────────

/// THE trap (ADR 0031 R8): `GuardResult::enabled()` means "SOMEBODY satisfies
/// this", not "THIS block satisfies this". Here a sibling is a Page and the
/// subject is not — a gate written on `enabled()` waves it through.
#[tokio::test]
async fn another_block_satisfying_the_guard_does_not_admit_the_subject() {
    let (dispatcher, calls) = harness(vec![page("b:page", &["Page"]), page("b:plain", &[])]);
    let err = dispatch(&dispatcher, GUARDED, "b:plain")
        .await
        .expect_err("the SUBJECT does not satisfy the guard; a sibling doing so is irrelevant");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the provider ran on a sibling's binding: {err}"
    );
}

/// A declared guard with no world installed must refuse, not pass. Silence
/// here would be fail-open at exactly the composition sites that forget to
/// wire the seam.
#[tokio::test]
async fn a_declared_guard_without_a_world_refuses_loudly() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(CountingProvider {
        ops: vec![descriptor(
            GUARDED,
            OpGuard::parse(GUARD_SRC).expect("guard parses"),
        )],
        calls: calls.clone(),
    });
    let dispatcher = OperationDispatcher::new(vec![provider]);
    let err = dispatch(&dispatcher, GUARDED, "b:plain")
        .await
        .expect_err("no GuardWorld installed, so a declared guard cannot be honoured");
    assert!(err.contains("GuardWorld"), "got: {err}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
