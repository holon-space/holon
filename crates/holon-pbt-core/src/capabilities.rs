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
