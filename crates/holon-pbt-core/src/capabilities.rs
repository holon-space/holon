//! PBT capability traits — Stage A (Phase 1 draft).
//!
//! Defines the minimum reference-side and SUT-side capability traits the
//! seven Phase 5 T0 transitions need (TypeChars, DeleteBackward,
//! MoveCursor, MoveUp, MoveDown, SplitBlock, JoinBlock, Indent, Outdent).
//!
//! ## Status — DRAFT (Phase 1, hypothesis-verification stage)
//!
//! This module is currently *not wired into any consumer*. Phase 2 lands
//! blanket impls on `ReferenceState`; Phase 3 migrates the seven
//! transitions to bind on these traits; Phase 4 mirrors the same shape on
//! the SUT side.
//!
//! ## Three axes × two access modes
//!
//! - **BlockTree**: in-memory block structure (parent/child, sort order,
//!   content, tags). `RefBlockTree` (read) / `RefBlockTreeMut` (write).
//! - **EditorMirror**: active-editor text + cursor mirror — what the GPUI
//!   `InputState` shows. `RefEditorMirror` / `RefEditorMirrorMut`.
//! - **Focus**: per-region focused block id + cursor position.
//!   `RefFocus` / `RefFocusMut`.
//!
//! Plus one administrative trait, [`RefLifecycle`], for gate predicates
//! (`app_started`, `is_properly_setup`, `enable_loro`) that wide-PBT
//! transitions check. Pure-slice impls return constants; wide-PBT impls
//! delegate to `ReferenceState`.
//!
//! ## SUT side
//!
//! Symmetric mirror: [`SutBlockTreeWrite`], [`SutEditorMirrorWrite`],
//! [`SutFocusWrite`], [`SutQuiesce`]. Methods take only what they need —
//! no `ref_state` leak (wide PBT keeps its `doc_uri_map` and similar
//! internal state via interior mutability on the SUT itself).

use std::collections::BTreeSet;

// NOTE — Phase 1 draft uses stringly-typed identifiers and owned values
// so this crate doesn't grow `holon-api` as a dep before the migration
// commits to it. Phase 2's blanket impls translate at the boundary.

/// Stringly-typed block identifier as carried in capability-trait
/// signatures. The wide PBT uses `holon_api::EntityUri`; the pure slice
/// can use any `Into<String>` newtype. Concrete impls translate.
pub type CapBlockId = String;

/// Symbolic region. Wide PBT uses `holon_api::Region` (Main / Sidebar);
/// pure slice has only a single region — its impl ignores the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapRegion {
    Main,
    Sidebar,
    /// Used by impls that have no region distinction.
    Single,
}

/// Cursor position in the editor mirror. Wide PBT carries `line`+`column`
/// to mirror GPUI; pure slice tracks byte offset only. Concrete impls
/// adapt; the trait carries the structural shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapCursor {
    pub line: usize,
    pub column: usize,
}

// ─── Reference-side: BlockTree ────────────────────────────────────────

/// Read-side block-tree queries used by Phase 5 T0 transitions and their
/// generators.
pub trait RefBlockTree {
    /// Returns block content text. `None` if the block does not exist.
    fn block_content(&self, id: &CapBlockId) -> Option<&str>;

    /// True if the block exists and is a Text-typed block (the only kind
    /// editor transitions care about).
    fn is_text_block(&self, id: &CapBlockId) -> bool;

    /// Editable Text descendants of the focus root in `Main` region.
    /// Empty in pure slice if the test fixture didn't seed any.
    fn main_editable_descendants(&self) -> Vec<CapBlockId>;

    /// Block ids of the current focus roots in `region`. Wide PBT
    /// computes from `expected_focus_root_ids`; pure slice may just
    /// return the root id of its single doc.
    fn focus_root_ids(&self, region: CapRegion) -> BTreeSet<CapBlockId>;

    /// Sibling navigation.
    fn previous_sibling(&self, id: &CapBlockId) -> Option<CapBlockId>;
    fn next_sibling(&self, id: &CapBlockId) -> Option<CapBlockId>;

