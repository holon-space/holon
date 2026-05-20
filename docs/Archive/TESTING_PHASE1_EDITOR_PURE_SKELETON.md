# Phase 1 — EditorPureRef + EditorPureSut skeleton (Phase 5 starting point)

**Status**: paper draft. Compiles in theory against the current `capabilities.rs`. Materialise in `crates/holon-integration-tests/tests/editor_pure_pbt.rs` once Phase 3 lands the seven-transition migration.

This skeleton is the **forwarding-only product struct** the `PbtSlicing.md` design doc calls for — no logic, just capability impls.

---

## EditorPureRef — backing structures

```rust
use std::collections::{BTreeMap, BTreeSet, HashMap};
use holon_pbt_core::capabilities::{
    EntityUri, CapCursor, CapRegion,
    RefBlockTree, RefBlockTreeMut, RefEditorMirror, RefEditorMirrorMut,
    RefFocus, RefFocusMut, RefLifecycle,
};

/// In-memory block — narrow data the pure slice needs.
#[derive(Debug, Clone)]
struct PureBlock {
    id: EntityUri,
    parent: Option<EntityUri>,
    content: String,
    sort_key: String,         // simple fractional index (e.g. "a", "b", "ab") — enough for ordering
    is_text: bool,
}

/// EditorMirror state — mirror of ActiveEditor.
#[derive(Debug, Clone, Default)]
struct PureEditor {
    block_id: Option<EntityUri>,
    text: String,             // pending in-memory content (pre-commit)
    cursor: usize,             // byte offset
}

pub struct EditorPureRef {
    blocks: BTreeMap<EntityUri, PureBlock>,
    /// Root id for the single doc this slice deals with.
    root_id: EntityUri,
    editor: PureEditor,
    focus_main: Option<EntityUri>,
    focus_cursor_main: Option<CapCursor>,
    next_id: u64,
    last_transition_kind: Option<&'static str>,
}

impl EditorPureRef {
    pub fn new_with_seed(root_text: &str) -> Self {
        let root_id = "block:root".to_string();
        let mut blocks = BTreeMap::new();
        blocks.insert(
            root_id.clone(),
            PureBlock {
                id: root_id.clone(),
                parent: None,
                content: root_text.to_string(),
                sort_key: "a".into(),
                is_text: true,
            },
        );
        EditorPureRef {
            blocks,
            root_id: root_id.clone(),
            editor: PureEditor { block_id: Some(root_id.clone()), text: root_text.into(), cursor: 0 },
            focus_main: Some(root_id),
            focus_cursor_main: Some(CapCursor::default()),
            next_id: 1,
            last_transition_kind: None,
        }
    }

    fn fresh_id(&mut self) -> EntityUri {
        let id = format!("block:n{}", self.next_id);
        self.next_id += 1;
        id
    }
}
```

## Capability impls (forwarding only)

