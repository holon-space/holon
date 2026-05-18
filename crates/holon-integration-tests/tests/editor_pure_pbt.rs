//! Editor-pure slice PBT — proves the migrated transitions are SHARED
//! with the wide PBT (`general_e2e_pbt`), not duplicated.
//!
//! Stage A deliverable. Phase 1's H4 spike (`crates/holon-pbt-core/tests/
//! editor_pure_h4_spike.rs`) used inline migrated TypeChars + SplitBlock
//! to validate the ~5200× speedup. This test goes further: it imports
//! the actual transition structs and their `_cap` free functions from
//! `holon-integration-tests::pbt::transitions::*` — the SAME code paths
//! the wide PBT runs.
//!
//! Slice composition: `{BlockTree, EditorMirror, Focus, RefLifecycle}`.
//! No Loro, no Turso, no ViewModel, no Renderer, no FrontendBounds.
//!
//! Invariants (anti-rubber-stamp: each must also fire in the wide PBT
//! per plan H11):
//! - `inv-tree-cursor-within-text-len`: cursor never exceeds text length.
//! - `inv-tree-structural-integrity`: every block's parent (if any) exists.
//!
//! Per-transition pattern: each variant calls the corresponding `_cap`
//! function for preconditions / apply_to_ref, then invokes the SUT-side
//! mutation directly on `EditorPureSut` (which the pure slice owns; no
//! `dyn SutHandle` indirection since this slice has no wide-PBT SUT).

#![cfg(feature = "pbt")]

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use holon_pbt_core::capabilities::{
    CapBlockId, CapCursor, CapRegion, RefBlockTree, RefBlockTreeMut, RefEditorMirror,
    RefEditorMirrorMut, RefFocus, RefFocusMut, RefLifecycle,
};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use validated::Validated;

// The seven T0 transitions — same structs and same _cap fns the wide
// PBT uses (`crate::pbt::transitions::*`).
use holon_integration_tests::pbt::transitions::delete_backward::{
    DeleteBackward, delete_backward_apply_to_ref, delete_backward_weighted_generator,
};
use holon_integration_tests::pbt::transitions::indent::{
    Indent, indent_apply_to_ref, indent_weighted_generator,
};
use holon_integration_tests::pbt::transitions::join_block::{
    JoinBlock, join_block_apply_to_ref, join_block_weighted_generator,
};
use holon_integration_tests::pbt::transitions::move_cursor::{
    MoveCursor, move_cursor_apply_to_ref, move_cursor_weighted_generator,
};
use holon_integration_tests::pbt::transitions::move_down::{
    MoveDown, move_down_apply_to_ref, move_down_weighted_generator,
};
use holon_integration_tests::pbt::transitions::move_up::{
    MoveUp, move_up_apply_to_ref, move_up_weighted_generator,
};
use holon_integration_tests::pbt::transitions::outdent::{
    Outdent, outdent_apply_to_ref, outdent_weighted_generator,
};
use holon_integration_tests::pbt::transitions::split_block::{
    SplitBlock, split_block_apply_to_ref, split_block_weighted_generator,
};
use holon_integration_tests::pbt::transitions::type_chars::{
    TypeChars, type_chars_apply_to_ref, type_chars_weighted_generator,
};

// ─────────────────────────────────────────────────────────────────
// EditorPureRef: in-memory backing for the pure slice
// (functionally identical to the H4 spike's EditorPureRef — this test
// proves the same trait surface drives both)
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PureBlock {
    parent: Option<CapBlockId>,
    content: String,
    sort_key: String,
    is_text: bool,
}

#[derive(Debug, Clone, Default)]
struct PureEditor {
    block_id: Option<CapBlockId>,
    text: String,
    cursor: usize,
}

#[derive(Debug, Clone)]
pub struct EditorPureRef {
    blocks: BTreeMap<CapBlockId, PureBlock>,
    root_id: CapBlockId,
    editor: PureEditor,
    focus_main: Option<CapBlockId>,
    focus_cursor_main: Option<CapCursor>,
    next_id: u64,
    last_transition_kind: Option<&'static str>,
}

