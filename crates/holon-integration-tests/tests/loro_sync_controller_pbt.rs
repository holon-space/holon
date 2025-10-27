//! Layer 3 bridge PBT: exercises `LoroSyncController` against the shared
//! `GroupTransition` stream from `holon::sync::multi_peer`.
//!
//! The SUT (`StubSyncSut`) wraps a `StubSut` (from the shared PBT kit)
//! plus a tokio runtime, so `StateMachineTest::apply` — which is sync —
//! can drive the async controller via `block_on`.
//!
//! Each transition:
//! 1. Has already been applied to the reference `GroupState<()>` by the
//!    state machine harness.
//! 2. Is forwarded to the `StubSut`, which mirrors the reference state's
//!    peer 0 Loro doc into the SUT's primary doc. The controller's
//!    `subscribe_root` fires and reconciles the diff into the stub
//!    `OperationProvider`'s in-memory block store.
//! 3. `wait_for_quiescence` blocks until `last_synced == oplog_frontiers`.
//! 4. `check_invariants` runs both the multi-peer structural/convergence
//!    checks (S1–S3, C1–C3) and the bridge-level checks (I1–I3).

use std::sync::Arc;

use holon_integration_tests::pbt::loro_sync::{
    LoroSyncReference, LoroSyncSut, StubSut, check_all_invariants,
};
use proptest::prelude::*;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};

/// Thin wrapper that owns a tokio runtime and a `StubSut`. Needed because
/// `StateMachineTest::apply` is sync but `LoroSyncSut` is async.
struct StubSyncSut {
    runtime: Arc<tokio::runtime::Runtime>,
    inner: StubSut,
}

impl std::fmt::Debug for StubSyncSut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StubSyncSut")
            .field("inner", &self.inner)
            .finish()
    }
}

struct LoroSyncControllerPbt;

impl StateMachineTest for LoroSyncControllerPbt {
    type SystemUnderTest = StubSyncSut;
    type Reference = LoroSyncReference;

    fn init_test(
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("build tokio runtime"),
        );
        let inner = runtime.block_on(async {
            let mut sut = StubSut::new().await.expect("StubSut::new");
            // Drain the synthetic startup wake so the SUT is quiescent
            // before any assertions run against it.
            sut.wait_for_quiescence().await;
            sut
        });
        StubSyncSut { runtime, inner }
    }

    fn apply(
        mut sut: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        let runtime = sut.runtime.clone();
        runtime.block_on(async {
            sut.inner
                .apply(ref_state, &transition)
                .await
                .expect("StubSut::apply");
            sut.inner.wait_for_quiescence().await;
        });
        sut
    }

    fn check_invariants(
        sut: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        let runtime = sut.runtime.clone();
        runtime.block_on(async {
            check_all_invariants(&sut.inner, ref_state).await;
        });
    }
}

prop_state_machine! {
    #![proptest_config(ProptestConfig {
        cases: 40,
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::WithSource("pbt-regressions")
        )),
        timeout: 60_000,
        verbose: 2,
        .. ProptestConfig::default()
    })]

    #[test]
    fn loro_sync_controller_bridge_pbt(sequential 1..40 => LoroSyncControllerPbt);
}
