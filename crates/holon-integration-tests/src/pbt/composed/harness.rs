//! **F2 E3 — the generic composed-SUT `StateMachineTest` harness (increment 3).**
//!
//! Every composed slice's `StateMachineTest` had a near-identical skeleton: a
//! `CapMap` + a shared `IdResolver` + a runtime; an `apply` that snapshots the SUT
//! id-set before/after the transition and reconciles the one minted id; a
//! `check_invariants` that runs the shared catalog and asserts no failures + a
//! non-vacuity floor. This module owns the skeleton; a slice provides only the axes
//! that genuinely differ via [`ComposedSlice`].
//!
//! ## The axes a slice varies
//! - **alphabet** — `Transition`/`Machine` + the per-tick `apply_transition` `match`.
//! - **seed** — `build` boots the component(s) and returns the cap map (+ scaffold ids).
//! - **runtime/settle** — `MULTI_THREAD` + `SETTLE` (≈0 for a synchronous backend).
//! - **id alignment** — [`ComposedSlice::align_ids`]. Default = nothing (the harness's
//!   generic per-tick reconcile maps a uuid-minting backend's real ids onto the
//!   oracle's synthetic ones). A counter-sync backend (`MemoryBackend`) overrides it
//!   to push the oracle's `next_id` into its split-id hint so the next mint *is* the
//!   oracle's id — the reconcile then sees an identity pair.
//! - **check scope** — [`ComposedSlice::run_report`]. Default = full catalog over the
//!   scaffold-seeded, reconcile-resolved oracle. A focus-only slice (nav) overrides it
//!   to a `RefFocus`-only `run_selected` over the raw oracle.
//!
//! The reconcile + scaffold-injection + check are the composed-SUT *execution
//! kernel* — the same logic the eventual wide-PBT-over-`compose_sut` path needs, so
//! this is not slice-only scaffolding.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use holon_api::EntityUri;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::composition::{CapMap, InvariantId, RunReport};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
use crate::pbt::is_synthetic_ref_id;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::{ReferenceState, Resolved};

/// Synthetic ref ids the COMPOSED per-tick reconcile maps to freshly-minted SUT
/// ids: split tails (`block::split-N`, via the global predicate) PLUS
/// CreateDocument-minted doc pages (`block:ref-doc-N`, from
/// `ReferenceState::next_synthetic_doc_uri`). Composed-local on purpose — the global
/// [`is_synthetic_ref_id`] stays split-only so E2ESut's mapping is unaffected.
fn is_composed_minted_synthetic_id(id: &EntityUri) -> bool {
    is_synthetic_ref_id(id) || id.as_str().starts_with("block:ref-doc-")
}

/// Ids of peer-created blocks (`block:peer-HHHH-LLLL-IIII-SSSS`, minted by
/// `transitions::deterministic_peer_block_id`). These are STABLE and IDENTICAL on both
/// the oracle (`merge_peer_blocks_into_primary`) and the SUT (peer-create uses the raw
/// stable id) — no UUID is minted, so they need NO synthetic→real mapping. They must be
/// excluded from `real_new` in the reconcile: a `MergeFromPeer` surfaces a fresh
/// `block:peer-…` row in the SUT `block_raw`, but there is no matching synthetic id, so
/// the 1:1 `synthetic.len() == real_new.len()` guard would otherwise panic. The
/// identity-resolved peer id already lines up with the oracle, so dropping it here is
/// correct, not a fudge.
fn is_peer_scheme_id(id: &EntityUri) -> bool {
    id.as_str().starts_with("block:peer-")
}

/// The SUT `block_raw` id-set (via the `SutBackend` cap) — the reconcile loop's
/// before/after snapshot.
pub(crate) async fn sut_ids(caps: &CapMap) -> BTreeSet<EntityUri> {
    caps.expect::<dyn SutBackend>()
        .block_raw_snapshot()
        .await
        .into_iter()
        .map(|b| b.id.clone())
        .collect()
}

