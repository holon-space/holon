//! Transition: agent edits a dense projection over the MCP data tools.
//!
//! @pbt rung mcp-data
//!   `SutDenseTools`: `dense_query` the children of a page, apply one edit to
//!   the dense text (append a headline / move the first row to the end), and
//!   `dense_patch` it back — the canonical agent round trip through the REAL
//!   MCP tool → op-execution path (the layer the pure planner PBT
//!   `frontends/mcp/tests/dense_patch_pbt.rs` cannot see).
//! @pbt covers dense-roundtrip — dense_query → edit → dense_patch round trips
//! the store must agree on (create-with-position AND positional move)
//!
//! Cap-gated (`SutDenseTools`, the explicit MCP-data-tool capability): only
//! compositions that serve MCP tools — today `LiveMcpE2E` — insert the cap, so
//! headless compositions deselect this transition via cap-set narrowing,
//! exactly like the Loro peer ops. Weight-family name `Dense*` for
//! `HOLON_PBT_WEIGHTS` focus runs (`'DenseProjectionEdit:100'`).
//!
//! Born from BugFunnel 2026-07-27 (+1 COV): the dense_patch tool → op seam had
//! zero end-to-end coverage and hid TWO defects in `move_block_after`
//! (`frontends/mcp/src/tools.rs`):
//! - `AppendChild`: a created row with ANY preceding sibling issues a separate
//!   `move_block` with NO `parent_id` — hard error (swallowed to a generic
//!   message) AND the already-committed create leaks as an orphan.
//! - `MoveFirstChildToEnd`: the anchor is sent as `position_after_block_id` but
//!   the op's param bridge reads `after_block_id` — the anchor is SILENTLY
//!   dropped, so the move "succeeds" while the row lands first-child instead of
//!   after the anchor.
//!
//! Oracle: the model applies the same edit (append = `create_block_under`
//! with a synthetic `create-N` id the harness reconcile pairs with the
//! SUT-minted uuid; move = `push_undo_snapshot` + `move_block(first, parent,
//! after=last)`). The existing block-set / children-order invariants then
//! assert the round trip; no bespoke oracle.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::CACHE_EVENT_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

holon_pbt_core::step_field_via_json!(
    DenseEditKind,
    vec![
        DenseEditKind::AppendChild {
            content: "appended".to_string(),
        },
        DenseEditKind::MoveFirstChildToEnd,
    ]
);

/// One dense-text edit the agent applies between `dense_query` and
/// `dense_patch`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum DenseEditKind {
    /// Append a new top-level headline (no `{#alias}` token → CREATE, with
    /// `after` = the last existing row when one exists).
    AppendChild {
        /// Title of the appended headline. Short org-safe ASCII so the dense
        /// round trip is byte-faithful.
        content: String,
    },
    /// Move the FIRST top-level row to the END (token kept → positional MOVE
    /// with `after` = the previous last row). Requires ≥ 2 children.
    MoveFirstChildToEnd,
}

/// Edit a dense projection of `parent`'s children through the dense_query →
/// edit → dense_patch MCP round trip.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I apply dense edit {edit} under block {parent_id}")]
pub struct DenseProjectionEdit {
    /// The projected parent — a page whose children the dense projection
    /// anchors under (`#+ID:` header = this id).
    pub parent_id: EntityUri,
    pub edit: DenseEditKind,
}

/// Candidate parents for a dense edit: a Main FOCUS-ROOT page with at least
/// one child, all children non-page COMPARED (non-seed) text rows.
///
/// Scoping to the focus roots (rather than every ref page with children) keeps
/// the candidate live-real: the oracle also models pages the live composition
/// never seeds (e.g. the wide seed's `forward-edge-page` — scaffold-classified
/// on the SUT side), whose live dense projection is EMPTY; the focused page is
/// by construction present and rendered in the SUT. It is also what an agent
/// actually edits. The child floor matters twice over — an empty projection
/// loses its page anchor (`SYNTHETIC_ROOT`), and a preceding sibling is
/// exactly what routes the patch through the create+position path this
/// transition exists to cover. All-non-page keeps the projection's row set
/// equal to the ref's child list (`build_projection` silently drops `is_page`
/// rows, which would skew the edit anchors).
fn dense_edit_parents<R: RefBlockTree>(state: &R) -> Vec<EntityUri> {
    use holon_pbt_core::capabilities::CapRegion;
    let non_seed = state.all_non_seed_block_ids();
    state
        .focus_root_ids(CapRegion::Main)
        .into_iter()
        .filter(|p| {
            if !state.is_page_block(p) || state.is_layout_block(p) {
                return false;
            }
            let children = state.sorted_children(p);
            !children.is_empty()
                && children.iter().all(|c| {
                    non_seed.contains(c)
                        && state.is_text_block(c)
                        && !state.is_page_block(c)
                        && !state.is_layout_block(c)
                        && !state.is_no_content_update(c)
                })
        })
        .collect()
}