    /// Parent of `id`. `None` if `id` is root or has a sentinel parent
    /// (wide PBT: `EntityUri::is_no_parent` / `is_sentinel`; pure slice:
    /// `parent: None`).
    fn parent_of(&self, id: &CapBlockId) -> Option<CapBlockId>;

    /// Grandparent for outdent.
    fn grandparent(&self, id: &CapBlockId) -> Option<CapBlockId>;

    /// Children of a parent, sorted by sort_key. Returns ids only —
    /// callers join back through `block_content` if they need data.
    fn sorted_children(&self, parent: &CapBlockId) -> Vec<CapBlockId>;

    /// True if `id` is a descendant of any ancestor in `ancestors`.
    fn is_descendant_of_any(&self, id: &CapBlockId, ancestors: &BTreeSet<CapBlockId>) -> bool;

    /// Layout blocks (the layout scaffolding the user can't focus into).
    /// Wide PBT: `layout_blocks` set; pure slice: empty.
    fn is_layout_block(&self, id: &CapBlockId) -> bool;

    /// True if `id` exists and is focusable (i.e. not a layout block,
    /// not immutable, has the right content type).
    fn is_focusable(&self, id: &CapBlockId) -> bool;

    /// True if `id` is in the "no content update" set — render sources,
    /// query sources, profile blocks. Wide PBT consults
    /// `layout_blocks.render_source_ids` + `layout_blocks.query_source_ids`
    /// + `profile_block_ids`. Pure slice has no such concept → returns
    /// `false`.
    fn is_no_content_update(&self, id: &CapBlockId) -> bool;

    /// True if `id` is a Page block (tagged `Page`). Mirrors
    /// `Block::is_page()`. Pure slice has no pages → returns `false`.
    fn is_page_block(&self, id: &CapBlockId) -> bool;
}

/// Block-tree mutations. Concrete impls maintain whatever bookkeeping
/// they need (sort_key generation, undo snapshots, focus follow-ups);
/// the trait only commits to the shape of the operation.
pub trait RefBlockTreeMut: RefBlockTree {
    /// Push the current state onto the undo stack. Wide PBT: real
    /// snapshot; pure slice: may be a no-op if undo isn't tested.
    fn push_undo_snapshot(&mut self);

    /// Set the content text of `id`. Used by `commit_active_editor_if_changed`
    /// and any future direct-write transitions.
    fn set_block_content(&mut self, id: &CapBlockId, text: &str);

    /// Split `id` at `position`. Returns the id of the newly-created
    /// block holding the tail.
    fn split_block(&mut self, id: &CapBlockId, position: usize) -> CapBlockId;

    /// Join `id` into its previous sibling (or parent if no previous
    /// sibling). Returns the cursor position of the join point in the
    /// merged block's content.
    fn join_block(&mut self, id: &CapBlockId) -> usize;

    /// Indent `id` — re-parent under previous sibling.
    fn indent(&mut self, id: &CapBlockId);

    /// Outdent `id` — move up to grandparent level.
    fn outdent(&mut self, id: &CapBlockId);

    /// Re-parent `id` under `new_parent`, placing it after `after` (or
    /// first if `after` is None). Used by Indent/Outdent helpers when
    /// they don't want to bake the parent-discovery logic into the
    /// transition body. The wide-PBT impl is
    /// `ReferenceState::move_block`.
    fn move_block(&mut self, id: &CapBlockId, new_parent: CapBlockId, after: Option<&CapBlockId>);

    /// Swap two siblings (used by MoveUp / MoveDown).
    fn swap_siblings(&mut self, a: &CapBlockId, b: &CapBlockId);
}

// ─── Reference-side: EditorMirror ────────────────────────────────────

/// Read-side active-editor state.
pub trait RefEditorMirror {
    /// Block id whose editor is currently active, or `None` if no editor
    /// is open. Pure slice typically has this populated by a setup
    /// transition; wide PBT mirrors GPUI's `InputState`.
    fn active_editor_block(&self) -> Option<CapBlockId>;

