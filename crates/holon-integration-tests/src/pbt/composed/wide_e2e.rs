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

use holon_api::repository::NewBlock;
use holon_api::{Block, EntityUri, Region};
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::composition::{CapMap, CapSet, InvariantId};
use holon_pbt_core::{
    Actor, ComponentSet, Projection, StorageAdapter, TransitionImpl, TransitionRef, Wiring,
};
use proptest::strategy::{BoxedStrategy, Strategy};
use proptest_state_machine::ReferenceStateMachine;

use crate::pbt::composed::builder::{
    compose_sut, compose_sut_seeded, compose_sut_windowed_base_seeded,
};
use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::composed::harness::{ComposedSlice, sut_ids};
use crate::pbt::composed::seed_primitives::{C1, C2, PARENT, fixed_ids};
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

    // The page IS a user document: its org file is `structural-page.org` (what
    // `boot_and_seed_wide` writes `WIDE_TREE_ORG` to) and its doc-uri is the page id
    // (`block:structural-page` — the `#+ID:`-derived `file_id` the parser hands the SUT's
    // `documents` key). Populating this un-gates the External (org) `ApplyMutation` arm and
    // `BulkExternalAdd` (both require a non-empty `files.documents`); the value matches the
    // SUT org filename so the native StartApp name-reconcile stays aligned too.
    state
        .files
        .documents
        .insert(page.clone(), "structural-page.org".to_string());

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
    // SQL-matview per-block equality INCLUDING the junction edge fields (`tags`,
    // `requires`) — the `/block_raw` variant compares only the {content, properties}
    // subset and lacks the junction columns. `full_headless` hosts `SutBackend +
    // SutSqlProjection`, so requiring it every tick makes the ONE PBT the owner of
    // the Loro→SQL edge-field projection check (catches H12: `blocks_differ`
    // omitting `requires` from its change gate). Uses `retry_until_ok` (5s) per tick.
    "inv-blocks-match-ref/matview",
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

/// The wide working tree (`page_root` → `parent`/`c1`/`c2` siblings) as a structured
/// boot seed — the non-frontend face of the same tree `WIDE_TREE_ORG` encodes for the
/// frontend org boot, derived from the SAME fixed ids + contents `structural_ref_wired`
/// re-roots the oracle into, so SUT and oracle agree by construction. Order matters:
/// `page_root` first, then its children (so the builder's `create_block` replay nests
/// them and the sibling sequence is `0,1,2`).
fn wide_seed_tree() -> Vec<NewBlock> {
    let ids = fixed_ids();
    let page = page_root();
    vec![
        NewBlock::text(EntityUri::no_parent(), "structural-page").with_id(page.clone()),
        NewBlock::text(page.clone(), PARENT).with_id(ids.parent),
        NewBlock::text(page.clone(), C1).with_id(ids.c1),
        NewBlock::text(page, C2).with_id(ids.c2),
    ]
}

