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
use std::time::Duration;

pub use holon_api::EdgeFieldUpdate;
pub use holon_api::EntityUri;

/// Block identifier carried in capability-trait signatures. Aliased to
/// the real domain type [`holon_api::EntityUri`] — the wide PBT and the
/// pure slice both construct ids via `EntityUri::parse` / `EntityUri::block`,
/// so no boundary translation is needed. Kept as an alias (rather than a
/// bare `EntityUri`) so the capability surface reads as "block id" at the
/// call sites and so the type can be revisited centrally.
pub type CapBlockId = holon_api::EntityUri;

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
///
/// `#[capmap_adapter]` hosts this on `CapMap`. The trait is fully sync, so no
/// `#[async_trait]` wrapper is emitted (existing `impl RefBlockTree for
/// ReferenceState` is untouched); the borrow-returning `block_content ->
/// Option<&str>` forwards through `CapMap::expect_ref` so it doesn't dangle.
#[holon_macros::capmap_adapter] // sync trait → no async-trait; emits CapName + `impl … for CapMap`
pub trait RefBlockTree {
    /// Returns block content text. `None` if the block does not exist.
    fn block_content(&self, id: &EntityUri) -> Option<&str>;

    /// True if the block exists and is a Text-typed block (the only kind
    /// editor transitions care about).
    fn is_text_block(&self, id: &EntityUri) -> bool;

    /// Editable Text descendants of the focus root in `Main` region.
    /// Empty in pure slice if the test fixture didn't seed any.
    fn main_editable_descendants(&self) -> Vec<EntityUri>;

    /// Block ids of the current focus roots in `region`. Wide PBT
    /// computes from `expected_focus_root_ids`; pure slice may just
    /// return the root id of its single doc.
    fn focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri>;

    /// Sibling navigation.
    fn previous_sibling(&self, id: &EntityUri) -> Option<EntityUri>;
    fn next_sibling(&self, id: &EntityUri) -> Option<EntityUri>;

    /// Parent of `id`. `None` if `id` is root or has a sentinel parent
    /// (wide PBT: `EntityUri::is_no_parent` / `is_sentinel`; pure slice:
    /// `parent: None`).
    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri>;

    /// Grandparent for outdent.
    fn grandparent(&self, id: &EntityUri) -> Option<EntityUri>;

    /// Children of a parent, sorted by sort_key. Returns ids only —
    /// callers join back through `block_content` if they need data.
    fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri>;

    /// True if `id` is a descendant of any ancestor in `ancestors`.
    fn is_descendant_of_any(&self, id: &EntityUri, ancestors: &BTreeSet<EntityUri>) -> bool;

    /// Layout blocks (the layout scaffolding the user can't focus into).
    /// Wide PBT: `layout_blocks` set; pure slice: empty.
    fn is_layout_block(&self, id: &EntityUri) -> bool;

    /// True if `id` exists and is focusable (i.e. not a layout block,
    /// not immutable, has the right content type).
    fn is_focusable(&self, id: &EntityUri) -> bool;

    /// True if `id` is in the "no content update" set — render sources,
    /// query sources, profile blocks. Wide PBT consults
    /// `layout_blocks.render_source_ids` + `layout_blocks.query_source_ids`
    /// + `profile_block_ids`. Pure slice has no such concept → returns `false`.
    fn is_no_content_update(&self, id: &EntityUri) -> bool;

    /// True if `id` is a Page block (tagged `Page`). Mirrors
    /// `Block::is_page()`. Pure slice has no pages → returns `false`.
    fn is_page_block(&self, id: &EntityUri) -> bool;

    /// All block ids tracked by the reference model, EXCLUDING seed
    /// blocks (those with sentinel/no_parent docs — they're inserted
    /// via direct SQL, never reverse-synced to Loro, and don't appear
    /// in the matview the wide PBT compares against).
    ///
    /// Used by `inv-block-ids-match-ref` to compare against
    /// `SutSqlProjection::all_block_ids()` for set-equality drift
    /// detection at the storage layer.
    fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri>;

    /// True if `id`'s content type makes its *sibling order* non-canonical:
    /// `Source` / `Image` render artifacts (`::src::`, `::render::`) whose
    /// relative order legitimately differs between the SQL projection (ordered
    /// by `sort_key`) and the ref model (ordered by `(sequence, id)`) after a
    /// file-sync round trip reassigns sort_keys. `inv-live-children-match-ref`
    /// uses this to exempt intra-source-group *reordering* — membership is
    /// still enforced, only order is relaxed.
    ///
    /// Default `false` (pure slices have no source/image render artifacts).
    fn is_order_exempt_sibling(&self, _: &EntityUri) -> bool {
        false
    }
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
    fn set_block_content(&mut self, id: &EntityUri, text: &str);

    /// Split `id` at `position`. Returns the id of the newly-created
    /// block holding the tail.
    fn split_block(&mut self, id: &EntityUri, position: usize) -> EntityUri;

    /// Join `id` into its previous sibling (or parent if no previous
    /// sibling). Returns the cursor position of the join point in the
    /// merged block's content.
    fn join_block(&mut self, id: &EntityUri) -> usize;

    /// Indent `id` — re-parent under previous sibling.
    fn indent(&mut self, id: &EntityUri);

    /// Outdent `id` — move up to grandparent level.
    fn outdent(&mut self, id: &EntityUri);

    /// Re-parent `id` under `new_parent`, placing it after `after` (or
    /// first if `after` is None). Used by Indent/Outdent helpers when
    /// they don't want to bake the parent-discovery logic into the
    /// transition body. The wide-PBT impl is
    /// `ReferenceState::move_block`.
    fn move_block(&mut self, id: &EntityUri, new_parent: EntityUri, after: Option<&EntityUri>);

    /// Swap two siblings (used by MoveUp / MoveDown).
    fn swap_siblings(&mut self, a: &EntityUri, b: &EntityUri);
}

// ─── Reference-side: EditorMirror ────────────────────────────────────

/// Read-side active-editor state.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync trait → no `#[async_trait]`,
/// `impl RefEditorMirror for ReferenceState` untouched). The borrow-returning
/// `active_editor_text -> Option<&str>` forwards through `CapMap::expect_ref`.
#[holon_macros::capmap_adapter]
pub trait RefEditorMirror {
    /// Block id whose editor is currently active, or `None` if no editor
    /// is open. Pure slice typically has this populated by a setup
    /// transition; wide PBT mirrors GPUI's `InputState`.
    fn active_editor_block(&self) -> Option<EntityUri>;

    /// Live in-memory editor text. Pre-blur, this can diverge from
    /// `block_content(active_editor_block())` — the divergence is what
    /// surfaces split-with-pending-edit bugs.
    fn active_editor_text(&self) -> Option<&str>;

    /// Cursor byte offset within `active_editor_text`.
    fn active_editor_cursor(&self) -> Option<usize>;

    /// True iff modeled typing/deleting touched the active editor's text
    /// since it opened (or since the last commit). Distinguishes
    /// user-authored pending text (commits on blur / at structural commit
    /// points) from a mirror that merely went stale against an external
    /// change (prod's data subscription refreshes idle editors; committing
    /// a stale mirror writes old text into the ref). Default `false` keeps
    /// lean slice models, which never type, on the never-commits path.
    fn active_editor_dirty(&self) -> bool {
        false
    }
}

/// Editor-mirror mutations. Apply to whichever editor is active.
pub trait RefEditorMirrorMut: RefEditorMirror {
    fn type_chars(&mut self, text: &str);
    fn delete_backward(&mut self, count: usize);
    fn move_cursor(&mut self, byte_position: usize);

    /// Clear the dirty flag after a commit. Default no-op for models
    /// without dirty tracking.
    fn mark_active_editor_committed(&mut self) {}
}

// ─── Reference-side: Focus ───────────────────────────────────────────

/// Read-side focus queries.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (fully sync, owned returns → no
/// `#[async_trait]` wrapper; the existing `impl RefFocus for ReferenceState` is
/// untouched). Needed so `inv-navigation-focus` / `inv-focus-roots` can select
/// on `CapId::of::<dyn RefFocus>()` from a composed ref `CapMap`.
#[holon_macros::capmap_adapter]
pub trait RefFocus {
    /// Expected focus-root ids per region as `(region_string, [root_id])`, for
    /// `inv-focus-roots`. Region strings match the `focus_roots` matview;
    /// already resolved into SUT id space by `with_resolved_doc_uris` (the
    /// `open_pins` block_ids it derives from are remapped there).
    fn expected_focus_root_rows(&self) -> Vec<(String, Vec<String>)>;

    /// Per-region navigation focus as `(region_string, block_id_string)` for
    /// the regions the reference has navigation history for, keyed by the SQL
    /// region strings (matching the `current_focus` matview). `block_id` is
    /// `None` for a region navigated home. Already resolved into SUT id space
    /// by `with_resolved_doc_uris`. Used by `inv-navigation-focus`, which needs
    /// LeftSidebar/RightSidebar granularity that [`CapRegion`] collapses, so it
    /// keys by string rather than `CapRegion`.
    fn navigation_focus_rows(&self) -> Vec<(String, Option<String>)>;

    /// Currently focused block in `region`. Wide PBT: per-region map;
    /// pure slice: returns from a single field.
    fn current_focus(&self, region: CapRegion) -> Option<EntityUri>;

    /// Cursor position of the focused block's editor (if known).
    fn focused_cursor(&self, region: CapRegion) -> Option<CapCursor>;
}

