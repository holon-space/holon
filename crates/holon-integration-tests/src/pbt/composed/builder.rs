//! **F2 E3 — the CapMap-by-subsystem builder (increment 2).**
//!
//! `compose_sut(set, resolver)` assembles the SUT `CapMap` for an active
//! [`ComponentSet`] by registering exactly the components that set's subsystems
//! realize, resolving cap overlaps with [`CapMap::merge_missing`] (precedence =
//! registration order, shadowed caps disclosed). This is the one function that
//! replaces the dozen hand-written per-slice `*_wide` builders — see the
//! backlog "★ SCOPED: the CapMap-by-subsystem builder".
//!
//! **Contract (the honest framing).** `compose_sut` provides the **full**
//! cap-set the active subsystems realize — NOT the hand-tuned minimal subsets
//! the slice builders carry (e.g. `sql_loro_wide` drops Loro's `SutLoroLog` to
//! avoid selecting Loro block invariants its seed can't satisfy). Selection
//! (`Needs::selected_against`)
//! + the seed then decide what runs. So this is verified by **cap-set
//!   membership +
//! precedence-shadow reporting**, not byte-parity with the old builders.
//! (Unifying the *seed* so every selected invariant passes is the next
//! increment — the harness.)
//!
//! **Scope (increment 2a).** Storage backends only: Turso
//! (`SqlProjectionComponent`) and Loro (`LoroBackendComponent`), with the
//! Turso+Loro precedence merge that `sql_loro_wide` hand-rolls today. The
//! frontend (`HeadlessFrontendComponent`, async org boot + scaffold capture),
//! editor (`InMemEditorComponent`), and windowed GPUI (E4) arms are not yet
//! wired — guarded fail-loud below, not silently skipped.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use holon::api::BackendEngine;
use holon::sync::LoroDocumentStore;
use holon_api::EntityUri;
use holon_api::repository::CoreOperations;
use holon_api::repository::NewBlock;
use holon_loro::LoroBackend;
use holon_loro_testing::LoroBackendComponent;
use holon_pbt_core::Actor;
use holon_pbt_core::ComponentSet;
use holon_pbt_core::Projection;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutBlockCreate;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutEdgeFieldWrite;
use holon_pbt_core::capabilities::SutEditorMirrorRead;
use holon_pbt_core::capabilities::SutFocus;
use holon_pbt_core::capabilities::SutQueryResults;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;
use holon_pbt_core::types::DocUriMap;

use crate::mutation_driver::DirectUserDriver;
use crate::pbt::driver_input::DriverInputComponent;
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::is_synthetic_ref_id;
use crate::pbt::memory_slice::components::CoreOpsCommit;
use crate::pbt::memory_slice::components::EditorCommitTarget;
use crate::pbt::memory_slice::components::InMemEditorComponent;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::sql_slice::SqlProjectionComponent;
use crate::pbt::sql_slice::builders::new_sql_engine_with_structural_ops;
use crate::pbt::sut_loro::LoroSut;

/// The booted-frontend ids present BEFORE any working tree is seeded — captured
/// so the harness can seed-inject them into the oracle (they filter out of the
/// id-set-exact block comparison). Reads through the `SutBackend` cap.
async fn booted_scaffold_ids(caps: &CapMap) -> BTreeSet<EntityUri> {
    caps.expect::<dyn SutBackend>()
        .block_raw_snapshot()
        .await
        .into_iter()
        .map(|b| b.id.clone())
        .filter(|id| !is_synthetic_ref_id(id))
        .collect()
}

/// The composed SUT plus the aux a `StateMachineTest` harness needs to drive
/// it.
pub struct ComposedSut {
    /// The assembled SUT capability map — what invariant selection runs
    /// against.
    pub caps: CapMap,
    /// The Turso `BackendEngine` backing the canonical store, when present —
    /// the caller seeds the working tree through it (production create op)
    /// and it backs the resolver-sharing block-tree writer. `None` for
    /// Loro-only configs.
    pub engine: Option<Arc<BackendEngine>>,
    /// The booted windowless `FrontendSession` (frontend arm only; `None`
    /// otherwise) — so the windowed repoint can hand it to a gpui window
    /// that RENDERS this same reactive tree while the composed
    /// backend/storage caps read it (§ Round 5, "reuse compose_sut's
    /// boot"). The window binds `session` + `reactive`; `engine` is for MCP
    /// only.
    pub session: Option<Arc<holon_frontend::FrontendSession>>,
    /// The booted frontend `ReactiveEngine` (frontend arm only; `None`
    /// otherwise) — the render tree a gpui window paints. Pairs with
    /// `session` for the windowed bind.
    pub reactive: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,
    /// The booted frontend component (frontend arm only; `None` otherwise) —
    /// surfaced so the windowed overlay (`overlay_windowed_caps`) can
    /// INSERT the gesture-write caps bound to the window driver via
    /// `register_gesture_writes` (§8.12 C-3). Kept CONCRETE (not a
    /// `dyn GestureWriteSource`) because the windowed base's pre-overlay seed
    /// focus-align (`boot_and_seed_wide_windowed_base`) needs the
    /// component's own headless `driver()` to drive `NavigateFocus` through
    /// a throwaway gesture map — a bare trait object can't provide that, so
    /// the trait indirection is net-negative.
    pub frontend: Option<Arc<HeadlessFrontendComponent>>,
    /// Whether the harness must use a multi-thread runtime (a booted
    /// `FrontendSession` needs it). False for the synchronous storage backends.
    pub multi_thread: bool,
    /// CDC settle window after a write (≈0 for the synchronous in-memory
    /// stores).
    pub settle: Duration,
    /// Booted-frontend scaffold ids to seed-inject into the oracle (empty until
    /// the frontend arm lands).
    pub scaffold_ids: BTreeSet<EntityUri>,
    /// Caps a lower-precedence component would have provided but were already
    /// supplied by a higher-precedence one — disclosed, never silently dropped.
    pub shadowed: Vec<&'static str>,
}

/// Assemble the SUT `CapMap` for `set`, threading `resolver` into the
/// block-tree writer so the generic reconcile loop works for id-minting (Turso)
/// backends.
///
/// Precedence (registration order, first wins): **Turso** is the canonical
/// `SutBackend`; Loro then contributes only its non-overlapping read caps
/// (`SutLoroLog`/`SutLoroTaskState`) via `merge_missing`, its own `SutBackend`
/// reported in [`ComposedSut::shadowed`].
pub async fn compose_sut(set: &ComponentSet, resolver: &IdResolver) -> ComposedSut {
    compose_sut_seeded(set, resolver, DEFAULT_FRONTEND_SEED_ORG, &[]).await
}

/// Which driver backs the composed gesture caps (`SutDriver` /
/// `SutBlockInteract` / `SutArrowNavigate`) — §8.11 layer-localization. Making
/// this an explicit choice (not a registration-order accident) lets the
/// windowed repoint build a base with NO driver caps and then INSERT the
/// window's `GpuiUserDriver`-backed ones exactly once, instead of registering
/// the headless driver and silently overriding it.
pub enum DriverPlacement {
    /// Install the frontend's own headless `ReactiveEngineDriver` as the
    /// VM-rung driver (the default for every headless config — the gesture
    /// alphabet drives UI-adjacently over the same live
    /// `HeadlessEditorMirror` the editor-write caps share).
    HeadlessReactive,
    /// Do NOT install any driver — leave the gesture caps ABSENT so a higher
    /// rung (the windowed `GpuiUserDriver`, via
    /// [`crate::pbt::window_slice::builders::overlay_windowed_caps`])
    /// supplies them. Used only by [`compose_sut_windowed_base`].
    Deferred,
}

/// The headless BASE for the §Round-5 windowed repoint: a full `compose_sut`
/// CapMap with the driver rung **deferred** (`DriverPlacement::Deferred`) —
/// everything (backend / storage / editor / ViewModel caps + the `IdResolver`
/// reconcile) EXCEPT the gesture-driver caps. A windowed harness boots this
/// headless, attaches a gpui window as a pure renderer over the
/// same `reactive`, and inserts the window's `GpuiUserDriver`-backed gesture
/// caps via `overlay_windowed_caps`. Because the base never registered a
/// driver, those inserts are the SOLE providers — no cap is
/// registered-then-overridden.
pub async fn compose_sut_windowed_base(set: &ComponentSet, resolver: &IdResolver) -> ComposedSut {
    compose_sut_seeded_impl(
        set,
        resolver,
        DEFAULT_FRONTEND_SEED_ORG,
        &[],
        DriverPlacement::Deferred,
    )
    .await
}