/// Boot the windowless production SUT for the oracle's wiring via the PRODUCTION builder
/// (`compose_sut_seeded`) and seed the working tree, then (for a focus-capable config)
/// drive the initial focus onto the page root (matching the oracle) and return the cap
/// map + the scaffold ids to seed-inject into the oracle.
///
/// The builder owns boot+seed for every wiring: a **frontend** (Turso+ViewModel) config
/// ingests `WIDE_TREE_ORG` through its session's file-sync adapter; a **non-frontend**
/// (Loro-only) config has no session, so the builder creates [`wide_seed_tree`] directly
/// into the canonical Loro backend. Both faces encode the same tree, so the SUT matches the
/// oracle either way. Org carries no special status here (ADR 0004 — the domain is
/// canonical, org/Loro/Turso are peer adapters): it's just the serialization the frontend
/// session's file-sync happens to read; the non-frontend face is structured domain CRUD.
pub async fn boot_and_seed_wide(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
) -> (CapMap, BTreeSet<EntityUri>) {
    // SUT-side parameterization seam: the booted set follows the oracle's drawn wiring
    // (today fixed to `full_headless` by `wide_e2e_ref`; `init_state` draws
    // `any_valid_wiring()` once the ref-side wiring + required-invariants sub-steps land).
    let set = set_for_wiring(&ref_state.wiring);
    let has_frontend = set.has_projection(Projection::ViewModel);
    let bundle = compose_sut_seeded(
        &set,
        resolver,
        &[("structural-page.org", WIDE_TREE_ORG)],
        &wide_seed_tree(),
    )
    .await;
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

    // Scaffold = everything the SUT booted OR the oracle models, EXCEPT the non-seed
    // working tree (parent/c1/c2) — and, for a frontend config, EXCEPT `block:journals`.
    //
    // The union makes the seed wiring-agnostic: a frontend SUT boots `block:journals` +
    // the index.org layout (in `booted`); a non-frontend SUT does NOT, but the oracle
    // still models that layout, so those ids must come from the ref side to be
    // seed-injected and filtered — otherwise they'd false-diverge.
    //
    // `block:journals` is the ONE first-boot page that is self-documenting
    // (`block_documents[journals]=journals`, i.e. NON-seed) rather than seed-classified
    // like `__default__`/index.org. For a frontend config it is present on BOTH sides
    // (SUT boots it, oracle models it), so we keep it OUT of the seed-filter and let
    // `inv-blocks-match-ref/block_raw` ASSERT it — the user-visible first-boot journals
    // page is verified, not hidden. A non-frontend SUT never boots it, so there it stays
    // in the scaffold (filtered) to match the oracle's modeled-but-not-booted copy.
    let ids = fixed_ids();
    let journals = EntityUri::parse("block:journals").expect("journals id");
    let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
        .into_iter()
        .collect();
    let booted = sut_ids(&caps).await;
    let ref_ids: BTreeSet<EntityUri> = ref_state
        .domain
        .block_state
        .blocks
        .keys()
        .cloned()
        .collect();
    let scaffold: BTreeSet<EntityUri> = booted
        .union(&ref_ids)
        .filter(|id| !tree.contains(id))
        .filter(|id| !(has_frontend && **id == journals))
        .cloned()
        .collect();

    // Fresh-drive the initial focus on the SUT to match the oracle (page root) — only for
    // a focus-capable config. A non-frontend (Loro-only) SUT has no `SutFocusWrite` cap
    // (no ViewModel/nav), and its focus/nav invariants deselect, so there is nothing to
    // align; driving `NavigateFocus` there would hit an absent cap.
    if has_frontend {
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
    }

    (caps, scaffold)
}

