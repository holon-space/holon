//! Transition: agent edits a dense projection over the MCP data tools.
//!
//! @pbt rung mcp-data
//!   `SutDenseTools`: `dense_query` the children of a page, apply one edit to
//!   the dense text (insert a headline with tags and a property drawer first,
//!   between two rows, as a row's first child or last / move
//!   the first row to the end / retitle the first row, replace or keep its
//!   body and set its keyword, all in one patch), and
//!   `dense_patch` it back — the canonical agent round trip through the REAL
//!   MCP tool → op-execution path (the layer the pure planner PBT
//!   `frontends/mcp/tests/dense_patch_pbt.rs` cannot see).
//! @pbt covers dense-roundtrip — dense_query → edit → dense_patch round trips
//! the store must agree on (create-with-position carrying tags and drawer
//! properties AND positional move)
//!
//! Cap-gated (`SutDenseTools`, the explicit MCP-data-tool capability): the
//! compositions that serve MCP insert the cap — `LiveMcpE2E` over the live
//! app, and every headless frontend composition over an embedded MCP server on
//! its own engine (every frontend embeds one); others deselect the transition
//! via cap-set narrowing. Weight-family name `Dense*` for `HOLON_PBT_WEIGHTS`
//! focus runs (`'DenseProjectionEdit:100'`).
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
//! Oracle: the model applies the same edit (create =
//! `create_block_under_with_attributes` with a synthetic `create-N` id the
//! harness reconcile pairs with the SUT-minted uuid, then `move_block` to the
//! drawn place; move = `move_block(first, parent, after=last)`). The MCP
//! writes as an agent, whose ops prod does not journal, so every arm carries
//! its write across the undo history. The existing block-set /
//! children-order invariants then assert the round trip; no bespoke oracle.

use std::collections::BTreeMap;

use holon_api::EntityUri;
use holon_api::Tags;
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

use crate::pbt::dense_text::NewRowPlace;
use crate::pbt::dense_text::dense_properties;
use crate::pbt::dense_text::dense_tags;
use crate::pbt::dense_text::new_row_place;
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
        DenseEditKind::CreateChild {
            place: NewRowPlace::Before(0),
            content: "appended".to_string(),
            tags: Tags::from_tag_iter(["ops".to_string()]),
            properties: BTreeMap::from([("owner".to_string(), "a1".to_string())]),
        },
        DenseEditKind::MoveFirstChildToEnd,
        DenseEditKind::EditFirstRow {
            title: "renamed".to_string(),
            body: Some(vec!["new body".to_string()]),
            state: Some("TODO".to_string()),
        },
    ]
);

/// One dense-text edit the agent applies between `dense_query` and
/// `dense_patch`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum DenseEditKind {
    /// Insert a new headline at `place` (no `{#alias}` token → CREATE with
    /// its parent and predecessor sibling, `None` for a first child).
    #[serde(alias = "AppendChild")]
    CreateChild {
        #[serde(default)]
        place: NewRowPlace,
        /// Title of the new headline. Short org-safe ASCII so the dense
        /// round trip is byte-faithful.
        content: String,
        /// The headline's tag group.
        tags: Tags,
        /// The headline's `:PROPERTIES:` drawer.
        properties: BTreeMap<String, String>,
    },
    /// Move the FIRST top-level row to the END (token kept → positional MOVE
    /// with `after` = the previous last row). Requires ≥ 2 children.
    MoveFirstChildToEnd,
    /// Edit the FIRST row in place in ONE patch: retitle it, replace its body
    /// lines (keep them when `None`) and set its task keyword.
    EditFirstRow {
        title: String,
        body: Option<Vec<String>>,
        state: Option<String>,
    },
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
            DenseEditKind::CreateChild { content, .. } => {
                check(!content.is_empty(), Reason::PreconditionFailed)
            }
            // A meaningful move needs ≥ 2 children (first != last).
            DenseEditKind::MoveFirstChildToEnd => check(
                state.sorted_children(&self.parent_id).len() >= 2,
                Reason::PreconditionFailed,
            ),
            DenseEditKind::EditFirstRow { title, .. } => {
                check(!title.is_empty(), Reason::PreconditionFailed)
            }
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
        // cap): the dense projection reads the Turso `block` matview. The MCP
        // server is not a wiring choice here — every frontend embeds one.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
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
            let edits = proptest::strategy::Union::new(
                parents
                    .iter()
                    .map(|parent| first_row_edit(parent.clone(), row_keywords(state, parent)))
                    .collect::<Vec<_>>(),
            )
            .boxed();
            let content = proptest::string::string_regex("[a-z]{1,8}").expect("valid regex");
            let append = (
                proptest::sample::select(parents),
                new_row_place(),
                content,
                dense_tags(),
                dense_properties(),
            )
                .prop_map(
                    |(parent_id, place, content, tags, properties)| DenseProjectionEdit {
                        parent_id,
                        edit: DenseEditKind::CreateChild {
                            place,
                            content,
                            tags,
                            properties,
                        },
                    },
                )
                .boxed();
            let strat = if movable.is_empty() {
                proptest::strategy::Union::new_weighted(vec![(1, append), (6, edits)]).boxed()
            } else {
                let mv = proptest::sample::select(movable)
                    .prop_map(|parent_id| DenseProjectionEdit {
                        parent_id,
                        edit: DenseEditKind::MoveFirstChildToEnd,
                    })
                    .boxed();
                proptest::strategy::Union::new_weighted(vec![(1, append), (1, mv), (6, edits)])
                    .boxed()
            };
            // Weighted so a default run likely draws the in-place row edit
            // before its first red ends the exploration.
            (90, strat)
        })
    }
}

