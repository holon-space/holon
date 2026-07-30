//! **F2 E3 Phase C2.0 — the generic reconcile+settle loop on the SUT-swap
//! target.**
//!
//! Drive the production *structural* alphabet (Split/Join/Indent/Outdent)
//! through a composed `CapMap` hosted on the REAL [`HeadlessFrontendComponent`]
//! (the windowless production `FrontendSession` over Turso that will replace
//! `E2ESut` as the `general_e2e_pbt` SUT), checked by the shared
//! composed-invariant catalog against the live `ReferenceState` oracle.
//!
//! Unlike the `memory_slice` structural PBT — which keeps ids in lockstep with
//! a `MemoryBackend`'s `set_next_split_id` hint — the headless component runs
//! the real Turso `split_block` op, which mints a fresh **real** `uuid` per
//! split (not a hintable id). So this slice carries the spike's [`IdResolver`]
//! reconcile kernel: per tick, diff the SUT `block_raw` id-set before/after the
//! transition, pair the one freshly-minted real id against the oracle's one
//! freshly-minted synthetic `block::split-N`, and accumulate the `synthetic →
//! real` map. At check time the oracle is `with_resolved_doc_uris`-remapped
//! into SUT id space. This is the FIRST reconcile-based structural SUT on the
//! live (non-spike) component — the spike proved the kernel over a bare engine;
//! here it runs over the full production boot.
//!
//! **Scaffold seed-injection.** The full production boot leaves ~13 scaffold
//! blocks (`__default__`, the layout/sidebar tree + their PRQL query children,
//! `journals`, the booted org doc) that the spike's bare engine never has. The
//! id-set-exact `compare_block_subset` would count them on the SUT side, so
//! each booted id is injected into the oracle as
//! `block_documents[id]=no_parent` — joining `seed_block_ids()` and filtering
//! out of the SUT snapshot, reducing the comparison to the working
//! `{parent,c1,c2}(+split)` tree on both sides. (Headless analog of
//! E1 `SutOrgRead` seeding the oracle from booted blocks; proven by
//! `components::tests::headless_structural_seed_and_reconcile_probe`.)
//!
//! Scope = the 4 reparenting structural transitions. MoveUp/MoveDown are gated
//! out of generation for the same sibling-*order* reason the `memory_slice`
//! documents (no invariant compares child order; the store's order can drift
//! from the oracle's canonical `sequence()`+id order). The editor/focus caps
//! are NOT wired: the minimal capmap hosts only `SutBackend` +
//! `SutBlockTreeWrite`, so the focus/editor invariants deselect and a focused
//! oracle (needed so the generators have editable candidates) never false-REDs.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_api::Block;
use holon_api::EntityUri;
use holon_api::PAGE_TAG;
use holon_api::Region;
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::ComponentSet;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutEditorMirrorRead;
use holon_pbt_core::capabilities::SutFocus;
use holon_pbt_core::capabilities::SutQueryResults;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;
use holon_pbt_core::types::CycleTarget;
use holon_pbt_core::weighted_arm;
use proptest::prelude::Just;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Strategy;
use proptest::strategy::Union;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::prop_state_machine;
use validated::Validated;

use crate::pbt::composed::builder::compose_sut_seeded;
use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::composed::harness::ComposedSlice;
use crate::pbt::composed::harness::ComposedSut;
use crate::pbt::composed::harness::inject_scaffold_seed;
use crate::pbt::composed::harness::sut_ids;
use crate::pbt::composed::seed_primitives::C1;
use crate::pbt::composed::seed_primitives::C2;
use crate::pbt::composed::seed_primitives::PARENT;
use crate::pbt::composed::seed_primitives::fixed_ids;
use crate::pbt::composed::subsystem_seed::build_started_ref;
use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
// THE SWAP machinery, relocated to the `pbt`-gated `composed::wide_e2e` (single source
// of truth) so the `tests/` integration test can drive it; the lib slices/teeth here
// consume it: page_root/SETTLE/WIDE_TREE_ORG/structural_ref{,_wired}/wide_ref/
// boot_and_seed_wide/full_headless_cap_set/wide_e2e_ref/WideE2E{,Machine}.
use crate::pbt::composed::wide_e2e::{
    SETTLE, WIDE_TREE_ORG, boot_and_seed_wide, folder_journal_page, frontend_wired, page_root,
    seed_folder_companion, seed_folder_companion_subdir, structural_ref, subdir_journal_page,
    wide_e2e_ref, wide_ref,
};
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::is_synthetic_ref_id;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::op_write_cap::OpDispatchWriter;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::sql_slice::SqlProjectionComponent;
use crate::pbt::transitions::CreateDocument;
use crate::pbt::transitions::DeleteBackward;
use crate::pbt::transitions::DeleteDocument;
use crate::pbt::transitions::E2ETransition;
use crate::pbt::transitions::FocusEditableText;
use crate::pbt::transitions::Indent;
use crate::pbt::transitions::JoinBlock;
use crate::pbt::transitions::NavigateBack;
use crate::pbt::transitions::NavigateFocus;
use crate::pbt::transitions::NavigateForward;
use crate::pbt::transitions::NavigateHome;
use crate::pbt::transitions::Nothing;
use crate::pbt::transitions::Outdent;
use crate::pbt::transitions::PinBlock;
use crate::pbt::transitions::SimulateRestart;
use crate::pbt::transitions::SplitBlock;
use crate::pbt::transitions::ToggleState;
use crate::pbt::transitions::TypeChars;

/// The structural slice's transition alphabet — `Split` (the id-minting
/// transition that drives the reconcile loop, the point of C2.0) + `Join`, each
/// binding only `S: SutBlockTreeWrite`, so the aggregate dispatches against a
/// composed `CapMap` without needing `SutHandle`.
///
/// **Why only Split + Join over leaf siblings** (the working blocks are direct
/// children of a page root, and stay leaves because no transition nests them):
/// - `Outdent` (and any split of a `no_parent` block) moves a block to the top
///   level, where the production Turso `split_block`/`outdent` op writes a
///   literal `NULL` `parent_id` (whereas the bootstrap writes the
///   `sentinel:no_parent` string), which `Block::try_from` rejects when reading
///   `block_raw`. `MemoryBackend` tolerates it, so the `memory_slice` never hit
///   it. **Real store/bootstrap inconsistency to FILE.** Avoided here: the page
///   root keeps the working blocks off the top level and is never itself split.
/// - `Indent` nests a block under its previous sibling, turning a leaf into a
///   parent-with-children. Splitting a block *with children* then diverges:
///   Turso makes the new block a **child** of the split block; the oracle makes
///   it a **sibling**. **Second real divergence to FILE.** Avoided here by
///   excluding `Indent` so every candidate stays a leaf.
///
/// Both are documented follow-on investigations (like the `go_back` smell), not
/// blockers for the reconcile-loop keystone.
#[derive(Clone, Debug)]
enum StructTransition {
    SplitBlock(SplitBlock),
    JoinBlock(JoinBlock),
    /// No-op fallback so `transitions()` never returns an empty strategy: a
    /// Join sequence can collapse the focus root's editable descendants to
    /// none (then no structural transition applies). Mirrors the spike's
    /// `SqlTransition::Nothing`. The invariants still run this tick (the
    /// catalog re-checks every step), so it costs nothing but robustness.
    Nothing,
}

/// The seeded oracle: the started `parent/c1/c2` blocks (NON-seed → compared
/// every tick) re-rooted as **leaf siblings** directly under a seed
/// `page_root`, with focus on the page root. The page root is the focus
/// container so its children are the `main_editable_descendants` candidates,
/// but it is itself a page (excluded from candidates) and a seed (excluded from
/// the comparison) — so it is never split and its page-ness never compared.
/// With the working blocks as leaf siblings and no nesting transition (Indent
/// excluded), every candidate stays a leaf: `Split` lands a new leaf sibling
/// under the page (a real id, never `no_parent`), `Join` merges a leaf into its
/// previous sibling. No UI subsystem is wired, so `build_started_ref`
/// seeds no editor; the focus nav is the only UI state, and the minimal capmap
/// hosts no focus caps so it never false-REDs.
/// Invariants that MUST run each tick — the non-vacuity guard so "green" means
/// "ran over real data", not "deselected everything".
const REQUIRED_INVARIANTS: &[&str] = &[
    "inv-no-orphan-blocks",
    "inv-no-parent-cycles",
    "inv-blocks-match-ref/block_raw",
    "inv-block-parent/block_raw",
];

/// One arm per structural transition via the shared `weighted_arm` over the
/// SAME generic `TransitionFactory<ReferenceState>` impls the wide PBT uses.
fn aggregate(state: &ReferenceState) -> BoxedStrategy<StructTransition> {
    let mut arms: Vec<(u32, BoxedStrategy<StructTransition>)> = vec![];
    macro_rules! arm {
        ($ty:ty, $variant:path) => {
            if let Validated::Good(Some(a)) =
                weighted_arm::<_, $ty, StructTransition>(state, 1, $variant)
            {
                arms.push(a);
            }
        };
    }
    arm!(SplitBlock, StructTransition::SplitBlock);
    arm!(JoinBlock, StructTransition::JoinBlock);
    if arms.is_empty() {
        // No structural transition applies in this state — fall back to the no-op so
        // proptest can still step (and the invariants re-check) rather than panic.
        return Just(StructTransition::Nothing).boxed();
    }
    Union::new_weighted(arms).boxed()
}

struct StructMachine;

impl ReferenceStateMachine for StructMachine {
    type State = ReferenceState;
    type Transition = StructTransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(structural_ref()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        aggregate(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        // Re-gate on production preconditions during shrink replay: a reparenting
        // transition valid when generated can become invalid after the shrinker
        // drops an earlier one. Delegating lets proptest reject the shrink instead
        // of applying it and panicking the oracle's `apply_to_ref`.
        match transition {
            StructTransition::SplitBlock(t) => t.preconditions(state).is_good(),
            StructTransition::JoinBlock(t) => t.preconditions(state).is_good(),
            StructTransition::Nothing => true,
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            StructTransition::SplitBlock(t) => t.apply_to_ref(&mut state),
            StructTransition::JoinBlock(t) => t.apply_to_ref(&mut state),
            StructTransition::Nothing => {}
        }
        state
    }
}

/// Boot the windowless production session, seed the page-rooted leaf-sibling
/// tree via the production create op, and build the minimal structural capmap
/// (`SutBackend` + the `resolver`-sharing writer). Returns the capmap plus the
/// booted scaffold ids (to seed-inject into the oracle). Shared by the
/// StateMachineTest `init_test` and the teeth so they exercise the exact same
/// SUT-swap target.
async fn boot_and_seed(resolver: &IdResolver) -> (CapMap, BTreeSet<EntityUri>) {
    let comp = Arc::new(
        HeadlessFrontendComponent::new(
            &[("doc0.org", "#+ID: ref-doc-0\n* Doc zero\n")],
            Duration::from_millis(300),
        )
        .await,
    );
    let engine = comp.engine();

    // Capture the booted scaffold ids (everything present BEFORE the working tree)
    // — these become the oracle's seed set so they filter out of the SUT-side
    // id comparison.
    let scaffold_ids: BTreeSet<EntityUri> = {
        let mut c = CapMap::new();
        c.insert(comp.clone() as Arc<dyn SutBackend>);
        sut_ids(&c)
            .await
            .into_iter()
            .filter(|id| !is_synthetic_ref_id(id))
            .collect()
    };

    // Seed the page-rooted tree: `page_root` (under `no_parent`) →
    // `parent`,`c1`,`c2` as LEAF SIBLINGS. The page root keeps candidates off
    // the top level (so `Split` never writes a `no_parent` block) and, being a
    // page, is never itself a split target. Filtered from the comparison by the
    // same seed-injection as the scaffold.
    let ids = fixed_ids();
    let seeder = SqlProjectionComponent::new(engine.clone());
    seeder
        .create_block(&page_root(), &EntityUri::no_parent(), "structural-page")
        .await;
    // The oracle models `structural-page` as a genuine page doc-root
    // (`set_page(true)` → tag `Page`); the raw `create` op above never derives
    // the Page tag for a `no_parent` root. Tag it through the production
    // element-wise `add_tag` op so the SUT's `is_page()` matches the oracle
    // (legal: the page-under-non-page guard exempts `no_parent` roots).
    seeder.add_tag(&page_root(), PAGE_TAG).await;
    seeder.create_block(&ids.parent, &page_root(), PARENT).await;
    seeder.create_block(&ids.c1, &page_root(), C1).await;
    seeder.create_block(&ids.c2, &page_root(), C2).await;
    tokio::time::sleep(SETTLE).await;

    let mut caps = CapMap::new();
    caps.insert(comp as Arc<dyn SutBackend>);
    caps.insert(
        Arc::new(OpDispatchWriter::with_resolver(engine, resolver.clone()))
            as Arc<dyn SutBlockTreeWrite>,
    );
    (caps, scaffold_ids)
}

/// The frontend structural slice as a thin [`ComposedSlice`] — the alphabet
/// (`StructTransition`/`StructMachine`), the seed (`boot_and_seed`), and the
/// per-tick dispatch. Everything else (the runtime, the `IdResolver` reconcile,
/// the scaffold-injection + catalog check) is the generic [`ComposedSut`]
/// harness's.
struct FrontendStructural;

impl ComposedSlice for FrontendStructural {
    type Transition = StructTransition;
    type Machine = StructMachine;
    type Handle = ();
    const REQUIRED_INVARIANTS: &'static [&'static str] = REQUIRED_INVARIANTS;
    const SETTLE: Duration = SETTLE;

    async fn build(resolver: &IdResolver, _: &ReferenceState) -> (CapMap, (), BTreeSet<EntityUri>) {
        let (caps, scaffold) = boot_and_seed(resolver).await;
        (caps, (), scaffold)
    }

    async fn apply_transition(
        transition: &StructTransition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
    ) {
        match transition {
            StructTransition::SplitBlock(t) => t.apply_to_sut(ref_state, caps).await,
            StructTransition::JoinBlock(t) => t.apply_to_sut(ref_state, caps).await,
            StructTransition::Nothing => {}
        }
    }
}

prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 24,
        max_shrink_iters: 200,
        failure_persistence: None,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn frontend_structural_pbt(sequential 1..10 => ComposedSut<FrontendStructural>);
}

// ═════════════════════════════════════════════════════════════════
// WIDE alphabet generator (`wide_aggregate`) — retained for the `teeth` tests
// below.
//
// The standalone wide-frontend swap PBT (`WideFrontend`/`WideMachine`/
// `frontend_wide_pbt`) that this generator once drove has been DELETED as a
// redundant frontend variant of the ONE PBT `general_e2e_composed_pbt`
// (`ComposedSut<WideE2E>`), whose subsystem-config draw already covers the full
// frontend cap set + catalog. `wide_aggregate` survives only because the
// lockstep `teeth` tests below still use it to build single-transition
// alphabets. ═════════════════════════════════════════════════════════════════

/// One arm per drivable wide transition, wrapped in the production
/// `E2ETransition` enum (vs `FrontendStructural`'s bespoke `StructTransition`).
/// Same `weighted_arm` over the same generic factories the wide PBT uses.
///
/// `NavigateFocus` joins the structural pair: it's total, mints no blocks (the
/// reconcile is a clean no-op), and its target is drawn by the production
/// generator from the oracle's focusable descendants — so the SUT and oracle
/// navigate in lockstep and the focus matviews stay aligned. This exercises the
/// focus/nav invariants DYNAMICALLY (multiple navigations across a sequence),
/// integrated with the block/org/viewmodel checks each tick — the integration
/// the swap needs, beyond the navigation slice's focus-only check.
/// `NavigateBack/Forward/Pin/Unpin` stay out (they need the nav-history-depth /
/// history-id-counter alignment the dedicated nav slice carries; folding those
/// into the full-catalog drive is a later increment).
fn wide_aggregate(state: &ReferenceState) -> BoxedStrategy<E2ETransition> {
    let mut arms: Vec<(u32, BoxedStrategy<E2ETransition>)> = vec![];
    macro_rules! arm {
        ($ty:ty, $variant:path) => {
            if let Validated::Good(Some(a)) =
                weighted_arm::<_, $ty, E2ETransition>(state, 1, $variant)
            {
                arms.push(a);
            }
        };
    }
    arm!(SplitBlock, E2ETransition::SplitBlock);
    arm!(JoinBlock, E2ETransition::JoinBlock);
    // Indent/Outdent (pure moves — mint no ids → clean reconcile). They were
    // excluded for 2 filed "Turso smells", but EMPIRICAL reproduce over the
    // composed `full_headless` path shows BOTH are stale here (deterministic
    // teeth `wide_indent_outdent_roundtrip_lockstep`
    // + `wide_indent_then_split_parent_lockstep`, plus the random sweep): #1
    //   (top-level NULL
    // parent_id) never fires — the page-rooted tree outdents only to the real page
    // block, and the composed reader tolerates NULL anyway; #2
    // (split-of-a-block-with-children → Loro positional child-vs-sibling) does
    // NOT reproduce — the Loro-authority→Turso path places the new block as a
    // sibling correctly. So they're simply un-blocked, no prod fix needed.
    arm!(Indent, E2ETransition::Indent);
    arm!(Outdent, E2ETransition::Outdent);
    arm!(NavigateFocus, E2ETransition::NavigateFocus);
    // `ToggleState` self-gates via its render/focus-based generator: it only
    // proposes candidates when the focused region root is an
    // interactively-rendered text block, so it fires only after a
    // `NavigateFocus` lands focus on a text child (parent/c1/c2).
    // A pure property write (`set_field task_state`) — mints no blocks, reconcile
    // no-op.
    arm!(ToggleState, E2ETransition::ToggleState);
    // Editor arms (#2 — the combined "one PBT"). `FocusEditableText` opens an
    // editor on a focusable text child (self-gates: only when no editor is
    // active + a text block is focusable); `TypeChars`/`DeleteBackward` then
    // drive keystrokes (self-gate on an active editor). With no `MoveCursor`,
    // the caret stays at end-of-text so backspace never joins (no block
    // removal) — pure content edits, no reconcile-removal needed.
    // The editor↔structural interplay (Split/NavigateFocus while editor open) is
    // exercised here; any caret/focus-after-structural divergence is the narrow
    // #3 piece to add.
    arm!(FocusEditableText, E2ETransition::FocusEditableText);
    arm!(TypeChars, E2ETransition::TypeChars);
    arm!(DeleteBackward, E2ETransition::DeleteBackward);
    // Seam-rebuild SR-1: `CreateDocument` mints a new doc (the production
    // `SutAppLifecycle::create_document` writes an empty org file; the watcher
    // mints the doc block). The oracle's synthetic `block:ref-doc-N` is paired
    // to the minted real id by the harness's per-tick reconcile
    // (doc-uri-minting generalization) — the doc-uri case the old E2ESut
    // `block_tree_post_action` CreateDocument arm handled.
    arm!(CreateDocument, E2ETransition::CreateDocument);
    // Inverse of SR-1: `DeleteDocument` removes a `CreateDocument`-minted org file
    // via the production `FileSystem::remove` seam (the watcher observes the
    // deletion). Its generator self-gates on a synthetic `doc_<n>.org`
    // existing, so it only fires after a create landed.
    arm!(DeleteDocument, E2ETransition::DeleteDocument);
    // Nav-history transitions folded from the nav slice (toward deleting it). The
    // wide boot's nav-history is aligned in `structural_ref_wired`
    // ([journals#1, page#2], next=3), and the probe proved the
    // structural/editor/doc transitions write NO nav rows, so the AUTOINCREMENT
    // counter stays in lockstep. `NavigateHome` self-gates (idempotent when already
    // home); `PinBlock` draws a real pinnable text child via its weighted
    // generator (the wide oracle seeds `block_state`, unlike the RefFocus-only
    // nav slice); `NavigateBack/Forward` self-gate via their `can_go_back`/
    // `can_go_forward` preconditions over the aligned stack. `UnpinBlock`
    // is layered in `WideMachine::transitions` (state-dependent — its `history_id`
    // is drawn from the pins the oracle currently holds, so it always matches a
    // SUT-assigned id).
    arm!(NavigateHome, E2ETransition::NavigateHome);
    // `PinBlock` targets the FIXED stable seed block `c1` (Text, non-page,
    // focusable — always passes preconditions, always present). NOT the
    // weighted generator: that draws from Main's editable descendants, which
    // after a `SplitBlock` includes the synthetic `block::split-N`.
    // `SutNavHistoryDrive::pin_block` does NOT resolve oracle→real ids (only
    // the `OpDispatchWriter` block-tree path does), so pinning a synthetic id pins
    // a GHOST on the SUT (`focus_roots(right_sidebar)` then diverges, since
    // pins persist). A stable target needs no resolution. (Mirrors the nav
    // slice's fixed `PINNABLE_ID`.)
    arms.push((
        2,
        Just(E2ETransition::PinBlock(PinBlock {
            region: Region::RightSidebar,
            block_id: fixed_ids().c1,
        }))
        .boxed(),
    ));
    arm!(NavigateBack, E2ETransition::NavigateBack);
    arm!(NavigateForward, E2ETransition::NavigateForward);
    // Lifecycle: `SimulateRestart` re-triggers the FileSyncController watcher
    // (file-touch), re-parsing the org tree. Blocks are preserved (`:ID:`
    // drawers make re-parse id-stable), so `apply_to_ref` is a no-op and the
    // reconcile is clean. `SutAppLifecycle::simulate_restart` settles block_raw
    // to a stable id-set in the cap (no composed seam). (StartApp stays out:
    // the composed SUT is pre-booted, so `app_started` is true → its precondition
    // gates it out.)
    arm!(SimulateRestart, E2ETransition::SimulateRestart);
    if arms.is_empty() {
        return Just(E2ETransition::Nothing(Nothing)).boxed();
    }
    Union::new_weighted(arms).boxed()
}