/// Focus mutations.
pub trait RefFocusMut: RefFocus {
    /// Set focus to `id` in `region`, resetting cursor to `cursor`.
    fn set_focus(&mut self, region: CapRegion, id: EntityUri, cursor: CapCursor);

    /// Clear focus if it currently points at a now-deleted block.
    fn clear_focus_if_deleted(&mut self, id: &EntityUri);

    /// Open an active editor on `id` with `content` and the caret at
    /// `cursor_byte`, replacing any prior active editor. Mirrors prod's split
    /// focus (ADR 0010): `split_block` returns the freshly-created block as the
    /// focus target at position 0 (op response, applied in-process), so a
    /// *subsequent* Enter splits the NEW block — not the block the prior
    /// `FocusEditableText` targeted.
    /// Without this the ref leaves `active_editor` stale and `PressKey(Enter)`
    /// splits the wrong block, diverging from prod (and the headless SUT once
    /// its `focused_block` settles). Default no-op for pure-slice reference
    /// machines that have no editor state.
    fn open_active_editor(&mut self, _: EntityUri, _: String, _: usize) {}

    /// Close the active editor (e.g. after a Backspace-at-0 join deletes the
    /// edited block — prod closes that block's editor). Counterpart of
    /// [`Self::open_active_editor`]; default no-op for editor-less refs.
    fn close_active_editor(&mut self) {}
}

// ─── Reference-side: Lifecycle (admin gates) ─────────────────────────

/// Setup/lifecycle predicates that wide-PBT transitions gate on.
/// Pure-slice impls return constants (always started, always set up,
/// loro off for pure-logic-only).
pub trait RefLifecycle {
    fn app_started(&self) -> bool;
    fn is_properly_setup(&self) -> bool;
    fn enable_loro(&self) -> bool;

    /// Whether this reference owns an **editor buffer** carrying uncommitted
    /// text — the headless atomic-editor capability. This is the single gate
    /// for the editor transitions (TypeChars / DeleteBackward / MoveCursor /
    /// FocusEditableText / PressKey), replacing the old pairing of
    /// `atomic_editor_enabled()` (env-var gated) and `enable_loro() ||
    /// real_editor_enabled()` (storage-coupled). Editor buffering is a property
    /// of the wired editor component, independent of *Loro-as-storage* (the CRDT
    /// can buffer text regardless of where blocks persist) and of any process
    /// env var. Defaults to `false`: a ref with no editor buffer never
    /// generates editor transitions.
    fn has_editor_buffer(&self) -> bool {
        false
    }

    /// Whether a block-interaction transition (indent / drag / chord / …) can
    /// dispatch against `block_id` under the active main-panel layout: the block
    /// must be in the layout query's rendered set AND rendered with an
    /// interactive widget.
    ///
    /// The default layout queries `focus_root` (navigation-aware, transitive)
    /// and renders each block via `render_entity()` (operations + `draggable` +
    /// `editable_text`), so any focused-subtree block qualifies. A user
    /// `index.org` layout renders a possibly-different set (a `from children`
    /// query surfaces only the layout block's direct children; an all-blocks
    /// query surfaces everything) through a possibly-static template
    /// (`row(text(...))`, no operations) — the reference evaluates BOTH axes
    /// faithfully (see `ReferenceState::renders_block_interactively`) rather than
    /// blanket-excluding every custom layout. Defaults to `true`;
    /// `ReferenceState` overrides it.
    fn renders_block_interactively(&self, block_id: &EntityUri) -> bool {
        let _ = block_id;
        true
    }

    /// The previous-transition kind, for Markov weighting. Returns
    /// `None` on the first step or when the impl doesn't track history.
    fn last_transition_kind(&self) -> Option<&'static str>;
}

// ─── SUT-side traits (mirror of reference-side write traits) ─────────

/// SUT mutations on the block tree. Methods do NOT take `ref_state` —
/// concrete impls (e.g. wide-PBT `E2ESut`) keep any needed ref→SUT id
/// mapping in interior state (e.g. `doc_uri_map`).
///
/// `&self` + interior mutability (not `&mut self`): the underlying stores are
/// already interior-mutable (`MemoryBackend` = `Arc<RwLock>`, `E2ESut`'s
/// `Arc<Mutex>` fields), so `#[capmap_adapter]` hosts the write cap on `CapMap`
/// exactly like the read caps — the composed slice's `CapMap` *is* the
/// `SutTransitionTarget`.
#[holon_macros::capmap_adapter]
pub trait SutBlockTreeWrite {
    async fn apply_split_block(&self, id: &EntityUri, position: usize);
    async fn apply_join_block(&self, id: &EntityUri);
    async fn apply_indent(&self, id: &EntityUri);
    async fn apply_outdent(&self, id: &EntityUri);
    async fn apply_move_up(&self, id: &EntityUri);
    async fn apply_move_down(&self, id: &EntityUri);
}

/// SUT capability: write a block **edge field** (`tags`/`requires`, the
/// junction-backed set-valued attributes) on an EXISTING block, parameterized
/// over *which* field via [`EdgeFieldUpdate`] so neither is special-cased.
///
/// `&self` + interior mutability like the other write caps, so `#[capmap_adapter]`
/// hosts it on `CapMap` — the composed `CapMap` IS the `SutTransitionTarget`. The
/// realization calls the production edge-field writers (`set_block_tags` /
/// `set_block_requires` on the Loro backend — the same functions the org re-scan
/// reconciliation uses), so the write flows Loro → `project()` → SQL exactly as
/// production does. That is what lets the composed `/matview` invariant observe a
/// dropped edge-field re-projection (e.g. H12: `blocks_differ` omitting `requires`).
#[holon_macros::capmap_adapter]
pub trait SutEdgeFieldWrite {
    async fn apply_set_edge_field(&self, id: &EntityUri, update: &EdgeFieldUpdate);
}

#[holon_macros::capmap_adapter]
pub trait SutEditorMirrorWrite {
    async fn apply_type_chars(&self, text: &str);
    async fn apply_delete_backward(&self, count: usize);
    async fn apply_move_cursor(&self, byte_position: usize);
}

/// Drive an `Arc`-shared write component through `SutTransitionTarget::apply_to_sut`
/// (which takes `&mut S`). The editor write component is held behind `Arc` because
/// the SAME instance is also registered as the read-side `SutEditorMirrorRead` cap;
/// the write methods are `&self` (interior mutability), so forwarding through the
/// shared `Arc` is sound.
#[async_trait::async_trait(?Send)]
impl<T: SutEditorMirrorWrite + ?Sized> SutEditorMirrorWrite for std::sync::Arc<T> {
    async fn apply_type_chars(&self, text: &str) {
        (**self).apply_type_chars(text).await
    }
    async fn apply_delete_backward(&self, count: usize) {
        (**self).apply_delete_backward(count).await
    }
    async fn apply_move_cursor(&self, byte_position: usize) {
        (**self).apply_move_cursor(byte_position).await
    }
}

/// Read-side editor-mirror state: the SUT's tracked caret byte and live
/// (pre-commit) editor text for a block. `ref_`-side id space is accepted
/// — impls resolve synthetic ids themselves (mirroring
/// `SutDriver::resolve_ref_block_id`). Binds
/// `inv-editor-caret-matches-ref` and `inv-editor-text-matches-ref`.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync, owned `Result` returns →
/// no `#[async_trait]`; existing `E2ESut` impl untouched).
#[holon_macros::capmap_adapter]
pub trait SutEditorMirrorRead {
    /// `Err(reason)` = caret unobservable in this SUT/driver medium (the
    /// invariant reports a disclosed Skip); `Ok(None)` = observable medium
    /// but no caret tracked for this block yet.
    fn editor_caret_byte(&self, block_id: &EntityUri) -> Result<Option<usize>, String>;

    /// The live editor text for `block_id` (the `MutableText`/`InputState`
    /// value keystrokes mutate, which pre-blur can diverge from the
    /// committed block content). `Err(reason)` = unobservable in this
    /// medium / for this block right now (disclosed Skip).
    fn editor_live_text(&self, block_id: &EntityUri) -> Result<String, String>;
}

#[holon_macros::capmap_adapter]
pub trait SutFocusWrite {
    async fn apply_navigate_focus(&self, region: CapRegion, id: &EntityUri);
    async fn apply_focus_editable_text(&self, id: &EntityUri);
}

/// Navigation-history writes (`go_home`/`go_back`/`go_forward`) — distinct from
/// `SutFocusWrite` (focus → a specific block) because these traverse the
/// navigation history. `go_home` is `navigation.focus(region, None)`: it *clears*
/// current focus and the region's open pins (so it moves both current focus and
/// focus roots). `apply_navigate_back`/`apply_navigate_forward` are deferred to the
/// windowed `GpuiWindowComponent` (E4) — headless prod does not yet mirror them
/// (see `maybe_mirror_navigation_focus`), so only `apply_navigate_home` lands now.
#[holon_macros::capmap_adapter]
pub trait SutNavHistoryWrite {
    async fn apply_navigate_home(&self, region: CapRegion);
}

/// Watch registration — the `setup_watch` write path decomposed off `SutHandle`
/// (SutHandle decomposition INC 3). Takes the **already-compiled** query
/// (`source` + `lang`) rather than the integration-test-local `TestQuery`, which
/// pbt-core cannot name; the `SetupWatch` transition compiles `TestQuery` at the
/// boundary (`compile_for`) and passes the result here. The read side
/// (`SutWatchRows`) and the watch invariants already exist and bite headlessly
/// (`frontend_slice` B5 teeth) — this is the missing **write** cap that lets a
/// composed `CapMap` drive `SetupWatch`, so the watch invariants run over a
/// composed-driven watch, not only an `E2ESut`-driven one. `&self` (like every
/// cap): the E2ESut realization is sound because its watch state is now
/// interior-mutable (`TestEnvironment::setup_watch` is `&self`).
#[holon_macros::capmap_adapter]
pub trait SutWatchRegister {
    async fn register_watch(&self, query_id: &str, source: &str, lang: holon_api::QueryLanguage);