/// The keywords a dense edit may set on `parent`'s rows: declared by the
/// rows' document. `?` has rules of its own.
fn row_keywords<R: RefBlockTreeMut>(state: &R, parent: &EntityUri) -> Vec<String> {
    state
        .block_task_vocabulary(parent)
        .all_keywords()
        .into_iter()
        .filter(|k| k != "?")
        .collect()
}

/// A retitle of `parent`'s first row with a new or kept multi-line body and a
/// new keyword; a title that starts with the keyword only when the row keeps
/// one, which the engine converges away unless the state is written first.
fn first_row_edit(parent: EntityUri, keywords: Vec<String>) -> BoxedStrategy<DenseProjectionEdit> {
    let state = if keywords.is_empty() {
        Just(None).boxed()
    } else {
        proptest::option::of(proptest::sample::select(keywords)).boxed()
    };
    let body = proptest::option::of(proptest::collection::vec(
        prop_oneof!["[a-z][a-z ]{0,8}[a-z]", "- [a-z]{1,4}"],
        1..3,
    ));
    (
        proptest::string::string_regex("[A-Z][a-z]{1,6}( [a-z]{1,5})?").expect("valid regex"),
        any::<bool>(),
        body,
        state,
    )
        .prop_map(move |(title, keyword_headed, body, state)| {
            let title = match &state {
                Some(keyword) if keyword_headed => format!("{keyword} {title}"),
                _ => title,
            };
            DenseProjectionEdit {
                parent_id: parent.clone(),
                edit: DenseEditKind::EditFirstRow { title, body, state },
            }
        })
        .boxed()
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
            DenseEditKind::CreateChild {
                place,
                content,
                tags,
                properties,
            } => {
                let rows = state.sorted_children(&self.parent_id);
                state.create_block_under_with_attributes(
                    &self.parent_id,
                    content,
                    tags,
                    properties,
                );
                let created = state
                    .sorted_children(&self.parent_id)
                    .last()
                    .expect("the create appended a child")
                    .clone();
                let (parent, after) = match *place {
                    NewRowPlace::Last => return,
                    NewRowPlace::Before(n) => {
                        let at = n % rows.len();
                        (self.parent_id.clone(), at.checked_sub(1).map(|i| &rows[i]))
                    }
                    NewRowPlace::FirstChildOf(n) => (rows[n % rows.len()].clone(), None),
                };
                state.move_block(&created, parent, after);
                state.carry_block_placement_across_history(&created);
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
                state.move_block(&first, self.parent_id.clone(), Some(&last));
                state.carry_block_placement_across_history(&first);
            }
            DenseEditKind::EditFirstRow {
                title,
                body,
                state: keyword,
            } => {
                let first = state
                    .sorted_children(&self.parent_id)
                    .first()
                    .expect("a dense edit parent has a child")
                    .clone();
                let stored = state.block_content(&first).unwrap_or_default().to_string();
                let body = body
                    .as_ref()
                    .map(|lines| lines.join("\n"))
                    .or_else(|| stored.split_once('\n').map(|(_, b)| b.to_string()));
                let content = match body {
                    Some(body) => format!("{title}\n{body}"),
                    None => title.clone(),
                };
                state.set_block_task_state(&first, keyword.as_deref());
                state.set_block_content(&first, &content);
                state.carry_block_text_across_history(&first);
                // An idle editor open on the row re-seeds from the patched block.
                state.refresh_clean_editor_surface(&first);
            }
        }
    }
}

crate::cap_transition! {
    DenseProjectionEdit: holon_pbt_core::capabilities::SutDenseTools,
    where R: [ RefLifecycle + RefBlockTree + RefBlockTreeMut + RefLayoutMutate ],
    |me, _state, sut| {
        match &me.edit {
            DenseEditKind::CreateChild {
                place,
                content,
                tags,
                properties,
            } => {
                sut.dense_create_child(&me.parent_id, *place, content, tags, properties)
                    .await;
            }
            DenseEditKind::MoveFirstChildToEnd => {
                sut.dense_move_first_child_to_end(&me.parent_id).await;
            }
            DenseEditKind::EditFirstRow { title, body, state } => {
                sut.dense_edit_first_row(
                    &me.parent_id,
                    title,
                    body.as_deref(),
                    state.as_deref(),
                )
                .await;
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