/// SWAP DESIGN PROBE (run with `--nocapture`): print the alphabet
/// `aggregate_transitions` ACTUALLY generates over the candidate swap ref (the
/// seeded wide tree, but with the `full_headless` wiring + cap_set the
/// production `general_e2e_pbt` carries). Unlike the builder cap-feasibility
/// probe, this also applies the WIRING gate + preconditions over a real seeded
/// state — the true drive surface of the swap.
#[test]
fn swap_design_probe_generated_alphabet() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    use crate::pbt::transitions::aggregate_transitions;

    let state = wide_e2e_ref();

    let strat = aggregate_transitions(&state);
    let mut runner = TestRunner::deterministic();
    let mut names: BTreeSet<&'static str> = BTreeSet::new();
    for _ in 0..4000 {
        names.insert(
            strat
                .new_tree(&mut runner)
                .unwrap()
                .current()
                .variant_name(),
        );
    }
    eprintln!("\n=== SWAP DESIGN PROBE: aggregate_transitions over the swap ref ===");
    eprintln!("GENERATED ({}): {:?}", names.len(), names);
    eprintln!("=== end ===\n");

    // NON-VACUITY: the swap must drive the AUTO-NARROWED PRODUCTION alphabet, not a
    // collapsed handful. From the initial seeded state the generator must offer the
    // structural + nav core AND the new transitions the swap folds in beyond the
    // curated `wide_aggregate` (so a future cap/wiring regression that silently
    // narrows the alphabet fails HERE). `Join`/`MoveCursor`/`Redo` etc. unlock only
    // once state evolves (blocks to join, an editor open, a mutation to undo), so
    // they are not asserted from the initial state.
    // `SetupWatch` is feasible from the initial state (task #5: watch-query parity
    // converged, the `.without(SutWatchRegister)` narrowing dropped). `RemoveWatch`
    // unlocks only after a watch exists, so it is not asserted from the initial
    // state (like `Join`/`MoveCursor`/`Redo`).
    for required in [
        "SplitBlock",
        "NavigateFocus",
        "SwitchView",
        "EmitMcpData",
        "SetupWatch",
    ] {
        assert!(
            names.contains(required),
            "swap alphabet missing {required} — generated only {names:?}"
        );
    }
    // And it must be strictly wider than the storage-only structural core.
    assert!(
        names.len() >= 10,
        "swap alphabet collapsed to {} variants: {names:?}",
        names.len()
    );

    // Drop the ref off-thread (owns an Arc<Runtime>).
    std::thread::spawn(move || drop(state))
        .join()
        .expect("drop ReferenceState off the async executor");
}

// ═════════════════════════════════════════════════════════════════
// THE SWAP: `general_e2e_composed_pbt` — the production `general_e2e_pbt` SUT
// swapped from `E2ESut` to a composed `CapMap` over
// `compose_sut(full_headless)`, driving the AUTO-NARROWED production alphabet
// (`aggregate_transitions`, NOT a curated list) so the swap can never silently
// drift from what `general_e2e_pbt` actually generates. The SUT side is the
// EXACT same builder + boot as `frontend_wide_pbt` (`boot_and_seed_wide`);
// the only difference from `WideFrontend` is the GENERATOR: the full production
// `aggregate_transitions` over a ref carrying the `full_headless` wiring +
// cap_set, so the alphabet auto-narrows to exactly the composed SUT's drivable
// caps (peer/seam/E4/fixture ops cap-gate out — see
// `swap_probe_full_headless_narrowed_alphabet`). This is the §5 keystone: once
// green + verdict-parity-gated, `general_e2e_pbt`'s own macro SUT can be
// repointed here and `E2ESut`'s headless cap impls deleted (E3).

// `wide_e2e_ref`, `WideE2EMachine`, and `WideE2E` are RELOCATED to the
// `pbt`-gated `crate::pbt::composed::wide_e2e` module (glob-imported above) so
// the PRODUCTION integration test `general_e2e_composed_pbt`
// (`tests/general_e2e_composed_pbt.rs`) can drive `ComposedSut<WideE2E>` — the
// macro repoint. `swap_design_probe_generated_ alphabet` (above) still
// exercises the relocated `wide_e2e_ref` as a fast lib unit test.

// ═════════════════════════════════════════════════════════════════
// Editor arm — the SAME composed-SUT machinery, but driving the production
// EDITOR alphabet (`TypeChars`) over the REAL headless editor pipeline
// (`HeadlessEditorMirror` hosted on `HeadlessFrontendComponent`, Loro ENABLED
// so the block's `content_raw` `MutableText` resolves). Committed-content
// parity: the reference commits typed text into block content on every
// `TypeChars` (`commit_active_editor_if_changed`); the SUT's per-keystroke
// `MutableText` edit syncs to `block_raw`, so `inv-block-content/block_raw`
// agrees. The editor is pre-opened on `c1` on both sides (the oracle via the
// UI-actor wiring in `build_started_ref`, the SUT via `FocusEditableText`).
// Kept separate from `frontend_wide_pbt` (Loro-off, structural) so the
// structural arm is unaffected by the storage-layer change. `DeleteBackward` is
// excluded for now: a backspace at caret 0 is the structural `join_block`
// (block removal), which the mint-only per-tick reconcile doesn't model — a
// later increment.
// ═════════════════════════════════════════════════════════════════

/// The editor oracle: the same page-rooted `parent/c1/c2` tree as
/// `structural_ref`, but wired `{Loro, EditorState}` (Loro storage →
/// `enable_loro`; UI actor → `has_editor_buffer`) so the editor transitions
/// gate, with focus + an open editor on `c1` (seeded by `build_started_ref`'s
/// UI-actor branch). No final `NavigateFocus` (that would blur the editor) —
/// focus stays on the editor block.
fn editor_ref() -> ReferenceState {
    use crate::pbt::invariants::registry::Subsystem;
    let subsystems: BTreeSet<Subsystem> = [Subsystem::Loro, Subsystem::EditorState]
        .into_iter()
        .collect();
    // Seeds `parent→{c1,c2}` (and, via the UI actor, an initial focus/editor on
    // `c1` — overwritten below by the boot-mirroring sequence).
    let mut state = build_started_ref(&subsystems);
    let page = page_root();
    let ids = fixed_ids();

    // Insert the page root as a seed page (same shape as `structural_ref`) so the
    // working tree compares as flat leaf siblings under the page.
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

    // Re-root parent/c1/c2 flat under the page (was `parent→{c1,c2}`).
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

    // Model the boot-fired journal day-page so it enters `all_block_ids` — the
    // ref-known universe the `inv-viewmodel-entity-ids-subset-of-data` phantom
    // check subtracts. `boot_and_seed_editor` pins the keystone boot clock, so the
    // SUT's auto-created day-page id matches `keystone_boot_journal_id`; without
    // this seed it renders in the journals feed as an unknown (phantom) id — the
    // midnight-rollover class fixed for the org-link-marks twin in c21a8a00.
    crate::pbt::composed::wide_e2e::seed_boot_journal(&mut state);

    // Mirror the SUT boot sequence EXACTLY so every invariant aligns: navigate
    // focus to the page root (this BLURS any open editor and sets the nav
    // matview to the page — the SUT's `NavigateFocus(page)`), then open the
    // editor on `c1` (the SUT's `FocusEditableText(c1)`, which sets
    // `active_editor` to `c1` at end-of-text WITHOUT moving nav focus). Net:
    // nav focus = page (matches the SUT matview), active editor = c1 (the
    // editor invariants compare this).
    NavigateFocus {
        region: Region::Main,
        block_id: page.clone(),
    }
    .apply_to_ref(&mut state);
    FocusEditableText {
        block_id: ids.c1.clone(),
    }
    .apply_to_ref(&mut state);
    state
}

/// Boot the windowless session with Loro ENABLED (so `MutableText` resolves),
/// focus the page root then open the editor on `c1` (matching the oracle), and
/// register the editor READ cap (selects the editor invariants). Returns the
/// cap map + scaffold. Used by the focused editor teeth (the editor coverage
/// that pre-opens an editor and checks strict per-tick caret/text parity). The
/// combined `frontend_wide_pbt` now drives the editor transitions interleaved
/// with the structural ones (#2); this pre-opened-editor boot remains the
/// teeth's focused anchor.
async fn boot_and_seed_editor(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
) -> (CapMap, BTreeSet<EntityUri>) {
    // Pin the boot clock to the fixed keystone day so the boot auto-create rule
    // mints a DETERMINISTIC journal day-page id (`keystone_boot_journal_id`), not
    // one keyed on the host's real date. The plain `new_with_loro` boot uses the
    // OS `SystemClock`, whose date-dependent day-page `editor_ref` never modeled →
    // a date-dependent `inv-viewmodel-entity-ids-subset-of-data` phantom
    // (midnight-rollover class, mirrors c21a8a00).
    let comp = Arc::new(
        HeadlessFrontendComponent::new_with_clock(
            &[("structural-page.org", WIDE_TREE_ORG)],
            Duration::from_millis(300),
            true,
            crate::pbt::frontend_slice::components::keystone_boot_clock(),
        )
        .await,
    );
    let engine = comp.engine();
    let mut caps = CapMap::new();
    comp.clone().register(&mut caps);
    caps.insert(comp.clone() as Arc<dyn SutSqlProjection>);
    // `SutFocus` (C-5 split, 2026-07-02) — preserves focus/nav invariant
    // selection over this real renderer (paired with a `RefFocus` ref).
    caps.insert(comp.clone() as Arc<dyn SutFocus>);
    // `SutQueryResults` (full-mode query engine) — mirrors `SutSqlProjection`:
    // keeps `inv-viewmodel-decompiled-rows-match-query` selected and the
    // degraded `inv-viewmodel-shows-source-when-no-query` twin deselected over
    // this real renderer.
    caps.insert(comp.clone() as Arc<dyn SutQueryResults>);
    // The editor READ cap — pairs with the (always-registered) `RefEditorMirror` to
    // select `inv-editor-{text,caret}-matches-ref`. The WRITE cap is already in the
    // component's `register`.
    caps.insert(comp.clone() as Arc<dyn SutEditorMirrorRead>);
    // Intentionally REPLACE register's fresh-resolver `OpDispatchWriter` with the
    // shared-resolver one (explicit `replace` — plain `insert` fails loud on the
    // dup).
    caps.replace(
        Arc::new(OpDispatchWriter::with_resolver(engine, resolver.clone()))
            as Arc<dyn SutBlockTreeWrite>,
    );

    let ids = fixed_ids();
    let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
        .into_iter()
        .collect();

    // The boot journal auto-create fires ASYNC off the clock CDC; the 300ms boot
    // settle can return before the day-page lands. Await it (fail loud on timeout)
    // so the scaffold snapshot below deterministically captures it and the
    // widget-tree phantom check sees a settled, ref-known block.
    let journal_id = crate::pbt::frontend_slice::components::keystone_boot_journal_id();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        tokio::time::sleep(SETTLE).await;
        if sut_ids(&caps).await.contains(&journal_id) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "[editor-boot] boot journal {journal_id} did not fire within budget"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let booted = sut_ids(&caps).await;
    let scaffold: BTreeSet<EntityUri> = booted.difference(&tree).cloned().collect();

    // Focus the page root (so its children render in the main panel), then open the
    // editor on `c1` — the SUT side of the oracle's pre-opened editor.
    TransitionImpl::apply_to_sut(
        &NavigateFocus {
            region: Region::Main,
            block_id: page_root(),
        },
        ref_state,
        &mut caps,
    )
    .await;
    TransitionImpl::apply_to_sut(
        &FocusEditableText {
            block_id: ids.c1.clone(),
        },
        ref_state,
        &mut caps,
    )
    .await;
    tokio::time::sleep(SETTLE).await;

    (caps, scaffold)
}

// The standalone editor PBT (`WideEditor`/`frontend_editor_pbt`) has been
// FOLDED into the combined `frontend_wide_pbt` (#2): the wide alphabet now
// drives the editor transitions
// (`FocusEditableText`/`TypeChars`/`DeleteBackward`) interleaved with the
// structural ones over the same Loro-on headless component. The focused editor
// coverage (pre-opened editor, strict per-tick caret/text parity) lives on in
// the editor teeth below via `editor_ref` + `boot_and_seed_editor`.