/// The §Round-5 windowed dual of [`boot_and_seed_wide`]: boot the SAME wide working tree
/// through the production builder, but with the driver rung **deferred**
/// ([`compose_sut_windowed_base_seeded`]) so the gpui-thread harness can INSERT the window's
/// `GpuiUserDriver`-backed gesture caps via `overlay_windowed_caps`. Returns the full builder
/// bundle (its booted `session`/`reactive` are what the window binds as a pure renderer) plus
/// the scaffold ids to seed-inject into the oracle — identical scaffold math to
/// `boot_and_seed_wide`, so the SAME [`wide_e2e_ref`] oracle matches. The initial focus-align
/// (page root) is driven LATER, through the overlaid caps (they carry the window driver), by
/// [`windowed_composed_sut`]. A window needs a session, so the frontend arm is mandatory here.
pub async fn boot_and_seed_wide_windowed_base(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
) -> (
    crate::pbt::composed::builder::ComposedSut,
    BTreeSet<EntityUri>,
) {
    let set = set_for_wiring(&ref_state.wiring);
    assert!(
        set.has_projection(Projection::ViewModel),
        "the windowed wide base needs a frontend (ViewModel) session for the window to \
         render; got {set:?}"
    );
    let mut bundle = compose_sut_windowed_base_seeded(
        &set,
        resolver,
        &[("structural-page.org", WIDE_TREE_ORG)],
        &wide_seed_tree(),
    )
    .await;

    // Align the initial focus onto the oracle's page root via the production `NavigateFocus`
    // cap — `SutFocusWrite` dispatches through the reactive engine's `dispatch_intent_sync`,
    // which runs the `navigation.focus` SQL write AND mirrors focus into `engine.focused_block()`
    // (`maybe_mirror_navigation_focus`), exactly as a production sidebar page-nav does. Done on
    // the deferred base pre-window; window bring-up does not reset engine focus, so the first
    // render paints the already-focused engine. Mirrors `boot_and_seed_wide`'s headless drive.
    //
    // §8.12 insert-only: the deferred base's `bundle.caps` is gesture-CAPLESS so the gpui-thread
    // overlay can INSERT the window-driver gesture caps. So this seed focus-align (NOT a tested
    // transition — it's boot state) drives through a THROWAWAY gesture map bound to the component's
    // OWN headless `ReactiveEngineDriver`. The focus effect lands on the SHARED engine/reactive,
    // while `bundle.caps` stays capless for the overlay.
    let comp = bundle
        .frontend
        .clone()
        .expect("windowed wide base is a frontend arm, so it has a booted component");
    let mut seed_focus_caps = CapMap::new();
    comp.clone()
        .register_gesture_writes(&mut seed_focus_caps, comp.driver());
    TransitionImpl::apply_to_sut(
        &NavigateFocus {
            region: Region::Main,
            block_id: page_root(),
        },
        ref_state,
        &mut seed_focus_caps,
    )
    .await;
    tokio::time::sleep(SETTLE).await;

    // Scaffold = booted UNION ref_ids MINUS working tree (identical to `boot_and_seed_wide`).
    let ids = fixed_ids();
    let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
        .into_iter()
        .collect();
    let booted = sut_ids(&bundle.caps).await;
    let ref_ids: BTreeSet<EntityUri> = ref_state
        .domain
        .block_state
        .blocks
        .keys()
        .cloned()
        .collect();
    let scaffold: BTreeSet<EntityUri> = booted
        .union(&ref_ids)
        .filter(|id| !tree.contains(id))
        .cloned()
        .collect();

    (bundle, scaffold)
}

/// Assemble the windowed [`ComposedSut<WideE2E>`](crate::pbt::composed::harness::ComposedSut)
/// around the OVERLAID windowed caps (the gpui-thread harness produced them by attaching a
/// window over a [`boot_and_seed_wide_windowed_base`] session and calling
/// `overlay_windowed_caps`). The initial page-root focus-align is already done on the base by
/// [`boot_and_seed_wide_windowed_base`] (pre-window), so this just wraps the caps via
/// [`ComposedSut::from_parts`]. `settle` pumps the window before each check; `rt` drives the
/// apply/check futures (the booted backend runs on its own session runtime).
pub fn windowed_composed_sut(
    caps: CapMap,
    resolver: IdResolver,
    scaffold_ids: BTreeSet<EntityUri>,
    rt: tokio::runtime::Runtime,
    settle: crate::pbt::composed::harness::SettleHook,
) -> crate::pbt::composed::harness::ComposedSut<WideE2E> {
    crate::pbt::composed::harness::ComposedSut::<WideE2E>::from_parts(
        caps,
        (),
        resolver,
        scaffold_ids,
        rt,
        settle,
    )
}