impl DenseProjectionEdit {
    fn kind_preconditions<R: RefLifecycle + RefBlockTree>(
        &self,
        state: &R,
    ) -> Validated<(), Reason> {
        match &self.edit {
            DenseEditKind::AppendChild { content } => {
                check(!content.is_empty(), Reason::PreconditionFailed)
            }
            // A meaningful move needs ≥ 2 children (first != last).
            DenseEditKind::MoveFirstChildToEnd => check(
                state.sorted_children(&self.parent_id).len() >= 2,
                Reason::PreconditionFailed,
            ),
        }
    }
}

impl<R: RefLifecycle + RefBlockTree + RefBlockTreeMut + RefLayoutMutate> TransitionFactory<R>
    for DenseProjectionEdit
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Necessary, not sufficient (the decisive gate is the `SutDenseTools`
        // cap): the dense projection reads the Turso `block` matview, and the
        // wiring must declare a served MCP surface.
        ::holon_pbt_core::RequiredWiring::All(vec![
            ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso),
            ::holon_pbt_core::RequiredWiring::HasActor(::holon_pbt_core::Actor::MCPServer),
        ])
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let parents = dense_edit_parents(state);
        // Parents with ≥ 2 children can host the move arm too.
        let movable: Vec<EntityUri> = parents
            .iter()
            .filter(|p| state.sorted_children(p).len() >= 2)
            .cloned()
            .collect();
        let gate: Validated<Vec<()>, Reason> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!parents.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect();
        gate.map(|_| {
            let content = proptest::string::string_regex("[a-z]{1,8}").expect("valid regex");
            let append = (proptest::sample::select(parents), content)
                .prop_map(|(parent_id, content)| DenseProjectionEdit {
                    parent_id,
                    edit: DenseEditKind::AppendChild { content },
                })
                .boxed();
            let strat = if movable.is_empty() {
                append
            } else {
                let mv = proptest::sample::select(movable)
                    .prop_map(|parent_id| DenseProjectionEdit {
                        parent_id,
                        edit: DenseEditKind::MoveFirstChildToEnd,
                    })
                    .boxed();
                proptest::strategy::Union::new_weighted(vec![(2, append), (1, mv)]).boxed()
            };
            // Moderate: one agent-write family among many user-write
            // families; focus runs boost it via HOLON_PBT_WEIGHTS.
            (5, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefBlockTreeMut + RefLayoutMutate> TransitionRef<R>
    for DenseProjectionEdit
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                dense_edit_parents(state).contains(&self.parent_id),
                Reason::PreconditionFailed,
            ),
            self.kind_preconditions(state),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        match &self.edit {
            // Mint-when-absent: dense_patch mints the uuid server-side, so
            // the oracle allocates a synthetic `create-N` the harness
            // reconcile pairs with the SUT's minted id. This is a server-side
            // patch, NOT the creation-slot gesture — it stays ONE user-origin
            // create, which is why it keeps `create_block_under` while
            // `CreateBlockUnderFocus{id: None}` moved to
            // `birth_block_via_creation_slot`.
            DenseEditKind::AppendChild { content } => {
                state.create_block_under(&self.parent_id, content);
            }
            DenseEditKind::MoveFirstChildToEnd => {
                let children = state.sorted_children(&self.parent_id);
                let first = children
                    .first()
                    .expect("MoveFirstChildToEnd precondition guarantees >= 2 children")
                    .clone();
                let last = children
                    .last()
                    .expect("MoveFirstChildToEnd precondition guarantees >= 2 children")
                    .clone();
                state.push_undo_snapshot();
                state.move_block(&first, self.parent_id.clone(), Some(&last));
            }
        }
    }
}

crate::cap_transition! {
    DenseProjectionEdit: holon_pbt_core::capabilities::SutDenseTools,
    where R: [ RefLifecycle + RefBlockTree + RefBlockTreeMut + RefLayoutMutate ],
    |me, _state, sut| {
        match &me.edit {
            DenseEditKind::AppendChild { content } => {
                sut.dense_append_child(&me.parent_id, content).await;
            }
            DenseEditKind::MoveFirstChildToEnd => {
                sut.dense_move_first_child_to_end(&me.parent_id).await;
            }
        }
    }
    sql_budget: |_me, state| {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        // A create/move, plus the dense_query projection read + the
        // optimistic-concurrency re-read on top of it.
        let create = expected_sql_for_kind(MutationKind::Create, watches, blocks, docs);
        ExpectedSql {
            reads: create.reads + CACHE_EVENT_READS + 2,
            writes: create.writes,
            ddl: 0,
            tolerance: create.tolerance + REACTIVE_BASE,
        }
    }
}