    /// Tear down a previously-registered watch by its `query_id`. Drives the
    /// `RemoveWatch` PBT transition.
    async fn unregister_watch(&self, query_id: &str);
}

/// SUT capability: switch the active view/mode by name. Drives the `SwitchView`
/// PBT transition. Names only primitives, so it lives in `holon-pbt-core`.
#[holon_macros::capmap_adapter]
pub trait SutViewControl {
    async fn switch_view(&self, view_name: &str);
}

/// SUT capability: emit the current state over the MCP integration. Drives the
/// `EmitMcpData` PBT transition (no payload, no `ref_state`).
#[holon_macros::capmap_adapter]
pub trait SutMcpEmit {
    async fn emit_mcp_data(&self);
}

/// SUT capability: undo/redo the last committed mutation. Drives the
/// `UndoLastMutation` / `Redo` PBT transitions. The block-convergence settle is
/// `ref_state`-dependent and lives in the harness seam (`block_tree_post_action`),
/// so the cap itself is a pure `&self` action over the engine's undo stack.
#[holon_macros::capmap_adapter]
pub trait SutHistoryWrite {
    async fn undo_last_mutation(&self);
    async fn redo(&self);
}

/// SUT capability: drive nav-history navigation and sidebar pinning through the
/// UI driver (leader chords / synthetic dispatch). Drives the `NavigateBack`,
/// `NavigateForward`, `PinBlock`, `UnpinBlock` PBT transitions. These are
/// driver-realized only (the headless `frontend_slice` does not drive them), so
/// the cap keeps the concrete `holon_api::Region` — `pin_block` forwards the
/// region string into the dispatch params, so the lossy `CapRegion` abstraction
/// would not round-trip.
#[holon_macros::capmap_adapter]
pub trait SutNavHistoryDrive {
    async fn navigate_back(&self, region: holon_api::Region);
    async fn navigate_forward(&self, region: holon_api::Region);
    async fn pin_block(&self, region: holon_api::Region, block_id: &holon_api::EntityUri);
    async fn unpin_block(&self, history_id: i64);
}

/// SUT capability: block-level UI interactions driven through the UI driver —
/// clicking, drag-and-drop re-parenting, expand/collapse chevrons, the slash
/// command menu, and raw key chords. Drives the `ClickBlock`, `DragDropBlock`,
/// `ExpandToggle`, `CollapseToggle`, `TriggerSlashCommand`, `PressKey`
/// transitions. Driver-realized only (no headless `frontend_slice` driver), so
/// `E2ESut` is the sole impl; names only `holon_api` types. The `PressKey`
/// Enter-split reconciliation is `ref_state`-dependent and lives in the harness
/// seam (`block_tree_post_action`), so the cap action is a pure keystroke send.
#[holon_macros::capmap_adapter]
pub trait SutBlockInteract {
    async fn click_block(&self, region: holon_api::Region, block_id: &holon_api::EntityUri);
    async fn drag_drop_block(&self, source: &holon_api::EntityUri, target: &holon_api::EntityUri);
    async fn expand_toggle(&self, block_id: &holon_api::EntityUri);
    async fn collapse_toggle(&self, block_id: &holon_api::EntityUri);
    async fn trigger_slash_command(&self, block_id: &holon_api::EntityUri);
    async fn press_key(&self, chord: &holon_api::KeyChord);
    /// Click a rendered element by its bounds-registry id (a plain block
    /// `EntityUri` or a geometry handle `<kind>::<block-uri>`). Drives the shared
    /// `holon_layout_testing` bodies (`ToggleCollapse`/`ToggleDrawer`/
    /// `SwitchViewMode`) via `SutClickAdapter`.
    async fn click_at_element(&self, element_id: &str);
}

/// Uniform quiescence abstraction. Pure slice: no-op. Wide PBT: drains
/// CDC, flushes reactive engine, awaits Loro sync.
#[holon_macros::capmap_adapter]
pub trait SutQuiesce {
    async fn quiesce(&self);
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

/// Character-level text operations on a peer's LoroText container.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TextOp {
    Insert {
        pos_codepoint: usize,
        text: String,
    },
    Delete {
        pos_codepoint: usize,
        len_codepoint: usize,
    },
}

/// Operations that can be performed on a peer's Loro tree.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PeerEditOp {
    Create {
        parent_stable_id: Option<String>,
        content: String,
        /// Deterministic stable ID from `deterministic_peer_block_id`.
        stable_id: String,
    },
    Update {
        stable_id: String,
        content: String,
    },
    Delete {
        stable_id: String,
    },
}

/// SUT-side peer-Loro write surface. Methods are `async` because the
/// wide-PBT SUT performs real LoroDoc imports/exports + reactive-engine
/// quiescence between ops.
///
/// `&self` (not `&mut self`): the peer mesh's only structurally-mutated state is the
/// `peers` vec (one `push` in `apply_add_peer`, never across an `.await`), so its
/// provider holds it behind interior mutability. This makes the trait object-safe so
/// `#[capmap_adapter]` can host it on `CapMap` (the `&self`/`Arc<dyn SutLoro>` adapter),
/// which is what lets a composed `CapMap` satisfy `SutHandle` (PCG-4).
#[holon_macros::capmap_adapter]
pub trait SutLoro {
    async fn apply_add_peer(&self);

    async fn apply_peer_create(
        &self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    );

    async fn apply_peer_update(&self, peer_idx: usize, stable_id: &str, content: &str);

    async fn apply_peer_delete(&self, peer_idx: usize, stable_id: &str);

    async fn apply_peer_char_insert(
        &self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    );

    async fn apply_peer_char_delete(
        &self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    );

    async fn apply_sync_with_peer(&self, peer_idx: usize);

    async fn apply_merge_from_peer(&self, peer_idx: usize);

    /// Construct a fresh peer holding a STALE snapshot (lag-N export).
    /// Wide PBT replays N pre-recorded snapshots; pure slice no-ops.
    ///
    /// Named distinctly from `SutHandle::apply_create_stale_loro` (the
    /// file-corruption variant) to avoid an ambiguous-method collision now
    /// that `SutHandle: SutLoro`.
    async fn apply_create_stale_peer(&self, lag_steps: usize);

    /// Post-startup: edit a block on a peer's LoroDoc directly.
    async fn apply_peer_edit(&self, peer_idx: usize, op: &PeerEditOp);

    /// Post-startup: edit a block's LoroText container on a peer at character level.
    async fn apply_peer_char_edit(&self, peer_idx: usize, block_id: &str, op: &TextOp);
}

/// App-runtime error log — the SUT's general "did anything error during the
/// run" surface, distinct from the component-specific error checks (the Loro
/// log in [`SutLoroLog`], the ViewModel/frontend error widgets in
/// [`SutViewModel`]/[`SutLayout`]). Today this is the Flutter/event publish
/// errors logged during the initial document sync; `inv-no-errors` asserts the
/// count is zero. This is the home for any future non-component-specific error
/// source.
#[holon_macros::capmap_adapter]
pub trait SutErrorLog {
    /// Number of app-level error events logged since startup.
    async fn app_error_count(&self) -> usize;

    /// Identifiers (document names) present when the errors occurred — context
    /// for the failure message. Empty when there are no errors.
    async fn app_error_context(&self) -> Vec<String>;
}

/// Read-side observation of Loro state for invariants.
/// Phase 7 will bind `inv-loro-no-errors`, `inv-live-children-match-ref`
/// on this trait.
#[holon_macros::capmap_adapter]
pub trait SutLoroLog {
    /// True if the LoroSyncController logged any error since startup.
    async fn loro_had_errors(&self) -> bool;

    /// Snapshot of Loro tree children for a parent — stable-id order.
    /// `None` if the parent isn't represented in Loro.
    async fn loro_children_of(&self, parent_stable_id: &str) -> Option<Vec<String>>;

    /// Every block held in the live Loro tree as typed `Block` values — the
    /// Loro store's contribution to the `inv-blocks-match-ref/loro` composite.
    /// `None` when Loro isn't enabled on this SUT (e.g. the SqlOnly variant),
    /// so the body can `Skip` rather than compare an empty store.
    async fn loro_block_snapshot(&self) -> Option<Vec<holon_api::block::Block>>;
}

// ─── Phase 6b — Turso/CDC cluster (Stage B) ──────────────────────────
//
// Binds: WriteOrgFile, BulkExternalAdd, all matview-touching invariants
// (`inv-matview-consistent-with-ref`, `inv-watch-rows-match-ref`,
// `inv-focus-roots`, `inv-backend-blocks-match-ref` Turso side,
// `inv-sql-budget`). Required by Phase 8 storage-consistency slice.

/// SUT-side SQL projection read surface. Methods reflect Turso state
/// AFTER CDC quiescence — invariants must call `quiesce()` first.
#[holon_macros::capmap_adapter]
pub trait SutSqlProjection {
    /// Read a hydrated `block` matview row by id. `None` = row not
    /// present (deleted or never inserted). The flat Vec is the row's
    /// fields as Strings in matview-column-declaration order — concrete
    /// impls expose accessor helpers; the trait surface stays generic.
    async fn block_row(&self, id: &EntityUri) -> Option<Vec<String>>;