/// Inject `scaffold_ids` into the oracle as `block_documents[id]=no_parent` so they
/// join `seed_block_ids()` and filter out of the SUT-side block comparison.
pub(crate) fn inject_scaffold_seed(
    oracle: &mut Resolved<ReferenceState>,
    scaffold_ids: &BTreeSet<EntityUri>,
) {
    let oracle = oracle.inner_mut();
    for id in scaffold_ids {
        oracle
            .domain
            .block_state
            .block_documents
            .insert(id.clone(), EntityUri::no_parent());
    }
}

/// What a composed slice contributes to the generic harness — the six axes that
/// differ between slices. Everything else (reconcile, check, runtime) is the
/// harness's.
#[allow(async_fn_in_trait)]
pub trait ComposedSlice {
    /// The slice's transition alphabet enum.
    type Transition: Clone + std::fmt::Debug;
    /// The reference machine that generates/applies those transitions over a
    /// [`ReferenceState`].
    type Machine: ReferenceStateMachine<State = ReferenceState, Transition = Self::Transition>;
    /// A slice-owned handle stored beside the caps — e.g. a backend component a
    /// counter-sync slice pushes `next_id` into. `()` when the cap map is enough
    /// (the caps already keep their component alive).
    type Handle;

    /// Invariants that MUST run each tick — the non-vacuity floor so "green" means
    /// "ran over real data", not "deselected everything".
    const REQUIRED_INVARIANTS: &'static [&'static str];
    /// CDC settle window after a write (≈0 for synchronous stores, ~150ms for a
    /// booted `FrontendSession`).
    const SETTLE: Duration;
    /// Whether the SUT needs a multi-thread runtime (a booted session does).
    const MULTI_THREAD: bool = false;

    /// The non-vacuity floor for THIS draw. Default: the full static
    /// [`REQUIRED_INVARIANTS`](Self::REQUIRED_INVARIANTS) (correct for a fixed-wiring
    /// slice). A wiring-parameterized slice (`WideE2E`) overrides this to intersect the
    /// static floor with the invariants its drawn `ref_state.wiring` can actually select —
    /// so a Loro-only draw is not required to run the SQL/ViewModel/focus invariants it has
    /// no caps for (which would false-RED the floor). Returns parsed [`InvariantId`]s,
    /// not raw strings, so the floor is compared against the report's typed ids directly.
    fn required_invariants(_: &ReferenceState) -> Vec<InvariantId> {
        Self::REQUIRED_INVARIANTS
            .iter()
            .copied()
            .map(InvariantId)
            .collect()
    }

    /// Boot + seed the SUT, returning the cap map, the slice handle, and the booted
    /// scaffold ids to seed-inject into the oracle. `resolver` is shared with the
    /// reconcile loop so a uuid-minting backend's writer resolves synthetic→real ids;
    /// `ref_state` is the initial oracle (a counter-sync slice seeds its `next_id`).
    async fn build(
        resolver: &IdResolver,
        ref_state: &ReferenceState,
    ) -> (CapMap, Self::Handle, BTreeSet<EntityUri>);

    /// Dispatch one transition onto the SUT caps (the per-alphabet `match`).
    async fn apply_transition(
        transition: &Self::Transition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
    );

    /// Align SUT-minted ids with the oracle. Called once after `build` (the initial
    /// seed) and after every apply. Default: nothing — the harness's generic per-tick
    /// reconcile already maps a uuid-minting backend's real ids onto the oracle's
    /// synthetic ones. A counter-sync backend overrides this to push
    /// `ref_state.domain.block_state.next_id` into its split-id hint.
    fn align_ids(_: &Self::Handle, _: &ReferenceState) {}

    /// Produce the catalog run report for a check. Default: resolve the oracle's
    /// doc-uris through the reconcile map, seed-inject the booted scaffold, and run the
    /// full catalog via `run_with_seeded_ref`. A focus-only slice (nav) overrides this
    /// to a `RefFocus`-only `run_selected` over the raw oracle (no minting, no scaffold).
    async fn run_report(
        caps: &CapMap,
        resolver: &IdResolver,
        scaffold_ids: &BTreeSet<EntityUri>,
        ref_state: &ReferenceState,
    ) -> RunReport {
        let map = resolver.lock().expect("resolver lock").clone();
        let mut resolved = ref_state.with_resolved_doc_uris(&map);
        inject_scaffold_seed(&mut resolved, scaffold_ids);
        // Freeze the budget window for `inv-sql-budget` (if a span-metrics provider is
        // wired): everything up to this check is the transition's cost; the invariant
        // bodies below must not count against it. Hands the host the post-transition
        // oracle its `expected_sql` inspects. Mirrors the native runner's
        // `freeze_at_check_start`. No-op when no `SutMetricsLifecycle` is present.
        #[cfg(feature = "otel-testing")]
        if let Some(m) = caps.get::<dyn crate::pbt::composed::span_metrics::SutMetricsLifecycle>() {
            m.freeze_for_check(resolved.get());
        }
        run_with_seeded_ref(&composed_invariant_catalog(), caps, resolved).await
    }
}

