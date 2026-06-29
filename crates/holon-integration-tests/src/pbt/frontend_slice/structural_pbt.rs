//! **F2 E3 Phase C2.0 — the generic reconcile+settle loop on the SUT-swap target.**
//!
//! Drive the production *structural* alphabet (Split/Join/Indent/Outdent) through a
//! composed `CapMap` hosted on the REAL [`HeadlessFrontendComponent`] (the windowless
//! production `FrontendSession` over Turso that will replace `E2ESut` as the
//! `general_e2e_pbt` SUT), checked by the shared composed-invariant catalog against
//! the live `ReferenceState` oracle.
//!
//! Unlike the `memory_slice` structural PBT — which keeps ids in lockstep with a
//! `MemoryBackend`'s `set_next_split_id` hint — the headless component runs the real
//! Turso `split_block` op, which mints a fresh **real** `uuid` per split (not a
//! hintable id). So this slice carries the spike's [`IdResolver`] reconcile kernel:
//! per tick, diff the SUT `block_raw` id-set before/after the transition, pair the
//! one freshly-minted real id against the oracle's one freshly-minted synthetic
//! `block::split-N`, and accumulate the `synthetic → real` map. At check time the
//! oracle is `with_resolved_doc_uris`-remapped into SUT id space. This is the FIRST
//! reconcile-based structural SUT on the live (non-spike) component — the spike
//! proved the kernel over a bare engine; here it runs over the full production boot.
//!
//! **Scaffold seed-injection.** The full production boot leaves ~13 scaffold blocks
//! (`__default__`, the layout/sidebar tree + their PRQL query children, `journals`,
//! the booted org doc) that the spike's bare engine never has. The id-set-exact
//! `compare_block_subset` would count them on the SUT side, so each booted id is
//! injected into the oracle as `block_documents[id]=no_parent` — joining
//! `seed_block_ids()` and filtering out of the SUT snapshot, reducing the comparison
//! to the working `{parent,c1,c2}(+split)` tree on both sides. (Headless analog of
//! E1 `SutOrgRead` seeding the oracle from booted blocks; proven by
//! `components::tests::headless_structural_seed_and_reconcile_probe`.)
//!
//! Scope = the 4 reparenting structural transitions. MoveUp/MoveDown are gated out
//! of generation for the same sibling-*order* reason the `memory_slice` documents
//! (no invariant compares child order; the store's order can drift from the oracle's
//! canonical `sequence()`+id order). The editor/focus caps are NOT wired: the
//! minimal capmap hosts only `SutBackend` + `SutBlockTreeWrite`, so the focus/editor
//! invariants deselect and a focused oracle (needed so the generators have editable
//! candidates) never false-REDs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use holon_api::{Block, EntityUri, Region};
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::{ComponentSet, TransitionImpl, TransitionRef, weighted_arm};
use proptest::prelude::Just;
use proptest::strategy::{BoxedStrategy, Strategy, Union};
use proptest_state_machine::{ReferenceStateMachine, prop_state_machine};
use validated::Validated;

use crate::pbt::composed::builder::compose_sut_seeded;
use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::composed::harness::{ComposedSlice, ComposedSut, inject_scaffold_seed, sut_ids};
use crate::pbt::composed::seed_primitives::{C1, C2, PARENT, fixed_ids};
use crate::pbt::composed::subsystem_seed::{build_started_ref, run_with_seeded_ref};
// THE SWAP machinery, relocated to the `pbt`-gated `composed::wide_e2e` (single source
// of truth) so the `tests/` integration test can drive it; the lib slices/teeth here
// consume it: page_root/SETTLE/WIDE_TREE_ORG/structural_ref{,_wired}/wide_ref/
// boot_and_seed_wide/WIDE_REQUIRED_INVARIANTS/full_headless_cap_set/wide_e2e_ref/WideE2E{,Machine}.
use crate::pbt::composed::wide_e2e::{
    SETTLE, WIDE_TREE_ORG, boot_and_seed_wide, page_root, structural_ref, wide_e2e_ref, wide_ref,
};
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::is_synthetic_ref_id;
use crate::pbt::op_write_cap::{IdResolver, OpDispatchWriter};
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::sql_slice::SqlProjectionComponent;
use crate::pbt::transitions::toggle_state::CycleTarget;
use crate::pbt::transitions::{
    CreateDocument, DeleteBackward, E2ETransition, FocusEditableText, Indent, JoinBlock,
    NavigateBack, NavigateFocus, NavigateForward, NavigateHome, Nothing, Outdent, PinBlock,
    SimulateRestart, SplitBlock, ToggleState, TypeChars,
};
use holon_pbt_core::capabilities::{
    SutBackend, SutBlockTreeWrite, SutEditorMirrorRead, SutQueryResults, SutSqlProjection,
};
use holon_pbt_core::composition::CapProvider;

