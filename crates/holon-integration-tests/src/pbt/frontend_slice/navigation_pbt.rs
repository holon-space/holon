//! SutHandle decomposition — increments 1 + 2: drive a **composed `CapMap`** through
//! the production `NavigateFocus` (→ `SutFocusWrite`) and `NavigateHome`
//! (→ `SutNavHistoryWrite`) transitions, decomposed off `SutHandle` onto fine-grained
//! caps, checked by the shared composed-invariant catalog's focus invariants against
//! a real `ReferenceState` focus oracle.
//!
//! This is the first PBT to drive the *navigation* alphabet against a composed
//! slice (NOT the monolithic `E2ESut`): the slice's `CapMap` hosts the `&self`
//! `SutFocusWrite` + `SutNavHistoryWrite` caps on a real headless `FrontendSession`,
//! so each transition runs its real `apply_to_sut` (the production `navigation.focus`
//! / `navigation.go_home` ops) against the windowless engine, and the
//! `current_focus` / `focus_roots` matviews it writes are compared to the oracle via
//! `inv-navigation-focus` / `inv-focus-roots`.
//!
//! Scope = `{ NavigateFocus, NavigateHome }` — the first composed `StateMachineTest`
//! exercising *multiple* decomposed transitions in one heterogeneous loop.
//! `NavigateHome` lowers to `navigation.go_home` (`set_focus(None)` + clear the
//! region's open pins), so it *clears* focus and roots — exercising a settle path
//! `NavigateFocus` (set) never did. `navigate_forward`/`back` (not mirrored headless,
//! see `maybe_mirror_navigation_focus`) are deferred to the windowed component (E4).
//!
//! The ref is hosted as a **`RefFocus`-only** `CapMap`: only the two focus
//! invariants select (they need `RefFocus`), so the slice stays focused on the
//! navigation payoff without the block-tree alignment a full ref would require
//! (the focus invariants read `navigation_history`/`open_pins`, not `block_state`).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use holon_api::{EntityUri, Region};
use holon_pbt_core::capabilities::{
    RefFocus, SutBackend, SutFocus, SutFocusWrite, SutHistoryWrite, SutMcpEmit,
    SutNavHistoryDrive, SutNavHistoryWrite, SutOrgRead, SutOrgRender, SutRenderer,
    SutSqlProjection, SutViewControl, SutViewSelection, SutWatchRegister, SutWatchRows,
};
use holon_pbt_core::composition::{CapInvariant, CapMap, RunReport, run_selected};
use holon_pbt_core::{TransitionImpl, TransitionRef};
use proptest::prelude::Just;
use proptest::prop_oneof;
use proptest::strategy::{BoxedStrategy, Strategy};
use proptest_state_machine::{ReferenceStateMachine, prop_state_machine};

use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::composed::harness::{ComposedSlice, ComposedSut};
use crate::pbt::composed::subsystem_seed::build_started_ref;
use crate::pbt::frontend_slice::builders::frontend_navigation_wide;
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transitions::{
    NavigateBack, NavigateFocus, NavigateForward, NavigateHome, PinBlock, UnpinBlock,
};

/// The two pinned-id page docs the SUT boots (`#+ID: <bare>` → `block:` scheme at
/// the parser boundary), so the SUT doc-block ids match the reference verbatim —
/// no doc-id remapping for this slice. (Verified by the Step-0 probe:
/// `block_raw` carries `block:ref-doc-0` / `block:ref-doc-1`.)
const DOC0_ID: &str = "block:ref-doc-0";
const DOC1_ID: &str = "block:ref-doc-1";

