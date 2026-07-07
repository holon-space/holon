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
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transitions::{E2ETransition, NavigateFocus};
use holon::api::BackendEngine;

/// The seed **page** the working blocks sit directly under. It is the focus root (so
/// its children are the editable candidates) but is itself excluded from candidates
/// (`is_page`) and from the comparison (seed), so it is never split and its page-ness
/// is never compared.
pub fn page_root() -> EntityUri {
    EntityUri::block("structural-page")
}

/// Settle window for the headless CDC pump after a write. This is the CAP on the
/// [`converge_projections`] convergence wait, not a flat sleep: a settled SUT returns in
/// ~one quiet-floor poll, and a busy one is bounded by this so it never over-waits vs the
/// old flat `sleep(SETTLE)`.
pub const SETTLE: Duration = Duration::from_millis(150);

/// The slice handle for [`WideE2E`] — the store handles the post-write settle needs to
/// prove all three projections (Turso CDC + Loro + org) have drained, instead of a flat
/// `sleep(SETTLE)`. Absent handles (a Loro-only draw has no Turso engine / frontend org
/// sync) make the corresponding projection a no-op — those stores are synchronous, so
/// there is nothing to wait for.
#[derive(Clone, Default)]
pub struct WideHandle {
    /// The canonical Turso `BackendEngine` — its `db_handle().cdc_emitted_watermark()` is
    /// the CDC drain signal (`None` for a Loro-only draw).
    engine: Option<Arc<BackendEngine>>,
    /// The booted frontend component — the lazy accessor for the Loro sync handle /
    /// doc-store (Loro quiescence) and the `OrgSyncIdleSignal` (org re-render drain).
    /// `None` for a non-frontend (Loro-only) draw. Queried at settle time, not at boot,
    /// because the sync controller resolves on a spawned `post_ready_work` task.
    frontend: Option<Arc<HeadlessFrontendComponent>>,
}

impl WideHandle {
    /// Build the settle handle from a booted builder bundle — the windowed harness
    /// ([`windowed_composed_sut`]) reuses the base session's engine/frontend so its
    /// per-apply settle converges the same three projections as the headless path.
    pub fn from_bundle(bundle: &crate::pbt::composed::builder::ComposedSut) -> Self {
        Self {
            engine: bundle.engine.clone(),
            frontend: bundle.frontend.clone(),
        }
    }
}

/// The 3-projection convergence settle that replaces the flat `sleep(SETTLE)` after a
/// write. Waits — capped at `budget` — for every projection the invariants read to reach
/// quiescence:
///
/// 1. **Turso CDC** — `cdc_emitted_watermark` stable for one quiet floor (the `block_raw`
///    matview the block invariants query is CDC-fed).
/// 2. **Loro** — the sync controller's `last_synced_frontiers()` catches up to the
///    authority doc's `oplog_frontiers()` (a peer/merge write projects async).
/// 3. **org** — the file-sync controller's `OrgSyncIdleSignal` goes quiescent (the org
///    re-render `inv-blocks-match-ref/org` reads has drained).
///
/// A CDC-only signal (the reverted lever 2) under-settled — Loro/org lagged and the
/// block/org invariants diverged; this covers all three. Signal-level core shared with
/// the `HeadlessFrontendComponent` boot settle: [`crate::pbt::convergence::converge_signals`].
async fn converge_projections(handle: &WideHandle, budget: Duration) {
    // The frontend accessors are queried at settle time, not at boot: the sync
    // controller / idle signal resolve on a spawned `post_ready_work` task.
    let (sync, store, org_idle) = match &handle.frontend {
        Some(comp) => (
            comp.loro_sync_handle(),
            comp.loro_doc_store(),
            comp.org_idle_signal(),
        ),
        None => (None, None, None),
    };
    crate::pbt::convergence::converge_signals(
        handle.engine.as_ref(),
        sync,
        store,
        org_idle,
        budget,
    )
    .await;
}

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