/// The generic composed SUT: a `CapMap` driven through a slice's alphabet, with the
/// per-tick `IdResolver` reconcile and the shared-catalog check.
pub struct ComposedSut<S: ComposedSlice> {
    caps: CapMap,
    handle: S::Handle,
    resolver: IdResolver,
    scaffold_ids: BTreeSet<EntityUri>,
    rt: tokio::runtime::Runtime,
    _slice: PhantomData<S>,
}

impl<S: ComposedSlice> StateMachineTest for ComposedSut<S> {
    type SystemUnderTest = Self;
    type Reference = S::Machine;

    fn init_test(ref_state: &ReferenceState) -> Self {
        let rt = if S::MULTI_THREAD {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build multi-thread runtime")
        } else {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build current-thread runtime")
        };
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let (caps, handle, scaffold_ids) = rt.block_on(S::build(&resolver, ref_state));
        S::align_ids(&handle, ref_state);
        Self {
            caps,
            handle,
            resolver,
            scaffold_ids,
            rt,
            _slice: PhantomData,
        }
    }

    fn apply(mut sut: Self, ref_state: &ReferenceState, transition: S::Transition) -> Self {
        let (before, after) = {
            let caps = &mut sut.caps;
            sut.rt.block_on(async move {
                let before = sut_ids(caps).await;
                S::apply_transition(&transition, ref_state, caps).await;
                tokio::time::sleep(S::SETTLE).await;
                let after = sut_ids(caps).await;
                (before, after)
            })
        };
        // Per-tick reconciliation: a single transition mints at most one block, so
        // the one unmapped synthetic id (oracle, post-apply) pairs 1:1 with the one
        // new real id (SUT). Accumulate into the shared resolver. (For a counter-sync
        // backend the real id *is* the synthetic id, so this maps it to itself — the
        // `align_ids` hook below kept the next mint in lockstep.)
        //
        // The synthetic schemes the harness reconciles: `block::split-N` (split tails)
        // AND `block:ref-doc-N` (CreateDocument-minted doc pages — the doc-uri-minting
        // generalization of the seam's `block_tree_post_action` CreateDocument arm). Both
        // are placeholders the oracle allocates that the SUT backend materializes as fresh
        // ids. This is a COMPOSED-LOCAL predicate, deliberately NOT the global
        // `is_synthetic_ref_id` (which E2ESut keys its split-only mapping off — widening it
        // there would make E2ESut mis-treat doc-uris as splits).
        let mut map = sut.resolver.lock().expect("resolver lock");
        let synthetic: Vec<EntityUri> = ref_state
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| is_composed_minted_synthetic_id(id) && !map.contains_key(id))
            .cloned()
            .collect();
        // Peer-merged blocks (`block:peer-…`) surface in the SUT `block_raw` with a
        // stable id already shared with the oracle — they need no synthetic→real mapping,
        // so exclude them from `real_new` to keep the 1:1 split/doc guard intact (a
        // `MergeFromPeer` would otherwise make `real_new` outrun `synthetic` and panic).
        // Born-equal ids (External `ApplyMutation::Create` and `BulkExternalAdd` write the
        // block WITH its oracle id in the `:ID:` drawer, so `resolve_mutation_ids` leaves a
        // Create's NEW id as-is) surface in the SUT with the SAME id the oracle already
        // holds. Like peer-merged blocks they are shared, need no synthetic→real mapping,
        // and would otherwise make `real_new` outrun `synthetic` (which only counts
        // synthetic-scheme oracle ids) and panic. `resolve_id` passes unmapped ids through
        // as identity, so dropping them here is correct.
        let real_new: Vec<EntityUri> = after
            .difference(&before)
            .filter(|id| !is_peer_scheme_id(id))
            .filter(|id| !ref_state.domain.block_state.blocks.contains_key(*id))
            .cloned()
            .collect();
        assert_eq!(
            synthetic.len(),
            real_new.len(),
            "per-tick reconcile: one synthetic per minted real id (syn={synthetic:?}, real={real_new:?})"
        );
        for (syn, real) in synthetic.into_iter().zip(real_new) {
            map.insert(syn, real);
        }
        drop(map);
        S::align_ids(&sut.handle, ref_state);
        sut
    }

    fn check_invariants(sut: &Self, ref_state: &ReferenceState) {
        let report = sut.rt.block_on(S::run_report(
            &sut.caps,
            &sut.resolver,
            &sut.scaffold_ids,
            ref_state,
        ));
        // `HOLON_PBT_INVARIANTS` disclosed softening: a matched `warn`/`skip` failure is
        // logged loudly and made non-fatal (a DISCLOSED degraded run, not a clean pass);
        // unmatched failures stay fatal. The composed home of the knob relocated from the
        // deleted native runner core. The panic prefix is unchanged so `bisect_driver`'s
        // `reproduction_signature()` still recognizes a composed divergence.
        use crate::pbt::invariant_mode_override::{ModeOverride, invariant_mode_override};
        let hard: Vec<(&str, &str)> = report
            .failures()
            .into_iter()
            .filter(|(id, msg)| match invariant_mode_override(id) {
                Some(ModeOverride::Warn | ModeOverride::Skip) => {
                    eprintln!(
                        "[HOLON_PBT_INVARIANTS] softened (DISCLOSED degraded run) {id}: {msg}"
                    );
                    false
                }
                _ => true,
            })
            .collect();
        assert!(
            hard.is_empty(),
            "reconciled composed sequence diverged from the oracle: {hard:?}"
        );
        for id in S::required_invariants(ref_state) {
            assert!(
                report.ran.iter().any(|(ran_id, _)| *ran_id == id),
                "non-vacuity: {} must run over real data (ran: {:?})",
                id.0,
                report.ran_ids()
            );
        }
    }
}

/// Gherkin `Then`-assertion bridge: a `ComposedSut` evaluates the assert vocabulary
/// (`widget contains`, `focus is on`) against its composed cap surface via
/// [`evaluate_assertion_caps`](crate::pbt::fixtures::assert::evaluate_assertion_caps),
/// reusing the same `rt`/`caps`/`resolver` the PBT path drives. This is what lets the
/// deterministic `.feature` replays run over the composed SUT instead of `E2ESut` — so
/// gherkin becomes one more way to steer the ONE PBT, with the full composed catalog
/// still checked every tick (`replay_steps` calls `check_invariants`).
impl<S: ComposedSlice> crate::pbt::fixtures::FixtureAssertable for ComposedSut<S> {
    fn evaluate_assert(
        &self,
        assertion: &crate::pbt::fixtures::assert::Assertion,
        ref_state: &ReferenceState,
    ) -> Result<(), String> {
        self.rt
            .block_on(crate::pbt::fixtures::assert::evaluate_assertion_caps(
                assertion,
                ref_state,
                &self.caps,
                &self.resolver,
            ))
    }
}