    /// Live in-memory editor text. Pre-blur, this can diverge from
    /// `block_content(active_editor_block())` — the divergence is what
    /// surfaces split-with-pending-edit bugs.
    fn active_editor_text(&self) -> Option<&str>;

    /// Cursor byte offset within `active_editor_text`.
    fn active_editor_cursor(&self) -> Option<usize>;
}

/// Editor-mirror mutations. Apply to whichever editor is active.
pub trait RefEditorMirrorMut: RefEditorMirror {
    fn type_chars(&mut self, text: &str);
    fn delete_backward(&mut self, count: usize);
    fn move_cursor(&mut self, byte_position: usize);
}

// ─── Reference-side: Focus ───────────────────────────────────────────

/// Read-side focus queries.
pub trait RefFocus {
    /// Currently focused block in `region`. Wide PBT: per-region map;
    /// pure slice: returns from a single field.
    fn current_focus(&self, region: CapRegion) -> Option<CapBlockId>;

    /// Cursor position of the focused block's editor (if known).
    fn focused_cursor(&self, region: CapRegion) -> Option<CapCursor>;
}

/// Focus mutations.
pub trait RefFocusMut: RefFocus {
    /// Set focus to `id` in `region`, resetting cursor to `cursor`.
    fn set_focus(&mut self, region: CapRegion, id: CapBlockId, cursor: CapCursor);

    /// Clear focus if it currently points at a now-deleted block.
    fn clear_focus_if_deleted(&mut self, id: &CapBlockId);
}

// ─── Reference-side: Lifecycle (admin gates) ─────────────────────────

/// Setup/lifecycle predicates that wide-PBT transitions gate on.
/// Pure-slice impls return constants (always started, always set up,
/// loro off for pure-logic-only).
pub trait RefLifecycle {
    fn app_started(&self) -> bool;
    fn is_properly_setup(&self) -> bool;
    fn enable_loro(&self) -> bool;

    /// The previous-transition kind, for Markov weighting. Returns
    /// `None` on the first step or when the impl doesn't track history.
    fn last_transition_kind(&self) -> Option<&'static str>;

    /// Mirror of `ReferenceState::atomic_editor_enabled` (env-var gated).
    /// Pure slice always returns `true` — pure-logic editor is the
    /// reason the slice exists.
    fn atomic_editor_enabled() -> bool
    where
        Self: Sized;
}

// ─── SUT-side traits (mirror of reference-side write traits) ─────────

/// SUT mutations on the block tree. Methods do NOT take `ref_state` —
/// concrete impls (e.g. wide-PBT `E2ESut`) keep any needed ref→SUT id
/// mapping in interior state (e.g. `doc_uri_map`).
#[allow(async_fn_in_trait)]
pub trait SutBlockTreeWrite {
    async fn apply_split_block(&mut self, id: &CapBlockId, position: usize);
    async fn apply_join_block(&mut self, id: &CapBlockId);
    async fn apply_indent(&mut self, id: &CapBlockId);
    async fn apply_outdent(&mut self, id: &CapBlockId);
    async fn apply_move_up(&mut self, id: &CapBlockId);
    async fn apply_move_down(&mut self, id: &CapBlockId);
}

#[allow(async_fn_in_trait)]
pub trait SutEditorMirrorWrite {
    async fn apply_type_chars(&mut self, text: &str);
    async fn apply_delete_backward(&mut self, count: usize);
    async fn apply_move_cursor(&mut self, byte_position: usize);
}

#[allow(async_fn_in_trait)]
pub trait SutFocusWrite {
    async fn apply_navigate_focus(&mut self, region: CapRegion, id: &CapBlockId);
    async fn apply_focus_editable_text(&mut self, id: &CapBlockId);
}

/// Uniform quiescence abstraction. Pure slice: no-op. Wide PBT: drains
/// CDC, flushes reactive engine, awaits Loro sync.
#[allow(async_fn_in_trait)]
pub trait SutQuiesce {
    async fn quiesce(&mut self);
}

