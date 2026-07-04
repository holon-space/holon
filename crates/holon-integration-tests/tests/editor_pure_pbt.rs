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
//!
//! @pbt kind harness
//! @pbt covers editor-transition-sharing — proves migrated editor transitions
//! are SHARED with WideE2E, not duplicated @pbt overlaps
//! general_e2e_composed_pbt — kept as fast storage-free editor fuzz + sharing
//! proof

#![cfg(feature = "pbt")]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::time::Instant;

// The seven T0 transitions — same structs and same _cap fns the wide
// PBT uses (`crate::pbt::transitions::*`).
// `_weighted_generator` free fns are no longer imported: generation now
// routes through the generic `TransitionFactory<EditorPureRef>` trait
// impls via `holon_pbt_core::weighted_arm` (the shared aggregation path).
// The `_apply_to_ref` fns are still called directly by `apply`.
use holon_integration_tests::pbt::transitions::delete_backward::{
    DeleteBackward, delete_backward_apply_to_ref,
};
use holon_integration_tests::pbt::transitions::indent::Indent;
use holon_integration_tests::pbt::transitions::indent::indent_apply_to_ref;
use holon_integration_tests::pbt::transitions::join_block::JoinBlock;
use holon_integration_tests::pbt::transitions::join_block::join_block_apply_to_ref;
use holon_integration_tests::pbt::transitions::move_cursor::MoveCursor;
use holon_integration_tests::pbt::transitions::move_cursor::move_cursor_apply_to_ref;
use holon_integration_tests::pbt::transitions::move_down::MoveDown;
use holon_integration_tests::pbt::transitions::move_down::move_down_apply_to_ref;
use holon_integration_tests::pbt::transitions::move_up::MoveUp;
use holon_integration_tests::pbt::transitions::move_up::move_up_apply_to_ref;
use holon_integration_tests::pbt::transitions::outdent::Outdent;
use holon_integration_tests::pbt::transitions::outdent::outdent_apply_to_ref;
use holon_integration_tests::pbt::transitions::split_block::SplitBlock;
use holon_integration_tests::pbt::transitions::split_block::split_block_apply_to_ref;
use holon_integration_tests::pbt::transitions::type_chars::TypeChars;
use holon_integration_tests::pbt::transitions::type_chars::type_chars_apply_to_ref;
use holon_pbt_core::capabilities::CapCursor;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefLifecycle;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;
use validated::Validated;

// ─────────────────────────────────────────────────────────────────
// EditorPureRef: in-memory backing for the pure slice
// (functionally identical to the H4 spike's EditorPureRef — this test
// proves the same trait surface drives both)
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PureBlock {
    parent: Option<EntityUri>,
    content: String,
    sort_key: String,
    is_text: bool,
}

#[derive(Debug, Clone, Default)]
struct PureEditor {
    block_id: Option<EntityUri>,
    text: String,
    cursor: usize,
}

#[derive(Debug, Clone)]
pub struct EditorPureRef {
    blocks: BTreeMap<EntityUri, PureBlock>,
    root_id: EntityUri,
    editor: PureEditor,
    focus_main: Option<EntityUri>,
    focus_cursor_main: Option<CapCursor>,
    next_id: u64,
    last_transition_kind: Option<&'static str>,
}

