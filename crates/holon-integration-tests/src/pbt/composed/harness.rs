//! **F2 E3 — the generic composed-SUT `StateMachineTest` harness (increment
//! 3).**
//!
//! Every composed slice's `StateMachineTest` had a near-identical skeleton: a
//! `CapMap` + a shared `IdResolver` + a runtime; an `apply` that snapshots the
//! SUT id-set before/after the transition and reconciles the one minted id; a
//! `check_invariants` that runs the shared catalog and asserts no failures + a
//! non-vacuity floor. This module owns the skeleton; a slice provides only the
//! axes that genuinely differ via [`ComposedSlice`].
//!
//! ## The axes a slice varies
//! - **alphabet** — `Transition`/`Machine` + the per-tick `apply_transition`
//!   `match`.
//! - **seed** — `build` boots the component(s) and returns the cap map (+
//!   scaffold ids).
//! - **runtime/settle** — `MULTI_THREAD` + `SETTLE` (≈0 for a synchronous
//!   backend).
//! - **id alignment** — [`ComposedSlice::align_ids`]. Default = nothing (the
//!   harness's generic per-tick reconcile maps a uuid-minting backend's real
//!   ids onto the oracle's synthetic ones). A counter-sync backend
//!   (`MemoryBackend`) overrides it to push the oracle's `next_id` into its
//!   split-id hint so the next mint *is* the oracle's id — the reconcile then
//!   sees an identity pair.
//! - **check scope** — [`ComposedSlice::run_report`]. Default = full catalog
//!   over the scaffold-seeded, reconcile-resolved oracle. A focus-only slice
//!   (nav) overrides it to a `RefFocus`-only `run_selected` over the raw
//!   oracle.
//!
//! The reconcile + scaffold-injection + check are the composed-SUT *execution
//! kernel* — the same logic the eventual wide-PBT-over-`compose_sut` path
//! needs, so this is not slice-only scaffolding.
//!
//! @pbt kind infra
//! @pbt covers composed-harness — generic ComposedSut StateMachineTest:
//! per-tick reconcile + catalog check + non-vacuity floor
//!
//! ## No sanctioned baseline RED (the keystone is fully green)
//! HISTORY (resolved 2026-07-22, BugFunnel row 28): the shipped keystone
//! carried ONE accepted baseline RED for months — the journals
//! "ingest-data-loss" (`inv-blocks-match-ref/org` missing
//! `block:journals::action::0`, the `daily_journal` `holon_rule` action). It
//! was NOT a store bug (`/block_raw`, `/matview`, `/loro` were always green)
//! but a REAL org ROUND-TRIP data loss:
//! `convert_block_to_page(journals::auto-create)` moves the `holon_rule` child
//! into the new page's file as a TOP-LEVEL `#+BEGIN_SRC` (no enclosing `*
//! headline`), and the parser (`holon_org_format::parser::parse_org_file`) only
//! walked `doc.headlines()` — silently dropping every pre-first-headline source
//! block on read-back. Fixed at TWO layers: (1) the parser now extracts the
//! document's top-level section sources as direct doc children
//! (`emit_section_children` shared with the headline path); (2)
//! `extract_org_snapshot` filters the SUT org blocks by `seed_block_ids` — the
//! symmetric twin of turso-testing's `extract_block_raw` — since the fix newly
//! surfaces the seed display sources (`journals::src::0` / `render::0`) the ref
//! already excludes. So there is NO sanctioned red to whitelist; ANY red is a
//! real regression.
// KNOWN PRE-EXISTING RED (2026-07-22, verifier-discovered while de-sanctioning
// row-28): a fresh-seed draw can hit the unremapped synthetic `block:ref-doc-0`
// peer-doc — inv-viewmodel-entity-ids-subset-of-data 'phantom entity' +
// inv-blocks-match-ref/org field divergence; shrink involves
// AddPeer/CreateDoc/PeerEdit/UndoLastMutation. Parser-independent (reproduced
// with the row-28 fix reverted). Ledgered in BugFunnel 2026-07-22 (ref-doc-0
// remap row); fix = the reference-doc id remap in reference_state.rs:798 /
// action_actor_state.rs:52 must survive that op sequence. Until fixed, a red
// with EXACTLY this signature is that bug; any OTHER red is a regression.
//!
//! CAUTION (still true): proptest reports and shrinks to the FIRST failing
//! case, and the shrunk minimal input can be a locally-rejected transition when
//! the real fault is at the tick-0 seed comparison — read the panic's
//! `only_in_ref`/`only_in_actual` block diff, not the shrunk transition, to
//! localize. A newly-un-blinded render invariant and the per-sequence
//! engagement floor in `teardown` are only exercised on a REACHED full_headless
//! case; force one with `PROPTEST_CASES=1 HOLON_PBT_FORCE_FULL=1` when
//! validating a new invariant.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_api::EntityUri;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::InvariantId;
use holon_pbt_core::composition::RunReport;
use holon_pbt_core::invariant::InvariantResult;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
use crate::pbt::is_synthetic_ref_id;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::reference_state::Resolved;

/// Synthetic ref ids the COMPOSED per-tick reconcile maps to freshly-minted SUT
/// ids: split tails (`block::split-N`, via the global predicate) PLUS
/// CreateDocument-minted doc pages (`block:ref-doc-N`, from
/// `ReferenceState::next_synthetic_doc_uri`). Composed-local on purpose — the
/// global [`is_synthetic_ref_id`] stays split-only so E2ESut's mapping is
/// unaffected. Action-kind label for a transition, derived from its `Debug`
/// head token (`Indent { .. }` -> "Indent"). Generic over the slice
/// `Transition` enum (only `Debug` is available at the harness layer), used
/// solely for the `target="holon_latency"` per-action summary — never for
/// control flow.
fn action_label<T: std::fmt::Debug>(t: &T) -> String {
    let dbg = format!("{t:?}");
    dbg.split(|c: char| c == ' ' || c == '(' || c == '{' || c == '\n')
        .next()
        .unwrap_or("<transition>")
        .to_string()
}

