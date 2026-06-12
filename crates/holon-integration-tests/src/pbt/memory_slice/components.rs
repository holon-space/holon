//! The SUT components of the memory-wide slice. Each is a [`CapProvider`] that
//! contributes one or more capabilities to a composed `CapMap`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use holon::api::types::Traversal;
use holon::api::{BackendEngine, MemoryBackend};
use holon_api::repository::CoreOperations;
use holon_api::types::ContentType;
use holon_api::{ApiError, BlockContent, EntityUri, StorageEntity, Value};
use holon_frontend::editor_caret;
use holon_pbt_core::capabilities::{
    SutBackend, SutBlockTreeWrite, SutEditorMirrorRead, SutEditorMirrorWrite,
};
use holon_pbt_core::composition::{CapMap, CapProvider};

use crate::pbt::types::normalize_content_for_org_roundtrip;

/// A composition component wrapping an in-memory block store. Provides the
/// [`SutBackend`] capability by reading the store directly — no projection, no
/// CDC, no async settle. The whole point of the memory slice's speed.
///
/// Holds the backend behind `Arc` so a write target (e.g. [`InProcEditorSut`])
/// can commit into the SAME store this read cap observes. `MemoryBackend::clone`
/// deep-copies, so sharing MUST go through the `Arc`, not a clone.
pub struct MemoryBackendComponent {
    backend: Arc<MemoryBackend>,
    /// Mirror of the reference oracle's `next_id`, kept in lockstep by the driver
    /// (`set_next_split_id` after each tick) so `apply_split_block` mints the SAME
    /// synthetic `:split-N` id the reference does. The pure-memory slice has an
    /// identity ref↔SUT id space, so the ids must agree for `inv-blocks-match-ref`.
    next_split_id: Mutex<usize>,
}

impl MemoryBackendComponent {
    pub fn new(backend: MemoryBackend) -> Self {
        Self::new_shared(Arc::new(backend))
    }

    /// Wrap an already-shared backend so the read cap and a write target observe
    /// one store. The F2 keystone for committed-content parity.
    pub fn new_shared(backend: Arc<MemoryBackend>) -> Self {
        Self {
            backend,
            next_split_id: Mutex::new(0),
        }
    }

    /// Sync the SUT's synthetic-id counter to the reference's `next_id`. The
    /// driver calls this at init and after every transition so a following
    /// `SplitBlock` mints the id the oracle minted (see `next_split_id`).
    pub fn set_next_split_id(&self, next_id: usize) {
        *self.next_split_id.lock().unwrap() = next_id;
    }
}

#[async_trait::async_trait(?Send)]
impl SutBackend for MemoryBackendComponent {
    /// No CDC matview here — the "live" view *is* the store. Reading the same
    /// convergent truth as `block_raw_snapshot` makes the slice CDC-lag-free by
    /// construction (the `inv-no-orphan-blocks` staleness gate can never fire).
    async fn live_block_snapshot(&self) -> Vec<holon_api::Block> {
        self.backend
            .get_all_blocks(Traversal::ALL)
            .await
            .expect("MemoryBackend::get_all_blocks (live_block_snapshot) must not fail in-memory")
    }

    async fn block_raw_snapshot(&self) -> Vec<holon_api::Block> {
        self.backend
            .get_all_blocks(Traversal::ALL)
            .await
            .expect("MemoryBackend::get_all_blocks (block_raw_snapshot) must not fail in-memory")
    }

    /// The memory slice has no focus-roots projection. None of the cap-selected
    /// invariants read it (selection guarantees this), so an empty mirror is the
    /// honest answer rather than a fabricated row.
    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// Private structural-tree helpers — read the live order/parentage from the
/// store so the write ops below target the SAME blocks the reference does.
impl MemoryBackendComponent {
    async fn block(&self, id: &EntityUri) -> holon_api::Block {
        self.backend
            .get_block(id.as_str())
            .await
            .unwrap_or_else(|e| panic!("MemoryBackendComponent: get_block({id}) failed: {e}"))
    }

