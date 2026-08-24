//! ADR 0028 C3 — the boundary/authz seam is LIVE in the engine's operation
//! dispatch.
//!
//! The sharing machinery (crossing log, policy set, `check_boundary`) landed as
//! a library nothing called: with a denying policy committed, an op that moves
//! a block out of a shared container still executed. These properties pin the
//! seam itself.
//!
//! **Non-vacuity.** Every case asserts a WITNESS: the enforcer's `check` was
//! actually invoked for the dispatched op. A registered-but-never-consulted
//! enforcer would satisfy every "allowed" case vacuously, so the witness — not
//! the allow/reject outcome — is what proves the check-path executes.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon::api::OperationDispatcher;
use holon_api::BoundaryBehavior;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::Value;
use holon_core::BoundaryEnforcer;
use holon_core::BoundaryRejection;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;
use holon_sharing::Capabilities;
use holon_sharing::CrossingLog;
use holon_sharing::Lease;
use holon_sharing::MapContainment;
use holon_sharing::Policy;
use holon_sharing::PolicyOverlayEnforcer;
use holon_sharing::PolicySet;
use holon_sharing::Principal;
use holon_sharing::SignedPolicy;
use holon_sharing::UnverifiedVerifier;
use holon_sharing::types::BlockId;
use holon_sharing::types::StablePeerId;
use holon_sharing::types::UnverifiedAuthority;
use proptest::prelude::*;

// ── The vault under test ─────────────────────────────────────────────────
//
// One shared subtree `share-root` = {s1, s2}; everything else is owner-private
// and resolves to the root container.

const SHARED_SELECTOR: &str = "share-root";

fn shared_subtree() -> MapContainment {
    MapContainment::new().with_subtree(
        BlockId(SHARED_SELECTOR.into()),
        [BlockId("s1".into()), BlockId("s2".into())],
    )
}

/// The container the model expects a block to live in.
fn model_container(block: &str) -> &'static str {
    match block {
        SHARED_SELECTOR | "s1" | "s2" => SHARED_SELECTOR,
        _ => holon_sharing::ROOT_CONTAINER,
    }
}

/// `sharing_on == false` is the single-user vault: no committed policy, hence
/// one container.
fn overlay(sharing_on: bool) -> PolicyOverlayEnforcer {
    if !sharing_on {
        return PolicyOverlayEnforcer::inert();
    }
    let mut set = PolicySet::new();
    let rel = shared_subtree();
    let log = CrossingLog::new(StablePeerId(1), Box::new(UnverifiedAuthority));
    let policy = Policy {
        selector: BlockId(SHARED_SELECTOR.into()),
        principals: [Principal("colleague".into())].into_iter().collect(),
        capabilities: Capabilities::read_only(),
        delegation: false,
        lease: Lease::starting_at(0, 1000),
    };
    set.commit(
        SignedPolicy::sign(policy, &UnverifiedAuthority),
        &rel,
        &UnverifiedVerifier,
        &log,
    )
    .expect("a single selector cannot overlap anything");
    PolicyOverlayEnforcer::new(set, Box::new(shared_subtree()))
}

// ── Witness: proof the seam executed ─────────────────────────────────────

struct WitnessEnforcer {
    inner: PolicyOverlayEnforcer,
    calls: Arc<AtomicUsize>,
}

impl BoundaryEnforcer for WitnessEnforcer {
    fn check(
        &self,
        op_name: &str,
        behavior: &BoundaryBehavior,
        subject: &str,
        target_parent: Option<&str>,
    ) -> std::result::Result<(), BoundaryRejection> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.check(op_name, behavior, subject, target_parent)
    }
}

// ── A provider that records what actually ran ────────────────────────────

struct RecordingProvider {
    executed: Arc<Mutex<Vec<String>>>,
    ops: Vec<OperationDescriptor>,
}

#[async_trait]
impl OperationProvider for RecordingProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        self.ops.clone()
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        op_name: &str,
        _: StorageEntity,
    ) -> Result<OperationResult> {
        self.executed.lock().unwrap().push(op_name.to_string());
        Ok(OperationResult::irreversible(Vec::new()))
    }
}

fn descriptor(op_name: &str, behavior: BoundaryBehavior) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: "block".into(),
        entity_short_name: "block".to_string(),
        id_column: "id".to_string(),
        name: op_name.to_string(),
        display_name: op_name.to_string(),
        description: op_name.to_string(),
        required_params: vec![],
        affected_fields: vec![],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Block,
        boundary_behavior: behavior,
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

/// One dispatch through the real `OperationDispatcher` with the overlay
/// installed. Returns `(dispatch outcome, ops the provider actually ran,
/// times the boundary seam was consulted)`.
async fn dispatch(
    sharing_on: bool,
    op_name: &str,
    behavior: BoundaryBehavior,
    subject: &str,
    target_parent: Option<&str>,
) -> (std::result::Result<(), String>, Vec<String>, usize) {
    let executed = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(RecordingProvider {
        executed: executed.clone(),
        ops: vec![descriptor(op_name, behavior)],
    });
    let mut dispatcher = OperationDispatcher::new(vec![provider as Arc<dyn OperationProvider>]);
    dispatcher.set_boundary_enforcer(Arc::new(WitnessEnforcer {
        inner: overlay(sharing_on),
        calls: calls.clone(),
    }));

    let mut params = StorageEntity::new();
    params.insert("id".into(), Value::String(subject.into()));
    if let Some(parent) = target_parent {
        params.insert("parent_id".into(), Value::String(parent.into()));
    }

    let outcome = dispatcher
        .execute_operation(&EntityName::new("block"), op_name, params)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string());
    let ran = executed.lock().unwrap().clone();
    let consulted = calls.load(Ordering::SeqCst);
    (outcome, ran, consulted)
}

