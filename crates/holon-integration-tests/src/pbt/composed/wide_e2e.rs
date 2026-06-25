//! **THE SWAP (§5) — the composed `general_e2e` slice, `pbt`-gated so it can drive a
//! `tests/` integration test.**
//!
//! Relocated out of `frontend_slice/structural_pbt.rs` (a `#[cfg(test)]`-only module)
//! into this `#[cfg(any(test, feature = "pbt"))]` module so the production integration
//! test `general_e2e_composed_pbt` (in `tests/`) — which links the lib built WITHOUT
//! `cfg(test)` — can declare [`ComposedSut<WideE2E>`]. The lib slices/teeth in
//! `structural_pbt.rs` now `use` these items instead of defining their own copies, so
//! there is a SINGLE source of truth (North Star: one composed convergence PBT).
//!
//! [`WideE2E`] drives the PRODUCTION `E2ETransition` enum via the PRODUCTION
//! `aggregate_transitions` generator (NOT a curated list) over `compose_sut(full_headless)`
//! — the exact SUT + alphabet the `general_e2e_pbt` swap targets. The alphabet
//! auto-narrows to the composed SUT's drivable caps (peer/seam/E4/fixture cap-gate out;
//! watches + mutate are DELIBERATELY narrowed pending B5 / the Loro-doc-unification fix —
//! see [`wide_e2e_ref`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use holon_api::{Block, EntityUri, Region};
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::composition::{CapMap, CapSet};
use holon_pbt_core::{ComponentSet, TransitionImpl, TransitionRef};
use proptest::prelude::Just;
use proptest::strategy::{BoxedStrategy, Strategy};
use proptest_state_machine::ReferenceStateMachine;

use crate::pbt::composed::builder::{compose_sut, compose_sut_seeded};
use crate::pbt::composed::harness::{ComposedSlice, sut_ids};
use crate::pbt::composed::seed_primitives::fixed_ids;
use crate::pbt::composed::subsystem_seed::build_started_ref;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transitions::{E2ETransition, NavigateFocus};

/// The seed **page** the working blocks sit directly under. It is the focus root (so
/// its children are the editable candidates) but is itself excluded from candidates
/// (`is_page`) and from the comparison (seed), so it is never split and its page-ness
/// is never compared.
pub fn page_root() -> EntityUri {
    EntityUri::block("structural-page")
}

/// Settle window for the headless CDC pump after a write.
pub const SETTLE: Duration = Duration::from_millis(150);

/// The working tree AS the boot org (page-rooted leaf siblings, pinned bare `:ID:`),
/// so the session ingests it into the store AND `SutOrgRead` parses it — store and
/// org share one source, keeping `inv-blocks-match-ref/org` green. The filename is the
/// page title the viewmodel renders (the oracle's page content is `structural-page`).
pub const WIDE_TREE_ORG: &str = "#+ID: structural-page\n\
    * parent\n:PROPERTIES:\n:ID: parent\n:END:\n\
    * c1\n:PROPERTIES:\n:ID: c1\n:END:\n\
    * c2\n:PROPERTIES:\n:ID: c2\n:END:\n";