fn is_composed_minted_synthetic_id(id: &EntityUri) -> bool {
    is_synthetic_ref_id(id)
        || id.as_str().starts_with("block:ref-doc-")
        // `CreateBlockUnderFocus` mints `block::create-N` in the oracle; the SUT's
        // creation-slot `block.create` op materializes a fresh uuid the per-tick
        // reconcile pairs 1:1 with it. Composed-local (not the global split-only
        // `is_synthetic_ref_id`) for the same reason `ref-doc-` is.
        || id.as_str().starts_with("block::create-")
        // `CreateBlockUnderFocus`'s born-equal arm mints `block:gen-N` directly on
        // both oracle and SUT (identity, no uuid). Treated like `ref-doc-` born-equal
        // doc pages so the born-equal self-map loop retains it in the reconcile map
        // (hence in `history_ever_created`), keeping the append-only SUT create row
        // covered after a later delete/join — otherwise it looks like a false phantom.
        || id.as_str().starts_with("block:gen-")
        // `BulkExternalAdd` writes each block into the org file WITH its oracle id
        // in the `:ID:` drawer, so the SUT re-ingests the SAME `block:bulk-N-M` id.
        // Born-equal exactly like `block:gen-`, and retained in the reconcile map
        // for the same reason: `history_ever_created` is derived from that map, so
        // without it a bulk block that a UI op touched (recording a real
        // `block_history` row) and that a later `DeleteDocument` removed reads as a
        // false phantom.
        || id.as_str().starts_with("block:bulk-")
}

/// Ids of peer-created blocks (`block:peer-HHHH-LLLL-IIII-SSSS`, minted by
/// `transitions::deterministic_peer_block_id`). These are STABLE and IDENTICAL
/// on both the oracle (`merge_peer_blocks_into_primary`) and the SUT
/// (peer-create uses the raw stable id) — no UUID is minted, so they need NO
/// synthetic→real mapping. They must be excluded from `real_new` in the
/// reconcile: a `MergeFromPeer` surfaces a fresh `block:peer-…` row in the SUT
/// `block_raw`, but there is no matching synthetic id, so the 1:1 `synthetic.
/// len() == real_new.len()` guard would otherwise panic. The identity-resolved
/// peer id already lines up with the oracle, so dropping it here is
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

/// E-solid clock side-channel: write the SUT's scalar Lamport height into the
/// oracle's shared `clock_feed` cell. Called after boot and after every
/// apply+settle, so the value the NEXT ref transition pads the shadow primary
/// to is exactly the height that transition's SUT write will land at
/// (proptest-state-machine interleaves ref-apply N → sut-apply N). The height
/// is the ONLY observable that ever flows SUT→oracle. Keeps the last-known
/// height when the cap is absent or the doc is not live yet.
pub(crate) async fn feed_sut_clock(caps: &CapMap, ref_state: &ReferenceState) {
    if let Some(log) = caps.get::<dyn SutLoroLog>()
        && let Some(h) = log.loro_lamport_height().await
    {
        *ref_state.loro.clock_feed.lock().expect("clock_feed lock") = Some(h);
    }
}

