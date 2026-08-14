//! **THE SWAP (§5) — the composed `general_e2e` slice, `pbt`-gated so it can
//! drive a `tests/` integration test.**
//!
//! Relocated out of `frontend_slice/structural_pbt.rs` (a `#[cfg(test)]`-only
//! module) into this `#[cfg(any(test, feature = "pbt"))]` module so the
//! production integration test `general_e2e_composed_pbt` (in `tests/`) — which
//! links the lib built WITHOUT `cfg(test)` — can declare
//! [`ComposedSut<WideE2E>`]. The lib slices/teeth in `structural_pbt.rs` now
//! `use` these items instead of defining their own copies, so there is a SINGLE
//! source of truth (North Star: one composed convergence PBT).
//!
//! [`WideE2E`] drives the PRODUCTION `E2ETransition` enum via the PRODUCTION
//! `aggregate_transitions` generator (NOT a curated list) over
//! `compose_sut(full_headless)` — the exact SUT + alphabet the
//! `general_e2e_pbt` swap targets. The alphabet auto-narrows to the composed
//! SUT's drivable caps (peer/seam/E4/fixture cap-gate out; watches + mutate are
//! DELIBERATELY narrowed pending B5 / the Loro-doc-unification fix —
//! see [`wide_e2e_ref`]).
//!
//! @pbt kind slice
//! @pbt covers wide-e2e-swap — the composed general_e2e slice: production
//! E2ETransition over compose_sut, wiring-drawn

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon::api::BackendEngine;
use holon_api::Block;
use holon_api::EntityUri;
use holon_api::Region;
use holon_api::repository::NewBlock;
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::Actor;
use holon_pbt_core::ComponentSet;
use holon_pbt_core::Projection;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::Wiring;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapSet;
use holon_pbt_core::composition::InvariantId;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Strategy;
use proptest_state_machine::ReferenceStateMachine;

use crate::pbt::composed::boundary::Boundary;
use crate::pbt::composed::boundary::BoundaryEvidence;
use crate::pbt::composed::boundary::BoundaryOutcome;
use crate::pbt::composed::boundary::BoundaryWindow;
use crate::pbt::composed::boundary::settled_and_pending;
use crate::pbt::composed::builder::compose_sut;
use crate::pbt::composed::builder::compose_sut_seeded;
use crate::pbt::composed::builder::compose_sut_seeded_with_peer_id;
use crate::pbt::composed::builder::compose_sut_windowed_base_seeded;
use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::composed::harness::BurnedPairs;
use crate::pbt::composed::harness::ComposedSlice;
use crate::pbt::composed::harness::sut_ids;
use crate::pbt::composed::seed_primitives::C1;
use crate::pbt::composed::seed_primitives::C2;
use crate::pbt::composed::seed_primitives::PARENT;
use crate::pbt::composed::seed_primitives::fixed_ids;
use crate::pbt::composed::subsystem_seed::build_started_ref;
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transitions::E2ETransition;
use crate::pbt::transitions::NavigateFocus;

/// The seed **page** the working blocks sit directly under. It is the focus
/// root (so its children are the editable candidates) but is itself excluded
/// from candidates (`is_page`) and from the comparison (seed), so it is never
/// split and its page-ness is never compared.
pub fn page_root() -> EntityUri {
    EntityUri::block("structural-page")
}

/// Settle window for the headless CDC pump after a write. This is the CAP on
/// the [`converge_projections`] convergence wait, not a flat sleep: a settled
/// SUT returns in ~one quiet-floor poll, and a busy one is bounded by this so
/// it never over-waits vs the old flat `sleep(SETTLE)`.
pub const SETTLE: Duration = Duration::from_millis(150);

/// Convergence CAP for [`converge_projections`]. Unlike [`SETTLE`] (a
/// flat-sleep replacement sized for a quiet SUT), this bounds the
/// LOOP-to-fixed-point wait: a settled SUT returns in ~one quiet-floor poll,
/// but a heavy transition whose Loro->SQL projection is the latency dominator
/// (~1.84s at vault scale) needs far more than 150ms to actually land its
/// `sort_key` writes. `SETTLE` was too small, so the settle gave up
/// mid-projection and the invariant read half- projected SQL (the org-render
/// sibling-order flake). This is a real hang backstop, not a per-transition
/// budget — only genuine non-convergence hits it.
pub const CONVERGE_BUDGET: Duration = Duration::from_secs(30);

/// The slice handle for [`WideE2E`] — the store handles the post-write settle
/// needs to prove all three projections (Turso CDC + Loro + org) have drained,
/// instead of a flat `sleep(SETTLE)`. Absent handles (a Loro-only draw has no
/// Turso engine / frontend org sync) make the corresponding projection a no-op
/// — those stores are synchronous, so there is nothing to wait for.
#[derive(Clone, Default)]
pub struct WideHandle {
    /// The canonical Turso `BackendEngine` — its
    /// `db_handle().cdc_emitted_watermark()` is the CDC drain signal
    /// (`None` for a Loro-only draw).
    engine: Option<Arc<BackendEngine>>,
    /// The booted frontend component — the lazy accessor for the Loro sync
    /// handle / doc-store (Loro quiescence) and the `OrgSyncIdleSignal`
    /// (org re-render drain). `None` for a non-frontend (Loro-only) draw.
    /// Queried at settle time, not at boot, because the sync controller
    /// resolves on a spawned `post_ready_work` task.
    frontend: Option<Arc<HeadlessFrontendComponent>>,
}

impl WideHandle {
    /// The booted Turso engine (`None` for a Loro-only draw). Deterministic
    /// tests use it to dispatch production operations headlessly via
    /// `BackendEngine::execute_operation` — the same dispatch the frontend's
    /// op_buttons reach.
    pub fn engine(&self) -> Option<&Arc<BackendEngine>> {
        self.engine.as_ref()
    }

    /// The booted `ReactiveEngine` — the `BuilderServices` host that carries
    /// the advice weave sidecar. `None` for a Loro-only (no-frontend) draw.
    /// The live-MCP advice gate backs an embedded MCP server with this exact
    /// instance so `describe_ui` over the wire reads the same woven rows the
    /// composed SUT settles in-process.
    pub fn reactive(&self) -> Option<Arc<holon_frontend::reactive::ReactiveEngine>> {
        self.frontend.as_ref().map(|f| f.reactive())
    }

    /// The booted frontend component (`None` for a Loro-only draw). Tests that
    /// attach an embedded MCP server to this session get its `DebugServices`
    /// population from here.
    pub fn frontend(&self) -> Option<&Arc<HeadlessFrontendComponent>> {
        self.frontend.as_ref()
    }

    /// Build the settle handle from a booted builder bundle — the windowed
    /// harness ([`windowed_composed_sut`]) reuses the base session's
    /// engine/frontend so its per-apply settle converges the same three
    /// projections as the headless path.
    pub fn from_bundle(bundle: &crate::pbt::composed::builder::ComposedSut) -> Self {
        let handle = Self {
            engine: bundle.engine.clone(),
            frontend: bundle.frontend.clone(),
        };
        // Arm the per-tick snapshot memo — the WideE2E harness clears it before
        // every mutation, so an armed memo only ever caches settled state.
        if let Some(frontend) = &handle.frontend {
            frontend.enable_render_cache();
        }
        handle
    }
}

/// The 3-projection convergence settle that replaces the flat `sleep(SETTLE)`
/// after a write. Waits — capped at `budget` — for every projection the
/// invariants read to reach quiescence:
///
/// 1. **Turso CDC** — `cdc_emitted_watermark` stable for one quiet floor (the
///    `block_raw` matview the block invariants query is CDC-fed).
/// 2. **Loro** — the sync controller's `last_synced_frontiers()` catches up to
///    the authority doc's `oplog_frontiers()` (a peer/merge write projects
///    async).
/// 3. **org** — the file-sync controller's `OrgSyncIdleSignal` goes quiescent
///    (the org re-render `inv-blocks-match-ref/org` reads has drained).
///
/// A CDC-only signal (the reverted lever 2) under-settled — Loro/org lagged and
/// the block/org invariants diverged; this covers all three. Signal-level core
/// shared with the `HeadlessFrontendComponent` boot settle:
/// [`crate::pbt::convergence::converge_signals`].
pub async fn converge_handle(handle: &WideHandle, budget: Duration) {
    converge_projections(handle, budget).await
}

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
    // The reactive engine whose watch-consumer drain the settle must also wait
    // out (see `converge_signals`' reactive-epoch stage): its `snapshot()` is
    // what the ViewModel invariants read.
    let reactive = handle.reactive();
    // Loop to a combined fixed point with a generous cap (a settled SUT still
    // returns in ~one quiet floor). Honour a larger caller budget if given, but
    // never wait less than CONVERGE_BUDGET — 150ms is below the heavy projection
    // pass and was the flake's root cause.
    let cap = budget.max(CONVERGE_BUDGET);
    let converged = crate::pbt::convergence::converge_signals(
        handle.engine.as_ref(),
        sync,
        store,
        org_idle,
        reactive.as_ref(),
        cap,
    )
    .await;
    assert!(
        converged,
        "[converge_projections] projections did not reach a combined fixed point within {cap:?}: \
         the Loro->SQL sort_key projection / Turso CDC / org re-render are still churning. \
         Reading invariants now would race a half-projected sink (the org-render sibling-order \
         class). This is a real non-convergence, not a flaky timeout to swallow."
    );

    // Advice weave (ADR 0022): now that CDC has converged (the `advice_rule_{slug}`
    // matview + `advice_suppressed` reflect the settled SQL state), recompute the
    // frontend's session-level advice sidecar so the pure snapshot the keystone's
    // `inv-advice-rows-woven` reads sees the woven rows. Deterministic (one-shot
    // canonical read), fail-loud through the same read path — no invariant
    // softened.
    if let Some(comp) = &handle.frontend {
        comp.reactive().refresh_advice_sidecar().await;
    }

    // Editor data-sync converge (Inc 4, EditorBufferOwnership plan): with CDC
    // settled, the focused editor's cell-free VM buffer converges against the
    // stored (trimmed) `block.content` carrying the same `write_seq` — the
    // headless analogue of prod's data subscription. The own trailing-whitespace
    // echo is kept (`AdoptBaseline`); a substantive external change converges.
    // This is what makes the `inv-editor-text/mirror` echo composition
    // observable headless (and red-capable on an echo-guard regression).
    if let Some(comp) = &handle.frontend {
        comp.converge_active_editors().await;
    }
}

/// The working tree AS the boot org (page-rooted leaf siblings, pinned bare
/// `:ID:`), so the session ingests it into the store AND `SutOrgRead` parses it
/// — store and org share one source, keeping `inv-blocks-match-ref/org` green.
/// The filename is the page title the viewmodel renders (the oracle's page
/// content is `structural-page`).
pub const WIDE_TREE_ORG: &str = "#+ID: structural-page\n* parent\n:PROPERTIES:\n:ID: \
                                 parent\n:END:\n* c1\n:PROPERTIES:\n:ID: c1\n:END:\n* \
                                 c2\n:PROPERTIES:\n:ID: c2\n:END:\n";

/// The `#+ID:` page id of the forward-edge ingest corpus.
pub fn forward_edge_page() -> EntityUri {
    EntityUri::block("forward-edge-page")
}

/// The FORWARD-EDGE INGEST regression corpus (dogfood 2026-07-10 P0).
///
/// `fe-blocked` carries a forward same-file `:REQUIRES: fe-target` edge — the
/// target is a LATER sibling in the SAME file. Before the fix, `fe-blocked`'s
/// create transaction hit the `block_requires.required_id` FK at COMMIT (the
/// target row not yet inserted), aborted the WHOLE file ingest, and silently
/// dropped every block from `fe-blocked` onward (`fe-blocked` + `fe-target`)
/// while `fe-parent` survived. The fix drops that soft target FK
/// (`crates/holon-turso/sql/schema/block_requires.sql`) so all three land.
///
/// This is the environment half of the
/// `inv-blocks-match-ref/{block_raw,matview}` SQL-projection arms (whose
/// enhanced missing-id direction reports a dropped block as loud INGEST DATA
/// LOSS). It is seeded (by [`boot_and_seed_wide`]) ONLY for a frontend draw
/// whose oracle carries the corpus ([`seed_forward_edge_corpus`]) — the Turso
/// ingest path is where the bug lives; a Loro-only draw has no such corpus and
/// those SQL-projection arms deselect there. Mirrors the standalone
/// `forward_edge_ingest_regression` shape, minus its cross-file dangling
/// `now-query-mcp` target: the INNER-JOIN `block_requirement_edges_matview`
/// drops a dangling target (matview `requires=[]`) while the org drawer keeps
/// it, so a dangling target would make the matview and org projections disagree
/// on the compared `requires` field. A forward SAME-FILE ref alone reproduces
/// the COMMIT-time FK abort with all three projections agreeing on
/// `requires=[fe-target]`.
pub const FORWARD_EDGE_ORG: &str = "#+ID: forward-edge-page\n* fe-parent\n:PROPERTIES:\n:ID: \
                                    fe-parent\n:END:\n* fe-blocked\n:PROPERTIES:\n:ID: \
                                    fe-blocked\n:REQUIRES: fe-target\n:END:\n* \
                                    fe-target\n:PROPERTIES:\n:ID: fe-target\n:END:\n";

/// The three non-page ids the forward-edge corpus MUST ingest (in file order).
/// Every one of these must land in the SUT projection — `fe-blocked` (the
/// forward-`:REQUIRES:` block) and `fe-target` are what the pre-fix FK abort
/// dropped.
pub const FORWARD_EDGE_IDS: [&str; 3] = ["fe-parent", "fe-blocked", "fe-target"];

/// Seed the forward-edge ingest corpus into `state` as NON-seed working blocks
/// under a seed `forward-edge-page` — the reference half that makes the
/// symmetric `inv-blocks-match-ref/*` arms expect
/// `fe-parent`/`fe-blocked`/`fe-target` in the SUT projection. Called by
/// [`wide_e2e_ref_for`] ONLY for a frontend wiring, so a Loro-only draw's
/// oracle never carries the corpus (and the file is never seeded there —
/// [`boot_and_seed_wide`] keys the org seed on the corpus being present in the
/// ref). The page is a seed page (filtered from the block comparison); its
/// children are non-seed and stay out of the scaffold seed (their ids are in
/// [`boot_and_seed_wide`]'s `tree` set), so a dropped `fe-blocked`/`fe-target`
/// diverges the ref/SUT block-id sets and fires `inv-blocks-match-ref/
/// {block_raw,matview}` as INGEST DATA LOSS.
pub fn seed_forward_edge_corpus(state: &mut ReferenceState) {
    let page = forward_edge_page();
    let mut page_block = Block::new_text(page.clone(), EntityUri::no_parent(), "forward-edge-page");
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
    state
        .files
        .documents
        .insert(page.clone(), "forward-edge-page.org".to_string());

    for (i, name) in FORWARD_EDGE_IDS.into_iter().enumerate() {
        let id = EntityUri::block(name);
        let mut block = Block::new_text(id.clone(), page.clone(), name);
        block.set_sequence(i as i64);
        // `fe-blocked` forward-`:REQUIRES:` its later sibling `fe-target` — the edge
        // that the pre-fix `block_requires.required_id` FK rejected at COMMIT.
        // The org drawer, the `block` matview (inner-join keeps the present
        // target), and the Loro authority all project `requires=[fe-target]`,
        // so the field comparison stays consistent.
        if name == "fe-blocked" {
            block.requires = vec![EntityUri::block("fe-target")];
        }
        state.domain.block_state.blocks.insert(id.clone(), block);
    }
}

// ── Companion page-tag demotion keystone closure (dogfood 2026-07-12) ────────
//
// Real Logseq-shaped vaults carry FOLDER-PAGE DUPLICATION: a per-page file
// (`2026-07-10.org` — a page-file whose `#+ID:` doc-root gets the `Page` tag)
// AND a COMPANION file (`Journals.org`) that inlines that SAME block id as a
// plain heading (no `Page` tag). Cold boot ingests the page-file FIRST and the
// companion LAST (`2026-07-10.org` sorts before `Journals.org` in the seed
// vec). When the companion's inlined heading (tags=[]) reconciles against the
// already- created page-file doc-root (tags=[Page]), the ingest either STRIPS
// the `Page` tag (SqlOnly — silent demotion) or, in Loro mode, its
// `create_in_tree` of the already-rooted id never lands under the companion's
// parent and the whole `Journals.org` ingest times out + quarantines. The
// oracle here models the CORRECT post-fix state (the date page STAYS a `Page`
// doc-root, page-file authoritative), so a demoting SUT diverges
// `inv-sidebar-page-tag-preserved` and a quarantining SUT diverges
// `inv-no-observed-errors`. Mirrors `seed_forward_edge_corpus`: seeded ONLY for
// a frontend draw, keyed into `boot_and_seed_wide` by the page being present in
// the ref.
//
// NOTE — the page-file is at vault ROOT (a top-level page), NOT under a
// `Journals/` subdirectory. A subdir page-file (`Journals/2026-07-10.org`)
// roots the date page UNDER the `journals` folder-page (path→name-chain), and a
// page NESTED under another page trips a SEPARATE Pages-sidebar render PANIC
// (`holon-frontend/src/row_origin.rs` "disjoint root rows" — the Pages list
// asserts every page root is `no_parent`) at BOOT. That nested-page render bug
// is distinct from this foreign-page tag-authority bug and is reported
// separately; the flat page-file isolates the Fork-A concern so this closure
// boots green once the tag-authority fix lands.