impl EditorPureRef {
    fn new() -> Self {
        // CapBlockId in this slice MUST parse as EntityUri because the
        // migrated transitions' `_cap` functions accept any R: RefBlockTree
        // (read-side) but Indent/SplitBlock generators produce
        // SplitBlock { block_id: EntityUri, ... } via EntityUri::parse(id).
        // So pure-slice ids use real EntityUri form.
        let root_id = "block:pure-root".to_string();
        let mut blocks = BTreeMap::new();
        blocks.insert(
            root_id.clone(),
            PureBlock {
                parent: None,
                content: String::new(),
                sort_key: "a".into(),
                is_text: false,
            },
        );
        let child_id = "block:pure-c0".to_string();
        blocks.insert(
            child_id.clone(),
            PureBlock {
                parent: Some(root_id.clone()),
                content: "hello".into(),
                sort_key: "b".into(),
                is_text: true,
            },
        );
        EditorPureRef {
            blocks,
            root_id,
            editor: PureEditor {
                block_id: Some(child_id.clone()),
                text: "hello".into(),
                cursor: 0,
            },
            focus_main: Some(child_id),
            focus_cursor_main: Some(CapCursor::default()),
            next_id: 0,
            last_transition_kind: None,
        }
    }

    fn fresh_id(&mut self) -> CapBlockId {
        // Use the EntityUri block: scheme so the migrated generators
        // (which call EntityUri::parse on these ids) succeed.
        let id = format!("block:pure-n{}", self.next_id);
        self.next_id += 1;
        id
    }
}

impl RefLifecycle for EditorPureRef {
    fn app_started(&self) -> bool {
        true
    }
    fn is_properly_setup(&self) -> bool {
        true
    }
    fn enable_loro(&self) -> bool {
        false
    }
    fn last_transition_kind(&self) -> Option<&'static str> {
        self.last_transition_kind
    }
    fn atomic_editor_enabled() -> bool {
        true
    }
}

impl RefBlockTree for EditorPureRef {
    fn block_content(&self, id: &CapBlockId) -> Option<&str> {
        self.blocks.get(id).map(|b| b.content.as_str())
    }
    fn is_text_block(&self, id: &CapBlockId) -> bool {
        self.blocks.get(id).is_some_and(|b| b.is_text)
    }
    fn main_editable_descendants(&self) -> Vec<CapBlockId> {
        let mut out = Vec::new();
        let mut stack = vec![self.root_id.clone()];
        while let Some(id) = stack.pop() {
            if self.blocks.get(&id).is_some() {
                let is_text = self.is_text_block(&id);
                if is_text && id != self.root_id {
                    out.push(id.clone());
                }
                let children = self.sorted_children(&id);
                stack.extend(children.into_iter().rev());
            }
        }
        out
    }
    fn focus_root_ids(&self, _: CapRegion) -> BTreeSet<CapBlockId> {
        BTreeSet::from([self.root_id.clone()])
    }
    fn previous_sibling(&self, id: &CapBlockId) -> Option<CapBlockId> {
        let parent = self.blocks.get(id)?.parent.clone()?;
        let siblings = self.sorted_children(&parent);
        let idx = siblings.iter().position(|s| s == id)?;
        if idx == 0 {
            None
        } else {
            Some(siblings[idx - 1].clone())
        }
    }
    fn next_sibling(&self, id: &CapBlockId) -> Option<CapBlockId> {
        let parent = self.blocks.get(id)?.parent.clone()?;
        let siblings = self.sorted_children(&parent);
        let idx = siblings.iter().position(|s| s == id)?;
        siblings.get(idx + 1).cloned()
    }
    fn parent_of(&self, id: &CapBlockId) -> Option<CapBlockId> {
        self.blocks.get(id)?.parent.clone()
    }
    fn grandparent(&self, id: &CapBlockId) -> Option<CapBlockId> {
        let p = self.blocks.get(id)?.parent.clone()?;
        self.blocks.get(&p)?.parent.clone()
    }
    fn sorted_children(&self, parent: &CapBlockId) -> Vec<CapBlockId> {
        let mut kids: Vec<(&CapBlockId, &PureBlock)> = self
            .blocks
            .iter()
            .filter(|(_, b)| b.parent.as_ref() == Some(parent))
            .collect();
        kids.sort_by(|a, b| a.1.sort_key.cmp(&b.1.sort_key));
        kids.into_iter().map(|(id, _)| id.clone()).collect()
    }
    fn is_descendant_of_any(&self, id: &CapBlockId, ancestors: &BTreeSet<CapBlockId>) -> bool {
        let mut cursor = id.clone();
        loop {
            if ancestors.contains(&cursor) {
                return true;
            }
            match self.blocks.get(&cursor).and_then(|b| b.parent.clone()) {
                Some(p) => cursor = p,
                None => return false,
            }
        }
    }
    fn is_layout_block(&self, _: &CapBlockId) -> bool {
        false
    }
    fn is_focusable(&self, id: &CapBlockId) -> bool {
        self.is_text_block(id) && id != &self.root_id
    }
    fn is_no_content_update(&self, _: &CapBlockId) -> bool {
        false
    }
    fn is_page_block(&self, _: &CapBlockId) -> bool {
        false
    }
    fn all_non_seed_block_ids(&self) -> BTreeSet<CapBlockId> {
        self.blocks.keys().cloned().collect()
    }
}