/// The structural slice's transition alphabet — `Split` (the id-minting transition
/// that drives the reconcile loop, the point of C2.0) + `Join`, each binding only
/// `S: SutBlockTreeWrite`, so the aggregate dispatches against a composed `CapMap`
/// without needing `SutHandle`.
///
/// **Why only Split + Join over leaf siblings** (the working blocks are direct
/// children of a page root, and stay leaves because no transition nests them):
/// - `Outdent` (and any split of a `no_parent` block) moves a block to the top
///   level, where the production Turso `split_block`/`outdent` op writes a literal
///   `NULL` `parent_id` (whereas the bootstrap writes the `sentinel:no_parent`
///   string), which `Block::try_from` rejects when reading `block_raw`.
///   `MemoryBackend` tolerates it, so the `memory_slice` never hit it. **Real
///   store/bootstrap inconsistency to FILE.** Avoided here: the page root keeps the
///   working blocks off the top level and is never itself split.
/// - `Indent` nests a block under its previous sibling, turning a leaf into a
///   parent-with-children. Splitting a block *with children* then diverges: Turso
///   makes the new block a **child** of the split block; the oracle makes it a
///   **sibling**. **Second real divergence to FILE.** Avoided here by excluding
///   `Indent` so every candidate stays a leaf.
///
/// Both are documented follow-on investigations (like the `go_back` smell), not
/// blockers for the reconcile-loop keystone.
#[derive(Clone, Debug)]
enum StructTransition {
    SplitBlock(SplitBlock),
    JoinBlock(JoinBlock),
    /// No-op fallback so `transitions()` never returns an empty strategy: a Join
    /// sequence can collapse the focus root's editable descendants to none (then no
    /// structural transition applies). Mirrors the spike's `SqlTransition::Nothing`.
    /// The invariants still run this tick (the catalog re-checks every step), so it
    /// costs nothing but robustness.
    Nothing,
}

/// The seeded oracle: the started `parent/c1/c2` blocks (NON-seed → compared every
/// tick) re-rooted as **leaf siblings** directly under a seed `page_root`, with focus
/// on the page root. The page root is the focus container so its children are the
/// `main_editable_descendants` candidates, but it is itself a page (excluded from
/// candidates) and a seed (excluded from the comparison) — so it is never split and
/// its page-ness never compared. With the working blocks as leaf siblings and no
/// nesting transition (Indent excluded), every candidate stays a leaf: `Split` lands
/// a new leaf sibling under the page (a real id, never `no_parent`), `Join` merges a
/// leaf into its previous sibling. No UI subsystem is wired, so `build_started_ref`
/// seeds no editor; the focus nav is the only UI state, and the minimal capmap hosts
/// no focus caps so it never false-REDs.
/// Invariants that MUST run each tick — the non-vacuity guard so "green" means "ran
/// over real data", not "deselected everything".
const REQUIRED_INVARIANTS: &[&str] = &[
    "inv-no-orphan-blocks",
    "inv-no-parent-cycles",
    "inv-blocks-match-ref/block_raw",
    "inv-block-parent-matches-ref/block_raw",
];

/// One arm per structural transition via the shared `weighted_arm` over the SAME
/// generic `TransitionFactory<ReferenceState>` impls the wide PBT uses.
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