/// Inject `scaffold_ids` into the oracle as `block_documents[id]=no_parent` so
/// they join `seed_block_ids()` and filter out of the SUT-side block
/// comparison.
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
    /// counter-sync slice pushes `next_id` into. `()` when the cap map is
    /// enough (the caps already keep their component alive).
    type Handle;

    /// Invariants that MUST run each tick — the non-vacuity floor so "green"
    /// means "ran over real data", not "deselected everything".
    const REQUIRED_INVARIANTS: &'static [&'static str];
    /// CDC settle window after a write (≈0 for synchronous stores, ~150ms for a
    /// booted `FrontendSession`).
    const SETTLE: Duration;
    /// Whether the SUT needs a multi-thread runtime (a booted session does).
    const MULTI_THREAD: bool = false;

    /// The non-vacuity floor for THIS draw. Default: the full static
    /// [`REQUIRED_INVARIANTS`](Self::REQUIRED_INVARIANTS) (correct for a
    /// fixed-wiring slice). A wiring-parameterized slice (`WideE2E`)
    /// overrides this to intersect the static floor with the invariants its
    /// drawn `ref_state.wiring` can actually select — so a Loro-only draw
    /// is not required to run the SQL/ViewModel/focus invariants it has
    /// no caps for (which would false-RED the floor). Returns parsed
    /// [`InvariantId`]s, not raw strings, so the floor is compared against
    /// the report's typed ids directly.
    fn required_invariants(_: &ReferenceState) -> Vec<InvariantId> {
        Self::REQUIRED_INVARIANTS
            .iter()
            .copied()
            .map(InvariantId)
            .collect()
    }

    /// Boot + seed the SUT, returning the cap map, the slice handle, and the
    /// booted scaffold ids to seed-inject into the oracle. `resolver` is
    /// shared with the reconcile loop so a uuid-minting backend's writer
    /// resolves synthetic→real ids; `ref_state` is the initial oracle (a
    /// counter-sync slice seeds its `next_id`).
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

    /// Settle after a transition's write before reading the projections.
    /// Default: a flat `sleep(SETTLE)` (correct for a synchronous store,
    /// ≈0). A slice whose SUT projects asynchronously (`WideE2E`: Turso CDC
    /// + Loro + org) overrides this to a convergence wait over the slice's
    /// [`Handle`](Self::Handle) — bounded by `SETTLE`, so it returns
    /// fast when quiescent but never over-waits vs the flat sleep.
    async fn settle_after_apply(_: &Self::Handle, _: &CapMap) {
        tokio::time::sleep(Self::SETTLE).await;
    }

    /// Invalidate any per-tick render memo the SUT holds, called in `apply`
    /// immediately BEFORE `apply_transition` — i.e. before the state mutates.
    /// Default: nothing (a synchronous store has no memo). `WideE2E` clears the
    /// frontend component's `widget_tree_snapshot` cache so that ~12
    /// snapshot-reading invariants share ONE recompute per tick (the snapshot
    /// is the same settled tree for all of them) while a stale frame stays
    /// unrepresentable: the memo is empty across the whole mutate+settle window
    /// and only ever repopulated by the first check-time read of settled state.
    fn invalidate_render_caches(_: &Self::Handle) {}

    /// Align SUT-minted ids with the oracle. Called once after `build` (the
    /// initial seed) and after every apply. Default: nothing — the
    /// harness's generic per-tick reconcile already maps a uuid-minting
    /// backend's real ids onto the oracle's synthetic ones. A counter-sync
    /// backend overrides this to push `ref_state.domain.block_state.
    /// next_id` into its split-id hint.
    fn align_ids(_: &Self::Handle, _: &ReferenceState) {}

    /// Produce the catalog run report for a check. Default: resolve the
    /// oracle's doc-uris through the reconcile map, seed-inject the booted
    /// scaffold, and run the full catalog via `run_with_seeded_ref`. A
    /// focus-only slice (nav) overrides this to a `RefFocus`-only
    /// `run_selected` over the raw oracle (no minting, no scaffold).
    async fn run_report(
        caps: &CapMap,
        resolver: &IdResolver,
        burned: &BurnedPairs,
        scaffold_ids: &BTreeSet<EntityUri>,
        ref_state: &ReferenceState,
    ) -> RunReport {
        let map = resolver.lock().expect("resolver lock").clone();
        let mut resolved = ref_state.with_resolved_doc_uris(&map);
        inject_scaffold_seed(&mut resolved, scaffold_ids);
        // C2 provenance oracle: stash the id-reconcile map's expectation into the
        // resolved ref so `RefHistoryExpectation` can surface it. `ever_created`
        // = every real id the oracle minted (phantom-history anchor).
        // `min_op_groups` counts only synthetic→real reconciled (UI-driven)
        // creates — born-equal external/peer ids (key == value) are excluded, as
        // their org-ingest / Loro path records no engine history.
        {
            let inner = resolved.inner_mut();
            inner.history_ever_created = map
                .values()
                .chain(burned.iter().map(|(_, real)| real))
                .cloned()
                .collect();
            inner.history_min_op_groups = map.iter().filter(|(k, v)| k != v).count()
                + burned.iter().filter(|(syn, real)| syn != real).count();
        }
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

/// A per-check window-settle hook. The §Round-5 windowed harness
/// ([`ComposedSut::from_parts`]) injects a closure that pumps the gpui window
/// to a fixed point before invariants read its geometry; the headless proptest
/// path ([`StateMachineTest::init_test`]) leaves it a no-op. `Send` (not
/// `Sync`) — the windowed closure carries a `*const TestApp` behind a `Send`
/// newtype (same single-thread contract as `SimUserDriver`); it is only ever
/// called on the gpui thread that owns the window.
pub type SettleHook = Box<dyn Fn() + Send>;

/// Synthetic→real pairs the reconcile map once held and the SUT has since
/// destroyed (`UndoLastMutation` deletes the real block; a later `Redo`
/// re-creates it under a FRESH uuid — prod burns block ids). The live resolver
/// must re-point the synthetic at the new uuid, but the burned uuid still owns
/// its `block_history` rows, so it stays here as the append-only half of the
/// C2 provenance oracle (`history_ever_created` / `history_min_op_groups`).
pub type BurnedPairs = BTreeSet<(EntityUri, EntityUri)>;

/// The generic composed SUT: a `CapMap` driven through a slice's alphabet, with
/// the per-tick `IdResolver` reconcile and the shared-catalog check.
pub struct ComposedSut<S: ComposedSlice> {
    caps: CapMap,
    handle: S::Handle,
    resolver: IdResolver,
    burned: BurnedPairs,
    scaffold_ids: BTreeSet<EntityUri>,
    rt: tokio::runtime::Runtime,
    /// Pumps an attached gpui window to a fixed point before each
    /// `check_invariants` read; a no-op for the headless path. See
    /// [`SettleHook`].
    settle: SettleHook,
    /// Engagement ledger (F2 + vacuity-dashboard sweep): a per-invariant tally
    /// of how many ticks produced a NON-`Skipped` (gate-passed) verdict vs a
    /// `Skipped` one, over THIS sequence, for EVERY invariant that ran (not
    /// just the floor set). Populated in `check_invariants` (interior
    /// mutability — the trait method borrows `&Self`); the floor set is
    /// derived (engaged > 0) and asserted in `teardown`, which also emits
    /// an observe-only summary line. Distinguishes "selected + ran" (the
    /// per-tick floor) from "exercised with teeth": an invariant that
    /// early-returns `Skipped` on every tick is selected but never proves
    /// anything, and must fail the engagement floor (for floor-set ids)
    /// rather than pass vacuously. The full per-invariant tally is the
    /// vacuity dashboard primitive: it surfaces always-`Skipped` invariants
    /// (engaged=0) even outside the floor set.
    engaged: std::cell::RefCell<std::collections::BTreeMap<&'static str, EngageTally>>,
    /// Per-case telemetry accumulator (weights-spike step 0). Populated in
    /// `apply` (behind interior mutability, like `engaged`) only when
    /// `HOLON_PBT_TELEMETRY=1`; `teardown` emits one machine-parseable line
    /// from it + the engagement ledger + the drawn wiring. Empty and unread on
    /// the default (flag-off) path, so runs stay byte-identical.
    telemetry: std::cell::RefCell<super::telemetry::CaseTelemetry>,
    _slice: PhantomData<S>,
}

/// Per-invariant engagement tally for the observe-only vacuity dashboard: how
/// many ticks this sequence saw the invariant produce a gate-passed
/// (non-`Skipped`) verdict vs a `Skipped` one. `engaged == 0 && skipped > 0`
/// is the always-Skipped (potentially-vacuous) signal.
#[derive(Default, Clone, Copy)]
pub(crate) struct EngageTally {
    pub engaged: u32,
    pub skipped: u32,
}

impl<S: ComposedSlice> ComposedSut<S> {
    /// Construct a `ComposedSut` around ALREADY-BOOTED caps instead of
    /// tokio-booting via [`ComposedSlice::build`] (the §Round-5 windowed
    /// repoint: the gpui-thread harness boots a `compose_sut` session,
    /// attaches a window as a pure renderer over its `reactive`, and
    /// overlays the window's gesture caps — so the SUT is assembled outside
    /// this crate). `rt` drives the apply/check futures (leaf-polls; the booted
    /// backend runs on its own session runtime); `settle` pumps the attached
    /// window to a fixed point before each `check_invariants` read
    /// (headless callers use [`StateMachineTest::init_test`], whose
    /// `settle` is a no-op).
    pub fn from_parts(
        caps: CapMap,
        handle: S::Handle,
        resolver: IdResolver,
        scaffold_ids: BTreeSet<EntityUri>,
        rt: tokio::runtime::Runtime,
        settle: SettleHook,
    ) -> Self {
        // Same per-case observability isolation as `init_test` (the windowed
        // harness constructs the SUT through this entry point instead).
        crate::test_tracing::mark_driver_thread();
        crate::test_tracing::SpanCollector::global().reset();
        Self {
            caps,
            handle,
            resolver,
            burned: BurnedPairs::new(),
            scaffold_ids,
            rt,
            settle,
            engaged: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            telemetry: std::cell::RefCell::new(super::telemetry::new_case()),
            _slice: PhantomData,
        }
    }

    /// The SUT's shared runtime — the windowed harness drives the initial
    /// focus-align on it before the first check (mirrors
    /// `boot_and_seed_wide`'s headless focus drive).
    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.rt
    }

    /// The slice handle — a deterministic test needs it to reach the booted
    /// stores (e.g. `WideHandle::engine` for a production op dispatch).
    pub fn handle(&self) -> &S::Handle {
        &self.handle
    }

    /// Settle the slice's async projections now — the SAME post-apply settle
    /// [`StateMachineTest::apply`] runs. For deterministic tests that dispatch
    /// a write outside the transition alphabet (e.g. a production operation)
    /// and must not read invariants against a half-projected sink.
    pub fn settle_projections(&self) {
        let caps = &self.caps;
        let handle = &self.handle;
        self.rt.block_on(S::settle_after_apply(handle, caps));
    }

    /// Produce the catalog run report over the current SUT/oracle state — the
    /// SAME report [`StateMachineTest::check_invariants`] asserts over. Lets a
    /// deterministic test prove an invariant actually RAN (present in
    /// `report.ran`), not merely that nothing failed — a silently deselected
    /// invariant must fail such a test, not pass it vacuously.
    pub fn run_report_now(&self, ref_state: &ReferenceState) -> RunReport {
        (self.settle)();
        self.rt.block_on(S::run_report(
            &self.caps,
            &self.resolver,
            &self.burned,
            &self.scaffold_ids,
            ref_state,
        ))
    }

    /// The number of blocks the SUT store currently holds (the same
    /// `sut_ids` set the per-tick reconcile and the block invariants read).
    /// The soak reseed-reproduction rung uses it to prove the vault booted to
    /// scale before it starts measuring reseeds — a post-boot count far below
    /// the requested `HOLON_SOAK_SEED_BLOCKS` is itself a finding (boot ended
    /// unsettled / under-seeded), not a silent pass.
    pub fn sut_block_count(&self) -> usize {
        self.rt.block_on(sut_ids(&self.caps)).len()
    }

    /// The live cap set this SUT's `CapMap` actually provides. The windowed
    /// harness feeds this into the oracle (`wide_e2e_windowed_ref`) so
    /// `aggregate_transitions` narrows the generated alphabet to exactly
    /// what THIS SUT can drive — the overlaid window caps
    /// (`SutLayout`/`SutDriver`/…) included, with no hand-maintained list.
    pub fn cap_set(&self) -> holon_pbt_core::composition::CapSet {
        self.caps.cap_set()
    }
}