/// The page-rooted leaf-sibling oracle (`parent`/`c1`/`c2` re-rooted under a seed
/// `page_root`, focused on the page), wired by `subsystems` (invariant selection) +
/// nav-history aligned to the headless boot stack `[journals, page]`.
pub fn structural_ref_wired(
    subsystems: &BTreeSet<crate::pbt::invariants::registry::Subsystem>,
) -> ReferenceState {
    let mut state = build_started_ref(subsystems);
    let page = page_root();
    let ids = fixed_ids();

    // Insert the page root: a seed block (`block_documents[page]=no_parent`, filtered
    // out of the comparison) AND a page (excluded from `main_editable_descendants`).
    let mut page_block = Block::new_text(page.clone(), EntityUri::no_parent(), "structural-page");
    page_block.set_page(true);
    state
        .domain
        .block_state
        .blocks
        .insert(page.clone(), page_block);
    state
        .domain
        .block_state
        .block_documents
        .insert(page.clone(), EntityUri::no_parent());

    // Re-root parent/c1/c2 as leaf siblings directly under the page.
    for (i, id) in [&ids.parent, &ids.c1, &ids.c2].into_iter().enumerate() {
        let b = state
            .domain
            .block_state
            .blocks
            .get_mut(id)
            .expect("seed block present");
        b.parent_id = page.clone();
        b.set_sequence(i as i64);
    }

    NavigateFocus {
        region: Region::Main,
        block_id: page.clone(),
    }
    .apply_to_ref(&mut state);

    // Nav-history boot alignment: mirror the headless SUT's `[journals, page]` cursor-1
    // stack (page-pin id 2, next_history_id 3) so the folded nav transitions stay in
    // lockstep with the AUTOINCREMENT counter.
    let journals = EntityUri::parse("block:journals").expect("journals id");
    let history = state
        .ui
        .tab
        .navigation_history
        .entry(Region::Main)
        .or_default();
    history.entries = vec![Some(journals), Some(page)];
    history.cursor = 1;
    if let Some(pins) = state.ui.user.open_pins.get_mut(&Region::Main) {
        for p in pins.iter_mut() {
            p.history_id = 2;
        }
    }
    state.ui.tab.next_history_id = 3;
    state
}

/// The structural oracle (no extra subsystems wired — focus caps absent so it never
/// false-REDs the focus invariants).
pub fn structural_ref() -> ReferenceState {
    structural_ref_wired(&BTreeSet::new())
}

/// The combined wide oracle: the same page-rooted tree as [`structural_ref`], wired
/// `{Loro, EditorState}` so the editor/focus transitions gate. No editor open at start
/// (the boot's auto-open on `c1` is blurred by the final `NavigateFocus(page)`).
pub fn wide_ref() -> ReferenceState {
    use crate::pbt::invariants::registry::Subsystem;
    let subsystems: BTreeSet<Subsystem> = [Subsystem::Loro, Subsystem::EditorState]
        .into_iter()
        .collect();
    structural_ref_wired(&subsystems)
}

/// Non-vacuity floor for the wide/swap slices: block + focus/nav/viewmodel/org + Loro
/// invariants that only the FULL `full_headless` cap set selects. "Green" means the
/// production enum drove the real headless render+nav+org+loro pipeline and ALL agreed.
pub const WIDE_REQUIRED_INVARIANTS: &[&str] = &[
    "inv-no-orphan-blocks",
    "inv-no-parent-cycles",
    "inv-blocks-match-ref/block_raw",
    "inv-block-parent-matches-ref/block_raw",
    "inv-blocks-match-ref/org",
    "inv-navigation-focus",
    // SQL-projection per-block content equality. `full_headless` hosts `SutSqlProjection`
    // (the SQL `block_raw.content` read), so requiring it every tick makes the ONE PBT the
    // owner of this check — replacing the deleted standalone `split_block_content_pbt`.
    "inv-block-content-matches-ref",
    "inv-focus-roots",
    "inv-viewmodel-no-error-widgets",
    // ViewModel liveness (C-remainder port, 2026-06-23): same root-VM readiness as
    // `inv-viewmodel-no-error-widgets`, so required every tick = a non-vacuity proof
    // they run over the real headless render pipeline (not silently skipped).
    "inv-frontend-engine",
    "inv-frontend-root-not-error",
    "inv-loro-no-errors",
    "inv-loro-children-match-ref",
    // Per-transition SQL/wall/RSS budget: required every tick = a non-vacuity proof the
    // composed `ComposedSpanMetrics` lifecycle (reset-on-apply / freeze-on-check) is
    // actually driven over the production full-headless CapMap. Runs `Ok` (clean) or
    // `Skipped` (unenforced violation) by default — both count as "ran".
    "inv-sql-budget",
    // Cross-store task_state coherence (SQL `json_extract` vs Loro `properties` scalar).
    // `full_headless` always hosts BOTH `SutSqlProjection` + `SutLoroTaskState` (asserted
    // in `builder.rs`), so requiring it every tick is safe and makes the ONE PBT the
    // owner of this check — replacing the deleted standalone `task_state_coherence_pbt`.
    // It RUNS every tick (both projections compared, trivially coherent when untouched);
    // that a real `ToggleState` moves both stores in lockstep is the separate non-vacuity
    // teeth in `composed::invariants::task_state_storage_coherence`.
    "inv-task-state-storage-coherence",
];