/// Umbrella trait for the seven T0 transitions' SUT target. Blanket-impl
/// so any `S` satisfying the four constituent traits is automatically a
/// `SutTransitionTarget`. Keeps `apply_to_sut` `where` clauses tight.
pub trait SutTransitionTarget:
    SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce
{
}

impl<T> SutTransitionTarget for T where
    T: ?Sized + SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce
{
}

// ─── Phase 6a — Loro cluster (Stage B) ───────────────────────────────
//
// Peer-Loro transitions: AddPeer, PeerEdit, PeerCharEdit, SyncWithPeer,
// MergeFromPeer, CreateStaleLoro. Surface intentionally avoids the
// integration-tests crate's `PeerEditOp` enum — the trait uses scalar +
// owned-String params so pbt-core stays dep-free of holon-api.

/// Reference-side peer-Loro read surface. Wide PBT impl delegates to
/// `ReferenceState::peers`; pure slice has no peers (returns `0`/empty).
pub trait RefPeers {
    fn peers_len(&self) -> usize;

    /// Stable IDs (peer-internal, NOT EntityUri) the peer currently holds.
    fn peer_block_stable_ids(&self, peer_idx: usize) -> Vec<String>;

    /// Content of a peer's block by its stable id.
    fn peer_block_content(&self, peer_idx: usize, stable_id: &str) -> Option<String>;

    /// Parent stable id of a peer's block (None for root-level peer blocks).
    fn peer_block_parent(&self, peer_idx: usize, stable_id: &str) -> Option<String>;
}

/// Reference-side peer-Loro write surface.
pub trait RefPeersMut: RefPeers {
    /// Snapshot the primary's non-seed, non-page blocks into a new peer.
    /// Wide PBT computes the snapshot from `ReferenceState::block_state`;
    /// pure slice no-ops (returns peer_id=0).
    fn add_peer_from_primary_snapshot(&mut self) -> u64;

    fn peer_apply_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    );

    fn peer_apply_update(&mut self, peer_idx: usize, stable_id: &str, content: &str);

    fn peer_apply_delete(&mut self, peer_idx: usize, stable_id: &str);

    /// Codepoint-level insert into a peer's block content (PeerCharEdit).
    fn peer_apply_char_insert(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    );

    fn peer_apply_char_delete(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    );

    /// Propagate primary's current state to peer (SyncWithPeer).
    fn peer_sync_from_primary(&mut self, peer_idx: usize);

    /// Propagate peer's pending edits back into primary (MergeFromPeer).
    fn peer_merge_into_primary(&mut self, peer_idx: usize);
}

/// SUT-side peer-Loro write surface. Methods are `async` because the
/// wide-PBT SUT performs real LoroDoc imports/exports + reactive-engine
/// quiescence between ops.
#[allow(async_fn_in_trait)]
pub trait SutLoro {
    async fn apply_add_peer(&mut self);

    async fn apply_peer_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    );

    async fn apply_peer_update(&mut self, peer_idx: usize, stable_id: &str, content: &str);

    async fn apply_peer_delete(&mut self, peer_idx: usize, stable_id: &str);

    async fn apply_peer_char_insert(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    );

    async fn apply_peer_char_delete(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    );

    async fn apply_sync_with_peer(&mut self, peer_idx: usize);

    async fn apply_merge_from_peer(&mut self, peer_idx: usize);

    /// Construct a fresh peer holding a STALE snapshot (lag-N export).
    /// Wide PBT replays N pre-recorded snapshots; pure slice no-ops.
    async fn apply_create_stale_loro(&mut self, lag_steps: usize);
}

/// Read-side observation of Loro state for invariants.
/// Phase 7 will bind `inv-loro-no-errors`, `inv-live-children-match-ref`
/// on this trait.
#[allow(async_fn_in_trait)]
pub trait SutLoroLog {
    /// True if the LoroSyncController logged any error since startup.
    async fn loro_had_errors(&self) -> bool;

    /// Snapshot of Loro tree children for a parent — stable-id order.
    /// `None` if the parent isn't represented in Loro.
    async fn loro_children_of(&self, parent_stable_id: &str) -> Option<Vec<String>>;
}