/// Endgame risk probe (compile-only) — the **scaled-up** union-of-bounds keystone
/// question: can ONE concrete SUT type satisfy the union of all transition caps at
/// once (the doc's named gating risk, the miniature of the ~50-transition endgame)?
/// As E3 Phase A ported more caps onto `HeadlessFrontendComponent`, this scaled
/// from the original 3 → 4 → now **14** caps the single type satisfies with no
/// wrapper: the focus/nav drive caps (`SutFocusWrite`, `SutNavHistoryWrite`,
/// `SutNavHistoryDrive`), the A1/A2a drive caps (`SutViewControl`, `SutMcpEmit`,
/// `SutHistoryWrite`), the watch-register drive cap, and the render/projection/org
/// read oracle caps. The bound holds trivially — the union endgame stays tractable
/// at this scale. (`SutBlockTreeWrite` is provided by a *separate* `OpDispatchWriter`
/// Arc in the CapMap, not by this type, so it is intentionally absent here.)
/// See the backlog's "SutHandle decomposition" track.
const _: fn() = || {
    fn assert_cap_union<
        T: SutFocusWrite
            + SutNavHistoryWrite
            + SutNavHistoryDrive
            + SutViewControl
            + SutMcpEmit
            + SutHistoryWrite
            + SutWatchRegister
            + SutWatchRows
            + SutSqlProjection
            + SutFocus
            + SutBackend
            + SutRenderer
            + SutViewSelection
            + SutOrgRead
            + SutOrgRender,
    >() {
    }
    let _ = assert_cap_union::<HeadlessFrontendComponent>;
};

fn doc_org_files() -> Vec<(&'static str, &'static str)> {
    vec![
        // doc0 carries a **stable-id** Text child block (`block:ref-block-0`) under
        // its page (a `:PROPERTIES: :ID:` drawer pins the id, so it survives a boot
        // re-parse deterministically). It is the pinnable `ContentType::Text`,
        // non-page descendant of Main the `PinBlock` path needs — page docs alone
        // yield `NoPinCandidates`. Verified by `headless_pin_block_right_sidebar_probe`.
        (
            "doc0.org",
            "#+ID: ref-doc-0\n* Heading zero\n:PROPERTIES:\n:ID: ref-block-0\n:END:\nFirst pinnable paragraph\n",
        ),
        ("doc1.org", "#+ID: ref-doc-1\n* Doc one heading\n"),
    ]
}

/// The stable-id pinnable Text block doc0 carries (see [`doc_org_files`]).
const PINNABLE_ID: &str = "block:ref-block-0";

fn nav_doc_ids() -> Vec<EntityUri> {
    vec![
        EntityUri::parse(DOC0_ID).expect("valid doc0 id"),
        EntityUri::parse(DOC1_ID).expect("valid doc1 id"),
    ]
}

fn navigate(block_id: EntityUri) -> NavigateFocus {
    NavigateFocus {
        region: Region::Main,
        block_id,
    }
}

fn navigate_home() -> NavigateHome {
    NavigateHome {
        region: Region::Main,
    }
}

/// Pin a block to the right sidebar (`PinBlock` always targets `RightSidebar` —
/// the only place the bullet's shift-click dispatches `focus_pin`).
fn pin(block_id: EntityUri) -> PinBlock {
    PinBlock {
        region: Region::RightSidebar,
        block_id,
    }
}

fn nav_back() -> NavigateBack {
    NavigateBack {
        region: Region::Main,
    }
}

fn nav_forward() -> NavigateForward {
    NavigateForward {
        region: Region::Main,
    }
}

/// The default block the headless `FrontendSession` boots focused on — its
/// `current_focus(main)` / `focus_roots(main)` carry `block:journals` before any
/// navigation (verified by the failing first-check: the SUT starts here).
const JOURNALS_ID: &str = "block:journals";

