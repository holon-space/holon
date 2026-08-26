//! H4 keystone spike — self-contained pure-slice PBT.
//!
//! Validates:
//! - H1: pbt-core's `TransitionFactory` / `TransitionImpl` are sufficient.
//! - H2: the 6 reference capability traits (+ `RefLifecycle`) cover what a
//!   pure-editor transition needs.
//! - H4: pure-slice PBT cost is dramatically lower than wide-PBT cost.
//! - H9: generators rebind to capability bounds with no new helpers.
//!
//! Self-contained — no deps on `holon-integration-tests` or `holon-frontend`.
//! Uses only `holon-pbt-core::capabilities` + `proptest-state-machine`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::time::Instant;

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
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
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutEditorMirrorWrite;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::capabilities::SutQuiesce;
use holon_pbt_core::capabilities::commit_active_editor_if_changed;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use validated::Validated;

// ─────────────────────────────────────────────────────────────────
// EditorPureRef: in-memory backing for the pure slice
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
        let root_id = EntityUri::block("root");
        let mut blocks = BTreeMap::new();
        blocks.insert(
            root_id.clone(),
            PureBlock {
                parent: None,
                content: String::new(),
                sort_key: "a".into(),
                is_text: false, // root is non-text
            },
        );
        // Seed with one focusable text child so transitions have something to act on.
        let child_id = EntityUri::block("child0");
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
            next_id: 1,
            last_transition_kind: None,
        }
    }

    fn fresh_id(&mut self) -> EntityUri {
        let id = EntityUri::block(&format!("n{}", self.next_id));
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
            if self.blocks.contains_key(&id) {
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
    fn main_panel_renders(&self, id: &EntityUri) -> bool {
        // No panel query and no pages in this slice: everything under the
        // single root renders.
        self.is_descendant_of_any(id, &BTreeSet::from([self.root_id.clone()]))
    }
    fn owns_query_source(&self, _: &EntityUri) -> bool {
        false
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
    fn all_non_seed_block_ids(&self) -> std::collections::BTreeSet<EntityUri> {
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
    /// The id follows the text: at position 0 `id` keeps the whole string and
    /// the minted block is the empty one sorted BEFORE it, so the returned
    /// focus target is `id` itself.
    fn split_block(&mut self, id: &EntityUri, position: usize) -> EntityUri {
        let new_id = self.fresh_id();
        let at_start = position == 0;
        let (parent, minted_content, new_sort_key) = {
            let b = self.blocks.get_mut(id).expect("split target exists");
            let position = position.min(b.content.len());
            let tail = b.content.split_off(position);
            if at_start {
                // The minted (empty) block takes the origin's slot and the
                // origin steps one slot down, so it ends up BELOW.
                let stepped_key = format!("{}m", b.sort_key);
                let minted_key = std::mem::replace(&mut b.sort_key, stepped_key);
                let minted = std::mem::replace(&mut b.content, tail);
                (b.parent.clone(), minted, minted_key)
            } else {
                let sort_key = format!("{}m", b.sort_key);
                (b.parent.clone(), tail, sort_key)
            }
        };
        self.blocks.insert(
            new_id.clone(),
            PureBlock {
                parent,
                content: minted_content,
                sort_key: new_sort_key,
                is_text: true,
            },
        );
        if at_start { id.clone() } else { new_id }
    }

    fn remint_block(
        &mut self,
        // ALLOW(unused_param): trait signature requires old_id, unreachable arm below
        _old_id: &EntityUri,
    ) -> EntityUri {
        unimplemented!(
            "remint_block: only reachable via StaleExternalRewrite, which requires the composed \
             environment"
        )
    }
    fn join_block(&mut self, id: &EntityUri) -> usize {
        let into = holon_pbt_core::capabilities::join_merge_target(id, self)
            .expect("join_block: no prev sibling AND no parent");
        // The joined-away block's children move onto the merge target, as in
        // `ReferenceState::join_block`; dropping them orphans a subtree.
        for block in self.blocks.values_mut() {
            if block.parent.as_ref() == Some(id) {
                block.parent = Some(into.clone());
            }
        }
        let appended = self.blocks.remove(id).expect("join target exists").content;
        let into_block = self.blocks.get_mut(&into).expect("join destination exists");
        let cursor_at_join = into_block.content.len();
        into_block.content.push_str(&appended);
        cursor_at_join
    }
    fn indent(&mut self, id: &EntityUri) {
        let prev = self
            .previous_sibling(id)
            .expect("indent: previous sibling required");
        let last_sort = self
            .sorted_children(&prev)
            .last()
            .and_then(|cid| self.blocks.get(cid).map(|b| b.sort_key.clone()));
        let new_sort = last_sort
            .map(|s| format!("{}m", s))
            .unwrap_or_else(|| "a".into());
        if let Some(b) = self.blocks.get_mut(id) {
            b.parent = Some(prev);
            b.sort_key = new_sort;
        }
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
            self.editor.cursor -= 1;
            self.editor.text.remove(self.editor.cursor);
        }
    }
    fn move_cursor(&mut self, byte_position: usize) {
        self.editor.cursor = byte_position.min(self.editor.text.len());
    }
    fn reseed_active_editor(&mut self, text: &str, cursor: usize) {
        self.editor.text = text.to_string();
        self.editor.cursor = cursor.min(self.editor.text.len());
    }
}

impl RefFocus for EditorPureRef {
    fn expected_focus_root_rows(&self) -> Vec<(String, Vec<String>)> {
        Vec::new()
    }
    fn navigation_focus_rows(&self) -> Vec<(String, Option<String>)> {
        Vec::new()
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
// EditorPureSut: thin wrapper, forwards to inner ref
// ─────────────────────────────────────────────────────────────────

pub struct EditorPureSut {
    // Interior mutability so the write caps are `&self` (hostable on `CapMap`).
    inner: std::cell::RefCell<EditorPureRef>,
}

impl EditorPureSut {
    fn new(seed: &EditorPureRef) -> Self {
        Self {
            inner: std::cell::RefCell::new(seed.clone()),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutEditorMirrorWrite for EditorPureSut {
    async fn apply_type_chars(&self, text: &str) {
        <EditorPureRef as RefEditorMirrorMut>::type_chars(&mut self.inner.borrow_mut(), text);
    }
    async fn apply_delete_backward(&self, count: usize) {
        <EditorPureRef as RefEditorMirrorMut>::delete_backward(&mut self.inner.borrow_mut(), count);
    }
    async fn apply_move_cursor(&self, byte_position: usize) {
        <EditorPureRef as RefEditorMirrorMut>::move_cursor(
            &mut self.inner.borrow_mut(),
            byte_position,
        );
    }
}

#[async_trait::async_trait(?Send)]
impl SutBlockTreeWrite for EditorPureSut {
    async fn apply_split_block(&self, id: &EntityUri, position: usize) {
        let mut inner = self.inner.borrow_mut();
        let new_id = <EditorPureRef as RefBlockTreeMut>::split_block(&mut inner, id, position);
        <EditorPureRef as RefFocusMut>::set_focus(
            &mut inner,
            CapRegion::Main,
            new_id,
            CapCursor::default(),
        );
    }
    async fn apply_join_block(&self, id: &EntityUri) {
        let _ = <EditorPureRef as RefBlockTreeMut>::join_block(&mut self.inner.borrow_mut(), id);
    }
    async fn apply_indent(&self, id: &EntityUri) {
        <EditorPureRef as RefBlockTreeMut>::indent(&mut self.inner.borrow_mut(), id);
    }
    async fn apply_outdent(&self, id: &EntityUri) {
        <EditorPureRef as RefBlockTreeMut>::outdent(&mut self.inner.borrow_mut(), id);
    }
    async fn apply_move_up(&self, id: &EntityUri) {
        let prev = self.inner.borrow().previous_sibling(id);
        if let Some(prev) = prev {
            <EditorPureRef as RefBlockTreeMut>::swap_siblings(
                &mut self.inner.borrow_mut(),
                id,
                &prev,
            );
        }
    }
    async fn apply_move_down(&self, id: &EntityUri) {
        let next = self.inner.borrow().next_sibling(id);
        if let Some(next) = next {
            <EditorPureRef as RefBlockTreeMut>::swap_siblings(
                &mut self.inner.borrow_mut(),
                id,
                &next,
            );
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutFocusWrite for EditorPureSut {
    async fn apply_navigate_focus(&self, region: CapRegion, id: &EntityUri) {
        <EditorPureRef as RefFocusMut>::set_focus(
            &mut self.inner.borrow_mut(),
            region,
            id.clone(),
            CapCursor::default(),
        );
    }
    async fn apply_focus_editable_text(&self, id: &EntityUri) {
        <EditorPureRef as RefFocusMut>::set_focus(
            &mut self.inner.borrow_mut(),
            CapRegion::Main,
            id.clone(),
            CapCursor::default(),
        );
    }
}

#[async_trait::async_trait(?Send)]
impl SutQuiesce for EditorPureSut {
    async fn quiesce(&self) {}
}

// ─────────────────────────────────────────────────────────────────
// Two migrated transitions (TypeChars + SplitBlock) — capability-bound
// ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Rej {
    LoroOff,
    NotStarted,
    NotSetup,
    NoFocus,
    NoEditor,
    BadPosition,
    NoCandidates,
    NotText,
    NotInFocusTree,
}

fn ok<E>(cond: bool, e: E) -> Validated<(), E> {
    if cond {
        Validated::Good(())
    } else {
        Validated::fail(e)
    }
}

#[derive(Clone, Debug)]
pub struct TypeChars {
    pub text: String,
}

impl TypeChars {
    fn preconditions_static<R: RefEditorMirror + RefFocus + RefLifecycle>(
        state: &R,
    ) -> Validated<(), Rej> {
        let checks: Vec<Validated<(), Rej>> = vec![
            ok(state.app_started(), Rej::NotStarted),
            ok(state.is_properly_setup(), Rej::NotSetup),
            ok(state.current_focus(CapRegion::Main).is_some(), Rej::NoFocus),
            ok(state.active_editor_block().is_some(), Rej::NoEditor),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }
}

impl<R> TransitionFactory<R> for TypeChars
where
    R: RefEditorMirror + RefFocus + RefLifecycle,
{
    type Reason = Rej;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Rej> {
        Self::preconditions_static(state).map(|_| {
            let strat = "[a-z]{1,4}"
                .prop_map(|text: String| TypeChars { text })
                .boxed();
            (6_u32, strat)
        })
    }
}

impl<R> TransitionRef<R> for TypeChars
where
    R: RefEditorMirror + RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
{
    type Reason = Rej;
    fn preconditions(&self, state: &R) -> Validated<(), Rej> {
        Self::preconditions_static(state)
    }
    fn apply_to_ref(&self, state: &mut R) {
        state.type_chars(&self.text);
        if state.enable_loro() {
            commit_active_editor_if_changed(state);
        }
    }
}

impl<R, S> TransitionImpl<R, S> for TypeChars
where
    R: RefEditorMirror + RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
    S: ?Sized + SutEditorMirrorWrite,
{
    async fn apply_to_sut(&self, _: &R, sut: &mut S) {
        sut.apply_type_chars(&self.text).await;
    }
}

#[derive(Clone, Debug)]
pub struct SplitBlock {
    pub block_id: EntityUri,
    pub position: usize,
}

impl SplitBlock {
    fn preconditions_static<R: RefBlockTree + RefLifecycle>(
        state: &R,
        block_id: &EntityUri,
        position: usize,
    ) -> Validated<(), Rej> {
        let focus_roots = state.focus_root_ids(CapRegion::Main);
        let checks: Vec<Validated<(), Rej>> = vec![
            ok(state.app_started(), Rej::NotStarted),
            ok(state.is_properly_setup(), Rej::NotSetup),
            ok(state.is_text_block(block_id), Rej::NotText),
            ok(
                state
                    .block_content(block_id)
                    .is_some_and(|t| position <= t.len()),
                Rej::BadPosition,
            ),
            ok(!state.is_layout_block(block_id), Rej::NotInFocusTree),
            ok(
                state.is_descendant_of_any(block_id, &focus_roots),
                Rej::NotInFocusTree,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }
}

impl<R> TransitionFactory<R> for SplitBlock
where
    R: RefBlockTree + RefLifecycle,
{
    type Reason = Rej;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Rej> {
        let mut candidates: Vec<(EntityUri, usize)> = vec![];
        for id in state.main_editable_descendants() {
            if let Some(text) = state.block_content(&id) {
                let len = text.len();
                for position in 0..=len {
                    if SplitBlock::preconditions_static(state, &id, position).is_good() {
                        candidates.push((id.clone(), position));
                    }
                }
            }
        }
        if candidates.is_empty() {
            Validated::fail(Rej::NoCandidates)
        } else {
            let strat = prop::sample::select(candidates)
                .prop_map(|(block_id, position)| SplitBlock { block_id, position })
                .boxed();
            Validated::Good((20_u32, strat))
        }
    }
}

impl<R> TransitionRef<R> for SplitBlock
where
    R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefLifecycle,
{
    type Reason = Rej;
    fn preconditions(&self, state: &R) -> Validated<(), Rej> {
        Self::preconditions_static(state, &self.block_id, self.position)
    }
    fn apply_to_ref(&self, state: &mut R) {
        state.push_undo_snapshot();
        let new_id = state.split_block(&self.block_id, self.position);
        state.set_focus(CapRegion::Main, new_id, CapCursor::default());
    }
}

impl<R, S> TransitionImpl<R, S> for SplitBlock
where
    R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefLifecycle,
    S: ?Sized + SutBlockTreeWrite + SutQuiesce,
{
    async fn apply_to_sut(&self, _: &R, sut: &mut S) {
        sut.apply_split_block(&self.block_id, self.position).await;
    }
}

// ─────────────────────────────────────────────────────────────────
// Transition enum + dispatch
// ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum PureTransition {
    TypeChars(TypeChars),
    SplitBlock(SplitBlock),
}

impl PureTransition {
    fn variant_name(&self) -> &'static str {
        match self {
            PureTransition::TypeChars(_) => "TypeChars",
            PureTransition::SplitBlock(_) => "SplitBlock",
        }
    }
}

fn aggregate(state: &EditorPureRef) -> BoxedStrategy<PureTransition> {
    use proptest::strategy::Union;
    let mut arms: Vec<(u32, BoxedStrategy<PureTransition>)> = vec![];
    if let Validated::Good((w, s)) =
        <TypeChars as TransitionFactory<EditorPureRef>>::weighted_generator(state)
    {
        arms.push((w, s.prop_map(PureTransition::TypeChars).boxed()));
    }
    if let Validated::Good((w, s)) =
        <SplitBlock as TransitionFactory<EditorPureRef>>::weighted_generator(state)
    {
        arms.push((w, s.prop_map(PureTransition::SplitBlock).boxed()));
    }
    assert!(!arms.is_empty(), "no transitions applicable in state");
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

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        match transition {
            PureTransition::TypeChars(t) => {
                <TypeChars as TransitionRef<EditorPureRef>>::preconditions(t, state).is_good()
            }
            PureTransition::SplitBlock(t) => {
                <SplitBlock as TransitionRef<EditorPureRef>>::preconditions(t, state).is_good()
            }
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        let name = transition.variant_name();
        match transition {
            PureTransition::TypeChars(t) => {
                <TypeChars as TransitionRef<EditorPureRef>>::apply_to_ref(t, &mut state)
            }
            PureTransition::SplitBlock(t) => {
                <SplitBlock as TransitionRef<EditorPureRef>>::apply_to_ref(t, &mut state)
            }
        };
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
        EditorPureSut::new(ref_state)
    }

    fn apply(
        mut state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        // pollster::block_on or a tiny executor — pollster avoids tokio dep.
        // Use a hand-rolled noop-waker poll loop since all our async fns
        // are synchronous in body.
        match transition {
            PureTransition::TypeChars(t) => {
                let fut = <TypeChars as TransitionImpl<EditorPureRef, EditorPureSut>>::apply_to_sut(
                    &t, ref_state, &mut state,
                );
                noop_block_on(fut);
            }
            PureTransition::SplitBlock(t) => {
                let fut =
                    <SplitBlock as TransitionImpl<EditorPureRef, EditorPureSut>>::apply_to_sut(
                        &t, ref_state, &mut state,
                    );
                noop_block_on(fut);
            }
        }
        state
    }

    fn check_invariants(
        sut: &Self::SystemUnderTest,
        _: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        let inner = sut.inner.borrow();
        // inv-tree-cursor-within-text-len
        if let (Some(text), Some(cursor)) = (
            <EditorPureRef as RefEditorMirror>::active_editor_text(&inner),
            <EditorPureRef as RefEditorMirror>::active_editor_cursor(&inner),
        ) {
            assert!(
                cursor <= text.len(),
                "[inv-tree-cursor-within-text-len] cursor {} > text len {}",
                cursor,
                text.len()
            );
        }
        // inv-tree-structural-integrity
        let known: BTreeSet<&EntityUri> = inner.blocks.keys().collect();
        for (id, b) in &inner.blocks {
            if let Some(parent) = &b.parent {
                assert!(
                    known.contains(parent),
                    "[inv-tree-structural-integrity] block {} parent {} does not exist",
                    id,
                    parent
                );
            } else {
                assert_eq!(
                    id, &inner.root_id,
                    "[inv-tree-structural-integrity] non-root block has no parent: {}",
                    id
                );
            }
        }
    }
}

// Minimal future-polling helper. All our `apply_to_sut` bodies are
// synchronous under the hood (no real awaits), so a noop waker + poll
// suffices. Avoids pulling in `tokio` or `futures-executor`.
fn noop_block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::pin::Pin;
    use std::task::Context;
    use std::task::Poll;
    use std::task::RawWaker;
    use std::task::RawWakerVTable;
    use std::task::Waker;

    fn noop_raw_waker() -> RawWaker {
        fn no_op(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            noop_raw_waker()
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
        RawWaker::new(std::ptr::null(), &VTABLE)
    }

    let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
    let mut ctx = Context::from_waker(&waker);
    // SAFETY: `fut` is owned + never moved after pinning here.
    let mut pinned = unsafe { Pin::new_unchecked(&mut fut) };
    match pinned.as_mut().poll(&mut ctx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("noop_block_on: future yielded Pending; pure slice should not"),
    }
}

// ─────────────────────────────────────────────────────────────────
// The test itself + a wall-clock micro-benchmark for H4
// ─────────────────────────────────────────────────────────────────

proptest_state_machine::prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 256,
        max_shrink_iters: 200,
        failure_persistence: None,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn editor_pure_state_machine(sequential 1..30 => EditorPureSut);
}

/// Wall-clock micro-benchmark for H4 keystone. Prints to stdout.
/// Run with `cargo nextest run -p holon-pbt-core --test editor_pure_h4_spike h4
/// --nocapture` or `cargo test -p holon-pbt-core --test editor_pure_h4_spike h4
/// -- --nocapture`.
#[test]
fn h4_microbenchmark() {
    let cases = 256_u32;
    let steps_per_case = 30_usize;

    let mut total_transitions = 0_usize;
    let mut rejected = 0_usize;
    let start = Instant::now();

    let mut runner = proptest::test_runner::TestRunner::default();

    for _ in 0..cases {
        let mut state = match EditorPureMachine::init_state().new_tree(&mut runner) {
            Ok(tree) => tree.current(),
            Err(_) => continue,
        };
        let mut sut = EditorPureSut::new(&state);
        for _ in 0..steps_per_case {
            let strategy = EditorPureMachine::transitions(&state);
            let transition = match strategy.new_tree(&mut runner) {
                Ok(tree) => tree.current(),
                Err(_) => break,
            };
            if !EditorPureMachine::preconditions(&state, &transition) {
                rejected += 1;
                continue;
            }
            // Apply to ref
            state = EditorPureMachine::apply(state, &transition);
            // Apply to SUT (mirrors what StateMachineTest does)
            sut = <EditorPureSut as StateMachineTest>::apply(sut, &state, transition);
            <EditorPureSut as StateMachineTest>::check_invariants(&sut, &state);
            total_transitions += 1;
        }
    }

    let elapsed = start.elapsed();
    let per_case_us = elapsed.as_micros() as f64 / cases as f64;
    let per_transition_us = if total_transitions > 0 {
        elapsed.as_micros() as f64 / total_transitions as f64
    } else {
        0.0
    };
    println!("===== H4 microbenchmark =====");
    println!("Cases:                {}", cases);
    println!("Steps/case (target):  {}", steps_per_case);
    println!("Transitions applied:  {}", total_transitions);
    println!("Rejected:             {}", rejected);
    println!("Total wall:           {:?}", elapsed);
    println!("Per case:             {:.1} µs", per_case_us);
    println!("Per transition:       {:.1} µs", per_transition_us);
    println!("===== Wide PBT baseline (recorded P1.0a) =====");
    println!("Per case:             ~4-6 seconds");
    println!("===== Ratio =====");
    let baseline_per_case_us = 5_000_000.0;
    println!(
        "Estimated speedup:    {:.0}x",
        baseline_per_case_us / per_case_us.max(1.0)
    );
    println!("============================");
}