/// Boot the windowless production session with the working tree as the boot org, via
/// the PRODUCTION builder (`compose_sut_seeded`) over `full_headless` — the EXACT CapMap
/// the `general_e2e_pbt` swap targets — then drive the initial focus onto the page root
/// (matching the oracle) and return the cap map + the booted scaffold ids.
pub async fn boot_and_seed_wide(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
) -> (CapMap, BTreeSet<EntityUri>) {
    let set = ComponentSet::full_headless();
    let bundle =
        compose_sut_seeded(&set, resolver, &[("structural-page.org", WIDE_TREE_ORG)]).await;
    let mut caps = bundle.caps;

    // `inv-sql-budget` coverage: a span-metrics provider hosting the SAME `MetricsSut`
    // the native E2ESut uses, exposed through `ComposedBudget` (the read) +
    // `SutMetricsLifecycle` (the `ComposedSut` harness drives reset-on-apply /
    // freeze-on-check). One `Arc`, registered as both caps.
    #[cfg(feature = "otel-testing")]
    {
        use crate::pbt::composed::span_metrics::{
            ComposedBudget, ComposedSpanMetrics, SutMetricsLifecycle,
        };
        let m = std::sync::Arc::new(ComposedSpanMetrics::new());
        caps.insert(m.clone() as std::sync::Arc<dyn ComposedBudget>);
        caps.insert(m as std::sync::Arc<dyn SutMetricsLifecycle>);
    }

    // Scaffold = everything booted EXCEPT the non-seed working tree (parent/c1/c2);
    // `structural-page` stays in the scaffold (injected). The builder's own
    // `scaffold_ids` assumes a post-boot engine seed, so recompute against the tree.
    let ids = fixed_ids();
    let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
        .into_iter()
        .collect();
    let booted = sut_ids(&caps).await;
    let scaffold: BTreeSet<EntityUri> = booted.difference(&tree).cloned().collect();

    // Fresh-drive the initial focus on the SUT to match the oracle (page root).
    TransitionImpl::apply_to_sut(
        &NavigateFocus {
            region: Region::Main,
            block_id: page_root(),
        },
        ref_state,
        &mut caps,
    )
    .await;
    tokio::time::sleep(SETTLE).await;

    (caps, scaffold)
}

/// The `full_headless` cap set, computed ONCE by booting `compose_sut(full_headless)`
/// on a throwaway current-thread runtime and extracting the (runtime-free) `CapSet`.
/// The swap ref carries this so `aggregate_transitions` auto-narrows the production
/// alphabet to exactly what the composed SUT can drive.
pub fn full_headless_cap_set() -> CapSet {
    use std::sync::OnceLock;
    static CELL: OnceLock<CapSet> = OnceLock::new();
    CELL.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime for cap_set extraction");
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let cs = rt.block_on(async {
            compose_sut(&ComponentSet::full_headless(), &resolver)
                .await
                .caps
                .cap_set()
        });
        drop(rt);
        cs
    })
    .clone()
}

/// The swap oracle: the seeded wide tree re-wired to `full_headless` (the production
/// `general_e2e_pbt` wiring — full storage + projections, NO UI actor) carrying the
/// `full_headless` cap_set, so `aggregate_transitions` gates the alphabet to the
/// composed SUT's real caps.
pub fn wide_e2e_ref() -> ReferenceState {
    let mut state = wide_ref();
    state.wiring = ComponentSet::full_headless().wiring;
    // NO deliberate narrowing remains: the swap drives the FULL production
    // `aggregate_transitions` alphabet auto-narrowed by the real `full_headless` cap set.
    // - `SutMutate` → `ToggleState`: un-narrowed task #4 (Loro read-doc unify + real
    //   `cycle_task_state` toggle).
    // - `SutWatchRegister` → `SetupWatch`/`RemoveWatch`: un-narrowed task #5 — the watch
    //   invariant now seed-excludes both sides (scaffold blocks on the SUT, the phantom
    //   `started-ref-layout-query` on the oracle), so `inv-watch-rows-match-ref` compares
    //   only the non-seed working tree.
    state.with_cap_set(full_headless_cap_set())
}