    /// All non-deleted block IDs visible in the projection.
    async fn all_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Child block IDs of `parent` in the SQL projection, ordered by
    /// `sort_key` (the authoritative fractional index). Used by
    /// `inv-live-children-match-ref` to compare per-parent sibling order
    /// against the reference model's `RefBlockTree::sorted_children`.
    async fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri>;

    /// Row count for a watched query (used by `inv-watch-rows-match-ref`).
    async fn watch_row_count(&self, query_id: &str) -> Option<usize>;

    /// Raw block table read (no matview hydration). Used by WARN/SKIP
    /// classifier's `block_raw` truth-check.
    async fn block_raw_row(&self, id: &EntityUri) -> Option<Vec<String>>;

    /// Distinct block_id values present in the `block_tags` junction table.
    /// Used by `inv-block-tags-references-exist` to check for orphan rows
    /// (tag references whose block_id doesn't exist in block_raw).
    async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri>;

    /// `task_state` JSON property for `id` from `block_raw.properties`.
    /// Returns `None` if the block doesn't exist or has no `task_state`
    /// property. Used by `inv-task-state-storage-coherence`.
    async fn block_task_state(&self, id: &EntityUri) -> Option<String>;

    /// `content` column of `block_raw` for `id`. Returns `None` if the block
    /// doesn't exist. Used by `inv-block-content-matches-ref` (split-block
    /// content-routing slice).
    async fn block_content(&self, id: &EntityUri) -> Option<String>;
}

/// SUT-side navigation-focus projection surface (C-5 split off
/// [`SutSqlProjection`], 2026-07-02). Kept a SEPARATE cap so a storage-only slice
/// (no navigation driven, e.g. `sql_slice`) does NOT register it and the
/// focus/navigation invariants honestly DESELECT there — instead of selecting
/// against an honest-empty `SutSqlProjection` focus family and passing vacuously
/// (empty focus vs an unnavigated ref). Registered only where navigation is
/// actually driven through a Turso `current_focus`/`focus_roots`/
/// `navigation_history` projection (the frontend slice / `full_headless`).
/// Methods reflect Turso state AFTER CDC quiescence — invariants `quiesce()` first.
#[holon_macros::capmap_adapter]
pub trait SutFocusProjection {
    /// Rows of the `current_focus` matview as `(region, block_id)`. `block_id`
    /// is `None` for a region navigated home (NULL in SQL). Used by
    /// `inv-navigation-focus` to compare the SUT's per-region navigation focus
    /// against the reference.
    async fn current_focus_rows(&self) -> Vec<(String, Option<String>)>;

    /// Rows of the `focus_roots` matview as `(region, root_id)` — the
    /// convergent truth-check for `inv-focus-roots`' CDC-lag downgrade.
    async fn focus_roots_rows(&self) -> Vec<(String, String)>;

    /// Open rows of the BASE `navigation_history` table as `(region, block_id)`
    /// — exactly the set the `focus_roots` matview projects from
    /// (`WHERE closed_at IS NULL AND block_id IS NOT NULL`). Lets
    /// `inv-focus-roots` distinguish a genuine matview/IVM drift (base no longer
    /// has the row, matview still does) from a holon close-path bug (base still
    /// has the row open, so the matview is *correctly* showing it).
    async fn nav_history_open_rows(&self) -> Vec<(String, String)>;
}

/// SUT-side typed block-snapshot surface for `inv-backend-blocks-match-ref`.
///
/// Deliberately separate from [`SutSqlProjection`]: that trait stays
/// format-agnostic (rows as `Vec<String>`), whereas the backend-blocks
/// invariant needs the deep, per-field comparison that only typed
/// [`holon_api::Block`] values support. Coupling *this* trait to `Block`
/// keeps `SutSqlProjection`'s String surface intact.
#[allow(async_fn_in_trait)]
#[holon_macros::capmap_adapter] // emits async-trait + CapName + `impl … for CapMap`
pub trait SutBackend {
    /// Snapshot of the CDC-driven `block` matview mirror (`live_blocks`)
    /// as fully-hydrated `Block` values. Read AFTER CDC quiescence — the
    /// caller must `quiesce()` first (the wide-PBT runner does, via the
    /// shared invariant prep + convergence wait).
    async fn live_block_snapshot(&self) -> Vec<holon_api::Block>;

    /// Snapshot of the write-side `block_raw` table as `Block` values — the
    /// convergent source of truth before the IVM CDC projection. Carries only
    /// `block_raw`'s native columns (id, parent, content, content_type,
    /// source_language, properties); the junction-derived `tags`/`requires`
    /// are NOT populated, so the `inv-blocks-match-ref/block_raw` store
    /// compares a field SUBSET. Read after `quiesce()`.
    async fn block_raw_snapshot(&self) -> Vec<holon_api::Block>;

    /// Rows of the live `focus_roots` mirror (`LiveData<FocusRoot>`) as
    /// `(region, root_id)` — the CDC-driven mirror `inv-focus-roots` compares,
    /// with the `focus_roots` matview as the CDC-lag truth-check. The mirror is
    /// part of the live-CDC-mirror component alongside `live_block_snapshot`.
    async fn live_focus_root_rows(&self) -> Vec<(String, String)>;
}

/// Loro-side task_state projection. Phase 7 addition for
/// `inv-task-state-storage-coherence`. Separate from `SutLoroLog` to
/// keep the Loro-tree surface (children snapshot) isolated from the
/// property-projection surface.
#[holon_macros::capmap_adapter]
pub trait SutLoroTaskState {
    /// Task state string for `block_id` as projected from the Loro block's
    /// `properties["task_state"]` scalar — the same value the SQL sibling
    /// [`SutSqlProjection::block_task_state`] reads via
    /// `json_extract(properties,'$.task_state')`, so the two are directly
    /// comparable by `inv-task-state-storage-coherence`. `None` when Loro
    /// isn't enabled, the block is absent, or it carries no `task_state`.
    async fn loro_task_state_of(&self, block_id: &str) -> Option<String>;
}

// ─── SutOrgFileWrite ── DELETED (E3, 2026-06-24) ─────────────────────
// Was a thin redundant wrapper that delegated straight to
// `local_caps::SutFixtureFs::write_org_file`; the `WriteOrgFile` transition
// binds `SutFixtureFs` directly, so this coarse trait had zero callers,
// transitions, invariants, or composed hosts. Fully vestigial → removed.

// ─── Phase 6c — ViewModel/Renderer cluster ───────────────────────────
//
// Binds: ViewModel-touching invariants (`inv-viewmodel-*`,
// `inv-frontend-root-not-error`). Pure slice doesn't bind this.

/// Narrow viewport the value-fn provider probe forces before interpreting,
/// so the root layout picks the `if_space`-gated mobile action bar
/// (`focus_chain()` + `ops_of(...)`) on every run instead of only when the
/// generator happens to choose a chain fixture.
#[derive(Debug, Clone, Copy)]
pub struct ViewportHint {
    pub width_px: f32,
    pub height_px: f32,
}

/// Structural report on the streaming `ReactiveRowProvider`s produced by
/// value functions (`focus_chain`, `ops_of`, `chain_ops`) when the root
/// layout is interpreted. Computed SUT-side (the `ReactiveEngine` /
/// `interpret_pure` / `ProviderCache` coupling stays there) so
/// `inv-value-fn-provider-arg-variance-13` can assert purely.
/// Returned by [`SutFrontendEmissions::provider_stability_report`]; `None` from
/// that method means the root is still initializing (loading/spacer).
#[derive(Debug, Clone)]
pub struct ProviderStabilityReport {
    /// The active render_expr mentions `bottom_dock` (inv_bar precondition).
    pub mentions_bottom_dock: bool,
    /// Count of `BottomDock` nodes in the interpreted tree.
    pub bottom_dock_count: usize,
    /// The active render_expr mentions `focus_chain` (arg-variance precondition).
    pub mentions_focus_chain: bool,
    /// Total streaming providers collected in pass 1.
    pub total_providers: usize,
    /// Any pass-1 provider produced rows.
    pub any_nonempty: bool,
    /// `Some(msg)` when a `(template, rows)` group resolved to more than one
    /// `cache_identity` — provider identity instability (vfn12).
    pub identity_instability: Option<String>,
    /// Count of cache identities present in pass 1 but missing in pass 2 —
    /// provider cache flicker across re-interpret (vfn13).
    pub flicker_count: usize,
}

/// A resolved snapshot of the frontend engine's root-layout ViewModel.
/// Returned by [`SutFrontendEngine::frontend_root_vm`]; `None` from that method
/// means "no frontend engine / still loading", so any value here is a
/// settled, non-loading root.
#[derive(Debug, Clone)]
pub struct FrontendRootVm {
    /// The root widget kind (`widget_name`), e.g. `"columns"`, `"table"`.
    /// `"table"` signals the render-expr matview hasn't delivered yet (a
    /// transient loading state the bounds checks gate off of).
    pub root_kind: String,
    /// Entity ids the frontend ViewModel surfaces, in ViewModel order
    /// (`collect_entity_ids`). The geometry y-order / contiguity / coverage
    /// checks compare the rendered elements against this ordering.
    pub entity_ids: Vec<EntityUri>,
}

#[holon_macros::capmap_adapter]
pub trait SutViewModel {
    /// Drain pending ViewModel emissions. Drain-once semantics —
    /// after drain, subsequent calls return `Vec::new` until next emit.
    /// Phase 7 `CachingProxy` memoizes this per-tick.
    async fn drain_vm_emissions(&mut self) -> Vec<String>;