```rust
impl RefLifecycle for EditorPureRef {
    fn app_started(&self) -> bool { true }
    fn is_properly_setup(&self) -> bool { true }
    fn enable_loro(&self) -> bool { false }  // pure logic, no Loro
    fn last_transition_kind(&self) -> Option<&'static str> { self.last_transition_kind }
    fn atomic_editor_enabled() -> bool { true }  // pure slice's reason for existing
}

impl RefBlockTree for EditorPureRef {
    fn block_content(&self, id: &EntityUri) -> Option<&str> {
        self.blocks.get(id).map(|b| b.content.as_str())
    }
    fn is_text_block(&self, id: &EntityUri) -> bool {
        self.blocks.get(id).map_or(false, |b| b.is_text)
    }
    fn main_editable_descendants(&self) -> Vec<EntityUri> {
        // All text descendants of root_id; in pure slice this is everything reachable.
        let mut out = Vec::new();
        let mut stack = vec![self.root_id.clone()];
        while let Some(id) = stack.pop() {
            if let Some(b) = self.blocks.get(&id) {
                if b.is_text && id != self.root_id { out.push(id.clone()); }
                let children = self.sorted_children(&id);
                stack.extend(children.into_iter().rev());
            }
        }
        out
    }
    fn focus_root_ids(&self, _: CapRegion) -> BTreeSet<EntityUri> {
        BTreeSet::from([self.root_id.clone()])
    }
    fn previous_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let block = self.blocks.get(id)?;
        let parent = block.parent.as_ref()?;
        let siblings = self.sorted_children(parent);
        let idx = siblings.iter().position(|s| s == id)?;
        if idx == 0 { None } else { Some(siblings[idx - 1].clone()) }
    }
    fn next_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let block = self.blocks.get(id)?;
        let parent = block.parent.as_ref()?;
        let siblings = self.sorted_children(parent);
        let idx = siblings.iter().position(|s| s == id)?;
        siblings.get(idx + 1).cloned()
    }
    fn grandparent(&self, id: &EntityUri) -> Option<EntityUri> {
        let parent = self.blocks.get(id)?.parent.as_ref()?;
        self.blocks.get(parent)?.parent.clone()
    }
    fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        let mut kids: Vec<&PureBlock> = self.blocks
            .values()
            .filter(|b| b.parent.as_ref() == Some(parent))
            .collect();
        kids.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        kids.into_iter().map(|b| b.id.clone()).collect()
    }
    fn is_descendant_of_any(&self, id: &EntityUri, ancestors: &BTreeSet<EntityUri>) -> bool {
        let mut cursor = id.clone();
        loop {
            if ancestors.contains(&cursor) { return true; }
            match self.blocks.get(&cursor).and_then(|b| b.parent.clone()) {
                Some(p) => cursor = p,
                None => return false,
            }
        }
    }
    fn is_layout_block(&self, _: &EntityUri) -> bool { false }
    fn is_focusable(&self, id: &EntityUri) -> bool {
        self.is_text_block(id) && id != &self.root_id
    }
}

impl RefBlockTreeMut for EditorPureRef {
    fn push_undo_snapshot(&mut self) { /* no undo in pure slice — no-op */ }

    fn set_block_content(&mut self, id: &EntityUri, text: &str) {
        if let Some(b) = self.blocks.get_mut(id) {
            b.content = text.to_string();
        }
    }

    fn split_block(&mut self, id: &EntityUri, position: usize) -> EntityUri {
        let new_id = self.fresh_id();
        let (parent, tail, new_sort_key) = {
            let b = self.blocks.get_mut(id).expect("split target exists");
            let position = position.min(b.content.len());
            let tail = b.content.split_off(position);
            // New sort key: just append "_next" — good enough for pure-slice ordering.
            let parent = b.parent.clone();
            let new_sort_key = format!("{}m", b.sort_key);
            (parent, tail, new_sort_key)
        };
        self.blocks.insert(new_id.clone(), PureBlock {
            id: new_id.clone(),
            parent,
            content: tail,
            sort_key: new_sort_key,
            is_text: true,
        });
        new_id
    }

    fn join_block(&mut self, id: &EntityUri) -> usize {
        // Join `id` into its previous sibling (or parent if none).
        let into = self.previous_sibling(id).unwrap_or_else(|| {
            self.blocks.get(id).and_then(|b| b.parent.clone())
                .expect("join_block: no previous sibling AND no parent — root cannot be joined")
        });
        let appended = self.blocks.remove(id).expect("join target exists").content;
        let into_block = self.blocks.get_mut(&into).expect("join destination exists");
        let cursor_at_join = into_block.content.len();
        into_block.content.push_str(&appended);
        cursor_at_join
    }

    fn indent(&mut self, id: &EntityUri) {
        let prev = self.previous_sibling(id).expect("indent: previous sibling required");
        // Place under prev as new last child.
        let last_child_sort = self.sorted_children(&prev).last()
            .and_then(|cid| self.blocks.get(cid).map(|b| b.sort_key.clone()));
        let new_sort = match last_child_sort {
            Some(s) => format!("{}m", s),
            None => "a".to_string(),
        };
        if let Some(b) = self.blocks.get_mut(id) {
            b.parent = Some(prev);
            b.sort_key = new_sort;
        }
    }

    fn outdent(&mut self, id: &EntityUri) {
        // Re-parent to grandparent; place immediately after current parent.
        let gp = self.grandparent(id);
        let parent_id = self.blocks.get(id).and_then(|b| b.parent.clone());
        let parent_sort = parent_id.as_ref().and_then(|p| self.blocks.get(p).map(|b| b.sort_key.clone()));
        if let (Some(_gp_id), Some(parent_sort)) = (gp.as_ref(), parent_sort) {
            let new_sort = format!("{}m", parent_sort);
            if let Some(b) = self.blocks.get_mut(id) {
                b.parent = gp;
                b.sort_key = new_sort;
            }
        }
    }

    fn move_block(&mut self, id: &EntityUri, new_parent: EntityUri, after: Option<&EntityUri>) {
        let after_sort = after.and_then(|a| self.blocks.get(a).map(|b| b.sort_key.clone()));
        let new_sort = match after_sort {
            Some(s) => format!("{}m", s),
            None => "a".to_string(),
        };
        if let Some(b) = self.blocks.get_mut(id) {
            b.parent = Some(new_parent);
            b.sort_key = new_sort;
        }
    }

    fn swap_siblings(&mut self, a: &EntityUri, b: &EntityUri) {
        let sort_a = self.blocks.get(a).map(|x| x.sort_key.clone());
        let sort_b = self.blocks.get(b).map(|x| x.sort_key.clone());
        if let (Some(sa), Some(sb)) = (sort_a, sort_b) {
            self.blocks.get_mut(a).unwrap().sort_key = sb;
            self.blocks.get_mut(b).unwrap().sort_key = sa;
        }
    }
}

impl RefEditorMirror for EditorPureRef {
    fn active_editor_block(&self) -> Option<EntityUri> { self.editor.block_id.clone() }
    fn active_editor_text(&self) -> Option<&str> {
        self.editor.block_id.as_ref().map(|_| self.editor.text.as_str())
    }
    fn active_editor_cursor(&self) -> Option<usize> {
        self.editor.block_id.as_ref().map(|_| self.editor.cursor)
    }
}

impl RefEditorMirrorMut for EditorPureRef {
    fn type_chars(&mut self, text: &str) {
        self.editor.text.insert_str(self.editor.cursor, text);
        self.editor.cursor += text.len();
    }
    fn delete_backward(&mut self, count: usize) {
        for _ in 0..count {
            if self.editor.cursor == 0 { break; }
            self.editor.cursor -= 1;
            self.editor.text.remove(self.editor.cursor);
        }
    }
    fn move_cursor(&mut self, byte_position: usize) {
        self.editor.cursor = byte_position.min(self.editor.text.len());
    }
}

impl RefFocus for EditorPureRef {
    fn current_focus(&self, region: CapRegion) -> Option<EntityUri> {
        match region { CapRegion::Main | CapRegion::Single => self.focus_main.clone(), _ => None }
    }
    fn focused_cursor(&self, _: CapRegion) -> Option<CapCursor> { self.focus_cursor_main }
}

impl RefFocusMut for EditorPureRef {
    fn set_focus(&mut self, _: CapRegion, id: EntityUri, cursor: CapCursor) {
        // Update editor mirror to reflect newly-focused block.
        let text = self.blocks.get(&id).map(|b| b.content.clone()).unwrap_or_default();
        self.editor = PureEditor { block_id: Some(id.clone()), text, cursor: 0 };
        self.focus_main = Some(id);
        self.focus_cursor_main = Some(cursor);
    }
    fn clear_focus_if_deleted(&mut self, id: &EntityUri) {
        if self.focus_main.as_ref() == Some(id) {
            self.focus_main = None;
            self.focus_cursor_main = None;
            self.editor = PureEditor::default();
        }
    }
}
```