/// The seeded focus oracle: a started `ReferenceState` whose initial navigation
/// focus is `block:journals`, matching the SUT's boot focus (the headless session
/// starts focused on journals, exactly as production `StartApp` does). Without
/// this the very first `check_invariants` sees the SUT's boot `current_focus`
/// (journals) as a ghost the ref never navigated to. Each generated
/// `NavigateFocus::apply_to_ref` then advances the `navigation_history` /
/// `open_pins` the focus invariants read.
fn navigation_ref() -> ReferenceState {
    let journals = EntityUri::parse(JOURNALS_ID).expect("valid journals id");
    let mut state = build_started_ref(&BTreeSet::new());
    navigate(journals.clone()).apply_to_ref(&mut state);
    // Trim the back-stack to journals-only at cursor 0, MATCHING the headless SUT's
    // boot history. `build_started_ref` seeds a `c1` focus (for editor preconditions
    // unused by this nav slice), so without this the ref carries a `[None, c1,
    // journals]` history (cursor 2) — a deeper back-stack than the SUT's journals-only
    // boot. A `NavigateBack` would then walk the ref through `c1`/`None` entries the
    // SUT never had, diverging `current_focus`/`focus_roots` at the boundary. With the
    // back-stack trimmed, `can_go_back` is false at the seed (so back-nav only fires
    // after a `NavigateFocus` builds real history) and every back lands on a focus the
    // SUT also visited.
    let history = state
        .ui
        .tab
        .navigation_history
        .entry(Region::Main)
        .or_default();
    history.entries = vec![Some(journals)];
    history.cursor = 0;
    // Align the history-id counter with the SUT boot too (same `c1` root cause): the
    // headless session assigns journals `navigation_history.id = 1` (AUTOINCREMENT) at
    // boot, so the next pin gets id 2. `build_started_ref`'s `c1` seed bumped the ref's
    // `next_history_id` an extra time (journals got id 2, next is 3) — a lockstep
    // `PinBlock` would then predict id 3 while the SUT assigns 2, breaking `UnpinBlock`
    // (`close(3)` targets nothing). Reset the journals open-pin to id 1 and
    // `next_history_id` to 2 so the ids mirror the boot. (Verified by
    // `headless_unpin_block_probe` + `unpin_block_lockstep_stays_green`.)
    if let Some(pins) = state.ui.user.open_pins.get_mut(&Region::Main) {
        for p in pins.iter_mut() {
            p.history_id = 1;
        }
    }
    state.ui.tab.next_history_id = 2;
    state
}

/// Invariants that MUST run each tick — the non-vacuity guard so "green" means
/// "ran over real focus data", not "deselected everything".
const REQUIRED_INVARIANTS: &[&str] = &["inv-navigation-focus", "inv-focus-roots"];

/// The navigation alphabet — a **heterogeneous mixed alphabet** over a single
/// composed `CapMap`: `NavigateFocus` (drawn from the two pinned page-doc ids — a
/// state-derived candidate set, so no nonexistent-id SUT/oracle divergence),
/// `NavigateHome`, and `PinBlock` (always the stable [`PINNABLE_ID`] Text block into
/// the right sidebar). `PinBlock` is emitted directly rather than through its
/// `weighted_generator`: this `RefFocus`-only slice intentionally does not seed
/// `block_state`, so the factory's `main_editable_descendants` candidate set is empty
/// — but `apply_to_ref` is total (it only pushes `open_pins`) and `inv-focus-roots`
/// is a pure id-set compare, and the block *is* a real pinnable Text block on the SUT
/// (the `:ID:` seed), so the lockstep stays faithful. Verified make-or-break:
/// `headless_pin_block_right_sidebar_probe`; teeth: `pin_block_lockstep_stays_green`.
///
/// `NavigateBack`/`NavigateForward` are gated by [`NavMachine::preconditions`] via each
/// transition's own `can_go_back`/`can_go_forward`. They dispatch the production
/// `navigation.go_back`/`go_forward` ops headlessly (`SutNavHistoryDrive`); parity is
/// proven by `headless_back_forward_focus_parity_probe`. Two seed/ref bugs had to be
/// fixed before they were safe in the generated alphabet: (1) the boot nav-history-depth
/// mismatch ([`navigation_ref`] trims its phantom `c1`/`None` back-stack to journals-only
/// at cursor 0), and (2) **`navigate_home`'s missing idempotency guard** — `go_home` was
/// pushing a `None` history entry on EVERY call, so `NavigateHome`×N → N phantom home
/// entries that `NavigateBack` walked back through, while production `focus(None)` is
/// idempotent when already home (`headless_go_home_idempotency_probe`: go_home×3 → 1 NULL
/// row). Both fixed; this alphabet is robust at `PROPTEST_CASES=40`.
///
/// `UnpinBlock` is layered on in [`NavMachine::transitions`] (not here) because it is
/// **state-dependent** — its `history_id` is drawn from the pins the ref currently holds
/// (see [`unpin_candidates`]), so the id always matches one the SUT assigned (the risk-C
/// history-id alignment, kept exact by [`navigation_ref`]'s `next_history_id` reset).
fn aggregate() -> BoxedStrategy<NavTransition> {
    prop_oneof![
        proptest::sample::select(nav_doc_ids())
            .prop_map(|block_id| NavTransition::NavigateFocus(navigate(block_id))),
        Just(NavTransition::NavigateHome(navigate_home())),
        Just(NavTransition::PinBlock(pin(
            EntityUri::parse(PINNABLE_ID).expect("pinnable id")
        ))),
        Just(NavTransition::NavigateBack(nav_back())),
        Just(NavTransition::NavigateForward(nav_forward())),
    ]
    .boxed()
}