// ─── Phase 6b — Turso/CDC cluster (Stage B) ──────────────────────────
//
// Binds: WriteOrgFile, BulkExternalAdd, all matview-touching invariants
// (`inv-matview-consistent-with-ref`, `inv-watch-rows-match-ref`,
// `inv-focus-roots`, `inv-backend-blocks-match-ref` Turso side,
// `inv-sql-budget`). Required by Phase 8 storage-consistency slice.

/// SUT-side SQL projection read surface. Methods reflect Turso state
/// AFTER CDC quiescence — invariants must call `quiesce()` first.
#[allow(async_fn_in_trait)]
pub trait SutSqlProjection {
    /// Read a hydrated `block` matview row by id. `None` = row not
    /// present (deleted or never inserted). The flat Vec is the row's
    /// fields as Strings in matview-column-declaration order — concrete
    /// impls expose accessor helpers; the trait surface stays generic.
    async fn block_row(&self, id: &CapBlockId) -> Option<Vec<String>>;

    /// All non-deleted block IDs visible in the projection.
    async fn all_block_ids(&self) -> BTreeSet<CapBlockId>;

    /// Row count for a watched query (used by `inv-watch-rows-match-ref`).
    async fn watch_row_count(&self, query_id: &str) -> Option<usize>;

    /// Raw block table read (no matview hydration). Used by WARN/SKIP
    /// classifier's `block_raw` truth-check.
    async fn block_raw_row(&self, id: &CapBlockId) -> Option<Vec<String>>;
}

/// SUT-side write surface for org-file-driven mutations (WriteOrgFile,
/// BulkExternalAdd). External-source-of-truth path that bypasses the
/// reactive engine and writes via OrgFileWatcher.
#[allow(async_fn_in_trait)]
pub trait SutOrgFileWrite {
    /// Write `contents` to `path`. Wide-PBT impl invokes the real
    /// OrgFileWatcher's scan; pure slice writes to an in-memory map.
    async fn write_org_file(&mut self, path: &str, contents: &str);
}

/// SUT-side CDC observation surface.
#[allow(async_fn_in_trait)]
pub trait SutCdc {
    /// True if any CDC stage is mid-flight (used by `live_blocks_stale`
    /// classifier). Wide PBT: checks WatermarkState; pure slice: false.
    async fn cdc_in_flight(&self) -> bool;

    /// Drain pending CDC events into the projection. Idempotent.
    async fn drain_cdc(&mut self);
}

// ─── Phase 6c — ViewModel/Renderer cluster ───────────────────────────
//
// Binds: ViewModel-touching invariants (`inv-viewmodel-*`,
// `inv-frontend-root-not-error`). Pure slice doesn't bind this.

#[allow(async_fn_in_trait)]
pub trait SutViewModel {
    /// Drain pending ViewModel emissions. Drain-once semantics —
    /// after drain, subsequent calls return `Vec::new` until next emit.
    /// Phase 7 `CachingProxy` memoizes this per-tick.
    async fn drain_vm_emissions(&mut self) -> Vec<String>;

    /// True if the frontend root ViewModel is the Error variant.
    async fn frontend_root_is_error(&self) -> bool;
}

#[allow(async_fn_in_trait)]
pub trait SutRenderer {
    /// Stringified render-tree for a block id (debug-formatted).
    /// Used by `inv-displayed-text` and OrgRender fixed-point checks.
    async fn render_tree_of(&self, id: &CapBlockId) -> Option<String>;
}

// ─── Phase 6d — Layout/Bounds cluster ────────────────────────────────
//
// Re-export trait over `holon_pbt_core::user_driver::UserDriver` geometry
// methods. Phase 7 binds `inv-frontend-bounds-*`,
// `inv-editable-text-has-draggable`, `inv-frontend-no-error-widgets`.

#[allow(async_fn_in_trait)]
pub trait SutLayout {
    /// True if a widget for `id` is currently registered with bounds.
    async fn has_registered_bounds(&self, id: &CapBlockId) -> bool;