    /// Count Error widget nodes in the headless `ReactiveEngine`'s rendered
    /// ViewModel tree. Returns `None` when the headless engine is not
    /// installed or the tree isn't ready to inspect yet (loading / placeholder
    /// / shadow-interpretation panicked). Returns `Some(n)` otherwise.
    ///
    /// `Some(0)` means "the rendered tree has no Error widgets"; the
    /// `inv-viewmodel-no-error-widgets` body asserts on that.
    async fn headless_error_node_count(&self) -> Option<usize>;

    /// The currently selected view mode (e.g. `"all"`, `"today"`) — UI
    /// view-selection state. `inv-view-selection` compares it to the
    /// reference's [`RefRender::current_view`].
    async fn current_view(&self) -> String;
}

/// Windowed-only root-ViewModel resolution surface (C-5 Tier-2 split off
/// [`SutViewModel`], 2026-07-02). Kept a SEPARATE cap because a live gpui
/// `ReactiveEngine` is the only faithful source: the headless keystone has no
/// window, so [`HeadlessFrontendComponent`] does NOT register this cap and
/// `inv-frontend-engine` / `inv-frontend-root-not-error` honestly DESELECT
/// there (auto-dropped from the `WIDE_REQUIRED_INVARIANTS` floor by the
/// `Needs::selected_against` filter) instead of "running" vacuously against an
/// honest-`None`/`false` shadow. Registered only where a live window's engine
/// is present (the windowed composition). `inv-frontend-bounds-rendered` (the
/// windowed geometry family) also reads `frontend_root_vm` for the entity order
/// its y-order / contiguity / coverage checks compare against.
///
/// [`HeadlessFrontendComponent`]: (test-crate component)
#[holon_macros::capmap_adapter]
pub trait SutFrontendEngine {
    /// Resolve the FRONTEND engine's root-layout ViewModel (the gpui window's
    /// own `ReactiveEngine`) and return its root widget kind plus the ORDERED
    /// entity-id list it surfaces. `None` when the root is still loading.
    ///
    /// Read by `inv-frontend-engine` (resolution liveness) and
    /// `inv-frontend-bounds-rendered` (the entity order the geometry y-order /
    /// contiguity / coverage checks compare against).
    async fn frontend_root_vm(&self) -> Option<FrontendRootVm>;

    /// True if the frontend root ViewModel is the Error variant.
    /// Drives `inv-frontend-root-not-error`.
    async fn frontend_root_is_error(&self) -> bool;
}

/// Windowed-only ViewModel streaming-emission observer surface (C-5 Tier-1
/// split off [`SutViewModel`], 2026-07-02). These three methods reconstruct the
/// intermediate-emission / provider-cache / live-tree behaviour of the GPUI
/// frontend directly from a live `ReactiveEngine`; a headless slice with no
/// window genuinely has no such emission surface, so
/// [`HeadlessFrontendComponent`] does NOT register this cap and the three
/// value-fn / live-tree invariants honestly DESELECT there instead of
/// "running" vacuously against honest-`None`/`[]` shadows. Registered only
/// where a live window's engine is present (the windowed composition), where
/// the checks have real teeth over the actual render pipeline.
///
/// [`HeadlessFrontendComponent`]: (test-crate component)
#[holon_macros::capmap_adapter]
pub trait SutFrontendEmissions {
    /// Force `viewport`, interpret the reactive root layout twice, and report
    /// on the streaming providers (arg variance, identity stability, cache
    /// flicker, bottom_dock presence). `None` when the root is still
    /// initializing (loading/spacer). Drives
    /// `inv-value-fn-provider-arg-variance-13`.
    async fn provider_stability_report(
        &self,
        viewport: ViewportHint,
    ) -> Option<ProviderStabilityReport>;

    /// Drain the intermediate ViewModel emissions accumulated during the last
    /// transition and extract every `StateToggle` node's `(block_id, current)`
    /// value. Drains the buffer (one-shot per tick). Drives
    /// `inv-value-fn-provider-identity`, which compares each against the
    /// reference's task state to catch CDC-enrichment glitches visible in a
    /// transient emission before a structural re-render masks them.
    async fn drain_vm_emission_toggles(&self) -> Vec<(EntityUri, String)>;

    /// Compare the persistent live ViewModel tree (the collection driver's
    /// `set_data` path, mirroring the GPUI frontend) against a freshly
    /// re-interpreted tree built from the same data rows. The fresh tree always
    /// reflects current data, so it can't catch bugs where `set_data` fails to
    /// propagate updated props to child widgets — only the live tree can. Drives
    /// `inv-live-tree-matches-fresh`.
    ///
    /// Returns:
    /// - `None` when the comparison can't run yet (no engine, root/main-panel
    ///   still loading, no rows, or no item template) — the body Skips.
    /// - `Some(vec![])` when live and fresh trees agree.
    /// - `Some(diffs)` listing the per-item prop divergences (stale props on
    ///   existing items) — the body Fails.
    async fn live_vs_fresh_tree_diff(&self) -> Option<Vec<String>>;
}

/// Frontend-agnostic widget-tree IR. The minimum surface renderer-required
/// invariants need to walk; frontends translate from their native structure
/// (e.g. `ReactiveEngine.display_tree`, real GPUI render tree) into this.
///
/// `kind`: widget type identifier matching the frontend's ViewKind tag
/// ("editable_text", "draggable", "state_toggle", "live_block", etc.).
/// `entity_id`: the block / row id this widget renders, if any.
/// `props`: scalar widget properties as canonical strings — e.g. for a
/// `state_toggle`, this carries `field`, `current`, `label`, `states` as
/// JSON-encoded values. Invariants parse from this map; the contract is
/// "frontend serializes props it wants checked, in stable canonical form."
/// `operations`: bound operations as canonical strings, one per op, of
/// the shape `<op_name>:<key>:<value>` (e.g. `set_field:task_state:DONE`).
/// Invariants match by prefix.
/// `children`: nested widgets in render order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetSnapshot {
    pub kind: String,
    pub entity_id: Option<String>,
    pub props: std::collections::BTreeMap<String, String>,
    pub operations: Vec<String>,
    pub children: Vec<WidgetSnapshot>,
}

impl WidgetSnapshot {
    /// Pre-order recursive iterator over self + descendants.
    pub fn walk(&self) -> WidgetSnapshotIter<'_> {
        WidgetSnapshotIter { stack: vec![self] }
    }

    /// First operation whose canonical string starts with `prefix`.
    pub fn find_op(&self, prefix: &str) -> Option<&str> {
        self.operations
            .iter()
            .find(|op| op.starts_with(prefix))
            .map(String::as_str)
    }

    /// All non-None `entity_id` values reachable in the tree, deduped.
    /// `live_block` widgets carry the referenced block id as `entity_id`
    /// per the translator contract.
    pub fn collect_entity_ids(&self) -> BTreeSet<String> {
        self.walk().filter_map(|n| n.entity_id.clone()).collect()
    }

    /// All nodes whose `kind` equals `kind`.
    pub fn collect_by_kind<'a>(&'a self, kind: &str) -> Vec<&'a WidgetSnapshot> {
        self.walk().filter(|n| n.kind == kind).collect()
    }

    /// All `entity_id` values of nodes whose `kind` matches any of `kinds`.
    pub fn entity_ids_of_kinds(&self, kinds: &[&str]) -> BTreeSet<String> {
        self.walk()
            .filter(|n| kinds.iter().any(|k| n.kind == *k))
            .filter_map(|n| n.entity_id.clone())
            .collect()
    }
}

/// Pre-order traversal iterator over a `WidgetSnapshot` tree.
pub struct WidgetSnapshotIter<'a> {
    stack: Vec<&'a WidgetSnapshot>,
}

impl<'a> Iterator for WidgetSnapshotIter<'a> {
    type Item = &'a WidgetSnapshot;
    fn next(&mut self) -> Option<&'a WidgetSnapshot> {
        let node = self.stack.pop()?;
        for child in node.children.iter().rev() {
            self.stack.push(child);
        }
        Some(node)
    }
}

#[holon_macros::capmap_adapter]
pub trait SutRenderer {
    /// Stringified render-tree for a block id (debug-formatted).
    /// Used by `inv-displayed-text` and OrgRender fixed-point checks.
    async fn render_tree_of(&self, id: &EntityUri) -> Option<String>;

    /// Frontend-agnostic snapshot of the current widget tree. Any slice
    /// with a renderer (wide PBT, hypothetical Phase 9 in-memory + GPUI)
    /// can produce one; pure / storage-only slices have no widget tree
    /// and don't implement this trait at all.
    ///
    /// Returns the root widget; descendants reachable via `.children`.
    async fn widget_tree_snapshot(&self) -> WidgetSnapshot;

    /// Block ids in the data_rows that feed the current root layout's
    /// widget tree — i.e. what the renderer is reading from query
    /// results. Used by `inv-viewmodel-entity-ids-subset-of-data` to
    /// assert tree-referenced entity_ids are a subset of available
    /// data rows.
    async fn root_data_row_ids(&self) -> BTreeSet<EntityUri>;

    /// Widget tree for a SPECIFIC block id — the snapshot the renderer
    /// would produce if that block were the root of its own subtree.
    /// Used by invariants that need per-block-subtree BFS (e.g.
    /// `inv-editable-text-has-draggable`, which enforces pairing within
    /// each block_profile-rendered tree independently).
    ///
    /// Returns `None` if `block_id` doesn't resolve (no such block /
    /// not watchable yet). A "live_block" node referenced inside
    /// another tree's snapshot is the typical input: caller BFS-es by
    /// following live_block children, calling this method per discovered
    /// block id.
    async fn widget_tree_for(&self, block_id: &EntityUri) -> Option<WidgetSnapshot>;