#[derive(Clone, Debug)]
enum NavTransition {
    NavigateFocus(NavigateFocus),
    NavigateHome(NavigateHome),
    PinBlock(PinBlock),
    NavigateBack(NavigateBack),
    NavigateForward(NavigateForward),
    UnpinBlock(UnpinBlock),
}

/// The right-sidebar pin `history_id`s currently open — the candidate set for
/// `UnpinBlock` (drawn from `open_pins[RightSidebar]`, never the Main focus pin, so
/// `close(history_id)` only ever removes a sidebar pin, not the current focus row).
fn unpin_candidates(state: &ReferenceState) -> Vec<i64> {
    state
        .ui
        .user
        .open_pins
        .get(&Region::RightSidebar)
        .map(|pins| pins.iter().map(|p| p.history_id).collect())
        .unwrap_or_default()
}

struct NavMachine;

impl ReferenceStateMachine for NavMachine {
    type State = ReferenceState;
    type Transition = NavTransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(navigation_ref()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        // `UnpinBlock` is state-dependent: its `history_id` is drawn from the pins the
        // ref currently holds (so the id is always one the SUT also assigned — the
        // risk-C alignment). When no sidebar pin is open it is simply not offered.
        let candidates = unpin_candidates(state);
        if candidates.is_empty() {
            aggregate()
        } else {
            prop_oneof![
                4 => aggregate(),
                1 => proptest::sample::select(candidates)
                    .prop_map(|history_id| NavTransition::UnpinBlock(UnpinBlock { history_id })),
            ]
            .boxed()
        }
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        // `NavigateFocus`/`NavigateHome`/`PinBlock` are always applicable (total
        // `apply_to_ref`; PinBlock's own `block_state` precondition is intentionally
        // not gated here — see `aggregate`). `NavigateBack`/`NavigateForward` are gated
        // by their own `can_go_back`/`can_go_forward` — the ref's trimmed back-stack +
        // idempotent `go_home` (see `navigation_ref` and `navigate_home`) match the SUT.
        // `UnpinBlock` is shrink-gated: the `history_id` must STILL be an open pin (a
        // shrink that drops the creating `PinBlock` invalidates it).
        match transition {
            NavTransition::NavigateFocus(_)
            | NavTransition::NavigateHome(_)
            | NavTransition::PinBlock(_) => true,
            NavTransition::NavigateBack(t) => t.preconditions(state).is_good(),
            NavTransition::NavigateForward(t) => t.preconditions(state).is_good(),
            NavTransition::UnpinBlock(u) => state
                .ui
                .user
                .open_pins
                .values()
                .flatten()
                .any(|p| p.history_id == u.history_id),
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            NavTransition::NavigateFocus(t) => t.apply_to_ref(&mut state),
            NavTransition::NavigateHome(t) => t.apply_to_ref(&mut state),
            NavTransition::PinBlock(t) => t.apply_to_ref(&mut state),
            NavTransition::NavigateBack(t) => t.apply_to_ref(&mut state),
            NavTransition::NavigateForward(t) => t.apply_to_ref(&mut state),
            NavTransition::UnpinBlock(t) => t.apply_to_ref(&mut state),
        }
        state
    }
}

/// Boot the windowless navigation SUT and wire its focus caps. Shared by the
/// [`ComposedSlice`] `build` and the teeth (the latter pass their own runtime).
fn build_nav_sut(rt: &tokio::runtime::Runtime) -> (CapMap, Arc<HeadlessFrontendComponent>) {
    rt.block_on(boot_nav_caps())
}