/// The caps the widest headless wiring (`full_headless`) legitimately does NOT provide,
/// so the catalog invariants that `Needs` them deselect headless WITHOUT that being a
/// silent-deselection bug. Each entry is `(cap-name, why)`. This is the ONLY hand-maintained
/// list the `wide_cap_presence_guard` consults.
///
/// All four are the windowed/GPUI rung: they need a live gpui window (thread affinity
/// `compose_sut` cannot satisfy — it asserts `!Actor::UI` in `builder.rs`) and are supplied
/// ONLY by the windowed slice (`window_slice`), so the catalog invariants that `Needs` them
/// deselect headless AND run only in the windowed harness — NOT a silent-deselection bug. A cap
/// that SHOULD be headless-present but isn't is NOT allowed here; it is a real finding the guard
/// must surface (that is the whole point of listing each one explicitly, with a reason).
#[cfg(test)]
const WIDE_HEADLESS_ABSENT_CAPS: &[(&str, &str)] = &[
    (
        "SutLayout",
        "windowed-only: a laid-out widget tree + BoundsRegistry (geometry) comes only from \
         GpuiWindowComponent over a live gpui window; headless compose_sut has no window",
    ),
    (
        "SutDriver",
        "windowed-only: the engine-focus read (engine_focused_block / resolve_ref_block_id) is \
         a window cap; the headless path registers only the gesture WRITE caps \
         (register_gesture_writes), so the focus-read deselects in the keystone by design",
    ),
    (
        "SutFrontendEngine",
        "windowed-only: root-VM liveness reads (frontend_root_vm / \
         frontend_root_is_error / live_vs_fresh_tree_diff); the headless frontend registers no \
         window engine, only GpuiFrontendEngineComponent does",
    ),
    (
        "SutFrontendEmissions",
        "windowed-only: drain_vm_emissions / provider_stability_report need the \
         live windowed frontend engine; the headless ReactiveEngine returns honest-empty and \
         deselects rather than faking",
    ),
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
) -> (CapMap, WideHandle, BTreeSet<EntityUri>) {
    // SUT-side parameterization seam: the booted set follows the oracle's drawn wiring
    // (today fixed to `full_headless` by `wide_e2e_ref`; `init_state` draws
    // `any_valid_wiring()` once the ref-side wiring + required-invariants sub-steps land).
    let set = set_for_wiring(&ref_state.wiring);
    let has_frontend = set.has_projection(Projection::ViewModel);
    // Scale-soak inflation: extra synthetic doc files (deep trees, tasks, links,
    // unicode) appended to the SUT boot ONLY. Empty unless `HOLON_SOAK_SEED_BLOCKS`
    // is set, so the keystone is untouched by default. Their ids fold into the
    // oracle via the scaffold math below (booted-but-not-tree ⇒ seed-classified),
    // so the invariant catalog stays green while every action pays the whole-vault
    // projection/CDC/consolidator cost.
    let soak_files = crate::pbt::composed::soak_seed::soak_org_files();
    let mut seed_files: Vec<(&str, &str)> = vec![("structural-page.org", WIDE_TREE_ORG)];
    for (name, body) in &soak_files {
        seed_files.push((name.as_str(), body.as_str()));
    }
    let bundle = compose_sut_seeded(&set, resolver, &seed_files, &wide_seed_tree()).await;
    // The settle handles — the Turso engine (CDC watermark) and the frontend component
    // (Loro sync + org idle). Cloned out before `bundle.caps` is moved so the
    // post-write [`converge_projections`] settle can prove all three projections drained.
    let handle = WideHandle {
        engine: bundle.engine.clone(),
        frontend: bundle.frontend.clone(),
    };
    let mut caps = bundle.caps;

    // Scale-soak: drain the WHOLE seeded vault into `block_raw` BEFORE the scaffold
    // id-set is snapshotted below. The frontend boot settle is a flat 300ms — far too
    // short to project 5–10k blocks — so an un-drained soak block would be absent from
    // `booted`, escape seed-classification in the oracle, and later surface in the SUT
    // store with no matching oracle seed entry → a false `inv-blocks-match-ref`
    // divergence. Off (count 0) this is skipped entirely; the keystone is untouched.
    if crate::pbt::composed::soak_seed::soak_block_count() > 0 {
        converge_projections(&handle, crate::pbt::composed::soak_seed::soak_settle()).await;
    }

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
        converge_projections(&handle, crate::pbt::composed::soak_seed::soak_settle()).await;
    }

    (caps, handle, scaffold)
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
    let bundle = compose_sut_windowed_base_seeded(
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
    handle: WideHandle,
    resolver: IdResolver,
    scaffold_ids: BTreeSet<EntityUri>,
    rt: tokio::runtime::Runtime,
    settle: crate::pbt::composed::harness::SettleHook,
) -> crate::pbt::composed::harness::ComposedSut<WideE2E> {
    // The `handle` carries the base session's engine/frontend so the per-apply
    // [`converge_projections`] settle covers the same three projections as the headless
    // path. The `settle` hook additionally pumps the gpui window before each check.
    crate::pbt::composed::harness::ComposedSut::<WideE2E>::from_parts(
        caps,
        handle,
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

/// Re-wire a caller-built oracle to the `full_headless` (frontend) wiring WITHOUT
/// attaching a cap_set — the runtime-free half of [`wide_e2e_ref`].
///
/// `boot_and_seed_wide` reads ONLY `ref_state.wiring` (via [`set_for_wiring`]) to pick the
/// SUT's `ComponentSet`, so a Loro-only-wired oracle (`structural_ref`/`wide_ref`) yields a
/// Loro-thin SUT that is missing the frontend caps (`SutBlockTreeWrite`, `SutFocusWrite`,
/// `SutNavHistoryDrive`, `SutAppLifecycle`, …) the teeth's transitions select — the
/// "selected but absent from the CapMap" panic. This override gives the oracle the same
/// full_headless wiring `wide_e2e_ref` carries, so the SUT boots the full frontend cap map.
///
/// Unlike [`wide_e2e_ref`], it does NOT call `cap_set_for_wiring` (which boots its OWN
/// runtime to extract the cap_set) so it is safe to call from INSIDE a `#[tokio::test]`
/// (no "runtime within a runtime" panic). The teeth drive transitions by hand and never
/// generate, so the cap_set — a generator-narrowing hint — is irrelevant to them.
pub fn frontend_wired(mut state: ReferenceState) -> ReferenceState {
    state.wiring = ComponentSet::full_headless().wiring.clone();
    state
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
    // My change (settle): the per-apply convergence settle needs the store handles.
    type Handle = WideHandle;
    // Main's change (uvuvnwnn): the per-draw `required_invariants` override below supersedes
    // the static list, deriving the floor from the WHOLE shared catalog (every invariant this
    // draw's caps select). The cap-level `wide_cap_presence_guard` proves the widest wiring
    // selects the whole catalog; there is no per-id list to maintain (`WIDE_REQUIRED_INVARIANTS`
    // was retired).
    const REQUIRED_INVARIANTS: &'static [&'static str] = &[];
    const SETTLE: Duration = SETTLE;
    const MULTI_THREAD: bool = true;

    async fn build(
        resolver: &IdResolver,
        ref_state: &ReferenceState,
    ) -> (CapMap, WideHandle, BTreeSet<EntityUri>) {
        boot_and_seed_wide(resolver, ref_state).await
    }

    /// Replace the flat post-apply `sleep(SETTLE)` with the 3-projection convergence
    /// settle — the CDC-only lever under-settled (Loro/org lagged and the block/org
    /// invariants diverged). Capped at `SETTLE`, so it never over-waits vs the old sleep.
    async fn settle_after_apply(handle: &WideHandle, _: &CapMap) {
        converge_projections(handle, crate::pbt::composed::soak_seed::soak_settle()).await;
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

    /// Per-draw non-vacuity floor: every invariant in the WHOLE shared catalog that THIS
    /// draw's caps can actually select MUST run. The SUT axis is the drawn wiring's cap_set
    /// (already carried on the ref by [`wide_e2e_ref_for`]); the ref axis registers every ref
    /// cap unconditionally (see `impl CapProvider for ReferenceState`). A Loro-only draw thus
    /// drops the SQL/ViewModel/focus ids it has no caps for, while a `full_headless` draw
    /// keeps every headless-selectable catalog invariant. Selection here uses the SAME
    /// `Needs::selected_against` the runner uses, computed against the wiring's EXPECTED
    /// cap_set (not the actual booted caps), so the floor has teeth: if the wiring claims a
    /// cap the boot fails to wire, the invariant is required-but-deselected and the floor REDs.
    ///
    /// This is the runtime complement to the static `wide_cap_presence_guard`: that guard
    /// proves the WIDEST wiring's CapMap provides every `Needs` cap (so the widest config
    /// selects the whole catalog); this floor proves each per-draw wiring actually RUNS every
    /// invariant it selects. Neither needs a hand-maintained per-invariant-id list.
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
        composed_invariant_catalog()
            .iter()
            .filter(|inv| inv.needs().selected_against(&sut_caps, &ref_caps))
            .map(|inv| inv.id())
            .collect()
    }
}

/// The narrowed LIVE windowed cap set, captured once (by a throwaway windowed boot at the
/// top of a windowed random runner) before the proptest strategy is built.
/// [`WideE2EWindowedMachine::init_state`] reads it so the generated alphabet + the
/// non-vacuity floor narrow to exactly what the window can drive. Hoisted here
/// (increment 4c) so the gpui loop and the tui composed runner share ONE machine.
static WINDOWED_CAP_SET: std::sync::OnceLock<CapSet> = std::sync::OnceLock::new();

/// Capture the live windowed cap set (once per process). Panics on a second call —
/// a runner must capture it exactly once, before building the strategy.
pub fn set_windowed_cap_set(cap_set: CapSet) {
    WINDOWED_CAP_SET
        .set(cap_set)
        .expect("WINDOWED_CAP_SET set once");
}

/// Narrow a live windowed cap set to the windowed GENERATED alphabet.
///
/// The deferred windowed base is `full_headless` (a `HeadlessFrontendComponent`), which
/// still hosts the 6 EXCLUDED-row nav/history/view caps at the Direct-dispatch rung — but
/// no window-driver mechanism drives them yet (C-3 Rung Audit rows 19–24, tracked Phase 3
/// blockers). Driving them through the leftover dispatch impl while a window exists would
/// be an unfaithful cross-rung combination (Design §8.11), so they must NOT enter the
/// windowed generated alphabet. `CapSet::without` is the sanctioned, DISCLOSED narrowing:
/// the caps stay in the `CapMap` (their read invariants keep selecting), only the
/// generation gate drops their transitions. This is NOT the fix-the-cap-not-withhold
/// anti-pattern (that forbids faking a DIVERGENCE green) — it is the audit-prescribed
/// exclusion of a genuinely-undriveable transition class.
///
/// Cap → EXCLUDED transition rows:
/// - `SutNavHistoryWrite`  → NavigateHome (row 19)
/// - `SutNavHistoryDrive`  → NavigateBack/Forward, PinBlock, UnpinBlock (rows 20–22)
/// - `SutViewControl`      → SwitchView (row 23)
/// - `SutHistoryWrite`     → UndoLastMutation/Redo (row 24)
pub fn narrow_to_windowed_alphabet(cap_set: CapSet) -> CapSet {
    use holon_pbt_core::capabilities::{
        SutHistoryWrite, SutNavHistoryDrive, SutNavHistoryWrite, SutViewControl,
    };
    use holon_pbt_core::composition::CapId;
    cap_set
        .without(&CapId::of::<dyn SutNavHistoryWrite>())
        .without(&CapId::of::<dyn SutNavHistoryDrive>())
        .without(&CapId::of::<dyn SutViewControl>())
        .without(&CapId::of::<dyn SutHistoryWrite>())
}

/// Report which of the 6 EXCLUDED-row caps the LIVE windowed base actually carries, so the
/// narrowing is disclosed against reality (not assumed).
pub fn disclose_excluded(cap_set: &CapSet) {
    use holon_pbt_core::capabilities::{
        SutHistoryWrite, SutNavHistoryDrive, SutNavHistoryWrite, SutViewControl,
    };
    use holon_pbt_core::composition::CapId;
    for (name, present) in [
        (
            "SutNavHistoryWrite (NavigateHome)",
            cap_set.contains(&CapId::of::<dyn SutNavHistoryWrite>()),
        ),
        (
            "SutNavHistoryDrive (Back/Fwd/Pin/Unpin)",
            cap_set.contains(&CapId::of::<dyn SutNavHistoryDrive>()),
        ),
        (
            "SutViewControl (SwitchView)",
            cap_set.contains(&CapId::of::<dyn SutViewControl>()),
        ),
        (
            "SutHistoryWrite (Undo/Redo)",
            cap_set.contains(&CapId::of::<dyn SutHistoryWrite>()),
        ),
    ] {
        eprintln!(
            "[windowed-alphabet] EXCLUDED cap present-in-base={present}: {name} \
             (narrowed out of generation)"
        );
    }
}

/// The windowed sibling of [`WideE2EMachine`]: identical transition generation /
/// preconditions / apply (delegated), but `init_state` FIXES the oracle to the narrowed
/// live windowed cap set ([`set_windowed_cap_set`]) instead of drawing
/// `any_valid_wiring()`. That cap set auto-narrows `aggregate_transitions` to the windowed
/// alphabet (the REBIND/OK gesture rows) and drops the EXCLUDED rows, and it is the same
/// set the per-tick `check_invariants` non-vacuity floor (`required_invariants`) is
/// computed against.
pub struct WideE2EWindowedMachine;

impl ReferenceStateMachine for WideE2EWindowedMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        use proptest::strategy::Just;
        let cap_set = WINDOWED_CAP_SET
            .get()
            .expect("WINDOWED_CAP_SET must be captured (throwaway boot) before the strategy")
            .clone();
        Just(wide_e2e_windowed_ref(cap_set)).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        <WideE2EMachine as ReferenceStateMachine>::transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        <WideE2EMachine as ReferenceStateMachine>::preconditions(state, transition)
    }

    fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
        <WideE2EMachine as ReferenceStateMachine>::apply(state, transition)
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
            let (caps, _handle, scaffold) = boot_and_seed_wide(&resolver, &ref_state).await;
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

    /// CAP-PRESENCE GUARD: the WIDEST wiring (`full_headless`) must PROVIDE every cap the
    /// shared catalog's invariants declare in their `Needs` — so every catalog invariant is
    /// guaranteed SELECTED (and thus run, via the per-draw `required_invariants` floor) in the
    /// wide config. Deselection has exactly one cause — a `Needs` cap absent from the CapMap —
    /// so this guard catches it at the cap level, with no per-invariant-id list to keep in sync.
    ///
    /// The union of every `Needs.sut_present` is checked against the widest SUT cap_set
    /// (`full_headless_cap_set`); the union of every `Needs.ref_present` against the ref
    /// cap_set (the `ReferenceState` registers all ref caps unconditionally). A cap that is
    /// referenced but absent is a finding UNLESS it is on `WIDE_HEADLESS_ABSENT_CAPS` (the
    /// windowed/GPUI rung, structurally impossible headless). The failure names the missing
    /// cap AND the invariant ids that need it — actionable, fail-loud.
    #[test]
    fn wide_cap_presence_guard() {
        use holon_pbt_core::composition::CapProvider;

        // The REAL widest CapMap the keystone drives: `full_headless` booted through the
        // production builder (`boot_and_seed_wide`), INCLUDING the `ComposedSpanMetrics`
        // span-metrics caps it registers on top of the bare `compose_sut` map. The bare
        // `full_headless_cap_set()` is only the generation-narrowing hint and omits those,
        // so checking against it would false-flag `ComposedBudget`.
        let ref_state = wide_e2e_ref();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build multi-thread runtime");
        let sut_caps = rt.block_on(async {
            let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
            let (caps, _handle, _scaffold) = boot_and_seed_wide(&resolver, &ref_state).await;
            caps.cap_set()
        });
        drop(rt);

        let mut ref_map = CapMap::new();
        CapProvider::register(Arc::new(wide_ref()), &mut ref_map);
        let ref_caps = ref_map.cap_set();

        // cap-name → sorted, de-duped invariant ids that need it (on either axis).
        let mut missing: BTreeMap<&'static str, BTreeSet<&'static str>> = BTreeMap::new();
        for inv in composed_invariant_catalog() {
            let needs = inv.needs();
            let id = inv.id().0;
            for cap in &needs.sut_present {
                if !sut_caps.contains(cap) {
                    missing.entry(cap.name()).or_default().insert(id);
                }
            }
            for cap in &needs.ref_present {
                if !ref_caps.contains(cap) {
                    missing.entry(cap.name()).or_default().insert(id);
                }
            }
        }

        let excluded: BTreeSet<&'static str> =
            WIDE_HEADLESS_ABSENT_CAPS.iter().map(|(c, _)| *c).collect();

        // Every excluded cap must ACTUALLY be missing — a stale exclusion (a cap that is now
        // present) is itself a smell to prune, so fail loud on it too.
        let stale: Vec<&'static str> = excluded
            .iter()
            .copied()
            .filter(|c| !missing.contains_key(c))
            .collect();
        assert!(
            stale.is_empty(),
            "WIDE_HEADLESS_ABSENT_CAPS lists caps that ARE present in the widest wiring \
             (stale exclusions — remove them): {stale:?}"
        );

        let unexpected: Vec<(&'static str, Vec<&'static str>)> = missing
            .iter()
            .filter(|(cap, _)| !excluded.contains(**cap))
            .map(|(cap, ids)| (*cap, ids.iter().copied().collect()))
            .collect();
        assert!(
            unexpected.is_empty(),
            "the widest wiring (full_headless) is MISSING caps the shared catalog needs, and \
             they are NOT on the WIDE_HEADLESS_ABSENT_CAPS exclusion list — either the cap \
             regressed out of the widest CapMap (fix the wiring) or it is a genuinely \
             headless-absent cap (add it to WIDE_HEADLESS_ABSENT_CAPS with a reason). \
             missing cap → invariant ids that need it: {unexpected:?}"
        );
    }

    /// iOS-PARITY SUBSTRATE PIN (2026-07-06). The iOS GPUI app boots through
    /// `GpuiModule` → `HolonFrontendModule::configure` → `add_frontend`
    /// (frontends/gpui/src/di.rs, mobile.rs). Its ONLY material config delta vs a
    /// desktop boot is `holon_config.crdt.enabled = Some(true)`
    /// (frontends/gpui/src/mobile.rs ~L35), which makes `add_frontend`
    /// (holon-app/src/wiring.rs L148-184) register `LoroModule` AND the Loro
    /// `CrudAuthority(LoroBlockOperations)` — Loro owns block CRUD, SQL mirrors it.
    ///
    /// The composed keystone (`compose_sut(full_headless)`) boots the SAME substrate:
    /// `full_headless()` carries `Projection::EditorState`, so the builder's frontend
    /// arm calls `HeadlessFrontendComponent::new_with_loro(.., loro_enabled=true)`
    /// (builder.rs L279), which sets `crdt.enabled = Some(true)` and boots through
    /// `holon_app::new_from_config_with_di` → `add_frontend` — the exact same DI seam
    /// and `crdt_enabled()` branch the iOS app hits. So both register the Loro
    /// `CrudAuthority`.
    ///
    /// Audited parity table (knob | iOS app | keystone | match):
    ///   crdt.enabled           | Some(true)          | Some(true) via EditorState | YES
    ///   CrudAuthority          | Loro (add_frontend) | Loro (add_frontend)        | YES
    ///   storage backend        | Turso + Loro        | Turso + Loro               | YES
    ///   config seam            | add_frontend        | add_frontend               | YES (same fn)
    ///   locked_keys            | empty               | empty                      | YES
    ///   Actor::UI / MCP actor  | present (window/MCP)| absent (headless)          | by-design (full_headless drops UI)
    ///   db_path / vault root   | app sandbox         | tempdir                    | immaterial (path only)
    ///
    /// This pin fails loud if a future edit drops `EditorState` from `full_headless`
    /// (silently disabling the Loro authority substrate → the keystone would stop
    /// exercising what iOS runs) OR if the builder stops registering the Loro
    /// peer-mesh authority surface (`SutLoro`), which is present ONLY when the frontend
    /// booted its live Loro authority doc (builder.rs L328/L367/L489). Its presence is
    /// the observable proof that the CRDT/Loro-authority substrate is LIVE.
    #[test]
    fn keystone_boots_ios_crdt_loro_authority_substrate() {
        use holon_pbt_core::capabilities::SutLoro;

        // The config the keystone boots MUST carry EditorState — that projection is
        // exactly what drives `crdt.enabled = Some(true)` in the frontend arm, the iOS
        // material knob. (ViewModel + Turso pin the frontend/Turso half.)
        let set = ComponentSet::full_headless();
        assert!(
            set.has_projection(Projection::EditorState),
            "full_headless dropped EditorState — the keystone would boot the frontend arm \
             with crdt.enabled=Some(false), losing the Loro CrudAuthority substrate the iOS \
             app forces via crdt.enabled=Some(true). iOS parity broken."
        );
        assert!(
            set.has_projection(Projection::ViewModel) && set.has_storage(StorageAdapter::Turso),
            "full_headless must keep the Turso-backed frontend (ViewModel) arm — the iOS app \
             boots a real FrontendSession over Turso with Loro on."
        );

        // Boot the real SUT and prove the Loro authority surface is live.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build multi-thread runtime");
        let has_loro_authority = rt.block_on(async {
            let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
            let sut = compose_sut(&set, &resolver).await;
            sut.caps.get::<dyn SutLoro>().is_some()
        });
        drop(rt);
        assert!(
            has_loro_authority,
            "compose_sut(full_headless) did NOT register the Loro peer-mesh authority cap \
             (SutLoro) — the frontend arm booted WITHOUT a live Loro authority doc, so the \
             keystone is NOT exercising the CRDT/Loro-authority substrate the iOS app runs \
             (crdt.enabled=Some(true) → CrudAuthority(Loro)). iOS parity broken."
        );
    }

    /// COUNT FLOOR — belt against silent catalog deletion: the shared catalog has at least its
    /// current size. Rename-proof (counts entries, not ids). Update N when an invariant is
    /// DELIBERATELY removed from the catalog.
    #[test]
    fn composed_catalog_count_floor() {
        // N = today's catalog size (45 without `otel-testing`; `sql_budget` adds one under it).
        const N: usize = 45;
        let len = composed_invariant_catalog().len();
        assert!(
            len >= N,
            "composed catalog shrank to {len} (floor {N}) — an invariant was removed. If \
             deliberate, lower N; otherwise a `wire()` line was lost."
        );
    }
}