/// Boot the windowless production session, seed the page-rooted leaf-sibling tree
/// via the production create op, and build the minimal structural capmap
/// (`SutBackend` + the `resolver`-sharing writer). Returns the capmap plus the booted
/// scaffold ids (to seed-inject into the oracle). Shared by the StateMachineTest
/// `init_test` and the teeth so they exercise the exact same SUT-swap target.
async fn boot_and_seed(resolver: &IdResolver) -> (CapMap, BTreeSet<EntityUri>) {
    let comp = Arc::new(
        HeadlessFrontendComponent::new(
            &[("doc0.org", "#+ID: ref-doc-0\n* Doc zero\n")],
            Duration::from_millis(300),
        )
        .await,
    );
    let engine = comp.engine();

    // Capture the booted scaffold ids (everything present BEFORE the working tree) —
    // these become the oracle's seed set so they filter out of the SUT-side id
    // comparison.
    let scaffold_ids: BTreeSet<EntityUri> = {
        let mut c = CapMap::new();
        c.insert(comp.clone() as Arc<dyn SutBackend>);
        sut_ids(&c)
            .await
            .into_iter()
            .filter(|id| !is_synthetic_ref_id(id))
            .collect()
    };

    // Seed the page-rooted tree: `page_root` (under `no_parent`) → `parent`,`c1`,`c2`
    // as LEAF SIBLINGS. The page root keeps candidates off the top level (so `Split`
    // never writes a `no_parent` block) and, being a page, is never itself a split
    // target. Filtered from the comparison by the same seed-injection as the scaffold.
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
/// (`StructTransition`/`StructMachine`), the seed (`boot_and_seed`), and the per-tick
/// dispatch. Everything else (the runtime, the `IdResolver` reconcile, the
/// scaffold-injection + catalog check) is the generic [`ComposedSut`] harness's.
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
// WIDE alphabet generator (`wide_aggregate`) — retained for the `teeth` tests below.
//
// The standalone wide-frontend swap PBT (`WideFrontend`/`WideMachine`/
// `frontend_wide_pbt`) that this generator once drove has been DELETED as a
// redundant frontend variant of the ONE PBT `general_e2e_composed_pbt`
// (`ComposedSut<WideE2E>`), whose subsystem-config draw already covers the full
// frontend cap set + catalog. `wide_aggregate` survives only because the lockstep
// `teeth` tests below still use it to build single-transition alphabets.
// ═════════════════════════════════════════════════════════════════

/// One arm per drivable wide transition, wrapped in the production `E2ETransition`
/// enum (vs `FrontendStructural`'s bespoke `StructTransition`). Same `weighted_arm`
/// over the same generic factories the wide PBT uses.
///
/// `NavigateFocus` joins the structural pair: it's total, mints no blocks (the
/// reconcile is a clean no-op), and its target is drawn by the production generator
/// from the oracle's focusable descendants — so the SUT and oracle navigate in
/// lockstep and the focus matviews stay aligned. This exercises the focus/nav
/// invariants DYNAMICALLY (multiple navigations across a sequence), integrated with
/// the block/org/viewmodel checks each tick — the integration the swap needs, beyond
/// the navigation slice's focus-only check. `NavigateBack/Forward/Pin/Unpin` stay out
/// (they need the nav-history-depth / history-id-counter alignment the dedicated nav
/// slice carries; folding those into the full-catalog drive is a later increment).
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
    // Indent/Outdent (pure moves — mint no ids → clean reconcile). They were excluded for 2
    // filed "Turso smells", but EMPIRICAL reproduce over the composed `full_headless` path
    // shows BOTH are stale here (deterministic teeth `wide_indent_outdent_roundtrip_lockstep`
    // + `wide_indent_then_split_parent_lockstep`, plus the random sweep): #1 (top-level NULL
    // parent_id) never fires — the page-rooted tree outdents only to the real page block, and
    // the composed reader tolerates NULL anyway; #2 (split-of-a-block-with-children → Loro
    // positional child-vs-sibling) does NOT reproduce — the Loro-authority→Turso path places
    // the new block as a sibling correctly. So they're simply un-blocked, no prod fix needed.
    arm!(Indent, E2ETransition::Indent);
    arm!(Outdent, E2ETransition::Outdent);
    arm!(NavigateFocus, E2ETransition::NavigateFocus);
    // `ToggleState` self-gates via its render/focus-based generator: it only proposes
    // candidates when the focused region root is an interactively-rendered text block,
    // so it fires only after a `NavigateFocus` lands focus on a text child (parent/c1/c2).
    // A pure property write (`set_field task_state`) — mints no blocks, reconcile no-op.
    arm!(ToggleState, E2ETransition::ToggleState);
    // Editor arms (#2 — the combined "one PBT"). `FocusEditableText` opens an editor on
    // a focusable text child (self-gates: only when no editor is active + a text block is
    // focusable); `TypeChars`/`DeleteBackward` then drive keystrokes (self-gate on an
    // active editor). With no `MoveCursor`, the caret stays at end-of-text so backspace
    // never joins (no block removal) — pure content edits, no reconcile-removal needed.
    // The editor↔structural interplay (Split/NavigateFocus while editor open) is exercised
    // here; any caret/focus-after-structural divergence is the narrow #3 piece to add.
    arm!(FocusEditableText, E2ETransition::FocusEditableText);
    arm!(TypeChars, E2ETransition::TypeChars);
    arm!(DeleteBackward, E2ETransition::DeleteBackward);
    // Seam-rebuild SR-1: `CreateDocument` mints a new doc (the production
    // `SutAppLifecycle::create_document` writes an empty org file; the watcher mints the
    // doc block). The oracle's synthetic `block:ref-doc-N` is paired to the minted real id
    // by the harness's per-tick reconcile (doc-uri-minting generalization) — the
    // doc-uri case the old E2ESut `block_tree_post_action` CreateDocument arm handled.
    arm!(CreateDocument, E2ETransition::CreateDocument);
    // Nav-history transitions folded from the nav slice (toward deleting it). The wide boot's
    // nav-history is aligned in `structural_ref_wired` ([journals#1, page#2], next=3), and the
    // probe proved the structural/editor/doc transitions write NO nav rows, so the AUTOINCREMENT
    // counter stays in lockstep. `NavigateHome` self-gates (idempotent when already home);
    // `PinBlock` draws a real pinnable text child via its weighted generator (the wide oracle
    // seeds `block_state`, unlike the RefFocus-only nav slice); `NavigateBack/Forward` self-gate
    // via their `can_go_back`/`can_go_forward` preconditions over the aligned stack. `UnpinBlock`
    // is layered in `WideMachine::transitions` (state-dependent — its `history_id` is drawn from
    // the pins the oracle currently holds, so it always matches a SUT-assigned id).
    arm!(NavigateHome, E2ETransition::NavigateHome);
    // `PinBlock` targets the FIXED stable seed block `c1` (Text, non-page, focusable —
    // always passes preconditions, always present). NOT the weighted generator: that draws
    // from Main's editable descendants, which after a `SplitBlock` includes the synthetic
    // `block::split-N`. `SutNavHistoryDrive::pin_block` does NOT resolve oracle→real ids (only
    // the `OpDispatchWriter` block-tree path does), so pinning a synthetic id pins a GHOST on
    // the SUT (`focus_roots(right_sidebar)` then diverges, since pins persist). A stable target
    // needs no resolution. (Mirrors the nav slice's fixed `PINNABLE_ID`.)
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
    // Lifecycle: `SimulateRestart` re-triggers the FileSyncController watcher (file-touch),
    // re-parsing the org tree. Blocks are preserved (`:ID:` drawers make re-parse id-stable),
    // so `apply_to_ref` is a no-op and the reconcile is clean. `SutAppLifecycle::simulate_restart`
    // settles block_raw to a stable id-set in the cap (no composed seam). (StartApp stays out:
    // the composed SUT is pre-booted, so `app_started` is true → its precondition gates it out.)
    arm!(SimulateRestart, E2ETransition::SimulateRestart);
    if arms.is_empty() {
        return Just(E2ETransition::Nothing(Nothing)).boxed();
    }
    Union::new_weighted(arms).boxed()
}