/// [`compose_sut_windowed_base`] with a caller-chosen boot seed: the wide
/// windowed repoint boots its working tree AS org (`WIDE_TREE_ORG`, so the
/// attached window RENDERS it) with the driver rung deferred for
/// `overlay_windowed_caps`. Same body as [`compose_sut_seeded`] but
/// `DriverPlacement::Deferred` — the driver-deferred dual of the seeded entry.
pub async fn compose_sut_windowed_base_seeded(
    set: &ComponentSet,
    resolver: &IdResolver,
    frontend_seed_org: &[(&str, &str)],
    seed_tree: &[NewBlock],
) -> ComposedSut {
    compose_sut_seeded_impl(
        set,
        resolver,
        frontend_seed_org,
        seed_tree,
        DriverPlacement::Deferred,
    )
    .await
}

/// The frontend arm's default boot org — a minimal one-heading doc. Callers
/// that need the working tree to live IN the org (so
/// `SutOrgRead`/`inv-blocks-match-ref/org` see it, rather than an engine-only
/// seed that the org parse misses) pass their tree AS org via
/// [`compose_sut_seeded`].
pub const DEFAULT_FRONTEND_SEED_ORG: &[(&str, &str)] =
    &[("doc0.org", "#+ID: ref-doc-0\n* Doc zero\n")];

/// [`compose_sut`] with the boot seed chosen by the caller. The seed has two
/// faces, one per arm, so the **builder owns boot+seed for every config** (the
/// backend never escapes it):
/// - `frontend_seed_org` — the org files the **frontend (ViewModel) arm** boots
///   from; its session ingests them into the store (this is how the wide swap
///   boots its working tree AS org over the SAME production builder the
///   `general_e2e_pbt` swap targets — no hand-rolled CapMap lookalike). The
///   non-frontend arms have no session and ignore it.
/// - `seed_tree` — the working tree the **non-frontend arms** create directly
///   into the canonical backend via `CoreOperations::create_block` (in list
///   order, so children follow their parent). Empty for configs that need no
///   working tree; the frontend arm ignores it (it seeds from org). The caller
///   keeps the two faces in agreement (the wide swap derives both from the same
///   fixed ids + contents).
pub async fn compose_sut_seeded(
    set: &ComponentSet,
    resolver: &IdResolver,
    frontend_seed_org: &[(&str, &str)],
    seed_tree: &[NewBlock],
) -> ComposedSut {
    compose_sut_seeded_impl(
        set,
        resolver,
        frontend_seed_org,
        seed_tree,
        DriverPlacement::HeadlessReactive,
    )
    .await
}

