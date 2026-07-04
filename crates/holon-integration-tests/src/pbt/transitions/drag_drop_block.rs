//! Transition: drag the focused block onto a target, making it a child of the
//! target.
//!
//! @pbt rung input-pipeline
//!   `drag_drop_block` drives a real pointer drag (geometry) through the
//!   production UserDriver.
//! @pbt covers drag-reorder — pointer drag -> reparent under target
//!
//! Mirrors the legacy logic split across `state_machine.rs:1248-1319`
//! (generator), `state_machine.rs:3374-3425` (precondition),
//! `state_machine.rs:2679-2685` (ref-state apply),
//! `sut.rs:3569-3598` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::state_machine::DRAG_DROP_ENABLED;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

/// Drag the currently-focused block onto a target block, re-parenting the
/// source as a child of the target at the beginning (after=None).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DragDropBlock {
    pub source: EntityUri,
    pub target: EntityUri,
}

impl<
    R: RefLifecycle + RefBlockTree + RefBlockTreeMut + RefFocusRoots + RefLayout + RefLayoutInteract,
> TransitionFactory<R> for DragDropBlock
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let drag_source: Option<EntityUri> = state.focused_main_editable();
        if drag_source.is_none() {
            return check(false, Reason::NoFocusInMain).map(|_| unreachable!());
        }

        let source = drag_source.unwrap();
        let focus_roots = state.rendered_focus_root_ids(CapRegion::Main);
        let candidates: Vec<EntityUri> = state
            .all_block_ids()
            .into_iter()
            .filter(|id| {
                state.is_text_block(id)
                    && !state.is_page_block(id)
                    && *id != source
                    && !state.is_layout_block(id)
                    && state.is_descendant_of_any(id, &focus_roots)
            })
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

impl<
    R: RefLifecycle + RefBlockTree + RefBlockTreeMut + RefFocusRoots + RefLayout + RefLayoutInteract,
> TransitionRef<R> for DragDropBlock
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let focus_roots = state.rendered_focus_root_ids(CapRegion::Main);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            check(DRAG_DROP_ENABLED, Reason::DragDropDisabled),
            // Drag needs the source to be a rendered `draggable(...)` in the
            // main panel — the SUT's `drop_entity` grabs that node. Two
            // independent things must hold, both verified here:
            //
            // 1. The active item template must *render* a draggable for the source's row. Render
            //    it through the shadow interpreter and walk the tree (the default
            //    `render_entity()` → block profile does; a custom render like `row(text(...))`
            //    does not).
            check(
                state.block_renders_draggable(&self.source),
                Reason::SourceNotRendered,
            ),
            // 2. The source must actually be in the active layout's query rendered set — evaluated
            //    faithfully via `TestQuery::evaluate` (the default `focus_root` query, or the
            //    recovered `QuerySource` of a user `index.org`). A `from children` layout surfaces
            //    only the layout block's direct children; an all-blocks layout surfaces
            //    everything. Combined with the draggable-template check above, this dissolves the
            //    old "block in tree but not rendered" divergence without a blanket custom-layout
            //    exclusion.
            check(
                state.main_rendered_block_ids().contains(&self.source),
                Reason::SourceNotRendered,
            ),
        ];

        checks.push(check(
            state.region_focused_entity(CapRegion::Main).as_ref() == Some(&self.source),
            Reason::FocusedIsNotSelf,
        ));
        checks.push(check(self.source != self.target, Reason::NoOpParentMove));

        checks.push(check(
            state.is_text_block(&self.source),
            Reason::FocusedNotText,
        ));
        checks.push(check(
            state.is_text_block(&self.target),
            Reason::FocusedNotText,
        ));
        checks.push(check(
            !state.is_layout_block(&self.source),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            !state.is_layout_block(&self.target),
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

        // No-op: target is already source's parent. `parent_of` returns `None`
        // for a root/sentinel parent (never equal to `target`).
        checks.push(check(
            state.parent_of(&self.source).as_ref() != Some(&self.target),
            Reason::NoOpParentMove,
        ));

        // Cycle: target is a descendant of source. `parent_of` yields `None` at
        // the root/sentinel boundary, ending the walk.
        let mut current = self.target.clone();
        let mut is_cycle = false;
        for _ in 0..50 {
            let Some(parent) = state.parent_of(&current) else {
                break;
            };
            if parent == self.source {
                is_cycle = true;
                break;
            }
            current = parent;
        }
        checks.push(check(!is_cycle, Reason::CyclicParentMove));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.push_undo_snapshot();
        // Production's drop_zone dispatches `move_block(id=source,
        // parent_id=target, after_block_id=None)` which inserts at
        // the beginning of the target's children.
        state.move_block(&self.source, self.target.clone(), None);
    }
}

crate::cap_transition! {
    DragDropBlock: SutBlockInteract,
    where R: [
        RefLifecycle + RefBlockTree + RefBlockTreeMut + RefFocusRoots + RefLayout + RefLayoutInteract
    ],
    |me, _state, sut| {
        sut.drag_drop_block(&me.source, &me.target).await;
    }
    sql_budget: |_me, state| {
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        );
        sql.tolerance += 5; // extra margin for ordering operations
        sql
    }
}