## EditorPureSut — SUT side (mirrors Ref)

```rust
use holon_pbt_core::capabilities::{
    EntityUri, CapRegion,
    SutBlockTreeWrite, SutEditorMirrorWrite, SutFocusWrite, SutQuiesce,
};

pub struct EditorPureSut {
    /// In pure slice, SUT state IS the ref-state implementation. The two
    /// are *identical* by construction — no SUT to compare against.
    /// This is fine: pure-logic invariants are intra-state (cursor
    /// within text length, structural integrity); no ref↔SUT divergence
    /// to detect, because there is no separate SUT.
    ///
    /// Production transitions still go through the SUT trait — same
    /// trait surface, same method signatures, no special-case code in
    /// transitions. The "SUT" is just a second mutation pathway into
    /// the same in-memory tree.
    inner: EditorPureRef,
}

impl EditorPureSut {
    pub fn new_with_seed(seed: &str) -> Self {
        Self { inner: EditorPureRef::new_with_seed(seed) }
    }
}

impl SutEditorMirrorWrite for EditorPureSut {
    async fn apply_type_chars(&mut self, text: &str) {
        <EditorPureRef as RefEditorMirrorMut>::type_chars(&mut self.inner, text);
    }
    async fn apply_delete_backward(&mut self, count: usize) {
        <EditorPureRef as RefEditorMirrorMut>::delete_backward(&mut self.inner, count);
    }
    async fn apply_move_cursor(&mut self, byte_position: usize) {
        <EditorPureRef as RefEditorMirrorMut>::move_cursor(&mut self.inner, byte_position);
    }
}

impl SutBlockTreeWrite for EditorPureSut {
    async fn apply_split_block(&mut self, id: &EntityUri, position: usize) {
        let new_id = <EditorPureRef as RefBlockTreeMut>::split_block(&mut self.inner, id, position);
        // Pure-slice mirror of production's editor_focus_op follow-up.
        self.inner.set_focus(CapRegion::Main, new_id, Default::default());
    }
    async fn apply_join_block(&mut self, id: &EntityUri) {
        let _ = <EditorPureRef as RefBlockTreeMut>::join_block(&mut self.inner, id);
    }
    async fn apply_indent(&mut self, id: &EntityUri) {
        <EditorPureRef as RefBlockTreeMut>::indent(&mut self.inner, id);
    }
    async fn apply_outdent(&mut self, id: &EntityUri) {
        <EditorPureRef as RefBlockTreeMut>::outdent(&mut self.inner, id);
    }
    async fn apply_move_up(&mut self, id: &EntityUri) {
        if let Some(prev) = self.inner.previous_sibling(id) {
            <EditorPureRef as RefBlockTreeMut>::swap_siblings(&mut self.inner, id, &prev);
        }
    }
    async fn apply_move_down(&mut self, id: &EntityUri) {
        if let Some(next) = self.inner.next_sibling(id) {
            <EditorPureRef as RefBlockTreeMut>::swap_siblings(&mut self.inner, id, &next);
        }
    }
}

impl SutFocusWrite for EditorPureSut {
    async fn apply_navigate_focus(&mut self, region: CapRegion, id: &EntityUri) {
        self.inner.set_focus(region, id.clone(), Default::default());
    }
    async fn apply_focus_editable_text(&mut self, id: &EntityUri) {
        self.inner.set_focus(CapRegion::Main, id.clone(), Default::default());
    }
}

impl SutQuiesce for EditorPureSut {
    async fn quiesce(&mut self) { /* no-op: pure-logic state mutates synchronously */ }
}
```