/// Reference machine over the production `E2ETransition`, generated by the FULL
/// production `aggregate_transitions` (auto-narrowed by the ref's wiring + cap_set).
pub struct WideE2EMachine;

impl ReferenceStateMachine for WideE2EMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(wide_e2e_ref()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        crate::pbt::transitions::aggregate_transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        transition.preconditions(state).is_good()
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        transition.apply_to_ref(&mut state);
        state.action.last_transition_kind = Some(transition.variant_name());
        state
    }
}

/// The swap slice: production `E2ETransition` enum, production `aggregate_transitions`
/// generator, composed `compose_sut(full_headless)` SUT (via [`boot_and_seed_wide`]).
pub struct WideE2E;

impl ComposedSlice for WideE2E {
    type Transition = E2ETransition;
    type Machine = WideE2EMachine;
    type Handle = ();
    const REQUIRED_INVARIANTS: &'static [&'static str] = WIDE_REQUIRED_INVARIANTS;
    const SETTLE: Duration = SETTLE;
    const MULTI_THREAD: bool = true;

    async fn build(
        resolver: &IdResolver,
        ref_state: &ReferenceState,
    ) -> (CapMap, (), BTreeSet<EntityUri>) {
        let (caps, scaffold) = boot_and_seed_wide(resolver, ref_state).await;
        (caps, (), scaffold)
    }

    async fn apply_transition(
        transition: &E2ETransition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
    ) {
        // Reset the span collector + record the wall/RSS baseline for THIS transition,
        // before its SQL runs — so `inv-sql-budget` measures the transition, not the
        // accumulation of every prior tick. (`freeze_for_check` snapshots at check time.)
        #[cfg(feature = "otel-testing")]
        if let Some(m) = caps.get::<dyn crate::pbt::composed::span_metrics::SutMetricsLifecycle>() {
            m.note_transition_start(transition);
        }
        TransitionImpl::apply_to_sut(transition, ref_state, caps).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbt::transitions::{AddPeer, MergeFromPeer, PeerEdit, PeerEditOp, SyncWithPeer};
    use holon_pbt_core::TransitionImpl;

    /// A4 NON-VACUITY: the `full_headless` cap set now ADMITS the peer transitions, so
    /// `aggregate_transitions` auto-selects them into the swap alphabet. Before A2 this
    /// would FAIL (the builder withheld `SutLoro` in full mode → `required_caps()=[SutLoro]`
    /// were unsatisfiable → peer ops auto-narrowed out). A green `general_e2e_composed_pbt`
    /// run where peer ops never fired would be a false pass; this proves they CAN fire,
    /// deterministically and fast (no reliance on trace logs).
    #[test]
    fn full_headless_cap_set_admits_peer_transitions() {
        let oracle = wide_e2e_ref();
        let peer_transitions = [
            E2ETransition::AddPeer(AddPeer),
            E2ETransition::PeerEdit(PeerEdit {
                peer_idx: 0,
                op: PeerEditOp::Create {
                    parent_stable_id: None,
                    content: "x".into(),
                    stable_id: "peer-x".into(),
                },
            }),
            E2ETransition::MergeFromPeer(MergeFromPeer { peer_idx: 0 }),
            E2ETransition::SyncWithPeer(SyncWithPeer { peer_idx: 0 }),
        ];
        for t in &peer_transitions {
            assert!(
                oracle.caps_available(&t.required_caps()),
                "full_headless cap set must admit {:?} (required_caps={:?}) — peer mesh wired \
                 in A2; if this fails, SutLoro is not present in the composed full_headless build",
                t.variant_name(),
                t.required_caps()
            );
        }
    }
}