/// True when the companion closure is activated for a random keystone draw. OFF
/// by default (see `wide_e2e_ref_for`): post-fix the companion is lossy on org
/// round-trip (Fork B writeback-oracle territory). The dedicated deterministic
/// boot test seeds the topology directly instead of via this gate.
pub fn folder_companion_enabled() -> bool {
    std::env::var("HOLON_FOLDER_COMPANION_SEED").is_ok()
}

/// The `#+ID:` doc-root id of the companion date page-file (`2026-07-10.org`).
/// Its heading twin is inlined in `Journals.org`.
pub fn folder_journal_page() -> EntityUri {
    EntityUri::block("journal-2026-07-10")
}

/// The date PAGE-FILE (`2026-07-10.org`): a bare `#+ID:` doc-root whose page
/// title is the filename `2026-07-10`, PLUS a child heading so "children not
/// loaded" is observable for the `inv-embedded-page-collapsed-lazy` invariant.
/// Ingested FIRST (sorts before the companion), so it creates the `Page`-tagged
/// doc-root that the companion later tries (buggily) to demote.
pub const FOLDER_JOURNAL_PAGE_ORG: &str = "#+ID: journal-2026-07-10\n* A note on the journal date\n:PROPERTIES:\n:ID: \
     journal-date-child-note\n:END:\nSome body text under the date page.\n";

/// The `:ID:` of the child block nested under the date page in
/// [`FOLDER_JOURNAL_PAGE_ORG`]. The invariant
/// `inv-embedded-page-collapsed-lazy` asserts this child does NOT appear in the
/// main-panel widget tree unless the date page's expand-toggle is expanded.
pub fn folder_journal_page_child() -> EntityUri {
    EntityUri::block("journal-date-child-note")
}

/// `Journals.org` extended into the folder COMPANION: the bare `#+ID: journals`
/// page shell PLUS a plain heading (`* 2026-07-10`) carrying the SAME `:ID:` as
/// the page-file doc-root, with NO `Page` tag. Ingested LAST; its reconcile is
/// what demotes the page-file's `Page` tag pre-fix.
pub const FOLDER_COMPANION_JOURNALS_ORG: &str =
    "#+ID: journals\n* 2026-07-10\n:PROPERTIES:\n:ID: journal-2026-07-10\n:END:\n";

/// Seed the companion date page into `state` as a seed `Page` doc-root
/// (`block_documents[page]=no_parent`, filtered from the block-id comparison;
/// `is_page()=true` so `inv-sidebar-page-tag-preserved` expects the SUT to keep
/// the `Page` tag). Its org file is `2026-07-10.org`. Mirrors
/// [`seed_forward_edge_corpus`]; called by [`wide_e2e_ref_for`] ONLY for a
/// frontend wiring. The companion heading in `Journals.org` references the SAME
/// id (no new block), so the ref models exactly ONE block — the page-file
/// doc-root that must survive as a `Page`.
pub fn seed_folder_companion(state: &mut ReferenceState) {
    let page = folder_journal_page();
    let mut page_block = Block::new_text(page.clone(), EntityUri::no_parent(), "2026-07-10");
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
    state
        .files
        .documents
        .insert(page.clone(), "2026-07-10.org".to_string());

    // Seed the child block nested under the date page so
    // `inv-embedded-page-collapsed-lazy` has something to observe: a ref-known
    // block whose presence in the main-panel widget tree signals eager rendering.
    let child = folder_journal_page_child();
    let child_block = Block::new_text(child.clone(), page.clone(), "A note on the journal date");
    state
        .domain
        .block_state
        .blocks
        .insert(child.clone(), child_block);
    state
        .domain
        .block_state
        .block_documents
        .insert(child.clone(), page);
}

// ── Row-137 SUBDIR fileless-journal closure (Fork B B1) ──────────────────────
//
// The REAL BugFunnel row-137 shape (distinct from the flat top-level
// `seed_folder_companion` above, which is Fork A's page-tag-demotion closure):
// a journal DATE PAGE that is NESTED under the `journals` folder-page and is
// FILELESS — it exists ONLY as a `:Page:`-tagged heading inlined in the
// `Journals.org` companion, with NO date file of its own on disk. On boot the
// ingest creates the `Page`-tagged date block under `block:journals`; writeback
// must then (a) MATERIALIZE it into its OWN subdir file
// `Journals/2026-07-11.org` (`name_chain(date) = ["Journals","2026-07-11"]`,
// because the nearest ancestor page is `journals`) — else its body lives only
// in the store and vanishes on any store-rebuild-from-disk (the row-137 loss) —
// and (b) DE-INLINE the heading from `Journals.org` (the `get_blocks` CTE
// excludes the `Page`-tagged child). The ADR-0025 sibling-grounded union guard
// admits the de-inline (the child survives in its own now-materialized file),
// and the B2 boot sweep does the materialization + `last_projection` echo-seed.
//
// The date `2026-07-11` is chosen to NOT collide with the fixed keystone boot
// clock (`2026-01-15`, `keystone_boot_journal_date`) so the auto-create rule's
// `when: 'not block_exists("Journals/{today}")'` never touches it.

/// The `#+ID:` doc-root id of the fileless SUBDIR journal date page — nested
/// under `block:journals`, inlined (fileless) in the `Journals.org` companion.
pub fn subdir_journal_page() -> EntityUri {
    EntityUri::block("journal-2026-07-11")
}

/// `Journals.org` as the row-137 companion: the bare `#+ID: journals` page
/// shell PLUS a `:Page:`-tagged heading (`* 2026-07-11 :Page:`) carrying the
/// date page's `:ID:`, with body text that must not vanish. There is NO
/// `Journals/2026-07-11.org` on disk — writeback must materialize it (the loss
/// row 137 reports).
pub const SUBDIR_COMPANION_JOURNALS_ORG: &str = "#+ID: journals\n* 2026-07-11 :Page:\n:PROPERTIES:\n:ID: journal-2026-07-11\n:END:\nbody text \
     that must materialize into its own subdir file\n";

/// Seed the fileless subdir journal date page into `state` as a NON-SEED `Page`
/// doc-root nested under `block:journals`:
/// - `is_page()=true` — the two Fork-B oracles
///   (`inv-companion-has-no-child-page-headings`,
///   `inv-every-page-has-its-own-file`) treat it as a child page.
/// - `parent_id = block:journals` — nesting that drives `name_chain` to the
///   subdir path and satisfies `inv-no-page-under-non-page` (date→journals→root
///   all pages).
/// - `block_documents[page] = page` (self-documenting, NON-seed) so it is
///   INCLUDED in `all_non_seed_block_ids` — the `every-page-has-its-own-file`
///   oracle checks it (a seed-classified page would be skipped, leaving the
///   oracle inert on row 137).
/// - `files.documents[page] = "Journals/2026-07-11.org"` — the subdir file it
///   must own.
///
/// Mirrors [`seed_folder_companion`]; `block_and_seed_wide` keys the fileless
/// `Journals.org` companion seed on this page being present in the ref.
pub fn seed_folder_companion_subdir(state: &mut ReferenceState) {
    let journals = EntityUri::parse("block:journals").expect("journals id");
    let page = subdir_journal_page();
    let mut page_block = Block::new_text(page.clone(), journals.clone(), "2026-07-11");
    page_block.set_page(true);
    state
        .domain
        .block_state
        .blocks
        .insert(page.clone(), page_block);
    // NON-seed: the page is its own document root (owns its own file), so the
    // block-doc is the page itself — NOT `no_parent`/sentinel (which would seed-
    // classify it and hide it from `all_non_seed_block_ids`).
    state
        .domain
        .block_state
        .block_documents
        .insert(page.clone(), page.clone());
    state
        .files
        .documents
        .insert(page.clone(), "Journals/2026-07-11.org".to_string());
}

// ── Journals boot auto-create closure (dogfood #4, 2026-07-12) ───────────────
//
// Prod's `build_default_layout_blocks` seeds the journal auto-create RULE (a
// `holon_sql` trigger `SELECT today FROM clock` + a `holon_rule` action
// `block.create`) on every boot. On a Turso + ActionEngine frontend boot the
// production `ClockScheduler` seeds the `clock` day row, the trigger matview
// fires, and the `action_watcher` mints ONE journal day-block under
// `block:journals` with a WP2 deterministic id. The rule blocks themselves are
// modeled in `seed_booted_layout_into_ref` (they are seeded on every boot); the
// boot-FIRED journal day-block is modeled here because only a Turso +
// ActionEngine boot fires it. The keystone frontend boot injects the fixed
// `keystone_boot_clock` (Fork A) so the date + id are deterministic.
//
// This subsumes the retired env-gated `HOLON_JOURNALS_MACHINERY_SEED`
// trigger-only id-less-render closure: the real rule is now always seeded, so
// the id-less VALUE row travels the render path on every frontend boot (the
// panic→no-panic behaviour is also pinned at the unit layer by
// `reactive::tests::id_less_value_row_does_not_panic_apply_change`).

/// Seed the boot-fired journal day-block into `state` as a NON-seed child of
/// the seed `block:journals` page: the reference half that makes the
/// block-comparison invariants expect the auto-created journal in the SUT
/// projection. Its id is the deterministic effect id the production
/// `action_watcher` mints for the fixed keystone clock day
/// (`keystone_boot_journal_id`), so the ref lands in the SUT id space exactly.
/// Called by [`wide_e2e_ref_for`] for a frontend wiring whose
/// `Actor::ActionEngine` is present (implies Turso); the SUT fires it live.
pub fn seed_boot_journal(state: &mut ReferenceState) {
    use crate::pbt::frontend_slice::components::keystone_boot_journal_date;
    // The boot rule fires ONCE at boot, appended after the seeded
    // `Journal Auto-Create` heading (sequence 0) — sequence 1.
    seed_journal_day(state, &keystone_boot_journal_date(), 1);
}

/// Seed the journal day-block the production `journals_auto_create` rule fires
/// for `date` into the composed reference, as a NON-seed self-documenting
/// `Page` child of the seed `block:journals` page — the reference half that
/// makes the block-comparison invariants + the per-tick reconcile expect the
/// rule-minted journal in the SUT projection.
///
/// Shared by the boot seed (`seed_boot_journal`) and the clock-advance path
/// (`RefClockMut::advance_day`): both go through the SAME production emit
/// (`fire_emit`'s `place: page(journals)` branch), so the id is the CANONICAL
/// page identity `PageId::for_path("Journals/{date}")` — the exact
/// deterministic id the `action_watcher` mints — and the shape is a `Page`
/// owning its own subdir file `Journals/{date}.org` (LogSeq-parity daily-note
/// ruling 2026-07-19). Idempotent: a date already present (same-day re-tick, or
/// a re-seed) is left untouched.
///
/// `sequence` places the block in sibling order after the seed
/// `Journal Auto-Create` heading (sequence 0). The oracle orders siblings by
/// `(sibling_order_group, sequence, id)` (ADR 0005); the SUT appends each
/// rule-fired journal LAST (fractional index after all prior siblings), so a
/// strictly-increasing sequence per rollover (boot = 1, then 2, 3, …) matches
/// the SUT's created-order — see `RefClockMut::advance_day`.
pub fn seed_journal_day(state: &mut ReferenceState, date: &str, sequence: i64) {
    let id = holon_api::link_parser::PageId::for_path(&format!("Journals/{date}"))
        .expect("journal page path is well-formed")
        .into_entity_uri();
    // Idempotent: a same-day re-tick (or a re-seed) must not duplicate or churn.
    if state.domain.block_state.blocks.contains_key(&id) {
        return;
    }
    let journals = EntityUri::parse("block:journals").expect("journals id");
    let mut block = Block::new_text(id.clone(), journals, date);
    block.set_page(true);
    block.set_sequence(sequence);
    state.domain.block_state.blocks.insert(id.clone(), block);
    // Self-documenting page (`block_documents[id]=id`, NON-seed) so it is INCLUDED
    // in `all_non_seed_block_ids` — `inv-every-page-has-its-own-file` checks it.
    state
        .domain
        .block_state
        .block_documents
        .insert(id.clone(), id.clone());
    state
        .files
        .documents
        .insert(id, format!("Journals/{date}.org"));
}