    /// "Decompiler" content comparison for the root layout, used by
    /// `inv-viewmodel-decompiled-rows-match-query`.
    ///
    /// Interprets the root layout's render_expr against its data_rows into
    /// a display tree, extracts the per-row rendered content strings
    /// ("decompiled" inverse of the renderer), and pairs them with the
    /// `content` column of the underlying query `data_rows` filtered to the
    /// reference render expr's `visible_columns` (passed in `visible_columns`).
    ///
    /// Returns `Some((rendered_content, data_content))` — two `content`
    /// string vectors the body compares via an ordered-subset check
    /// (`rendered ⊆ data`, in order).
    ///
    /// Returns `None` when the comparison must not run — i.e. the root
    /// isn't ready (loading / spacer / not watchable), or any of the inline
    /// gates is empty (`rendered_rows`, `visible_columns`, or `data_rows`).
    /// The body treats `None` as `Ok`.
    async fn root_content_comparison(
        &self,
        visible_columns: &[String],
    ) -> Option<(Vec<String>, Vec<String>)>;

    /// Readiness signal for the root render.
    ///
    /// `true` iff the root layout's render expression is a real content
    /// expression (NOT the `loading` placeholder, NOT a `spacer`
    /// placeholder) AND the headless interpretation of it succeeds. This
    /// mirrors the inline `inv-viewmodel-snapshot` block's guards (skip on
    /// closed stream / `loading` / `spacer` / interpret panic): structural
    /// ViewModel assertions whose contract only holds for a settled content
    /// render must consult this first and skip when it returns `false`,
    /// rather than asserting against a transient placeholder root.
    async fn root_render_ready(&self) -> bool;

    /// The root `RenderExpr` `FunctionCall` name from the resolved watch
    /// snapshot's render-expr side — e.g. `"table"`, `"columns"`, or
    /// `"source_editor"` (the degraded no-query-engine render). `None` when the
    /// root is not ready: no watch resolved, a `loading`/`spacer` placeholder,
    /// or the render expr is not a `FunctionCall`. The degraded twin
    /// `inv-viewmodel-shows-source-when-no-query` reads this; `None` → it Skips.
    async fn root_render_kind(&self) -> Option<String>;
}

/// SUT-side query-results read surface. Present only when a real query engine
/// backs the render path (the Turso `BackendEngine` + `ReactiveEngine`
/// `watch_query_live` surface the full-mode frontend component owns). A degraded
/// no-Turso ("shows source") frontend has NO query engine and so does NOT
/// provide this cap — making it the negative-selection (`sut_absent`)
/// discriminator between the full-mode `inv-viewmodel-decompiled-rows-match-query`
/// twin and the degraded `inv-viewmodel-shows-source-when-no-query` twin.
#[holon_macros::capmap_adapter]
pub trait SutQueryResults {
    /// Number of rows the root layout's watch query produced. `None` when the
    /// root watch is not ready (no query engine, or still loading).
    async fn root_query_row_count(&self) -> Option<usize>;
}

// ─── Phase 6d — Layout/Bounds cluster ────────────────────────────────
//
// Re-export trait over `holon_pbt_core::user_driver::UserDriver` geometry
// methods. Phase 7 binds `inv-frontend-bounds-*`,
// `inv-editable-text-has-draggable`, `inv-frontend-no-error-widgets`.

/// One element from the rendered window's geometry registry — the
/// pbt-core-side mirror of `holon_frontend::geometry::ElementInfo`, so
/// `holon-pbt-core` carries no `holon-frontend` dependency.
///
/// Verdicts that depend on `holon-frontend`-only logic are computed on the
/// SUT side and stored here, keeping the invariant bodies pure:
/// - `expected_size_violation` is the result of `ElementInfo::expected_size.check(..)`
///   evaluated against the full element snapshot (`ProviderEvalCtx`).
/// - `is_error_widget` is `widget_type == "error"`.
#[derive(Debug, Clone)]
pub struct RenderedElement {
    /// Registry element id (e.g. `render-entity-block:…`, `editable-text-…`).
    pub el_id: String,
    /// Widget kind: `"editable_text"`, `"rendered_text"`, `"text"`,
    /// `"draggable"`, `"error"`, container kinds, …
    pub widget_type: String,
    /// The block this element is data-bound to, if any. Already in SUT id
    /// space (real UUIDs) — directly comparable to the runner's resolved ref.
    pub entity_id: Option<EntityUri>,
    /// The string actually on screen (live `InputState` value for
    /// `editable_text`, resolved prop for `text`). `None` for containers.
    pub displayed_text: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// False for empty containers.
    pub has_content: bool,
    /// Immediate tracked parent's el_id, `None` at the tracked-tree root.
    pub parent_id: Option<String>,
    /// `Some(violation)` when the observed `(w, h)` fails the element's
    /// declared `expected_size`; `None` when satisfied/unconstrained.
    pub expected_size_violation: Option<String>,
    /// `widget_type == "error"` — surfaced so bodies need no widget-type
    /// string match.
    pub is_error_widget: bool,
    /// Whether this widget's focus handle held WINDOW focus when the frame
    /// was committed. `None` for widgets without a focus handle. Engine
    /// `focused_block` moves synchronously; window focus follows via a
    /// spawned binding — the divergence window is exactly the
    /// steal-back/zombie-editor bug family.
    pub focused: Option<bool>,
}

#[holon_macros::capmap_adapter]
pub trait SutLayout {
    /// Snapshot every tracked element in the rendered window's geometry
    /// registry. Empty when no geometry provider is installed (headless
    /// variants) — the `[FrontendBounds]` invariants the registry selects
    /// only for the gpui suite treat an empty snapshot as `Skipped`.
    ///
    /// The single component-snapshot that `inv-frontend-bounds-rendered`,
    /// `inv-displayed-text`, and `inv-frontend-engine` read (mirrors the
    /// block-store `*_snapshot()` pattern). SUT-computed verdicts ride along
    /// on each [`RenderedElement`] so the bodies stay pure.
    async fn rendered_elements(&self) -> Vec<RenderedElement>;

    /// Uncached variant of [`Self::rendered_elements`]: always re-reads the
    /// live geometry registry, and implementations should pump a frame first
    /// when they can (an occluded GPUI window commits no frames on its own,
    /// so reads would otherwise stay frozen on the last committed pass).
    ///
    /// Poll-style invariants MUST use this: the per-tick `CachingProxy`
    /// memoises `rendered_elements`, so a retry loop polling the cached
    /// method observes the same frozen snapshot on every iteration and a
    /// transient lag (e.g. window focus trailing the engine by a frame or
    /// two) becomes a guaranteed "settled" failure.
    async fn rendered_elements_fresh(&self) -> Vec<RenderedElement> {
        self.rendered_elements().await
    }

    /// Fraction of content-area pixels (below the title bar) that differ from
    /// the background in the most recent window screenshot — the pixel-level
    /// ground truth for `inv-frontend-bounds-rendered`'s `not-visually-empty`
    /// backstop. `None` when no screenshot watcher is installed or no frame
    /// has been analysed yet. Near-0 means a blank window.
    async fn visual_content_fraction(&self) -> Option<f32>;

    /// True if a widget for `id` is currently registered with bounds.
    async fn has_registered_bounds(&self, id: &EntityUri) -> bool;

    /// True if a draggable handle is wired for `id`.
    async fn has_draggable_handle(&self, id: &EntityUri) -> bool;

    /// True if any rendered widget is an Error variant.
    async fn any_error_widget(&self) -> bool;

    /// Wait until a widget for `id` is registered in BoundsRegistry, or
    /// `timeout` elapses. Returns `Err(diagnostic_string)` on timeout —
    /// callers panic for input-bearing transitions per fail-loud policy.
    /// Implementations may issue a scroll-into-view RPC if the bounds are
    /// missing (virtualized lists do not prepaint offscreen rows).
    async fn wait_for_bounds(&self, id: &EntityUri, timeout: Duration) -> Result<(), String>;

    /// Wait until the widget rendered at `id` matches one of `accepted`
    /// kinds (e.g. `["editable_text", "rendered_text"]`), or `timeout`
    /// elapses. Stronger precondition than `wait_for_bounds`: confirms
    /// the click target is the *interactive* variant the transition
    /// expects, not just any element carrying the entity_id.
    ///
    /// Returns `Ok(())` when no geometry is installed (headless variants
    /// don't need widget-kind gating).
    async fn wait_for_widget_kind(
        &self,
        id: &EntityUri,
        accepted: &[&str],
        timeout: Duration,
    ) -> Result<(), String>;

    /// Wait until `id`'s `editable_text` widget reports it holds WINDOW
    /// focus (`ElementInfo::focused == Some(true)`), or `timeout` elapses.
    /// Engine focus moves synchronously; window focus follows a spawned
    /// binding — keystrokes dispatched before it lands are consumed by the
    /// previously-focused editor. Returns `Ok(())` when no geometry is
    /// installed (headless variants dispatch synchronously).
    async fn wait_for_window_focused_editor(
        &self,
        id: &EntityUri,
        timeout: Duration,
    ) -> Result<(), String>;
}

// ─── Phase 6e — Driver cluster ───────────────────────────────────────
//
// Re-export of `UserDriver` input methods. Phase 7 binds
// `inv-focus-matches-ref`. Driver methods are already trait-bound;
// this re-export keeps slice opt-in symmetric with the other clusters.