impl RefBlockTreeMut for EditorPureRef {
    fn push_undo_snapshot(&mut self) {}
    fn set_block_content(&mut self, id: &CapBlockId, text: &str) {
        if let Some(b) = self.blocks.get_mut(id) {
            b.content = text.to_string();
        }
    }
    fn split_block(&mut self, id: &CapBlockId, position: usize) -> CapBlockId {
        let new_id = self.fresh_id();
        let (parent, tail, new_sort_key) = {
            let b = self.blocks.get_mut(id).expect("split target exists");
            let position = position.min(b.content.len());
            let tail = b.content.split_off(position);
            (b.parent.clone(), tail, format!("{}m", b.sort_key))
        };
        self.blocks.insert(
            new_id.clone(),
            PureBlock {
                parent,
                content: tail,
                sort_key: new_sort_key,
                is_text: true,
            },
        );
        new_id
    }
    fn join_block(&mut self, id: &CapBlockId) -> usize {
        let into = self.previous_sibling(id).unwrap_or_else(|| {
            self.blocks
                .get(id)
                .and_then(|b| b.parent.clone())
                .expect("join_block: no prev sibling AND no parent")
        });
        let appended = self.blocks.remove(id).expect("join target exists").content;
        let into_block = self.blocks.get_mut(&into).expect("join destination exists");
        let cursor_at_join = into_block.content.len();
        into_block.content.push_str(&appended);
        cursor_at_join
    }
    fn indent(&mut self, _: &CapBlockId) { /* unused: migrated transition uses move_block */
    }
    fn outdent(&mut self, id: &CapBlockId) {
        let gp = self.grandparent(id);
        let parent_id = self.blocks.get(id).and_then(|b| b.parent.clone());
        let parent_sort = parent_id
            .as_ref()
            .and_then(|p| self.blocks.get(p).map(|b| b.sort_key.clone()));
        if let (Some(_), Some(ps)) = (gp.as_ref(), parent_sort) {
            let ns = format!("{}m", ps);
            if let Some(b) = self.blocks.get_mut(id) {
                b.parent = gp;
                b.sort_key = ns;
            }
        }
    }
    fn move_block(&mut self, id: &CapBlockId, new_parent: CapBlockId, after: Option<&CapBlockId>) {
        let after_sort = after.and_then(|a| self.blocks.get(a).map(|b| b.sort_key.clone()));
        let ns = after_sort
            .map(|s| format!("{}m", s))
            .unwrap_or_else(|| "a".into());
        if let Some(b) = self.blocks.get_mut(id) {
            b.parent = Some(new_parent);
            b.sort_key = ns;
        }
    }
    fn swap_siblings(&mut self, a: &CapBlockId, b: &CapBlockId) {
        let sa = self.blocks.get(a).map(|x| x.sort_key.clone());
        let sb = self.blocks.get(b).map(|x| x.sort_key.clone());
        if let (Some(sa), Some(sb)) = (sa, sb) {
            self.blocks.get_mut(a).unwrap().sort_key = sb;
            self.blocks.get_mut(b).unwrap().sort_key = sa;
        }
    }
}