async fn boot_nav_caps() -> (CapMap, Arc<HeadlessFrontendComponent>) {
    let component = Arc::new(
        HeadlessFrontendComponent::new(&doc_org_files(), std::time::Duration::from_millis(300))
            .await,
    );
    let caps = frontend_navigation_wide(component.clone());
    (caps, component)
}

/// Run the catalog against a **`RefFocus`-only** ref `CapMap`, so only the focus
/// invariants select. Mirrors `run_with_seeded_ref`'s off-thread drop of the
/// `Arc<ReferenceState>` (it owns a tokio `Runtime`; dropping it on the async
/// executor panics).
async fn run_focus_only(
    catalog: &[Box<dyn CapInvariant>],
    sut: &CapMap,
    ref_state: ReferenceState,
) -> RunReport {
    let arc = Arc::new(ref_state);
    let mut ref_caps = CapMap::new();
    ref_caps.insert(arc.clone() as Arc<dyn RefFocus>);
    let report = run_selected(catalog, sut, &ref_caps).await;
    drop(ref_caps);
    std::thread::spawn(move || drop(arc))
        .join()
        .expect("drop ReferenceState (owns a tokio Runtime) off the async executor");
    report
}

/// The navigation slice as a thin [`ComposedSlice`] — the heterogeneous focus
/// alphabet (`NavTransition`/`NavMachine`), a real headless `FrontendSession` seed,
/// and a **focus-only** check: it overrides [`run_report`](ComposedSlice::run_report)
/// to run the catalog against a `RefFocus`-only ref `CapMap` (only the two focus
/// invariants select) over the raw oracle. The navigation transitions mint no blocks,
/// so the harness's generic reconcile is a clean no-op (the default `align_ids`).
struct FrontendNavigation;

impl ComposedSlice for FrontendNavigation {
    type Transition = NavTransition;
    type Machine = NavMachine;
    type Handle = Arc<HeadlessFrontendComponent>;
    const REQUIRED_INVARIANTS: &'static [&'static str] = REQUIRED_INVARIANTS;
    const SETTLE: Duration = Duration::ZERO;
    const MULTI_THREAD: bool = true;

    async fn build(
        _: &IdResolver,
        _: &ReferenceState,
    ) -> (CapMap, Arc<HeadlessFrontendComponent>, BTreeSet<EntityUri>) {
        let (caps, component) = boot_nav_caps().await;
        (caps, component, BTreeSet::new())
    }

    async fn apply_transition(
        transition: &NavTransition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
    ) {
        match transition {
            NavTransition::NavigateFocus(t) => t.apply_to_sut(ref_state, caps).await,
            NavTransition::NavigateHome(t) => t.apply_to_sut(ref_state, caps).await,
            NavTransition::PinBlock(t) => t.apply_to_sut(ref_state, caps).await,
            NavTransition::NavigateBack(t) => t.apply_to_sut(ref_state, caps).await,
            NavTransition::NavigateForward(t) => t.apply_to_sut(ref_state, caps).await,
            NavTransition::UnpinBlock(t) => t.apply_to_sut(ref_state, caps).await,
        }
    }

    /// Focus-only check: a `RefFocus`-only ref `CapMap` over the raw oracle, so only
    /// the focus invariants select (the slice's payoff). No reconcile/scaffold needed.
    async fn run_report(
        caps: &CapMap,
        _: &IdResolver,
        _: &BTreeSet<EntityUri>,
        ref_state: &ReferenceState,
    ) -> RunReport {
        run_focus_only(&composed_invariant_catalog(), caps, ref_state.clone()).await
    }
}

prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        // A real Turso `FrontendSession` boots per case (`init_test`), so keep the
        // case/step counts modest — this proves the rebind mechanic, not breadth.
        cases: 12,
        max_shrink_iters: 64,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn frontend_navigation_pbt(sequential 1..6 => ComposedSut<FrontendNavigation>);
}