/// Invariants for which "selected every tick but `Skipped` every tick" is a
/// permanent-vacuity bug rather than legitimate not-applicable-this-sequence.
/// Engages at ANY settled render (the seeded 3-column layout is present in
/// every full_headless sequence, including at tick 0), so a per-sequence
/// engagement requirement is sound — and is exactly the F1 regression guard: it
/// was silently vacuous behind a `has_root_render_expr()` gate the wide
/// 3-column layout keeps permanently false, so it early-returned `Ok` and
/// satisfied the old "appears in `ran`" floor while checking nothing.
/// Empirically PROVEN to engage+pass over reached full-render cases (mask
/// bypassed).
///
/// NOTE — `inv-viewmodel-root-matches-render-expr` is DELIBERATELY NOT here.
/// Its `has_root_render_expr()` gate was un-blinded the same way (vacuous `Ok`
/// → honest `Skipped`), but its 3-column path drills the main panel and never
/// reaches a rendered content-kind verdict in the wide seed — it `Skipped` on
/// EVERY tick of every full-render case (this engagement floor CAUGHT that).
/// Making it engage needs a layout/generator change (a static main-panel render
/// expr whose rendered widget kind is assertable), i.e. a FORK — flagged,
/// deferred. Its restructure (honest `Skipped`, never a fake `Ok`) still lands
/// as a strict improvement.
const ENGAGEMENT_FLOOR: &[&str] = &["inv-viewmodel-entity-ids-subset-of-data"];

/// The engagement floor's pure decision (extracted so it is unit-testable
/// without booting a full slice — the `teardown` hook that calls it never runs
/// under the shipped keystone gate, which short-circuits on the journals RED in
/// `check_invariants` before any sequence reaches teardown). Returns the
/// [`ENGAGEMENT_FLOOR`] ids that this draw SELECTED (`required`) but that never
/// produced a non-`Skipped` verdict (`engaged`) — i.e. selected-but-vacuous.
fn engagement_floor_violations(
    engaged: &std::collections::BTreeSet<&str>,
    required: &std::collections::BTreeSet<&str>,
) -> Vec<&'static str> {
    ENGAGEMENT_FLOOR
        .iter()
        .copied()
        .filter(|id| required.contains(id))
        .filter(|id| !engaged.contains(id))
        .collect()
}