## Invariants (free functions, taking `(&Ref, &Sut)`)

```rust
use std::collections::HashSet;

/// inv-tree-cursor-within-text-len: cursor never exceeds text length.
pub fn check_cursor_within_text_len(_: &EditorPureRef, sut: &EditorPureSut) -> Result<(), String> {
    if let (Some(text), Some(cursor)) = (
        <EditorPureRef as RefEditorMirror>::active_editor_text(&sut.inner),
        <EditorPureRef as RefEditorMirror>::active_editor_cursor(&sut.inner),
    ) {
        if cursor > text.len() {
            return Err(format!("cursor {cursor} > text len {} for {:?}", text.len(),
                <EditorPureRef as RefEditorMirror>::active_editor_block(&sut.inner)));
        }
    }
    Ok(())
}

/// inv-tree-cursor-text-trim-stable: text never starts/ends with whitespace
/// that would be trimmed by the org-roundtrip normaliser.
pub fn check_text_trim_stable(_: &EditorPureRef, sut: &EditorPureSut) -> Result<(), String> {
    for (id, block) in &sut.inner.blocks {
        if block.content.trim() != block.content {
            return Err(format!("block {id} content '{}' has leading/trailing whitespace", block.content));
        }
    }
    Ok(())
}

/// inv-tree-structural-integrity: every block's parent (except root) exists.
pub fn check_structural_integrity(_: &EditorPureRef, sut: &EditorPureSut) -> Result<(), String> {
    let known_ids: HashSet<&EntityUri> = sut.inner.blocks.keys().collect();
    for (id, block) in &sut.inner.blocks {
        if let Some(parent) = &block.parent {
            if !known_ids.contains(parent) {
                return Err(format!("block {id} parent {parent} does not exist"));
            }
        } else if id != &sut.inner.root_id {
            return Err(format!("non-root block {id} has no parent"));
        }
    }
    Ok(())
}
```