impl EditorPureRef {
    fn new() -> Self {
        // EntityUri in this slice MUST parse as EntityUri because the
        // migrated transitions' `_cap` functions accept any R: RefBlockTree
        // (read-side) but Indent/SplitBlock generators produce
        // SplitBlock { block_id: EntityUri, ... } via EntityUri::parse(id).
        // So pure-slice ids use real EntityUri form.
        let root_id = EntityUri::block("pure-root");
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
        let child_id = EntityUri::block("pure-c0");
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

    fn fresh_id(&mut self) -> EntityUri {
        let id = EntityUri::block(&format!("pure-n{}", self.next_id));
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
    fn has_editor_buffer(&self) -> bool {
        // The pure slice owns an editor buffer — that's the reason it exists.
        true
    }
    fn last_transition_kind(&self) -> Option<&'static str> {
        self.last_transition_kind
    }
}

impl RefBlockTree for EditorPureRef {
    fn block_content(&self, id: &EntityUri) -> Option<&str> {
        self.blocks.get(id).map(|b| b.content.as_str())
    }
    fn is_text_block(&self, id: &EntityUri) -> bool {
        self.blocks.get(id).is_some_and(|b| b.is_text)
    }
    fn main_editable_descendants(&self) -> Vec<EntityUri> {
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
    fn focus_root_ids(&self, _: CapRegion) -> BTreeSet<EntityUri> {
        BTreeSet::from([self.root_id.clone()])
    }
    fn previous_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let parent = self.blocks.get(id)?.parent.clone()?;
        let siblings = self.sorted_children(&parent);
        let idx = siblings.iter().position(|s| s == id)?;
        if idx == 0 {
            None
        } else {
            Some(siblings[idx - 1].clone())
        }
    }
    fn next_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let parent = self.blocks.get(id)?.parent.clone()?;
        let siblings = self.sorted_children(&parent);
        let idx = siblings.iter().position(|s| s == id)?;
        siblings.get(idx + 1).cloned()
    }
    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri> {
        self.blocks.get(id)?.parent.clone()
    }
    fn grandparent(&self, id: &EntityUri) -> Option<EntityUri> {
        let p = self.blocks.get(id)?.parent.clone()?;
        self.blocks.get(&p)?.parent.clone()
    }
    fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        let mut kids: Vec<(&EntityUri, &PureBlock)> = self
            .blocks
            .iter()
            .filter(|(_, b)| b.parent.as_ref() == Some(parent))
            .collect();
        kids.sort_by(|a, b| a.1.sort_key.cmp(&b.1.sort_key));
        kids.into_iter().map(|(id, _)| id.clone()).collect()
    }
    fn is_descendant_of_any(&self, id: &EntityUri, ancestors: &BTreeSet<EntityUri>) -> bool {
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
    fn is_layout_block(&self, _: &EntityUri) -> bool {
        false
    }
    fn is_focusable(&self, id: &EntityUri) -> bool {
        self.is_text_block(id) && id != &self.root_id
    }
    fn is_no_content_update(&self, _: &EntityUri) -> bool {
        false
    }
    fn is_page_block(&self, _: &EntityUri) -> bool {
        false
    }
    fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri> {
        self.blocks.keys().cloned().collect()
    }
}

impl RefBlockTreeMut for EditorPureRef {
    fn push_undo_snapshot(&mut self) {}
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