impl<S: ComposedSlice> StateMachineTest for ComposedSut<S> {
    type SystemUnderTest = Self;
    type Reference = S::Machine;

    fn init_test(ref_state: &ReferenceState) -> Self {
        // Per-case observability isolation: this thread's panics are loud (they
        // unwind into proptest), so exclude them from swallowed-panic capture,
        // and clear any problems left over from a previous case/shrink
        // iteration so `inv-no-observed-errors` only ever sees THIS case's
        // window. Both halves guard against the shrink-time feedback loop
        // where the harness's own divergence panic re-entered the next
        // iteration's failure message (exponential escape blowup, 2026-07-11).
        crate::test_tracing::mark_driver_thread();
        crate::test_tracing::SpanCollector::global().reset();
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
        rt.block_on(feed_sut_clock(&caps, ref_state));
        Self {
            caps,
            handle,
            resolver,
            burned: BurnedPairs::new(),
            scaffold_ids,
            rt,
            settle: Box::new(|| {}),
            engaged: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            telemetry: std::cell::RefCell::new(super::telemetry::new_case()),
            _slice: PhantomData,
        }
    }

    fn apply(mut sut: Self, ref_state: &ReferenceState, transition: S::Transition) -> Self {
        let action = action_label(&transition);
        // Weights-spike telemetry: pre-clone the kind label (the async block
        // moves `action`), record after the timed apply. No-op when the flag is
        // off — `record_label` stays `None`.
        let record_label = super::telemetry::telemetry_enabled().then(|| action.clone());
        let (before, after, action_us) = {
            // Split the borrow: `settle_after_apply` reads `&sut.handle` while the apply
            // writes `&mut sut.caps` — disjoint fields, so borrow each separately before
            // the `block_on` (both are captured by the `async move`).
            let caps = &mut sut.caps;
            let handle = &sut.handle;
            sut.rt.block_on(async move {
                let before = sut_ids(caps).await;
                // Drop the SUT's per-tick render memo BEFORE mutating, so the
                // snapshot the invariants read this tick is recomputed against
                // the post-transition settled state (never a stale pre-mutation
                // frame). No-op for slices without a render memo.
                S::invalidate_render_caches(handle);
                let t_action = std::time::Instant::now();
                S::apply_transition(&transition, ref_state, caps).await;
                S::settle_after_apply(handle, caps).await;
                let action_us = t_action.elapsed().as_micros() as u64;
                // Latency (end-to-end, action->visible rows): dispatch through the
                // real pipeline plus the CDC settle — everything except final GPU
                // paint (headless harness). Greppable via target="holon_latency".
                tracing::debug!(
                    target: "holon_latency",
                    stage = "action_total",
                    action = %action,
                    total_ms = t_action.elapsed().as_millis() as u64,
                    "holon_latency",
                );
                feed_sut_clock(caps, ref_state).await;
                let after = sut_ids(caps).await;
                (before, after, action_us)
            })
        };
        // Weights-spike telemetry: record this transition's kind + wall micros
        // (flag-off => `record_label` is `None`, so no accumulation happens).
        if let Some(label) = record_label {
            sut.telemetry.borrow_mut().record(label, action_us);
        }
        // Per-tick reconciliation: a single transition mints at most one block, so
        // the one unmapped synthetic id (oracle, post-apply) pairs 1:1 with the one
        // new real id (SUT). Accumulate into the shared resolver. (For a counter-sync
        // backend the real id *is* the synthetic id, so this maps it to itself — the
        // `align_ids` hook below kept the next mint in lockstep.)
        //
        // The synthetic schemes the harness reconciles: `block::split-N` (split tails)
        // AND `block:ref-doc-N` (CreateDocument-minted doc pages — the doc-uri-minting
        // generalization of the seam's `block_tree_post_action` CreateDocument arm).
        // Both are placeholders the oracle allocates that the SUT backend
        // materializes as fresh ids. This is a COMPOSED-LOCAL predicate,
        // deliberately NOT the global `is_synthetic_ref_id` (which E2ESut keys
        // its split-only mapping off — widening it there would make E2ESut
        // mis-treat doc-uris as splits).
        let mut map = sut.resolver.lock().expect("resolver lock");
        // Resolver staleness across undo→redo. `UndoLastMutation` DELETES the real
        // block a synthetic maps to; `Redo` then re-creates it under a FRESH uuid
        // (prod burns block ids across undo — see `ReferenceState::pop_undo_to_redo`)
        // while the oracle's redo snapshot restores the SAME synthetic label. Retire
        // exactly the pairs that are BOTH live on the oracle side (the synthetic is
        // a block the oracle currently holds) and dead on the SUT side (its uuid is
        // gone) — those must re-pair below. A pair whose synthetic the oracle no
        // longer holds is NOT retired: a deleted block's mapping is still needed to
        // resolve oracle references that outlive it (e.g. `navigation_history`).
        // The retired pair moves to `burned`, which still feeds the C2 provenance
        // oracle — the dead uuid owns real `block_history` rows.
        // Observed counterexample: [SplitBlock(c1,0), UndoLastMutation, Redo] —
        // `tests/split_undo_redo_reconcile.rs`.
        let retired: Vec<(EntityUri, EntityUri)> = map
            .iter()
            .filter(|(syn, real)| {
                !after.contains(*real) && ref_state.domain.block_state.blocks.contains_key(*syn)
            })
            .map(|(syn, real)| (syn.clone(), real.clone()))
            .collect();
        for (syn, _) in &retired {
            map.remove(syn);
        }
        sut.burned.extend(retired);
        // Born-equal doc pages: `WriteOrgFile` pins the oracle's `block:ref-doc-N`
        // into the file via `#+ID:`, so the SUT ingests the SAME id — synthetic in
        // scheme, but with no fresh real partner. Self-map those (identity), like
        // the counter-sync case above; only synthetics the SUT does NOT already
        // hold (CreateDocument's empty file → fresh uuid) enter the 1:1 pairing.
        let unmapped: Vec<EntityUri> = ref_state
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| is_composed_minted_synthetic_id(id) && !map.contains_key(id))
            .cloned()
            .collect();
        let (born_equal, synthetic): (Vec<EntityUri>, Vec<EntityUri>) =
            unmapped.into_iter().partition(|id| after.contains(id));
        for id in born_equal {
            map.insert(id.clone(), id);
        }
        // Peer-merged blocks (`block:peer-…`) surface in the SUT `block_raw` with a
        // stable id already shared with the oracle — they need no synthetic→real
        // mapping, so exclude them from `real_new` to keep the 1:1 split/doc
        // guard intact (a `MergeFromPeer` would otherwise make `real_new`
        // outrun `synthetic` and panic). Born-equal ids (External
        // `ApplyMutation::Create` and `BulkExternalAdd` write the block WITH
        // its oracle id in the `:ID:` drawer, so `resolve_mutation_ids` leaves a
        // Create's NEW id as-is) surface in the SUT with the SAME id the oracle already
        // holds. Like peer-merged blocks they are shared, need no synthetic→real
        // mapping, and would otherwise make `real_new` outrun `synthetic`
        // (which only counts synthetic-scheme oracle ids) and panic.
        // `resolve_id` passes unmapped ids through as identity, so dropping
        // them here is correct.
        let real_new: Vec<EntityUri> = after
            .difference(&before)
            .filter(|id| !is_peer_scheme_id(id))
            .filter(|id| !ref_state.domain.block_state.blocks.contains_key(*id))
            .cloned()
            .collect();
        assert_eq!(
            synthetic.len(),
            real_new.len(),
            "per-tick reconcile: one synthetic per minted real id (syn={synthetic:?}, \
             real={real_new:?})"
        );
        // Pairing safety for the R2 StaleExternalRewrite CHURN (multiple mints in
        // one tick, unlike the usual one-mint transitions this zip was written for):
        // the churn re-mints only NON-UNIQUE / empty content (unique content remaps
        // via `tiered_match`, keeping its id — never appears here), so every element
        // of `synthetic`/`real_new` this tick shares the same (parent, content). The
        // zip is therefore order-insensitive: (1) `inv-blocks-match-ref` normalizes on
        // {id, parent, content, tags, …} with NO sort_key / position field, so
        // identical-(parent, content) blocks are interchangeable, and (2) no
        // cross-content mispair is possible because all churn mints carry the same
        // content. The oracle predicts the identical churn in `StaleExternalRewrite::
        // apply_to_ref` (shared `tiered_match`), so the two lengths match here.
        for (syn, real) in synthetic.into_iter().zip(real_new) {
            map.insert(syn, real);
        }
        drop(map);
        S::align_ids(&sut.handle, ref_state);
        sut
    }

    fn check_invariants(sut: &Self, ref_state: &ReferenceState) {
        // Pump an attached gpui window to a fixed point so the geometry/text/focus caps
        // read the post-transition frame (no-op headlessly — see `SettleHook`). Mirrors
        // the sim windowed per-tick hook's `settle_to_fixed_point` before its check.
        (sut.settle)();
        let report = sut.rt.block_on(S::run_report(
            &sut.caps,
            &sut.resolver,
            &sut.burned,
            &sut.scaffold_ids,
            ref_state,
        ));
        // `HOLON_PBT_INVARIANTS` disclosed softening: a matched `warn`/`skip` failure
        // is logged loudly and made non-fatal (a DISCLOSED degraded run, not a
        // clean pass); unmatched failures stay fatal. The composed home of the
        // knob relocated from the deleted native runner core. The panic prefix
        // is unchanged so `bisect_driver`'s `reproduction_signature()` still
        // recognizes a composed divergence.
        use crate::pbt::invariant_mode_override::ModeOverride;
        use crate::pbt::invariant_mode_override::invariant_mode_override;
        // Softened ids are RETAINED (not dropped) — their layer ran and went
        // red, so the coverage verdict must never call it green.
        let mut softened_out: Vec<&'static str> = Vec::new();
        let hard: Vec<(&str, &str)> = report
            .failures()
            .into_iter()
            .filter(|(id, msg)| match invariant_mode_override(id) {
                Some(ModeOverride::Warn | ModeOverride::Skip) => {
                    eprintln!(
                        "[HOLON_PBT_INVARIANTS] softened (DISCLOSED degraded run) {id}: {msg}"
                    );
                    softened_out.push(*id);
                    false
                }
                _ => true,
            })
            .collect();
        // Env-gated failure-tick DB dump (`HOLON_PBT_DUMP_DB=1`): on a hard
        // divergence, dump the `block` matview vs `block_raw` base per-id
        // row-counts so the Turso IVM over-count signature (a matview id with
        // more rows than the PK-unique base) is captured BEFORE teardown drops
        // the DB. Off by default — the run stays byte-identical.
        if !hard.is_empty() && std::env::var_os("HOLON_PBT_DUMP_DB").is_some() {
            dump_matview_vs_base(sut);
        }
        // FIRST-DIVERGENT-LAYER verdict: several invariants across the stack
        // usually red at once (a store divergence re-projects into the matview,
        // re-derives into the viewmodel, re-renders…), so name the EARLIEST
        // (most upstream) layer + its wiring source to start triage at the root.
        // Kept AFTER the unchanged `reconciled composed sequence diverged from
        // the oracle` prefix so `bisect_driver::reproduction_signature()` still
        // matches (substring). Fail-loud: an unmapped id is disclosed, not guessed.
        // Built ONLY on the failure path — a green tick pays nothing (the walk
        // is inside `inv-sql-budget`'s wall window).
        if !hard.is_empty() {
            let coverage = crate::pbt::composed::first_divergent::RunCoverage::from_report(
                &report,
                softened_out,
            );
            let verdict =
                crate::pbt::composed::first_divergent::first_divergent_verdict(&hard, &coverage);
            panic!("reconciled composed sequence diverged from the oracle: {hard:?}\n{verdict}");
        }
        // Per-tick SELECTION floor: every required invariant must be selected
        // (present in `report.ran`) at every tick — a silent deselect fails
        // loud HERE. Selection alone, however, is not proof of exercise: a body
        // may appear in `ran` with a `Skipped` verdict on every tick and still
        // observe nothing (F2). So, in addition, record engagement — an id that
        // produced a NON-`Skipped` verdict this tick — into the per-sequence
        // ledger; `teardown` asserts each required id engaged at least once.
        for id in S::required_invariants(ref_state) {
            assert!(
                report.ran.iter().any(|(ran_id, _)| *ran_id == id),
                "non-vacuity (selection): {} must be selected + run over real data (ran: {:?})",
                id.0,
                report.ran_ids()
            );
        }
        {
            let mut ledger = sut.engaged.borrow_mut();
            for (id, result) in &report.ran {
                let tally = ledger.entry(id.0).or_default();
                if matches!(result, InvariantResult::Skipped(_)) {
                    tally.skipped += 1;
                } else {
                    tally.engaged += 1;
                }
            }
        }
        // Windowed non-vacuity floor: when this SUT hosts a window (a `SutLayout` cap
        // is present — true only on the §Round-5 windowed path), the windowed
        // geometry family MUST have selected + run. Keyed off the ACTUAL caps
        // (not the ref cap_set, which stays headless), so a silent deselect
        // that turns the windowed families into "ran nothing" fails HERE
        // instead of passing vacuously — the same guard
        // `run_windowed_composed_check` enforces, now inside the unified
        // StateMachineTest.
        if sut.caps.get::<dyn SutLayout>().is_some() {
            // Disclose the windowed ran set (C-5: lets the windowed harness log prove the
            // `SutFrontendEngine` / `SutFrontendEmissions` invariants SELECT + RUN with
            // real teeth — grep `[windowed ran]` for their ids).
            eprintln!("[windowed ran] {:?}", report.ran_ids());
            assert!(
                report.ran_ids().contains(&"inv-frontend-bounds-rendered"),
                "windowed non-vacuity: inv-frontend-bounds-rendered must run when a window is \
                 attached (ran: {:?}, deselected: {:?})",
                report.ran_ids(),
                report.deselected.iter().map(|d| d.0).collect::<Vec<_>>(),
            );
        }
    }

    /// Per-sequence ENGAGEMENT floor (F2): a required invariant that was
    /// selected every tick but `Skipped` on every tick observed nothing — it is
    /// selected, not exercised. Assert each id in [`ENGAGEMENT_FLOOR`] that
    /// this draw actually selected produced at least one NON-`Skipped`
    /// (gate-passed) verdict somewhere across the sequence. This is the
    /// "selected ≠ exercised with teeth" gate: without it a body that
    /// early-returns before its assertion (the old vacuous `Ok`, now
    /// `Skipped`) satisfies the floor while proving nothing.
    ///
    /// SCOPE: the floor covers only invariants that engage at ANY settled
    /// render (so every full_headless sequence, even a zero-transition one,
    /// exercises them). A blanket "every required invariant must engage per
    /// sequence" is UNSOUND — invariants gated on state a short sequence
    /// never reaches (editor mirror with no edit, toggle with no toggle, …)
    /// legitimately Skip across a whole sequence and would false-red.
    /// Sweeping the floor to those needs a process-global (cross-sequence)
    /// accumulator, deferred (see the audit's "validate on the two F1
    /// invariants first, then sweep").
    fn teardown(sut: Self, ref_state: ReferenceState) {
        let ledger = sut.engaged.borrow();
        // Weights-spike telemetry: one machine-parseable per-case line
        // (`[pbt-telemetry] {json}`) from the accumulated transitions + this
        // ledger + the drawn wiring. Flag-gated, so a default run emits nothing.
        if super::telemetry::telemetry_enabled() {
            let tele = sut.telemetry.borrow();
            let line = super::telemetry::build_line(
                tele.case_index,
                &ref_state.harness.wiring,
                &tele.kinds,
                &tele.micros,
                &ledger,
            );
            super::telemetry::emit(&line);
        }
        // Observe-only vacuity dashboard (telemetry sweep): one line per
        // sequence, `<id>=<engaged>/<total>` for EVERY invariant that ran this
        // sequence. `engaged=0` (i.e. `0/N`) flags an always-`Skipped`
        // invariant — a vacuity candidate — even for ids outside the scoped
        // engagement floor. Greppable via `[engagement summary]`.
        eprintln!(
            "[engagement summary] {}",
            ledger
                .iter()
                .map(|(id, t)| format!("{id}={}/{}", t.engaged, t.engaged + t.skipped))
                .collect::<Vec<_>>()
                .join(" ")
        );
        // The engaged SET (ids with ≥1 non-`Skipped` verdict) drives the floor.
        let engaged: std::collections::BTreeSet<&'static str> = ledger
            .iter()
            .filter(|(_, t)| t.engaged > 0)
            .map(|(id, _)| *id)
            .collect();
        // Only draws that actually SELECT the invariant owe engagement — a
        // Loro-only draw with no renderer never selects the viewmodel ones.
        let required: std::collections::BTreeSet<&'static str> = S::required_invariants(&ref_state)
            .into_iter()
            .map(|id| id.0)
            .collect();
        let unengaged = engagement_floor_violations(&engaged, &required);
        assert!(
            unengaged.is_empty(),
            "non-vacuity (engagement): invariant(s) were selected but never produced a \
             non-Skipped (gate-passed) verdict across the whole sequence — selected, never \
             exercised with teeth: {unengaged:?} (engaged: {:?})",
            engaged.iter().collect::<Vec<_>>(),
        );
    }
}

