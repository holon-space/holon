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
    let comp = Arc::new(
        HeadlessFrontendComponent::new_with_loro(
            &[("structural-page.org", WIDE_TREE_ORG)],
            Duration::from_millis(300),
            true,
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
        const COMPANION_ORG: &str = "#+ID: my-notes\n* child-note :Page:\n:PROPERTIES:\n:ID: \
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
        const COMPANION_ORG: &str = "#+ID: my-notes\n* child-note :Page:\n:PROPERTIES:\n:ID: \
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
        let comp = Arc::new(
            HeadlessFrontendComponent::new(
                &[("structural-page.org", TREE_ORG)],
                Duration::from_millis(300),
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
        let booted = sut_ids(&caps).await;
        let scaffold: BTreeSet<EntityUri> = booted.difference(&tree).cloned().collect();

        let oracle = structural_ref();
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
        let comp = Arc::new(
            HeadlessFrontendComponent::new(
                &[("structural-page.org", TREE_ORG)],
                Duration::from_millis(300),
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
        let booted = sut_ids(&caps).await;
        let scaffold: BTreeSet<EntityUri> = booted.difference(&tree).cloned().collect();

        let mut oracle = structural_ref();
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

        let ids = fixed_ids();
        let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
            .into_iter()
            .collect();
        let booted = sut_ids(&caps).await;
        let scaffold: BTreeSet<EntityUri> = booted.difference(&tree).cloned().collect();

        let oracle = wide_ref();
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

    /// **Phase 1 RED: embedded page renders collapsed + lazy-loaded.**
    ///
    /// Boots a custom topology where a non-seed page (`test-date-page`) sits
    /// under `block:journals` with a child block (`test-date-child`), focuses
    /// the main panel on `block:journals`, then runs
    /// `inv-embedded-page-collapsed-lazy`. The invariant RED-fails because
    /// today embedded pages render eagerly — the child appears in the main
    /// panel widget tree with no collapsed `expand_toggle` marking the page.
    ///
    /// This is the Phase 1 RED oracle. It currently FAILS (expected). Phases
    /// 2+3 will make it green by implementing the display/query fix.
    #[tokio::test(flavor = "multi_thread")]
    async fn embedded_page_renders_collapsed_and_lazy() {
        use holon_pbt_core::capabilities::CapRegion;
        use holon_pbt_core::capabilities::RefNavHistoryMut;
        use holon_pbt_core::capabilities::SutFocusWrite;
        use holon_pbt_core::composition::CapInvariant;

        use crate::pbt::composed::invariants::embedded_page_collapsed_lazy;

        // Org files: a Journals.org shell with a non-seed Page heading and
        // a child note under it.
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

        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let comp = Arc::new(
            HeadlessFrontendComponent::new(
                &[("Journals.org", journals_org), ("structural-page.org", STRUCTURAL_PAGE_ORG)],
                Duration::from_millis(600),
            )
            .await,
        );
        let _engine = comp.engine();
        let mut caps = CapMap::new();
        comp.clone().register_non_gesture(&mut caps);
        comp.clone().register_gesture_writes(&mut caps, comp.driver());
        caps.insert(comp.clone() as Arc<dyn SutSqlProjection>);
        tokio::time::sleep(SETTLE).await;

        // Build the ref: seed structural-page, model test-date-page as a
        // non-seed page child of journals, with test-date-child as its child.
        let mut oracle = structural_ref();
        let journals = holon_api::EntityUri::parse("block:journals").expect("journals id");
        let date_page = holon_api::EntityUri::block("test-date-page");
        let child = holon_api::EntityUri::block("test-date-child");

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

        // Navigate the SUT focus to journals so the date page appears in
        // the main panel.
        comp.apply_navigate_focus(CapRegion::Main, &journals).await;
        tokio::time::sleep(SETTLE).await;

        // Also navigate the ref so focus roots include journals.
        oracle.nav_focus(holon_api::Region::Main, &journals);

        let resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);

        let registry: Vec<Box<dyn CapInvariant>> = vec![embedded_page_collapsed_lazy::wire()];
        let report = run_with_seeded_ref(&registry, &caps, resolved).await;

        let ran: Vec<_> = report.ran_ids().into_iter().collect();
        assert!(
            ran.iter().any(|id| *id == "inv-embedded-page-collapsed-lazy"),
            "inv-embedded-page-collapsed-lazy must select + run (ran: {ran:?})"
        );
        let failures = report.failures();
        assert!(
            !failures.is_empty(),
            "Phase 1 expected RED (inv-embedded-page-collapsed-lazy fails) but got green. \
             If embedded pages are ALREADY collapsed+lazy-loaded, the fix may already be in place."
        );
        eprintln!(
            "[embedded_page_renders_collapsed_and_lazy] RED (expected in Phase 1): failures = {failures:?}"
        );
    }
}