/// The workhorse behind [`compose_sut_seeded`] / [`compose_sut_windowed_base`].
/// Identical for every config except the driver rung, chosen by
/// `driver_placement` (see [`DriverPlacement`]).
async fn compose_sut_seeded_impl(
    set: &ComponentSet,
    resolver: &IdResolver,
    frontend_seed_org: &[(&str, &str)],
    seed_tree: &[NewBlock],
    driver_placement: DriverPlacement,
) -> ComposedSut {
    let has_turso = set.has_storage(StorageAdapter::Turso);
    let has_loro = set.has_storage(StorageAdapter::Loro);
    let has_frontend = set.has_projection(Projection::ViewModel);
    let has_editor = set.has_projection(Projection::EditorState);

    assert!(
        has_turso || has_loro,
        "compose_sut needs at least one storage backend (Turso or Loro); got {set:?}"
    );
    assert!(
        !has_frontend || has_turso,
        "compose_sut: the frontend (ViewModel) arm boots over a Turso backend; a ViewModel \
         projection without Turso storage is unsupported. got {set:?}"
    );
    // `compose_sut` builds the HEADLESS CapMap on the tokio runtime. A GPUI window
    // has thread affinity — it must be launched on the gpui thread by the windowed
    // harness, which then hands its live handles to the sibling entry. So a UI set
    // here is unbuildable by construction; fail loud rather than silently composing
    // an under-provisioned SUT that would deselect invariants and look "fine".
    assert!(
        !set.has_actor(Actor::UI),
        "compose_sut builds the headless CapMap on tokio; a GPUI window has thread affinity and \
         cannot be booted here. Build the windowed CapMap with \
         `window_slice::builders::compose_windowed_sut(set, geometry, engine, driver)` from the \
         gpui-thread harness's live window handles instead. got {set:?}"
    );

    let mut caps = CapMap::new();
    let mut shadowed = Vec::new();
    let mut engine = None;
    // Frontend-arm handles surfaced for the windowed repoint (window renders this
    // session/reactive; §Round 5). `None` for the non-frontend arms.
    let mut session: Option<Arc<holon_frontend::FrontendSession>> = None;
    let mut reactive: Option<Arc<holon_frontend::reactive::ReactiveEngine>> = None;
    let mut frontend: Option<Arc<HeadlessFrontendComponent>> = None;
    let mut multi_thread = false;
    let mut settle = Duration::ZERO;
    let mut scaffold_ids = BTreeSet::new();
    // The Loro backend Arc, kept for the editor arm's commit target when Loro is
    // the canonical (no-Turso) backend (`LoroBackend: CoreOperations`).
    let mut loro_backend: Option<Arc<LoroBackend>> = None;
    // The frontend's `LoroDocumentStore` (when the frontend boots with Loro on,
    // i.e. an editor config). The Loro arm builds its read caps over THIS
    // store's authority doc so a write through the frontend op pipeline is
    // visible to them (task #4).
    let mut frontend_loro_store: Option<holon::sync::LoroDocumentStore> = None;
    // The frontend session's `LoroSyncController` handle (when the frontend boots
    // with Loro on). In full mode (`has_turso && has_loro`) the Loro arm hands
    // this to `LoroSut` so a `MergeFromPeer` waits for the controller to
    // project the imported peer delta into Turso `block_raw` before the block
    // invariants read it. `None` in the pure-Loro fast path (no controller, no
    // SQL mirror — quiescence is a no-op).
    let mut frontend_sync_handle: Option<Arc<holon::sync::LoroSyncControllerHandle>> = None;

    // Canonical backend (Turso wins precedence, registered first).
    if has_turso && has_frontend {
        // Frontend arm: a real windowless `FrontendSession` over Turso IS the
        // canonical backend AND supplies the ViewModel/Renderer/Org/watch/nav caps.
        // Boots from a fixed minimal org page; the caller seeds the working tree
        // through `engine` afterward (the boot scaffold is captured here, pre-seed).
        // Loro ON when this config drives the editor: the frontend session then hosts
        // the REAL headless editor (`HeadlessEditorMirror` over `MutableText`), which
        // is why the editor arm below uses these caps instead of the
        // `InMemEditorComponent` stand-in. Loro OFF for non-editor frontend
        // configs (the Loro read caps, when `has_loro`, still come from the
        // separate Loro arm below). Inject the fixed keystone clock (Fork A) so
        // the production ClockScheduler seeds the `clock` day row at a
        // DETERMINISTIC date on every composed frontend boot. The boot
        // auto-create rule (`journals_auto_create_blocks`,
        // seeded by `build_default_layout_blocks`) then fires ONE journal day-block
        // whose content + deterministic id are known to the oracle
        // (`keystone_boot_journal_id`). Without a fixed clock the boot journal's
        // date would be the wall-clock day — non-deterministic, midnight-racy.
        // `SutClockAdvance` is intentionally NOT registered (AdvanceDay stays
        // dormant), so the clock never advances past boot in the keystone.
        let comp = Arc::new(
            HeadlessFrontendComponent::new_with_clock(
                frontend_seed_org,
                Duration::from_millis(300),
                has_editor,
                crate::pbt::frontend_slice::components::keystone_boot_clock(),
            )
            .await,
        );
        let eng = comp.engine();
        // Share the reconcile resolver so the component's id-taking nav/focus caps
        // (pin_block/navigate_focus/focus_editable_text) translate oracle synthetic
        // ids (e.g. `block::split-N`) to the real minted ids — the same map the
        // block-tree writer below uses. Without this, pinning/focusing a post-split
        // synthetic id targets a ghost and diverges `inv-focus-roots`/`inv-nav-focus`.
        comp.set_resolver(resolver.clone());
        // Register the NON-gesture caps (reads/projections/lifecycle). The
        // gesture-write family (`SutBlockTreeWrite`/`SutFocusWrite`/
        // `SutEditorMirrorWrite`/`SutMutate`) is registered separately, gated
        // by `driver_placement`, so the windowed Deferred base can withhold it
        // until a window exists (see the driver-placement block below).
        comp.clone().register_non_gesture(&mut caps);
        // `SutSqlProjection` (the TursoProjection focus/nav matview cap) is NOT in the
        // component's `register` (the nav slice adds it explicitly); add it here since
        // Turso storage is active and its invariants belong to this config.
        caps.insert(comp.clone() as Arc<dyn SutSqlProjection>);
        // `SutFocus` (C-5 split off `SutSqlProjection`, 2026-07-02): the
        // real navigation-focus matview surface. This frontend drives navigation
        // (Turso `current_focus`/`focus_roots`/`navigation_history`), so it hosts the
        // cap and `inv-navigation-focus`/`inv-focus-roots` keep their WIDE_REQUIRED
        // teeth in the keystone. Storage-only configs never reach this arm, so they
        // deselect those invariants honestly.
        caps.insert(comp.clone() as Arc<dyn SutFocus>);
        // `SutQueryResults` (the full-mode query-engine row-count surface) — same
        // per-config rationale as `SutSqlProjection`: NOT in `register`, added here
        // because a real query engine (Turso `BackendEngine` + reactive watch) backs
        // this frontend. Its presence keeps `inv-viewmodel-decompiled-rows-match-query`
        // selected for the wide composed PBT and (via `sut_absent`) keeps the degraded
        // `inv-viewmodel-shows-source-when-no-query` twin deselected here.
        caps.insert(comp.clone() as Arc<dyn SutQueryResults>);
        // The editor READ cap (the WRITE cap is in `register`). Selects the
        // `inv-editor-{text,caret}-matches-ref` invariants — added only when the config
        // drives the editor, so non-editor frontend configs keep their selection.
        if has_editor {
            caps.insert(comp.clone() as Arc<dyn SutEditorMirrorRead>);
        }
        // `SutFixtureFs` (write_org_file into the session's watched org_root) —
        // admits `WriteOrgFile` into the composed alphabet. This is the ONLY
        // transition that can mint an advice-rule block (ADR 0022 step 4), so
        // without this cap the two advice keystone invariants are structurally
        // vacuous (no rule ever enters the reference; empty-vs-empty stays
        // green). The four other `SutFixtureFs` transitions (CreateDirectory /
        // GitInit / JjGitInit / CreateStaleLoro) are `!app_started`-gated and
        // the composed oracle boots pre-started, so they stay honestly
        // unreachable (their cap methods fail loud if that ever changes).
        caps.insert(comp.clone() as Arc<dyn holon_pbt_core::capabilities::SutFixtureFs>);
        // Edge-field write cap (`tags` / `requires` on an existing block) — hosted
        // only when a Loro authority doc is present (`loro_doc_store()` is `Some`
        // iff Loro is on for this build). Routes through the production
        // `set_block_{tags,requires}` over that doc → `project()` → SQL, so the
        // composed `/matview` invariant catches a dropped edge-field re-projection
        // (H12). Absent Loro → not hosted → `SetEdgeField` honestly narrows out.
        // Hosted only when a Loro authority doc is present (edge-field writes are
        // Loro-authority-mode only). Routed through the production engine's
        // `set_field` op so the write is journaled for undo (`UndoLastMutation`).
        if comp.loro_doc_store().is_some() {
            caps.insert(Arc::new(crate::pbt::op_write_cap::EdgeFieldWriter::new(
                comp.engine(),
                resolver.clone(),
            )) as Arc<dyn SutEdgeFieldWrite>);
        }
        // The VM-rung driver (§8.11 layer-localization): install the frontend's OWN
        // headless `ReactiveEngineDriver` as THE driver backing the gesture caps, so
        // the composed `CapMap` drives user gestures UI-adjacently (click-intent
        // resolution + `MutableText`/`InputState` editing — the same production logic
        // the GPUI window runs, minus geometry/platform), not via `OpDispatchWriter`
        // dispatch. This admits the `SutBlockInteract`/`SutArrowNavigate` gesture
        // alphabet headless (ClickBlock/ExpandToggle/PressKey/…). Reuses the
        // component's driver (not a fresh one) so the single live
        // `HeadlessEditorMirror` stays shared with the editor-write caps. No
        // geometry → its `require_bounds` precheck is a no-op (the driver
        // resolves targets itself). Driver rung placement (§8.11): install the
        // headless VM-rung driver AND the gesture-write family bound to it ONLY
        // when the caller asked for it. The windowed repoint passes `Deferred`
        // and later INSERTs the live window's `GpuiUserDriver`-backed
        // gesture caps via `overlay_windowed_caps`, so the whole gesture rung
        // (`SutDriver`/`SutBlockInteract`/`SutArrowNavigate` +
        // `SutBlockTreeWrite`/`SutFocusWrite`/ `SutEditorMirrorWrite`/
        // `SutMutate`) is inserted ONCE by whoever owns the rung — never
        // registered here and then overridden. The gesture writes ride the component's
        // OWN `ReactiveEngineDriver` (keystroke split / sidebar-click focus /
        // editor keystrokes / `state_toggle` clicks) — the UI-adjacent VM rung,
        // not raw op dispatch; the resolver set above makes `SutBlockTreeWrite`
        // the shared-resolver keystroke writer.
        if matches!(driver_placement, DriverPlacement::HeadlessReactive) {
            comp.clone()
                .register_gesture_writes(&mut caps, comp.driver());
            Arc::new(DriverInputComponent::with_input_headless(
                comp.reactive(),
                comp.driver(),
                resolver.clone(),
            ))
            .register(&mut caps);
        }
        // Capture the frontend's Loro authority store (present iff Loro is on for this
        // build = an editor config) so the Loro arm below reads its caps over the SAME
        // doc the frontend op pipeline writes (task #4 read-doc unification).
        frontend_loro_store = comp.loro_doc_store();
        // In full mode (Loro on = editor config) also capture the sync-controller
        // handle so the Loro arm's `LoroSut` can wait for the controller to
        // project an imported peer delta into Turso `block_raw`. Resolution is
        // RACE-prone (`without_wait()` → spawned `post_ready_work`,
        // `wiring.rs:360`), so POLL until present — and FAIL LOUD if it never
        // resolves, because full-mode peer projection has no controller without
        // it (don't silently degrade to a no-op quiescence wait). Validated by
        // the A0 probe `headless_loro_sync_controller_resolves_after_boot`.
        if has_editor {
            // Scale-soak: a 5-10k-block org ingest delays controller resolution far past
            // the 2s default; scale the poll budget with `HOLON_SOAK_SETTLE_MS` (never
            // below the 2s default) so the fail-loud assert below stays meaningful.
            let boot_poll_ms: u64 = std::env::var("HOLON_SOAK_SETTLE_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(2_000)
                .max(2_000);
            for _ in 0..(boot_poll_ms / 50) {
                frontend_sync_handle = comp.loro_sync_handle();
                if frontend_sync_handle.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert!(
                frontend_sync_handle.is_some(),
                "compose_sut full mode: LoroSyncControllerHandle never resolved within the boot \
                 poll budget (>=2s, HOLON_SOAK_SETTLE_MS-scaled) of headless boot — peer deltas \
                 would not project to Turso. See the A0 probe."
            );
        }
        scaffold_ids = booted_scaffold_ids(&caps).await;
        engine = Some(eng);
        // Surface the booted session + reactive so a windowed harness can attach a gpui
        // window as a renderer over this same reactive tree (§Round 5 windowed
        // repoint).
        session = Some(comp.session());
        reactive = Some(comp.reactive());
        // Surface the component so the windowed overlay can INSERT its gesture-write
        // caps bound to the window driver (§8.12 C-3), and so the base seed
        // focus-align can drive `NavigateFocus` through the component's own
        // headless driver.
        frontend = Some(comp.clone());
        multi_thread = true; // the booted FrontendSession needs a multi-thread runtime
        settle = Duration::from_millis(150);
    } else if has_turso {
        let eng = new_sql_engine_with_structural_ops().await;
        Arc::new(SqlProjectionComponent::new(eng.clone())).register(&mut caps);
        // Override the block-tree writer with the shared-resolver DISPATCH FLOOR
        // (§8.11 LL-2): a storage-only config has no ViewModel/UI, so no higher driver
        // is available — the floor `DirectUserDriver` applies each structural op
        // directly to the engine (`synthetic_dispatch` == the old `OpDispatchWriter`
        // no-focus-sink path, behavior-identical), the bottom rung the subsystem
        // bisector descends to. `SqlProjectionComponent::register` installs a
        // fresh-resolver writer; this replaces it under the same cap `TypeId`
        // (explicit `replace` — a plain `insert` would fail loud on the duplicate).
        let floor = Arc::new(DirectUserDriver::with_resolver(
            eng.clone(),
            resolver.clone(),
        ));
        caps.replace(floor.clone() as Arc<dyn SutBlockTreeWrite>);
        // Also expose the op-floor `SutBlockCreate` so `CreateBlockUnderFocus`
        // runs under this no-UI (storage-only) pin — deterministic `block.create`
        // dispatch with AND without an explicit id (the mint-when-absent path),
        // which the UI-only creation-slot gesture could not reach here.
        caps.insert(floor as Arc<dyn SutBlockCreate>);
        engine = Some(eng);
    }

    // Loro: canonical `SutBackend` only when Turso is absent; otherwise
    // `merge_missing` keeps Turso's backend and adds Loro's read caps
    // (`SutLoroLog`/`SutLoroTaskState`), reporting the shadowed overlap. The
    // peer-mesh surface (`SutLoro`) is contributed ONLY when Loro is the
    // canonical backend (no Turso) — see the gate below.
    if has_loro {
        // Build a `LoroDocumentStore` and hand the read component a backend over its
        // global doc, so the read caps AND the peer-mesh `LoroSut` observe ONE shared
        // doc (a merged peer block must show up in the store invariants read). The
        // loro-only fast config never persists (no `save_all`), so `get_global_doc`
        // caches the doc in memory and the tempdir is needed only to construct the
        // store — it can drop immediately.
        // Read-doc unification (task #4): when the frontend booted with Loro on, build
        // the read caps over the FRONTEND's authority store, so a write through the
        // frontend op pipeline (e.g. `ToggleState`'s `cycle_task_state`) is visible to
        // `SutLoroTaskState`/`SutLoroLog`. Otherwise (non-frontend Loro configs) fall
        // back to a fresh store (the pure-Loro / peer-mesh path).
        let doc_store = if let Some(store) = frontend_loro_store.clone() {
            Arc::new(tokio::sync::RwLock::new(store))
        } else {
            let tempdir = tempfile::tempdir().expect("create loro tempdir");
            Arc::new(tokio::sync::RwLock::new(LoroDocumentStore::new(
                tempdir.path().to_path_buf(),
            )))
        };
        let global_doc = doc_store
            .read()
            .await
            .get_global_doc()
            .await
            .expect("init loro global doc");
        let backend = Arc::new(LoroBackend::from_document(global_doc));
        loro_backend = Some(backend.clone());

        let mut loro_caps = CapMap::new();
        // Full mode (frontend booted Loro on) carries the live sync-controller
        // handle captured above; hand it to the read component so
        // `SutLoroLog::loro_had_errors` reports the controller's REAL error
        // counter — the `inv-loro-no-errors` teeth in the keystone. Non-frontend
        // Loro configs have no controller (`frontend_sync_handle` is `None`), so
        // `loro_had_errors` honestly stays `false` there.
        Arc::new(LoroBackendComponent::new_shared_with_sync_handle(
            backend,
            frontend_sync_handle.clone(),
        ))
        .register(&mut loro_caps);
        // The peer-mesh surface (`SutLoro`) is contributed whenever the `doc_store` it
        // drives over is the SAME doc whose mutations reach the canonical `SutBackend`
        // the block invariants read — so a merged peer block is observable there:
        //
        //   * Pure-Loro (`!has_turso`): the Loro doc IS the canonical backend; read
        //     caps
        //     + peer mesh + `SutBackend` all observe the one shared global doc. No
        //     `LoroSyncController` (no SQL mirror), so `sync_handle = None` and
        //     `wait_for_quiescence` no-ops.
        //   * Full mode (`has_turso` + editor):
        //     `frontend_loro_store`/`frontend_sync_handle` are present, so `doc_store`
        //     IS the frontend's authority doc and the captured controller projects an
        //     imported peer delta into Turso `block_raw` (the canonical `SutBackend`).
        //     `wait_for_quiescence(sync_handle)` inside
        //     `apply_merge_from_peer`/`apply_sync_with_peer` blocks until that
        //     projection lands — so the peer-aware per-tick reconcile (`harness.rs`)
        //     sees the row.
        //
        // WITHHELD only in full mode WITHOUT an editor: there the frontend booted Loro
        // OFF, so `doc_store` is a FRESH store no controller watches — a peer merge
        // would land in a Loro doc invisible to the Turso backend and diverge
        // `inv-blocks-match-ref`. `frontend_sync_handle` is `None` there, so the gate
        // `!has_turso || frontend_sync_handle.is_some()` correctly skips it and the
        // peer transitions AUTO-NARROW out (honest cap-presence).
        //
        // `doc_uri_map` stays empty/identity: peer blocks carry a stable deterministic
        // `peer-…` id agreed by both oracle and SUT (no UUID minted), so
        // `resolve_stable_id`'s identity fallback resolves them.
        if !has_turso || frontend_sync_handle.is_some() {
            let doc_uri_map: DocUriMap =
                Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
            Arc::new(LoroSut::new(
                doc_store,
                frontend_sync_handle.clone(),
                doc_uri_map,
            ))
            .register(&mut loro_caps);
        }
        shadowed.extend(caps.merge_missing(loro_caps));
    }

    // Editor arm. When a frontend is present it already hosts the REAL headless
    // editor (the frontend arm above registered the read+write caps over the
    // production `HeadlessEditorMirror`, Loro on) — the North Star end state,
    // so no stand-in here. For a NON-frontend editor config (e.g. loro+editor,
    // or turso+editor without a ViewModel) there is no frontend session, so a
    // headless `InMemEditorComponent` provides `SutEditorMirrorRead/Write`,
    // committing into the CANONICAL backend so a committed keystroke lands
    // where the block invariants read: the Turso `BackendEngine` (`set_field`
    // op) when Turso is canonical, else the Loro store (`CoreOperations`). The
    // editor caps don't overlap any storage cap, so no shadow.
    if has_editor && !has_frontend {
        let commit: Arc<dyn EditorCommitTarget> = if let Some(eng) = &engine {
            eng.clone() as Arc<dyn EditorCommitTarget>
        } else if let Some(lb) = &loro_backend {
            Arc::new(CoreOpsCommit(lb.clone() as Arc<dyn CoreOperations>))
        } else {
            unreachable!("compose_sut: a storage backend is asserted present above")
        };
        Arc::new(InMemEditorComponent::new_commit(commit)).register(&mut caps);
    }

    // Non-frontend seed: a storage-only config (Loro-only / Turso-only) has no
    // session to ingest `frontend_seed_org`, so create the working tree
    // directly into the canonical backend (the SAME store backing the
    // `SutBackend` cap) — the transition alphabet only EDITS a pre-seeded tree
    // (split/join/…); creating the initial tree is a boot concern, done here so
    // the backend never leaves the builder. Frontend configs already seeded via
    // the org boot above, so skip.
    if !has_frontend && !seed_tree.is_empty() {
        // Loro is the canonical backend for every non-frontend config the wide swap
        // reaches (`set_for_wiring` makes Turso ⟹ ViewModel ⟹ frontend, so a
        // storage-only seed config is Loro-backed). `BackendEngine` deliberately does
        // not impl `CoreOperations`, so fail loud rather than silently skip the seed.
        let backend = loro_backend.as_ref().expect(
            "non-frontend seed_tree requires a Loro backend (Turso-only seeding has no \
             CoreOperations path here); a non-frontend Turso config cannot be seeded this way",
        );
        for nb in seed_tree {
            let created = backend
                .create_block(nb.parent_id.clone(), nb.content.clone(), nb.id.clone())
                .await
                .expect("seed non-frontend working tree block");
            // Doc-root ⟹ `Page`. The FRONTEND arm gets this for free from the org
            // boot (`WIDE_TREE_ORG`'s `#+ID:` head → parser `set_page(true)` on the
            // doc-root, parser.rs). The direct Loro seed here bypasses org, so a
            // page-rooted seed block would boot WITHOUT its `Page` tag — the
            // sidebar `tag='Page'` projection (and `inv-sidebar-page-tag-preserved`)
            // then correctly report it DEMOTED at boot. Mirror the parser's rule so
            // the Loro-only SUT seed is prod-faithful (every doc-root is a page).
            if nb.parent_id.is_no_parent() || nb.parent_id.is_sentinel() {
                backend
                    .set_block_tags(created.id.as_str(), &["Page".to_string()])
                    .await
                    .expect("tag seed doc-root as Page");
            }
        }
    }

    ComposedSut {
        caps,
        engine,
        session,
        reactive,
        frontend,
        multi_thread,
        settle,
        scaffold_ids,
        shadowed,
    }
}

#[cfg(test)]
mod tests {
    use holon_pbt_core::capabilities::SutBackend;
    use holon_pbt_core::capabilities::SutBlockTreeWrite;
    use holon_pbt_core::capabilities::SutEditorMirrorRead;
    use holon_pbt_core::capabilities::SutEditorMirrorWrite;
    use holon_pbt_core::capabilities::SutLoro;
    use holon_pbt_core::capabilities::SutLoroLog;
    use holon_pbt_core::capabilities::SutLoroTaskState;
    use holon_pbt_core::capabilities::SutSqlProjection;

    use super::*;

    // Bare storage sets (no ViewModel/EditorState projections — those arms are
    // deferred). `ComponentSet::sql_only()` bundles projections, so it can't be
    // used for the storage-only cap test yet.
    fn turso_only() -> ComponentSet {
        ComponentSet::of([StorageAdapter::Turso.into()])
    }
    fn loro_only() -> ComponentSet {
        ComponentSet::of([StorageAdapter::Loro.into()])
    }
    fn turso_and_loro() -> ComponentSet {
        ComponentSet::of([StorageAdapter::Turso.into(), StorageAdapter::Loro.into()])
    }
    fn turso_frontend() -> ComponentSet {
        ComponentSet::of([StorageAdapter::Turso.into(), Projection::ViewModel.into()])
    }
    fn turso_frontend_editor() -> ComponentSet {
        ComponentSet::of([
            StorageAdapter::Turso.into(),
            Projection::ViewModel.into(),
            Projection::EditorState.into(),
        ])
    }
    fn loro_editor() -> ComponentSet {
        ComponentSet::of([StorageAdapter::Loro.into(), Projection::EditorState.into()])
    }

    fn resolver() -> IdResolver {
        Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()))
    }

    /// Turso-only: the SQL backend caps, nothing shadowed, an engine to seed
    /// through.
    #[tokio::test]
    async fn compose_sut_turso_only_caps() {
        let sut = compose_sut(&turso_only(), &resolver()).await;
        assert!(sut.caps.get::<dyn SutBackend>().is_some());
        assert!(sut.caps.get::<dyn SutSqlProjection>().is_some());
        assert!(sut.caps.get::<dyn SutBlockTreeWrite>().is_some());
        assert!(sut.caps.get::<dyn SutLoroLog>().is_none());
        assert!(sut.shadowed.is_empty(), "no overlap: {:?}", sut.shadowed);
        assert!(
            sut.engine.is_some(),
            "Turso backend exposes an engine to seed through"
        );
    }

    /// Loro-only: the Loro backend + read caps + the **`SutLoro` peer-mesh
    /// surface** (so the peer transitions
    /// `AddPeer`/`PeerEdit`/`SyncWithPeer`/`MergeFromPeer`
    /// become cap-feasible — the loro-only fast config). Still NO block-tree
    /// write cap: Loro structural block-tree writes have no
    /// `SutBlockTreeWrite` provider here, so the *structural* alphabet
    /// (Split/Join/…) self-gates out, by design.
    #[tokio::test]
    async fn compose_sut_loro_only_caps() {
        let sut = compose_sut(&loro_only(), &resolver()).await;
        assert!(sut.caps.get::<dyn SutBackend>().is_some());
        assert!(sut.caps.get::<dyn SutLoroLog>().is_some());
        assert!(sut.caps.get::<dyn SutLoroTaskState>().is_some());
        assert!(
            sut.caps.get::<dyn SutLoro>().is_some(),
            "the loro arm must contribute the peer-mesh SutLoro cap"
        );
        assert!(sut.caps.get::<dyn SutBlockTreeWrite>().is_none());
        assert!(sut.engine.is_none());
    }

    /// **The loro-only fast-config payoff: peer transitions drive over the
    /// composed `CapMap`.** Before the `LoroSut`-as-`CapProvider` arm, a
    /// Loro `CapMap` had no `SutLoro` so the peer transitions gated out
    /// (PCG-5a). Now the arm wires the peer mesh over the SAME shared
    /// global doc the read caps observe, so the WIDE `E2ETransition` enum
    /// dispatches `AddPeer`→`PeerEdit`→`MergeFromPeer` over `&mut CapMap`
    /// and a peer-created block actually lands in the primary store the
    /// `SutBackend` cap reads — the end-to-end shared-doc proof (a non-shared
    /// store would leave the merged block invisible and fail this
    /// assertion).
    #[tokio::test(flavor = "multi_thread")]
    async fn compose_sut_loro_arm_drives_peer_transitions() {
        use holon_pbt_core::TransitionImpl;
        use holon_pbt_core::capabilities::PeerEditOp;

        use crate::pbt::reference_state::ReferenceState;
        use crate::pbt::state_machine::fresh_reference_state;
        use crate::pbt::transitions::AddPeer;
        use crate::pbt::transitions::E2ETransition;
        use crate::pbt::transitions::MergeFromPeer;
        use crate::pbt::transitions::PeerEdit;

        /// Drop a `ReferenceState` (owns an `Arc<tokio::Runtime>`) off the
        /// async executor — dropping it inside a `#[tokio::test]`
        /// context panics.
        fn drop_ref_off_thread(state: ReferenceState) {
            std::thread::spawn(move || drop(state))
                .join()
                .expect("drop ReferenceState off the async executor");
        }

        let set = loro_only();
        let composed = compose_sut(&set, &resolver()).await;
        let mut caps = composed.caps;
        // The cap gate's RHS — wired exactly as the eventual E2ESut→CapMap swap will.
        let oracle = fresh_reference_state(set.wiring.clone()).with_cap_set(caps.cap_set());

        // AddPeer is cap-feasible now that the arm provides `SutLoro` (it was gated out
        // of the loro `CapMap` before — PCG-5a's discrimination).
        let add = E2ETransition::AddPeer(AddPeer);
        assert!(
            oracle.caps_available(&add.required_caps()),
            "the loro cap set must admit the peer transitions"
        );

        // Drive AddPeer → PeerEdit(create) → MergeFromPeer over the WIDE enum on the
        // composed `&mut CapMap` (the `&oracle` arg is unused by the peer ops).
        TransitionImpl::apply_to_sut(&add, &oracle, &mut caps).await;
        let create = E2ETransition::PeerEdit(PeerEdit {
            peer_idx: 0,
            op: PeerEditOp::Create {
                parent_stable_id: None,
                content: "from-peer".to_string(),
                stable_id: "peer-created-block".to_string(),
            },
        });
        TransitionImpl::apply_to_sut(&create, &oracle, &mut caps).await;
        let merge = E2ETransition::MergeFromPeer(MergeFromPeer { peer_idx: 0 });
        TransitionImpl::apply_to_sut(&merge, &oracle, &mut caps).await;

        // The merged block must be visible in the PRIMARY store the read caps observe —
        // proves the peer mesh and the `SutBackend` read cap share one global doc.
        let blocks = caps.expect::<dyn SutBackend>().block_raw_snapshot().await;
        assert!(
            blocks.iter().any(|b| b.content == "from-peer"),
            "the peer-created+merged block must appear in the shared primary store; got {:?}",
            blocks.iter().map(|b| b.content.clone()).collect::<Vec<_>>()
        );

        drop_ref_off_thread(oracle);
    }

    /// A5 TEETH (full-mode peer mesh): the full-mode analogue of the loro-only
    /// proof above. Over `compose_sut(full_headless())` — Turso is the
    /// canonical `SutBackend`, Loro is the authority store the frontend's
    /// `LoroSyncController` watches — drive `AddPeer → PeerEdit(Create) →
    /// MergeFromPeer` and assert the merged block lands in the **Turso**
    /// `block_raw` the block invariants read. The merge's internal
    /// `wait_for_quiescence(sync_handle)` (now wired via A2) blocks until the
    /// controller projects the imported delta — the evidence that the async
    /// projection completed (no bare sleep). Also asserts the row's id is
    /// `block:peer-…` (the stable id A2's identity resolution and
    /// A-RECONCILE's peer-scheme exclusion both depend on).
    #[tokio::test(flavor = "multi_thread")]
    async fn compose_sut_full_headless_peer_mesh_projects_to_turso() {
        use holon_pbt_core::TransitionImpl;
        use holon_pbt_core::capabilities::PeerEditOp;

        use crate::pbt::reference_state::ReferenceState;
        use crate::pbt::state_machine::fresh_reference_state;
        use crate::pbt::transitions::AddPeer;
        use crate::pbt::transitions::E2ETransition;
        use crate::pbt::transitions::MergeFromPeer;
        use crate::pbt::transitions::PeerEdit;

        fn drop_ref_off_thread(state: ReferenceState) {
            std::thread::spawn(move || drop(state))
                .join()
                .expect("drop ReferenceState off the async executor");
        }

        let set = ComponentSet::full_headless();
        let composed = compose_sut(&set, &resolver()).await;
        let mut caps = composed.caps;
        // The peer mesh must be present in full mode now (A2) — withholding it is the
        // exact divergence Part A closes.
        assert!(
            caps.get::<dyn SutLoro>().is_some(),
            "full_headless must contribute SutLoro (the full-mode peer mesh)"
        );
        let oracle = fresh_reference_state(set.wiring.clone()).with_cap_set(caps.cap_set());
        let add = E2ETransition::AddPeer(AddPeer);
        assert!(
            oracle.caps_available(&add.required_caps()),
            "full_headless cap set must admit the peer transitions"
        );

        TransitionImpl::apply_to_sut(&add, &oracle, &mut caps).await;
        let create = E2ETransition::PeerEdit(PeerEdit {
            peer_idx: 0,
            op: PeerEditOp::Create {
                parent_stable_id: None,
                content: "from-peer".to_string(),
                stable_id: "peer-fullmode-block".to_string(),
            },
        });
        TransitionImpl::apply_to_sut(&create, &oracle, &mut caps).await;
        let merge = E2ETransition::MergeFromPeer(MergeFromPeer { peer_idx: 0 });
        TransitionImpl::apply_to_sut(&merge, &oracle, &mut caps).await;

        // After the merge's internal `wait_for_quiescence`, the controller has
        // projected the peer delta into the Turso `block_raw` the canonical
        // `SutBackend` reads.
        let blocks = caps.expect::<dyn SutBackend>().block_raw_snapshot().await;
        let merged = blocks.iter().find(|b| b.content == "from-peer");
        assert!(
            merged.is_some(),
            "the peer-merged block must project into the Turso SutBackend; got {:?}",
            blocks.iter().map(|b| b.content.clone()).collect::<Vec<_>>()
        );
        assert!(
            merged.unwrap().id.as_str().starts_with("block:peer-"),
            "the merged row must carry the peer's stable id (block:peer-…); got {:?}",
            merged.unwrap().id
        );

        drop_ref_off_thread(oracle);
    }

    /// Turso+Loro: the precedence merge that `sql_loro_wide` hand-rolls. Turso
    /// is the canonical `SutBackend` (Loro's is reported shadowed); Loro's
    /// read caps are added. Proves `merge_missing` replaces the
    /// hand-written non-overlap insert.
    #[tokio::test]
    async fn compose_sut_turso_and_loro_precedence_merge() {
        let sut = compose_sut(&turso_and_loro(), &resolver()).await;
        // Full union of caps, backend resolved by precedence.
        assert!(sut.caps.get::<dyn SutBackend>().is_some());
        assert!(sut.caps.get::<dyn SutSqlProjection>().is_some());
        assert!(sut.caps.get::<dyn SutBlockTreeWrite>().is_some());
        assert!(sut.caps.get::<dyn SutLoroLog>().is_some());
        assert!(sut.caps.get::<dyn SutLoroTaskState>().is_some());
        // The shadow is DISCLOSED: Loro's SutBackend lost to Turso's.
        assert!(
            sut.shadowed.contains(&"SutBackend"),
            "Turso+Loro must report Loro's shadowed SutBackend; got {:?}",
            sut.shadowed
        );
    }

    /// Frontend arm: a real windowless `HeadlessFrontendComponent` over Turso
    /// is the canonical backend AND supplies the
    /// ViewModel/Renderer/Org/watch/nav caps + `SutSqlProjection`. Boot
    /// leaves a scaffold (captured), needs a multi-thread runtime + a
    /// settle window. This is the arm that folds C2.0's `boot_and_seed`.
    #[tokio::test(flavor = "multi_thread")]
    async fn compose_sut_frontend_arm_caps_and_aux() {
        use holon_pbt_core::capabilities::SutOrgRead;
        use holon_pbt_core::capabilities::SutRenderer;
        use holon_pbt_core::capabilities::SutViewSelection;
        use holon_pbt_core::capabilities::SutWatch;
        let sut = compose_sut(&turso_frontend(), &resolver()).await;
        // Backend + structural write + Turso projection.
        assert!(sut.caps.get::<dyn SutBackend>().is_some());
        assert!(sut.caps.get::<dyn SutBlockTreeWrite>().is_some());
        assert!(sut.caps.get::<dyn SutSqlProjection>().is_some());
        // The frontend projection caps.
        assert!(sut.caps.get::<dyn SutViewSelection>().is_some());
        assert!(sut.caps.get::<dyn SutRenderer>().is_some());
        assert!(sut.caps.get::<dyn SutOrgRead>().is_some());
        assert!(sut.caps.get::<dyn SutWatch>().is_some());
        // Aux: engine to seed through, multi-thread, a settle window, and a non-empty
        // booted scaffold to seed-inject into the oracle.
        assert!(sut.engine.is_some());
        assert!(
            sut.multi_thread,
            "booted FrontendSession needs a multi-thread runtime"
        );
        assert!(sut.settle > Duration::ZERO);
        assert!(
            !sut.scaffold_ids.is_empty(),
            "the production boot must leave scaffold blocks to seed-inject"
        );
    }

    /// Editor arm (increment 2c) over a **Turso-canonical** config (the swap
    /// target shape: Turso + ViewModel + EditorState, no UI). The editor
    /// hosts both mirror caps, committing through the canonical
    /// `BackendEngine`'s `set_field` op — so `compose_sut` no longer panics
    /// on `EditorState` and an editor write lands in `block_raw`.
    /// Backend/projection/frontend caps remain present alongside.
    #[tokio::test(flavor = "multi_thread")]
    async fn compose_sut_editor_arm_turso_caps() {
        let sut = compose_sut(&turso_frontend_editor(), &resolver()).await;
        assert!(sut.caps.get::<dyn SutEditorMirrorRead>().is_some());
        assert!(sut.caps.get::<dyn SutEditorMirrorWrite>().is_some());
        // The canonical backend + frontend caps still compose alongside the editor.
        assert!(sut.caps.get::<dyn SutBackend>().is_some());
        assert!(sut.caps.get::<dyn SutSqlProjection>().is_some());
        assert!(
            sut.engine.is_some(),
            "Turso editor commits through the engine"
        );
    }

    /// Editor arm over a **Loro-canonical** config: the editor commits into the
    /// Loro store (`LoroBackend: CoreOperations`, via `CoreOpsCommit`) —
    /// the no-Turso path.
    #[tokio::test]
    async fn compose_sut_editor_arm_loro_caps() {
        let sut = compose_sut(&loro_editor(), &resolver()).await;
        assert!(sut.caps.get::<dyn SutEditorMirrorRead>().is_some());
        assert!(sut.caps.get::<dyn SutEditorMirrorWrite>().is_some());
        assert!(sut.caps.get::<dyn SutLoro>().is_some());
        assert!(sut.caps.get::<dyn SutBackend>().is_some());
        assert!(
            sut.engine.is_none(),
            "Loro-canonical config has no Turso engine"
        );
    }

    /// **The swap-target config composes.** `ComponentSet::full_headless()`
    /// (Loro+Org+ Turso storage + ViewModel + EditorState, no UI) is
    /// exactly what `general_e2e_pbt` drives. Before the editor arm,
    /// `compose_sut` PANICKED on its `EditorState`; now it assembles the
    /// full SUT — the canonical Turso frontend backend, the Loro read caps,
    /// AND the editor mirror caps. This is the unblock that lets a later
    /// increment point the wide PBT at `compose_sut(full_headless)` (still
    /// pending lifecycle/mutate/fixture arms + reconcile generalization +
    /// committed-content parity before it can DRIVE green).
    #[tokio::test(flavor = "multi_thread")]
    async fn compose_sut_full_headless_composes() {
        let sut = compose_sut(&ComponentSet::full_headless(), &resolver()).await;
        // Canonical Turso frontend backend + projection.
        assert!(sut.caps.get::<dyn SutBackend>().is_some());
        assert!(sut.caps.get::<dyn SutSqlProjection>().is_some());
        assert!(sut.caps.get::<dyn SutBlockTreeWrite>().is_some());
        // Loro read caps (storage also has Loro; SutBackend shadowed by Turso,
        // disclosed).
        assert!(sut.caps.get::<dyn SutLoroLog>().is_some());
        // The peer-mesh `SutLoro` is now PRESENT in full mode (Part A): the Loro arm
        // drives over the frontend's shared authority doc with the real
        // sync-controller handle, so a merged peer delta projects into the
        // canonical Turso backend the block invariants read. Peer transitions
        // therefore auto-SELECT into the full_headless swap alphabet (proved by
        // `compose_sut_full_headless_peer_mesh_projects_to_turso` +
        // `full_headless_cap_set_admits_peer_transitions`).
        assert!(sut.caps.get::<dyn SutLoro>().is_some());
        assert!(
            sut.shadowed.contains(&"SutBackend"),
            "Loro's SutBackend is shadowed by the canonical Turso backend; got {:?}",
            sut.shadowed
        );
        // The editor arm.
        assert!(sut.caps.get::<dyn SutEditorMirrorRead>().is_some());
        assert!(sut.caps.get::<dyn SutEditorMirrorWrite>().is_some());
        // The mutate arm (ToggleState) — headless `set_field task_state` via the
        // engine.
        assert!(
            sut.caps
                .get::<dyn holon_pbt_core::capabilities::SutMutate>()
                .is_some()
        );
        assert!(sut.engine.is_some());
    }

    /// Fail-loud: a config with no storage backend is a composition bug, not an
    /// empty SUT that would deselect everything and pass vacuously.
    #[tokio::test]
    #[should_panic(expected = "needs at least one storage backend")]
    async fn compose_sut_rejects_no_storage() {
        let _ = compose_sut(&ComponentSet::of([]), &resolver()).await;
    }

    /// **PCG-5a (early, pre-PCG-4): the wide alphabet auto-narrows to a real
    /// partial `compose_sut` cap set.** This is the North-Star
    /// "env-selected subsystems pick the feasible transitions" property,
    /// proven on the GENERATION side over a genuine `CapMap` (not a
    /// hand-built set). Driving the narrowed alphabet on the `CapMap` is
    /// PCG-5b, gated on PCG-4 (`CapMap: SutHandle` needs the `SutLoro`
    /// dyn-compat flip).
    // Plain `#[test]`, not `#[tokio::test]`: each `ReferenceState` owns an
    // `Arc<tokio::Runtime>`, and dropping that inside an async context panics. We boot
    // `compose_sut` on a scoped current-thread runtime, extract the (runtime-free)
    // `CapSet`, then do all the `ReferenceState` work on this plain thread where the
    // nested runtimes drop cleanly.
    #[test]
    fn wide_alphabet_narrows_to_partial_compose_sut_capset() {
        use std::collections::BTreeSet;

        use holon_pbt_core::TransitionFactory;
        use proptest::strategy::Strategy;
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        use crate::pbt::reference_state::ReferenceState;
        use crate::pbt::state_machine::fresh_reference_state;
        use crate::pbt::transitions::aggregate_transitions;
        use crate::pbt::transitions::{self as t};

        // Monomorphize the generic `TransitionFactory<R>` methods at `R =
        // ReferenceState` (the fine-grained transitions impl the factory for
        // any `R`, so a bare `T::required_caps()` can't infer `R`).
        fn rc<T: TransitionFactory<ReferenceState>>() -> Vec<holon_pbt_core::composition::CapId> {
            T::required_caps()
        }
        fn rw<T: TransitionFactory<ReferenceState>>() -> holon_pbt_core::RequiredWiring {
            T::required_wiring()
        }

        // A REAL partial SUT: Turso storage only → caps {SutBackend, SutSqlProjection,
        // SutBlockTreeWrite}. The wiring is taken from the same `ComponentSet`, so the
        // wiring gate and the cap gate are evaluated against one consistent config.
        let set = turso_only();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        let cap_set = rt.block_on(async {
            let sut = compose_sut(&set, &resolver()).await;
            sut.caps.cap_set()
        });
        drop(rt);
        // The gate's RHS, wired exactly as the composed harness will wire it.
        let state = fresh_reference_state(set.wiring.clone()).with_cap_set(cap_set.clone());

        // ── A. Deterministic discrimination on the real cap set (the gate fn itself).
        // ── Block-tree transitions are cap-feasible; the absent-cap families
        // are not.
        assert!(state.caps_available(&rc::<t::SplitBlock>()));
        assert!(state.caps_available(&rc::<t::JoinBlock>()));
        assert!(state.caps_available(&rc::<t::Indent>()));
        assert!(state.caps_available(&rc::<t::Outdent>()));
        assert!(state.caps_available(&rc::<t::Nothing>())); // no cap needed
        for absent in [
            rc::<t::NavigateFocus>(), // SutFocusWrite
            rc::<t::TypeChars>(),     // SutEditorMirrorWrite
            rc::<t::SetupWatch>(),    // SutWatchRegister
            rc::<t::SwitchView>(),    // SutViewControl
            rc::<t::StartApp>(),      // SutAppLifecycle
            rc::<t::ApplyMutation>(), // SutLoro (absent in turso_only → still gated out)
            rc::<t::ArrowNavigate>(), // SutArrowNavigate
        ] {
            assert!(
                !state.caps_available(&absent),
                "expected cap-gated out: {absent:?}"
            );
        }

        // The cap gate is STRICTLY FINER than the wiring gate: NavigateFocus passes the
        // wiring gate under Turso, yet its cap is absent — so only the cap gate
        // excludes it. (Without PCG-2 this transition would have generated and
        // panicked at `expect::<dyn SutFocusWrite>()`.)
        assert!(
            rw::<t::NavigateFocus>().satisfied_by(&set.wiring),
            "NavigateFocus passes the WIRING gate under Turso"
        );
        assert!(!state.caps_available(&rc::<t::NavigateFocus>()));

        // ── B. The gate is actually applied inside `aggregate_transitions`. ──
        // A fresh state (app not started) makes the FS-fixture transitions feasible
        // (CreateDirectory/GitInit/…), all of which need `SutFixtureFs` — absent from
        // the partial cap set. So FULL (no cap gate) generates them and NARROW
        // does not.
        let mut runner = TestRunner::deterministic();
        // Sample the FULL alphabet as VALUES so we can read each variant's
        // `required_caps`.
        let full_strat = aggregate_transitions(&fresh_reference_state(set.wiring.clone()));
        let mut caps_by_name: std::collections::BTreeMap<&'static str, Vec<_>> =
            std::collections::BTreeMap::new();
        for _ in 0..400 {
            let tr = full_strat.new_tree(&mut runner).unwrap().current();
            caps_by_name
                .entry(tr.variant_name())
                .or_insert_with(|| tr.required_caps());
        }
        let full: BTreeSet<&str> = caps_by_name.keys().copied().collect();

        let narrow_strat = aggregate_transitions(&state);
        let narrow: BTreeSet<&str> = (0..400)
            .map(|_| {
                narrow_strat
                    .new_tree(&mut runner)
                    .unwrap()
                    .current()
                    .variant_name()
            })
            .collect();

        assert!(
            narrow.is_subset(&full) && narrow != full,
            "the cap gate strictly narrows the wide alphabet: full={full:?} narrow={narrow:?}"
        );
        // The narrowing is EXACTLY the cap gate: every variant FULL has but NARROW
        // drops is cap-infeasible under the partial set (it was removed for the
        // right reason, not by sampling noise — on a fresh state the only
        // cap-feasible variant is the always-on `Nothing`, which appears in
        // both).
        for removed in full.difference(&narrow) {
            assert!(
                !state.caps_available(&caps_by_name[removed]),
                "variant {removed} dropped from NARROW but its caps ARE present: {:?}",
                caps_by_name[removed]
            );
        }

        // Soundness: EVERY transition the narrowed alphabet generates is cap-feasible —
        // the gate is applied, never bypassed (this is what prevents an absent-cap
        // `expect` panic once PCG-4 lets the CapMap drive the alphabet).
        for _ in 0..400 {
            let tr = narrow_strat.new_tree(&mut runner).unwrap().current();
            assert!(
                state.caps_available(&tr.required_caps()),
                "NARROW generated a cap-infeasible transition {}: {:?}",
                tr.variant_name(),
                tr.required_caps()
            );
        }
    }

    /// Swap-track discrimination under the FULL `full_headless` composed
    /// config:
    /// - `ToggleState` is cap-feasible (the headless component provides
    ///   `SutMutate`).
    /// - `ApplyMutation` is NOW cap-feasible too: it is SOURCE-ROUTED via
    ///   `SutApplyMutation` and its gate names `SutLoro` (the implemented
    ///   LoroPeer arm), which `full_headless` provides. The composed generator
    ///   restricts its source set to the implemented arms
    ///   (`state.harness.cap_set.is_some()` → LoroPeer only for now).
    /// - `BulkExternalAdd` is NOW cap-feasible too: `HeadlessFrontendComponent`
    ///   provides a real composed `SutSeamMutate` (org write via the live
    ///   FileSyncController), which is its gate.
    /// (`E2ESut`'s own runs keep everything: concrete-SUT runs leave `cap_set =
    /// None`, so the gate admits all.)
    #[test]
    fn full_headless_capset_admits_toggle_apply_mutation_and_bulk() {
        use holon_pbt_core::TransitionFactory;

        use crate::pbt::reference_state::ReferenceState;
        use crate::pbt::state_machine::fresh_reference_state;
        use crate::pbt::transitions as t;

        fn rc<T: TransitionFactory<ReferenceState>>() -> Vec<holon_pbt_core::composition::CapId> {
            T::required_caps()
        }

        let set = ComponentSet::full_headless();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        let cap_set = rt.block_on(async {
            let sut = compose_sut(&set, &resolver()).await;
            sut.caps.cap_set()
        });
        drop(rt);
        let state = fresh_reference_state(set.wiring.clone()).with_cap_set(cap_set);

        assert!(
            state.caps_available(&rc::<t::ToggleState>()),
            "full_headless provides SutMutate → ToggleState is cap-feasible"
        );
        assert!(
            state.caps_available(&rc::<t::ApplyMutation>()),
            "full_headless provides SutLoro → source-routed ApplyMutation is cap-feasible"
        );
        assert!(
            state.caps_available(&rc::<t::BulkExternalAdd>()),
            "full_headless frontend now provides SutSeamMutate → BulkExternalAdd is cap-feasible"
        );
        assert!(
            state.caps_available(&rc::<t::SetEdgeField>()),
            "full_headless hosts SutEdgeFieldWrite → SetEdgeField is cap-feasible"
        );
    }

    /// SWAP PROBE (run with `--nocapture`): print the auto-narrowed
    /// `full_headless` alphabet so the swap's drive surface is explicit.
    /// Sampled from a FRESH state, so this is the
    /// wiring+precondition+cap-feasible-from-fresh upper bound (a booted/
    /// seeded swap ref unlocks more, e.g. Join once blocks exist). Reports FULL
    /// (no cap gate) vs NARROW (full_headless cap_set) and the
    /// cap-gated-out delta — the latter is the E4/seam set that stays on
    /// `E2ESut`. Peer ops (`AddPeer`/…) appear in NARROW
    /// because `full_headless` registers `SutLoro` — and as of Part A this is
    /// now load-bearing: the full-mode peer mesh drives over the frontend's
    /// shared authority doc + real sync controller, so a merged peer delta
    /// projects into the Turso `SutBackend` and the peer ops are part of
    /// the swap alphabet (no longer excluded).
    #[test]
    fn swap_probe_full_headless_narrowed_alphabet() {
        use std::collections::BTreeMap;
        use std::collections::BTreeSet;

        use holon_pbt_core::TransitionFactory;

        use crate::pbt::state_machine::fresh_reference_state;
        use crate::pbt::transitions as t;

        let set = ComponentSet::full_headless();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        let cap_set = rt.block_on(async {
            let sut = compose_sut(&set, &resolver()).await;
            sut.caps.cap_set()
        });
        drop(rt);

        let narrowed = fresh_reference_state(set.wiring.clone()).with_cap_set(cap_set);

        // Cap-feasibility is state-independent (it's `cap_set ⊇ required_caps`), so
        // enumerate per transition type directly — the precise set the swap CAN drive
        // cap-wise, independent of preconditions/seed.
        fn rc<T: TransitionFactory<crate::pbt::reference_state::ReferenceState>>()
        -> Vec<holon_pbt_core::composition::CapId> {
            T::required_caps()
        }
        macro_rules! feas {
            ($state:expr, $( $name:ident ),* $(,)?) => {{
                let mut feasible: BTreeSet<&'static str> = BTreeSet::new();
                let mut infeasible: BTreeMap<&'static str, Vec<_>> = BTreeMap::new();
                $(
                    let caps = rc::<t::$name>();
                    if $state.caps_available(&caps) {
                        feasible.insert(stringify!($name));
                    } else {
                        infeasible.insert(stringify!($name), caps);
                    }
                )*
                (feasible, infeasible)
            }};
        }
        let (feasible, infeasible) = feas!(
            narrowed,
            SplitBlock,
            JoinBlock,
            Indent,
            Outdent,
            MoveUp,
            MoveDown,
            TypeChars,
            DeleteBackward,
            MoveCursor,
            NavigateFocus,
            FocusEditableText,
            NavigateHome,
            NavigateBack,
            NavigateForward,
            PinBlock,
            UnpinBlock,
            SetupWatch,
            RemoveWatch,
            SwitchView,
            EmitMcpData,
            Redo,
            UndoLastMutation,
            ToggleState,
            CreateDocument,
            DeleteDocument,
            SimulateRestart,
            StartApp,
            ConcurrentSchemaInit,
            ApplyMutation,
            BulkExternalAdd,
            AddPeer,
            PeerEdit,
            PeerCharEdit,
            SyncWithPeer,
            MergeFromPeer,
            ClickBlock,
            DragDropBlock,
            ExpandToggle,
            PressKey,
            TriggerSlashCommand,
            ArrowNavigate,
            CreateDirectory,
            GitInit,
            WriteOrgFile,
            Nothing,
        );

        eprintln!("\n=== SWAP PROBE: full_headless cap-feasibility ===");
        eprintln!("CAP-FEASIBLE ({}): {:?}", feasible.len(), feasible);
        eprintln!(
            "CAP-INFEASIBLE ({}): {:?}",
            infeasible.len(),
            infeasible.keys().collect::<Vec<_>>()
        );
        eprintln!("=== end swap probe ===\n");
    }
}