/// Frontend-engine focus, with "no engine installed" kept distinct from
/// "engine has no focus". Conflating the two (an `Option<EntityUri>` `None`)
/// made the focus steal-back bug family read as green: a lost focus looked
/// identical to SqlOnly mode and was skipped instead of failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineFocus {
    /// No frontend engine is installed (SqlOnly headless). Focus is
    /// unobservable here — checks must report `Skipped`, not `Ok`.
    NoEngine,
    /// An engine is installed but `focused_block()` is `None`. A real,
    /// comparable state: fails when the ref expects focus.
    Unfocused,
    Focused(EntityUri),
}

#[holon_macros::capmap_adapter] // mixed trait (7 async + 1 sync) → async-trait(?Send); emits CapName + `impl … for CapMap`
pub trait SutDriver {
    async fn driver_send_key_chord(&self, chord: &str);
    async fn driver_click(&self, id: &EntityUri);
    /// Region-aware click. Mirrors `UserDriver::click_entity(entity_id,
    /// region)` (`region` is "main", "left_sidebar", ...). `driver_click`
    /// is the region-defaulted convenience wrapper that panics on error;
    /// `click_entity` returns the result so callers can attach their own
    /// transition-specific diagnostic.
    async fn click_entity(&self, id: &EntityUri, region: &str) -> Result<(), String>;
    /// Poll until `engine_focused_block` returns `Some(id)` or `timeout`
    /// elapses. Used as a post-click barrier — GPUI's mouse-click goes
    /// through `dispatch_intent` (fire-and-forget), so subsequent
    /// transitions need an explicit gate before they read focus.
    async fn wait_for_engine_focus(&self, id: &EntityUri, timeout: Duration) -> Result<(), String>;
    /// Send a single raw key with modifiers. `key` is a key name like
    /// `"home"`, `"right"`, `"enter"`, `"backspace"`, or a single
    /// character (`"a"`). `modifiers` is a slice of `"cmd"`, `"ctrl"`,
    /// `"alt"`, `"shift"`. Mirrors `UserDriver::send_raw_keystroke`.
    async fn send_raw_keystroke(&self, key: &str, modifiers: &[&str]) -> Result<(), String>;
    async fn driver_current_focus(&self) -> Option<EntityUri>;
    /// The globally focused block id as tracked by the reactive/frontend
    /// engine (distinct from the per-region SQL `current_focus` matview).
    /// Set by click handlers; read by `inv-focus-matches-ref`.
    /// `NoEngine` when no frontend engine is installed (SqlOnly mode) —
    /// kept distinct from `Unfocused` so lost focus fails instead of skips.
    async fn engine_focused_block(&self) -> EngineFocus;
    /// Translate a reference-model block id (which may be a synthetic URI
    /// like `block:ref-doc-0`) to the resolved UUID-based id that the SUT
    /// engine tracks. Wide PBT: delegates to `E2ESut::resolve_uri` via
    /// `doc_uri_map`; pure slice: returns the id unchanged (no synthetic URIs).
    fn resolve_ref_block_id(&self, id: &EntityUri) -> EntityUri;
}

// ─── Phase 6f — OrgRender cluster ────────────────────────────────────
//
// Binds: `inv-org-render-fixed-point`.

#[holon_macros::capmap_adapter] // emits async-trait + CapName + `impl … for CapMap`
pub trait SutOrgRender {
    /// Snapshot every tracked org file as `(path, disk_text, rendered_text)`
    /// where `disk_text` is the bytes currently on disk and `rendered_text`
    /// is what the renderer would emit from the current SQL state.
    ///
    /// Used by `inv-org-render-fixed-point` to assert `disk == rendered`
    /// — required so the echo-suppression loop in `re_render_all_tracked`
    /// doesn't spin on a permanent disagreement.
    async fn snapshot_org_render_pairs(&self) -> Vec<(String, String, String)>;
}

// ─── Phase 6f' — OrgRead cluster ─────────────────────────────────────
//
// Binds: `inv-blocks-match-ref/org`. The org-file store in the
// block-equivalence composite — distinct from `SutOrgRender`, which reads
// the render-vs-disk fixed point. This one parses the on-disk org files
// back into blocks so they can be compared against the reference.

#[holon_macros::capmap_adapter]
pub trait SutOrgRead {
    /// Wait for the FileSyncController's background re-render to settle, then
    /// parse every tracked org file on disk back into `holon_api::Block`s.
    ///
    /// Folds the monolith's `wait_for_org_files_stable` + `parse_org_file_blocks`
    /// into one snapshot, mirroring the other block-store snapshot caps
    /// ([`SutBackend::block_raw_snapshot`], [`SutLoroLog::loro_block_snapshot`]).
    /// The org parser produces `block:<uuid>` parents for `#+ID:`-resolved docs
    /// and `file:<filename>` parents for unresolved ones — the reference side
    /// (`RefBackend::org_blocks`) mirrors that same parent resolution.
    async fn org_block_snapshot(&self) -> Vec<holon_api::Block>;
}

// ─── SutQueryCompile / SutLifecycle ── DELETED (E3, 2026-06-24) ──────
// Both fully vestigial. `SutQueryCompile` was never wired (the E2ESut impl
// was `unimplemented!()`); no transition/generator ever bound it. `SutLifecycle`
// (coarse `&mut self` start/restart) was superseded by the finer
// `local_caps::SutAppLifecycle`, which is what the `StartApp`/`SimulateRestart`
// transitions actually bind — the coarse trait had zero method callers. Removed.

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
    fn expected_focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri>;
}

/// Layout-block metadata needed by matview + ViewModel invariants.
#[holon_macros::capmap_adapter] // sync trait → no async-trait; emits CapName + `impl … for CapMap`
pub trait RefLayout {
    /// All block ids that are part of the layout scaffolding (headline,
    /// query-source, render-source). `is_layout_block` on `RefBlockTree`
    /// is the per-id predicate; this gives the full set for iteration.
    fn layout_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Block ids of the active profile blocks (from `profile_block_ids`).
    fn profile_block_ids(&self) -> BTreeSet<EntityUri>;

    /// True if the test has an active "block" profile override.
    /// Wide PBT: `ReferenceState::has_blocks_profile()`; pure slice: `false`.
    fn has_blocks_profile(&self) -> bool;

    /// Every block id the reference model tracks, **including** seed and
    /// source blocks. Unlike `RefBlockTree::all_non_seed_block_ids`, this
    /// keeps seed blocks so the matview-consistency invariant can build
    /// the full "known to the DB" set without false `extra` reports.
    /// Wide PBT: keys of `block_state.blocks`; pure slice: empty.
    fn all_block_ids(&self) -> BTreeSet<EntityUri>;

    /// The blocks the reactive root layout is *expected* to surface for
    /// `region`: non-source blocks that are descendants of the region's
    /// expected focus roots. Used by `inv-matview-consistent-with-ref` to
    /// detect rows the matview is missing. Wide PBT filters
    /// `block_state.blocks` by `content_type != Source` and
    /// `is_descendant_of_any(expected_focus_root_ids(region))`; pure
    /// slice: empty.
    fn expected_visible_content_ids(&self, region: CapRegion) -> BTreeSet<EntityUri>;

    /// True when the reference model has at least one user document. Gates the
    /// `inv-frontend-bounds-rendered` content checks (non-wrapper-content,
    /// not-visually-empty) that only make sense once docs exist. Wide PBT:
    /// `!ReferenceState::documents.is_empty()`; pure slice: `false`.
    fn has_user_documents(&self) -> bool;

    /// True when an entity is click/arrow-focused in `region` (the
    /// `focused_entity_id` map, distinct from navigation history). Used by the
    /// `not-visually-empty` backstop to pick the stricter content threshold
    /// for a focused main panel. Wide PBT:
    /// `ReferenceState::focused_entity_id.contains_key(region)`; pure slice:
    /// `false`.
    fn region_entity_focused(&self, region: CapRegion) -> bool;
}

/// Render-expression metadata exposed for ViewModel invariants.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (fully sync, owned returns → no
/// `#[async_trait]`, emits `CapName` + `impl RefRender for CapMap`). The
/// production `ReferenceState` provides it; the composed ref `CapMap` forwards.
#[holon_macros::capmap_adapter]
pub trait RefRender {
    /// Name of the active render expression for `region` (e.g. "tree",
    /// "list"). `None` when no render source block is set up yet.
    /// Wide PBT: `ReferenceState::active_render_expr_name(region)`.
    ///
    /// NOTE: this is *main-panel-preferring* — wide PBT returns
    /// `main_panel_render_expr().or(root_render_expr())`. For the
    /// `inv-viewmodel-root-matches-render-expr` check, which compares the
    /// SUT *root* widget, use `root_render_expr_name()` instead — the two
    /// diverge when a distinct main-panel render expr is set.
    fn active_render_expr_name(&self, region: CapRegion) -> Option<String>;

    /// Function-call name of the ROOT layout's render expression
    /// specifically (NOT main-panel-preferring). `None` when no root
    /// render source block is set up, OR when the root render expr is not
    /// a `FunctionCall`. Callers distinguish those two cases via
    /// `has_root_render_expr()`. Wide PBT: the `FunctionCall { name, .. }`
    /// of `ReferenceState::root_render_expr()`.
    fn root_render_expr_name(&self) -> Option<String>;

    /// The currently selected view mode (e.g. `"all"`, `"today"`) — the
    /// reference side of `inv-view-selection`. Wide PBT:
    /// `ReferenceState::current_view()`.
    fn current_view(&self) -> String;

    /// True if the reference model has a root render expression at all.
    /// Invariants gate on this before inspecting ViewModel structure.
    fn has_root_render_expr(&self) -> bool;