/// The page-rooted leaf-sibling oracle (`parent`/`c1`/`c2` re-rooted under a
/// seed `page_root`, focused on the page), wired by `subsystems` (invariant
/// selection) + nav-history aligned to the headless boot stack `[journals,
/// page]`.
pub fn structural_ref_wired(
    subsystems: &BTreeSet<crate::pbt::invariants::registry::Subsystem>,
) -> ReferenceState {
    let mut state = build_started_ref(subsystems);
    let page = page_root();
    let ids = fixed_ids();

    // Insert the page root: a seed block (`block_documents[page]=no_parent`,
    // filtered out of the comparison) AND a page (excluded from
    // `main_editable_descendants`).
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
    // `boot_and_seed_wide` writes `WIDE_TREE_ORG` to) and its doc-uri is the page
    // id (`block:structural-page` — the `#+ID:`-derived `file_id` the parser
    // hands the SUT's `documents` key). Populating this un-gates the External
    // (org) `ApplyMutation` arm and `BulkExternalAdd` (both require a non-empty
    // `files.documents`); the value matches the SUT org filename so the native
    // StartApp name-reconcile stays aligned too.
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

    // Nav-history boot alignment: mirror the headless SUT's `[journals, page]`
    // cursor-1 stack (page-pin id 2, next_history_id 3) so the folded nav
    // transitions stay in lockstep with the AUTOINCREMENT counter.
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

/// The structural oracle (no extra subsystems wired — focus caps absent so it
/// never false-REDs the focus invariants).
pub fn structural_ref() -> ReferenceState {
    structural_ref_wired(&BTreeSet::new())
}

/// The combined wide oracle: the same page-rooted tree as [`structural_ref`],
/// wired `{Loro, EditorState}` so the editor/focus transitions gate. No editor
/// open at start (the boot's auto-open on `c1` is blurred by the final
/// `NavigateFocus(page)`).
pub fn wide_ref() -> ReferenceState {
    use crate::pbt::invariants::registry::Subsystem;
    let subsystems: BTreeSet<Subsystem> = [Subsystem::Loro, Subsystem::EditorState]
        .into_iter()
        .collect();
    structural_ref_wired(&subsystems)
}

/// The caps the widest headless wiring (`full_headless`) legitimately does NOT
/// provide, so the catalog invariants that `Needs` them deselect headless
/// WITHOUT that being a silent-deselection bug. Each entry is `(cap-name,
/// why)`. This is the ONLY hand-maintained list the `wide_cap_presence_guard`
/// consults.
///
/// All four are the windowed/GPUI rung: they need a live gpui window (thread
/// affinity `compose_sut` cannot satisfy — it asserts `!Actor::UI` in
/// `builder.rs`) and are supplied ONLY by the windowed slice (`window_slice`),
/// so the catalog invariants that `Needs` them deselect headless AND run only
/// in the windowed harness — NOT a silent-deselection bug. A cap that SHOULD be
/// headless-present but isn't is NOT allowed here; it is a real finding the
/// guard must surface (that is the whole point of listing each one explicitly,
/// with a reason).
#[cfg(test)]
const WIDE_HEADLESS_ABSENT_CAPS: &[(&str, &str)] = &[
    (
        "SutLayout",
        "windowed-only: a laid-out widget tree + BoundsRegistry (geometry) comes only from \
         GpuiWindowComponent over a live gpui window; headless compose_sut has no window",
    ),
    (
        "SutDriver",
        "windowed-only: the engine-focus read (engine_focused_block / resolve_ref_block_id) is a \
         window cap; the headless path registers only the gesture WRITE caps \
         (register_gesture_writes), so the focus-read deselects in the keystone by design",
    ),
    (
        "SutFrontendEngine",
        "windowed-only: root-VM liveness reads (frontend_root_vm / frontend_root_is_error / \
         live_vs_fresh_tree_diff); the headless frontend registers no window engine, only \
         GpuiFrontendEngineComponent does",
    ),
    (
        "SutPerBlockShellMount",
        "windowed-only, and GPUI-only within that: names the frontend mount convention under \
         test (panel blocks wrapped in a per-block ReactiveShell, tracked as `live_block`). \
         Registered by overlay_windowed_caps from WindowMountConvention; headless compose_sut \
         mounts nothing",
    ),
    (
        "SutInlineRowMount",
        "windowed-only, and TUI-only within that: the sibling mount convention (doc-block rows \
         resolved inline, tracked as `render_entity`). Registered by overlay_windowed_caps from \
         WindowMountConvention; headless compose_sut mounts nothing",
    ),
    (
        "SutTwoInstance",
        "two-instance-only: sharing + sync control over a SECOND booted session and a relay; a \
         single-instance compose_sut has no second side to share with",
    ),
    (
        "SutReceiverBackend",
        "two-instance-only: the RECEIVER session's projections; a single-instance compose_sut has \
         no receiver, and reading the owner through this cap would pass vacuously",
    ),
    (
        "SutFrontendEmissions",
        "windowed-only: drain_vm_emissions / provider_stability_report need the live windowed \
         frontend engine; the headless ReactiveEngine returns honest-empty and deselects rather \
         than faking",
    ),
    (
        "SutClockAdvance",
        "env-gated, not absent: composed/builder.rs registers it ONLY under \
         HOLON_PBT_ADVANCE_DAY, deliberately dormant until the co-landed ref model + \
         inv-journal-one-per-day prove green under the full catalog. Flip the env on and the \
         cap — and inv-journal-one-per-day with it — is present. Same gate the non-vacuity \
         guard's ENV_GATED_ALLOWLIST cites for AdvanceDay",
    ),
];

/// The wide working tree (`page_root` → `parent`/`c1`/`c2` siblings) as a
/// structured boot seed — the non-frontend face of the same tree
/// `WIDE_TREE_ORG` encodes for the frontend org boot, derived from the SAME
/// fixed ids + contents `structural_ref_wired` re-roots the oracle into, so SUT
/// and oracle agree by construction. Order matters: `page_root` first, then its
/// children (so the builder's `create_block` replay nests them and the sibling
/// sequence is `0,1,2`).
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

/// Boot the windowless production SUT for the oracle's wiring via the
/// PRODUCTION builder (`compose_sut_seeded`) and seed the working tree, then
/// (for a focus-capable config) drive the initial focus onto the page root
/// (matching the oracle) and return the cap map + the scaffold ids to
/// seed-inject into the oracle.
///
/// The builder owns boot+seed for every wiring: a **frontend**
/// (Turso+ViewModel) config ingests `WIDE_TREE_ORG` through its session's
/// file-sync adapter; a **non-frontend** (Loro-only) config has no session, so
/// the builder creates [`wide_seed_tree`] directly into the canonical Loro
/// backend. Both faces encode the same tree, so the SUT matches the
/// oracle either way. Org carries no special status here (ADR 0004 — the domain
/// is canonical, org/Loro/Turso are peer adapters): it's just the serialization
/// the frontend session's file-sync happens to read; the non-frontend face is
/// structured domain CRUD.
pub async fn boot_and_seed_wide(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
) -> (CapMap, WideHandle, BTreeSet<EntityUri>) {
    boot_and_seed_wide_with_peer_id(resolver, ref_state, None).await
}

/// [`boot_and_seed_wide`] with this session's Loro peer id pinned — the
/// two-instance slice's owner arm. `None` keeps the env/random default every
/// single-instance caller relies on.
pub async fn boot_and_seed_wide_with_peer_id(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
    peer_id: Option<u64>,
) -> (CapMap, WideHandle, BTreeSet<EntityUri>) {
    // SUT-side parameterization seam: the booted set follows the oracle's DRAWN
    // wiring (`init_state` draws `any_valid_wiring()`; `HOLON_PBT_FORCE_FULL=1`
    // pins `full_headless`). Disclose the draw per case so a run log yields
    // per-wiring case counts (grep "wide-e2e wiring") -- the drawn grid is
    // auditable, not assumed.
    let set = set_for_wiring(&ref_state.harness.wiring);
    eprintln!(
        "[wide-e2e wiring] drawn: storage={:?} sync={:?} actors={:?} -> booted storage={:?} \
         projections={:?}",
        ref_state.harness.wiring.storage_adapters,
        ref_state.harness.wiring.sync_adapters,
        ref_state.harness.wiring.actors,
        set.wiring.storage_adapters,
        set.projections,
    );
    let has_frontend = set.has_projection(Projection::ViewModel);
    // Scale-soak inflation: extra synthetic doc files (deep trees, tasks, links,
    // unicode) appended to the SUT boot ONLY. Empty unless `HOLON_SOAK_SEED_BLOCKS`
    // is set, so the keystone is untouched by default. Their ids fold into the
    // oracle via the scaffold math below (booted-but-not-tree ⇒ seed-classified),
    // so the invariant catalog stays green while every action pays the whole-vault
    // projection/CDC/consolidator cost.
    let soak_files = crate::pbt::composed::soak_seed::soak_org_files();
    // `block:journals` is a first-boot page that is disk-backed in prod (the
    // packaged `assets/default/Journals.org`, fixed id `block:journals`). Seed a
    // bare page SHELL (`#+ID: journals`, no query/render/action body) so the
    // journals page is a genuine on-disk file AND is TRACKED for the `/org`
    // comparison. Prod-faithful for the keystone's non-empty vault: on a vault
    // that already has `.org` files, `seed_default_org_assets` does NOT re-seed
    // the packaged body, but `seed_default_layout` still creates the page shell
    // idempotently — exactly the shell modeled here. Body blocks are omitted on
    // purpose: ingesting the packaged query/render/action source blocks would add
    // rows the oracle's first-boot layout model does not carry (→ a `block_raw`
    // false-divergence). With the shell tracked, a non-page block created under
    // journals via `CreateBlockUnderFocus` lands INLINE in `Journals.org` (the
    // page-file-placement rule: `doc_id_to_path` → `name_chain(block:journals)` =
    // `["Journals"]` → `<root>/Journals.org`, because the child's nearest ancestor
    // page IS journals), so the `/org` snapshot observes it and matches the
    // reference's `org_blocks` (non-seed, non-page journals child). Mirrors the
    // `live_mcp` sibling harness's `Journals.org` seed, extended to also track it.
    // `Journals.org`: the bare page shell. The journal auto-create RULE (trigger +
    // action) is seeded PROGRAMMATICALLY by prod's `build_default_layout_blocks`
    // (not via this disk file), so the disk `Journals.org` stays a bare `#+ID:`
    // shell — matching prod, where `DEFAULT_ASSETS` is empty and journals is a
    // programmatic seed. (Non-machinery variant kept for the folder-companion.)
    // Companion demotion closure: when the oracle carries the date page
    // (`seed_folder_companion`, a frontend draw), `Journals.org` becomes the
    // COMPANION that inlines the page-file's id as a plain heading, and the
    // top-level page-file `2026-07-10.org` is seeded FIRST (it sorts before the
    // `Journals.org` companion → cold-boot ingests the `Page` doc-root before the
    // demoting companion reconcile). Keyed on the ref like forward-edge.
    let carries_folder_companion = ref_state
        .domain
        .block_state
        .blocks
        .contains_key(&folder_journal_page());
    // Row-137 subdir fileless closure (Fork B B1): when the oracle carries the
    // NESTED fileless date page (`seed_folder_companion_subdir`), `Journals.org`
    // becomes the row-137 companion that inlines it as a `:Page:` heading with NO
    // date file of its own — writeback must materialize `Journals/2026-07-11.org`
    // and de-inline the heading. Mutually exclusive with the flat Fork-A closure.
    let carries_subdir_companion = ref_state
        .domain
        .block_state
        .blocks
        .contains_key(&subdir_journal_page());
    let journals_org: &str = if carries_subdir_companion {
        SUBDIR_COMPANION_JOURNALS_ORG
    } else if carries_folder_companion {
        FOLDER_COMPANION_JOURNALS_ORG
    } else {
        "#+ID: journals\n"
    };
    let mut seed_files: Vec<(&str, &str)> = vec![("structural-page.org", WIDE_TREE_ORG)];
    if carries_folder_companion {
        seed_files.push(("2026-07-10.org", FOLDER_JOURNAL_PAGE_ORG));
    }
    // NB the subdir closure seeds NO date file on purpose (fileless — the loss
    // row 137 reports; writeback materializes it).
    seed_files.push(("Journals.org", journals_org));
    // Forward-edge ingest corpus (dogfood 2026-07-10 P0): seed
    // `forward-edge-page.org` through the REAL FileSyncController ingest ONLY
    // when this draw's oracle carries the corpus (a frontend draw —
    // `wide_e2e_ref_for` inserts it via `seed_forward_edge_corpus`). Keying the
    // file seed on the oracle keeps every non-corpus frontend boot (the teeth,
    // which build their own oracles) untouched: no corpus in the ref ⇒ no file on
    // disk ⇒ no `/org` divergence.
    if ref_state
        .domain
        .block_state
        .blocks
        .contains_key(&forward_edge_page())
    {
        seed_files.push(("forward-edge-page.org", FORWARD_EDGE_ORG));
    }
    for (name, body) in &soak_files {
        seed_files.push((name.as_str(), body.as_str()));
    }
    let bundle = match peer_id {
        Some(peer_id) => {
            compose_sut_seeded_with_peer_id(&set, resolver, &seed_files, &wide_seed_tree(), peer_id)
                .await
        }
        None => compose_sut_seeded(&set, resolver, &seed_files, &wide_seed_tree()).await,
    };
    // The settle handles — the Turso engine (CDC watermark) and the frontend
    // component (Loro sync + org idle). Cloned out before `bundle.caps` is
    // moved so the post-write [`converge_projections`] settle can prove all
    // three projections drained.
    let handle = WideHandle {
        engine: bundle.engine.clone(),
        frontend: bundle.frontend.clone(),
    };
    // Arm the per-tick snapshot memo now that the WideE2E harness (which clears
    // it before every mutation) owns this component — see `WideHandle`.
    if let Some(frontend) = &handle.frontend {
        frontend.enable_render_cache();
    }
    let mut caps = bundle.caps;

    // Scale-soak: drain the WHOLE seeded vault into `block_raw` BEFORE the scaffold
    // id-set is snapshotted below. The frontend boot settle is a flat 300ms — far
    // too short to project 5–10k blocks — so an un-drained soak block would be
    // absent from `booted`, escape seed-classification in the oracle, and later
    // surface in the SUT store with no matching oracle seed entry → a false
    // `inv-blocks-match-ref` divergence. Off (count 0) this is skipped
    // entirely; the keystone is untouched.
    if crate::pbt::composed::soak_seed::soak_block_count() > 0 {
        converge_projections(&handle, crate::pbt::composed::soak_seed::soak_settle()).await;
    }

    // Fork B (dogfood #4): the boot auto-create rule fires today's journal
    // day-block ASYNCHRONOUSLY off the clock CDC (fixed keystone clock → `clock`
    // day row → trigger matview → `action_watcher` → `block.create`). The flat
    // 300ms boot settle can return before that chain lands, so await the journal
    // in `block_raw` BEFORE the scaffold snapshot / invariant checks — else a
    // not-yet-fired journal false-diverges `inv-blocks-match-ref` as "missing in
    // SQL". EVERY frontend (Turso) boot fires it — the rule-firing machinery
    // (ClockScheduler + action watchers) runs unconditionally on any non-wasm
    // Turso boot, NOT behind the `Actor::ActionEngine` label (see the oracle's
    // `seed_boot_journal` gate in `wide_e2e_ref_for`). Await it on every frontend
    // draw so the snapshot is taken AFTER it lands; fail loud on timeout so a
    // genuinely-dropped firing is a RED, not a hang.
    if has_frontend {
        let journal_id = crate::pbt::frontend_slice::components::keystone_boot_journal_id();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            converge_projections(&handle, Duration::from_millis(300)).await;
            if sut_ids(&caps).await.contains(&journal_id) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "[boot journal] auto-create rule did not fire journal {journal_id} within budget"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    // `inv-settle-budget` coverage: the per-transition latency recorder the
    // harness fills from its timed apply+settle window (the same window the
    // `holon_latency` `stage=action_total` event reports). One `Arc`,
    // registered as both the write (lifecycle) and read cap. NOT otel-gated —
    // a wall clock needs no span collector.
    {
        use crate::pbt::composed::settle_latency::ComposedSettleLatency;
        use crate::pbt::composed::settle_latency::SettleLatency;
        use crate::pbt::composed::settle_latency::SettleLatencyLifecycle;
        let s = std::sync::Arc::new(ComposedSettleLatency::new());
        caps.insert(s.clone() as std::sync::Arc<dyn SettleLatency>);
        caps.insert(s as std::sync::Arc<dyn SettleLatencyLifecycle>);
    }

    // `inv-sql-budget` coverage: a span-metrics provider hosting the SAME
    // `MetricsSut` the native E2ESut uses, exposed through `ComposedBudget`
    // (the read) + `SutMetricsLifecycle` (the `ComposedSut` harness drives
    // reset-on-apply / freeze-on-check). One `Arc`, registered as both caps.
    #[cfg(feature = "otel-testing")]
    {
        use crate::pbt::composed::complexity_trend::ComposedTrend;
        use crate::pbt::composed::span_metrics::ComposedBudget;
        use crate::pbt::composed::span_metrics::ComposedSpanMetrics;
        use crate::pbt::composed::span_metrics::SutMetricsLifecycle;
        let m = std::sync::Arc::new(ComposedSpanMetrics::new());
        caps.insert(m.clone() as std::sync::Arc<dyn ComposedBudget>);
        // Same host, third cap: `inv-complexity-class-trend` fits the counter
        // series the budget's own freeze point already produces.
        caps.insert(m.clone() as std::sync::Arc<dyn ComposedTrend>);
        caps.insert(m as std::sync::Arc<dyn SutMetricsLifecycle>);

        use crate::pbt::composed::observed_errors::ComposedObservedErrors;
        use crate::pbt::composed::observed_errors::ObservedProblems;
        caps.insert(std::sync::Arc::new(ComposedObservedErrors::new())
            as std::sync::Arc<dyn ObservedProblems>);

        // Reseed-attribution pin (Inc 0): the read cap + a per-case reset of the
        // process-global observer, so each case's full-reseed attribution starts
        // clean (mirrors the `SpanCollector::reset` in the harness).
        use crate::pbt::composed::reseed_observer::ComposedReseedObserver;
        use crate::pbt::composed::reseed_observer::ReseedAttribution;
        use crate::pbt::composed::reseed_observer::ReseedObserver;
        ReseedObserver::global().reset();
        caps.insert(std::sync::Arc::new(ComposedReseedObserver::new())
            as std::sync::Arc<dyn ReseedAttribution>);
    }

    // Scaffold = everything the SUT booted OR the oracle models, EXCEPT the
    // non-seed working tree (parent/c1/c2) — and, for a frontend config, EXCEPT
    // `block:journals`.
    //
    // The union makes the seed wiring-agnostic: a frontend SUT boots
    // `block:journals` + the index.org layout (in `booted`); a non-frontend SUT
    // does NOT, but the oracle still models that layout, so those ids must come
    // from the ref side to be seed-injected and filtered — otherwise they'd
    // false-diverge.
    //
    // `block:journals` is the ONE first-boot page that is self-documenting
    // (`block_documents[journals]=journals`, i.e. NON-seed) rather than
    // seed-classified like `__default__`/index.org. For a frontend config it is
    // present on BOTH sides (SUT boots it, oracle models it), so we keep it OUT
    // of the seed-filter and let `inv-blocks-match-ref/block_raw` ASSERT it —
    // the user-visible first-boot journals page is verified, not hidden. A
    // non-frontend SUT never boots it, so there it stays in the scaffold
    // (filtered) to match the oracle's modeled-but-not-booted copy.
    let ids = fixed_ids();
    let journals = EntityUri::parse("block:journals").expect("journals id");
    // The non-seed working tree kept OUT of the scaffold seed (so it stays
    // compared): `parent`/`c1`/`c2` plus, for a frontend draw that seeded it,
    // the forward-edge corpus children (`fe-parent`/`fe-blocked`/`fe-target`).
    // Listing the corpus ids unconditionally is a no-op for a Loro-only draw
    // (they are in neither `booted` nor `ref_ids` there); for a frontend draw
    // it keeps them non-seed so a dropped `fe-blocked`/`fe-target` diverges the
    // block-id sets and fires `inv-blocks-match-ref/{block_raw,matview}` as
    // INGEST DATA LOSS. The journal auto-create RULE blocks (`Journal
    // Auto-Create` heading + `holon_rule` action, seeded on every boot) and the
    // boot-FIRED journal day-block are non-seed children of journals — kept
    // compared, like the forward-edge children, so a dropped rule block or a
    // missing/duplicate boot journal fires `inv-blocks-match-ref`. Listed
    // unconditionally: a no-op for a draw where an id is in neither `booted` nor
    // `ref_ids` (e.g. the auto-create rule + boot journal on a non-frontend
    // draw, which `wide_e2e_ref_for` keeps out of the oracle — they are seeded
    // only by a frontend boot's `build_default_layout_blocks`).
    let tree: BTreeSet<EntityUri> = [ids.parent.clone(), ids.c1.clone(), ids.c2.clone()]
        .into_iter()
        .chain(FORWARD_EDGE_IDS.into_iter().map(EntityUri::block))
        .chain([
            EntityUri::parse(holon_frontend::JOURNALS_AUTO_CREATE_ID).expect("auto-create id"),
            EntityUri::parse(holon_frontend::JOURNALS_ACTION_ID).expect("holon_rule id"),
            crate::pbt::frontend_slice::components::keystone_boot_journal_id(),
        ])
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

    // Fresh-drive the initial focus on the SUT to match the oracle (page root) —
    // only for a focus-capable config. A non-frontend (Loro-only) SUT has no
    // `SutFocusWrite` cap (no ViewModel/nav), and its focus/nav invariants
    // deselect, so there is nothing to align; driving `NavigateFocus` there
    // would hit an absent cap.
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

/// The §Round-5 windowed dual of [`boot_and_seed_wide`]: boot the SAME wide
/// working tree through the production builder, but with the driver rung
/// **deferred** ([`compose_sut_windowed_base_seeded`]) so the gpui-thread
/// harness can INSERT the window's `GpuiUserDriver`-backed gesture caps via
/// `overlay_windowed_caps`. Returns the full builder bundle (its booted
/// `session`/`reactive` are what the window binds as a pure renderer) plus
/// the scaffold ids to seed-inject into the oracle — identical scaffold math to
/// `boot_and_seed_wide`, so the SAME [`wide_e2e_ref`] oracle matches. The
/// initial focus-align (page root) is driven LATER, through the overlaid caps
/// (they carry the window driver), by [`windowed_composed_sut`]. A window needs
/// a session, so the frontend arm is mandatory here.
pub async fn boot_and_seed_wide_windowed_base(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
) -> (
    crate::pbt::composed::builder::ComposedSut,
    BTreeSet<EntityUri>,
) {
    let set = set_for_wiring(&ref_state.harness.wiring);
    assert!(
        set.has_projection(Projection::ViewModel),
        "the windowed wide base needs a frontend (ViewModel) session for the window to render; \
         got {set:?}"
    );
    // Seed-parity with the headless `boot_and_seed_wide`: the windowed oracle is a
    // frontend (ViewModel) draw, so `wide_e2e_ref_for` ALWAYS injects the
    // forward-edge corpus (`seed_forward_edge_corpus`). The windowed base MUST
    // ingest the matching `forward-edge-page.org` file or the SUT is permanently
    // missing `fe-parent`/`fe-blocked`/`fe-target` that the oracle models — a
    // `SetEdgeField`/`SetupWatch` targeting any fe-* then fails loud ("Block not
    // found: block:fe-target") or diverges `inv-watch-rows-match-ref`. Key it on
    // the oracle carrying the corpus, exactly like the headless path.
    let mut seed_files: Vec<(&str, &str)> = vec![("structural-page.org", WIDE_TREE_ORG)];
    if ref_state
        .domain
        .block_state
        .blocks
        .contains_key(&forward_edge_page())
    {
        seed_files.push(("forward-edge-page.org", FORWARD_EDGE_ORG));
    }
    let bundle =
        compose_sut_windowed_base_seeded(&set, resolver, &seed_files, &wide_seed_tree()).await;

    // Align the initial focus onto the oracle's page root via the production
    // `NavigateFocus` cap — `SutFocusWrite` dispatches through the reactive
    // engine's `dispatch_intent_sync`, which runs the `navigation.focus` SQL
    // write AND mirrors focus into `engine.focused_block()`
    // (`maybe_mirror_navigation_focus`), exactly as a production sidebar page-nav
    // does. Done on the deferred base pre-window; window bring-up does not
    // reset engine focus, so the first render paints the already-focused
    // engine. Mirrors `boot_and_seed_wide`'s headless drive.
    //
    // §8.12 insert-only: the deferred base's `bundle.caps` is gesture-CAPLESS so
    // the gpui-thread overlay can INSERT the window-driver gesture caps. So
    // this seed focus-align (NOT a tested transition — it's boot state) drives
    // through a THROWAWAY gesture map bound to the component's OWN headless
    // `ReactiveEngineDriver`. The focus effect lands on the SHARED engine/reactive,
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

    // Real START barrier (replaces the flat `sleep(SETTLE)`): drive the async
    // seed chains to a STORE-VISIBLE fixed point BEFORE the window loop begins
    // driving. `forward-edge-page.org` ingests through the REAL FileSyncController
    // (file -> Turso -> CDC -> Loro) and the boot journal fires async off the
    // clock CDC — both can lag a flat sleep, so the first transition could hit a
    // store where fe-* / the journal do not yet exist (the windowed-loop flake).
    // Await them through the SAME `sut_ids` read path the transitions use — the
    // headless `boot_and_seed_wide` uses this exact loop for the boot journal.
    // Fail loud on timeout so a genuinely-dropped seed is a RED, not a silent
    // green.
    let start_handle = WideHandle::from_bundle(&bundle);
    let mut start_expected: BTreeSet<EntityUri> = BTreeSet::new();
    if ref_state
        .domain
        .block_state
        .blocks
        .contains_key(&forward_edge_page())
    {
        start_expected.insert(forward_edge_page());
        start_expected.extend(FORWARD_EDGE_IDS.into_iter().map(EntityUri::block));
    }
    start_expected.insert(crate::pbt::frontend_slice::components::keystone_boot_journal_id());
    let start_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        converge_projections(&start_handle, Duration::from_millis(300)).await;
        let booted_now = sut_ids(&bundle.caps).await;
        if start_expected.iter().all(|id| booted_now.contains(id)) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < start_deadline,
            "[windowed base start barrier] seed set never became store-visible within budget; \
             missing: {:?}",
            start_expected.difference(&booted_now).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Scaffold = booted UNION ref_ids MINUS working tree (identical to
    // `boot_and_seed_wide`).
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

/// Assemble the windowed
/// [`ComposedSut<WideE2E>`](crate::pbt::composed::harness::ComposedSut)
/// around the OVERLAID windowed caps (the gpui-thread harness produced them by
/// attaching a window over a [`boot_and_seed_wide_windowed_base`] session and
/// calling `overlay_windowed_caps`). The initial page-root focus-align is
/// already done on the base by [`boot_and_seed_wide_windowed_base`]
/// (pre-window), so this just wraps the caps via [`ComposedSut::from_parts`].
/// `settle` pumps the window before each check; `rt` drives the apply/check
/// futures (the booted backend runs on its own session runtime).
pub fn windowed_composed_sut(
    caps: CapMap,
    handle: WideHandle,
    resolver: IdResolver,
    scaffold_ids: BTreeSet<EntityUri>,
    rt: tokio::runtime::Runtime,
    settle: crate::pbt::composed::harness::SettleHook,
) -> crate::pbt::composed::harness::ComposedSut<WideE2E> {
    // The `handle` carries the base session's engine/frontend so the per-apply
    // [`converge_projections`] settle covers the same three projections as the
    // headless path. The `settle` hook additionally pumps the gpui window
    // before each check.
    crate::pbt::composed::harness::ComposedSut::<WideE2E>::from_parts(
        caps,
        handle,
        resolver,
        scaffold_ids,
        rt,
        settle,
    )
}

/// Normalize a (possibly drawn) `Wiring` into the composed **headless**
/// `ComponentSet` the `general_e2e_composed_pbt` swap boots — the SUT half of
/// the wiring draw (`init_state` draws `any_valid_wiring()`; this maps each
/// draw to a bootable set). Mirrors the native `storage_selector_for_wiring`
/// backend choice so a Loro-only draw maps to the cheap `LoroMemory` SUT and a
/// Turso draw to the full `BackendEngine`:
///
/// - **strip `Actor::UI`** — the composed `CapMap` is headless by construction
///   (`compose_sut` fail-louds on a UI actor; a window is the sibling
///   gpui-thread harness's job, Design §8.10);
/// - **force `StorageAdapter::Loro` when Turso is absent** — the native
///   selector maps every non-Turso wiring onto the LoroMemory backend, and
///   `compose_sut` requires ≥1 of Loro/Turso;
/// - **select `ViewModel` only with Turso** (`compose_sut` asserts
///   `!has_frontend || has_turso`); always select `EditorState`.
///
/// Idempotent: an already-normalized wiring maps to itself, so
/// `set_for_wiring(&full_headless().wiring) == full_headless()`.
/// One-line disclosure of the RESOLVED execution-authority ROUTING for a drawn
/// wiring — the map the wiring draw ALONE does not reveal. The failure header
/// prints the wiring (`storage={Turso} … projections={…, EditorState}`), which
/// misled readers (and a prior task framing) into inferring Turso EXECUTES the
/// runtime writes. It does not: the `EditorState` projection is exactly what
/// turns Loro on as the block-CRUD authority (`crud.enabled` ⇒
/// `LoroBlockOperations`), so runtime `create`/`set_field` land in the Loro doc
/// and only PROJECT to `block_raw`/matview. This line makes that recoverable
/// without a code trace.
///
/// SINGLE-SOURCED from [`set_for_wiring`] — the SAME resolution
/// `boot_and_seed_wide`/`compose_sut` use — never a parallel truth table (which
/// could drift from the real wiring decision). Precedent: commit 8cdef3bc
/// (failure header discloses layer COVERAGE, never asserts unmeasured green).
pub fn authority_routing_disclosure(wiring: &Wiring) -> String {
    let set = set_for_wiring(wiring);
    // `has_editor` in `builder.rs` is exactly `set.has_projection(EditorState)`,
    // and that flag alone drives `loro_enabled`/`crud.enabled`. Read it here so
    // this disclosure cannot disagree with the actual block-CRUD wiring.
    let block_crud = if set.has_projection(Projection::EditorState) {
        "Loro(LoroBlockOperations, via EditorState)"
    } else {
        "Sql(SqlOperationProvider)"
    };
    // Turso storage is the SQL projection sink (block_raw + matviews). A
    // Loro-only draw has no SQL sink; block_raw is then absent.
    let projection_sinks = if set.wiring.has_storage(StorageAdapter::Turso) {
        "Sql(block_raw,matview)"
    } else {
        "none(Loro-only, no Sql sink)"
    };
    // Org write-back (block → `.org` file) is active only when an Org adapter is
    // wired into the draw.
    let org_writeback = if wiring.has_storage(StorageAdapter::Org) {
        "on"
    } else {
        "off"
    };
    format!(
        "authority: block-CRUD={block_crud}; projection-sinks={projection_sinks}; \
         org-writeback={org_writeback}"
    )
}

/// The floor the SHIPPED-DEFAULT (SQL block-CRUD authority) share of accepted
/// keystone draws must clear — see `sql_crud_authority_draw_share_meets_floor`.
/// Production defaults `crdt.enabled` to false, so a keystone that resolves
/// every draw to the Loro CRUD authority tests a mode nobody ships. The value
/// is a floor on the LIVE draw, not a target: the SQL-CRUD share is
/// `P(Turso ∧ ¬Loro)`, measured at 0.220 over 4096 deterministic draws, so this
/// line leaves headroom for generator tuning while still catching a collapse
/// back toward zero.
pub const MIN_SQL_CRUD_DRAW_SHARE: f64 = 1.0 / 8.0;

/// The floor on the share of GENERATED SqlOnly cases that must commit at least
/// one typed edit into block content — see
/// `sqlonly_generated_cases_commit_typed_edits_into_block_content`.
/// [`MIN_SQL_CRUD_DRAW_SHARE`] only proves the shipped-default mode is drawn;
/// this proves the mode can be TYPED in, which is the coverage both the
/// post-split sibling-order and the stale-marks prod bugs escaped through.
///
/// A COLLAPSE DETECTOR, not a target — set well below the measurement so
/// binomial noise cannot flake it. Measured over 64 cases × 40 steps: SqlOnly
/// 0.109 against a Loro control of 0.094, i.e. the two modes are at parity and
/// ~10% is simply what the SHARED generator offers (a case must draw
/// `FocusEditableText` and then `TypeChars` before anything blurs the editor).
/// Raising the rate is a generator question, not a mode question; this floor
/// only catches a return to the zero SqlOnly had.
pub const MIN_SQLONLY_TYPING_CASE_SHARE: f64 = 1.0 / 32.0;

/// Resolve a drawn manifest into the `ComponentSet` the composed SUT boots.
///
/// `Projection::EditorState` is selected IFF the resolved manifest wires
/// [`StorageAdapter::Loro`]: `compose_sut` passes that flag straight through as
/// `crdt.enabled`, so it decides whether `LoroBlockOperations` or the fallback
/// `SqlOperationProvider` is the block-CRUD authority. Tying it to the Loro
/// adapter keeps the manifest honest (no Loro adapter ⇒ no Loro authority) and
/// makes the SHIPPED DEFAULT — `crdt.enabled = false` — a drawable mode.
pub fn set_for_wiring(wiring: &Wiring) -> ComponentSet {
    let mut wiring = wiring.clone();
    wiring.actors.remove(&Actor::UI);
    if !wiring.has_storage(StorageAdapter::Turso) {
        wiring.storage_adapters.insert(StorageAdapter::Loro);
    }
    let mut projections: BTreeSet<Projection> = BTreeSet::new();
    if wiring.has_storage(StorageAdapter::Loro) {
        projections.insert(Projection::EditorState);
    }
    if wiring.has_storage(StorageAdapter::Turso) {
        projections.insert(Projection::ViewModel);
    }
    ComponentSet::new(wiring, projections)
}

/// The composed cap set for a (normalized) `wiring`, computed ONCE per distinct
/// `ComponentSet` by booting `compose_sut(set_for_wiring(wiring))` on a
/// throwaway current-thread runtime and extracting the (runtime-free) `CapSet`.
/// The swap ref carries this so `aggregate_transitions` auto-narrows the
/// production alphabet to exactly what THIS composed SUT can drive. Cached by
/// `Wiring` (linear scan — `Wiring` is `Eq` but not `Hash`/`Ord`, and the draw
/// set is tiny).
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

/// The `full_headless` cap set — the WIDEST wiring's cap set (used by the
/// cap-presence guard and the FORCE_FULL pin). Thin alias over
/// [`cap_set_for_wiring`].
pub fn full_headless_cap_set() -> CapSet {
    cap_set_for_wiring(&ComponentSet::full_headless().wiring)
}

/// The swap oracle for a (normalized) `wiring`: the seeded wide tree, re-wired
/// to that wiring and carrying its composed cap_set, so `aggregate_transitions`
/// gates the alphabet to exactly the caps THIS composed SUT provides. The
/// ref-side **subsystem** wiring stays `wide_ref()`'s `{Loro, EditorState}` for
/// every draw (the editor/focus transitions gate on it; the SUT-side cap_set,
/// not the ref subsystems, is what narrows per wiring) — so a Loro-only draw
/// reuses the same oracle tree with a narrower cap_set.
///
/// NO deliberate narrowing remains: the swap drives the FULL production
/// `aggregate_transitions` alphabet auto-narrowed by the real cap set.
/// - `SutMutate` → `ToggleState`: un-narrowed task #4 (Loro read-doc unify +
///   real `cycle_task_state` toggle).
/// - `SutWatchRegister` → `SetupWatch`/`RemoveWatch`: un-narrowed task #5 — the
///   watch invariant seed-excludes both sides, so `inv-watch-rows-match-ref`
///   compares only the non-seed working tree.
pub fn wide_e2e_ref_for(wiring: &Wiring) -> ReferenceState {
    let set = set_for_wiring(wiring);
    let mut state = wide_ref();
    state.harness.wiring = set.wiring.clone();
    // Forward-edge ingest corpus (dogfood 2026-07-10 P0): a Turso-ingest-only
    // regression, so seed it ONLY for a frontend draw. `boot_and_seed_wide` keys
    // the `forward-edge-page.org` seed on this corpus being present in the
    // oracle, so a Loro-only draw (no `ViewModel` projection) never carries it
    // and the `inv-blocks-match-ref/{block_raw,matview}` arms deselect there
    // (no `SutSqlProjection`).
    if set.has_projection(Projection::ViewModel) {
        seed_forward_edge_corpus(&mut state);
        // Companion page-tag demotion closure (dogfood 2026-07-12): a top-level
        // page-file (`2026-07-10.org`) whose `Page` doc-root is inlined as a plain
        // heading in the `Journals.org` companion. Frontend-only (a Turso org-
        // ingest topology); `boot_and_seed_wide` keys the file seed on this page
        // being in the ref.
        //
        // ENV-gated OFF by default (`folder_companion_enabled`): the
        // page-authority FIX (foreign-page protection) is required for a clean
        // boot, but POST-fix the companion's inlined heading is intentionally
        // lossy on org round-trip (the heading belongs to the page-file, so
        // `Journals.org` no longer renders it) — an `inv-org-render-fixed-point`
        // divergence whose oracle is Fork B's writeback work. Promote to
        // always-on-for-frontend once Fork B's companion-writeback oracle lands.
        // The dedicated deterministic boot test seeds this directly (env-
        // independent) and asserts only the ingest/tag invariants.
        if folder_companion_enabled() {
            seed_folder_companion(&mut state);
        }
        // Journals boot auto-create closure (dogfood #4): a frontend (Turso)
        // boot fires the seeded rule once for the fixed keystone clock day,
        // minting ONE journal day-block under `block:journals`. Model it as a
        // non-seed child of EVERY frontend draw.
        //
        // Prod evidence (2026-07-16): the rule-firing machinery is NOT gated on
        // the `Actor::ActionEngine` PBT label — the `ClockScheduler`
        // (`registration.rs`: `spawn_clock_scheduler` in `create_initialized_engine`,
        // "Every embedder resolves through this shared path") and
        // `start_action_watchers` (`holon-app/wiring.rs`, only `#[cfg(not(wasm32))]`)
        // BOTH run unconditionally on every non-wasm Turso frontend boot, and prod
        // has zero imports of the `Actor` enum. The SUT fires the boot journal on
        // any Turso draw, so the prior `has_actor(ActionEngine)` gate here was too
        // narrow: a `storage={Turso} actors={}` draw fired it in the SUT but the
        // oracle did not model it → a spurious `inv-blocks-match-ref/{org,block_raw,
        // matview}` divergence (SUT +1 block). The actor stays a label describing
        // which transitions a draw can express, not a prod gate. `seed_boot_journal`
        // is a no-op when no ViewModel booted (a non-frontend draw takes the `else`),
        // so the ViewModel gate alone is the right scope.
        seed_boot_journal(&mut state);
    } else {
        // Non-frontend (Loro/storage-only) draw: the journal auto-create RULE
        // (`Journal Auto-Create` heading + `holon_rule` action) is seeded ONLY by
        // a frontend boot's `build_default_layout_blocks` — a storage-only config
        // has no frontend session and never mints it. `seed_booted_layout_into_ref`
        // (run via `wide_ref()` under the fixed `{Loro, EditorState}` subsystem
        // wiring) models it unconditionally, which for a non-frontend draw made the
        // oracle expect a block the SUT structurally cannot have: a false
        // `inv-blocks-match-ref` INGEST-DATA-LOSS RED at boot AND — once that no
        // longer aborts — a `peer_update_block: block not found` panic when a
        // `PeerEdit` transition targets the ref-modeled-but-SUT-absent rule block.
        // Drop it from the oracle here (mirroring how the forward-edge corpus /
        // boot journal are added ONLY for a frontend draw), so a non-frontend draw
        // neither compares nor targets it. The page-display layer
        // (`journals`/`src::0`/`render::0`) stays modeled — it is scaffold-filtered
        // and never a mutation/peer target.
        for id in [
            holon_frontend::JOURNALS_AUTO_CREATE_ID,
            holon_frontend::JOURNALS_ACTION_ID,
        ] {
            let uri = EntityUri::parse(id).expect("journals rule id");
            state.domain.block_state.blocks.remove(&uri);
            state.domain.block_state.block_documents.remove(&uri);
            state.domain.layout_blocks.headline_ids.remove(&uri);
        }
    }
    state.with_cap_set(cap_set_for_wiring(wiring))
}

/// The swap oracle for the WIDEST wiring (`full_headless`) — the
/// `HOLON_PBT_FORCE_FULL` pin and the teeth's fixed target. Thin alias over
/// [`wide_e2e_ref_for`].
pub fn wide_e2e_ref() -> ReferenceState {
    wide_e2e_ref_for(&ComponentSet::full_headless().wiring)
}

/// Re-wire a caller-built oracle to the `full_headless` (frontend) wiring
/// WITHOUT attaching a cap_set — the runtime-free half of [`wide_e2e_ref`].
///
/// `boot_and_seed_wide` reads ONLY `ref_state.wiring` (via [`set_for_wiring`])
/// to pick the SUT's `ComponentSet`, so a Loro-only-wired oracle
/// (`structural_ref`/`wide_ref`) yields a Loro-thin SUT that is missing the
/// frontend caps (`SutBlockTreeWrite`, `SutFocusWrite`, `SutNavHistoryDrive`,
/// `SutAppLifecycle`, …) the teeth's transitions select — the "selected but
/// absent from the CapMap" panic. This override gives the oracle the same
/// full_headless wiring `wide_e2e_ref` carries, so the SUT boots the full
/// frontend cap map.
///
/// Unlike [`wide_e2e_ref`], it does NOT call `cap_set_for_wiring` (which boots
/// its OWN runtime to extract the cap_set) so it is safe to call from INSIDE a
/// `#[tokio::test]` (no "runtime within a runtime" panic). The teeth drive
/// transitions by hand and never generate, so the cap_set — a
/// generator-narrowing hint — is irrelevant to them.
pub fn frontend_wired(mut state: ReferenceState) -> ReferenceState {
    state.harness.wiring = ComponentSet::full_headless().wiring.clone();
    // Journals boot auto-create closure (dogfood #4): `frontend_wired` pins the
    // `full_headless` (Turso frontend) wiring, and `boot_and_seed_wide` fires the
    // seeded daily-journal rule once on EVERY frontend boot (ClockScheduler +
    // action watchers run unconditionally on any non-wasm Turso boot), minting one
    // journal day-block under `block:journals` and keeping its
    // `keystone_boot_journal_id` COMPARED (not scaffold-filtered, see the `tree`
    // set there). So the oracle must model it too — the SAME seed
    // `wide_e2e_ref_for` applies for a frontend draw. Without it the SUT-fired
    // day-block is a spurious `+1` block that false-diverges
    // `inv-blocks-match-ref/{org,loro,block_raw, matview}` and surfaces as an
    // `inv-history-no-phantom-rows` phantom (its id is unknown to the ref
    // universe). The hand-driven teeth build their oracle through this helper
    // rather than `wide_e2e_ref_for`, so seed it here for symmetry.
    seed_boot_journal(&mut state);
    state
}

/// The WINDOWED swap oracle: the same wide tree/wiring as [`wide_e2e_ref`], but
/// carrying the LIVE windowed SUT's cap set (read off the assembled SUT via
/// [`ComposedSut::cap_set`](crate::pbt::composed::harness::ComposedSut::cap_set) after
/// `overlay_windowed_caps`). `wide_e2e_ref`'s `full_headless_cap_set()` lacks
/// the window caps (`SutLayout`/`SutDriver`/…), so gesture transitions like
/// `ClickBlock` would deselect/misbehave under it; the live set admits exactly
/// what the window drives — including `SutFocusWrite`, which is faithfully
/// present (NO `.without()` subtraction: absence-faking a real cap is the
/// invalid-intermediate-state anti-pattern).
pub fn wide_e2e_windowed_ref(cap_set: CapSet) -> ReferenceState {
    wide_e2e_ref().with_cap_set(cap_set)
}

/// Reference machine over the production `E2ETransition`, generated by the FULL
/// production `aggregate_transitions` (auto-narrowed by the ref's wiring +
/// cap_set).
pub struct WideE2EMachine;

impl ReferenceStateMachine for WideE2EMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        // Draw the FULL valid-wiring space (shrinking toward Loro-only — the cheap
        // minimal backend) and build the per-wiring oracle. `wide_e2e_ref_for`
        // does a `block_on` for cap-set extraction, valid here because proptest
        // calls `init_state` in a sync context with no ambient runtime (see
        // `loro_only_wide_seed_runs_block_invariants_green`). The per-draw
        // non-vacuity floor (`required_invariants`) keeps a Loro-only draw from
        // false-REDing on the SQL/ViewModel ids it has no caps for.
        // `HOLON_PBT_FORCE_FULL=1` pins every draw to `full_headless` — the
        // deterministic exerciser for the frontend-only composed arms
        // (`ApplyMutation` External / `BulkExternalAdd`). NOT actually rare by
        // default: the validity filter (`Wiring::validate` rejects
        // empty-storage and ActionEngine-without-Turso draws) reweights the raw
        // 0.20 Turso inclusion to ≈0.39 of VALID draws, so a 16-case run misses
        // Turso entirely with probability well under 0.1%. `HOLON_PBT_PIN_WIRING="
        // storage;sync;actors"` pins every draw to ONE exact
        // manifest (fail-loud on a typo or invalid manifest) — the external-supply seam
        // for bottom-up ladder runs and subset-wiring repros. Mutually exclusive with
        // FORCE_FULL to keep a run's provenance unambiguous.
        if let Ok(spec) = std::env::var("HOLON_PBT_PIN_WIRING") {
            assert!(
                std::env::var("HOLON_PBT_FORCE_FULL").is_err(),
                "HOLON_PBT_PIN_WIRING and HOLON_PBT_FORCE_FULL are mutually exclusive"
            );
            let wiring = holon_pbt_core::wiring_from_exact_spec(&spec);
            return ::proptest::strategy::Strategy::boxed(::proptest::prelude::Just(
                wide_e2e_ref_for(&wiring),
            ));
        }
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
        // The alphabet gate comes FIRST, not just the ref-side precondition.
        // `init_state` draws the wiring, so the library shrinks it — and
        // `preconditions` is the only filter it re-applies to already-drawn
        // transitions against the shrunk state (transition value-trees are built
        // from the ORIGINAL state and can never be regenerated). Without the gate
        // a shrink toward the cheap Loro-only backend keeps a transition the
        // shrunk CapMap has no provider for, and the run dies in `CapMap::expect`
        // instead of shrinking the divergence it was chasing.
        crate::pbt::stepper::transition_applicable(state, transition)
            && transition.preconditions(state).is_good()
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        transition.apply_to_ref(&mut state);
        state.action.last_transition_kind = Some(transition.variant_name());
        state
    }
}

/// The swap slice: production `E2ETransition` enum, production
/// `aggregate_transitions` generator, composed
/// `compose_sut(set_for_wiring(drawn wiring))` SUT (via [`boot_and_seed_wide`])
/// -- one SUT per drawn point of the wiring grid.
pub struct WideE2E;

impl ComposedSlice for WideE2E {
    type Transition = E2ETransition;
    type Machine = WideE2EMachine;
    // My change (settle): the per-apply convergence settle needs the store handles.
    type Handle = WideHandle;
    // Main's change (uvuvnwnn): the per-draw `required_invariants` override below
    // supersedes the static list, deriving the floor from the WHOLE shared
    // catalog (every invariant this draw's caps select). The cap-level
    // `wide_cap_presence_guard` proves the widest wiring selects the whole
    // catalog; there is no per-id list to maintain (`WIDE_REQUIRED_INVARIANTS`
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

    /// Replace the flat post-apply `sleep(SETTLE)` with the 3-projection
    /// convergence settle — the CDC-only lever under-settled (Loro/org
    /// lagged and the block/org invariants diverged). Capped at `SETTLE`,
    /// so it never over-waits vs the old sleep.
    async fn settle_after_apply(handle: &WideHandle, _: &CapMap) {
        converge_projections(handle, crate::pbt::composed::soak_seed::soak_settle()).await;
    }

    /// The mask's alphabet is the transition enum's own variant names, not a
    /// string derived from `Debug` — so `HOLON_PBT_SCHED_KINDS=TypeChars` names
    /// exactly `E2ETransition::TypeChars` and a typo cannot silently arm
    /// nothing.
    fn transition_kind(t: &E2ETransition) -> String {
        t.variant_name().to_string()
    }

    /// Arm this session's engine for the fire-and-forget dispatch door, apply,
    /// disarm. Scoping the door to the transition (rather than the whole run)
    /// is what makes a red attributable to the masked kind: every other
    /// transition in the sequence still runs the awaiting door it always did.
    ///
    /// A Loro-only draw has no `ReactiveEngine`, so it has no fire-and-forget
    /// door and simply runs the ordinary apply — its armed run then reports an
    /// unobserved overlap rather than a false one.
    async fn apply_transition_detached(
        transition: &E2ETransition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
        handle: &WideHandle,
    ) {
        let engine = handle.reactive();
        if let Some(e) = &engine {
            e.ui_state().set_detached_dispatch(true);
        }
        Self::apply_transition(transition, ref_state, caps).await;
        if let Some(e) = &engine {
            e.ui_state().set_detached_dispatch(false);
        }
    }

    /// Wait for a real completion boundary: an intent settling in the dispatch
    /// journal, the Turso CDC watermark advancing, or every projection reaching
    /// a fixed point.
    ///
    /// The pre-check decides `Unobservable` BEFORE waiting, so reaching the
    /// deadline means work that should have crossed the boundary did not —
    /// reported as a wedge, never degraded.
    async fn await_boundary(
        handle: &WideHandle,
        boundary: Boundary,
        window: BoundaryWindow,
        deadline: Duration,
    ) -> BoundaryOutcome {
        let journal = Self::dispatch_journal(handle);
        let in_flight = || match (journal.as_ref(), window.journal_mark) {
            (Some(j), Some(m)) => settled_and_pending(j, m),
            _ => (0, 0),
        };
        let started = std::time::Instant::now();

        if let Boundary::AfterQuiescence = boundary {
            converge_projections(handle, deadline).await;
            return BoundaryOutcome::Observed(BoundaryEvidence {
                boundary,
                waited: started.elapsed(),
                detail: "projections reached a combined fixed point".to_string(),
            });
        }

        let (settled_at_entry, pending_at_entry) = in_flight();
        let engine = handle.engine();
        let unobservable = match boundary {
            Boundary::AfterIntents(n) if pending_at_entry < n as usize => Some(format!(
                "{pending_at_entry} intent(s) in flight, so {n} completion(s) can never be observed"
            )),
            Boundary::AfterCdcBatch if engine.is_none() => {
                Some("draw booted no Turso engine".to_string())
            }
            Boundary::AfterCdcBatch if pending_at_entry == 0 => {
                Some("no intent in flight to drive a projection".to_string())
            }
            _ => None,
        };
        if let Some(reason) = unobservable {
            return BoundaryOutcome::Unobservable(reason);
        }

        let cdc_at_entry = engine.map(|e| e.db_handle().cdc_emitted_watermark());
        let poll = Duration::from_millis(2);
        loop {
            let crossed = match boundary {
                Boundary::AfterIntents(n) => {
                    let (settled, _) = in_flight();
                    (settled >= settled_at_entry + n as usize)
                        .then(|| format!("settled {settled_at_entry} -> {settled}"))
                }
                Boundary::AfterCdcBatch => {
                    let now = engine
                        .expect("pre-check kept the engine")
                        .db_handle()
                        .cdc_emitted_watermark();
                    (Some(now) != cdc_at_entry)
                        .then(|| format!("cdc watermark {:?} -> {now}", cdc_at_entry))
                }
                Boundary::AfterQuiescence => unreachable!("handled before the pre-check"),
            };
            if let Some(detail) = crossed {
                return BoundaryOutcome::Observed(BoundaryEvidence {
                    boundary,
                    waited: started.elapsed(),
                    detail,
                });
            }
            if started.elapsed() >= deadline {
                let (_, pending) = in_flight();
                return BoundaryOutcome::TimedOutWithPendingWork {
                    pending,
                    waited: started.elapsed(),
                };
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// The overlap probe's source: the engine's own record of what it
    /// dispatched and what has not settled yet.
    fn dispatch_journal(
        handle: &WideHandle,
    ) -> Option<Arc<holon_frontend::dispatch_journal::DispatchJournal>> {
        handle.reactive().map(|r| r.ui_state().dispatch_journal())
    }

    /// Clear the frontend component's per-tick `widget_tree_snapshot` memo
    /// before the next mutation, so the ~12 snapshot-reading ViewModel/render
    /// invariants share ONE recompute per settle tick without ever observing a
    /// pre-mutation frame. No-op for a Loro-only (non-frontend) draw.
    fn invalidate_render_caches(handle: &WideHandle) {
        if let Some(comp) = &handle.frontend {
            comp.invalidate_render_cache();
        }
    }

    async fn apply_transition(
        transition: &E2ETransition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
    ) {
        // Reset the span collector + record the wall/RSS baseline for THIS transition,
        // before its SQL runs — so `inv-sql-budget` measures the transition, not the
        // accumulation of every prior tick. (`freeze_for_check` snapshots at check
        // time.)
        #[cfg(feature = "otel-testing")]
        if let Some(m) = caps.get::<dyn crate::pbt::composed::span_metrics::SutMetricsLifecycle>() {
            m.note_transition_start(transition);
        }
        // Reseed-attribution pin (Inc 0): mark the observer steady (post-seed) and
        // attribute every full-reseed event fired during this transition's apply +
        // settle to its label. Boot/seed projection events precede the first call
        // and stay tagged non-steady (legitimate `coldboot`, not a leak).
        #[cfg(feature = "otel-testing")]
        crate::pbt::composed::reseed_observer::ReseedObserver::global()
            .note_transition(&format!("{transition:?}"));
        TransitionImpl::apply_to_sut(transition, ref_state, caps).await;
    }

    /// Per-draw non-vacuity floor: every invariant in the WHOLE shared catalog
    /// that THIS draw's caps can actually select MUST run. The SUT axis is
    /// the drawn wiring's cap_set (already carried on the ref by
    /// [`wide_e2e_ref_for`]); the ref axis registers every ref
    /// cap unconditionally (see `impl CapProvider for ReferenceState`). A
    /// Loro-only draw thus drops the SQL/ViewModel/focus ids it has no caps
    /// for, while a `full_headless` draw keeps every headless-selectable
    /// catalog invariant. Selection here uses the SAME
    /// `Needs::selected_against` the runner uses, computed against the wiring's
    /// EXPECTED cap_set (not the actual booted caps), so the floor has
    /// teeth: if the wiring claims a cap the boot fails to wire, the
    /// invariant is required-but-deselected and the floor REDs.
    ///
    /// This is the runtime complement to the static `wide_cap_presence_guard`:
    /// that guard proves the WIDEST wiring's CapMap provides every `Needs`
    /// cap (so the widest config selects the whole catalog); this floor
    /// proves each per-draw wiring actually RUNS every invariant it
    /// selects. Neither needs a hand-maintained per-invariant-id list.
    fn required_invariants(ref_state: &ReferenceState) -> Vec<InvariantId> {
        let sut_caps = ref_state
            .harness
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

/// The narrowed LIVE windowed cap set, captured once (by a throwaway windowed
/// boot at the top of a windowed random runner) before the proptest strategy is
/// built. [`WideE2EWindowedMachine::init_state`] reads it so the generated
/// alphabet + the non-vacuity floor narrow to exactly what the window can
/// drive. Hoisted here (increment 4c) so the gpui loop and the tui composed
/// runner share ONE machine.
static WINDOWED_CAP_SET: std::sync::OnceLock<CapSet> = std::sync::OnceLock::new();

/// Capture the live windowed cap set (once per process). Panics on a second
/// call — a runner must capture it exactly once, before building the strategy.
pub fn set_windowed_cap_set(cap_set: CapSet) {
    WINDOWED_CAP_SET
        .set(cap_set)
        .expect("WINDOWED_CAP_SET set once");
}

/// Narrow a live windowed cap set to the windowed GENERATED alphabet.
///
/// The deferred windowed base is `full_headless` (a
/// `HeadlessFrontendComponent`), which still hosts the 6 EXCLUDED-row
/// nav/history/view caps at the Direct-dispatch rung — but no window-driver
/// mechanism drives them yet (C-3 Rung Audit rows 19–24, tracked Phase 3
/// blockers). Driving them through the leftover dispatch impl while a window
/// exists would be an unfaithful cross-rung combination (Design §8.11), so they
/// must NOT enter the windowed generated alphabet. `CapSet::without` is the
/// sanctioned, DISCLOSED narrowing: the caps stay in the `CapMap` (their read
/// invariants keep selecting), only the generation gate drops their
/// transitions. This is NOT the fix-the-cap-not-withhold anti-pattern (that
/// forbids faking a DIVERGENCE green) — it is the audit-prescribed exclusion of
/// a genuinely-undriveable transition class.
///
/// Cap → EXCLUDED transition rows:
/// - `SutNavHistoryWrite`  → NavigateHome (row 19)
/// - `SutNavHistoryDrive`  → NavigateBack/Forward, PinBlock, UnpinBlock (rows
///   20–22)
/// - `SutViewControl`      → SwitchView (row 23)
/// - `SutHistoryWrite`     → UndoLastMutation/Redo (row 24)
pub fn narrow_to_windowed_alphabet(cap_set: CapSet) -> CapSet {
    use holon_pbt_core::capabilities::SutHistoryWrite;
    use holon_pbt_core::capabilities::SutNavHistoryDrive;
    use holon_pbt_core::capabilities::SutNavHistoryWrite;
    use holon_pbt_core::capabilities::SutViewControl;
    use holon_pbt_core::composition::CapId;
    cap_set
        .without(&CapId::of::<dyn SutNavHistoryWrite>())
        .without(&CapId::of::<dyn SutNavHistoryDrive>())
        .without(&CapId::of::<dyn SutViewControl>())
        .without(&CapId::of::<dyn SutHistoryWrite>())
}

/// Report which of the 6 EXCLUDED-row caps the LIVE windowed base actually
/// carries, so the narrowing is disclosed against reality (not assumed).
pub fn disclose_excluded(cap_set: &CapSet) {
    use holon_pbt_core::capabilities::SutHistoryWrite;
    use holon_pbt_core::capabilities::SutNavHistoryDrive;
    use holon_pbt_core::capabilities::SutNavHistoryWrite;
    use holon_pbt_core::capabilities::SutViewControl;
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
            "[windowed-alphabet] EXCLUDED cap present-in-base={present}: {name} (narrowed out of \
             generation)"
        );
    }
}

/// The windowed sibling of [`WideE2EMachine`]: identical transition generation
/// / preconditions / apply (delegated), but `init_state` FIXES the oracle to
/// the narrowed live windowed cap set ([`set_windowed_cap_set`]) instead of
/// drawing `any_valid_wiring()`. That cap set auto-narrows
/// `aggregate_transitions` to the windowed alphabet (the REBIND/OK gesture
/// rows) and drops the EXCLUDED rows, and it is the same set the per-tick
/// `check_invariants` non-vacuity floor (`required_invariants`) is
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

// ── F11: wiring-draw distribution floor ──────────────────────────────────
//
// A generator regression that silently stops drawing an arm (e.g. the rare,
// low-probability Turso query-adapter at `QUERY_ADAPTER_INCLUSION_PROB`, or an
// actor) would make an entire invariant family (SQL projections, ActionEngine
// advice, …) run ZERO times over a keystone run and still pass GREEN. The floor
// makes that loud: over N draws of the SAME strategy the keystone uses
// (`any_valid_wiring()` over `wiring_axes()`), EVERY component in the drawable
// universe must be drawn at least once.
//
// Modeled on the F2 engagement floor: a pure, unit-testable decision
// (`wiring_draw_floor_violations`) plus a deterministic sampling test that is
// the actual regression guard ("over N cases"). Unlike the per-sequence
// engagement floor, the wiring floor is cross-draw by nature — each sequence
// draws exactly ONE wiring, so a per-sequence teardown hook could never observe
// "all arms drawn". Its teeth therefore live in the sampling test, not a
// teardown assertion. Per-case wiring is separately disclosed live via the
// `[wide-e2e wiring]` line, so a real run log still yields the observed
// distribution; this floor asserts the strategy can produce every arm.

/// One drawable wiring component across the three axes — the granularity the
/// distribution floor requires to be non-empty over a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WiringComponent {
    Storage(StorageAdapter),
    Sync(holon_pbt_core::SyncAdapter),
    Actor(Actor),
}

/// The drawable component universe for the CURRENT axes (`wiring_axes()`,
/// env-overridable). The floor requires each of these to be drawn ≥1× over a
/// run.
pub fn drawable_wiring_universe() -> BTreeSet<WiringComponent> {
    let (storage, sync, actors) = holon_pbt_core::wiring_axes();
    storage
        .into_iter()
        .map(WiringComponent::Storage)
        .chain(sync.into_iter().map(WiringComponent::Sync))
        .chain(actors.into_iter().map(WiringComponent::Actor))
        .collect()
}

/// The components a single drawn wiring exercises.
pub fn wiring_components(w: &Wiring) -> BTreeSet<WiringComponent> {
    w.storage_adapters
        .iter()
        .copied()
        .map(WiringComponent::Storage)
        .chain(w.sync_adapters.iter().copied().map(WiringComponent::Sync))
        .chain(w.actors.iter().copied().map(WiringComponent::Actor))
        .collect()
}

/// Pure floor decision: components in the drawable `universe` that never
/// appeared across the accumulated `seen` draws. Non-empty ⇒ an arm was never
/// drawn ⇒ its invariant family ran zero times (vacuity by omission).
pub fn wiring_draw_floor_violations(
    seen: &BTreeSet<WiringComponent>,
    universe: &BTreeSet<WiringComponent>,
) -> Vec<WiringComponent> {
    universe.difference(seen).copied().collect()
}

#[cfg(test)]
mod tests {
    use holon_pbt_core::capabilities::PeerEditOp;

    use super::*;
    use crate::pbt::transitions::AddPeer;
    use crate::pbt::transitions::CreateDocument;
    use crate::pbt::transitions::MergeFromPeer;
    use crate::pbt::transitions::PeerEdit;
    use crate::pbt::transitions::SyncWithPeer;

    /// F11 pure-floor: a component present in the drawable universe but never
    /// seen across the accumulated draws MUST be reported as a violation — this
    /// is the "a generator regression skewed an arm to zero" signal. Mirrors
    /// the F2 `engagement_floor_violations` unit test.
    #[test]
    fn wiring_draw_floor_flags_never_drawn_arm() {
        let universe: BTreeSet<WiringComponent> = [
            WiringComponent::Storage(StorageAdapter::Loro),
            WiringComponent::Storage(StorageAdapter::Turso),
            WiringComponent::Actor(Actor::ActionEngine),
        ]
        .into_iter()
        .collect();
        // Turso was never drawn (the regression the floor exists to catch).
        let seen: BTreeSet<WiringComponent> = [
            WiringComponent::Storage(StorageAdapter::Loro),
            WiringComponent::Actor(Actor::ActionEngine),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            wiring_draw_floor_violations(&seen, &universe),
            vec![WiringComponent::Storage(StorageAdapter::Turso)]
        );
    }

    /// F11 pure-floor: full coverage owes no violation.
    #[test]
    fn wiring_draw_floor_passes_when_every_arm_drawn() {
        let universe = drawable_wiring_universe();
        assert!(
            wiring_draw_floor_violations(&universe, &universe).is_empty(),
            "a seen-set equal to the universe must satisfy the floor"
        );
    }

    /// F11 live guard ("over N cases"): the ACTUAL keystone strategy
    /// (`any_valid_wiring()` over the current `wiring_axes()`) must be able to
    /// draw every component in its drawable universe over N draws. A generator
    /// regression that skews an arm (e.g. the rare
    /// `QUERY_ADAPTER_INCLUSION_PROB` Turso arm) to zero fails HERE instead
    /// of silently vacating that arm's invariant family in the keystone.
    /// Deterministic (fixed-seed runner).
    #[test]
    fn any_valid_wiring_draws_every_arm_over_n() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::Config;
        use proptest::test_runner::TestRunner;

        let strategy = holon_pbt_core::any_valid_wiring();
        let mut runner = TestRunner::new(Config {
            cases: 1,
            ..Config::default()
        });
        let mut seen: BTreeSet<WiringComponent> = BTreeSet::new();
        // 2048 draws: at Turso's 0.20 inclusion prob the rarest arm still lands
        // hundreds of times — the floor is deterministic, not flaky.
        for _ in 0..2048 {
            if let Ok(tree) = strategy.new_tree(&mut runner) {
                seen.extend(wiring_components(&tree.current()));
            }
        }
        let universe = drawable_wiring_universe();
        let missing = wiring_draw_floor_violations(&seen, &universe);
        assert!(
            missing.is_empty(),
            "wiring-draw floor: components in the drawable universe were NEVER drawn over 2048 \
             draws of `any_valid_wiring()` — the generator skews them to zero, which would \
             silence their invariant family in the keystone: {missing:?} (universe: {universe:?}, \
             seen: {seen:?})"
        );
    }

    /// Seed-generalization validation (the §8.10 next-step gate): a
    /// **Loro-only** wide draw (no Turso ⇒ no frontend) boots EMPTY through
    /// the builder's non-frontend arm, so `boot_and_seed_wide` must seed
    /// the working tree directly into the canonical Loro backend. This
    /// proves the block-comparison invariants RUN and are GREEN over the
    /// seeded Loro SUT — parent/c1/c2 match the oracle AND the oracle-modeled
    /// boot layout (`block:journals` + index.org) is filtered via the
    /// ref∪booted scaffold union, not falsely diverging. Without the seed
    /// this would deselect/false-RED; this is the gate for letting
    /// `init_state` draw non-frontend wirings.
    #[test]
    fn loro_only_wide_seed_runs_block_invariants_green() {
        // Build the ref OUTSIDE any ambient runtime (it does a `block_on` for cap-set
        // extraction — mirrors proptest's sync `init_state`), then drive the async boot
        // + catalog run on a manually-built multi-thread runtime (mirrors
        // `init_test`).
        let wiring = Wiring::custom(vec![StorageAdapter::Loro], vec![], vec![]);
        let ref_state = wide_e2e_ref_for(&wiring);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build multi-thread runtime");
        rt.block_on(async {
            let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
            let (caps, _handle, scaffold) = boot_and_seed_wide(&resolver, &ref_state).await;
            let report = <WideE2E as ComposedSlice>::run_report(
                &caps,
                &resolver,
                &BurnedPairs::new(),
                &std::collections::BTreeSet::new(),
                &scaffold,
                &ref_state,
            )
            .await;
            assert!(
                report.failures().is_empty(),
                "Loro-only wide seed must run the catalog green; failures: {:?}",
                report.failures()
            );
            let ran = report.ran_ids();
            assert!(
                ran.contains(&"inv-blocks-match-ref/block_raw"),
                "the block-id comparison must RUN over the seeded Loro SUT (non-vacuity proof the \
                 seed landed); ran: {ran:?}"
            );
        });
    }

    /// SHRINK-SAFETY: `preconditions` is the ONLY filter proptest-state-machine
    /// re-applies when it shrinks the initial state, so it must reproduce the
    /// alphabet gate `aggregate_transitions` applies at generation time
    /// (`required_wiring` + `required_caps`). Without that, shrinking the drawn
    /// wiring toward the cheap Loro-only backend keeps a transition the shrunk
    /// CapMap cannot host — `CapMap::expect::<dyn SutAppLifecycle>` then aborts
    /// the whole shrink ("selected but absent from the CapMap").
    ///
    /// `CreateDocument` is the sharpest witness: its ref precondition is only
    /// `app_started`, which the composed oracle sets at seed time for EVERY
    /// wiring, so nothing else can reject it.
    #[test]
    fn shrunk_wiring_rejects_a_transition_its_capmap_cannot_host() {
        let narrow = Wiring::custom(vec![StorageAdapter::Loro], vec![], vec![]);
        let shrunk = wide_e2e_ref_for(&narrow);
        let transition = E2ETransition::CreateDocument(CreateDocument {
            file_name: "shrink-probe.org".to_string(),
        });

        assert!(
            !shrunk.caps_available(&transition.required_caps()),
            "premise: a Loro-only draw composes no frontend component, so its CapMap has no \
             SutAppLifecycle"
        );
        assert!(
            <E2ETransition as TransitionRef<ReferenceState>>::preconditions(&transition, &shrunk)
                .is_good(),
            "premise: the ref-side precondition (app_started) holds for every wiring, so the cap \
             gate is the only thing that can reject this transition"
        );

        assert!(
            !<WideE2EMachine as ReferenceStateMachine>::preconditions(&shrunk, &transition),
            "shrink acceptance must mirror the generation gate: a transition whose caps the \
             shrunk wiring cannot provide has to be rejected, otherwise the shrink boots a SUT \
             that panics in CapMap::expect"
        );
    }

    /// The same law over the WHOLE alphabet, sampled from the widest wiring's
    /// generator and re-checked against a narrower (shrunk) initial state: no
    /// sampled transition may pass `preconditions` under a wiring that gates it
    /// out. Catches a future transition that reintroduces the same escape.
    #[test]
    fn no_gated_out_transition_survives_a_shrunk_initial_state() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        let wide = wide_e2e_ref();
        let narrow = wide_e2e_ref_for(&Wiring::custom(vec![StorageAdapter::Loro], vec![], vec![]));
        let strategy = crate::pbt::transitions::aggregate_transitions(&wide);
        let mut runner = TestRunner::deterministic();
        let mut checked = 0usize;
        for _ in 0..600 {
            let transition = strategy
                .new_tree(&mut runner)
                .expect("aggregate_transitions must produce a value")
                .current();
            if narrow.caps_available(&transition.required_caps())
                && transition
                    .required_wiring()
                    .satisfied_by(&narrow.harness.wiring)
            {
                continue;
            }
            checked += 1;
            assert!(
                !<WideE2EMachine as ReferenceStateMachine>::preconditions(&narrow, &transition),
                "{} is gated out of the shrunk wiring but survives its preconditions — the \
                 shrink-abort escape",
                transition.variant_name()
            );
        }
        assert!(
            checked > 0,
            "non-vacuity: the sample must contain transitions the narrow wiring gates out"
        );
    }

    /// END-TO-END over the REAL shrink loop, with a REAL failing property — the
    /// only faithful reproduction, because a bare `simplify()` walk deletes
    /// every transition before it ever reaches the `InitialState` phase (a
    /// shrink is only kept when the case still FAILS, which needs a failing
    /// property).
    ///
    /// The stand-in for a known-red divergence is "a `{Loro, Turso}` draw whose
    /// sequence carries a lifecycle transition". That draw is the one the
    /// wiring shrinker can actually narrow past a capability: dropping Turso
    /// leaves the valid `{Loro}` manifest, whose `ComponentSet` has no frontend
    /// component and therefore no `SutAppLifecycle` — while the lifecycle
    /// transition, being what the failure needs, is retained. (A Turso-ONLY
    /// draw cannot shrink at all: removing Turso leaves zero storage adapters
    /// and `Wiring::validate` rejects it.) The body asserts what the composed
    /// runner assumes — every carried transition is hostable by the CapMap this
    /// initial state boots — so a violation surfaces as that assertion instead
    /// of as the `CapMap::expect` abort it causes in a real run.
    ///
    /// NONDETERMINISTIC by construction: `TestRunner::new` seeds randomly, so
    /// which draw carries the trigger varies (~30s). Fail-safe rather than
    /// flaky-green — a seed that never draws a triggering sequence fails on the
    /// `expected the trigger to fail the property` arm instead of passing
    /// vacuously.
    #[test]
    fn the_shrinker_never_yields_a_sequence_its_capmap_cannot_host() {
        use proptest::test_runner::Config;
        use proptest::test_runner::TestCaseError;
        use proptest::test_runner::TestError;
        use proptest::test_runner::TestRunner;

        const TRIGGER: &str = "trigger: a shrinkable-wiring draw carries a lifecycle transition";
        let lifecycle_cap = holon_pbt_core::composition::CapId::of::<
            dyn holon_pbt_core::capabilities::SutAppLifecycle,
        >();

        let mut runner = TestRunner::new(Config {
            cases: 512,
            max_shrink_iters: 8192,
            ..Config::default()
        });
        let outcome = runner.run(
            &WideE2EMachine::sequential_strategy(2..10usize),
            |(initial, transitions, seen)| {
                for transition in &transitions {
                    // The library's own runner counts every transition it reaches;
                    // the shrinker uses that count to drop never-executed steps.
                    // Skipping it would make this walk shrink differently from a
                    // real run.
                    if let Some(seen) = seen.as_ref() {
                        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    if !crate::pbt::stepper::transition_applicable(&initial, transition) {
                        panic!(
                            "the shrinker kept {} under wiring {:?}, whose CapMap cannot host it \
                             — a real run aborts here in CapMap::expect",
                            transition.variant_name(),
                            initial.harness.wiring,
                        );
                    }
                }
                let carries_lifecycle = transitions
                    .iter()
                    .any(|t| t.required_caps().contains(&lifecycle_cap));
                let wiring = &initial.harness.wiring;
                let shrinkable_past_the_cap = wiring.has_storage(StorageAdapter::Loro)
                    && wiring.has_storage(StorageAdapter::Turso);
                if carries_lifecycle && shrinkable_past_the_cap {
                    return Err(TestCaseError::fail(TRIGGER));
                }
                Ok(())
            },
        );

        match outcome {
            // The trigger must be what fails, and it must survive to the minimal
            // case — that is the proof the shrink ran WITH the lifecycle
            // transition retained while everything else, the wiring included,
            // was minimized.
            Err(TestError::Fail(reason, (initial, transitions, _))) => {
                assert!(
                    reason.message().contains(TRIGGER),
                    "the shrink must end on the trigger, not on the applicability abort: {reason}"
                );
                assert!(
                    transitions
                        .iter()
                        .any(|t| t.required_caps().contains(&lifecycle_cap)),
                    "non-vacuity: the minimized sequence must still carry the lifecycle transition"
                );
                assert!(
                    initial.caps_available(&[lifecycle_cap]),
                    "the minimized initial state must be a wiring that can HOST what it carries"
                );
            }
            other => panic!("expected the trigger to fail the property, got {other:?}"),
        }
    }

    /// **Feed-driven write-back reachability** — the direct negation of the
    /// holder design's §9.5 finding, which observed a keystone draw with ZERO
    /// `[OrgMode]` / `FileSyncController` activity because `storage={Loro,
    /// Org}` carries no `BlockFeed`.
    ///
    /// `org.on_block_feed` is entered ONLY from the `OrgRerender` drain, and
    /// that drain's producer is spawned only inside `if let Some(feed) =
    /// block_feed` (`holon-orgmode/src/di.rs`). So a non-zero count is
    /// proof that a Turso-inclusive draw registers the `BlockFeed`, spawns
    /// the write-back resolver, and actually delivers through it — the
    /// layer Inc 2 makes the ONLY write-back path.
    #[test]
    fn turso_draw_reaches_the_feed_driven_writeback_path() {
        // Touch the collector BEFORE the SUT boots so the subscriber is installed
        // and the OTel layer records the span (an untouched collector captures
        // nothing).
        let collector = crate::test_tracing::SpanCollector::global();
        let scope = crate::test_tracing::ensure_test_scope();

        let wiring = Wiring::custom(
            vec![
                StorageAdapter::Loro,
                StorageAdapter::Org,
                StorageAdapter::Turso,
            ],
            vec![],
            vec![],
        );
        let ref_state = wide_e2e_ref_for(&wiring);
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all();
        // `org.on_block_feed` fires on a runtime worker, and spans are charged
        // to the scope owning the emitting thread — without this binding the
        // span this test asserts on belongs to nobody.
        crate::test_tracing::attach_scope_to_runtime(&mut builder, scope);
        let rt = builder.build().expect("build multi-thread runtime");
        rt.block_on(async {
            let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
            let (_caps, _handle, _scaffold) = boot_and_seed_wide(&resolver, &ref_state).await;
            // The resolver task drains asynchronously; poll rather than sleep a
            // fixed amount so a fast machine doesn't pay for a slow one's margin.
            for _ in 0..100 {
                if collector.count_spans("org.on_block_feed") > 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });
        let seen = collector.count_spans("org.on_block_feed");
        eprintln!("[writeback-reachability] org.on_block_feed spans: {seen}");
        assert!(
            seen > 0,
            "a Turso-inclusive draw must reach the feed-driven org write-back path, but \
             `org.on_block_feed` never ran — either no `BlockFeed` was registered (no \
             EventInfraModule / block matview watch) or the resolver task never delivered. This \
             is the §9.5 ENVIRONMENT gap: with no feed, the group_by/home_by resolver and the \
             FileSyncController delta drain are inert and the keystone tests write-back NOWHERE"
        );
    }

    /// **Task #22 — the SHIPPED DEFAULT mode must be a reachable draw.**
    ///
    /// Production defaults `crdt.enabled` to `false`
    /// (`holon-frontend/src/config.rs::crdt_enabled`), so the shipped desktop
    /// app registers NO `CrudAuthority` and block CRUD falls back to
    /// `SqlOperationProvider`. In the composed keystone that mode is
    /// `loro_enabled = false`, which `compose_sut` reads off
    /// `Projection::EditorState`. A draw whose manifest carries no
    /// `StorageAdapter::Loro` therefore MUST resolve to the SQL authority —
    /// otherwise the keystone claims a Loro CRUD authority the manifest never
    /// wired, and the shipped default never runs.
    #[test]
    fn a_turso_draw_without_loro_routes_block_crud_to_sql() {
        let sql_only = Wiring::sql_only();
        assert!(
            !sql_only.has_storage(StorageAdapter::Loro),
            "premise: the sql_only blessed manifest wires no Loro adapter"
        );
        let set = set_for_wiring(&sql_only);
        assert!(
            !set.has_projection(Projection::EditorState),
            "a Loro-free manifest must not select EditorState — EditorState IS the Loro-CRUD \
             switch (compose_sut passes it as `loro_enabled`), so selecting it turns the CRDT on \
             for a draw that never wired Loro and hides the shipped default; got {set:?}"
        );
        let line = authority_routing_disclosure(&sql_only);
        assert!(
            line.contains("block-CRUD=Sql(SqlOperationProvider)"),
            "the shipped default (crdt off) must resolve block CRUD to the SQL provider: {line}"
        );
    }

    /// **Task #22 non-vacuity floor.** The structural resolution above is
    /// worthless if no *drawn* wiring ever hits it. Over the live
    /// `any_valid_wiring()` draw (fixed-seed runner), the share of accepted
    /// draws that resolve block CRUD to SQL must clear
    /// [`MIN_SQL_CRUD_DRAW_SHARE`] — a generator change that starves the
    /// shipped-default mode fails loudly here instead of silently returning it
    /// to zero coverage.
    #[test]
    fn sql_crud_authority_draw_share_meets_floor() {
        use proptest::strategy::Strategy;
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        let strategy = holon_pbt_core::wiring::any_valid_wiring();
        let mut runner = TestRunner::deterministic();
        let draws = 4096;
        let mut sql_crud = 0usize;
        for _ in 0..draws {
            let w = strategy
                .new_tree(&mut runner)
                .expect("any_valid_wiring must always produce a valid manifest")
                .current();
            if !set_for_wiring(&w).has_projection(Projection::EditorState) {
                sql_crud += 1;
            }
        }
        let share = sql_crud as f64 / draws as f64;
        assert!(
            share >= MIN_SQL_CRUD_DRAW_SHARE,
            "shipped-default coverage floor: only {sql_crud}/{draws} ({share:.3}) accepted draws \
             resolve block CRUD to `SqlOperationProvider`, below the \
             {MIN_SQL_CRUD_DRAW_SHARE:.3} floor — the mode the desktop app actually ships \
             (crdt.enabled defaults to false) would run in fewer keystone cases than the floor \
             allows"
        );
    }

    /// **Task #22 end-to-end teeth.** The floor above only counts draws; this
    /// boots one. A `{Turso, Org}` draw must compose, seed, and run the
    /// composed invariant catalog GREEN with the block-id correspondence
    /// actually running (non-vacuity), while the SUT is in SQL-CRUD mode.
    #[test]
    fn sql_only_draw_boots_and_runs_the_catalog_green() {
        let wiring = Wiring::custom(
            vec![StorageAdapter::Org, StorageAdapter::Turso],
            vec![],
            vec![],
        );
        assert!(
            !set_for_wiring(&wiring).has_projection(Projection::EditorState),
            "premise: this draw must be in SQL-CRUD mode"
        );
        let ref_state = wide_e2e_ref_for(&wiring);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build multi-thread runtime");
        rt.block_on(async {
            let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
            let (caps, _handle, scaffold) = boot_and_seed_wide(&resolver, &ref_state).await;
            let report = <WideE2E as ComposedSlice>::run_report(
                &caps,
                &resolver,
                &BurnedPairs::new(),
                &std::collections::BTreeSet::new(),
                &scaffold,
                &ref_state,
            )
            .await;
            assert!(
                report.failures().is_empty(),
                "SqlOnly wide seed must run the catalog green; failures: {:?}",
                report.failures()
            );
            let ran = report.ran_ids();
            assert!(
                ran.contains(&"inv-blocks-match-ref/block_raw"),
                "the block-id comparison must RUN over the seeded SqlOnly SUT (non-vacuity proof \
                 the seed landed); ran: {ran:?}"
            );
        });
    }

    /// The shipped-default draw used by the two typing-engagement guards below
    /// (`{Org, Turso}`, no Loro ⇒ `crdt.enabled = false`).
    fn sqlonly_wiring() -> Wiring {
        Wiring::custom(
            vec![StorageAdapter::Org, StorageAdapter::Turso],
            vec![],
            vec![],
        )
    }

    /// Content fingerprint of the reference working tree — what "a typed edit
    /// reached block content" is measured against.
    fn ref_block_contents(state: &ReferenceState) -> BTreeMap<EntityUri, String> {
        state
            .domain
            .block_state
            .blocks
            .iter()
            .map(|(id, b)| (id.clone(), b.content.clone()))
            .collect()
    }

    /// **Tasks #20 / #52 — the shipped default must be TYPEABLE, per case.**
    ///
    /// The keystone's alphabet is narrowed by the composed SUT's cap set
    /// (`aggregate_transitions` → `caps_available`). A SqlOnly draw that hosts
    /// no `SutEditorMirrorWrite` drops
    /// `TypeChars`/`DeleteBackward`/`MoveCursor` entirely, so the mode the
    /// desktop app actually ships had ZERO typing coverage — the escape
    /// route of both the post-split sibling-order and the stale-marks prod
    /// bugs.
    ///
    /// Measured over GENERATED cases (the real `aggregate_transitions`
    /// alphabet under the real SqlOnly cap set, stepped on the reference): a
    /// case counts only when a drawn `TypeChars` actually CHANGED reference
    /// block content — i.e. the keystroke committed through
    /// `commit_active_editor_if_changed`, not merely sat in an editor buffer.
    /// The SUT half of that claim (the same commit landing in `block_raw`
    /// through the production editor sink) is
    /// `sqlonly_typing_commits_through_the_editor_sink_into_block_raw`.
    #[test]
    fn sqlonly_generated_cases_commit_typed_edits_into_block_content() {
        use proptest::strategy::Strategy;
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        use crate::pbt::transitions::aggregate_transitions;

        // Cases × steps mirror the live keystone's `sequential_strategy(1..40)`.
        const CASES: usize = 64;
        const STEPS: usize = 40;

        /// Share of generated cases in which a drawn `TypeChars` actually
        /// CHANGED reference block content.
        fn committed_typing_share(base: &ReferenceState) -> (usize, f64) {
            let mut runner = TestRunner::deterministic();
            let mut typed_cases = 0usize;
            for _ in 0..CASES {
                let mut state = base.clone();
                let mut committed = false;
                for _ in 0..STEPS {
                    let transition = aggregate_transitions(&state)
                        .new_tree(&mut runner)
                        .expect("aggregate_transitions must produce a value")
                        .current();
                    let typing = transition.variant_name() == "TypeChars";
                    let before = typing.then(|| ref_block_contents(&state));
                    transition.apply_to_ref(&mut state);
                    if let Some(before) = before {
                        committed |= ref_block_contents(&state) != before;
                    }
                }
                if committed {
                    typed_cases += 1;
                }
            }
            (typed_cases, typed_cases as f64 / CASES as f64)
        }

        let wiring = sqlonly_wiring();
        assert!(
            !set_for_wiring(&wiring).has_projection(Projection::EditorState),
            "premise: this draw runs the shipped default (SQL block-CRUD authority)"
        );
        // Boots one composed SqlOnly SUT to read its real cap set (cached).
        let (sql_cases, sql_share) = committed_typing_share(&wide_e2e_ref_for(&wiring));
        eprintln!(
            "[sqlonly-typing-engagement] {sql_cases}/{CASES} ({sql_share:.3}) cases committed a \
             typed edit into block content"
        );
        assert!(
            sql_share >= MIN_SQLONLY_TYPING_CASE_SHARE,
            "shipped-default TYPING coverage floor: only {sql_cases}/{CASES} ({sql_share:.3}) \
             generated SqlOnly cases committed a typed edit into block content, below the \
             {MIN_SQLONLY_TYPING_CASE_SHARE:.3} floor — the keystone cannot type in the mode the \
             desktop app ships (crdt.enabled defaults to false), so no property covers its \
             editor→`set_field(\"content\")` write path"
        );
    }

    /// The SUT half of the engagement claim: a keystroke on a SqlOnly composed
    /// SUT must travel the PRODUCTION editor sink (`ReactiveEngineDriver` →
    /// `HeadlessEditorMirror` → the cell-free `EditorViewModel` →
    /// `vm_commit_edit` → `set_field("content")`) and land in `block_raw`, with
    /// the reference in lockstep so the composed catalog compares it.
    #[test]
    fn sqlonly_typing_commits_through_the_editor_sink_into_block_raw() {
        use holon_pbt_core::capabilities::SutBackend;

        use crate::pbt::transitions::FocusEditableText;
        use crate::pbt::transitions::TypeChars;

        let wiring = sqlonly_wiring();
        let mut oracle = wide_e2e_ref_for(&wiring);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build multi-thread runtime");
        rt.block_on(async {
            let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
            let (mut caps, _handle, scaffold) = boot_and_seed_wide(&resolver, &oracle).await;

            let target = fixed_ids().c1;
            let focus = FocusEditableText {
                block_id: target.clone(),
            };
            focus.apply_to_ref(&mut oracle);
            TransitionImpl::apply_to_sut(&focus, &oracle, &mut caps).await;

            let typed = TypeChars {
                text: "Zq".to_string(),
            };
            typed.apply_to_ref(&mut oracle);
            TransitionImpl::apply_to_sut(&typed, &oracle, &mut caps).await;
            tokio::time::sleep(SETTLE).await;

            let expected = oracle
                .domain
                .block_state
                .blocks
                .get(&target)
                .expect("the typed target is in the reference tree")
                .content
                .clone();
            assert!(
                expected.contains("Zq"),
                "premise: the reference committed the typed text; got {expected:?}"
            );
            let sut_content = SutBackend::block_raw_snapshot(&caps)
                .await
                .into_iter()
                .find(|b| b.id == target)
                .map(|b| b.content)
                .expect("the typed target must exist in block_raw");
            assert_eq!(
                sut_content, expected,
                "a SqlOnly keystroke must commit through the editor sink into `block_raw` — the \
                 shipped default's `set_field(\"content\")` write path"
            );

            let report = <WideE2E as ComposedSlice>::run_report(
                &caps,
                &resolver,
                &BurnedPairs::new(),
                &std::collections::BTreeSet::new(),
                &scaffold,
                &oracle,
            )
            .await;
            assert!(
                report.failures().is_empty(),
                "SqlOnly focus+type must stay green; failures: {:?}",
                report.failures()
            );
            assert!(
                report.ran_ids().contains(&"inv-block-content/block_raw"),
                "non-vacuity: the committed-content comparison must RUN over the typed SqlOnly \
                 block; ran: {:?}",
                report.ran_ids()
            );
        });
    }

    /// The SUT-side parameterization seam's behaviour-preservation anchor: the
    /// swap's current fixed wiring round-trips through [`set_for_wiring`]
    /// to exactly `full_headless` (so flipping `init_state` to draw
    /// `any_valid_wiring()` cannot silently change today's full_headless
    /// run), and the normalizer maps a Loro-only draw onto the cheap
    /// headless backend (Loro forced, no ViewModel, no UI).
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

    /// The failure-header authority disclosure must name LORO as the block-CRUD
    /// authority — NOT Turso — for a draw that wires BOTH, while naming SQL as
    /// the projection sink (the reading that misled prior readers of a
    /// `storage={…,Turso}` header). Single-sourced from `set_for_wiring`, so it
    /// can never disagree with what the SUT actually boots.
    #[test]
    fn authority_disclosure_names_loro_as_block_crud_for_a_loro_turso_draw() {
        let loro_turso = Wiring::custom(
            vec![
                StorageAdapter::Loro,
                StorageAdapter::Org,
                StorageAdapter::Turso,
            ],
            vec![],
            vec![],
        );
        let line = authority_routing_disclosure(&loro_turso);
        assert_eq!(
            line,
            "authority: block-CRUD=Loro(LoroBlockOperations, via EditorState); \
             projection-sinks=Sql(block_raw,matview); org-writeback=on"
        );

        // The SHIPPED DEFAULT: the same draw minus the Loro adapter routes block
        // CRUD to the SQL provider over the same SQL sink.
        let sql_only = Wiring::custom(
            vec![StorageAdapter::Org, StorageAdapter::Turso],
            vec![],
            vec![],
        );
        assert_eq!(
            authority_routing_disclosure(&sql_only),
            "authority: block-CRUD=Sql(SqlOperationProvider); \
             projection-sinks=Sql(block_raw,matview); org-writeback=on"
        );

        let loro_only = Wiring::custom(vec![StorageAdapter::Loro], vec![], vec![]);
        let loro_line = authority_routing_disclosure(&loro_only);
        assert!(
            loro_line.contains("block-CRUD=Loro(LoroBlockOperations, via EditorState)"),
            "Loro-only draw is also EditorState/Loro-CRUD: {loro_line}"
        );
        assert!(
            loro_line.contains("projection-sinks=none(Loro-only, no Sql sink)"),
            "a Loro-only draw has no SQL sink: {loro_line}"
        );
    }

    /// A4 NON-VACUITY: the `full_headless` cap set now ADMITS the peer
    /// transitions, so `aggregate_transitions` auto-selects them into the
    /// swap alphabet. Before A2 this would FAIL (the builder withheld
    /// `SutLoro` in full mode → `required_caps()=[SutLoro]`
    /// were unsatisfiable → peer ops auto-narrowed out). A green
    /// `general_e2e_composed_pbt` run where peer ops never fired would be a
    /// false pass; this proves they CAN fire, deterministically and fast
    /// (no reliance on trace logs).
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
                "full_headless cap set must admit {:?} (required_caps={:?}) — peer mesh wired in \
                 A2; if this fails, SutLoro is not present in the composed full_headless build",
                t.variant_name(),
                t.required_caps()
            );
        }
    }

    /// CAP-PRESENCE GUARD: the WIDEST wiring (`full_headless`) must PROVIDE
    /// every cap the shared catalog's invariants declare in their `Needs` —
    /// so every catalog invariant is guaranteed SELECTED (and thus run, via
    /// the per-draw `required_invariants` floor) in the wide config.
    /// Deselection has exactly one cause — a `Needs` cap absent from the CapMap
    /// — so this guard catches it at the cap level, with no
    /// per-invariant-id list to keep in sync.
    ///
    /// The union of every `Needs.sut_present` is checked against the widest SUT
    /// cap_set (`full_headless_cap_set`); the union of every
    /// `Needs.ref_present` against the ref cap_set (the `ReferenceState`
    /// registers all ref caps unconditionally). A cap that is referenced
    /// but absent is a finding UNLESS it is on `WIDE_HEADLESS_ABSENT_CAPS` (the
    /// windowed/GPUI rung, structurally impossible headless). The failure names
    /// the missing cap AND the invariant ids that need it — actionable,
    /// fail-loud.
    #[test]
    fn wide_cap_presence_guard() {
        use holon_pbt_core::composition::CapProvider;

        // The REAL widest CapMap the keystone drives: `full_headless` booted through
        // the production builder (`boot_and_seed_wide`), INCLUDING the
        // `ComposedSpanMetrics` span-metrics caps it registers on top of the
        // bare `compose_sut` map. The bare `full_headless_cap_set()` is only
        // the generation-narrowing hint and omits those, so checking against it
        // would false-flag `ComposedBudget`.
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

        let mut excluded: BTreeSet<&'static str> =
            WIDE_HEADLESS_ABSENT_CAPS.iter().map(|(c, _)| *c).collect();
        // `SutClockAdvance` is excluded only while its env gate is OFF; with the
        // flag on the builder registers it, and the exclusion would read stale.
        if std::env::var("HOLON_PBT_ADVANCE_DAY").is_ok() {
            excluded.remove("SutClockAdvance");
        }

        // Every excluded cap must ACTUALLY be missing — a stale exclusion (a cap that
        // is now present) is itself a smell to prune, so fail loud on it too.
        let stale: Vec<&'static str> = excluded
            .iter()
            .copied()
            .filter(|c| !missing.contains_key(c))
            .collect();
        assert!(
            stale.is_empty(),
            "WIDE_HEADLESS_ABSENT_CAPS lists caps that ARE present in the widest wiring (stale \
             exclusions — remove them): {stale:?}"
        );

        let unexpected: Vec<(&'static str, Vec<&'static str>)> = missing
            .iter()
            .filter(|(cap, _)| !excluded.contains(**cap))
            .map(|(cap, ids)| (*cap, ids.iter().copied().collect()))
            .collect();
        assert!(
            unexpected.is_empty(),
            "the widest wiring (full_headless) is MISSING caps the shared catalog needs, and they \
             are NOT on the WIDE_HEADLESS_ABSENT_CAPS exclusion list — either the cap regressed \
             out of the widest CapMap (fix the wiring) or it is a genuinely headless-absent cap \
             (add it to WIDE_HEADLESS_ABSENT_CAPS with a reason). missing cap → invariant ids \
             that need it: {unexpected:?}"
        );
    }

    /// iOS-PARITY SUBSTRATE PIN (2026-07-06). The iOS GPUI app boots through
    /// `GpuiModule` → `HolonFrontendModule::configure` → `add_frontend`
    /// (frontends/gpui/src/di.rs, mobile.rs). Its ONLY material config delta vs
    /// a desktop boot is `holon_config.crdt.enabled = Some(true)`
    /// (frontends/gpui/src/mobile.rs ~L35), which makes `add_frontend`
    /// (holon-app/src/wiring.rs L148-184) register `LoroModule` AND the Loro
    /// `CrudAuthority(LoroBlockOperations)` — Loro owns block CRUD, SQL mirrors
    /// it.
    ///
    /// The composed keystone (`compose_sut(full_headless)`) boots the SAME
    /// substrate: `full_headless()` carries `Projection::EditorState`, so
    /// the builder's frontend
    /// arm calls `HeadlessFrontendComponent::new_with_loro(..,
    /// loro_enabled=true)` (builder.rs L279), which sets `crdt.enabled =
    /// Some(true)` and boots through `holon_app::new_from_config_with_di` →
    /// `add_frontend` — the exact same DI seam and `crdt_enabled()` branch
    /// the iOS app hits. So both register the Loro `CrudAuthority`.
    ///
    /// Audited parity table (knob | iOS app | keystone | match):
    ///   crdt.enabled           | Some(true)          | Some(true) via
    /// EditorState | YES   CrudAuthority          | Loro (add_frontend) |
    /// Loro (add_frontend)        | YES   storage backend        | Turso +
    /// Loro        | Turso + Loro               | YES   config seam
    /// | add_frontend        | add_frontend               | YES (same fn)
    ///   locked_keys            | empty               | empty
    /// | YES   Actor::UI / MCP actor  | present (window/MCP)| absent
    /// (headless)          | by-design (full_headless drops UI)   db_path /
    /// vault root   | app sandbox         | tempdir                    |
    /// immaterial (path only)
    ///
    /// This pin fails loud if a future edit drops `EditorState` from
    /// `full_headless` (silently disabling the Loro authority substrate →
    /// the keystone would stop exercising what iOS runs) OR if the builder
    /// stops registering the Loro peer-mesh authority surface (`SutLoro`),
    /// which is present ONLY when the frontend booted its live Loro
    /// authority doc (builder.rs L328/L367/L489). Its presence is
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
            "full_headless dropped EditorState — the keystone would boot the frontend arm with \
             crdt.enabled=Some(false), losing the Loro CrudAuthority substrate the iOS app forces \
             via crdt.enabled=Some(true). iOS parity broken."
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

    /// COUNT FLOOR — belt against silent catalog deletion: the shared catalog
    /// has at least its current size. Rename-proof (counts entries, not
    /// ids). Update N when an invariant is DELIBERATELY removed from the
    /// catalog.
    #[test]
    fn composed_catalog_count_floor() {
        // N = today's catalog size (45 without `otel-testing`; `sql_budget` adds one
        // under it).
        const N: usize = 45;
        let len = composed_invariant_catalog().len();
        assert!(
            len >= N,
            "composed catalog shrank to {len} (floor {N}) — an invariant was removed. If \
             deliberate, lower N; otherwise a `wire()` line was lost."
        );
    }
}
/// Pinned deterministic gate: boot a test engine, seed the canned
/// `block:tpl`/`block:tpl-c1` template blocks, dispatch
/// `instantiate_template` through the engine, and verify both
/// substitution and idempotency (same context_key twice -> no
/// duplicates).
#[tokio::test(flavor = "multi_thread")]
async fn instantiate_template_deterministic_gate() {
    use std::collections::HashMap;

    use holon::core::SqlOperationProvider;
    use holon::di::test_helpers::create_test_engine_with_providers;
    use holon::storage::BLOCK_WRITE_TABLE;
    use holon_api::EntityName;
    use holon_api::OpOrigin;
    use holon_api::Value;

    let engine = create_test_engine_with_providers(":memory:".into(), |module| {
        module.with_operation_provider_factory(|backend| {
            let db_handle =
                tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
            std::sync::Arc::new(SqlOperationProvider::new(
                db_handle,
                BLOCK_WRITE_TABLE.to_string(),
                "block".to_string(),
                "block".to_string(),
            ))
        })
    })
    .await
    .expect("boot engine");

    let block_entity = EntityName::new("block");

    // Create a target parent.
    engine
        .execute_operation(
            &block_entity,
            "create",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("id".into(), Value::String("block:target".into()));
                p.insert("content".into(), Value::String("Target".into()));
                p
            },
            OpOrigin::User,
        )
        .await
        .expect("create target parent");

    // Seed the template blocks (same shape as seed_template in
    // operation_engine.rs).
    engine
        .execute_operation(
            &block_entity,
            "create",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("id".into(), Value::String("block:tpl".into()));
                p.insert("content".into(), Value::String("{{date}}".into()));
                p.insert("template".into(), Value::String("t".into()));
                p.insert(
                    "template_vars".into(),
                    Value::String("date, mood=neutral".into()),
                );
                p
            },
            OpOrigin::User,
        )
        .await
        .expect("seed template root");

    engine
        .execute_operation(
            &block_entity,
            "create",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("id".into(), Value::String("block:tpl-c1".into()));
                p.insert("parent_id".into(), Value::String("block:tpl".into()));
                p.insert("content".into(), Value::String("see {{date}} now".into()));
                p.insert(
                    "marks".into(),
                    Value::String(r#"[{"start":0,"end":3,"kind":"Bold"}]"#.into()),
                );
                p
            },
            OpOrigin::User,
        )
        .await
        .expect("seed template child");

    // Dispatch instantiate_template.
    let context_key = "2026-07-15";
    let root_id = engine
        .execute_operation(
            &block_entity,
            "instantiate_template",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("template_id".into(), Value::String("block:tpl".into()));
                p.insert("target_parent".into(), Value::String("block:target".into()));
                p.insert("context_key".into(), Value::String(context_key.into()));
                let mut bindings: HashMap<String, Value> = HashMap::new();
                bindings.insert("date".into(), Value::String("2026-07-15".into()));
                p.insert("bindings".into(), Value::Object(bindings));
                p
            },
            OpOrigin::Rule {
                transition_id: "rule:test-template".into(),
            },
        )
        .await
        .expect("instantiate_template");
    let Some(Value::String(root_id)) = root_id.response else {
        panic!("instantiate_template must return root id")
    };

    // Verify substituted heading content.
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT * FROM block_raw WHERE id = '{}'",
                root_id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("query instance root");
    let root = &rows[0];
    assert_eq!(
        root.get("content")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string()),
        Some("2026-07-15".to_string()),
        "{{date}} substituted in heading"
    );

    // Verify child: substituted, marks survived.
    let children = engine
        .db_handle()
        .query(
            &format!(
                "SELECT * FROM block_raw WHERE parent_id = '{}'",
                root_id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("query instance children");
    assert_eq!(children.len(), 1, "one child");
    let child_content = children[0].get("content").and_then(|v| v.as_string());
    assert_eq!(
        child_content,
        Some("see 2026-07-15 now"),
        "{{date}} substituted in child"
    );
    let marks = children[0].get("marks");
    assert!(marks.is_some(), "bold mark survived instantiation");

    // Idempotency: second fire with same key converges.
    let count_before = engine
        .db_handle()
        .query(
            "SELECT 1 FROM block_raw WHERE parent_id = 'block:target'",
            HashMap::new(),
        )
        .await
        .expect("query count before")
        .len();

    engine
        .execute_operation(
            &block_entity,
            "instantiate_template",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("template_id".into(), Value::String("block:tpl".into()));
                p.insert("target_parent".into(), Value::String("block:target".into()));
                p.insert("context_key".into(), Value::String(context_key.into()));
                let mut bindings: HashMap<String, Value> = HashMap::new();
                bindings.insert("date".into(), Value::String("2026-07-15".into()));
                p.insert("bindings".into(), Value::Object(bindings));
                p
            },
            OpOrigin::Rule {
                transition_id: "rule:test-template".into(),
            },
        )
        .await
        .expect("second instantiate_template");

    let count_after = engine
        .db_handle()
        .query(
            "SELECT 1 FROM block_raw WHERE parent_id = 'block:target'",
            HashMap::new(),
        )
        .await
        .expect("query count after")
        .len();
    assert_eq!(
        count_before, count_after,
        "idempotent: same context_key must converge (no duplicates)"
    );
}