    /// Children of `parent` in store (insertion/move) order — mirrors the
    /// reference's `sorted_children_of` because every op below preserves the
    /// seed order and reorders via explicit `move_block` anchors.
    ///
    /// A virtual parent (`no_parent`/sentinel) is not a stored block, so
    /// `list_children` rejects it; its "children" are the top-level blocks, read
    /// from a full snapshot filtered by `parent_id` (preserving traversal order).
    async fn ordered_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        if parent.is_no_parent() || parent.is_sentinel() {
            return self
                .backend
                .get_all_blocks(Traversal::ALL)
                .await
                .expect("MemoryBackendComponent: get_all_blocks (ordered_children) must not fail")
                .into_iter()
                .filter(|b| b.parent_id == *parent)
                .map(|b| b.id)
                .collect();
        }
        self.backend
            .list_children(parent.as_str())
            .await
            .unwrap_or_else(|e| {
                panic!("MemoryBackendComponent: list_children({parent}) failed: {e}")
            })
            .iter()
            .map(|s| EntityUri::parse(s).expect("MemoryBackendComponent: child id parse"))
            .collect()
    }

    async fn prev_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let parent = self.block(id).await.parent_id;
        let kids = self.ordered_children(&parent).await;
        let idx = kids.iter().position(|k| k == id)?;
        (idx > 0).then(|| kids[idx - 1].clone())
    }

    async fn next_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let parent = self.block(id).await.parent_id;
        let kids = self.ordered_children(&parent).await;
        let idx = kids.iter().position(|k| k == id)?;
        kids.get(idx + 1).cloned()
    }

    async fn move_block(&self, id: &EntityUri, parent: EntityUri, after: Option<EntityUri>) {
        self.backend
            .move_block(id, parent, after)
            .await
            .unwrap_or_else(|e| panic!("MemoryBackendComponent: move_block({id}) failed: {e}"));
    }
}

/// The structural block-tree write cap over the real `MemoryBackend`. Each op
/// mirrors the corresponding `ReferenceState`/`transitions::*::apply_to_ref`
/// algorithm so the SUT tree converges to the oracle's. `MemoryBackend` has no
/// `sequence` column — child order IS `children_by_parent` insertion/move order,
/// so ordering is reproduced by driving `move_block` with the explicit anchor
/// each op implies (no order invariant compares it directly; the SUT's own
/// prev/next-sibling reads must match the ref, which these anchors guarantee).
#[async_trait::async_trait(?Send)]
impl SutBlockTreeWrite for MemoryBackendComponent {
    /// `transitions::indent`: move under the previous sibling, appended after its
    /// existing children (`move_block(id, prev, after=last child of prev)`).
    /// `MemoryBackend::move_block` with the last child as anchor == append to end.
    async fn apply_indent(&self, id: &EntityUri) {
        let prev = self
            .prev_sibling(id)
            .await
            .expect("apply_indent: no previous sibling (precondition should have gated)");
        let after = self.ordered_children(&prev).await.last().cloned();
        self.move_block(id, prev, after).await;
    }

    /// `ReferenceState::outdent_block`: move to the grandparent, placed
    /// immediately after the old parent (`move_block(id, grandparent, after=parent)`).
    async fn apply_outdent(&self, id: &EntityUri) {
        let parent = self.block(id).await.parent_id;
        let grandparent = self.block(&parent).await.parent_id;
        self.move_block(id, grandparent, Some(parent)).await;
    }

    /// `swap_siblings(id, prev)`: swap by moving the previous sibling to *after*
    /// `id`. Works whether or not `prev` is the first child (no front-insertion
    /// needed, which `MemoryBackend::move_block` can't express).
    async fn apply_move_up(&self, id: &EntityUri) {
        let prev = self
            .prev_sibling(id)
            .await
            .expect("apply_move_up: no previous sibling (precondition should have gated)");
        let parent = self.block(id).await.parent_id;
        self.move_block(&prev, parent, Some(id.clone())).await;
    }

    /// `swap_siblings(id, next)`: swap by moving `id` to *after* its next sibling.
    async fn apply_move_down(&self, id: &EntityUri) {
        let next = self
            .next_sibling(id)
            .await
            .expect("apply_move_down: no next sibling (precondition should have gated)");
        let parent = self.block(id).await.parent_id;
        self.move_block(id, parent, Some(next)).await;
    }