    /// True if a draggable handle is wired for `id`.
    async fn has_draggable_handle(&self, id: &CapBlockId) -> bool;

    /// True if any rendered widget is an Error variant.
    async fn any_error_widget(&self) -> bool;
}

// ─── Phase 6e — Driver cluster ───────────────────────────────────────
//
// Re-export of `UserDriver` input methods. Phase 7 binds
// `inv-focus-matches-ref`. Driver methods are already trait-bound;
// this re-export keeps slice opt-in symmetric with the other clusters.

#[allow(async_fn_in_trait)]
pub trait SutDriver {
    async fn driver_send_key_chord(&mut self, chord: &str);
    async fn driver_click(&mut self, id: &CapBlockId);
    async fn driver_current_focus(&self) -> Option<CapBlockId>;
    /// The globally focused block id as tracked by the reactive/frontend
    /// engine (distinct from the per-region SQL `current_focus` matview).
    /// Set by click handlers; read by `inv-focus-matches-ref`.
    /// Returns `None` when no frontend engine is installed (SqlOnly mode).
    async fn engine_focused_block(&self) -> Option<CapBlockId>;
    /// Translate a reference-model block id (which may be a synthetic URI
    /// like `block:ref-doc-0`) to the resolved UUID-based id that the SUT
    /// engine tracks. Wide PBT: delegates to `E2ESut::resolve_uri` via
    /// `doc_uri_map`; pure slice: returns the id unchanged (no synthetic URIs).
    fn resolve_ref_block_id(&self, id: &CapBlockId) -> CapBlockId;
}

// ─── Phase 6f — OrgRender cluster ────────────────────────────────────
//
// Binds: `inv-org-render-fixed-point`.

#[allow(async_fn_in_trait)]
pub trait SutOrgRender {
    /// Render the current document set to org-mode text. Used by the
    /// fixed-point invariant: `parse(render(parse(text))) == parse(text)`.
    async fn render_documents_to_org(&self) -> Vec<(String, String)>;
}

// ─── Phase 6g — QueryCompile cluster ─────────────────────────────────
//
// Bound by GENERATORS that synthesize query-content blocks (PRQL/SQL/GQL
// `query_source`). Transitions creating query-bearing blocks gate on
// this; slices without it produce no query-content. No invariants today.

#[allow(async_fn_in_trait)]
pub trait SutQueryCompile {
    /// Compile a query source string to its canonical form. `Err` on
    /// parse/typecheck failure. Generators use this to filter the
    /// proposed query string space to valid inputs.
    async fn compile_query(&self, language: &str, source: &str) -> Result<String, String>;
}

// ─── Phase 6h — Lifecycle cluster (discovered P1.2) ──────────────────
//
// SUT-side counterpart to RefLifecycle. Wide PBT: real app start; pure
// slice: synchronous no-op. Phase 7 binds the `app_started`/setup gates
// invariants reference today.

#[allow(async_fn_in_trait)]
pub trait SutLifecycle {
    async fn apply_start_app(&mut self);
    async fn apply_simulate_restart(&mut self);
    async fn is_app_started(&self) -> bool;
}

// ─── Reference-side: extended caps added in Phase 7 (Stage B) ───────
//
// These traits surface `ReferenceState` fields that are needed by the
// deferred invariant bodies. Each is a thin read-only projection; the
// blanket impl in `reference_capabilities.rs` delegates directly to the
// corresponding field/method on `ReferenceState`.

/// Focus-roots expected by the reference model — per-region set of
/// block ids that the reactive engine should use as pin roots.
pub trait RefFocusRoots {
    /// Expected focus-root block ids for `region`. Wide PBT reads from
    /// `ReferenceState::expected_focus_root_ids`; pure slice: empty set.
    fn expected_focus_root_ids(&self, region: CapRegion) -> BTreeSet<CapBlockId>;
}

/// Layout-block metadata needed by matview + ViewModel invariants.
pub trait RefLayout {
    /// All block ids that are part of the layout scaffolding (headline,
    /// query-source, render-source). `is_layout_block` on `RefBlockTree`
    /// is the per-id predicate; this gives the full set for iteration.
    fn layout_block_ids(&self) -> BTreeSet<CapBlockId>;