/// Env-gated (`HOLON_PBT_DUMP_DB`) failure-tick diagnostic. On a hard invariant
/// divergence, dump the live `block` matview snapshot vs the `block_raw` base
/// snapshot and report every id whose matview row-count differs from its base
/// row-count — the signature of the Turso IVM over-count (the matview carries a
/// duplicate/extra row the PK-unique `block_raw` cannot). Turso keeps its IVM
/// operator state in internal btrees (not SQL-exposed), so matview-vs-base row
/// divergence is the observable evidence. In-tree debugging affordance; the
/// call site is env-gated so default runs are byte-identical.
fn dump_matview_vs_base<S: ComposedSlice>(sut: &ComposedSut<S>) {
    let (matview, base) = sut.rt.block_on(async {
        (
            sut.caps.live_block_snapshot().await,
            sut.caps.block_raw_snapshot().await,
        )
    });
    let mut mv_counts: BTreeMap<EntityUri, usize> = BTreeMap::new();
    for b in &matview {
        *mv_counts.entry(b.id.clone()).or_default() += 1;
    }
    let mut base_counts: BTreeMap<EntityUri, usize> = BTreeMap::new();
    for b in &base {
        *base_counts.entry(b.id.clone()).or_default() += 1;
    }
    eprintln!(
        "[HOLON_PBT_DUMP_DB] block matview rows={} block_raw rows={}",
        matview.len(),
        base.len()
    );
    let mut ids: BTreeSet<&EntityUri> = BTreeSet::new();
    ids.extend(mv_counts.keys());
    ids.extend(base_counts.keys());
    let mut any = false;
    for id in ids {
        let mv = mv_counts.get(id).copied().unwrap_or(0);
        let bc = base_counts.get(id).copied().unwrap_or(0);
        if mv != bc {
            any = true;
            let base_parent = base
                .iter()
                .find(|b| &b.id == id)
                .map(|b| b.parent_id.to_string())
                .unwrap_or_else(|| "<absent-in-base>".into());
            eprintln!(
                "[HOLON_PBT_DUMP_DB] DIVERGENT id={id} matview_rows={mv} base_rows={bc} \
                 base_parent_id={base_parent}"
            );
            for (i, b) in matview.iter().filter(|b| &b.id == id).enumerate() {
                eprintln!(
                    "[HOLON_PBT_DUMP_DB]   matview[{i}] parent_id={} content={:?}",
                    b.parent_id, b.content
                );
            }
        }
    }
    if !any {
        eprintln!(
            "[HOLON_PBT_DUMP_DB] no per-id matview/base row-count divergence in this \
             snapshot pair (over-count may be field-level or org-store only)"
        );
    }
    // Field-level (parent_id) divergence: a matview row whose parent_id differs
    // from the PK-unique base row for the same id localizes a PROJECTION bug
    // (matview mis-maintained a re-parent delta); agreement instead points the
    // wrong-parent UPSTREAM (write authority / org-writeback), NOT the matview.
    // Also list every id parented under `block:journals` — the phantom target.
    let base_parent: BTreeMap<&EntityUri, &EntityUri> =
        base.iter().map(|b| (&b.id, &b.parent_id)).collect();
    for b in &matview {
        if let Some(bp) = base_parent.get(&b.id) {
            if *bp != &b.parent_id {
                eprintln!(
                    "[HOLON_PBT_DUMP_DB] PARENT-DIVERGE id={} matview_parent={} base_parent={} \
                     (projection mis-maintained the re-parent)",
                    b.id, b.parent_id, bp
                );
            }
        }
    }
    for b in &base {
        if b.parent_id.as_str() == "block:journals" {
            eprintln!(
                "[HOLON_PBT_DUMP_DB] JOURNALS-PARENTED (base) id={} content={:?} — write \
                 authority itself has this under journals",
                b.id, b.content
            );
        }
    }
}