    /// `ReferenceState::split_block`: original keeps `content[..pos].trim_end()`,
    /// a new `:split-N` sibling gets `content[pos..].trim_start()`, placed right
    /// after the original. `N` mirrors the oracle's `next_id` (`set_next_split_id`).
    async fn apply_split_block(&self, id: &EntityUri, position: usize) {
        let block = self.block(id).await;
        let parent = block.parent_id.clone();
        let content = block.content.clone();
        assert!(
            content.is_char_boundary(position),
            "apply_split_block: position {position} not a char boundary of {content:?}"
        );
        let before = content[..position].trim_end().to_string();
        let after = content[position..].trim_start().to_string();

        self.backend
            .update_block(id.as_str(), BlockContent::text(&before))
            .await
            .unwrap_or_else(|e| panic!("apply_split_block: update original failed: {e}"));

        let new_id = {
            let n = *self.next_split_id.lock().unwrap();
            EntityUri::block(&format!(":split-{n}"))
        };
        self.backend
            .create_block(
                parent.clone(),
                BlockContent::text(&after),
                Some(new_id.clone()),
            )
            .await
            .unwrap_or_else(|e| panic!("apply_split_block: create new block failed: {e}"));
        // `create_block` appends to the end of the parent's children; relocate to
        // immediately after the original to match the oracle's ordering. Skip for a
        // virtual (`no_parent`/sentinel) parent — `MemoryBackend::move_block` rejects
        // a virtual target, and top-level sibling order is not invariant-checked
        // (the new block keeps the original's `no_parent` parent either way).
        if !parent.is_no_parent() && !parent.is_sentinel() {
            self.move_block(&new_id, parent, Some(id.clone())).await;
        }
    }

    /// `ReferenceState::join_block`: target = previous sibling, else the parent
    /// (child→parent join). Append `id`'s content onto the target, re-parent
    /// `id`'s children onto the target, delete `id`. Order of the moved children
    /// is not invariant-checked (only parent/content/orphan/cycle are).
    async fn apply_join_block(&self, id: &EntityUri) {
        let block = self.block(id).await;
        let target = match self.prev_sibling(id).await {
            Some(prev) => prev,
            None => block.parent_id.clone(),
        };
        let target_content = self.block(&target).await.content;
        self.backend
            .update_block(
                target.as_str(),
                BlockContent::text(&format!("{}{}", target_content, block.content)),
            )
            .await
            .unwrap_or_else(|e| panic!("apply_join_block: update target failed: {e}"));

        for child in self.ordered_children(id).await {
            let anchor = self.ordered_children(&target).await.last().cloned();
            self.move_block(&child, target.clone(), anchor).await;
        }
        self.backend
            .delete_block(id.as_str())
            .await
            .unwrap_or_else(|e| panic!("apply_join_block: delete failed: {e}"));
    }
}

impl CapProvider for MemoryBackendComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self.clone() as Arc<dyn SutBackend>);
        caps.insert(self as Arc<dyn SutBlockTreeWrite>);
    }
}

/// The live state of the single active editor. A production GPUI editor tracks
/// a `MutableText` value plus a byte caret per focused block; the slice models
/// exactly that, one editor at a time (which is all the `RefEditorMirror`
/// "active editor" contract observes).
#[derive(Clone, Default)]
struct EditorCell {
    block: Option<EntityUri>,
    /// The live (pre-commit) text the keystrokes mutate.
    text: String,
    /// Caret position as a **byte** offset into `text`, matching production's
    /// `headless_editor_mirror.rs` cursor arithmetic.
    caret: usize,
    /// Set by typing/deleting; cleared on commit. Mirrors
    /// `RefEditorMirror::active_editor_dirty`.
    dirty: bool,
}

/// The narrow commit dependency of [`InMemEditorComponent`]: the editor only ever
/// writes a block's content on commit, so it depends on exactly that — not the full
/// [`CoreOperations`] surface. This lets the editor commit into any canonical
/// backend: a `CoreOperations` store (Loro/memory, via [`CoreOpsCommit`]) **or** a
/// Turso [`BackendEngine`] (via its production `set_field` op) — the latter is what
/// lets `compose_sut`'s Turso-canonical configs host the editor (no `CoreOperations`
/// exists over `BackendEngine`). Minimal interface = no faked methods.
#[async_trait::async_trait(?Send)]
pub trait EditorCommitTarget {
    /// Write `content` as block `id`'s text content into the canonical store.
    async fn commit_block_content(&self, id: &str, content: &str) -> Result<(), ApiError>;
}