    /// Visible column names of the ROOT render expression — the column set
    /// `inv-viewmodel-decompiled-rows-match-query` filters data rows to.
    /// Wide PBT: `root_render_expr().map(|e| e.visible_columns()).unwrap_or_default()`.
    /// Empty when there's no root render expr.
    fn root_visible_columns(&self) -> Vec<String>;

    /// Semantic id of the layout's main-panel container block, when the
    /// active layout is a multi-region layout (e.g. the 3-column layout).
    /// `None` in layout-less mode. Used by
    /// `inv-viewmodel-root-matches-render-expr` to locate the main-panel
    /// subtree in the SUT widget snapshot without hard-coding the layout's
    /// container id. Wide PBT: `ReferenceState::main_panel_block_id()`.
    fn main_panel_block_id(&self) -> Option<EntityUri>;

    /// Function-call name of the MAIN PANEL's render expression — the content
    /// the main panel should render in a multi-region layout. Falls back to
    /// the root render expr when no distinct main-panel render expr is set.
    /// `None` when neither resolves to a `FunctionCall`. Wide PBT:
    /// `ReferenceState::main_panel_render_expr().or(root_render_expr())`'s
    /// `FunctionCall { name, .. }`.
    fn main_panel_render_expr_name(&self) -> Option<String>;
}

/// A single watch-result row, field name → stringified value. `None`
/// means the column was SQL-NULL or absent (mirrors the inline check's
/// `Value::as_string()` returning `None`). Both sides of
/// `inv-watch-rows-match-ref` carry rows in this normalized shape so the
/// body compares `Option<String>` to `Option<String>` directly, exactly
/// as the inline check did with `.and_then(|v| v.as_string())`.
pub type WatchRow = std::collections::HashMap<String, Option<String>>;

/// Active watched queries on the reference model.
#[holon_macros::capmap_adapter] // sync owned-return → emits CapName + `impl … for CapMap`
pub trait RefWatches {
    /// Query ids of currently registered watches (stable, sorted).
    /// Wide PBT: keys of `ReferenceState::active_watches`; pure slice: empty.
    fn active_watch_ids(&self) -> Vec<String>;

    /// Expected result rows for the watch `query_id`, stringified into the
    /// [`WatchRow`] shape. Wide PBT: `query_results(active_watches[query_id])`
    /// evaluated against the (already SUT-ID-space-resolved) block state;
    /// pure slice: empty. Returns an empty Vec if `query_id` is not a
    /// registered watch.
    fn expected_watch_rows(&self, query_id: &str) -> Vec<WatchRow>;

    /// The selected columns of the watch `query_id` — the field set the
    /// per-row comparison checks. Wide PBT: `active_watches[query_id].query.columns`;
    /// empty if `query_id` is unknown.
    fn watch_query_columns(&self, query_id: &str) -> Vec<String>;

    /// The `block_raw` truth-check SQL for the watch `query_id` (reads the
    /// write-side base table, bypassing the matview). Used by the CDC-lag
    /// classifier. Wide PBT: `active_watches[query_id].query.to_block_raw_sql()`;
    /// empty string if `query_id` is unknown.
    fn watch_block_raw_sql(&self, query_id: &str) -> String;
}

/// SUT-side watch (CDC-driven `ui_model`) read surface for
/// `inv-watch-rows-match-ref`. Separate from [`SutSqlProjection`] so the
/// per-id String surface there stays focused; this trait carries the
/// keyed [`WatchRow`] shape the watch comparison needs plus the two
/// `block_raw` truth-check reads the CDC-lag classifier performs.
#[holon_macros::capmap_adapter]
pub trait SutWatchRows {
    /// Query ids of the watches currently registered on the SUT
    /// (`ui_model` keys). Wide PBT: keys of `TestContext::ui_model`.
    async fn watch_query_ids(&self) -> Vec<String>;

    /// CDC-delivered rows for the watch `query_id`, stringified into the
    /// [`WatchRow`] shape. Wide PBT: `ui_model[query_id].to_vec()` with each
    /// `Value` mapped through `as_string()`. Empty if `query_id` is not
    /// registered.
    async fn watch_rows(&self, query_id: &str) -> Vec<WatchRow>;

    /// Run the given `block_raw` truth-check SQL and return the set of
    /// `id` values it yields. Used by the CDC-lag classifier to decide
    /// whether a matview/`ui_model` divergence is a pure CDC delivery race
    /// (write-side already converged) or a real write-pipeline bug. Wide
    /// PBT: `ctx.query_sql(sql)` projecting the `id` column.
    async fn block_raw_query_ids(&self, sql: &str) -> BTreeSet<EntityUri>;

    /// Read a single `field` from `block_raw` for `id`. Used by the
    /// per-field CDC-lag classifier. Wide PBT:
    /// `SELECT {field} FROM block_raw WHERE id = ?`. `None` if absent/NULL.
    async fn block_raw_field(&self, id: &EntityUri, field: &str) -> Option<String>;
}

/// Global engine-focused block (distinct from the per-region navigation
/// focus). Set by click handlers in the reactive engine; read by
/// `inv-focus-matches-ref` to compare against `ReactiveEngine::focused_block`.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync, owned return). The
/// production `ReferenceState` provides it; the composed ref `CapMap` forwards.
#[holon_macros::capmap_adapter]
pub trait RefGlobalFocus {
    /// The globally focused block id, or `None` if nothing is focused.
    /// Wide PBT: `ReferenceState::focused_block`; pure slice: `None`.
    fn global_focused_block(&self) -> Option<EntityUri>;
}

/// Task-state read-side projection. Used by `inv-viewmodel-state-toggle-correct`
/// to compare block task_state values against ViewModel StateToggle nodes.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync, owned return). The
/// production `ReferenceState` provides it; the composed ref `CapMap` forwards.
#[holon_macros::capmap_adapter]
pub trait RefTaskState {
    /// Task state string for `id` (`"TODO"`, `"DONE"`, etc.), or `None`
    /// if the block has no task_state property.
    fn task_state_of(&self, id: &EntityUri) -> Option<String>;
}

/// Reference-side typed block surface for `inv-backend-blocks-match-ref`.
///
/// The runner already remaps the reference model into SUT ID space
/// (`with_resolved_doc_uris`), so the blocks this returns carry resolved
/// `id`/`parent_id` and can be compared directly against
/// [`SutBackend::live_block_snapshot`]. Coupled to `holon_api::Block` for
/// the same reason as [`SutBackend`]: the deep field-level comparison
/// needs typed values, not the format-agnostic id/content surface of
/// [`RefBlockTree`].
#[holon_macros::capmap_adapter] // sync trait → no async-trait; emits CapName + `impl … for CapMap`
pub trait RefBackend {
    /// All reference blocks EXCLUDING seed blocks (those whose document is
    /// `no_parent`/`sentinel` — inserted via direct SQL, never reverse-synced
    /// to the matview the backend comparison reads). Mirrors the monolith's
    /// `ref_blocks_no_seed`.
    fn non_seed_blocks(&self) -> Vec<holon_api::Block>;

    /// Resolved (SUT-ID-space) seed block ids. Used to filter seed rows out
    /// of the SUT's `block_raw` id set during the CDC-lag truth check.
    /// Mirrors the monolith's translated `seed_block_ids`.
    fn seed_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Reference blocks as they should appear ON DISK in org files:
    /// non-seed, non-page (org files hold no page blocks), with document
    /// parents resolved into the org parser's id space — `block:<uuid>` for
    /// `#+ID:`-resolved docs (already remapped by `with_resolved_doc_uris`),
    /// `file:<filename>` for docs the controller hasn't resolved yet. Mirrors
    /// the monolith's `ref_blocks_org_only`; compared against
    /// [`SutOrgRead::org_block_snapshot`] by `inv-blocks-match-ref/org`.
    fn org_blocks(&self) -> Vec<holon_api::Block>;
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
        state.mark_active_editor_committed();
        return false;
    }
    state.set_block_content(&block_id, &text);
    state.mark_active_editor_committed();
    true
}

/// Commit the active editor's pending text only if it is DIRTY — i.e. the
/// text was authored by modeled typing/deleting, not merely divergent from
/// `block.content` via an external change (a stale mirror; prod's data
/// subscription would have refreshed it, so committing it would write old
/// text into the ref). This models prod's blur / structural-commit-point
/// behavior: "structural ops are commit points" (docs/Architecture/UI.md),
/// and a click-away blur commits the previously focused editor's
/// user-authored text.
pub fn commit_active_editor_if_dirty<R>(state: &mut R) -> bool
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus,
{
    if !state.active_editor_dirty() {
        return false;
    }
    commit_active_editor_if_changed(state)
}

// ─── Additional cross-cuts discovered in Phase 1 (P1.3 spike) ────────
//
// Candidates known from code reading:
// - focus-shift on tree mutation (Indent/Outdent/Split do
//   `state.focused_block = Some(new_id)` after mutation — pattern
//   already in `split_block.rs:123-129`). Currently inlined per
//   transition; could become a free function
//   `fn refocus_after_split<R: RefFocusMut>(state: &mut R, new_id: EntityUri, region: CapRegion)`.
// - sibling re-key on join (join_block mutates parent's child order;
//   pure-slice impl can keep a Vec and recompute, wide PBT uses
//   gen_key_between).
// - descendant invalidation on outdent (Outdent moves a block up a
//   level; any cached descendant set goes stale).
//
// Final enumeration is a P1.3 deliverable. Trait surface above is a
// minimum-viable set — additions widen it, not break it.