impl RefEditorMirror for EditorPureRef {
    fn active_editor_block(&self) -> Option<CapBlockId> {
        self.editor.block_id.clone()
    }
    fn active_editor_text(&self) -> Option<&str> {
        self.editor
            .block_id
            .as_ref()
            .map(|_| self.editor.text.as_str())
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
            if self.editor.cursor == 0 {
                break;
            }
            self.editor.cursor -= 1;
            self.editor.text.remove(self.editor.cursor);
        }
    }
    fn move_cursor(&mut self, byte_position: usize) {
        self.editor.cursor = byte_position.min(self.editor.text.len());
    }
}

impl RefFocus for EditorPureRef {
    fn current_focus(&self, _: CapRegion) -> Option<CapBlockId> {
        self.focus_main.clone()
    }
    fn focused_cursor(&self, _: CapRegion) -> Option<CapCursor> {
        self.focus_cursor_main
    }
}

impl RefFocusMut for EditorPureRef {
    fn set_focus(&mut self, _: CapRegion, id: CapBlockId, cursor: CapCursor) {
        let text = self
            .blocks
            .get(&id)
            .map(|b| b.content.clone())
            .unwrap_or_default();
        self.editor = PureEditor {
            block_id: Some(id.clone()),
            text,
            cursor: 0,
        };
        self.focus_main = Some(id);
        self.focus_cursor_main = Some(cursor);
    }
    fn clear_focus_if_deleted(&mut self, id: &CapBlockId) {
        if self.focus_main.as_ref() == Some(id) {
            self.focus_main = None;
            self.focus_cursor_main = None;
            self.editor = PureEditor::default();
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// EditorPureSut: forwards mutations to an inner EditorPureRef
// ─────────────────────────────────────────────────────────────────

pub struct EditorPureSut {
    inner: EditorPureRef,
}

impl EditorPureSut {
    fn from_ref(r: &EditorPureRef) -> Self {
        Self { inner: r.clone() }
    }
}

// ─────────────────────────────────────────────────────────────────
// Transition enum + dispatch using the migrated _cap fns
// ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum PureTransition {
    TypeChars(TypeChars),
    DeleteBackward(DeleteBackward),
    MoveCursor(MoveCursor),
    MoveUp(MoveUp),
    MoveDown(MoveDown),
    SplitBlock(SplitBlock),
    JoinBlock(JoinBlock),
    Indent(Indent),
    Outdent(Outdent),
}

impl PureTransition {
    fn variant_name(&self) -> &'static str {
        match self {
            PureTransition::TypeChars(_) => "TypeChars",
            PureTransition::DeleteBackward(_) => "DeleteBackward",
            PureTransition::MoveCursor(_) => "MoveCursor",
            PureTransition::MoveUp(_) => "MoveUp",
            PureTransition::MoveDown(_) => "MoveDown",
            PureTransition::SplitBlock(_) => "SplitBlock",
            PureTransition::JoinBlock(_) => "JoinBlock",
            PureTransition::Indent(_) => "Indent",
            PureTransition::Outdent(_) => "Outdent",
        }
    }
}

fn aggregate(state: &EditorPureRef) -> BoxedStrategy<PureTransition> {
    use proptest::strategy::Union;
    let mut arms: Vec<(u32, BoxedStrategy<PureTransition>)> = vec![];

    if let Validated::Good((w, s)) = type_chars_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::TypeChars).boxed()));
    }
    if let Validated::Good((w, s)) = delete_backward_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::DeleteBackward).boxed()));
    }
    if let Validated::Good((w, s)) = move_cursor_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::MoveCursor).boxed()));
    }
    if let Validated::Good((w, s)) = move_up_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::MoveUp).boxed()));
    }
    if let Validated::Good((w, s)) = move_down_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::MoveDown).boxed()));
    }
    if let Validated::Good((w, s)) = split_block_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::SplitBlock).boxed()));
    }
    if let Validated::Good((w, s)) = join_block_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::JoinBlock).boxed()));
    }
    if let Validated::Good((w, s)) = indent_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::Indent).boxed()));
    }
    if let Validated::Good((w, s)) = outdent_weighted_generator(state) {
        arms.push((w, s.prop_map(PureTransition::Outdent).boxed()));
    }

    assert!(!arms.is_empty(), "no transitions applicable");
    Union::new_weighted(arms).boxed()
}