/// Adapts a [`CoreOperations`] store (Loro/memory) to [`EditorCommitTarget`] via
/// `update_block` — the commit path the memory/loro slices use.
pub struct CoreOpsCommit(pub Arc<dyn CoreOperations>);

#[async_trait::async_trait(?Send)]
impl EditorCommitTarget for CoreOpsCommit {
    async fn commit_block_content(&self, id: &str, content: &str) -> Result<(), ApiError> {
        self.0.update_block(id, BlockContent::text(content)).await
    }
}

/// Commits editor content into a Turso [`BackendEngine`] through the production
/// `block`/`set_field` operation — the SAME op `SqlProjectionComponent::update_content`
/// drives, so the committed text lands in `block_raw` where the block invariants read.
#[async_trait::async_trait(?Send)]
impl EditorCommitTarget for BackendEngine {
    async fn commit_block_content(&self, id: &str, content: &str) -> Result<(), ApiError> {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("field".into(), Value::String("content".to_string()));
        params.insert("value".into(), Value::String(content.to_string()));
        let entity = "block".to_string().into();
        self.execute_operation(&entity, "set_field", params)
            .await
            .map(|_| ())
            .map_err(|e| ApiError::InternalError {
                message: format!("editor commit set_field failed: {e}"),
            })
    }
}

/// The second SUT component (doc §6 `InMemEditor`): a headless active-editor
/// that delegates its caret/text math to the **shared** `editor_caret`
/// primitives production's `HeadlessEditorMirror::handle_keystroke` also calls
/// (single source of truth — this is a thin `String` wrapper, not a parallel
/// copy of the math), minus the `ReactiveEngine`/Loro coupling that ties the
/// real keystroke pipeline to Turso. Interior
/// mutability (`Mutex`) so the one `Arc` is both driven through `&self` writes
/// in the apply phase and registered as the read-only [`SutEditorMirrorRead`]
/// cap (§4.4: write caps mutate the concrete SUT, read caps join the map).
pub struct InMemEditorComponent {
    cell: Mutex<EditorCell>,
    /// Where this editor commits live text — the SAME canonical store the
    /// [`SutBackend`] cap reads, so a write is observed by both the editor-mirror
    /// and block-content invariants. A narrow [`EditorCommitTarget`] (not the full
    /// [`CoreOperations`]) so the editor commits into a `CoreOperations` store
    /// (Loro/memory) OR a Turso `BackendEngine` (via `set_field`) uniformly. Owning
    /// it lets the component host [`SutEditorMirrorWrite`] directly (E1, Stage-1b —
    /// the old `InProcEditorSut` split is collapsed: one editor component is both the
    /// read mirror and the keystroke-driven write target).
    commit_target: Arc<dyn EditorCommitTarget>,
}

impl InMemEditorComponent {
    /// Commit into a [`CoreOperations`] store (Loro/memory) — the memory/loro slices'
    /// path; the store is wrapped in [`CoreOpsCommit`].
    pub fn new(store: Arc<dyn CoreOperations>) -> Self {
        Self::new_commit(Arc::new(CoreOpsCommit(store)))
    }

    /// Commit into an explicit [`EditorCommitTarget`] — used by `compose_sut`'s
    /// Turso-canonical configs, where the canonical backend is a `BackendEngine`
    /// (no `CoreOperations`), committing through its production `set_field` op.
    pub fn new_commit(commit_target: Arc<dyn EditorCommitTarget>) -> Self {
        Self {
            cell: Mutex::new(EditorCell::default()),
            commit_target,
        }
    }

    /// Open an editor on `block`, seeding the caret to end-of-text — production
    /// clicks re-open an editor at end-of-text (`seed_for_click`).
    pub fn open(&self, block: EntityUri, text: String) {
        let mut c = self.cell.lock().unwrap();
        c.caret = text.len();
        c.text = text;
        c.block = Some(block);
        c.dirty = false;
    }

    /// Insert `s` at the caret and advance the caret by its byte length — the
    /// char-keystroke arm of `handle_keystroke`, via the shared primitive.
    pub fn type_chars(&self, s: &str) {
        let mut c = self.cell.lock().unwrap();
        let at = c.caret;
        c.caret = editor_caret::insert_at(&mut c.text, at, s);
        c.dirty = true;
    }

