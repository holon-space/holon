//! Shared PBT kit for testing `LoroSyncController` against different SUT
//! implementations.
//!
//! This is the Loro-side analogue of the `UserDriver` trait
//! (`crates/holon-integration-tests/src/mutation_driver.rs`): it hoists the
//! transition enum, generators, preconditions, reference-state machine, and
//! structural/convergence invariants from `holon::sync::multi_peer` so that
//! multiple tests can exercise the controller against different systems:
//!
//! - `StubSut` — a minimal in-process `LoroSyncController` wired to stub
//!   `OperationProvider` and `EventBus` implementations. Used by the focused
//!   bridge PBT (`tests/loro_sync_controller_pbt.rs`).
//! - `E2ESut` — the full production stack used by the e2e PBT. Gets the
//!   bridge invariants "for free" when Loro is enabled.
//!
//! All SUTs see the same `GroupTransition` stream and are judged against the
//! same multi-peer structural invariants (S1–S3, C1–C3) plus bridge-specific
//! invariants (I1–I3) defined in this module.

pub mod stub_sut;

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use holon::sync::multi_peer::{self, DirectSync, GroupState, GroupTransition};
use loro::Frontiers;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;

pub use stub_sut::StubSut;

/// A deterministic summary of a block, suitable for set comparison.
///
/// Kept small enough that `BTreeMap<BlockId, BlockSnapshot>` equality has a
/// clear failure mode when `assert_eq!` fires.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct BlockSnapshot {
    pub id: String,
    pub parent_id: String,
    pub content: String,
}

/// Abstract system under test for the Loro ↔ command/event bus bridge.
///
/// Implementations drive a real `LoroSyncController` instance (or equivalent)
/// and expose accessors so the shared invariant checks can inspect both
/// sides of the bridge.
#[async_trait::async_trait]
pub trait LoroSyncSut: Send + Sync + std::fmt::Debug {
    /// Apply a `GroupTransition` to the SUT.
    ///
    /// The caller has already advanced the reference state, so the SUT can
    /// read the post-transition `GroupState` for context (e.g. to snapshot
    /// what peer 0's LoroDoc now contains and mirror it into the primary
    /// doc the controller is watching).
    async fn apply(&mut self, state: &GroupState<()>, transition: &GroupTransition) -> Result<()>;

    /// Block until the controller is idle. Called once per transition
    /// before invariants are checked.
    async fn wait_for_quiescence(&mut self);

    /// Snapshot the downstream block store as the command/event bus side
    /// sees it. Used by `I1 — downstream mirror`.
    async fn downstream_snapshot(&self) -> BTreeMap<String, BlockSnapshot>;

    /// Current `last_synced` frontiers on the controller. `I2 — watermark
    /// never exceeds oplog`.
    async fn last_synced_frontiers(&self) -> Frontiers;

    /// Current `oplog_frontiers` on the SUT's primary Loro doc.
    async fn primary_oplog_frontiers(&self) -> Frontiers;

    /// Snapshot the SUT's primary Loro doc as a map of `BlockSnapshot`.
    /// Used by `I1` to compare against the downstream store.
    async fn primary_loro_snapshot(&self) -> BTreeMap<String, BlockSnapshot>;

    /// Accumulated error count reported by the controller.
    /// `I3 — no silent drops`.
    fn error_count(&self) -> usize;
}

/// Zero-sized marker implementing `ReferenceStateMachine` for the shared
/// transition enum. Tests use this as `type Reference = LoroSyncReference`.
pub struct LoroSyncReference;

impl ReferenceStateMachine for LoroSyncReference {
    type State = GroupState<()>;
    type Transition = GroupTransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(GroupState::new(Arc::new(DirectSync))).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        multi_peer::generate_transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        multi_peer::check_preconditions(state, transition)
    }

    fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
        multi_peer::apply_transition(state, transition)
    }
}

// -- Invariants ------------------------------------------------------------

/// Run the full set of bridge-level invariants against a `LoroSyncSut`.
///
/// Callers typically also invoke `multi_peer::check_invariants(ref_state)`
/// for the structural (S1–S3) and convergence (C1–C3) checks on the
/// reference-side peers. Those are orthogonal to what's tested here; this
/// function verifies that the SUT's downstream store accurately mirrors
/// the SUT's primary Loro doc through the controller.
pub async fn check_bridge_invariants<S: LoroSyncSut + ?Sized>(sut: &S) {
    // I1 — downstream mirror: the downstream store must match what the
    // SUT's primary Loro doc currently contains. This is the end-to-end
    // assertion that the controller's outbound direction is working.
    let downstream = sut.downstream_snapshot().await;
    let primary = sut.primary_loro_snapshot().await;
    similar_asserts::assert_eq!(
        downstream,
        primary,
        "I1 FAILED: downstream store does not mirror SUT primary Loro doc"
    );

    // I2 — watermark monotonicity: `last_synced` should never exceed
    // `oplog_frontiers`, and once quiescent should equal it.
    let last = sut.last_synced_frontiers().await;
    let current = sut.primary_oplog_frontiers().await;
    assert_eq!(
        last, current,
        "I2 FAILED: last_synced_frontiers != primary oplog_frontiers after quiescence",
    );

    // I3 — no silent drops.
    assert_eq!(
        sut.error_count(),
        0,
        "I3 FAILED: controller reported {} errors",
        sut.error_count(),
    );
}

/// Run both the multi-peer structural/convergence invariants (from
/// `holon::sync::multi_peer`) and the bridge-level invariants.
pub async fn check_all_invariants<S: LoroSyncSut + ?Sized>(sut: &S, ref_state: &GroupState<()>) {
    multi_peer::check_invariants(ref_state);
    check_bridge_invariants(sut).await;
}