## StateMachineTest impl

```rust
impl proptest_state_machine::StateMachineTest for EditorPureSut {
    type SystemUnderTest = Self;
    type Reference = EditorPureRefMachine;  // wrapper impl'ing ReferenceStateMachine

    fn init_test(_: &<EditorPureRefMachine as ReferenceStateMachine>::State) -> Self {
        EditorPureSut::new_with_seed("hello")
    }

    fn apply(mut sut: Self, _: &<EditorPureRefMachine as ReferenceStateMachine>::State, transition: EditorPureTransition) -> Self {
        // Apply the transition's SUT effect.
        // Note: pure-slice uses `block_on` since there's no real async work.
        futures::executor::block_on(transition.apply_to_sut(&sut.inner, &mut sut));
        sut
    }

    fn check_invariants(sut: &Self, ref_: &EditorPureRef) {
        check_cursor_within_text_len(ref_, sut).expect("cursor invariant");
        check_text_trim_stable(ref_, sut).expect("trim invariant");
        check_structural_integrity(ref_, sut).expect("structural invariant");
    }
}
```

## Approximate total LOC (Phase 5 deliverable)

- `EditorPureRef` struct + impls: ~280 LOC
- `EditorPureSut` struct + impls: ~80 LOC
- Invariant free functions: ~50 LOC
- `StateMachineTest` impl + harness boilerplate: ~50 LOC
- Module header + imports: ~30 LOC

**Total Phase 5 deliverable: ~500 LOC.** Plan said ~150-200; revised estimate based on actual draft is closer to 500. Still <5 s wall budget though — runtime cost is dominated by transition count, not setup.

## Open issues for next session

1. **`block_on(transition.apply_to_sut(...))`** in `StateMachineTest::apply` — proptest-state-machine's `apply` is sync; the transition's `apply_to_sut` is async. Need a sync executor or refactor. Look at how the wide PBT bridges this (`runtime.block_on` per case at `sut.rs:6918`).
2. **`E2ETransitionImpl::preconditions(&self, &ReferenceState)`** vs `holon_pbt_core::TransitionImpl<R, S>::preconditions(&self, &R)` — same shape but different trait. Migration is mechanical but every transition file changes.
3. **`EditorPureTransition` enum** — needs the second `declare_e2e_transitions!` invocation (per H12 Option B). The macro accepts `(ref RefType, sut SutType, ...)` per the H12 patch design.

---

## Why this skeleton is the right Phase 5 starting point

- **No abstraction inside the slice structs** (PbtSlicing.md §4). Only forwarding from `EditorPureSut` → `inner: EditorPureRef`. The "SUT" IS the ref state — and that's correct for a pure slice (no separate production system to diverge from).
- **All invariants are intra-state, not ref↔SUT comparisons** — pure slice has no SUT to diverge from. This means the wide PBT MUST also exercise these invariants (anti-rubber-stamp rule H11) — otherwise the pure slice rubber-stamps itself.
- **Cursor on existing block** invariant is intentionally not listed — the `set_focus` mutator already ensures focus + editor stay in sync. Add only if a bug surfaces it.
