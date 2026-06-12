//! Transition: drag the focused block onto a target, making it a child of the target.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1248-1319` (generator),
//! `state_machine.rs:3374-3425` (precondition),
//! `state_machine.rs:2679-2685` (ref-state apply),
//! `sut.rs:3569-3598` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use crate::pbt::state_machine::DRAG_DROP_ENABLED;
use holon_api::{ContentType, EntityUri};

/// Drag the currently-focused block onto a target block, re-parenting the source
/// as a child of the target at the beginning (after=None).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DragDropBlock {
    pub source: EntityUri,
    pub target: EntityUri,
}

impl TransitionFactory<ReferenceState> for DragDropBlock {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutBlockInteract,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let drag_source: Option<EntityUri> = state.focused_main_editable();
        if drag_source.is_none() {
            return check(false, Reason::NoFocusInMain).map(|_| unreachable!());
        }

        let source = drag_source.unwrap();
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let candidates: Vec<EntityUri> = state
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                b.content_type == ContentType::Text
                    && !b.is_page()
                    && b.id != source
                    && !state.domain.layout_blocks.contains(&b.id)
                    && state.is_descendant_of_any(&b.id, &focus_roots)
            })
            .map(|b| b.id.clone())
            .filter(|target| {
                DragDropBlock {
                    source: source.clone(),
                    target: target.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();

        check(!candidates.is_empty(), Reason::SourceNotRendered).map(|_| {
            let source_clone = source.clone();
            let strat = proptest::sample::select(candidates)
                .prop_map(move |target| DragDropBlock {
                    source: source_clone.clone(),
                    target,
                })
                .boxed();
            (1, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for DragDropBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            check(DRAG_DROP_ENABLED, Reason::DragDropDisabled),
            // Drag needs the source to be a rendered `draggable(...)` in the
            // main panel — the SUT's `drop_entity` grabs that node. Two
            // independent things must hold, both verified here:
            //
            // 1. The active item template must *render* a draggable for the
            //    source's row. Render it through the shadow interpreter and walk
            //    the tree (the default `render_entity()` → block profile does;
            //    a custom render like `row(text(...))` does not).
            check(
                state.block_renders_draggable(&self.source),
                Reason::SourceNotRendered,
            ),
            // 2. The source must actually be in the active layout's query
            //    rendered set — evaluated faithfully via `TestQuery::evaluate`
            //    (the default `focus_root` query, or the recovered `QuerySource`
            //    of a user `index.org`). A `from children` layout surfaces only
            //    the layout block's direct children; an all-blocks layout
            //    surfaces everything. Combined with the draggable-template check
            //    above, this dissolves the old "block in tree but not rendered"
            //    divergence without a blanket custom-layout exclusion.
            check(
                state.main_rendered_block_ids().contains(&self.source),
                Reason::SourceNotRendered,
            ),
        ];

        let focused_in_main = state.focused_entity(holon_api::Region::Main);
        checks.push(check(
            focused_in_main == Some(&self.source),
            Reason::FocusedIsNotSelf,
        ));
        checks.push(check(self.source != self.target, Reason::NoOpParentMove));

        checks.push(check(
            state
                .domain
                .block_state
                .blocks
                .get(&self.source)
                .is_some_and(|b| b.content_type == ContentType::Text),
            Reason::FocusedNotText,
        ));
        checks.push(check(
            state
                .domain
                .block_state
                .blocks
                .get(&self.target)
                .is_some_and(|b| b.content_type == ContentType::Text),
            Reason::FocusedNotText,
        ));
        checks.push(check(
            !state.domain.layout_blocks.contains(&self.source),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            !state.domain.layout_blocks.contains(&self.target),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            state.is_descendant_of_any(&self.source, &focus_roots),
            Reason::FocusedNotDescendantOfFocusRoot,
        ));
        checks.push(check(
            state.is_descendant_of_any(&self.target, &focus_roots),
            Reason::FocusedNotDescendantOfFocusRoot,
        ));

        // No-op: target is already source's parent.
        checks.push(check(
            state
                .domain
                .block_state
                .blocks
                .get(&self.source)
                .is_some_and(|b| b.parent_id != self.target),
            Reason::NoOpParentMove,
        ));

        // Cycle: target is a descendant of source.
        let mut current = self.target.clone();
        let mut is_cycle = false;
        for _ in 0..50 {
            let Some(b) = state.domain.block_state.blocks.get(&current) else {
                break;
            };
            if b.parent_id == self.source {
                is_cycle = true;
                break;
            }
            if b.parent_id.is_no_parent() || b.parent_id.is_sentinel() {
                break;
            }
            current = b.parent_id.clone();
        }
        checks.push(check(!is_cycle, Reason::CyclicParentMove));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        // Production's drop_zone dispatches `move_block(id=source,
        // parent_id=target, after_block_id=None)` which inserts at
        // the beginning of the target's children.
        state.move_block(&self.source, self.target.clone(), None);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutBlockInteract> TransitionImpl<ReferenceState, S> for DragDropBlock {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.drag_drop_block(&self.source, &self.target).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for DragDropBlock {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.mcp.active_watches.len(),
            state.domain.block_state.blocks.len(),
            state.files.documents.len(),
        );
        sql.tolerance += 5; // extra margin for ordering operations
        sql
    }
}