/// SWAP DESIGN PROBE (run with `--nocapture`): print the alphabet `aggregate_transitions`
/// ACTUALLY generates over the candidate swap ref (the seeded wide tree, but with the
/// `full_headless` wiring + cap_set the production `general_e2e_pbt` carries). Unlike the
/// builder cap-feasibility probe, this also applies the WIRING gate + preconditions over a
/// real seeded state — the true drive surface of the swap.
#[test]
fn swap_design_probe_generated_alphabet() {
    use crate::pbt::transitions::aggregate_transitions;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

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
    // once state evolves (blocks to join, an editor open, a mutation to undo), so they
    // are not asserted from the initial state.
    // `SetupWatch` is feasible from the initial state (task #5: watch-query parity
    // converged, the `.without(SutWatchRegister)` narrowing dropped). `RemoveWatch`
    // unlocks only after a watch exists, so it is not asserted from the initial state
    // (like `Join`/`MoveCursor`/`Redo`).
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
// THE SWAP: `general_e2e_composed_pbt` — the production `general_e2e_pbt` SUT swapped
// from `E2ESut` to a composed `CapMap` over `compose_sut(full_headless)`, driving the
// AUTO-NARROWED production alphabet (`aggregate_transitions`, NOT a curated list) so the
// swap can never silently drift from what `general_e2e_pbt` actually generates. The SUT
// side is the EXACT same builder + boot as `frontend_wide_pbt` (`boot_and_seed_wide`);
// the only difference from `WideFrontend` is the GENERATOR: the full production
// `aggregate_transitions` over a ref carrying the `full_headless` wiring + cap_set, so the
// alphabet auto-narrows to exactly the composed SUT's drivable caps (peer/seam/E4/fixture
// ops cap-gate out — see `swap_probe_full_headless_narrowed_alphabet`). This is the §5
// keystone: once green + verdict-parity-gated, `general_e2e_pbt`'s own macro SUT can be
// repointed here and `E2ESut`'s headless cap impls deleted (E3).

// `wide_e2e_ref`, `WideE2EMachine`, and `WideE2E` are RELOCATED to the `pbt`-gated
// `crate::pbt::composed::wide_e2e` module (glob-imported above) so the PRODUCTION
// integration test `general_e2e_composed_pbt` (`tests/general_e2e_composed_pbt.rs`)
// can drive `ComposedSut<WideE2E>` — the macro repoint. `swap_design_probe_generated_
// alphabet` (above) still exercises the relocated `wide_e2e_ref` as a fast lib unit test.

// ═════════════════════════════════════════════════════════════════
// Editor arm — the SAME composed-SUT machinery, but driving the production
// EDITOR alphabet (`TypeChars`) over the REAL headless editor pipeline
// (`HeadlessEditorMirror` hosted on `HeadlessFrontendComponent`, Loro ENABLED so
// the block's `content_raw` `MutableText` resolves). Committed-content parity: the
// reference commits typed text into block content on every `TypeChars`
// (`commit_active_editor_if_changed`); the SUT's per-keystroke `MutableText` edit
// syncs to `block_raw`, so `inv-block-content-matches-ref/block_raw` agrees. The
// editor is pre-opened on `c1` on both sides (the oracle via the UI-actor wiring in
// `build_started_ref`, the SUT via `FocusEditableText`). Kept separate from
// `frontend_wide_pbt` (Loro-off, structural) so the structural arm is unaffected by
// the storage-layer change. `DeleteBackward` is excluded for now: a backspace at
// caret 0 is the structural `join_block` (block removal), which the mint-only
// per-tick reconcile doesn't model — a later increment.
// ═════════════════════════════════════════════════════════════════

/// The editor oracle: the same page-rooted `parent/c1/c2` tree as `structural_ref`,
/// but wired `{Loro, EditorState}` (Loro storage → `enable_loro`; UI actor →
/// `has_editor_buffer`) so the editor transitions gate, with focus + an open editor
/// on `c1` (seeded by `build_started_ref`'s UI-actor branch). No final
/// `NavigateFocus` (that would blur the editor) — focus stays on the editor block.
fn editor_ref() -> ReferenceState {
    use crate::pbt::invariants::registry::Subsystem;
    let subsystems: BTreeSet<Subsystem> = [Subsystem::Loro, Subsystem::EditorState]
        .into_iter()
        .collect();
    // Seeds `parent→{c1,c2}` (and, via the UI actor, an initial focus/editor on `c1`
    // — overwritten below by the boot-mirroring sequence).
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

    // Mirror the SUT boot sequence EXACTLY so every invariant aligns: navigate focus
    // to the page root (this BLURS any open editor and sets the nav matview to the
    // page — the SUT's `NavigateFocus(page)`), then open the editor on `c1` (the
    // SUT's `FocusEditableText(c1)`, which sets `active_editor` to `c1` at end-of-text
    // WITHOUT moving nav focus). Net: nav focus = page (matches the SUT matview),
    // active editor = c1 (the editor invariants compare this).
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

/// Boot the windowless session with Loro ENABLED (so `MutableText` resolves), focus
/// the page root then open the editor on `c1` (matching the oracle), and register the
/// editor READ cap (selects the editor invariants). Returns the cap map + scaffold.
/// Used by the focused editor teeth (the editor coverage that pre-opens an editor and
/// checks strict per-tick caret/text parity). The combined `frontend_wide_pbt` now
/// drives the editor transitions interleaved with the structural ones (#2); this
/// pre-opened-editor boot remains the teeth's focused anchor.
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
    // `SutQueryResults` (full-mode query engine) — mirrors `SutSqlProjection`: keeps
    // `inv-viewmodel-decompiled-rows-match-query` selected and the degraded
    // `inv-viewmodel-shows-source-when-no-query` twin deselected over this real renderer.
    caps.insert(comp.clone() as Arc<dyn SutQueryResults>);
    // The editor READ cap — pairs with the (always-registered) `RefEditorMirror` to
    // select `inv-editor-{text,caret}-matches-ref`. The WRITE cap is already in the
    // component's `register`.
    caps.insert(comp.clone() as Arc<dyn SutEditorMirrorRead>);
    caps.insert(
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

// The standalone editor PBT (`WideEditor`/`frontend_editor_pbt`) has been FOLDED into
// the combined `frontend_wide_pbt` (#2): the wide alphabet now drives the editor
// transitions (`FocusEditableText`/`TypeChars`/`DeleteBackward`) interleaved with the
// structural ones over the same Loro-on headless component. The focused editor coverage
// (pre-opened editor, strict per-tick caret/text parity) lives on in the editor teeth
// below via `editor_ref` + `boot_and_seed_editor`.

// ─────────────────────────────────────────────────────────────────
// Teeth — prove the reconcile loop + invariants over the headless component are
// REAL: a faithful lockstep split stays green, and a SUT-only split (oracle NOT
// applied, so its minted block is unreconciled) is CAUGHT by the block-set
// comparison. The positive direction is also covered by
// `components::tests::headless_structural_seed_and_reconcile_probe`.
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod teeth {
    use super::*;
    use holon_pbt_core::TransitionImpl;

    /// Drop a `ReferenceState` (owns an `Arc<tokio::Runtime>`) off the async executor
    /// — dropping it inside a `#[tokio::test]` context panics.
    fn drop_ref_off_thread(state: ReferenceState) {
        std::thread::spawn(move || drop(state))
            .join()
            .expect("drop ReferenceState off the async executor");
    }

    /// **Increment-3 fresh-drive + ORG-SEED probe — the full catalog is green over
    /// `compose_sut(frontend)`.** The store-only
    /// seed left the working tree absent from the org files `SutOrgRead` parses, so
    /// `inv-blocks-match-ref/org` diverged. Here the tree IS the boot org (page-rooted
    /// leaf siblings, pinned `:ID:`), so the session ingests it into the store AND
    /// `SutOrgRead` parses it — store and org share one source. With the SUT focus
    /// driven, the FULL catalog (incl. the org invariant) must go green.
    #[tokio::test(flavor = "multi_thread")]
    async fn frontend_fresh_drive_org_seed_full_catalog_green() {
        use holon_pbt_core::capabilities::{SutQueryResults, SutSqlProjection};
        use holon_pbt_core::composition::CapProvider;

        // The page-rooted working tree AS org: `structural-page` is the doc/page,
        // parent/c1/c2 are its leaf-sibling children with pinned bare ids.
        const TREE_ORG: &str = "#+ID: structural-page\n\
            * parent\n:PROPERTIES:\n:ID: parent\n:END:\n\
            * c1\n:PROPERTIES:\n:ID: c1\n:END:\n\
            * c2\n:PROPERTIES:\n:ID: c2\n:END:\n";

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
        // `SutQueryResults` (full-mode query engine) — same rationale as the combined
        // boot above: keeps the full decompiled twin selected and the degraded twin off.
        caps.insert(comp.clone() as Arc<dyn SutQueryResults>);
        caps.insert(Arc::new(OpDispatchWriter::with_resolver(
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

    /// Apply `SplitBlock(c1)` to BOTH the oracle and the composed SUT, reconcile the
    /// minted ids, and run the catalog — the faithful structural write path over the
    /// real headless component stays green and the block invariants run non-vacuously.
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

    /// Teeth: apply `SplitBlock(c1)` to the SUT ONLY (oracle NOT advanced), so the
    /// real minted block has no reconciled counterpart in the oracle. The block-set
    /// comparison MUST catch the spurious block — proving the write actually mutated
    /// the real store AND the invariant has teeth over the headless component.
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
            "SUT-only split must be CAUGHT by inv-blocks-match-ref/block_raw (the minted \
             block is spurious vs the un-advanced oracle). Failures: {:?}",
            report.failures()
        );
    }

    /// Teeth for the `NavigateFocus` arm of the **wide** alphabet, in the FULL-catalog
    /// config (`boot_and_seed_wide` — the full frontend cap set). Drive a
    /// `NavigateFocus(c1)` on the SUT ONLY (oracle stays focused on the page root): the
    /// SUT's `current_focus` matview moves to `c1` while the oracle still holds the page,
    /// so `inv-navigation-focus` MUST `Fail`. Proves the focus invariant genuinely
    /// SELECTS and BITES here (not just runs vacuously) — the non-vacuity the random
    /// `frontend_wide_pbt` run relies on when `NavigateFocus` fires. The block/org
    /// invariants stay green (no block change), so this isolates the focus catch.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_sut_only_navigate_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = structural_ref(); // focused on the page root
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

        // SUT-only NavigateFocus to c1 — DON'T advance the oracle.
        let nav = NavigateFocus {
            region: Region::Main,
            block_id: fixed_ids().c1,
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

    /// `ToggleState` (mutate arm) over the composed frontend `CapMap`: toggling `c1`'s
    /// task_state to `TODO` on BOTH the oracle and the SUT in lockstep keeps the full
    /// catalog green — INCLUDING `inv-task-state-storage-coherence` (SQL↔Loro), which the
    /// composed SUT is the FIRST to ever run (`unimplemented!` on `E2ESut`).
    ///
    /// Task #4 done: the headless `SutMutate::toggle_state` drives the real
    /// `cycle_task_state` op (Loro authority doc → `block_raw` projection), and the
    /// composed Loro read cap is unified onto the frontend's authority doc — so the
    /// task_state write is visible to both the SQL and Loro read sides, in lockstep with
    /// `ToggleState::apply_to_ref`.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_toggle_state_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = structural_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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

    /// Teeth: toggle `c1`'s task_state on the SUT ONLY (oracle frozen) — the SUT's
    /// `block_raw.properties.task_state` becomes `TODO` while the oracle's stays unset,
    /// so `inv-blocks-match-ref/block_raw` MUST `Fail`. Proves the `set_field` op
    /// actually mutated the store AND the property comparison has teeth.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_sut_only_toggle_state_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = structural_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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
            "SUT-only ToggleState must be CAUGHT by inv-blocks-match-ref/block_raw; \
             failures: {:?}, ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// A fixed `AllBlocks` watch (the only shape `generate_test_query` produces),
    /// querying the columns the generator selects.
    fn all_blocks_watch(query_id: &str) -> crate::pbt::transitions::SetupWatch {
        use crate::pbt::query::{QuerySource, QueryTable, TestQuery};
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
    /// `CapMap`: register the SAME `AllBlocks` watch on BOTH the oracle and the SUT,
    /// run the catalog — the watch invariants stay green. The booted SUT's `AllBlocks`
    /// watch returns the scaffold/journals blocks (+ the page) that the hand-built
    /// oracle doesn't model as real blocks, and the oracle carries the phantom
    /// `started-ref-layout-query` seed block the SUT lacks; `inv-watch-rows-match-ref`
    /// must seed-exclude both sides (the same way `inv-blocks-match-ref` does) so only
    /// the non-seed working tree is compared. This is the last narrowing gating E3/E5.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_setup_watch_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = structural_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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

    /// Teeth: register the `AllBlocks` watch on BOTH sides, then `Split` `c1` on the SUT
    /// ONLY (oracle frozen). The split tail is a NON-seed user block that appears in the
    /// SUT's watch rows but not the oracle's expected rows, so `inv-watch-rows-match-ref`
    /// MUST `Fail` — proving the seed-exclusion does not over-mask a genuine user-row
    /// divergence (the watch invariant still has teeth on the working tree).
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_frontend_sut_only_watch_rows_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = structural_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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

    /// Teeth: type `Z` into `c1` on BOTH the oracle and the composed editor SUT, run
    /// the catalog — the editor write path over the REAL headless editor
    /// (`HeadlessEditorMirror`) stays green, with committed-content + editor-text +
    /// caret parity all running non-vacuously. The positive direction of the keystone.
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
            "inv-block-content-matches-ref/block_raw",
            "inv-editor-text-matches-ref",
            "inv-editor-caret-matches-ref",
        ] {
            assert!(
                report.ran_ids().contains(&id),
                "non-vacuity: {id} must run (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// Teeth: type `Z` into `c1` on the SUT ONLY (oracle frozen) — the SUT's editor
    /// live text + committed `block_raw.content` become `c1Z` while the oracle stays
    /// `c1`, so the content + editor-text invariants MUST `Fail`. Proves the headless
    /// editor keystroke actually mutated the `MutableText` AND committed it to the
    /// projection (the keystone), and that the parity checks have teeth.
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
                *id == "inv-block-content-matches-ref/block_raw"
                    || *id == "inv-editor-text-matches-ref"
            }),
            "SUT-only TypeChars must be CAUGHT by the content/editor-text parity; \
             failures: {:?}, ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// Teeth for the #3 split-then-type interleaving (the frontend focus-handoff fold).
    /// Split `c1`, then `TypeChars` DIRECTLY — with NO intervening `FocusEditableText`.
    /// The only thing that makes the keystroke land on the new block is the composed
    /// write's production focus-handoff (`OpDispatchWriter`'s `dispatch_intent_sync` →
    /// `apply_structural_focus`), which moves the SUT's `focused_block` onto the
    /// split-created block — exactly as `SplitBlock::apply_to_ref` does via `set_focus` +
    /// `open_active_editor`. Were the handoff absent (the old blur regime), this would
    /// panic ("no focused block") or type into the wrong block and the content/editor-text
    /// parity would `Fail`. Lockstep green here = split-then-type works on BOTH sides.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_split_then_type_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = wide_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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

        // Type DIRECTLY into the (now-focused-via-handoff) new block — no FocusEditableText.
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
        for id in [
            "inv-editor-text-matches-ref",
            "inv-block-content-matches-ref/block_raw",
        ] {
            assert!(
                report.ran_ids().contains(&id),
                "non-vacuity: {id} must run over the split-then-typed block (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// PROBE (swap-config widening): does `compose_sut(full_headless())` — which adds the
    /// Loro PEER arm (`SutLoro` + the loro read caps, selecting the loro invariants) on top
    /// of the turso-frontend-editor cap set — run the FULL catalog GREEN on the static
    /// seeded tree? If the Loro arm reads a DIFFERENT doc than the frontend's Turso session
    /// writes, the loro invariants would see an empty/divergent tree. This static check
    /// (no transitions) isolates whether full_headless is a drivable swap config before
    /// wiring an alphabet over it. Prints the ran set + failures.
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
            "full_headless static catalog must be green to be a drivable swap config; failures above"
        );
    }

    /// Seam-rebuild SR-1 teeth (doc-uri-minting reconcile generalization). Drive
    /// `CreateDocument` over the composed frontend CapMap: the real
    /// `SutAppLifecycle::create_document` writes an empty org file, the watcher mints the
    /// page block, and the harness-style reconcile maps the oracle's synthetic
    /// `block:ref-doc-N` to the minted real id. Lockstep green proves the minted doc page
    /// participates symmetrically in `block_raw` on both sides — the doc-uri-minting case
    /// the old E2ESut `block_tree_post_action` CreateDocument arm handled, now generic.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_create_document_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = wide_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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
            "lockstep CreateDocument must stay green over the composed frontend CapMap \
             (the minted page must match on both sides): {:?}",
            report.failures()
        );
        assert!(
            report.ran_ids().contains(&"inv-blocks-match-ref/block_raw"),
            "non-vacuity: blocks-match must run over the new doc page (ran: {:?})",
            report.ran_ids()
        );
    }

    /// Teeth: create the doc on the SUT ONLY (oracle frozen) — the SUT mints a new page
    /// the un-advanced oracle doesn't have, so `inv-blocks-match-ref/block_raw` MUST
    /// `Fail`. Proves `create_document` actually wrote+ingested the doc AND the block
    /// comparison has teeth over the composed path.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_sut_only_create_document_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = wide_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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
            "SUT-only CreateDocument must be CAUGHT by inv-blocks-match-ref/block_raw \
             (the minted page is spurious vs the un-advanced oracle); failures: {:?}, ran: {:?}",
            report.failures(),
            report.ran_ids()
        );
    }

    /// Nav-history fold teeth: pin `c1` to the right sidebar on BOTH oracle and SUT in
    /// lockstep — the focus-roots invariant runs over the composed nav-history drive and
    /// agrees. Proves `SutNavHistoryDrive::pin_block` lands the pin where the oracle's
    /// `open_pins` puts it (and that the boot history-id alignment in `wide_ref` is exact:
    /// the SUT-assigned pin id matches the oracle's predicted `next_history_id`).
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_pin_block_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = wide_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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

    /// Teeth: pin `c1` to the right sidebar on the SUT ONLY (oracle frozen) — the SUT's
    /// `focus_roots(right_sidebar)` gains the pin while the oracle's stays empty, so
    /// `inv-focus-roots` MUST `Fail`. Proves the headless pin op actually mutated the
    /// nav-history/focus matview AND the focus-roots comparison has teeth.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_sut_only_pin_block_is_caught() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let oracle = wide_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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

    /// Turso smell #1 reproduce (Outdent / top-level NULL parent_id): Indent `c2` under
    /// `c1` (depth 2), then Outdent `c2` back to the page (grandparent = the real page block,
    /// NOT no_parent). If smell #1 were live, the SUT would write a divergent parent for the
    /// outdented block. Lockstep green ⟹ smell #1 does not reproduce on the composed path
    /// (the page-rooted tree never outdents to the literal top level).
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_indent_outdent_roundtrip_lockstep() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = wide_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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
            "Indent→Outdent roundtrip must stay green (smell #1 stale on the page-rooted tree): {:?}",
            report.failures()
        );
    }

    /// Turso smell #2 reproduce (split-of-block-with-children → child-vs-sibling): Indent
    /// `c2` under `c1` (so `c1` HAS a child), then Split `c1`. The oracle makes the new block
    /// a SIBLING of `c1` (parent = page); if the Loro positional-placement smell is live, the
    /// SUT attaches it as a CHILD of `c1` → `inv-block-parent-matches-ref/block_raw` Fails.
    /// This is the decisive smell-#2 probe.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_indent_then_split_parent_lockstep() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = wide_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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

    /// Lifecycle tooth + id-stability make-or-break: `SimulateRestart` re-triggers the
    /// FileSyncController watcher (file-touch) and re-parses the org tree. This must PRESERVE
    /// the block_raw id-set (the `:ID:` drawers on disk make re-parse id-stable) — if ids
    /// drifted, the full catalog would diverge. Restart on the SUT (oracle `apply_to_ref` is a
    /// no-op), then the catalog must stay green with the block invariants running non-vacuously.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_simulate_restart_lockstep_stays_green() {
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
        let mut oracle = wide_ref();
        let (mut caps, scaffold_ids) = boot_and_seed_wide(&resolver, &oracle).await;

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

    /// Non-vacuity guard for the COMBINED alphabet (#2): prove the editor transitions
    /// actually GENERATE in `wide_aggregate` — else `frontend_wide_pbt` could pass
    /// "green" while never exercising the editor↔structural interplay it exists to
    /// cover (the CLAUDE.md "silently looks fine" trap). Samples the wide alphabet from
    /// `wide_ref()` and asserts `FocusEditableText` is offered (an editor can open), then
    /// applies it and asserts `TypeChars`/`DeleteBackward` become offered (keystrokes
    /// chain off an open editor) — alongside the structural arms.
    #[test]
    fn wide_combined_alphabet_includes_editor_transitions() {
        use proptest::strategy::{Strategy, ValueTree};
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
            "the combined alphabet must offer FocusEditableText (open an editor); got {base_variants:?}"
        );
        // Structural arms feasible from focus-on-page (Join/Toggle need focus on a
        // specific child first, so they're not offered at the page-focused base state).
        // `CreateDocument` (seam-rebuild SR-1) is feasible from the started base state.
        // Nav-history transitions folded from the nav slice. NavigateBack is offered at the
        // base state (the aligned boot stack has cursor=1 → can_go_back). NavigateForward is
        // NOT (cursor at top), so it's asserted post-Back below.
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
            "with an editor open the combined alphabet must offer TypeChars + DeleteBackward; \
             got {opened_variants:?}"
        );

        drop_ref_off_thread(base);
        drop_ref_off_thread(opened);
    }

    /// **PCG-5b: the production WIDE `E2ETransition` enum drives a composed `CapMap`.**
    /// The slice teeth above drive the *fine-grained* `SplitBlock` over the cap map; the
    /// payoff PCG-4 unlocked is that the whole-alphabet dispatch
    /// `<E2ETransition as TransitionImpl<ReferenceState, CapMap>>::apply_to_sut` — which
    /// requires `CapMap: SutHandle` — now runs over the composed SUT, exactly as it will
    /// when the wide PBT's SUT is swapped from `E2ESut` to a `CapMap`. We wrap the split
    /// in `E2ETransition`, set the cap gate's RHS via `with_cap_set` (and assert the gate
    /// would admit it), drive it through the wide enum in lockstep, reconcile, and the
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
        // cap set, so the wide alphabet would generate it (no absent-cap `expect` panic).
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
}