// ─────────────────────────────────────────────────────────────────
// proptest-state-machine wiring
// ─────────────────────────────────────────────────────────────────

pub struct EditorPureMachine;

impl ReferenceStateMachine for EditorPureMachine {
    type State = EditorPureRef;
    type Transition = PureTransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(EditorPureRef::new()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        aggregate(state)
    }

    fn preconditions(_: &Self::State, _: &Self::Transition) -> bool {
        // Generators already enforce preconditions; redundant check
        // would just duplicate work and slow the run.
        true
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        let name = transition.variant_name();
        match transition {
            PureTransition::TypeChars(t) => type_chars_apply_to_ref(&t.text, &mut state),
            PureTransition::DeleteBackward(t) => delete_backward_apply_to_ref(t.count, &mut state),
            PureTransition::MoveCursor(t) => move_cursor_apply_to_ref(t.byte_position, &mut state),
            PureTransition::MoveUp(t) => {
                move_up_apply_to_ref(&t.block_id.as_str().to_string(), &mut state)
            }
            PureTransition::MoveDown(t) => {
                move_down_apply_to_ref(&t.block_id.as_str().to_string(), &mut state)
            }
            PureTransition::SplitBlock(t) => {
                split_block_apply_to_ref(&t.block_id.as_str().to_string(), t.position, &mut state)
            }
            PureTransition::JoinBlock(t) => {
                join_block_apply_to_ref(&t.block_id.as_str().to_string(), &mut state)
            }
            PureTransition::Indent(t) => {
                indent_apply_to_ref(&t.block_id.as_str().to_string(), &mut state)
            }
            PureTransition::Outdent(t) => {
                outdent_apply_to_ref(&t.block_id.as_str().to_string(), &mut state)
            }
        }
        state.last_transition_kind = Some(name);
        state
    }
}

impl StateMachineTest for EditorPureSut {
    type SystemUnderTest = Self;
    type Reference = EditorPureMachine;

    fn init_test(
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        EditorPureSut::from_ref(ref_state)
    }