/// Gherkin `Then`-assertion bridge: a `ComposedSut` evaluates the assert
/// vocabulary (`widget contains`, `focus is on`) against its composed cap
/// surface via
/// [`evaluate_assertion_caps`](crate::pbt::fixtures::assert::evaluate_assertion_caps),
/// reusing the same `rt`/`caps`/`resolver` the PBT path drives. This is what
/// lets the deterministic `.feature` replays run over the composed SUT instead
/// of `E2ESut` — so gherkin becomes one more way to steer the ONE PBT, with the
/// full composed catalog still checked every tick (`replay_steps` calls
/// `check_invariants`).
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

#[cfg(test)]
mod engagement_floor_tests {
    use std::collections::BTreeSet;

    use super::ENGAGEMENT_FLOOR;
    use super::engagement_floor_violations;

    /// The exact "selected ≠ exercised" case the shipped keystone gate MASKS:
    /// an `ENGAGEMENT_FLOOR` invariant that this draw SELECTED (present in
    /// `required`) but that only ever returned `Skipped` (absent from
    /// `engaged`) MUST violate the floor. Without this, the un-blinded F1
    /// invariants could early-return `Skipped` on every tick and still pass —
    /// the vacuity the floor exists to catch. (The `teardown` hook that calls
    /// this never runs under the standard keystone, which reds on the journals
    /// baseline in `check_invariants` first, so this pure check is the guard.)
    #[test]
    fn floor_fails_when_selected_but_never_engaged() {
        let id = ENGAGEMENT_FLOOR[0];
        let required: BTreeSet<&str> = [id].into_iter().collect();
        let engaged: BTreeSet<&str> = BTreeSet::new();
        assert_eq!(engagement_floor_violations(&engaged, &required), vec![id]);
    }

    /// A single non-`Skipped` verdict somewhere in the sequence satisfies it.
    #[test]
    fn floor_passes_when_engaged() {
        let id = ENGAGEMENT_FLOOR[0];
        let required: BTreeSet<&str> = [id].into_iter().collect();
        let engaged: BTreeSet<&str> = [id].into_iter().collect();
        assert!(engagement_floor_violations(&engaged, &required).is_empty());
    }

    /// A draw that never SELECTED the invariant (a Loro-only draw with no
    /// renderer) owes no engagement — the floor must not false-red there.
    #[test]
    fn floor_ignores_unselected_invariants() {
        let required: BTreeSet<&str> = BTreeSet::new();
        let engaged: BTreeSet<&str> = BTreeSet::new();
        assert!(engagement_floor_violations(&engaged, &required).is_empty());
    }
}