/// Normalize a (possibly drawn) `Wiring` into the composed **headless** `ComponentSet`
/// the `general_e2e_composed_pbt` swap boots — the SUT-side seam env-parameterization
/// flips (drawing `any_valid_wiring()` instead of fixing `full_headless`). Mirrors the
/// native `storage_selector_for_wiring` backend choice so a Loro-only draw maps to the
/// cheap `LoroMemory` SUT and a Turso draw to the full `BackendEngine`:
///
/// - **strip `Actor::UI`** — the composed `CapMap` is headless by construction
///   (`compose_sut` fail-louds on a UI actor; a window is the sibling gpui-thread
///   harness's job, Design §8.10);
/// - **force `StorageAdapter::Loro` when Turso is absent** — the native selector maps
///   every non-Turso wiring onto the LoroMemory backend, and `compose_sut` requires ≥1
///   of Loro/Turso;
/// - **select `ViewModel` only with Turso** (`compose_sut` asserts `!has_frontend ||
///   has_turso`); always select `EditorState`.
///
/// Idempotent: an already-normalized wiring maps to itself, so
/// `set_for_wiring(&full_headless().wiring) == full_headless()`.
pub fn set_for_wiring(wiring: &Wiring) -> ComponentSet {
    let mut wiring = wiring.clone();
    wiring.actors.remove(&Actor::UI);
    if !wiring.has_storage(StorageAdapter::Turso) {
        wiring.storage_adapters.insert(StorageAdapter::Loro);
    }
    let mut projections: BTreeSet<Projection> = [Projection::EditorState].into_iter().collect();
    if wiring.has_storage(StorageAdapter::Turso) {
        projections.insert(Projection::ViewModel);
    }
    ComponentSet::new(wiring, projections)
}