// ─────────────────────────────────────────────────────────────────
// Teeth — prove the navigation write path is real (the focus invariants
// catch a divergence, and a faithful focus stays green), independent of
// the generated run above.
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod teeth {
    use super::*;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// Positive: navigating focus to `ref-doc-1` on BOTH the SUT and the oracle in
    /// lockstep keeps the focus invariants green — the faithful navigation write
    /// path (and non-vacuous: both focus invariants must have run).
    #[test]
    fn navigate_in_lockstep_stays_green() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let mut ref_state = navigation_ref();
        let t = navigate(EntityUri::parse(DOC1_ID).unwrap());
        t.apply_to_ref(&mut ref_state);
        rt.block_on(t.apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        assert!(
            report.failures().is_empty(),
            "lockstep NavigateFocus should be green: {:?}",
            report.failures()
        );
        for id in REQUIRED_INVARIANTS {
            assert!(
                report.ran_ids().contains(id),
                "non-vacuity: {id} must have run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Teeth: navigating focus on the SUT but NOT the oracle (oracle frozen at the
    /// journals-focused seed) makes the SUT's `current_focus` / `focus_roots` matviews
    /// hold `ref-doc-1` while the oracle still holds journals — both focus invariants
    /// MUST `Fail` (not `Skipped`). Proves the op actually moved the matviews and
    /// the invariants have teeth. The `focus_roots` mirror==matview wiring
    /// (`live_focus_root_rows` reads the matview) guarantees a real `Fail`, not a
    /// CDC-lag `Skipped` (V4).
    #[test]
    fn sut_only_navigate_is_caught_by_focus_invariants() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let ref_state = navigation_ref(); // oracle frozen at the journals-focused seed
        let t = navigate(EntityUri::parse(DOC1_ID).unwrap());
        rt.block_on(t.apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        let failures = report.failures();
        for id in REQUIRED_INVARIANTS {
            assert!(
                failures.iter().any(|(fid, _)| fid == id),
                "SUT-only NavigateFocus must trip {id} with a Fail; failures were {failures:?}, \
                 ran={:?}",
                report.ran_ids()
            );
        }
    }

    /// Positive (also the increment-2 H4 settle probe): `go_home` on BOTH the SUT and
    /// the oracle in lockstep, from the `block:journals`-focused seed, keeps the focus
    /// invariants green. This proves the production `navigation.go_home` focus-*clear*
    /// settles to a stable empty `current_focus`/`focus_roots` headlessly — the settle
    /// path INC 1's focus-*set* never exercised — matching the ref's cleared state.
    /// Non-vacuous (both focus invariants must have run).
    #[test]
    fn navigate_home_lockstep_stays_green() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let mut ref_state = navigation_ref();
        let t = navigate_home();
        t.apply_to_ref(&mut ref_state);
        rt.block_on(t.apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        assert!(
            report.failures().is_empty(),
            "lockstep NavigateHome should be green: {:?}",
            report.failures()
        );
        for id in REQUIRED_INVARIANTS {
            assert!(
                report.ran_ids().contains(id),
                "non-vacuity: {id} must have run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Teeth: `go_home` on the SUT but NOT the oracle. The `navigation_ref()` seed
    /// boots focused on `block:journals` WITH journals as an open-pin focus root, so
    /// the SUT's `go_home` clears its `current_focus` (→ None) AND its `focus_roots`
    /// (→ empty) while the frozen oracle still holds journals — so BOTH focus
    /// invariants MUST `Fail` (not `Skipped`). This also deterministically dispatches
    /// `NavigateHome` through the real `apply_to_sut` path (the H5 coverage guarantee,
    /// robust to nextest's per-test process isolation where a cross-test counter can't
    /// observe the randomized run).
    #[test]
    fn sut_only_navigate_home_is_caught_by_focus_invariants() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let ref_state = navigation_ref(); // oracle frozen at the journals-focused seed
        let t = navigate_home();
        rt.block_on(t.apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        let failures = report.failures();
        for id in REQUIRED_INVARIANTS {
            assert!(
                failures.iter().any(|(fid, _)| fid == id),
                "SUT-only NavigateHome must trip {id} with a Fail; failures were {failures:?}, \
                 ran={:?}",
                report.ran_ids()
            );
        }
    }

    /// Positive (C1 PinBlock increment): `focus_pin(RightSidebar, ref-block-0)` on
    /// BOTH the SUT and the oracle in lockstep keeps the focus invariants green. The
    /// production `navigation.focus_pin` op, dispatched through the windowless session
    /// by `SutNavHistoryDrive::pin_block`, populates `focus_roots(right_sidebar)` with
    /// the pinned block — matching the ref's `open_pins` — while `current_focus(main)`
    /// stays journals (pins don't move the cursor). Non-vacuous (both focus invariants
    /// must have run). Verified make-or-break: `headless_pin_block_right_sidebar_probe`.
    #[test]
    fn pin_block_lockstep_stays_green() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let mut ref_state = navigation_ref();
        let t = pin(EntityUri::parse(PINNABLE_ID).unwrap());
        t.apply_to_ref(&mut ref_state);
        rt.block_on(t.apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        assert!(
            report.failures().is_empty(),
            "lockstep PinBlock should be green: {:?}",
            report.failures()
        );
        for id in REQUIRED_INVARIANTS {
            assert!(
                report.ran_ids().contains(id),
                "non-vacuity: {id} must have run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Teeth: pinning on the SUT but NOT the oracle leaves the SUT's
    /// `focus_roots(right_sidebar)` holding `ref-block-0` while the oracle's
    /// `open_pins` stays empty — so `inv-focus-roots` MUST `Fail` (not `Skipped`),
    /// proving the `focus_pin` op actually moved the matview and the invariant has
    /// teeth. `inv-navigation-focus` stays green (pins don't touch `current_focus`),
    /// so this asserts the focus-roots invariant specifically.
    #[test]
    fn sut_only_pin_block_is_caught_by_focus_roots() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let ref_state = navigation_ref(); // oracle has NO pin
        let t = pin(EntityUri::parse(PINNABLE_ID).unwrap());
        rt.block_on(t.apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        let failures = report.failures();
        assert!(
            failures.iter().any(|(fid, _)| fid == &"inv-focus-roots"),
            "SUT-only PinBlock must trip inv-focus-roots with a Fail; failures were \
             {failures:?}, ran={:?}",
            report.ran_ids()
        );
    }

    /// Positive (C1 NavigateBack/Forward increment): build nav history
    /// journals(boot)→d0→d1 in lockstep, then `go_back` then `go_forward` — each on
    /// BOTH the SUT and the oracle — keeps the focus invariants green. Proves the
    /// production `navigation.go_back`/`go_forward` ops, dispatched headlessly by
    /// `SutNavHistoryDrive`, move `current_focus(main)` to track the ref's
    /// `navigation_history.cursor` (the historically-doubted parity, proven by
    /// `headless_back_forward_focus_parity_probe`). Non-vacuous.
    #[test]
    fn navigate_back_forward_roundtrip_lockstep_stays_green() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let mut ref_state = navigation_ref();

        // Drive a transition on BOTH the oracle and the SUT in lockstep.
        macro_rules! lockstep {
            ($t:expr) => {{
                let t = $t;
                t.apply_to_ref(&mut ref_state);
                rt.block_on(t.apply_to_sut(&ref_state, &mut caps));
            }};
        }
        lockstep!(navigate(EntityUri::parse(DOC0_ID).unwrap()));
        lockstep!(navigate(EntityUri::parse(DOC1_ID).unwrap()));
        lockstep!(nav_back());
        lockstep!(nav_forward());

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        assert!(
            report.failures().is_empty(),
            "lockstep NavigateBack/Forward roundtrip should be green: {:?}",
            report.failures()
        );
        for id in REQUIRED_INVARIANTS {
            assert!(
                report.ran_ids().contains(id),
                "non-vacuity: {id} must have run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Teeth: build history to d1 in lockstep, then `go_back` on the SUT but NOT the
    /// oracle — the SUT's `current_focus(main)` moves back to d0 while the frozen
    /// oracle still holds d1, so `inv-navigation-focus` MUST `Fail` (not `Skipped`).
    /// Proves the `go_back` op actually moved the matview and the invariant bites.
    #[test]
    fn sut_only_navigate_back_is_caught_by_focus() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let mut ref_state = navigation_ref();
        // Build history to d1 in lockstep (both oracle and SUT focused on d1).
        for id in [DOC0_ID, DOC1_ID] {
            let t = navigate(EntityUri::parse(id).unwrap());
            t.apply_to_ref(&mut ref_state);
            rt.block_on(t.apply_to_sut(&ref_state, &mut caps));
        }
        // SUT-only go_back: the oracle stays at d1.
        rt.block_on(nav_back().apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(fid, _)| fid == &"inv-navigation-focus"),
            "SUT-only NavigateBack must trip inv-navigation-focus with a Fail; failures \
             were {failures:?}, ran={:?}",
            report.ran_ids()
        );
    }

    /// The `history_id` the ref assigned the right-sidebar pin of [`PINNABLE_ID`].
    fn ref_pin_history_id(ref_state: &ReferenceState) -> i64 {
        ref_state.ui.user.open_pins[&Region::RightSidebar]
            .iter()
            .find(|p| p.block_id.as_ref().map(|b| b.as_str()) == Some(PINNABLE_ID))
            .expect("ref pinned ref-block-0 into the right sidebar")
            .history_id
    }

    /// Positive (C1 UnpinBlock — the pin→unpin cycle): pin `ref-block-0` into the
    /// right sidebar lockstep, then `UnpinBlock` it by the ref-predicted `history_id`
    /// — which MUST equal the SUT's `navigation_history.id` AUTOINCREMENT (2: journals
    /// is 1, the pin is 2). That id alignment is the "risk C" the C1 research flagged;
    /// it is asserted here and only holds because [`navigation_ref`] resets
    /// `next_history_id` to mirror the boot. Both sides clear; `inv-focus-roots` stays
    /// green (and non-vacuous). Make-or-break: `headless_unpin_block_probe`.
    #[test]
    fn unpin_block_lockstep_stays_green() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let mut ref_state = navigation_ref();
        let p = pin(EntityUri::parse(PINNABLE_ID).unwrap());
        p.apply_to_ref(&mut ref_state);
        rt.block_on(p.apply_to_sut(&ref_state, &mut caps));

        let hid = ref_pin_history_id(&ref_state);
        assert_eq!(
            hid, 2,
            "ref must assign the pin history_id=2, matching the SUT AUTOINCREMENT \
             (journals=1, pin=2) — else UnpinBlock's close(history_id) targets the wrong row"
        );

        let u = UnpinBlock { history_id: hid };
        u.apply_to_ref(&mut ref_state);
        rt.block_on(u.apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        assert!(
            report.failures().is_empty(),
            "lockstep PinBlock→UnpinBlock should be green: {:?}",
            report.failures()
        );
        assert!(
            report.ran_ids().contains(&"inv-focus-roots"),
            "non-vacuity: inv-focus-roots must have run (ran: {:?})",
            report.ran_ids()
        );
    }

    /// Teeth: pin lockstep, then `UnpinBlock` on the SUT but NOT the oracle — the SUT
    /// clears `focus_roots(right_sidebar)` while the oracle still holds the pin, so
    /// `inv-focus-roots` MUST `Fail`. Proves `close(history_id)` actually removed the
    /// matview row (and that the id targeted the right one).
    #[test]
    fn sut_only_unpin_block_is_caught_by_focus_roots() {
        let rt = rt();
        let (mut caps, _component) = build_nav_sut(&rt);
        let mut ref_state = navigation_ref();
        let p = pin(EntityUri::parse(PINNABLE_ID).unwrap());
        p.apply_to_ref(&mut ref_state);
        rt.block_on(p.apply_to_sut(&ref_state, &mut caps));
        let hid = ref_pin_history_id(&ref_state);

        // SUT-only unpin: the oracle keeps the pin.
        rt.block_on(UnpinBlock { history_id: hid }.apply_to_sut(&ref_state, &mut caps));

        let report = rt.block_on(run_focus_only(
            &composed_invariant_catalog(),
            &caps,
            ref_state,
        ));
        let failures = report.failures();
        assert!(
            failures.iter().any(|(fid, _)| fid == &"inv-focus-roots"),
            "SUT-only UnpinBlock must trip inv-focus-roots with a Fail; failures were \
             {failures:?}, ran={:?}",
            report.ran_ids()
        );
    }
}