// ─────────────────────────────────────────────────────────────────
// Teeth — prove the reconcile loop + invariants over the headless component are
// REAL: a faithful lockstep split stays green, and a SUT-only split (oracle NOT
// applied, so its minted block is unreconciled) is CAUGHT by the block-set
// comparison. The positive direction is also covered by
// `components::tests::headless_structural_seed_and_reconcile_probe`.
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod teeth {
    use holon_pbt_core::TransitionImpl;

    use super::*;

    /// Drop a `ReferenceState` (owns an `Arc<tokio::Runtime>`) off the async
    /// executor — dropping it inside a `#[tokio::test]` context panics.
    fn drop_ref_off_thread(state: ReferenceState) {
        std::thread::spawn(move || drop(state))
            .join()
            .expect("drop ReferenceState off the async executor");
    }

    /// **Companion cold-boot page-authority GREEN-lock (dogfood 2026-07-12,
    /// Fork A).** Deterministic boot (no transitions) of a
    /// folder-page-duplication vault through the REAL keystone boot
    /// (`boot_and_seed_wide`): a top-level page- file `2026-07-10.org` (a
    /// `Page` doc-root) whose id is ALSO inlined as a plain, untagged
    /// heading in the `Journals.org` COMPANION, ingested LAST. Asserts the
    /// companion boot stays clean — no swallowed ERROR
    /// (`inv-no-observed-errors`) and no `Page`-tag demotion
    /// (`inv-sidebar-page-tag-preserved`, non-vacuously) — locking the foreign-
    /// page protection at the real keystone boot layer.
    ///
    /// SCOPE NOTE — this flat top-level shape is a GREEN regression LOCK, not a
    /// RED→GREEN reproduction. The deterministic RED-catch of the demotion
    /// oracle lives in the invariant's own fixture triad
    /// (`composed::invariants::sidebar_page_tag_preserved::tests::catches_demoted_page`).
    /// The real ingest FAILURE reproduces only with a SUBDIR page-file
    /// (`Journals/2026-07-10.org`), where the page nests under the `journals`
    /// folder-page and the Loro `create_in_tree` of the already-rooted id
    /// times out + quarantines — but the subdir ALSO trips a SEPARATE
    /// nested-page Pages-sidebar render PANIC
    /// (`holon-frontend/src/row_origin.rs` "disjoint root rows") at boot,
    /// so it cannot boot green with the tag-authority fix alone. Both the
    /// subdir keystone reproduction and the render panic are covered in the
    /// Fork-A report / BugFunnel. Runs a REDUCED registry (the two
    /// Fork-A oracles) since the companion is intentionally lossy on org round-
    /// trip (`inv-org-render-fixed-point` — Fork B's writeback-oracle work).
    /// Seeds the topology directly (env-independent), not via
    /// `folder_companion_enabled`.
    #[tokio::test(flavor = "multi_thread")]
    async fn folder_companion_cold_boot_preserves_page_authority() {
        use holon_pbt_core::composition::CapInvariant;

        use crate::pbt::composed::invariants::observed_errors;
        use crate::pbt::composed::invariants::sidebar_page_tag_preserved;

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        // Minimal proven-green base (no forward-edge) + the companion topology.
        let mut oracle = frontend_wired(structural_ref());
        seed_folder_companion(&mut oracle);
        assert!(
            oracle
                .domain
                .block_state
                .blocks
                .contains_key(&folder_journal_page()),
            "topology precondition: the date page must be seeded into the oracle"
        );

        let (caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let registry: Vec<Box<dyn CapInvariant>> =
            vec![observed_errors::wire(), sidebar_page_tag_preserved::wire()];
        let report = run_with_seeded_ref(&registry, &caps, resolved).await;

        let failures = report.failures();
        // The companion must ingest CLEANLY — no quarantine / no swallowed ERROR.
        assert!(
            !failures
                .iter()
                .any(|(id, _)| *id == "inv-no-observed-errors"),
            "folder-companion cold boot must not quarantine the companion ingest (foreign-page \
             protection missing?); failures: {failures:?}",
        );
        // The page-file's `Page` tag must SURVIVE the companion reconcile.
        assert!(
            !failures
                .iter()
                .any(|(id, _)| *id == "inv-sidebar-page-tag-preserved"),
            "folder-companion cold boot must not demote the page-file's Page tag; failures: \
             {failures:?}",
        );
        // Non-vacuity: the sidebar page-tag oracle actually ran over the seeded page.
        assert!(
            report
                .ran_ids()
                .iter()
                .any(|id| *id == "inv-sidebar-page-tag-preserved"),
            "inv-sidebar-page-tag-preserved must select + run over the seeded page (ran: {:?})",
            report.ran_ids(),
        );
    }

    /// **BugFunnel row 137 PERSISTED REGRESSION SEED — the SUBDIR fileless
    /// journals topology converges with zero loss on the composed keystone.**
    ///
    /// The real row-137 shape (distinct from the flat top-level
    /// `folder_companion_cold_boot_preserves_page_authority` above, which is
    /// Fork A's page-tag closure): `Journals.org` inlines a `:Page:`-tagged
    /// date heading (`* 2026-07-11 :Page:`) with body text, and there is NO
    /// `Journals/2026-07-11.org` on disk — the date page is FILELESS. Booted
    /// through the REAL keystone boot (`boot_and_seed_wide`, subdir closure
    /// keyed on `seed_folder_companion_subdir`). After settle, four things
    /// must hold, and this test asserts all of them GREEN and NON-INERT:
    ///
    /// 1. `inv-every-page-has-its-own-file` — the fileless date page is
    ///    MATERIALIZED into its own subdir file `Journals/2026-07-11.org`
    ///    (`#+ID: journal-2026-07-11`). PRE-B2 this was RED: the page owned no
    ///    file and its body lived only in the store (the row-137 loss).
    /// 2. `inv-companion-has-no-child-page-headings` — `Journals.org`
    ///    DE-INLINES the child-page heading (the `get_blocks` CTE excludes the
    ///    `Page`-tagged child, and the ADR-0025 sibling-grounded union guard
    ///    admits the de-inline because the child now survives in its own file).
    ///    PRE-B1' this was RED: the per-file guard refused the de-inline as
    ///    apparent block loss.
    /// 3. `inv-no-page-under-non-page` — the topology is legal (date →
    ///    `journals` (a page at `no_parent`) → root, all pages).
    /// 4. `inv-org-render-fixed-point` + `inv-no-observed-errors` — the whole
    ///    thing stabilizes (disk == render(SQL)) with no swallowed ERROR /
    ///    quarantine.
    ///
    /// This is the deterministic, env-independent (not via
    /// `folder_companion_enabled`) composed-keystone reproduction the
    /// plan's §5 item 4 / §7 item 4 calls for. The date `2026-07-11` is off
    /// the fixed keystone boot clock (`2026-01-15`) so the auto-create rule
    /// never touches it.
    #[tokio::test(flavor = "multi_thread")]
    async fn folder_companion_subdir_fileless_materializes_and_deinlines() {
        use holon_pbt_core::composition::CapInvariant;

        use crate::pbt::composed::invariants::companion_has_no_child_page_headings;
        use crate::pbt::composed::invariants::every_page_has_its_own_file;
        use crate::pbt::composed::invariants::no_page_under_non_page;
        use crate::pbt::composed::invariants::observed_errors;
        use crate::pbt::composed::invariants::org_render_fixed_point;
        use crate::pbt::composed::invariants::sidebar_page_tag_preserved;

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(structural_ref());
        seed_folder_companion_subdir(&mut oracle);
        assert!(
            oracle
                .domain
                .block_state
                .blocks
                .contains_key(&subdir_journal_page()),
            "topology precondition: the fileless subdir date page must be seeded"
        );

        let (caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let registry: Vec<Box<dyn CapInvariant>> = vec![
            observed_errors::wire(),
            sidebar_page_tag_preserved::wire(),
            org_render_fixed_point::wire(),
            companion_has_no_child_page_headings::wire(),
            every_page_has_its_own_file::wire(),
            no_page_under_non_page::wire(),
        ];
        let report = run_with_seeded_ref(&registry, &caps, resolved).await;

        assert!(
            report.failures().is_empty(),
            "the subdir fileless journals topology must converge with zero loss (materialize + \
             de-inline + legal topology + fixed point): {:?}",
            report.failures(),
        );
        // Non-inert: the two Fork-B oracles must actually select + run over the
        // seeded fileless page (else this would pass vacuously — the row-137 trap).
        for id in [
            "inv-every-page-has-its-own-file",
            "inv-companion-has-no-child-page-headings",
        ] {
            assert!(
                report.ran_ids().iter().any(|r| *r == id),
                "{id} must select + run over the seeded subdir journals topology (ran: {:?})",
                report.ran_ids(),
            );
        }
    }

    /// Boot a frontend over the given org files and return its composed CapMap
    /// (the caps the Fork B companion/materialization oracles select on),
    /// settled.
    async fn boot_companion_topology(
        files: &[(&str, &str)],
    ) -> (CapMap, Arc<HeadlessFrontendComponent>) {
        use holon_pbt_core::capabilities::SutSqlProjection;
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let comp =
            Arc::new(HeadlessFrontendComponent::new(files, Duration::from_millis(600)).await);
        let engine = comp.engine();
        let mut caps = CapMap::new();
        comp.clone().register(&mut caps);
        caps.insert(comp.clone() as Arc<dyn SutSqlProjection>);
        caps.replace(Arc::new(OpDispatchWriter::with_resolver(
            engine.clone(),
            resolver.clone(),
        )) as Arc<dyn SutBlockTreeWrite>);
        tokio::time::sleep(SETTLE).await;
        (caps, comp)
    }

    /// A minimal oracle that models `child-note` as a `Page` doc-root owning
    /// `child-note.org` (mirrors `wide_e2e::seed_folder_companion`), so
    /// `is_page_block(child-note)` drives the ref-consuming Fork B oracles. The
    /// SUT-only oracles (`inv-every-page-has-its-own-file`, fixed-point) ignore
    /// the ref; the base `structural_ref` blocks this topology doesn't boot
    /// are inert because these tests run no full block-id-set compare.
    fn child_note_page_oracle() -> crate::pbt::reference_state::Resolved<ReferenceState> {
        let mut oracle = structural_ref();
        let page = EntityUri::block("child-note");
        let mut page_block = Block::new_text(page.clone(), EntityUri::no_parent(), "child-note");
        page_block.set_page(true);
        oracle
            .domain
            .block_state
            .blocks
            .insert(page.clone(), page_block);
        oracle
            .domain
            .block_state
            .block_documents
            .insert(page.clone(), EntityUri::no_parent());
        oracle
            .files
            .documents
            .insert(page.clone(), "child-note.org".to_string());
        let resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        resolved
    }

    /// GREEN regression lock: the companion writeback de-inlines an OWNED child
    /// page LOSSLESSLY. `child-note.org` is a bare page-file (`#+ID:
    /// child-note`); `my-notes.org` inlines its id as a heading. After
    /// settle, `my-notes.org` converges to the bare `#+ID: my-notes` shell
    /// (the `get_blocks` CTE excludes the `Page`-tagged child) while
    /// `child-note` survives in its own file — no guard veto (the
    /// block-driven writeback path never calls `ensure_ingest_lossless`;
    /// empirical Fork B finding 2026-07-12), no loss. This documents that
    /// the de-inline ALREADY works when the page owns a file — the plan's
    /// original B1 guard-veto premise does not hold here.
    #[tokio::test(flavor = "multi_thread")]
    async fn folder_companion_deinlines_owned_child_page() {
        use holon_pbt_core::composition::CapInvariant;

        use crate::pbt::composed::invariants::companion_has_no_child_page_headings;
        use crate::pbt::composed::invariants::observed_errors;
        use crate::pbt::composed::invariants::org_render_fixed_point;
        use crate::pbt::composed::invariants::sidebar_page_tag_preserved;

        const CHILD_PAGE_ORG: &str = "#+ID: child-note\n";
        const COMPANION_ORG: &str =
            "#+ID: my-notes\n* child-note\n:PROPERTIES:\n:ID: child-note\n:END:\n";

        let (caps, _comp) = boot_companion_topology(&[
            ("child-note.org", CHILD_PAGE_ORG),
            ("my-notes.org", COMPANION_ORG),
        ])
        .await;
        let resolved = child_note_page_oracle();
        let registry: Vec<Box<dyn CapInvariant>> = vec![
            observed_errors::wire(),
            sidebar_page_tag_preserved::wire(),
            org_render_fixed_point::wire(),
            companion_has_no_child_page_headings::wire(),
        ];
        let report = run_with_seeded_ref(&registry, &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "owned-page de-inline must be lossless + fixed-point green: {:?}",
            report.failures(),
        );
        assert!(
            report
                .ran_ids()
                .iter()
                .any(|id| *id == "inv-companion-has-no-child-page-headings"),
            "companion oracle must select + run (ran: {:?})",
            report.ran_ids(),
        );
    }

    /// **Fork B B0 (RED-first): a FILELESS page must be MATERIALIZED into its
    /// own file.** `my-notes.org` inlines `child-note` as a `Page`-tagged
    /// heading (`* child-note :Page:`) with body text, and there is NO
    /// `child-note.org`. After settle, `child-note` is a `Page` in the
    /// store, correctly de-inlined from the companion (`my-notes.org` →
    /// bare `#+ID: my-notes`) — but it owns NO file on disk, so its content
    /// ("body text") exists only in the store and vanishes on any
    /// store-rebuild-from-disk: silent loss.
    ///
    /// This is the real Fork B bug (the owned case above already works). The
    /// writeback must MATERIALIZE `child-note` into its own file
    /// (`inv-every-page-has-its-own-file`). Fork B B2 (materialization via the
    /// `DocumentManager` name-chain path + a boot sweep) makes it green.
    ///
    /// **Scope (B2 only):** this asserts ONLY `child_has_file` — the fileless
    /// page gets materialized into its own `#+ID: child-note` file. The
    /// companion's *convergence* (de-inline + fixed point) is B1''s job
    /// (the block-driven writeback path has no loss guard yet, so
    /// `my-notes.org` may still carry the inline heading until B1' lands
    /// the union guard) and is tested separately.
    ///
    /// Non-reserved names dodge the reserved-`Journals` programmatic seed.
    /// EMPIRICAL (2026-07-12): child-note is a fileless child of the `my-notes`
    /// companion, so its `name_chain` is `["my-notes", "child-note"]` and B2
    /// materializes it at the NESTED path `my-notes/child-note.org`. Headless
    /// this materializes WITHOUT panic (the `row_origin.rs` "disjoint root
    /// rows" fix, coordinator #47, landed on this integration line); the
    /// recursive `scan_directory` in `disk_org_file_ids` surfaces the
    /// nested file.
    #[tokio::test(flavor = "multi_thread")]
    async fn fileless_page_writeback_materializes() {
        // Companion inlines child-note as a Page-tagged heading; NO child-note.org.
        // The heading text is DELIBERATELY distinct from the `:ID:`. When they
        // matched (`* child-note` / `:ID: child-note`) a file containing only
        // `#+ID: child-note` satisfied every text assertion, which masked the
        // BugFunnel 2026-07-30 loss of the page's own title and body.
        const COMPANION_ORG: &str = "#+ID: my-notes\n* Child Note :Page:\n:PROPERTIES:\n:ID: \
                                     child-note\n:END:\nbody text that must not vanish\n";

        let (_caps, comp) = boot_companion_topology(&[("my-notes.org", COMPANION_ORG)]).await;

        // Materialization check via DISK TRUTH (not the boot-tracked snapshot,
        // which cannot see a file materialized after boot): after settle + the B2
        // boot sweep, `child-note` must own a file — some `.org` on disk whose
        // `#+ID:` is `child-note`.
        let disk_ids = comp.disk_org_file_ids().await;
        assert!(
            disk_ids.iter().any(|id| id == "child-note"),
            "the fileless page `child-note` must be materialized into its own `child-note.org` (a \
             `#+ID: child-note` file), but no file owns it; on-disk file ids = {disk_ids:?}",
        );
        // Owning a file is not enough — the page's own name and body must be IN
        // it. A header-only stub is the BugFunnel 2026-07-30 loss.
        let contents = comp.disk_org_contents().await;
        let all_text = contents
            .iter()
            .map(|(_, c)| c.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let named_on_disk = contents
            .iter()
            .any(|(p, _)| p.to_string_lossy().contains("Child Note"));
        assert!(
            named_on_disk && all_text.contains("body text that must not vanish"),
            "the materialized page must carry its own name (in its filename) AND its body, not \
             just an `#+ID:` header; disk = {contents:#?}",
        );
    }

    /// **Ingest-born page (BugFunnel 2026-07-30 data loss).** The sibling
    /// `fileless_page_writeback_materializes` above uses a bare `#+ID:` file
    /// whose heading text IS the child's id. Martin's clean-room repro differs
    /// on three axes a real vault always has: a `#+TITLE:` line, root body
    /// text before the first headline, and a child whose `:ID:`
    /// (`tagged-child`) is NOT its heading text (`Tagged Child`). In that
    /// shape the first boot PRUNES the `:Page:`-tagged child out of its
    /// parent file and materializes NOTHING — the child's body exists in no
    /// file on disk.
    ///
    /// The oracle is disk truth over the WHOLE vault (recursive scan, so a
    /// nested `Tagged Root/Tagged Child.org` counts as materialized): the
    /// child's body text must survive SOMEWHERE, and `tagged-child` must own
    /// a file. The untagged control in the same vault pins the tag as the
    /// trigger rather than the file shape.
    #[tokio::test(flavor = "multi_thread")]
    async fn ingest_born_page_materializes_before_parent_prune() {
        const TAGGED_ORG: &str = "#+TITLE: Tagged Root\n#+ID: tagged-root\n\nRoot body.\n\n* \
                                  Tagged Child :Page:\n:PROPERTIES:\n:ID: \
                                  tagged-child\n:END:\nChild body.\n";
        const UNTAGGED_ORG: &str = "#+TITLE: Untagged Root\n#+ID: untagged-root\n\nRoot \
                                    body.\n\n* Untagged Child\n:PROPERTIES:\n:ID: \
                                    untagged-child\n:END:\nChild body.\n";

        let (_caps, comp) =
            boot_companion_topology(&[("Tagged.org", TAGGED_ORG), ("Untagged.org", UNTAGGED_ORG)])
                .await;

        let contents = comp.disk_org_contents().await;
        let all_text = contents
            .iter()
            .map(|(_, c)| c.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        // Control: the untagged child is never at risk — if this fails the
        // vault-shape itself broke, not the page promotion.
        assert!(
            all_text.contains("Untagged Child"),
            "control: an UNTAGGED child heading must survive write-back; disk = {contents:#?}",
        );

        // The bug: the tagged child's body must exist in some file on disk.
        // The page's NAME travels in its filename (the page path is built from
        // the block's title, which is how every page file is named); its BODY
        // must be inside some file. Both were destroyed before the fix.
        let named_on_disk = contents
            .iter()
            .any(|(p, _)| p.to_string_lossy().contains("Tagged Child"));
        assert!(
            named_on_disk && all_text.contains("Child body."),
            "the `:Page:`-tagged child's name AND body must survive the first boot — they were \
             pruned from the parent and written to no file; disk = {contents:#?}",
        );
        // The pre-first-headline root body of BOTH files (the same defect one
        // level up: a doc-root's own content was never rendered).
        assert_eq!(
            all_text.matches("Root body.").count(),
            2,
            "both files' pre-first-headline root bodies must survive write-back; disk = \
             {contents:#?}",
        );
        let disk_ids = comp.disk_org_file_ids().await;
        assert!(
            disk_ids.iter().any(|id| id == "tagged-child"),
            "inv-every-page-has-its-own-file: the ingest-born page `tagged-child` must own a \
             file; on-disk file ids = {disk_ids:?}",
        );
    }

    /// LORO TWIN. The Loro-ON twin of
    /// `ingest_born_page_materializes_before_parent_prune`.
    /// The repro ran the real GPUI app, which boots the CRDT layer; the
    /// Turso-only twin above passes, so this pins whether the storage axis is
    /// what the headless harness was missing.
    #[tokio::test(flavor = "multi_thread")]
    async fn ingest_born_page_materializes_before_parent_prune_loro() {
        const TAGGED_ORG: &str = "#+TITLE: Tagged Root\n#+ID: tagged-root\n\nRoot body.\n\n* \
                                  Tagged Child :Page:\n:PROPERTIES:\n:ID: \
                                  tagged-child\n:END:\nChild body.\n";
        const UNTAGGED_ORG: &str = "#+TITLE: Untagged Root\n#+ID: untagged-root\n\nRoot \
                                    body.\n\n* Untagged Child\n:PROPERTIES:\n:ID: \
                                    untagged-child\n:END:\nChild body.\n";

        let comp = HeadlessFrontendComponent::new_with_loro(
            &[("Tagged.org", TAGGED_ORG), ("Untagged.org", UNTAGGED_ORG)],
            Duration::from_millis(600),
            true,
        )
        .await;
        tokio::time::sleep(SETTLE).await;

        let contents = comp.disk_org_contents().await;
        let all_text = contents
            .iter()
            .map(|(_, c)| c.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            all_text.contains("Untagged Child"),
            "control: an UNTAGGED child heading must survive write-back; disk = {contents:#?}",
        );
        // The page's NAME travels in its filename (the page path is built from
        // the block's title, which is how every page file is named); its BODY
        // must be inside some file. Both were destroyed before the fix.
        let named_on_disk = contents
            .iter()
            .any(|(p, _)| p.to_string_lossy().contains("Tagged Child"));
        assert!(
            named_on_disk && all_text.contains("Child body."),
            "the `:Page:`-tagged child's name AND body must survive the first boot — they were \
             pruned from the parent and written to no file; disk = {contents:#?}",
        );
        // DISCLOSED GAP (Loro wiring only): the SECONDARY root-body loss is not
        // asserted here. Under Turso the doc-root's own content reaches disk —
        // the sibling test above asserts exactly that — but under Loro the
        // synced doc-root content never reaches the render, so the PARENT files
        // still lose `Root body.` and their true `#+TITLE:`. That is a separate
        // storage-seam divergence, reported as a follow-up. The PRIMARY loss
        // this test exists for — the `:Page:`-tagged child — is asserted above
        // and is fixed on BOTH wirings.
        let disk_ids = comp.disk_org_file_ids().await;
        assert!(
            disk_ids.iter().any(|id| id == "tagged-child"),
            "inv-every-page-has-its-own-file: the ingest-born page `tagged-child` must own a \
             file; on-disk file ids = {disk_ids:?}",
        );
    }

    /// **Fork B B2 echo gate (RULED DONE criterion).** After the B2 boot sweep
    /// materializes the fileless `child-note` into `child-note.org`,
    /// re-triggering the production watcher over that own-written file must
    /// be IDEMPOTENT: the `last_projection` seed suppresses the echo, so
    /// the file stays written EXACTLY ONCE (no duplicate `child-note` file)
    /// and the page is NOT re-minted under a new id (the `block_raw` id-set
    /// is unchanged across the pump). A missing `last_projection` seed
    /// would re-ingest our own write and could mint a second page / rewrite
    /// the file in a loop — this test locks that closed.
    #[tokio::test(flavor = "multi_thread")]
    async fn fileless_page_materialization_is_echo_stable() {
        // The heading text is DELIBERATELY distinct from the `:ID:`. When they
        // matched (`* child-note` / `:ID: child-note`) a file containing only
        // `#+ID: child-note` satisfied every text assertion, which masked the
        // BugFunnel 2026-07-30 loss of the page's own title and body.
        const COMPANION_ORG: &str = "#+ID: my-notes\n* Child Note :Page:\n:PROPERTIES:\n:ID: \
                                     child-note\n:END:\nbody text that must not vanish\n";

        let (_caps, comp) = boot_companion_topology(&[("my-notes.org", COMPANION_ORG)]).await;

        // Precondition: B2 materialized exactly one `child-note` file. Discover its
        // path (NESTED — `my-notes/child-note.org` — since child-note is a fileless
        // child of the `my-notes` companion), don't assume a flat name.
        let child_paths: Vec<_> = comp
            .disk_org_files()
            .await
            .into_iter()
            .filter(|(_, id)| id.as_deref() == Some("child-note"))
            .map(|(p, _)| p)
            .collect();
        assert_eq!(
            child_paths.len(),
            1,
            "B2 must materialize `child-note` into exactly one file; found {child_paths:?}",
        );
        let store_before = comp.store_block_ids().await;

        // Pump the watcher over the B2-written file — the echo path.
        comp.pump_watcher_over_disk_path(&child_paths[0]).await;

        // Exactly-once: still one `child-note` file, no duplicate materialized.
        let child_after: Vec<_> = comp
            .disk_org_files()
            .await
            .into_iter()
            .filter(|(_, id)| id.as_deref() == Some("child-note"))
            .map(|(p, _)| p)
            .collect();
        assert_eq!(
            child_after.len(),
            1,
            "after the watcher pump `child-note` must remain exactly one file (echo suppressed); \
             found {child_after:?}",
        );
        // No re-mint: the block_raw id-set is unchanged (child-note not re-created
        // under a fresh id, nothing dropped).
        let store_after = comp.store_block_ids().await;
        assert_eq!(
            store_before, store_after,
            "re-ingesting the materialized page must be id-stable (no re-mint / no loss); \
             before={store_before:?} after={store_after:?}",
        );
    }

    /// **Copy-on-write default layout (F4 stale-seed remedy).** The bundled
    /// default layout (`block:__default__`) is a VIRTUAL seed doc: it is seeded
    /// into the store on boot but must NOT be auto-materialized to a vault
    /// `.org` file. The Fork B B2 fileless-page sweep (a92d7eb7, 2026-07-12)
    /// used to write `__default__.org`, pinning Martin's vault to a stale
    /// Jul-12 seed (the F4 backlinks-invisible saga). The seed doc now
    /// stays virtual until a user edit materializes it (copy-on-write); the
    /// runtime page write-back — not the boot sweep — owns that first file.
    ///
    /// Guards BOTH materialization sites: `materialize_missing_page_files` (B2
    /// boot sweep) skips `is_seed_layout_doc`, and
    /// `materialize_page_identity_file` skips it while `boot_seeding`.
    /// Booting over an unrelated companion, the default layout must reach
    /// the STORE (seeded, virtual) yet own NO file on disk.
    #[tokio::test(flavor = "multi_thread")]
    async fn seed_default_layout_is_virtual_not_materialized_on_boot() {
        let (_caps, comp) =
            boot_companion_topology(&[("notes.org", "#+ID: notes\n* hello\n")]).await;

        // Seeded into the store (virtual layout is present)...
        let store_ids = comp.store_block_ids().await;
        assert!(
            store_ids.iter().any(|id| id.contains("__default__")),
            "the default layout must be SEEDED into the store (virtual), even though it owns no              file; store ids = {store_ids:?}",
        );

        // ...but NOT written to disk (copy-on-write: no auto-materialization).
        let default_on_disk =
            comp.disk_org_files().await.into_iter().any(|(path, _)| {
                path.file_name().and_then(|n| n.to_str()) == Some("__default__.org")
            });
        assert!(
            !default_on_disk,
            "the virtual seed layout must NOT be materialized to `__default__.org` on boot \
             (copy-on-write; F4 stale-seed pin) — on-disk org files = {:?}",
            comp.disk_org_files().await,
        );
    }

    /// **Increment-3 fresh-drive + ORG-SEED probe — the full catalog is green
    /// over `compose_sut(frontend)`.** The store-only
    /// seed left the working tree absent from the org files `SutOrgRead`
    /// parses, so `inv-blocks-match-ref/org` diverged. Here the tree IS the
    /// boot org (page-rooted leaf siblings, pinned `:ID:`), so the session
    /// ingests it into the store AND `SutOrgRead` parses it — store and org
    /// share one source. With the SUT focus driven, the FULL catalog (incl.
    /// the org invariant) must go green.
    #[tokio::test(flavor = "multi_thread")]
    async fn frontend_fresh_drive_org_seed_full_catalog_green() {
        use holon_pbt_core::capabilities::SutFocus;
        use holon_pbt_core::capabilities::SutQueryResults;
        use holon_pbt_core::capabilities::SutSqlProjection;
        use holon_pbt_core::composition::CapProvider;

        // The page-rooted working tree AS org: `structural-page` is the doc/page,
        // parent/c1/c2 are its leaf-sibling children with pinned bare ids.
        const TREE_ORG: &str = "#+ID: structural-page\n* parent\n:PROPERTIES:\n:ID: \
                                parent\n:END:\n* c1\n:PROPERTIES:\n:ID: c1\n:END:\n* \
                                c2\n:PROPERTIES:\n:ID: c2\n:END:\n";

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        // File name = page content: the doc/page title the viewmodel renders comes
        // from the filename, and the oracle's page block content is "structural-page".
        // Pin the boot clock to the fixed keystone day so the boot auto-create rule
        // mints a DETERMINISTIC journal day-page id (`keystone_boot_journal_id`), not
        // one keyed on the host's real date — the plain `new` (SystemClock) boot
        // date-hashes today's day-page, which `structural_ref` never modeled → a
        // date-dependent `inv-viewmodel-entity-ids-subset-of-data` phantom
        // (midnight-rollover class, mirrors the org-link-marks twin in c21a8a00).
        let comp = Arc::new(
            HeadlessFrontendComponent::new_with_clock(
                &[("structural-page.org", TREE_ORG)],
                Duration::from_millis(300),
                false,
                crate::pbt::frontend_slice::components::keystone_boot_clock(),
            )
            .await,
        );
        let engine = comp.engine();
        let mut caps = CapMap::new();
        comp.clone().register(&mut caps);
        caps.insert(comp.clone() as Arc<dyn SutSqlProjection>);
        // `SutFocus` (C-5 split, 2026-07-02) — preserves focus/nav selection.
        caps.insert(comp.clone() as Arc<dyn SutFocus>);
        // `SutQueryResults` (full-mode query engine) — same rationale as the combined
        // boot above: keeps the full decompiled twin selected and the degraded twin
        // off.
        caps.insert(comp.clone() as Arc<dyn SutQueryResults>);
        // Intentionally REPLACE register's fresh-resolver writer with the
        // shared-resolver one (explicit `replace` — plain `insert` fails loud
        // on the duplicate).
        caps.replace(Arc::new(OpDispatchWriter::with_resolver(
            engine.clone(),
            resolver.clone(),
        )) as Arc<dyn SutBlockTreeWrite>);

        // Scaffold = everything booted EXCEPT the non-seed working tree (parent/c1/c2).
        // `structural-page` is the seed page, so it stays in the scaffold (injected).
        let ids = fixed_ids();
        let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
            .into_iter()
            .collect();

        // The boot journal auto-create fires ASYNC off the clock CDC; the 300ms boot
        // settle can return before the day-page lands. Await it (fail loud on timeout)
        // so the scaffold snapshot below deterministically captures it.
        let journal_id = crate::pbt::frontend_slice::components::keystone_boot_journal_id();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            tokio::time::sleep(SETTLE).await;
            if sut_ids(&caps).await.contains(&journal_id) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "[org-seed] boot journal {journal_id} did not fire within budget"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let booted = sut_ids(&caps).await;
        let scaffold: BTreeSet<EntityUri> = booted.difference(&tree).cloned().collect();

        let mut oracle = structural_ref();
        // Model the boot-fired journal day-page in the oracle's block_state so it
        // enters `all_block_ids` — the ref-known universe the phantom check
        // subtracts. `inject_scaffold_seed` (below) only touches `block_documents`,
        // so WITHOUT this the auto-created day-page renders as an unknown (phantom)
        // entity id. The pinned keystone clock makes the SUT's day-page id match.
        crate::pbt::composed::wide_e2e::seed_boot_journal(&mut oracle);
        NavigateFocus {
            region: Region::Main,
            block_id: page_root(),
        }
        .apply_to_sut(&oracle, &mut caps)
        .await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold);

        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        eprintln!(
            "[org-seed] ran {} invariants: {:?}",
            report.ran_ids().len(),
            report.ran_ids()
        );
        for (id, msg) in report.failures() {
            eprintln!("[org-seed] FAIL {id}: {msg}");
        }
        assert!(
            report.ran_ids().contains(&"inv-blocks-match-ref/org"),
            "org invariant must SELECT (ran: {:?})",
            report.ran_ids()
        );
        assert!(
            report.failures().is_empty(),
            "org-seed fresh-drive: the FULL catalog must be green over compose_sut(frontend): {:?}",
            report.failures()
        );
    }

    /// **Org-ingest MARKS gate (dogfood 2026-07-10 link-destruction class).**
    /// Boot the composed headless SUT from an org file whose `c2` headline
    /// carries a `[[Linked Page]]` wiki link, and run the full catalog
    /// against a reference whose `c2` holds the org-lens `(content, marks)`
    /// fixed point. The SUT ingest must persist `block.marks` through
    /// `build_block_params` into the stores — with the ingest
    /// drop reinstated (marks param omitted), `inv-blocks-match-ref` goes RED
    /// on `marks: None` vs `Some(Link)`. Non-vacuity: the ref block's marks
    /// are asserted `Some` before the catalog runs, so this can never
    /// silently degrade into the plain-text org-seed probe above.
    #[tokio::test(flavor = "multi_thread")]
    async fn org_ingest_link_marks_survive_full_catalog() {
        use holon_pbt_core::capabilities::SutFocus;
        use holon_pbt_core::capabilities::SutQueryResults;
        use holon_pbt_core::capabilities::SutSqlProjection;
        use holon_pbt_core::composition::CapProvider;

        const TREE_ORG: &str = "#+ID: structural-page\n* parent\n:PROPERTIES:\n:ID: \
                                parent\n:END:\n* c1\n:PROPERTIES:\n:ID: c1\n:END:\n* See [[Linked \
                                Page]] here\n:PROPERTIES:\n:ID: c2\n:END:\n";
        const C2_RAW: &str = "See [[Linked Page]] here";

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        // Pin the boot clock to the fixed keystone day (2026-01-15) so the boot
        // auto-create rule mints a DETERMINISTIC journal day-page id
        // (`keystone_boot_journal_id`) — not one keyed on the host's real date.
        // The plain `new` boot uses the OS `SystemClock`, whose date-dependent
        // day-page the structural oracle never modeled → a date-dependent
        // `inv-viewmodel-entity-ids-subset-of-data` phantom (the day-page renders
        // in the journals feed but is unknown to the ref). Mirrors the
        // `full_headless_static_catalog_probe` recipe: pinned clock + awaited +
        // `seed_boot_journal`d day-page.
        let comp = Arc::new(
            HeadlessFrontendComponent::new_with_clock(
                &[("structural-page.org", TREE_ORG)],
                Duration::from_millis(300),
                false,
                crate::pbt::frontend_slice::components::keystone_boot_clock(),
            )
            .await,
        );
        let engine = comp.engine();
        let mut caps = CapMap::new();
        comp.clone().register(&mut caps);
        caps.insert(comp.clone() as Arc<dyn SutSqlProjection>);
        caps.insert(comp.clone() as Arc<dyn SutFocus>);
        caps.insert(comp.clone() as Arc<dyn SutQueryResults>);
        caps.replace(Arc::new(OpDispatchWriter::with_resolver(
            engine.clone(),
            resolver.clone(),
        )) as Arc<dyn SutBlockTreeWrite>);

        let ids = fixed_ids();
        let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
            .into_iter()
            .collect();

        // The boot journal auto-create fires ASYNC off the clock CDC; the 300ms
        // boot settle can return before the day-page lands. Await it (fail loud
        // on timeout) so the scaffold snapshot below deterministically captures
        // it and the widget-tree phantom check sees a settled, ref-known block.
        let journal_id = crate::pbt::frontend_slice::components::keystone_boot_journal_id();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            tokio::time::sleep(SETTLE).await;
            if sut_ids(&caps).await.contains(&journal_id) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "[org-link-marks] boot journal {journal_id} did not fire within budget"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let booted = sut_ids(&caps).await;
        let scaffold: BTreeSet<EntityUri> = booted.difference(&tree).cloned().collect();

        let mut oracle = structural_ref();
        // Model the boot-fired journal day-page in the oracle's block_state so
        // it enters `all_block_ids` — the ref-known universe the phantom check
        // subtracts. `inject_scaffold_seed` (below) only touches `block_documents`,
        // so WITHOUT this the auto-created day-page renders as an unknown
        // (phantom) entity id. Mirrors `frontend_wired`'s seed for the composed
        // keystone; the pinned keystone clock makes the SUT's day-page id match.
        crate::pbt::composed::wide_e2e::seed_boot_journal(&mut oracle);
        // c2 on disk spells a wiki link: the reference holds the org-lens fixed
        // point — label-stripped content + the extracted Link mark.
        let (c2_content, c2_marks) = crate::pbt::types::normalize_content_for_org_roundtrip(
            C2_RAW,
            holon_api::ContentType::Text,
        );
        assert!(
            c2_marks.as_ref().is_some_and(|m| !m.is_empty()),
            "non-vacuity: the org lens must extract a Link mark from {C2_RAW:?}, got {c2_marks:?}"
        );
        assert_eq!(c2_content, "See Linked Page here");
        let ref_c2_marks = c2_marks.clone();
        {
            let b = oracle
                .domain
                .block_state
                .blocks
                .get_mut(&ids.c2)
                .expect("seed block c2 present");
            b.content = c2_content;
            b.marks = c2_marks;
        }

        NavigateFocus {
            region: Region::Main,
            block_id: page_root(),
        }
        .apply_to_sut(&oracle, &mut caps)
        .await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold);

        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        for (id, msg) in report.failures() {
            eprintln!("[org-link-marks] FAIL {id}: {msg}");
        }
        assert!(
            report.ran_ids().contains(&"inv-blocks-match-ref/block_raw")
                || report.ran_ids().contains(&"inv-blocks-match-ref/matview")
                || report.ran_ids().contains(&"inv-blocks-match-ref/org"),
            "block invariants must SELECT (ran: {:?})",
            report.ran_ids()
        );
        assert!(
            report.failures().is_empty(),
            "org-ingested [[Linked Page]] must survive as block.marks in every store: {:?}",
            report.failures()
        );

        // Links increment 2 — keystone-machinery consistency arm: the SUT's
        // block_links junction must equal the links DERIVED FROM THE
        // REFERENCE's marks (shared oracle: holon_api::derive_block_links).
        // `Linked Page` names no existing page, so the row is DANGLING
        // (resolved_id NULL — lazy page creation, no placeholder) and the
        // backlinks matview stays empty.
        let expected: Vec<(String, String)> = holon_api::derive_block_links(
            ref_c2_marks
                .as_ref()
                .expect("ref c2 marks asserted Some above"),
        )
        .into_iter()
        .map(|l| (l.target, l.kind.as_str().to_string()))
        .collect();
        assert!(
            !expected.is_empty(),
            "non-vacuity: ref marks must derive at least one link"
        );
        let rows = engine
            .db_handle()
            .query(
                "SELECT target, kind, resolved_id FROM block_links WHERE source_block_id = \
                 'block:c2' ORDER BY target, kind",
                std::collections::HashMap::new(),
            )
            .await
            .expect("block_links query");
        let actual: Vec<(String, String)> = rows
            .iter()
            .map(|r| {
                assert!(
                    matches!(r.get("resolved_id"), None | Some(holon_api::Value::Null)),
                    "no page named 'Linked Page' exists — the link must stay dangling, got {r:?}"
                );
                (
                    r.get("target")
                        .and_then(|v| v.as_string())
                        .expect("target")
                        .to_string(),
                    r.get("kind")
                        .and_then(|v| v.as_string())
                        .expect("kind")
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            actual, expected,
            "SUT block_links must equal the reference-derived link set"
        );
        let backlink_rows = engine
            .db_handle()
            .query("SELECT id FROM backlinks", std::collections::HashMap::new())
            .await
            .expect("backlinks query");
        assert!(
            backlink_rows.is_empty(),
            "dangling links must not surface in the backlinks matview: {backlink_rows:?}"
        );
    }

    /// Apply `SplitBlock(c1)` to BOTH the oracle and the composed SUT,
    /// reconcile the minted ids, and run the catalog — the faithful
    /// structural write path over the real headless component stays green
    /// and the block invariants run non-vacuously.
    #[tokio::test(flavor = "multi_thread")]
    async fn frontend_structural_split_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let (mut caps, scaffold_ids) = boot_and_seed(&resolver).await;
        let mut oracle = structural_ref();

        let before = sut_ids(&caps).await;
        let split = SplitBlock {
            block_id: fixed_ids().c1,
            position: 1,
        };
        split.apply_to_ref(&mut oracle); // mints synthetic block::split-N
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await; // mints real uuid
        tokio::time::sleep(SETTLE).await;
        let after = sut_ids(&caps).await;

        // Reconcile the one synthetic ↔ one real id.
        let synthetic: Vec<EntityUri> = oracle
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| is_synthetic_ref_id(id))
            .cloned()
            .collect();
        let real_new: Vec<EntityUri> = after.difference(&before).cloned().collect();
        assert_eq!(synthetic.len(), 1, "one synthetic split id");
        assert_eq!(real_new.len(), 1, "one real minted id");
        let map: BTreeMap<EntityUri, EntityUri> =
            std::iter::once((synthetic[0].clone(), real_new[0].clone())).collect();
        let mut resolved = oracle.with_resolved_doc_uris(&map);
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);

        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "lockstep split must stay green over the headless component: {:?}",
            report.failures()
        );
        for id in REQUIRED_INVARIANTS {
            assert!(
                report.ran_ids().contains(id),
                "non-vacuity: {id} must run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Teeth: apply `SplitBlock(c1)` to the SUT ONLY (oracle NOT advanced), so
    /// the real minted block has no reconciled counterpart in the oracle.
    /// The block-set comparison MUST catch the spurious block — proving the
    /// write actually mutated the real store AND the invariant has teeth
    /// over the headless component.
    #[tokio::test(flavor = "multi_thread")]
    async fn frontend_structural_sut_only_split_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let (mut caps, scaffold_ids) = boot_and_seed(&resolver).await;
        let oracle = structural_ref();

        // Drive the split on the SUT only — DON'T advance the oracle, DON'T reconcile.
        let split = SplitBlock {
            block_id: fixed_ids().c1,
            position: 1,
        };
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;

        let block_raw_failed = report
            .failures()
            .iter()
            .any(|(id, _)| *id == "inv-blocks-match-ref/block_raw");
        assert!(
            block_raw_failed,
            "SUT-only split must be CAUGHT by inv-blocks-match-ref/block_raw (the minted block is \
             spurious vs the un-advanced oracle). Failures: {:?}",
            report.failures()
        );
    }

    /// Teeth for the `NavigateFocus` arm of the **wide** alphabet, in the
    /// FULL-catalog config (`boot_and_seed_wide` — the full frontend cap
    /// set). Drive a `NavigateFocus(journals)` on the SUT ONLY (oracle
    /// stays focused on the page root): the SUT's `current_focus` matview
    /// moves to `journals` while the oracle still holds the page, so
    /// `inv-navigation-focus` MUST `Fail`. Proves the focus invariant genuinely
    /// SELECTS and BITES here (not just runs vacuously) — the non-vacuity the
    /// random `frontend_wide_pbt` run relies on when `NavigateFocus` fires.
    /// The block/org invariants stay green (no block change), so this
    /// isolates the focus catch.
    ///
    /// The target is `block:journals`, a SIDEBAR-LISTED first-boot page — the
    /// faithful `SutFocusWrite` focus path is a LeftSidebar entry click, so
    /// the divergence target must be a sidebar-listed page (a child block
    /// like `c1` has no sidebar entry and cannot be focused this way).
    /// Divergence from the oracle's page-root focus is all this
    /// teeth needs; the specific page is incidental.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_sut_only_navigate_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = frontend_wired(structural_ref()); // focused on the page root
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        // SUT-only NavigateFocus to the sidebar-listed `journals` page — DON'T advance
        // the oracle (which stays on the structural-page root).
        let nav = NavigateFocus {
            region: Region::Main,
            block_id: EntityUri::parse("block:journals").expect("journals id"),
        };
        TransitionImpl::apply_to_sut(&nav, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;

        assert!(
            report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-navigation-focus"),
            "SUT-only NavigateFocus must be CAUGHT by inv-navigation-focus in the full-catalog \
             wide config; failures: {:?}, ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// `ToggleState` (mutate arm) over the composed frontend `CapMap`: toggling
    /// `c1`'s task_state to `TODO` on BOTH the oracle and the SUT in
    /// lockstep keeps the full catalog green — INCLUDING
    /// `inv-task-state-storage-coherence` (SQL↔Loro), which the
    /// composed SUT is the FIRST to ever run (`unimplemented!` on `E2ESut`).
    ///
    /// Task #4 done: the headless `SutMutate::toggle_state` drives the real
    /// `cycle_task_state` op (Loro authority doc → `block_raw` projection), and
    /// the composed Loro read cap is unified onto the frontend's authority
    /// doc — so the task_state write is visible to both the SQL and Loro
    /// read sides, in lockstep with `ToggleState::apply_to_ref`.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_toggle_state_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(structural_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        let toggle = ToggleState {
            block_id: fixed_ids().c1,
            new_state: CycleTarget::Todo,
        };
        toggle.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&toggle, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "lockstep ToggleState must stay green over the composed frontend CapMap: {:?}",
            report.failures()
        );
        assert!(
            report.ran_ids().contains(&"inv-blocks-match-ref/block_raw"),
            "non-vacuity: blocks-match must run (ran: {:?})",
            report.ran_ids()
        );
    }

    /// COVERAGE TOOTH (2026-07-05): the keystone's generator must PROPOSE
    /// `ToggleState` in a booted, page-focused, seeded state — the exact
    /// state the wide alphabet reaches after boot. The dogfood/latency
    /// triage found ToggleState NEVER fires in keystone draws even at
    /// `HOLON_PBT_WEIGHTS=ToggleState:200` (change-status had
    /// ZERO PBT coverage). This tooth pins the generator side: if
    /// `ToggleState::weighted_generator` rejects the canonical seeded ref, the
    /// keystone alphabet silently lost its only change-status transition.
    /// On failure it prints the precise rejection `Reason`s.
    #[test]
    fn toggle_state_generator_proposes_in_wide_seeded_state() {
        use holon_pbt_core::TransitionFactory;
        for (name, state) in [
            ("structural_ref", structural_ref()),
            ("wide_ref", crate::pbt::composed::wide_e2e::wide_ref()),
        ] {
            match ToggleState::weighted_generator(&state) {
                validated::Validated::Good((w, _strat)) => {
                    assert!(w > 0, "[{name}] ToggleState arm has zero weight");
                }
                validated::Validated::Fail(reasons) => {
                    drop_ref_off_thread(state);
                    panic!(
                        "[{name}] ToggleState generator REJECTS the seeded wide state — \
                         change-status has zero keystone coverage. Reasons: {reasons:?}"
                    );
                }
            }
            drop_ref_off_thread(state);
        }
    }

    /// Teeth: toggle `c1`'s task_state on the SUT ONLY (oracle frozen) — the
    /// SUT's `block_raw.properties.task_state` becomes `TODO` while the
    /// oracle's stays unset, so `inv-blocks-match-ref/block_raw` MUST
    /// `Fail`. Proves the `set_field` op actually mutated the store AND the
    /// property comparison has teeth.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_sut_only_toggle_state_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = frontend_wired(structural_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        // SUT-only toggle — DON'T advance the oracle.
        let toggle = ToggleState {
            block_id: fixed_ids().c1,
            new_state: CycleTarget::Todo,
        };
        TransitionImpl::apply_to_sut(&toggle, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-blocks-match-ref/block_raw"),
            "SUT-only ToggleState must be CAUGHT by inv-blocks-match-ref/block_raw; failures: \
             {:?}, ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// A fixed `AllBlocks` watch (the only shape `generate_test_query`
    /// produces), querying the columns the generator selects.
    fn all_blocks_watch(query_id: &str) -> crate::pbt::transitions::SetupWatch {
        use crate::pbt::query::QuerySource;
        use crate::pbt::query::QueryTable;
        use crate::pbt::query::TestQuery;
        crate::pbt::transitions::SetupWatch {
            query_id: query_id.to_string(),
            query: TestQuery {
                table: QueryTable::Blocks,
                columns: [
                    "id",
                    "content",
                    "content_type",
                    "source_language",
                    "source_name",
                    "parent_id",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
                predicates: vec![],
                source: QuerySource::AllBlocks,
            },
            language: holon_api::QueryLanguage::HolonSql,
        }
    }

    /// `SetupWatch` (B5 watch-query parity, task #5) over the composed frontend
    /// `CapMap`: register the SAME `AllBlocks` watch on BOTH the oracle and the
    /// SUT, run the catalog — the watch invariants stay green. The booted
    /// SUT's `AllBlocks` watch returns the scaffold/journals blocks (+ the
    /// page) that the hand-built oracle doesn't model as real blocks, and
    /// the oracle carries the phantom `started-ref-layout-query` seed block
    /// the SUT lacks; `inv-watch-rows-match-ref` must seed-exclude both
    /// sides (the same way `inv-blocks-match-ref` does) so only
    /// the non-seed working tree is compared. This is the last narrowing gating
    /// E3/E5.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_setup_watch_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(structural_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        let watch = all_blocks_watch("query-allblocks");
        watch.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&watch, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "lockstep SetupWatch must stay green over the composed frontend CapMap: {:?}",
            report.failures()
        );
        for id in ["inv-active-watches-match-ref", "inv-watch-rows-match-ref"] {
            assert!(
                report.ran_ids().contains(&id),
                "non-vacuity: {id} must run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Teeth: register the `AllBlocks` watch on BOTH sides, then `Split` `c1`
    /// on the SUT ONLY (oracle frozen). The split tail is a NON-seed user
    /// block that appears in the SUT's watch rows but not the oracle's
    /// expected rows, so `inv-watch-rows-match-ref` MUST `Fail` — proving
    /// the seed-exclusion does not over-mask a genuine user-row divergence
    /// (the watch invariant still has teeth on the working tree).
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_sut_only_watch_rows_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(structural_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        // Watch on BOTH sides (so the subscription set agrees) ...
        let watch = all_blocks_watch("query-allblocks");
        watch.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&watch, &oracle, &mut caps).await;

        // ... but split `c1` on the SUT ONLY — its split tail is a non-seed block the
        // oracle never learns about.
        let split = SplitBlock {
            block_id: fixed_ids().c1,
            position: 1,
        };
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-watch-rows-match-ref"),
            "SUT-only split must be CAUGHT by inv-watch-rows-match-ref; failures: {:?}, ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// Teeth: type `Z` into `c1` on BOTH the oracle and the composed editor
    /// SUT, run the catalog — the editor write path over the REAL headless
    /// editor (`HeadlessEditorMirror`) stays green, with committed-content
    /// + editor-text + caret parity all running non-vacuously. The positive
    /// direction of the keystone.
    #[tokio::test(flavor = "multi_thread")]
    async fn editor_type_chars_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = editor_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_editor(&resolver, &oracle).await;

        let typ = TypeChars {
            text: "Z".to_string(),
        };
        typ.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&typ, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "lockstep TypeChars must stay green over the composed editor CapMap: {:?}",
            report.failures()
        );
        // The committed-content + live editor parity invariants must all run (the
        // editor keystone's non-vacuity guard).
        for id in [
            "inv-blocks-match-ref/block_raw",
            "inv-block-content/block_raw",
            "inv-editor-text/mirror",
            "inv-editor-caret/mirror",
        ] {
            assert!(
                report.ran_ids().contains(&id),
                "non-vacuity: {id} must run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Teeth: type `Z` into `c1` on the SUT ONLY (oracle frozen) — the SUT's
    /// editor live text + committed `block_raw.content` become `c1Z` while
    /// the oracle stays `c1`, so the content + editor-text invariants MUST
    /// `Fail`. Proves the headless editor keystroke actually mutated the
    /// `MutableText` AND committed it to the projection (the keystone), and
    /// that the parity checks have teeth.
    #[tokio::test(flavor = "multi_thread")]
    async fn editor_sut_only_type_chars_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = editor_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_editor(&resolver, &oracle).await;

        // SUT-only type — DON'T advance the oracle.
        let typ = TypeChars {
            text: "Z".to_string(),
        };
        TransitionImpl::apply_to_sut(&typ, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().iter().any(|(id, _)| {
                *id == "inv-block-content/block_raw" || *id == "inv-editor-text/mirror"
            }),
            "SUT-only TypeChars must be CAUGHT by the content/editor-text parity; failures: {:?}, \
             ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// Teeth for the #3 split-then-type interleaving (the frontend
    /// focus-handoff fold). Split `c1`, then `TypeChars` DIRECTLY — with NO
    /// intervening `FocusEditableText`. The only thing that makes the
    /// keystroke land on the new block is the composed write's production
    /// focus-handoff (`OpDispatchWriter`'s `dispatch_intent_sync` →
    /// `apply_structural_focus`), which moves the SUT's `focused_block` onto
    /// the split-created block — exactly as `SplitBlock::apply_to_ref` does
    /// via `set_focus` + `open_active_editor`. Were the handoff absent (the
    /// old blur regime), this would panic ("no focused block") or type into
    /// the wrong block and the content/editor-text parity would `Fail`.
    /// Lockstep green here = split-then-type works on BOTH sides.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_split_then_type_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(wide_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        // Split `c1` at position 1 — the new block (the "1" tail of "c1") becomes the
        // focused, active editor at caret 0 on BOTH sides.
        let before = sut_ids(&caps).await;
        let split = SplitBlock {
            block_id: fixed_ids().c1,
            position: 1,
        };
        split.apply_to_ref(&mut oracle); // mints synthetic block::split-N, focuses + opens it
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await; // focus-handoff focuses real new
        tokio::time::sleep(SETTLE).await;
        let after = sut_ids(&caps).await;

        // Reconcile the one synthetic split id ↔ the one real minted id.
        let synthetic: Vec<EntityUri> = oracle
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| is_synthetic_ref_id(id))
            .cloned()
            .collect();
        let real_new: Vec<EntityUri> = after.difference(&before).cloned().collect();
        assert_eq!(synthetic.len(), 1, "one synthetic split id");
        assert_eq!(real_new.len(), 1, "one real minted id");
        let map: BTreeMap<EntityUri, EntityUri> =
            std::iter::once((synthetic[0].clone(), real_new[0].clone())).collect();

        // Type DIRECTLY into the (now-focused-via-handoff) new block — no
        // FocusEditableText.
        let typ = TypeChars {
            text: "Q".to_string(),
        };
        typ.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&typ, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&map);
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "split-then-type must stay green (#3 focus-handoff): {:?}",
            report.failures()
        );
        // The editor live-text + committed-content parity must RUN — proving the type
        // landed on the new block (active editor selected, not skipped).
        for id in ["inv-editor-text/mirror", "inv-block-content/block_raw"] {
            assert!(
                report.ran_ids().contains(&id),
                "non-vacuity: {id} must run over the split-then-typed block (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Perf-cliff guard: a booted, quiescent full_headless tree must contain NO
    /// `loading`/`unknown` nodes. `widget_tree_snapshot` treats those as
    /// still-resolving and pays its full cautious resample window (4×120 ms) on
    /// EVERY check while any exist — a permanently-pending node (e.g. a
    /// `ViewKind` whose `widget_name()` returns `None`, snapshot kind
    /// "unknown") silently made that the keystone's dominant wall-time cost
    /// (measured 2026-07-03: ~83% of the run). If this fails, name the kind in
    /// `view_model_to_snapshot` (like `Empty`/`Loading`) instead of letting it
    /// fall through to "unknown".
    #[tokio::test(flavor = "multi_thread")]
    async fn booted_widget_tree_has_no_pending_placeholders() {
        use holon_pbt_core::capabilities::SutRenderer;
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let bundle = compose_sut_seeded(
            &ComponentSet::full_headless(),
            &resolver,
            &[("structural-page.org", WIDE_TREE_ORG)],
            &[],
        )
        .await;
        let snap = bundle
            .caps
            .expect::<dyn SutRenderer>()
            .widget_tree_snapshot()
            .await;
        fn dump(
            n: &holon_pbt_core::capabilities::WidgetSnapshot,
            path: &str,
            out: &mut Vec<String>,
        ) {
            let p = format!("{path}/{}", n.kind);
            if n.kind == "loading" || n.kind == "unknown" {
                out.push(format!("{p} entity={:?} props={:?}", n.entity_id, n.props));
            }
            for c in &n.children {
                dump(c, &p, out);
            }
        }
        let mut out = Vec::new();
        dump(&snap, "", &mut out);
        assert!(
            out.is_empty(),
            "booted widget tree holds permanent loading/unknown placeholders — every \
             widget_tree_snapshot() will pay the full 4×120ms resample window: {out:?}"
        );
    }

    /// Regression guard for the link-label whitespace fix (`strip_link` trims
    /// the label at the parse boundary — product rule: link labels carry no
    /// leading/ trailing whitespace). Seeds `[[a ]]` (trailing inner space)
    /// and `[[ tl]]` (leading) and asserts EVERY observable agrees on the
    /// trimmed label — the live Loro editor cell (the content authority),
    /// the SQL `block_raw` projection, and the org re-render. Before the
    /// fix the editor cell held the untrimmed `"a "`/`" tl"` while
    /// block_raw/org trimmed to `"a"`/`"tl"`, which
    /// diverged `inv-editor-text/mirror` (found by a CASES=256 soak).
    #[tokio::test(flavor = "multi_thread")]
    async fn link_label_whitespace_consistent_across_observables() {
        use holon_pbt_core::capabilities::SutBackend;
        use holon_pbt_core::capabilities::SutEditorMirrorRead;
        use holon_pbt_core::capabilities::SutOrgRead;
        const PROBE_ORG: &str = "#+ID: probe-page\n* [[a ]]\n:PROPERTIES:\n:ID: trail\n:END:\n* \
                                 [[ tl]]\n:PROPERTIES:\n:ID: lead\n:END:\n";
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let bundle = compose_sut_seeded(
            &ComponentSet::full_headless(),
            &resolver,
            &[("probe-page.org", PROBE_ORG)],
            &[],
        )
        .await;
        let raw = bundle
            .caps
            .expect::<dyn SutBackend>()
            .block_raw_snapshot()
            .await;
        let org = bundle
            .caps
            .expect::<dyn SutOrgRead>()
            .org_block_snapshot()
            .await;
        let editor = bundle.caps.expect::<dyn SutEditorMirrorRead>();
        let content_of = |blocks: &[holon_api::Block], sid: &str| {
            blocks
                .iter()
                .find(|b| b.id.as_str() == format!("block:{sid}"))
                .map(|b| b.content_text().to_string())
        };
        for (sid, expected) in [("trail", "a"), ("lead", "tl")] {
            let block_raw = content_of(&raw, sid);
            let org_c = content_of(&org, sid);
            let editor_c = editor
                .editor_live_text(&holon_api::EntityUri::block(sid))
                .ok();
            assert_eq!(
                (block_raw.as_deref(), org_c.as_deref(), editor_c.as_deref()),
                (Some(expected), Some(expected), Some(expected)),
                "link-label {sid}: every observable (block_raw, org, live editor cell) must agree \
                 on the whitespace-trimmed label {expected:?} — a divergence means the editor \
                 Loro authority kept whitespace the projections trimmed"
            );
        }
    }

    /// PROBE (swap-config widening): does `compose_sut(full_headless())` —
    /// which adds the Loro PEER arm (`SutLoro` + the loro read caps,
    /// selecting the loro invariants) on top of the turso-frontend-editor
    /// cap set — run the FULL catalog GREEN on the static seeded tree? If
    /// the Loro arm reads a DIFFERENT doc than the frontend's Turso session
    /// writes, the loro invariants would see an empty/divergent tree. This
    /// static check (no transitions) isolates whether full_headless is a
    /// drivable swap config before wiring an alphabet over it. Prints the
    /// ran set + failures.
    #[tokio::test(flavor = "multi_thread")]
    async fn full_headless_static_catalog_probe() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let set = ComponentSet::full_headless();
        let bundle = compose_sut_seeded(
            &set,
            &resolver,
            &[("structural-page.org", WIDE_TREE_ORG)],
            &[],
        )
        .await;
        let mut caps = bundle.caps;

        // The oracle models the boot-fired journal day-block (`frontend_wired` →
        // `seed_boot_journal`): every full_headless (Turso) boot fires the seeded
        // daily-journal rule once, minting a day-block under `block:journals`. As a
        // REAL ref block it enters `all_block_ids` → the phantom-history universe,
        // so its `block_history` create is no longer a phantom. This probe seeds no
        // `Journals.org`, so the day-block never reaches the `/org` snapshot; it
        // folds into `scaffold` below (seed-classified via `inject_scaffold_seed`),
        // which excludes it from the block-set comparison exactly as before — while
        // still counting for the history universe (`all_block_ids` reads `blocks`
        // regardless of seed classification).
        let oracle = frontend_wired(wide_ref());

        // The rule fires ASYNC off the clock CDC; the boot settle can return before
        // it lands. Await the day-block before the scaffold snapshot / checks so a
        // not-yet-fired journal does not false-diverge; fail loud on timeout.
        let journal_id = crate::pbt::frontend_slice::components::keystone_boot_journal_id();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            tokio::time::sleep(SETTLE).await;
            if sut_ids(&caps).await.contains(&journal_id) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "[full_headless probe] boot journal {journal_id} did not fire within budget"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let ids = fixed_ids();
        let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
            .into_iter()
            .collect();
        let booted = sut_ids(&caps).await;
        let scaffold: BTreeSet<EntityUri> = booted.difference(&tree).cloned().collect();

        TransitionImpl::apply_to_sut(
            &NavigateFocus {
                region: Region::Main,
                block_id: page_root(),
            },
            &oracle,
            &mut caps,
        )
        .await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        eprintln!(
            "[full_headless probe] ran {} invariants: {:?}",
            report.ran_ids().len(),
            report.ran_ids()
        );
        for (id, msg) in report.failures() {
            eprintln!("[full_headless probe] FAIL {id}: {msg}");
        }
        assert!(
            report.failures().is_empty(),
            "full_headless static catalog must be green to be a drivable swap config; failures \
             above"
        );
    }

    /// DIAGNOSTIC (journals-machinery peer RED, 2026-07-11): is the
    /// programmatically-seeded journals display machinery (`::src::0` /
    /// `::render::0`) Loro-backed under a Loro wiring? The oracle's peer fork
    /// includes them (non-seed, non-page), so `ApplyMutation(LoroPeer)` draws
    /// them as update targets; `peer_update_block` then panics if the peer's
    /// forked doc (a snapshot of the GLOBAL doc) lacks them.
    #[tokio::test(flavor = "multi_thread")]
    async fn journals_machinery_is_loro_backed_under_loro_wiring() {
        use holon_pbt_core::capabilities::SutLoroLog;

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let bundle = compose_sut_seeded(
            &ComponentSet::full_headless(),
            &resolver,
            &[("structural-page.org", WIDE_TREE_ORG)],
            &[],
        )
        .await;
        let caps = bundle.caps;
        let kids = caps
            .expect::<dyn SutLoroLog>()
            .loro_children_of("block:journals")
            .await;
        assert!(
            kids.as_ref()
                .is_some_and(|k| k.iter().any(|c| c.contains("journals::src::0"))
                    && k.iter().any(|c| c.contains("journals::render::0"))),
            "journals display machinery must be Loro-backed (peer forks snapshot the global doc; \
             the oracle's peer model includes these blocks): got {kids:?}"
        );
    }

    /// Peer-merge sibling-order projection guard: two blocks concurrently
    /// peer-created under one parent must show the SAME order in the SQL
    /// projection (`sorted_children`, ORDER BY sort_key) as in Loro's tree
    /// fractional index (insertion order — the canonical order per the
    /// 2026-07-03 decision). Pre-fix the Loro fi never reached a distinct SQL
    /// `sort_key` (both rows tied → id-string fallback), so SQL showed
    /// [aaa, zzz] while Loro held [zzz, aaa]. Cheap deterministic regression
    /// guard for the projection gap, distinct from the PBT invariant.
    #[tokio::test(flavor = "multi_thread")]
    async fn peer_merge_sibling_order_sql_matches_loro() {
        use holon_pbt_core::capabilities::SutLoro;
        use holon_pbt_core::capabilities::SutLoroLog;
        use holon_pbt_core::capabilities::SutSqlProjection;

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let bundle = compose_sut_seeded(
            &ComponentSet::full_headless(),
            &resolver,
            &[("structural-page.org", WIDE_TREE_ORG)],
            &[],
        )
        .await;
        let caps = bundle.caps;

        {
            let loro = caps.expect::<dyn SutLoro>();
            loro.apply_add_peer().await;
            loro.apply_peer_create(0, Some("parent"), "created-first", "peer-zzz")
                .await;
            loro.apply_add_peer().await;
            loro.apply_peer_create(1, Some("parent"), "created-second", "peer-aaa")
                .await;
            loro.apply_sync_with_peer(1).await;
            loro.apply_sync_with_peer(0).await;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let loro_order = caps
            .expect::<dyn SutLoroLog>()
            .loro_children_of("block:parent")
            .await
            .expect("block:parent must be present in Loro");
        let sql_order: Vec<String> = caps
            .expect::<dyn SutSqlProjection>()
            .sorted_children(&EntityUri::block("parent"))
            .await
            .into_iter()
            .map(|u| u.to_string())
            .collect();

        assert_eq!(
            loro_order,
            vec!["block:peer-zzz".to_string(), "block:peer-aaa".to_string()],
            "Loro must hold insertion order (peer-zzz created first)"
        );
        assert_eq!(
            sql_order, loro_order,
            "SQL sorted_children must match Loro insertion order — a tie in sort_key means the \
             Loro fractional index never reached the SQL projection for the peer-merge path"
        );
    }

    /// E-solid walking skeleton: a SHADOW peer mesh — fresh docs, seeded with
    /// only the working-tree base strings, clock-padded to the PRODUCTION
    /// SUT's scalar lamport heights at each fork/sync boundary, driving the
    /// same logical ops through the same `multi_peer` helpers — must predict
    /// the production doc's peer-merge outcomes EXACTLY: tied-sibling order
    /// (op-id tie-break) and concurrent-text interleaving. The only values
    /// crossing SUT→shadow are lamport heights (clocks, not data).
    ///
    /// Mechanism parity is proven pure-loro in
    /// `holon_loro::multi_peer::clock_parity_spike` (incl. the negative
    /// control); THIS test proves it against the real production boot, whose
    /// primary carries org-scan/boot history the shadow never replays.
    #[tokio::test(flavor = "multi_thread")]
    async fn shadow_mesh_predicts_sut_peer_merge_exactly() {
        use holon::sync::multi_peer::create_block_with_id;
        use holon::sync::multi_peer::pad_to_height;
        use holon::sync::multi_peer::sync_docs_direct;
        use holon::sync::multi_peer::{self};
        use holon_pbt_core::capabilities::SutLoro;
        use holon_pbt_core::capabilities::SutLoroLog;

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let bundle = compose_sut_seeded(
            &ComponentSet::full_headless(),
            &resolver,
            &[("structural-page.org", WIDE_TREE_ORG)],
            &[],
        )
        .await;
        let caps = bundle.caps;
        let sut_loro = caps.expect::<dyn SutLoro>();
        let log = caps.expect::<dyn SutLoroLog>();
        let sut_height = || async {
            log.loro_lamport_height()
                .await
                .expect("live Loro doc must report a lamport height")
        };

        // Shadow universe: primary seeded with ONLY the working tree (same
        // base strings as WIDE_TREE_ORG), peers forked at clock-padded heights.
        let shadow_primary = multi_peer::init_doc(1);
        let page =
            create_block_with_id(&shadow_primary, None, "structural-page", "structural-page");
        // parent/c1/c2 are SIBLINGS under the page (WIDE_TREE_ORG is flat).
        create_block_with_id(&shadow_primary, Some(page), "parent", "parent");
        create_block_with_id(&shadow_primary, Some(page), "c1", "c1");
        create_block_with_id(&shadow_primary, Some(page), "c2", "c2");
        let mut shadow_peers: Vec<loro::LoroDoc> = Vec::new();
        let shadow_add_peer = |peers: &mut Vec<loro::LoroDoc>, h: u32| {
            pad_to_height(&shadow_primary, h);
            let doc = multi_peer::init_doc(100 + peers.len() as u64);
            doc.import(
                &shadow_primary
                    .export(loro::ExportMode::Snapshot)
                    .expect("shadow snapshot"),
            )
            .expect("shadow peer import");
            peers.push(doc);
        };

        // ── Script (s6 shape: reversed creation + lamport bumps + concurrent
        //    text on c1, driven on SUT and shadow in lockstep) ──
        let h = sut_height().await;
        shadow_add_peer(&mut shadow_peers, h);
        sut_loro.apply_add_peer().await;

        let h = sut_height().await;
        shadow_add_peer(&mut shadow_peers, h);
        sut_loro.apply_add_peer().await;

        let shadow_update = |idx: usize, sid: &str, content: &str| {
            let node = crate::pbt::peer_ops::find_node_by_stable_id(&shadow_peers[idx], sid)
                .unwrap_or_else(|| panic!("shadow peer {idx} lacks {sid}"));
            multi_peer::update_block(&shadow_peers[idx], node, content);
        };
        let shadow_create = |idx: usize, parent_sid: &str, content: &str, sid: &str| {
            let parent =
                crate::pbt::peer_ops::find_node_by_stable_id(&shadow_peers[idx], parent_sid)
                    .unwrap_or_else(|| panic!("shadow peer {idx} lacks {parent_sid}"));
            create_block_with_id(&shadow_peers[idx], Some(parent), content, sid);
        };

        shadow_update(1, "c1", "from-b");
        sut_loro.apply_peer_update(1, "c1", "from-b").await;
        shadow_update(0, "c1", "from-a");
        sut_loro.apply_peer_update(0, "c1", "from-a").await;
        for _ in 0..3 {
            shadow_update(1, "c2", "bump");
            sut_loro.apply_peer_update(1, "c2", "bump").await;
        }
        // HIGHER peer id creates FIRST — the reversed-creation tie shape.
        shadow_create(1, "parent", "b-block", "peer-b");
        sut_loro
            .apply_peer_create(1, Some("parent"), "b-block", "peer-b")
            .await;
        shadow_create(0, "parent", "a-block", "peer-a");
        sut_loro
            .apply_peer_create(0, Some("parent"), "a-block", "peer-a")
            .await;

        let h = sut_height().await;
        pad_to_height(&shadow_primary, h);
        sync_docs_direct(&shadow_primary, &shadow_peers[1]);
        sut_loro.apply_sync_with_peer(1).await;

        let h = sut_height().await;
        pad_to_height(&shadow_primary, h);
        sync_docs_direct(&shadow_primary, &shadow_peers[0]);
        sut_loro.apply_sync_with_peer(0).await;

        // ── Compare: the shadow's PREDICTION vs the production doc ──
        let strip = |v: Vec<String>| -> Vec<String> {
            v.into_iter()
                .map(|s| s.strip_prefix("block:").map(str::to_string).unwrap_or(s))
                .collect()
        };
        let sut_children = strip(
            log.loro_children_of("block:parent")
                .await
                .expect("block:parent present in SUT Loro"),
        );
        let shadow_tree = shadow_primary.get_tree(multi_peer::TREE_NAME);
        let shadow_parent =
            crate::pbt::peer_ops::find_node_by_stable_id(&shadow_primary, "parent").unwrap();
        let shadow_children: Vec<String> = shadow_tree
            .children(shadow_parent)
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                crate::pbt::peer_ops::read_node_stable_id(&shadow_primary, c)
                    .expect("shadow child stable id")
            })
            .collect();
        assert_eq!(
            strip(shadow_children),
            sut_children,
            "shadow mesh failed to predict the SUT's tied-sibling order"
        );

        let sut_c1 = log
            .loro_block_snapshot()
            .await
            .expect("loro snapshot")
            .into_iter()
            .find(|b| b.id.as_str() == "block:c1")
            .expect("c1 present")
            .content;
        let shadow_c1_node =
            crate::pbt::peer_ops::find_node_by_stable_id(&shadow_primary, "c1").unwrap();
        let shadow_c1 = multi_peer::read_text(&shadow_tree, shadow_c1_node);
        assert_eq!(
            shadow_c1, sut_c1,
            "shadow mesh failed to predict the SUT's merged text interleaving"
        );
    }

    /// E-solid walking skeleton #2 — CONCURRENT PRIMARY+PEER edit: the primary
    /// types into `c1` through the real editor path while peer 0 holds an
    /// unsynced `c1` edit; the sync merges them (the concurrent-merge case).
    /// The shadow mirrors the primary edit LAMPORT-EXACTLY (pad to the SUT
    /// height the edit will land at, then apply the same content change), so
    /// the shadow merge must reproduce the SUT's exact interleaving.
    #[tokio::test(flavor = "multi_thread")]
    async fn shadow_mesh_predicts_concurrent_primary_peer_merge() {
        use holon::sync::multi_peer::create_block_with_id;
        use holon::sync::multi_peer::pad_to_height;
        use holon::sync::multi_peer::sync_docs_direct;
        use holon::sync::multi_peer::{self};
        use holon_pbt_core::capabilities::SutLoro;
        use holon_pbt_core::capabilities::SutLoroLog;

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let bundle = compose_sut_seeded(
            &ComponentSet::full_headless(),
            &resolver,
            &[("structural-page.org", WIDE_TREE_ORG)],
            &[],
        )
        .await;
        let mut caps = bundle.caps;
        let mut oracle = frontend_wired(wide_ref());

        // Shadow universe (same flat working tree as WIDE_TREE_ORG).
        let shadow_primary = multi_peer::init_doc(1);
        let page =
            create_block_with_id(&shadow_primary, None, "structural-page", "structural-page");
        create_block_with_id(&shadow_primary, Some(page), "parent", "parent");
        create_block_with_id(&shadow_primary, Some(page), "c1", "c1");
        create_block_with_id(&shadow_primary, Some(page), "c2", "c2");

        let height = |caps: &CapMap| {
            let log = caps.expect::<dyn SutLoroLog>();
            async move {
                log.loro_lamport_height()
                    .await
                    .expect("live Loro doc must report a lamport height")
            }
        };

        // AddPeer (clock-padded fork on the shadow side).
        let h = height(&caps).await;
        pad_to_height(&shadow_primary, h);
        let shadow_peer = multi_peer::init_doc(100);
        shadow_peer
            .import(&shadow_primary.export(loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        caps.expect::<dyn SutLoro>().apply_add_peer().await;

        // Peer 0 edits c1 (unsynced) — mirrored on the shadow peer.
        {
            let node = crate::pbt::peer_ops::find_node_by_stable_id(&shadow_peer, "c1").unwrap();
            multi_peer::update_block(&shadow_peer, node, "peer-side");
        }
        caps.expect::<dyn SutLoro>()
            .apply_peer_update(0, "c1", "peer-side")
            .await;

        // PRIMARY edit through the real editor path: focus c1, type "Z".
        let ids = fixed_ids();
        TransitionImpl::apply_to_sut(
            &NavigateFocus {
                region: Region::Main,
                block_id: page_root(),
            },
            &oracle,
            &mut caps,
        )
        .await;
        let focus = FocusEditableText {
            block_id: ids.c1.clone(),
        };
        focus.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&focus, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        // Pad the shadow to the height the primary edit will land at, THEN
        // mirror it (lamport-exact), THEN let the SUT type.
        let h = height(&caps).await;
        pad_to_height(&shadow_primary, h);
        let typ = TypeChars {
            text: "Z".to_string(),
        };
        typ.apply_to_ref(&mut oracle);
        let oracle_c1 = oracle.domain.block_state.blocks[&ids.c1]
            .content_text()
            .to_string();
        {
            let node = crate::pbt::peer_ops::find_node_by_stable_id(&shadow_primary, "c1").unwrap();
            multi_peer::update_block(&shadow_primary, node, &oracle_c1);
        }
        TransitionImpl::apply_to_sut(&typ, &oracle, &mut caps).await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Guard: the oracle's predicted primary content must match the SUT's
        // committed content BEFORE the merge — otherwise the mechanism test
        // below would fail for an unrelated (editor-model) reason.
        let sut_c1_pre = caps
            .expect::<dyn SutLoroLog>()
            .loro_block_snapshot()
            .await
            .expect("loro snapshot")
            .into_iter()
            .find(|b| b.id.as_str() == "block:c1")
            .expect("c1 present")
            .content;
        assert_eq!(
            oracle_c1, sut_c1_pre,
            "pre-merge: oracle editor model diverged from the SUT commit"
        );

        // Sync peer 0 — the concurrent merge — clock-padded on the shadow.
        let h = height(&caps).await;
        pad_to_height(&shadow_primary, h);
        sync_docs_direct(&shadow_primary, &shadow_peer);
        caps.expect::<dyn SutLoro>().apply_sync_with_peer(0).await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        let sut_c1 = caps
            .expect::<dyn SutLoroLog>()
            .loro_block_snapshot()
            .await
            .expect("loro snapshot")
            .into_iter()
            .find(|b| b.id.as_str() == "block:c1")
            .expect("c1 present")
            .content;
        let shadow_tree = shadow_primary.get_tree(multi_peer::TREE_NAME);
        let shadow_c1_node =
            crate::pbt::peer_ops::find_node_by_stable_id(&shadow_primary, "c1").unwrap();
        let shadow_c1 = multi_peer::read_text(&shadow_tree, shadow_c1_node);
        drop_ref_off_thread(oracle);
        assert_eq!(
            shadow_c1, sut_c1,
            "shadow mesh failed to predict the concurrent primary+peer merge interleaving"
        );
    }

    /// Seam-rebuild SR-1 teeth (doc-uri-minting reconcile generalization).
    /// Drive `CreateDocument` over the composed frontend CapMap: the real
    /// `SutAppLifecycle::create_document` writes an empty org file, the watcher
    /// mints the page block, and the harness-style reconcile maps the
    /// oracle's synthetic `block:ref-doc-N` to the minted real id. Lockstep
    /// green proves the minted doc page participates symmetrically in
    /// `block_raw` on both sides — the doc-uri-minting case the old E2ESut
    /// `block_tree_post_action` CreateDocument arm handled, now generic.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_create_document_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(wide_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        let before = sut_ids(&caps).await;
        let cd = CreateDocument {
            file_name: "sr1-new-doc.org".to_string(),
        };
        cd.apply_to_ref(&mut oracle); // mints synthetic block:ref-doc-N page
        TransitionImpl::apply_to_sut(&cd, &oracle, &mut caps).await; // writes file → watcher mints page
        tokio::time::sleep(SETTLE).await;
        let after = sut_ids(&caps).await;

        // Reconcile the one synthetic doc-uri ↔ the one real minted page id (the
        // doc-uri generalization of the harness per-tick reconcile).
        let synthetic: Vec<EntityUri> = oracle
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| id.as_str().starts_with("block:ref-doc-"))
            .cloned()
            .collect();
        let real_new: Vec<EntityUri> = after.difference(&before).cloned().collect();
        assert_eq!(synthetic.len(), 1, "one synthetic doc-uri minted");
        assert_eq!(
            real_new.len(),
            1,
            "one real page block minted by the watcher"
        );
        let map: BTreeMap<EntityUri, EntityUri> =
            std::iter::once((synthetic[0].clone(), real_new[0].clone())).collect();

        let mut resolved = oracle.with_resolved_doc_uris(&map);
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "lockstep CreateDocument must stay green over the composed frontend CapMap (the \
             minted page must match on both sides): {:?}",
            report.failures()
        );
        assert!(
            report.ran_ids().contains(&"inv-blocks-match-ref/block_raw"),
            "non-vacuity: blocks-match must run over the new doc page (ran: {:?})",
            report.ran_ids()
        );
    }

    /// Teeth: create the doc on the SUT ONLY (oracle frozen) — the SUT mints a
    /// new page the un-advanced oracle doesn't have, so
    /// `inv-blocks-match-ref/block_raw` MUST `Fail`. Proves
    /// `create_document` actually wrote+ingested the doc AND the block
    /// comparison has teeth over the composed path.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_sut_only_create_document_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = frontend_wired(wide_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        // SUT-only create — DON'T advance the oracle, DON'T reconcile.
        let cd = CreateDocument {
            file_name: "sr1-spurious.org".to_string(),
        };
        TransitionImpl::apply_to_sut(&cd, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-blocks-match-ref/block_raw"),
            "SUT-only CreateDocument must be CAUGHT by inv-blocks-match-ref/block_raw (the minted \
             page is spurious vs the un-advanced oracle); failures: {:?}, ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// Nav-history fold teeth: pin `c1` to the right sidebar on BOTH oracle and
    /// SUT in lockstep — the focus-roots invariant runs over the composed
    /// nav-history drive and agrees. Proves `SutNavHistoryDrive::pin_block`
    /// lands the pin where the oracle's `open_pins` puts it (and that the
    /// boot history-id alignment in `wide_ref` is exact: the SUT-assigned
    /// pin id matches the oracle's predicted `next_history_id`).
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_pin_block_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(wide_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        let pin = PinBlock {
            region: Region::RightSidebar,
            block_id: fixed_ids().c1,
        };
        pin.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&pin, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "lockstep PinBlock must stay green over the composed nav-history drive: {:?}",
            report.failures()
        );
        assert!(
            report.ran_ids().contains(&"inv-focus-roots"),
            "non-vacuity: inv-focus-roots must run over the pinned block (ran: {:?})",
            report.ran_ids()
        );
    }

    /// Teeth: pin `c1` to the right sidebar on the SUT ONLY (oracle frozen) —
    /// the SUT's `focus_roots(right_sidebar)` gains the pin while the
    /// oracle's stays empty, so `inv-focus-roots` MUST `Fail`. Proves the
    /// headless pin op actually mutated the nav-history/focus matview AND
    /// the focus-roots comparison has teeth.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_sut_only_pin_block_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = frontend_wired(wide_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        let pin = PinBlock {
            region: Region::RightSidebar,
            block_id: fixed_ids().c1,
        };
        TransitionImpl::apply_to_sut(&pin, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-focus-roots"),
            "SUT-only PinBlock must be CAUGHT by inv-focus-roots; failures: {:?}, ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// Turso smell #1 reproduce (Outdent / top-level NULL parent_id): Indent
    /// `c2` under `c1` (depth 2), then Outdent `c2` back to the page
    /// (grandparent = the real page block, NOT no_parent). If smell #1 were
    /// live, the SUT would write a divergent parent for the
    /// outdented block. Lockstep green ⟹ smell #1 does not reproduce on the
    /// composed path (the page-rooted tree never outdents to the literal
    /// top level).
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_indent_outdent_roundtrip_lockstep() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(wide_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        for t in [
            E2ETransition::Indent(Indent {
                block_id: fixed_ids().c2,
            }),
            E2ETransition::Outdent(Outdent {
                block_id: fixed_ids().c2,
            }),
        ] {
            t.apply_to_ref(&mut oracle);
            TransitionImpl::apply_to_sut(&t, &oracle, &mut caps).await;
            tokio::time::sleep(SETTLE).await;
        }

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "Indent→Outdent roundtrip must stay green (smell #1 stale on the page-rooted tree): \
             {:?}",
            report.failures()
        );
    }

    /// Turso smell #2 reproduce (split-of-block-with-children →
    /// child-vs-sibling): Indent `c2` under `c1` (so `c1` HAS a child),
    /// then Split `c1`. The oracle makes the new block a SIBLING of `c1`
    /// (parent = page); if the Loro positional-placement smell is live, the
    /// SUT attaches it as a CHILD of `c1` → `inv-block-parent/block_raw` Fails.
    /// This is the decisive smell-#2 probe.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_indent_then_split_parent_lockstep() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(wide_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        let indent = E2ETransition::Indent(Indent {
            block_id: fixed_ids().c2,
        });
        indent.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&indent, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;

        let before = sut_ids(&caps).await;
        let split = E2ETransition::SplitBlock(SplitBlock {
            block_id: fixed_ids().c1,
            position: 1,
        });
        split.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;
        let after = sut_ids(&caps).await;

        // Reconcile the one synthetic split id ↔ the one real minted id.
        let synthetic: Vec<EntityUri> = oracle
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| is_synthetic_ref_id(id))
            .cloned()
            .collect();
        let real_new: Vec<EntityUri> = after.difference(&before).cloned().collect();
        assert_eq!(synthetic.len(), 1, "one synthetic split id");
        assert_eq!(real_new.len(), 1, "one real minted id");
        let map: BTreeMap<EntityUri, EntityUri> =
            std::iter::once((synthetic[0].clone(), real_new[0].clone())).collect();

        let mut resolved = oracle.with_resolved_doc_uris(&map);
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "split-of-block-with-children must stay green (smell #2): the new block must be a \
             SIBLING of c1 (parent = page), not c1's child. Failures: {:?}",
            report.failures()
        );
    }

    /// Lifecycle tooth + id-stability make-or-break: `SimulateRestart`
    /// re-triggers the FileSyncController watcher (file-touch) and
    /// re-parses the org tree. This must PRESERVE the block_raw id-set (the
    /// `:ID:` drawers on disk make re-parse id-stable) — if ids
    /// drifted, the full catalog would diverge. Restart on the SUT (oracle
    /// `apply_to_ref` is a no-op), then the catalog must stay green with
    /// the block invariants running non-vacuously.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_simulate_restart_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = frontend_wired(wide_ref());
        let (mut caps, _handle, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        let before = sut_ids(&caps).await;
        let restart = SimulateRestart;
        restart.apply_to_ref(&mut oracle); // no-op (blocks preserved)
        TransitionImpl::apply_to_sut(&restart, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;
        let after = sut_ids(&caps).await;
        assert_eq!(
            before, after,
            "[restart make-or-break] block_raw id-set must be UNCHANGED across simulate_restart \
             (re-parse must be :ID:-stable); before={before:?} after={after:?}"
        );

        let mut resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "lockstep SimulateRestart must stay green (id-stable re-parse): {:?}",
            report.failures()
        );
        assert!(
            report.ran_ids().contains(&"inv-blocks-match-ref/block_raw"),
            "non-vacuity: blocks-match must run after restart (ran: {:?})",
            report.ran_ids()
        );
    }

    /// Non-vacuity guard for the COMBINED alphabet (#2): prove the editor
    /// transitions actually GENERATE in `wide_aggregate` — else
    /// `frontend_wide_pbt` could pass "green" while never exercising the
    /// editor↔structural interplay it exists to cover (the CLAUDE.md
    /// "silently looks fine" trap). Samples the wide alphabet from
    /// `wide_ref()` and asserts `FocusEditableText` is offered (an editor can
    /// open), then applies it and asserts `TypeChars`/`DeleteBackward`
    /// become offered (keystrokes chain off an open editor) — alongside the
    /// structural arms.
    #[test]
    fn wide_combined_alphabet_includes_editor_transitions() {
        use proptest::strategy::Strategy;
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        let mut runner = TestRunner::deterministic();
        let base = wide_ref();
        let base_strat = wide_aggregate(&base);
        let mut base_variants = std::collections::BTreeSet::new();
        for _ in 0..400 {
            base_variants.insert(
                base_strat
                    .new_tree(&mut runner)
                    .unwrap()
                    .current()
                    .variant_name(),
            );
        }
        assert!(
            base_variants.contains("FocusEditableText"),
            "the combined alphabet must offer FocusEditableText (open an editor); got \
             {base_variants:?}"
        );
        // Structural arms feasible from focus-on-page (Join/Toggle need focus on a
        // specific child first, so they're not offered at the page-focused base state).
        // `CreateDocument` (seam-rebuild SR-1) is feasible from the started base state.
        // Nav-history transitions folded from the nav slice. NavigateBack is offered at
        // the base state (the aligned boot stack has cursor=1 → can_go_back).
        // NavigateForward is NOT (cursor at top), so it's asserted post-Back
        // below.
        for v in [
            "SplitBlock",
            "NavigateFocus",
            "CreateDocument",
            "NavigateHome",
            "PinBlock",
            "NavigateBack",
            "SimulateRestart",
        ] {
            assert!(
                base_variants.contains(v),
                "the combined alphabet must still offer {v}; got {base_variants:?}"
            );
        }

        // Open an editor on a focusable child, then the keystroke arms must appear.
        let mut opened = base.clone();
        FocusEditableText {
            block_id: fixed_ids().c1,
        }
        .apply_to_ref(&mut opened);
        let opened_strat = wide_aggregate(&opened);
        let mut opened_variants = std::collections::BTreeSet::new();
        for _ in 0..400 {
            opened_variants.insert(
                opened_strat
                    .new_tree(&mut runner)
                    .unwrap()
                    .current()
                    .variant_name(),
            );
        }
        assert!(
            opened_variants.contains("TypeChars") && opened_variants.contains("DeleteBackward"),
            "with an editor open the combined alphabet must offer TypeChars + DeleteBackward; got \
             {opened_variants:?}"
        );

        drop_ref_off_thread(base);
        drop_ref_off_thread(opened);
    }

    /// **PCG-5b: the production WIDE `E2ETransition` enum drives a composed
    /// `CapMap`.** The slice teeth above drive the *fine-grained*
    /// `SplitBlock` over the cap map; the payoff PCG-4 unlocked is that the
    /// whole-alphabet dispatch `<E2ETransition as
    /// TransitionImpl<ReferenceState, CapMap>>::apply_to_sut` — which
    /// requires `CapMap: SutHandle` — now runs over the composed SUT, exactly
    /// as it will when the wide PBT's SUT is swapped from `E2ESut` to a
    /// `CapMap`. We wrap the split in `E2ETransition`, set the cap gate's
    /// RHS via `with_cap_set` (and assert the gate would admit it), drive
    /// it through the wide enum in lockstep, reconcile, and the
    /// composed catalog stays green and non-vacuous.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_e2e_transition_drives_composed_capmap() {
        use crate::pbt::transitions::E2ETransition;

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let (mut caps, scaffold_ids) = boot_and_seed(&resolver).await;
        // The cap gate's RHS — wired exactly as the eventual E2ESut→CapMap swap will.
        let mut oracle = structural_ref().with_cap_set(caps.cap_set());

        let before = sut_ids(&caps).await;
        // Wrap the split in the WIDE production enum; dispatch goes through
        // `impl<S: SutHandle> TransitionImpl for E2ETransition` with `S = CapMap`.
        let split = E2ETransition::SplitBlock(SplitBlock {
            block_id: fixed_ids().c1,
            position: 1,
        });
        // The gate admits it: `SplitBlock`'s `[SutBlockTreeWrite]` is in the composed
        // cap set, so the wide alphabet would generate it (no absent-cap `expect`
        // panic).
        assert!(
            oracle.caps_available(&split.required_caps()),
            "the composed cap set must admit SplitBlock"
        );

        split.apply_to_ref(&mut oracle); // wide enum → SplitBlock::apply_to_ref (synthetic id)
        // The whole point: drive the WIDE enum over `&mut CapMap`.
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await;
        tokio::time::sleep(SETTLE).await;
        let after = sut_ids(&caps).await;

        // Reconcile the one synthetic ↔ one real id (same kernel as the slice).
        let synthetic: Vec<EntityUri> = oracle
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| is_synthetic_ref_id(id))
            .cloned()
            .collect();
        let real_new: Vec<EntityUri> = after.difference(&before).cloned().collect();
        assert_eq!(synthetic.len(), 1, "one synthetic split id");
        assert_eq!(real_new.len(), 1, "one real minted id");
        let map: BTreeMap<EntityUri, EntityUri> =
            std::iter::once((synthetic[0].clone(), real_new[0].clone())).collect();
        let mut resolved = oracle.with_resolved_doc_uris(&map);
        drop_ref_off_thread(oracle);
        inject_scaffold_seed(&mut resolved, &scaffold_ids);

        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "wide-enum split over the composed CapMap must stay green: {:?}",
            report.failures()
        );
        for id in REQUIRED_INVARIANTS {
            assert!(
                report.ran_ids().contains(id),
                "non-vacuity: {id} must run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Shared fixture for the embedded-page tests: boots a `Journals.org` shell
    /// with a non-seed Page heading (`test-date-page`) and a child note
    /// (`test-date-child`) under it, registers the frontend caps, and focuses
    /// the main panel on `block:journals` so the date page renders embedded.
    /// Returns `(comp, caps, journals, date_page, child)`.
    async fn setup_embedded_page_sut() -> (
        Arc<HeadlessFrontendComponent>,
        CapMap,
        EntityUri,
        EntityUri,
        EntityUri,
    ) {
        use holon_pbt_core::capabilities::CapRegion;
        use holon_pbt_core::capabilities::SutFocusWrite;

        let journals_org = concat!(
            "#+ID: journals\n",
            "* 2026-07-14 :Page:\n",
            ":PROPERTIES:\n",
            ":ID: test-date-page\n",
            ":END:\n",
            "A journal date page.\n",
            "** A note for the day\n",
            ":PROPERTIES:\n",
            ":ID: test-date-child\n",
            ":END:\n",
            "This child should be lazy-loaded.\n",
        );
        const STRUCTURAL_PAGE_ORG: &str = "#+ID: structural-page\n";

        let comp = Arc::new(
            HeadlessFrontendComponent::new(
                &[
                    ("Journals.org", journals_org),
                    ("structural-page.org", STRUCTURAL_PAGE_ORG),
                ],
                Duration::from_millis(600),
            )
            .await,
        );
        let mut caps = CapMap::new();
        comp.clone().register_non_gesture(&mut caps);
        comp.clone()
            .register_gesture_writes(&mut caps, comp.driver());
        caps.insert(comp.clone() as Arc<dyn SutSqlProjection>);
        tokio::time::sleep(SETTLE).await;

        let journals = holon_api::EntityUri::parse("block:journals").expect("journals id");
        let date_page = holon_api::EntityUri::block("test-date-page");
        let child = holon_api::EntityUri::block("test-date-child");

        // Navigate the SUT focus to journals so the date page appears in the
        // main panel.
        comp.apply_navigate_focus(CapRegion::Main, &journals).await;
        tokio::time::sleep(SETTLE).await;

        (comp, caps, journals, date_page, child)
    }

    /// The ref-model oracle matching [`setup_embedded_page_sut`]: seeds
    /// structural-page, models `test-date-page` as a non-seed page child of
    /// journals with `test-date-child` under it, and focuses Main on journals.
    fn embedded_page_ref(
        journals: &EntityUri,
        date_page: &EntityUri,
        child: &EntityUri,
    ) -> ReferenceState {
        use holon_pbt_core::capabilities::RefNavHistoryMut;

        let mut oracle = structural_ref();
        let mut date_block = Block::new_text(date_page.clone(), journals.clone(), "2026-07-14");
        date_block.set_page(true);
        oracle
            .domain
            .block_state
            .blocks
            .insert(date_page.clone(), date_block);
        oracle
            .domain
            .block_state
            .block_documents
            .insert(date_page.clone(), date_page.clone());
        let child_block = Block::new_text(child.clone(), date_page.clone(), "A note for the day");
        oracle
            .domain
            .block_state
            .blocks
            .insert(child.clone(), child_block);
        oracle
            .domain
            .block_state
            .block_documents
            .insert(child.clone(), date_page.clone());
        oracle.nav_focus(holon_api::Region::Main, journals);
        oracle
    }

    /// **Phase A GREEN (enforced): embedded page renders collapsed + lazy.**
    ///
    /// Boots the embedded-page topology, focuses Main on `block:journals`, then
    /// runs `inv-embedded-page-collapsed-lazy`. Both display prongs pass: (a)
    /// the `embedded_page` profile variant wraps the page in a collapsed
    /// `expand_toggle`, and (b) the holon_sql recursive CTE stops at non-root
    /// page boundaries so no descendants leak into the widget tree snapshot.
    ///
    /// The expand half (drive `set_block_expanded`, assert the SUT toggle
    /// reports expanded + children load) lives in the separate,
    /// `#[ignore]`d `embedded_page_expand_toggle_drives_expanded` — it is
    /// blocked on a design ruling (see that test).
    #[tokio::test(flavor = "multi_thread")]
    async fn embedded_page_renders_collapsed_and_lazy() {
        use holon_pbt_core::composition::CapInvariant;

        use crate::pbt::composed::invariants::embedded_page_collapsed_lazy;

        let (_comp, caps, journals, date_page, child) = setup_embedded_page_sut().await;

        let registry: Vec<Box<dyn CapInvariant>> = vec![embedded_page_collapsed_lazy::wire()];

        let resolved_a = {
            let oracle = embedded_page_ref(&journals, &date_page, &child);
            let resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
            drop_ref_off_thread(oracle);
            resolved
        };
        let report_a = run_with_seeded_ref(&registry, &caps, resolved_a).await;
        let ran_a: Vec<_> = report_a.ran_ids().into_iter().collect();
        assert!(
            ran_a
                .iter()
                .any(|id| *id == "inv-embedded-page-collapsed-lazy"),
            "Phase A (collapsed): inv-embedded-page-collapsed-lazy must select + run (ran: \
             {ran_a:?})"
        );
        let failures_a = report_a.failures();
        assert!(
            failures_a.is_empty(),
            "Phase A (collapsed): inv-embedded-page-collapsed-lazy must PASS — embedded page with \
             collapsed expand_toggle, no leaked descendants. Failures: {failures_a:?}"
        );
        eprintln!(
            "[embedded_page_renders_collapsed_and_lazy] Phase A GREEN: collapsed expand_toggle \
             present, no leaked descendants."
        );
    }

    /// **Phase B GREEN: drive expand on the embedded page (Option B store).**
    ///
    /// Drives `set_block_expanded` on the embedded page and asserts the SUT's
    /// rendered `expand_toggle` reports `expanded=true`. Green via the
    /// view-local expansion store (RATIFIED 2026-07-16, Option B):
    /// `set_block_expanded` records the intent in the engine's non-persistent
    /// `UiState.expanded_view`, and the `expand_toggle` shadow builder seeds
    /// its gate from it on rebuild — so the flip survives the fresh
    /// `widget_tree_snapshot()` even though embedded pages carry no `collapsed`
    /// document field.
    #[tokio::test(flavor = "multi_thread")]
    async fn embedded_page_expand_toggle_drives_expanded() {
        use holon_pbt_core::capabilities::RefToggleMut;
        use holon_pbt_core::composition::CapInvariant;

        use crate::pbt::composed::invariants::embedded_page_collapsed_lazy;

        let (comp, caps, journals, date_page, child) = setup_embedded_page_sut().await;

        let registry: Vec<Box<dyn CapInvariant>> = vec![embedded_page_collapsed_lazy::wire()];

        // Drive expand via SUT plumbing.
        comp.driver()
            .set_block_expanded(&date_page, true)
            .await
            .expect("set_block_expanded must succeed for embedded page toggle");
        tokio::time::sleep(Duration::from_millis(3000)).await;

        let resolved_b = {
            let mut oracle = embedded_page_ref(&journals, &date_page, &child);
            oracle.set_expanded_view_local(&date_page, true);
            let resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
            drop_ref_off_thread(oracle);
            resolved
        };
        let report_b = run_with_seeded_ref(&registry, &caps, resolved_b).await;
        let ran_b: Vec<_> = report_b.ran_ids().into_iter().collect();
        assert!(
            ran_b
                .iter()
                .any(|id| *id == "inv-embedded-page-collapsed-lazy"),
            "Phase B (expanded): inv-embedded-page-collapsed-lazy must select + run (ran: \
             {ran_b:?})"
        );
        let failures_b = report_b.failures();
        assert!(
            failures_b.is_empty(),
            "Phase B (expanded): inv-embedded-page-collapsed-lazy must PASS — embedded page \
             expanded, descendants permitted/present via lazy live_query. Failures: {failures_b:?}"
        );
        eprintln!(
            "[embedded_page_expand_toggle_drives_expanded] Phase B GREEN: expanded toggle \
             accepted, descendants permitted."
        );
    }

    /// **Journal Overview feed (DOGFOOD_MVP A2+A3).**
    ///
    /// The `block:journals` page's own feed (`::src::0` holon_sql +
    /// `::render::0`) lists its `Page`-tagged day-entries NEWEST-FIRST,
    /// each rendered as a DEFAULT-EXPANDED embedded page (via the
    /// `embedded_page_expanded` profile variant, keyed on the
    /// `expand_default` column + the `default_expanded` expand_toggle
    /// param), separated by a `divider()`. Prong (a): the feed snapshot has
    /// one expanded `expand_toggle` per day-entry in newest-first
    /// order, with one `divider` each. Prong (b): a plain embedded page under a
    /// DIFFERENT focus root (no `expand_default`) still renders COLLAPSED — the
    /// global default is unchanged; only the feed context expands.
    #[tokio::test(flavor = "multi_thread")]
    async fn journal_feed_expanded_newest_first_with_divider() {
        use holon_pbt_core::capabilities::CapRegion;
        use holon_pbt_core::capabilities::SutFocusWrite;
        use holon_pbt_core::capabilities::SutRenderer;

        // Journals doc: two Page day-entries (newest = 2026-07-15), each with a
        // child note under it. The child's parent is the day-entry, NOT journals,
        // so the feed query (parent_id == journals) never lists it directly.
        let journals_org = concat!(
            "#+ID: journals\n",
            "* 2026-07-14 :Page:\n",
            ":PROPERTIES:\n:ID: day-0714\n:END:\n",
            "Log for the 14th.\n",
            "** morning note\n",
            ":PROPERTIES:\n:ID: day-0714-child\n:END:\n",
            "* 2026-07-15 :Page:\n",
            ":PROPERTIES:\n:ID: day-0715\n:END:\n",
            "Log for the 15th.\n",
            "** evening note\n",
            ":PROPERTIES:\n:ID: day-0715-child\n:END:\n",
        );
        // A plain notebook Page holding a plain sub-page — the collapsed control.
        let plain_org = concat!(
            "#+ID: plain-doc\n",
            "* Notebook :Page:\n",
            ":PROPERTIES:\n:ID: plain-notebook\n:END:\n",
            "** A plain day :Page:\n",
            ":PROPERTIES:\n:ID: plain-page\n:END:\n",
            "Plain content.\n",
            "*** a child note\n",
            ":PROPERTIES:\n:ID: plain-child\n:END:\n",
        );

        let comp = Arc::new(
            HeadlessFrontendComponent::new(
                &[("Journals.org", journals_org), ("plain-doc.org", plain_org)],
                Duration::from_millis(600),
            )
            .await,
        );
        tokio::time::sleep(SETTLE).await;

        let journals = EntityUri::parse("block:journals").expect("journals id");

        // ── Prong (a): the journals feed, newest-first, expanded, with dividers.
        // Poll until the feed query has populated (the block_tags 'Page' rows +
        // CDC settle asynchronously; under parallel-test CPU load a single
        // snapshot can race ahead of that population and see an empty list).
        let mut feed = comp
            .widget_tree_for(&journals)
            .await
            .expect("journals page renders its own feed (::src::0 + ::render::0)");
        for _ in 0..40 {
            if feed.collect_by_kind("expand_toggle").len() >= 2 {
                break;
            }
            tokio::time::sleep(SETTLE).await;
            feed = comp
                .widget_tree_for(&journals)
                .await
                .expect("journals page renders its own feed (::src::0 + ::render::0)");
        }

        let toggles = feed.collect_by_kind("expand_toggle");
        let order: Vec<&str> = toggles
            .iter()
            .filter_map(|n| n.props.get("target_id").map(String::as_str))
            .collect();
        // Relative NEWEST-FIRST: 2026-07-15 must precede 2026-07-14. (The journal
        // auto-create rule may inject a further "today" entry ahead of both — a
        // still-newest-first extra — so assert relative order, not an exact set.)
        let pos_0715 = order.iter().position(|id| *id == "block:day-0715");
        let pos_0714 = order.iter().position(|id| *id == "block:day-0714");
        assert!(
            matches!((pos_0715, pos_0714), (Some(a), Some(b)) if a < b),
            "feed lists day-entries NEWEST-FIRST (ORDER BY content DESC): 0715 before 0714, got \
             order {order:?}: {feed:#?}"
        );
        for n in &toggles {
            assert_eq!(
                n.props.get("expanded").map(String::as_str),
                Some("true"),
                "feed entry {:?} must be DEFAULT-EXPANDED (embedded_page_expanded variant): {n:#?}",
                n.props.get("target_id"),
            );
        }
        let dividers = feed.collect_by_kind("divider");
        assert_eq!(
            dividers.len(),
            toggles.len(),
            "one divider() between/after each feed entry ({} entries): {feed:#?}",
            toggles.len(),
        );

        // ── Prong (b): a plain embedded page elsewhere stays COLLAPSED.
        let notebook = EntityUri::parse("block:plain-notebook").expect("notebook id");
        comp.apply_navigate_focus(CapRegion::Main, &notebook).await;
        tokio::time::sleep(SETTLE).await;
        let root = comp.widget_tree_snapshot().await;
        let plain_toggle = root
            .collect_by_kind("expand_toggle")
            .into_iter()
            .find(|n| n.props.get("target_id").map(String::as_str) == Some("block:plain-page"))
            .expect("plain embedded page renders an expand_toggle in the main panel");
        assert_eq!(
            plain_toggle.props.get("expanded").map(String::as_str),
            Some("false"),
            "a plain embedded page (no expand_default) stays COLLAPSED while feed entries expand: \
             {plain_toggle:#?}"
        );

        eprintln!(
            "[journal_feed_expanded_newest_first_with_divider] GREEN: feed newest-first \
             (0715,0714) expanded + dividers; plain embedded page collapsed."
        );
    }

    /// **dogfood #6 row 34 — RED-first repro pinning the OPEN architecture
    /// bug.**
    ///
    /// The journal feed (`block:journals::render::0` =
    /// `list(sortkey:"-content", item_template: column(render_entity(),
    /// divider()))` over `::src::0` which tags rows with `expand_default`)
    /// is UNREACHABLE via the app's navigation
    /// path. `apply_navigate_focus(Main, journals)` makes the main panel render
    /// `block:default-main-panel` — a query-source block with NO
    /// `render_source`, so `BlockDomain::render_expr_for` resolves the
    /// collection profile's `tree_view`
    /// (`assets/default/types/collection_profile.yaml`): a tree keyed
    /// on `sort_key`, `item_template = render_entity()`, level-0 forced to
    /// `page_title`. `render_entity` → `shared_render_entity_build` resolves
    /// the render PURELY from the entity PROFILE and NEVER consults a
    /// block's own `render_source`; only `render_expr_for`'s
    /// `has_render_source` arm does, and that arm is reached only by
    /// directly watching `block:journals` (which `widget_tree_for(&
    /// journals)` — the A2/A3 test path — does, but the app's
    /// focus navigation does not). So the day-entries render as generic
    /// `embedded_page` (collapsed, no `expand_default`) tree items sorted by
    /// `sort_key` — explaining ALL THREE dogfood symptoms at once (arrival/
    /// sort_key order instead of content DESC; no `divider()`; mixed/collapsed
    /// expansion instead of default-expanded).
    ///
    /// `#[ignore]` because it is RED on `main` and pins an OPEN architecture
    /// decision (how a focused Page delegates the main panel to its own
    /// `render_source`) — see docs/Testing/BugFunnel.md row 34. Compare the two
    /// dumps below: DIRECT (`widget_tree_for`) shows the real feed; MAIN-PANEL
    /// (the app path) does not. Remove `#[ignore]` once the delegation lands.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "RED on main: journal feed render_source unreachable via focus \
                navigation — open architecture bug (BugFunnel row 34)"]
    async fn journal_feed_via_main_panel_focus_shows_feed() {
        use holon_pbt_core::capabilities::CapRegion;
        use holon_pbt_core::capabilities::SutFocusWrite;
        use holon_pbt_core::capabilities::SutRenderer;

        let journals_org = concat!(
            "#+ID: journals\n",
            "* 2026-07-14 :Page:\n",
            ":PROPERTIES:\n:ID: day-0714\n:END:\n",
            "Log for the 14th.\n",
            "** morning note\n",
            ":PROPERTIES:\n:ID: day-0714-child\n:END:\n",
            "* 2026-07-15 :Page:\n",
            ":PROPERTIES:\n:ID: day-0715\n:END:\n",
            "Log for the 15th.\n",
            "** evening note\n",
            ":PROPERTIES:\n:ID: day-0715-child\n:END:\n",
        );

        let comp = Arc::new(
            HeadlessFrontendComponent::new(
                &[("Journals.org", journals_org)],
                Duration::from_millis(600),
            )
            .await,
        );
        tokio::time::sleep(SETTLE).await;

        let journals = EntityUri::parse("block:journals").expect("journals id");
        comp.apply_navigate_focus(CapRegion::Main, &journals).await;
        tokio::time::sleep(SETTLE).await;

        let root = comp.widget_tree_snapshot().await;

        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        for n in root.walk() {
            *kinds.entry(n.kind.clone()).or_default() += 1;
        }
        eprintln!("[repro] MAIN-PANEL (app path) widget kinds: {kinds:?}");
        for t in root.collect_by_kind("expand_toggle") {
            eprintln!(
                "[repro] MAIN-PANEL expand_toggle target={:?} expanded={:?}",
                t.props.get("target_id"),
                t.props.get("expanded")
            );
        }
        // Reference: DIRECT render of the journals page (the A2/A3 test path).
        let feed = comp
            .widget_tree_for(&journals)
            .await
            .expect("direct journals render");
        eprintln!(
            "[repro] DIRECT feed dividers={} expand_toggles={} (the feed the app SHOULD show)",
            feed.collect_by_kind("divider").len(),
            feed.collect_by_kind("expand_toggle").len(),
        );

        // The app's focus path MUST show the journals feed: default-expanded
        // embedded pages, newest-first, divider-separated. These fail today.
        let toggles = root.collect_by_kind("expand_toggle");
        let pos = |id: &str| {
            toggles
                .iter()
                .position(|t| t.props.get("target_id").map(String::as_str) == Some(id))
        };
        assert!(
            matches!((pos("block:day-0715"), pos("block:day-0714")), (Some(a), Some(b)) if a < b),
            "main panel must list day-entries NEWEST-FIRST (0715 before 0714): {root:#?}"
        );
        for t in &toggles {
            assert_eq!(
                t.props.get("expanded").map(String::as_str),
                Some("true"),
                "main-panel feed entries must be DEFAULT-EXPANDED: {t:#?}"
            );
        }
        assert_eq!(
            root.collect_by_kind("divider").len(),
            toggles.len(),
            "one divider() per feed entry in the main panel: {root:#?}"
        );
    }

    /// **Sidebar sort-order (dogfood phase-3 bug 4) — RED-first repro.**
    ///
    /// The left sidebar's seed query (`assets/default/index.org`
    /// `left_sidebar:: src::0`) declares `ORDER BY b.content ASC`, so the
    /// sidebar's page rows must render ALPHABETICALLY by content. But the
    /// sidebar render (`left_sidebar::render::0`) is a `tree(sortkey:
    /// col("sort_key"))`, and the tree builder re-sorts siblings by that
    /// key — silently OVERRIDING the query's `ORDER BY`. So pages render in
    /// `sort_key` (creation/ingest) order, not alphabetically.
    ///
    /// Repro seed: two sibling `Page` headings created NON-alphabetically —
    /// `zzz-...` first (lower `sort_key`), `aaa-...` second (higher
    /// `sort_key`). Content-ASC order is `[aaa, zzz]`; `sort_key` order is
    /// `[zzz, aaa]`. The declared query order therefore DIVERGES from the
    /// render sort. This test asserts the sidebar renders `aaa` before
    /// `zzz` (the declared order):
    ///   - RED on `sortkey: col("sort_key")` — renders `zzz` first (sort_key).
    ///   - GREEN once the sidebar render honors the declared content order
    ///     (`sortkey: col("content")`).
    ///
    /// @pbt kind harness
    /// @pbt covers sidebar-sort-order — the left sidebar's rendered page order
    /// must match its seed query's declared `ORDER BY content ASC`, not the
    /// `tree()` render's `sort_key` override.
    #[tokio::test(flavor = "multi_thread")]
    async fn sidebar_renders_pages_in_declared_content_order() {
        use holon_pbt_core::capabilities::SutRenderer;

        // Two sibling Page headings under a container doc-root, authored in
        // REVERSE-alphabetical order so file/ingest order (== sort_key) is the
        // opposite of content-ASC order.
        const ORG: &str = concat!(
            "#+ID: ssort-container\n",
            "* zzz-sidebar-zebra :Page:\n",
            ":PROPERTIES:\n:ID: ssort-zebra\n:END:\n",
            "* aaa-sidebar-apple :Page:\n",
            ":PROPERTIES:\n:ID: ssort-apple\n:END:\n",
        );

        let comp = Arc::new(
            HeadlessFrontendComponent::new(
                &[("SsortContainer.org", ORG)],
                Duration::from_millis(600),
            )
            .await,
        );
        tokio::time::sleep(SETTLE).await;

        let root = comp.widget_tree_snapshot().await;

        // Locate the left-sidebar subtree (scheme-agnostic match on the seeded
        // layout block id).
        let sidebar = root
            .walk()
            .find(|n| {
                n.entity_id
                    .as_deref()
                    .is_some_and(|e| e.contains("default-left-sidebar"))
            })
            .unwrap_or(&root);

        // The rendered page rows carry the page block id as entity_id, in
        // pre-order (render) order.
        let order: Vec<String> = sidebar.walk().filter_map(|n| n.entity_id.clone()).collect();
        let pos = |needle: &str| order.iter().position(|e| e.contains(needle));
        let apple = pos("ssort-apple");
        let zebra = pos("ssort-zebra");

        assert!(
            apple.is_some() && zebra.is_some(),
            "both seeded sidebar pages must render (apple={apple:?}, zebra={zebra:?}); \
             rendered sidebar entity order = {order:?}"
        );
        assert!(
            apple < zebra,
            "the sidebar must render pages in the seed query's declared ORDER BY content ASC \
             (aaa-sidebar-apple BEFORE zzz-sidebar-zebra), NOT the tree()'s sort_key/creation \
             order. Got apple@{apple:?} zebra@{zebra:?}; rendered sidebar entity order = {order:?}"
        );
    }

    // **Right-sidebar ordering ORACLE — locks the declared-sort semantics
    // (vault deliverable: "Oracle-lock the right-sidebar ordering
    // semantics — compile+execute covered; ORDER BY semantics are not").**
    //
    // The right sidebar shows PINNED block subtrees. Its backing query
    // (`assets/default/index.org` `default-right-sidebar::src::0`) declares
    // `... RETURN d ORDER BY fr.added_ts DESC, d.sort_key` — so pin ROOTS must
    // render MOST-RECENTLY-PINNED-FIRST (`added_ts DESC`), which is exactly what
    // the reference models (`RefBoot::pin_block` move-to-top by
    // `added_ts_logical`, ref_caps/boot.rs). But the render
    // (`default-right-sidebar::render::0`) is `tree(sortkey: col("sort_key"))`,
    // and `OutlineTree::from_rows` (render_eval.rs) sorts ALL rows — INCLUDING
    // level-0 roots — by that single `sort_key` before partitioning, silently
    // DISCARDING the query's `ORDER BY fr.added_ts DESC`. So pins render in
    // `sort_key` (document/ingest) order, not pin-recency order. Same class as
    // the left-sidebar bug (B, fixed) and BugFunnel F7 (journals feed ignores
    // declared ORDER BY / sortkey).
    //
    // TWO causes were escalated on lane 7. They are split into two tests:
    //
    // 1. REGION-LITERAL mismatch — FIXED. The seed GQL filtered `fr.region =
    //    'right'`, but `focus_pin` writes `navigation_history.region =
    //    Region::RightSidebar.as_str() = 'right_sidebar'`; SQL equality never
    //    matched, so the sidebar rendered EMPTY (prod + SUT). The composed keystone
    //    missed it because the ref interpreter (`pbt::query.rs` `gql_focus_region`)
    //    mirrored whatever literal the seed carried (both empty → agree). Fixed by
    //    canonicalizing the seed + `di/registration.rs` corpus to `'right_sidebar'`
    //    and making `gql_focus_region` PARSE the literal into the `Region` enum
    //    (unknown literal = loud panic, not a silently-empty filter). The
    //    `right_sidebar_renders_pins` presence prong below is the permanent
    //    regression guard (un-ignored, GREEN).
    //
    // 2. SORTKEY OVERRIDE — STILL OPEN (surfaces now that pins render). A
    //    single-column tree() `sortkey` CANNOT express "roots by `added_ts DESC`,
    //    descendants by `sort_key`", so honoring the query's declared sort is a
    //    render-DSL fork (per-level sortkey via the existing `rules` mechanism, OR
    //    making `OutlineTree` preserve the backing query's row order for roots)
    //    with codebase-wide consequences — escalated to Martin, hence the
    //    `#[ignore]` on the ordering prong
    //    (`right_sidebar_renders_pins_in_declared_added_ts_order`). Remove the
    //    `#[ignore]` once that ruling + fix land.

    /// Shared driver for the two right-sidebar oracles: seed a container doc
    /// with two sibling non-Page headings whose ingest order (== `sort_key`) is
    /// REVERSE-alphabetical (`zebra` first → lower `sort_key`, `apple` second →
    /// higher `sort_key`), pin `zebra` FIRST (added_ts=1) then `apple`
    /// (added_ts=2), and return the rendered right-sidebar entity order. The
    /// declared `added_ts DESC` order is `[apple, zebra]` — the OPPOSITE of the
    /// `sort_key` (ingest) order `[zebra, apple]` the render currently applies.
    async fn right_sidebar_pin_render_order() -> Vec<String> {
        use holon_pbt_core::capabilities::SutNavHistoryDrive;
        use holon_pbt_core::capabilities::SutRenderer;

        const ORG: &str = concat!(
            "#+ID: rsort-container\n",
            "* zzz-pin-zebra\n",
            ":PROPERTIES:\n:ID: rsort-zebra\n:END:\n",
            "* aaa-pin-apple\n",
            ":PROPERTIES:\n:ID: rsort-apple\n:END:\n",
        );

        let comp = Arc::new(
            HeadlessFrontendComponent::new(
                &[("RsortContainer.org", ORG)],
                Duration::from_millis(600),
            )
            .await,
        );
        tokio::time::sleep(SETTLE).await;

        let zebra = EntityUri::parse("block:rsort-zebra").expect("zebra id");
        let apple = EntityUri::parse("block:rsort-apple").expect("apple id");

        SutNavHistoryDrive::pin_block(comp.as_ref(), Region::RightSidebar, &zebra).await;
        tokio::time::sleep(SETTLE).await;
        SutNavHistoryDrive::pin_block(comp.as_ref(), Region::RightSidebar, &apple).await;
        tokio::time::sleep(SETTLE).await;

        // Render the FULL layout via the RECURSIVE snapshot (like the
        // left-sidebar oracle) — `widget_tree_for` uses the shallow
        // `interpret_pure`, whose nested `live_block`/tree rows stay
        // placeholders, so the right-sidebar pin rows never resolve. The nested
        // CDC settle can lag (esp. under concurrent-build CPU load), so
        // RE-SNAPSHOT until both pins appear or a generous deadline — the
        // "both render" precondition must not be a timing race, isolating the
        // real ORDER assertion (same pattern as the journal-feed oracle).
        let sidebar_order = |root: &holon_pbt_core::capabilities::WidgetSnapshot| -> Vec<String> {
            root.walk()
                .find(|n| {
                    n.entity_id
                        .as_deref()
                        .is_some_and(|e| e.contains("default-right-sidebar"))
                })
                .map(|s| s.walk().filter_map(|n| n.entity_id.clone()).collect())
                .unwrap_or_default()
        };
        let mut order: Vec<String> = Vec::new();
        for _ in 0..40 {
            let root = comp.widget_tree_snapshot().await;
            order = sidebar_order(&root);
            let has_both = order.iter().any(|e| e.contains("rsort-apple"))
                && order.iter().any(|e| e.contains("rsort-zebra"));
            if has_both {
                break;
            }
            tokio::time::sleep(SETTLE).await;
        }
        order
    }

    /// **Right-sidebar PRESENCE prong (region-literal regression guard).**
    ///
    /// The right sidebar's backing GQL (`default-right-sidebar::src::0`)
    /// filters `fr.region = 'right_sidebar'` — the canonical
    /// `Region::RightSidebar .as_str()` value `focus_pin` writes to
    /// `navigation_history.region` and the focus matview keys by. When the
    /// seed literal drifted to a bare `'right'`, SQL equality never matched
    /// and the right sidebar rendered ZERO pins in prod (lane-7
    /// region-literal bug). This asserts that BOTH pinned blocks actually
    /// render — a permanent RED the moment the seed literal, the
    /// `di/registration.rs` corpus, or the focus keying drifts off
    /// `Region::as_str()` again. GREEN once the literal is canonical.
    ///
    /// @pbt kind harness
    /// @pbt covers right-sidebar-region-literal — pinned blocks must render in
    /// the right sidebar (the seed region filter must equal the value
    /// `focus_pin` writes, `Region::RightSidebar.as_str()`).
    #[tokio::test(flavor = "multi_thread")]
    async fn right_sidebar_renders_pins() {
        let order = right_sidebar_pin_render_order().await;
        let pos = |needle: &str| order.iter().position(|e| e.contains(needle));
        let apple_pos = pos("rsort-apple");
        let zebra_pos = pos("rsort-zebra");
        assert!(
            apple_pos.is_some() && zebra_pos.is_some(),
            "both pinned blocks must render in the right sidebar (apple={apple_pos:?}, \
             zebra={zebra_pos:?}); rendered right-sidebar entity order = {order:?}. An EMPTY \
             sidebar means the seed region filter drifted off Region::RightSidebar.as_str() \
             ('right_sidebar')."
        );
    }

    /// @pbt kind harness
    /// @pbt covers right-sidebar-sort-order — the right sidebar's rendered pin
    /// order must match its query's declared `ORDER BY fr.added_ts DESC`
    /// (pin-recency), not the `tree()` render's `sort_key` override.
    #[tokio::test(flavor = "multi_thread")]
    async fn right_sidebar_renders_pins_in_declared_added_ts_order() {
        let order = right_sidebar_pin_render_order().await;
        let pos = |needle: &str| order.iter().position(|e| e.contains(needle));
        let apple_pos = pos("rsort-apple");
        let zebra_pos = pos("rsort-zebra");

        assert!(
            apple_pos.is_some() && zebra_pos.is_some(),
            "both pinned blocks must render in the right sidebar (apple={apple_pos:?}, \
             zebra={zebra_pos:?}); rendered right-sidebar entity order = {order:?}"
        );
        assert!(
            apple_pos < zebra_pos,
            "the right sidebar must render pins in the query's declared ORDER BY fr.added_ts DESC \
             (aaa-pin-apple pinned LAST → FIRST, before zzz-pin-zebra), NOT the tree()'s sort_key/ \
             ingest order. Got apple@{apple_pos:?} zebra@{zebra_pos:?}; rendered order = {order:?}"
        );
    }
}