/// The composed cap set for a (normalized) `wiring`, computed ONCE per distinct
/// `ComponentSet` by booting `compose_sut(set_for_wiring(wiring))` on a throwaway
/// current-thread runtime and extracting the (runtime-free) `CapSet`. The swap ref
/// carries this so `aggregate_transitions` auto-narrows the production alphabet to
/// exactly what THIS composed SUT can drive. Cached by `Wiring` (linear scan — `Wiring`
/// is `Eq` but not `Hash`/`Ord`, and the draw set is tiny).
pub fn cap_set_for_wiring(wiring: &Wiring) -> CapSet {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<Vec<(Wiring, CapSet)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let set = set_for_wiring(wiring);
    if let Some((_, cs)) = cache
        .lock()
        .expect("cap_set cache mutex")
        .iter()
        .find(|(w, _)| *w == set.wiring)
    {
        return cs.clone();
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime for cap_set extraction");
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
    let cs = rt.block_on(async { compose_sut(&set, &resolver).await.caps.cap_set() });
    drop(rt);
    cache
        .lock()
        .expect("cap_set cache mutex")
        .push((set.wiring.clone(), cs.clone()));
    cs
}

/// The `full_headless` cap set — the swap's current fixed wiring. Thin alias over
/// [`cap_set_for_wiring`] (the parameterized seam).
pub fn full_headless_cap_set() -> CapSet {
    cap_set_for_wiring(&ComponentSet::full_headless().wiring)
}

/// The swap oracle for a (normalized) `wiring`: the seeded wide tree, re-wired to that
/// wiring and carrying its composed cap_set, so `aggregate_transitions` gates the alphabet
/// to exactly the caps THIS composed SUT provides. The ref-side **subsystem** wiring stays
/// `wide_ref()`'s `{Loro, EditorState}` for every draw (the editor/focus transitions gate
/// on it; the SUT-side cap_set, not the ref subsystems, is what narrows per wiring) — so a
/// Loro-only draw reuses the same oracle tree with a narrower cap_set.
///
/// NO deliberate narrowing remains: the swap drives the FULL production
/// `aggregate_transitions` alphabet auto-narrowed by the real cap set.
/// - `SutMutate` → `ToggleState`: un-narrowed task #4 (Loro read-doc unify + real
///   `cycle_task_state` toggle).
/// - `SutWatchRegister` → `SetupWatch`/`RemoveWatch`: un-narrowed task #5 — the watch
///   invariant seed-excludes both sides, so `inv-watch-rows-match-ref` compares only the
///   non-seed working tree.
pub fn wide_e2e_ref_for(wiring: &Wiring) -> ReferenceState {
    let set = set_for_wiring(wiring);
    let mut state = wide_ref();
    state.wiring = set.wiring.clone();
    state.with_cap_set(cap_set_for_wiring(wiring))
}

/// The swap oracle for the current fixed wiring (`full_headless`). Thin alias over
/// [`wide_e2e_ref_for`] (the parameterized seam).
pub fn wide_e2e_ref() -> ReferenceState {
    wide_e2e_ref_for(&ComponentSet::full_headless().wiring)
}

/// The WINDOWED swap oracle: the same wide tree/wiring as [`wide_e2e_ref`], but carrying
/// the LIVE windowed SUT's cap set (read off the assembled SUT via
/// [`ComposedSut::cap_set`](crate::pbt::composed::harness::ComposedSut::cap_set) after
/// `overlay_windowed_caps`). `wide_e2e_ref`'s `full_headless_cap_set()` lacks the window
/// caps (`SutLayout`/`SutDriver`/…), so gesture transitions like `ClickBlock` would
/// deselect/misbehave under it; the live set admits exactly what the window drives —
/// including `SutFocusWrite`, which is faithfully present (NO `.without()` subtraction:
/// absence-faking a real cap is the invalid-intermediate-state anti-pattern).
pub fn wide_e2e_windowed_ref(cap_set: CapSet) -> ReferenceState {
    wide_e2e_ref().with_cap_set(cap_set)
}

/// Reference machine over the production `E2ETransition`, generated by the FULL
/// production `aggregate_transitions` (auto-narrowed by the ref's wiring + cap_set).
pub struct WideE2EMachine;

impl ReferenceStateMachine for WideE2EMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        // Draw the FULL valid-wiring space (shrinking toward Loro-only — the cheap minimal
        // backend) and build the per-wiring oracle. `wide_e2e_ref_for` does a `block_on` for
        // cap-set extraction, valid here because proptest calls `init_state` in a sync
        // context with no ambient runtime (see `loro_only_wide_seed_runs_block_invariants_green`).
        // The per-draw non-vacuity floor (`required_invariants`) keeps a Loro-only draw from
        // false-REDing on the SQL/ViewModel ids it has no caps for.
        // `HOLON_PBT_FORCE_FULL=1` pins every draw to `full_headless` — the deterministic
        // exerciser for the frontend-only composed arms (`ApplyMutation` External /
        // `BulkExternalAdd`), which `any_valid_wiring` only reaches on a rare Turso draw.
        if std::env::var("HOLON_PBT_FORCE_FULL").is_ok() {
            return ::proptest::strategy::Strategy::boxed(
                ::proptest::prelude::Just(wide_e2e_ref()),
            );
        }
        holon_pbt_core::any_valid_wiring()
            .prop_map(|w| wide_e2e_ref_for(&w))
            .boxed()
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

    /// Per-draw non-vacuity floor: keep only the `WIDE_REQUIRED_INVARIANTS` that THIS
    /// draw's caps can actually select. The SUT axis is the drawn wiring's cap_set (already
    /// carried on the ref by [`wide_e2e_ref_for`]); the ref axis registers every ref cap
    /// unconditionally (see `impl CapProvider for ReferenceState`). A Loro-only draw thus
    /// drops the SQL/ViewModel/focus ids it has no caps for, while a `full_headless` draw
    /// keeps all of them (the intersection is a no-op there — the keystone floor is
    /// unchanged). Selection here uses the SAME `Needs::selected_against` the runner uses,
    /// computed against the wiring's EXPECTED cap_set (not the actual booted caps), so the
    /// floor still has teeth: if the wiring claims a cap the boot fails to wire, the
    /// invariant is required-but-deselected and the floor REDs. The returned ids are
    /// parsed FROM the catalog ([`CapInvariant::id`]), so each `WIDE_REQUIRED_INVARIANTS`
    /// string is a selector validated against the live registry (the `panic!` is the parse).
    fn required_invariants(ref_state: &ReferenceState) -> Vec<InvariantId> {
        let sut_caps = ref_state
            .cap_set
            .clone()
            .expect("composed wide draw must carry a cap_set (set by wide_e2e_ref_for)");
        let mut ref_map = CapMap::new();
        holon_pbt_core::composition::CapProvider::register(
            Arc::new(ref_state.clone()),
            &mut ref_map,
        );
        let ref_caps = ref_map.cap_set();
        let catalog = composed_invariant_catalog();
        WIDE_REQUIRED_INVARIANTS
            .iter()
            .copied()
            .map(|id| {
                catalog
                    .iter()
                    .find(|inv| inv.id().0 == id)
                    .unwrap_or_else(|| {
                        panic!("WIDE_REQUIRED invariant {id:?} is not in the composed catalog")
                    })
            })
            .filter(|inv| inv.needs().selected_against(&sut_caps, &ref_caps))
            .map(|inv| inv.id())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbt::transitions::{AddPeer, MergeFromPeer, PeerEdit, PeerEditOp, SyncWithPeer};

    /// Seed-generalization validation (the §8.10 next-step gate): a **Loro-only** wide
    /// draw (no Turso ⇒ no frontend) boots EMPTY through the builder's non-frontend arm,
    /// so `boot_and_seed_wide` must seed the working tree directly into the canonical Loro
    /// backend. This proves the block-comparison invariants RUN and are GREEN over the
    /// seeded Loro SUT — parent/c1/c2 match the oracle AND the oracle-modeled boot layout
    /// (`block:journals` + index.org) is filtered via the ref∪booted scaffold union, not
    /// falsely diverging. Without the seed this would deselect/false-RED; this is the gate
    /// for letting `init_state` draw non-frontend wirings.
    #[test]
    fn loro_only_wide_seed_runs_block_invariants_green() {
        // Build the ref OUTSIDE any ambient runtime (it does a `block_on` for cap-set
        // extraction — mirrors proptest's sync `init_state`), then drive the async boot +
        // catalog run on a manually-built multi-thread runtime (mirrors `init_test`).
        let wiring = Wiring::custom(vec![StorageAdapter::Loro], vec![], vec![]);
        let ref_state = wide_e2e_ref_for(&wiring);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build multi-thread runtime");
        rt.block_on(async {
            let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
            let (caps, scaffold) = boot_and_seed_wide(&resolver, &ref_state).await;
            let report =
                <WideE2E as ComposedSlice>::run_report(&caps, &resolver, &scaffold, &ref_state)
                    .await;
            assert!(
                report.failures().is_empty(),
                "Loro-only wide seed must run the catalog green; failures: {:?}",
                report.failures()
            );
            let ran = report.ran_ids();
            assert!(
                ran.contains(&"inv-blocks-match-ref/block_raw"),
                "the block-id comparison must RUN over the seeded Loro SUT (non-vacuity \
                 proof the seed landed); ran: {ran:?}"
            );
        });
    }

    /// The SUT-side parameterization seam's behaviour-preservation anchor: the swap's
    /// current fixed wiring round-trips through [`set_for_wiring`] to exactly
    /// `full_headless` (so flipping `init_state` to draw `any_valid_wiring()` cannot
    /// silently change today's full_headless run), and the normalizer maps a Loro-only
    /// draw onto the cheap headless backend (Loro forced, no ViewModel, no UI).
    #[test]
    fn set_for_wiring_preserves_full_headless_and_maps_loro_only() {
        let full = ComponentSet::full_headless();
        assert_eq!(
            set_for_wiring(&full.wiring),
            full,
            "set_for_wiring must be identity on the already-normalized full_headless wiring"
        );

        // A bare Loro-only manifest (the fast-path target) → Loro backend, EditorState
        // only (ViewModel needs Turso), no UI.
        let loro_only = Wiring::custom(vec![StorageAdapter::Loro], vec![], vec![]);
        let set = set_for_wiring(&loro_only);
        assert!(set.has_storage(StorageAdapter::Loro));
        assert!(!set.has_storage(StorageAdapter::Turso));
        assert!(!set.has_projection(Projection::ViewModel));
        assert!(set.has_projection(Projection::EditorState));
        assert!(!set.has_actor(Actor::UI));

        // A Turso draw selects the frontend (ViewModel) arm.
        let turso = Wiring::custom(vec![StorageAdapter::Turso], vec![], vec![]);
        assert!(set_for_wiring(&turso).has_projection(Projection::ViewModel));
    }

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