    // ALLOW(unused_param): trait signature requires old_id, unreachable arm below
    fn remint_block(&mut self, _old_id: &EntityUri) -> EntityUri {
        unimplemented!(
            "remint_block: only reachable via StaleExternalRewrite, which requires the composed \
             environment"
        )
    }
    fn join_block(&mut self, id: &EntityUri) -> usize {
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
    fn indent(&mut self, _: &EntityUri) { /* unused: migrated transition uses move_block */
    }
    fn outdent(&mut self, id: &EntityUri) {
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
    fn move_block(&mut self, id: &EntityUri, new_parent: EntityUri, after: Option<&EntityUri>) {
        let after_sort = after.and_then(|a| self.blocks.get(a).map(|b| b.sort_key.clone()));
        let ns = after_sort
            .map(|s| format!("{}m", s))
            .unwrap_or_else(|| "a".into());
        if let Some(b) = self.blocks.get_mut(id) {
            b.parent = Some(new_parent);
            b.sort_key = ns;
        }
    }
    fn swap_siblings(&mut self, a: &EntityUri, b: &EntityUri) {
        let sa = self.blocks.get(a).map(|x| x.sort_key.clone());
        let sb = self.blocks.get(b).map(|x| x.sort_key.clone());
        if let (Some(sa), Some(sb)) = (sa, sb) {
            self.blocks.get_mut(a).unwrap().sort_key = sb;
            self.blocks.get_mut(b).unwrap().sort_key = sa;
        }
    }
}

impl RefEditorMirror for EditorPureRef {
    fn active_editor_block(&self) -> Option<EntityUri> {
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
            // Step back one full char, not one byte — multibyte content (the
            // byte/codepoint bug class these transitions exist to catch) makes
            // `String::remove` panic off a char boundary. Mirrors the
            // production `ReferenceState::ActiveEditor::delete_backward`.
            let prev = self.editor.text[..self.editor.cursor]
                .char_indices()
                .next_back()
                .expect("cursor > 0 implies a preceding char")
                .0;
            self.editor.text.remove(prev);
            self.editor.cursor = prev;
        }
    }
    fn move_cursor(&mut self, byte_position: usize) {
        self.editor.cursor = byte_position.min(self.editor.text.len());
    }
}

impl RefFocus for EditorPureRef {
    fn expected_focus_root_rows(&self) -> Vec<(String, Vec<String>)> {
        Vec::new() // editor-pure slice has no focus roots
    }
    fn navigation_focus_rows(&self) -> Vec<(String, Option<String>)> {
        Vec::new() // editor-pure slice has no navigation history
    }
    fn current_focus(&self, _: CapRegion) -> Option<EntityUri> {
        self.focus_main.clone()
    }
    fn focused_cursor(&self, _: CapRegion) -> Option<CapCursor> {
        self.focus_cursor_main
    }
}

impl RefFocusMut for EditorPureRef {
    fn set_focus(&mut self, _: CapRegion, id: EntityUri, cursor: CapCursor) {
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
    fn clear_focus_if_deleted(&mut self, id: &EntityUri) {
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
    use holon_pbt_core::weighted_arm;
    use proptest::strategy::Union;
    let mut arms: Vec<(u32, BoxedStrategy<PureTransition>)> = vec![];

    // One arm per variant via the shared `holon_pbt_core::weighted_arm`
    // helper, calling the SAME generic `TransitionFactory<EditorPureRef>`
    // impls the wide PBT uses (no per-variant weight tuning → multiplier
    // `1`; rejections discarded — the pure slice has no rejection log).
    macro_rules! arm {
        ($ty:ty, $variant:path) => {
            if let Validated::Good(Some(a)) =
                weighted_arm::<_, $ty, PureTransition>(state, 1, $variant)
            {
                arms.push(a);
            }
        };
    }
    arm!(TypeChars, PureTransition::TypeChars);
    arm!(DeleteBackward, PureTransition::DeleteBackward);
    arm!(MoveCursor, PureTransition::MoveCursor);
    arm!(MoveUp, PureTransition::MoveUp);
    arm!(MoveDown, PureTransition::MoveDown);
    arm!(SplitBlock, PureTransition::SplitBlock);
    arm!(JoinBlock, PureTransition::JoinBlock);
    arm!(Indent, PureTransition::Indent);
    arm!(Outdent, PureTransition::Outdent);

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
            PureTransition::MoveUp(t) => move_up_apply_to_ref(&t.block_id, &mut state),
            PureTransition::MoveDown(t) => move_down_apply_to_ref(&t.block_id, &mut state),
            PureTransition::SplitBlock(t) => {
                split_block_apply_to_ref(&t.block_id, t.position, &mut state)
            }
            PureTransition::JoinBlock(t) => join_block_apply_to_ref(&t.block_id, &mut state),
            PureTransition::Indent(t) => indent_apply_to_ref(&t.block_id, &mut state),
            PureTransition::Outdent(t) => outdent_apply_to_ref(&t.block_id, &mut state),
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
            PureTransition::MoveUp(t) => move_up_apply_to_ref(&t.block_id, &mut sut.inner),
            PureTransition::MoveDown(t) => move_down_apply_to_ref(&t.block_id, &mut sut.inner),
            PureTransition::SplitBlock(t) => {
                split_block_apply_to_ref(&t.block_id, t.position, &mut sut.inner)
            }
            PureTransition::JoinBlock(t) => join_block_apply_to_ref(&t.block_id, &mut sut.inner),
            PureTransition::Indent(t) => indent_apply_to_ref(&t.block_id, &mut sut.inner),
            PureTransition::Outdent(t) => outdent_apply_to_ref(&t.block_id, &mut sut.inner),
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
        let known: BTreeSet<&EntityUri> = sut.inner.blocks.keys().collect();
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
        failure_persistence: None,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn editor_pure_pbt(sequential 1..30 => EditorPureSut);
}

/// Microbenchmark: the SAME measurement as the H4 spike, but against the
/// production transition structs imported from the integration-tests
/// crate. Confirms shared code paths run at the same per-transition cost
/// (~50 µs). Run with `cargo test --features pbt -- h4_microbenchmark_shared
/// --nocapture`.
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

// ── Phase 2 cross-medium proof: generic transition factories ──────────
//
// `EditorPureRef` is NOT the E2E `ReferenceState`; it implements only the
// `Ref*` capability traits. These assertions compile only because the wide-PBT
// transition `TransitionFactory`/`TransitionRef` impls are now generic over the
// reference type (`impl<R: RefBlockTree + ...>`) rather than bound to the
// concrete `ReferenceState`. If any reverts to
// `TransitionFactory<ReferenceState>`, this stops compiling.

fn assert_factory_ref_generic<R, T>()
where
    T: holon_pbt_core::TransitionFactory<R> + holon_pbt_core::TransitionRef<R>,
{
}

#[test]
fn wide_transition_factories_run_on_non_e2e_ref() {
    assert_factory_ref_generic::<EditorPureRef, Indent>();
    assert_factory_ref_generic::<EditorPureRef, Outdent>();
    assert_factory_ref_generic::<EditorPureRef, SplitBlock>();
    assert_factory_ref_generic::<EditorPureRef, JoinBlock>();
    assert_factory_ref_generic::<EditorPureRef, MoveUp>();
    assert_factory_ref_generic::<EditorPureRef, MoveDown>();
    assert_factory_ref_generic::<EditorPureRef, TypeChars>();
    assert_factory_ref_generic::<EditorPureRef, DeleteBackward>();
    assert_factory_ref_generic::<EditorPureRef, MoveCursor>();
}