    /// Block ids of the active profile blocks (from `profile_block_ids`).
    fn profile_block_ids(&self) -> BTreeSet<CapBlockId>;

    /// True if the test has an active "block" profile override.
    /// Wide PBT: `ReferenceState::has_blocks_profile()`; pure slice: `false`.
    fn has_blocks_profile(&self) -> bool;
}

/// Render-expression metadata exposed for ViewModel invariants.
pub trait RefRender {
    /// Name of the active render expression for `region` (e.g. "tree",
    /// "list"). `None` when no render source block is set up yet.
    /// Wide PBT: `ReferenceState::active_render_expr_name(region)`.
    fn active_render_expr_name(&self, region: CapRegion) -> Option<String>;

    /// True if the reference model has a root render expression at all.
    /// Invariants gate on this before inspecting ViewModel structure.
    fn has_root_render_expr(&self) -> bool;
}

/// Active watched queries on the reference model.
pub trait RefWatches {
    /// Query ids of currently registered watches (stable, sorted).
    /// Wide PBT: keys of `ReferenceState::active_watches`; pure slice: empty.
    fn active_watch_ids(&self) -> Vec<String>;
}

/// Global engine-focused block (distinct from the per-region navigation
/// focus). Set by click handlers in the reactive engine; read by
/// `inv-focus-matches-ref` to compare against `ReactiveEngine::focused_block`.
pub trait RefGlobalFocus {
    /// The globally focused block id, or `None` if nothing is focused.
    /// Wide PBT: `ReferenceState::focused_block`; pure slice: `None`.
    fn global_focused_block(&self) -> Option<CapBlockId>;
}

/// Task-state read-side projection. Used by `inv-viewmodel-state-toggle-correct`
/// to compare block task_state values against ViewModel StateToggle nodes.
pub trait RefTaskState {
    /// Task state string for `id` (`"TODO"`, `"DONE"`, etc.), or `None`
    /// if the block has no task_state property.
    fn task_state_of(&self, id: &CapBlockId) -> Option<String>;
}

// ─── Cross-cut helpers ───────────────────────────────────────────────

/// Cross-cut helper used by `TypeChars::apply_to_ref` and
/// `DeleteBackward::apply_to_ref` when Loro is enabled. Reads the active
/// editor's pending text, commits it to `block_content` of the focused
/// block. Lifted from `ReferenceState::commit_active_editor_if_changed`.
///
/// Returns `true` if a commit happened; `false` if no editor was active
/// or content already matched.
pub fn commit_active_editor_if_changed<R>(state: &mut R) -> bool
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus,
{
    let (block_id, text) = match (
        state.active_editor_block(),
        state.active_editor_text().map(|s| s.to_owned()),
    ) {
        (Some(id), Some(t)) => (id, t),
        _ => return false,
    };
    let current = state.block_content(&block_id).map(|s| s.to_owned());
    if current.as_deref() == Some(&text) {
        return false;
    }
    state.set_block_content(&block_id, &text);
    true
}

// ─── Additional cross-cuts discovered in Phase 1 (P1.3 spike) ────────
//
// Candidates known from code reading:
// - focus-shift on tree mutation (Indent/Outdent/Split do
//   `state.focused_block = Some(new_id)` after mutation — pattern
//   already in `split_block.rs:123-129`). Currently inlined per
//   transition; could become a free function
//   `fn refocus_after_split<R: RefFocusMut>(state: &mut R, new_id: CapBlockId, region: CapRegion)`.
// - sibling re-key on join (join_block mutates parent's child order;
//   pure-slice impl can keep a Vec and recompute, wide PBT uses
//   gen_key_between).
// - descendant invalidation on outdent (Outdent moves a block up a
//   level; any cached descendant set goes stale).
//
// Final enumeration is a P1.3 deliverable. Trait surface above is a
// minimum-viable set — additions widen it, not break it.