fn block_strategy() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("s1"),
        Just("s2"),
        Just(SHARED_SELECTOR),
        Just("p1"),
        Just("p2"),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// P1 + P2. A vault with no committed share policy is unaffected: every op
    /// runs. And the seam was consulted anyway — the green is not the green of
    /// a skipped check.
    #[test]
    fn inert_overlay_allows_every_op_and_is_still_consulted(
        subject in block_strategy(),
        parent in block_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (outcome, ran, consulted) = rt.block_on(dispatch(
            false,
            "move_block",
            BoundaryBehavior::Crossing { widens_audience: true },
            subject,
            Some(parent),
        ));
        prop_assert!(
            outcome.is_ok(),
            "a vault with no share policy must not reject anything, got: {outcome:?}"
        );
        prop_assert_eq!(ran, vec!["move_block".to_string()]);
        prop_assert_eq!(
            consulted, 1,
            "WITNESS: the boundary seam must be consulted once per dispatched op"
        );
    }

    /// P3. With the policy committed, a `move_block` whose subject and
    /// destination sit in different containers changes the audience. It must be
    /// refused loudly and the provider must never run.
    ///
    /// P4 is the complement: same container ⇒ the op still works. Sharing must
    /// not freeze ordinary editing inside a share.
    #[test]
    fn committed_policy_refuses_cross_container_moves_and_permits_the_rest(
        subject in block_strategy(),
        parent in block_strategy(),
    ) {
        let crosses = model_container(subject) != model_container(parent);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (outcome, ran, consulted) = rt.block_on(dispatch(
            true,
            "move_block",
            BoundaryBehavior::Crossing { widens_audience: true },
            subject,
            Some(parent),
        ));
        prop_assert_eq!(
            consulted, 1,
            "WITNESS: the boundary seam must be consulted once per dispatched op"
        );
        if crosses {
            let err = outcome.expect_err(
                "moving a block between containers changes its audience and must be refused",
            );
            prop_assert!(
                err.contains("move_block") && err.contains("boundary"),
                "the rejection must name the op and the boundary, got: {}",
                err
            );
            prop_assert!(
                ran.is_empty(),
                "a refused op must NOT reach the provider, but it ran: {:?}",
                ran
            );
        } else {
            prop_assert!(
                outcome.is_ok(),
                "a move within one container changes no audience, got: {:?}",
                outcome
            );
            prop_assert_eq!(ran, vec!["move_block".to_string()]);
        }
    }

    /// A within-container mutation (`PrivateOnly`, names no destination) keeps
    /// working everywhere, shared subtree included.
    #[test]
    fn private_ops_are_unaffected_by_the_overlay(
        sharing_on in any::<bool>(),
        subject in block_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (outcome, ran, consulted) = rt.block_on(dispatch(
            sharing_on,
            "cycle_task_state",
            BoundaryBehavior::PrivateOnly,
            subject,
            None,
        ));
        prop_assert_eq!(consulted, 1, "WITNESS: the boundary seam must be consulted");
        prop_assert!(
            outcome.is_ok(),
            "a PrivateOnly op crosses nothing and must run, got: {:?}",
            outcome
        );
        prop_assert_eq!(ran, vec!["cycle_task_state".to_string()]);
    }
}

/// An op that CAN relocate a block but whose intent names no destination
/// (`indent` computes its new parent from the tree) cannot be judged at this
/// seam. While its subject is inside a share, it is refused rather than waved
/// through — the container-resolving layer owes the seam both endpoints.
#[tokio::test]
async fn relocating_op_without_a_named_destination_is_refused_inside_a_share() {
    let (outcome, ran, consulted) = dispatch(
        true,
        "indent",
        BoundaryBehavior::Crossing {
            widens_audience: true,
        },
        "s1",
        None,
    )
    .await;
    assert_eq!(consulted, 1, "WITNESS: the boundary seam must be consulted");
    let err = outcome.expect_err("an unjudgeable relocation inside a share must be refused");
    assert!(
        err.contains("indent") && err.contains("names no destination"),
        "the rejection must say why it could not be judged, got: {err}"
    );
    assert!(ran.is_empty(), "a refused op must not reach the provider");

    // Outside any share the same op is unaffected.
    let (outcome, ran, _) = dispatch(
        true,
        "indent",
        BoundaryBehavior::Crossing {
            widens_audience: true,
        },
        "p1",
        None,
    )
    .await;
    assert!(
        outcome.is_ok(),
        "owner-private editing is untouched: {outcome:?}"
    );
    assert_eq!(ran, vec!["indent".to_string()]);
}