    /// Delete `count` characters before the caret (codepoint-wise), retreating
    /// the caret by the bytes removed — the mid-line backspace arm, via the
    /// shared primitive.
    pub fn delete_backward(&self, count: usize) {
        let mut c = self.cell.lock().unwrap();
        let at = c.caret;
        c.caret = editor_caret::delete_back(&mut c.text, at, count);
        c.dirty = true;
    }

    /// Move the caret to `byte`, clamped to the nearest char boundary at or
    /// before `text.len()` (production's `home`/`end`/`left`/`right` all land
    /// on boundaries), via the shared primitive.
    pub fn move_cursor(&self, byte: usize) {
        let mut c = self.cell.lock().unwrap();
        c.caret = editor_caret::clamp_boundary(&c.text, byte);
    }

    /// The active block + its live text, to commit into the backing store
    /// (`MemoryBackend::update_block`). Clears the dirty flag. `None` if no
    /// editor is open.
    pub fn take_commit(&self) -> Option<(EntityUri, String)> {
        let mut c = self.cell.lock().unwrap();
        c.dirty = false;
        c.block.clone().map(|b| (b, c.text.clone()))
    }

    /// Commit the editor's live text into the shared store, normalizing the SAME
    /// way the reference does (`commit_active_editor_if_changed`) so the RAW
    /// content `inv-block-content-matches-ref/block_raw` compares matches
    /// byte-for-byte.
    ///
    /// `take_commit` returns `Some` whenever a block is open — it does NOT gate on
    /// the dirty flag — so this fires on every type/delete, re-writing identical
    /// content when nothing changed (e.g. a backspace at caret 0). That redundant
    /// same-value write is harmless; the reference content-gates and converges to
    /// the same string.
    ///
    /// `ContentType::Text` is hardcoded: Stage 1 only ever opens the editor on a
    /// text block. Reading the block's real `content_type` is a later-stage
    /// concern (non-Text editing).
    async fn commit(&self) {
        if let Some((id, text)) = self.take_commit() {
            let normalized = normalize_content_for_org_roundtrip(&text, ContentType::Text);
            self.commit_target
                .commit_block_content(id.as_str(), &normalized)
                .await
                .expect(
                    "InMemEditorComponent commit: commit_block_content into shared store must not fail",
                );
        }
    }
}

impl SutEditorMirrorRead for InMemEditorComponent {
    fn editor_caret_byte(&self, block_id: &EntityUri) -> Result<Option<usize>, String> {
        let c = self.cell.lock().unwrap();
        match &c.block {
            Some(b) if b == block_id => Ok(Some(c.caret)),
            // Observable medium, but no editor tracked for this block.
            _ => Ok(None),
        }
    }

    fn editor_live_text(&self, block_id: &EntityUri) -> Result<String, String> {
        let c = self.cell.lock().unwrap();
        match &c.block {
            Some(b) if b == block_id => Ok(c.text.clone()),
            _ => Err(format!(
                "no active editor for {block_id} in the in-memory editor"
            )),
        }
    }
}

/// The editor hosts BOTH the read mirror and the keystroke-driven write target
/// (E1, Stage-1b): the `TypeChars`/`DeleteBackward`/`MoveCursor` `apply_to_sut`
/// bodies drive the real caret/text math, then commit the live text into the
/// shared store — the headless analogue of `E2ESut`'s keystroke-driven
/// `SutEditorMirrorWrite`, no `UserDriver`/GPUI window. The composed `CapMap` is a
/// `SutTransitionTarget` for editor ops just as `MemoryBackendComponent` makes it
/// one for structural ops.
#[async_trait::async_trait(?Send)]
impl SutEditorMirrorWrite for InMemEditorComponent {
    async fn apply_type_chars(&self, text: &str) {
        self.type_chars(text);
        self.commit().await;
    }

    async fn apply_delete_backward(&self, count: usize) {
        self.delete_backward(count);
        self.commit().await;
    }

    async fn apply_move_cursor(&self, byte_position: usize) {
        // No commit — matches the reference `MoveCursor`, which moves the caret
        // without writing block content.
        self.move_cursor(byte_position);
    }
}

impl CapProvider for InMemEditorComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self.clone() as Arc<dyn SutEditorMirrorRead>);
        caps.insert(self as Arc<dyn SutEditorMirrorWrite>);
    }
}