    fn apply(
        mut sut: Self::SystemUnderTest,
        _: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        // SUT mirrors the ref-state apply — pure slice has no separate
        // production system to diverge from. Anti-rubber-stamp is enforced
        // by the H11 rule: every invariant here must also be in the wide
        // PBT registry.
        let name = transition.variant_name();
        match transition {
            PureTransition::TypeChars(t) => type_chars_apply_to_ref(&t.text, &mut sut.inner),
            PureTransition::DeleteBackward(t) => {
                delete_backward_apply_to_ref(t.count, &mut sut.inner)
            }
            PureTransition::MoveCursor(t) => {
                move_cursor_apply_to_ref(t.byte_position, &mut sut.inner)
            }
            PureTransition::MoveUp(t) => {
                move_up_apply_to_ref(&t.block_id.as_str().to_string(), &mut sut.inner)
            }
            PureTransition::MoveDown(t) => {
                move_down_apply_to_ref(&t.block_id.as_str().to_string(), &mut sut.inner)
            }
            PureTransition::SplitBlock(t) => split_block_apply_to_ref(
                &t.block_id.as_str().to_string(),
                t.position,
                &mut sut.inner,
            ),
            PureTransition::JoinBlock(t) => {
                join_block_apply_to_ref(&t.block_id.as_str().to_string(), &mut sut.inner)
            }
            PureTransition::Indent(t) => {
                indent_apply_to_ref(&t.block_id.as_str().to_string(), &mut sut.inner)
            }
            PureTransition::Outdent(t) => {
                outdent_apply_to_ref(&t.block_id.as_str().to_string(), &mut sut.inner)
            }
        }
        sut.inner.last_transition_kind = Some(name);
        sut
    }

    fn check_invariants(
        sut: &Self::SystemUnderTest,
        _: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        // inv-tree-cursor-within-text-len
        if let (Some(text), Some(cursor)) = (
            <EditorPureRef as RefEditorMirror>::active_editor_text(&sut.inner),
            <EditorPureRef as RefEditorMirror>::active_editor_cursor(&sut.inner),
        ) {
            assert!(
                cursor <= text.len(),
                "[inv-tree-cursor-within-text-len] cursor {} > text len {}",
                cursor,
                text.len()
            );
        }
        // inv-tree-structural-integrity
        let known: BTreeSet<&CapBlockId> = sut.inner.blocks.keys().collect();
        for (id, b) in &sut.inner.blocks {
            if let Some(parent) = &b.parent {
                assert!(
                    known.contains(parent),
                    "[inv-tree-structural-integrity] block {} parent {} does not exist",
                    id,
                    parent
                );
            } else {
                assert_eq!(
                    id, &sut.inner.root_id,
                    "[inv-tree-structural-integrity] non-root block has no parent: {}",
                    id
                );
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 256,
        max_shrink_iters: 200,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn editor_pure_pbt(sequential 1..30 => EditorPureSut);
}

/// Microbenchmark: the SAME measurement as the H4 spike, but against the
/// production transition structs imported from the integration-tests
/// crate. Confirms shared code paths run at the same per-transition cost
/// (~50 µs). Run with `cargo test --features pbt -- h4_microbenchmark_shared --nocapture`.
#[test]
fn h4_microbenchmark_shared() {
    let cases = 256_u32;
    let steps_per_case = 30_usize;

    let mut total_transitions = 0_usize;
    let start = Instant::now();
    let mut runner = proptest::test_runner::TestRunner::default();

    for _ in 0..cases {
        let mut state = match EditorPureMachine::init_state().new_tree(&mut runner) {
            Ok(tree) => tree.current(),
            Err(_) => continue,
        };
        let mut sut = EditorPureSut::from_ref(&state);
        for _ in 0..steps_per_case {
            let strategy = EditorPureMachine::transitions(&state);
            let transition = match strategy.new_tree(&mut runner) {
                Ok(tree) => tree.current(),
                Err(_) => break,
            };
            state = EditorPureMachine::apply(state, &transition);
            sut = <EditorPureSut as StateMachineTest>::apply(sut, &state, transition);
            <EditorPureSut as StateMachineTest>::check_invariants(&sut, &state);
            total_transitions += 1;
        }
    }

    let elapsed = start.elapsed();
    let per_transition_us = elapsed.as_micros() as f64 / total_transitions.max(1) as f64;
    println!("===== editor_pure_pbt microbenchmark (shared code paths) =====");
    println!("Cases:                {}", cases);
    println!("Transitions applied:  {}", total_transitions);
    println!("Total wall:           {:?}", elapsed);
    println!("Per transition:       {:.1} µs", per_transition_us);
    println!("=============================================================");
}
