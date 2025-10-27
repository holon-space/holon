//! Transition: toggle the task state of a block via the StateToggle widget path.
//!
//! Mirrors the legacy logic split across `state_machine.rs:941-1054` (generator),
//! `state_machine.rs:3236-3262` (precondition),
//! `state_machine.rs:2519-2533` (ref-state apply),
//! `sut.rs:2176-2359` (SUT apply), and
//! `transition_budgets.rs:279-281` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use holon_api::EntityUri;
use holon_orgmode::OrgBlockExt;

/// Toggle the task state of a block via the StateToggle widget.
#[derive(Clone, Debug)]
pub struct ToggleState {
    pub block_id: EntityUri,
    pub new_state: String,
}

impl E2ETransitionFactory for ToggleState {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started || state.current_focus(holon_api::Region::Main).is_none() {
            return None;
        }

        let owned_render_expr = state
            .main_panel_render_expr()
            .or_else(|| state.root_render_expr())
            .cloned()
            .unwrap_or_else(super::super::reference_state::default_root_render_expr);

        let main_focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let text_block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .values()
            .filter(|b| {
                b.content_type == holon_api::ContentType::Text
                    && !b.is_page()
                    && !state.layout_blocks.contains(&b.id)
            })
            .map(|b| b.id.clone())
            .collect();
        let visible_text_block_ids: Vec<EntityUri> = text_block_ids
            .iter()
            .filter(|id| main_focus_roots.contains(*id))
            .cloned()
            .collect();

        let rows: Vec<holon_api::widget_spec::DataRow> = visible_text_block_ids
            .iter()
            .filter_map(|id| state.block_state.blocks.get(id))
            .map(super::super::reference_state::block_to_data_row)
            .collect();
        let arc_rows: Vec<std::sync::Arc<_>> = rows.into_iter().map(std::sync::Arc::new).collect();
        let vm = holon_frontend::interpret_pure(&owned_render_expr, &arc_rows, state);
        let toggle_block_ids: Vec<EntityUri> = vm
            .snapshot()
            .state_toggle_block_ids()
            .into_iter()
            .filter_map(|id| holon_api::EntityUri::parse(&id).ok())
            .collect();

        const RENDERED_DEFAULT_STATES: [&str; 4] = ["", "TODO", "DOING", "DONE"];
        let candidate_states: Vec<String> = match &state.keyword_set {
            Some(ks) => {
                let allowed: std::collections::HashSet<String> =
                    ks.all_keywords().into_iter().collect();
                RENDERED_DEFAULT_STATES
                    .iter()
                    .filter(|s| s.is_empty() || allowed.contains(**s))
                    .map(|s| s.to_string())
                    .collect()
            }
            None => RENDERED_DEFAULT_STATES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        let pairs: Vec<(EntityUri, String)> = toggle_block_ids
            .iter()
            .flat_map(|id| {
                let current_state = state
                    .block_state
                    .blocks
                    .get(id)
                    .and_then(|b| b.task_state())
                    .map(|ts| ts.keyword.to_string())
                    .unwrap_or_default();
                let bid = id.clone();
                candidate_states
                    .iter()
                    .filter(move |&s| s != &current_state)
                    .cloned()
                    .map(move |s| (bid.clone(), s))
            })
            .collect();

        if pairs.is_empty() {
            return None;
        }

        let strat = prop::sample::select(pairs)
            .prop_map(|(block_id, new_state)| ToggleState {
                block_id,
                new_state,
            })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for ToggleState {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        state.app_started
            && state.current_focus(holon_api::Region::Main).is_some()
            // `region_predictable(Main)` rejects both custom/unparseable
            // root layouts and layout-mutated panel render sources —
            // either case produces rows with no entity binding, so
            // `wait_for_entity_in_resolved_view_model` would time out.
            && state.region_predictable(holon_api::Region::Main)
            && focus_roots.contains(&self.block_id)
            // Layout headlines (in `layout_blocks.headline_ids`) define
            // their own render expression via a child render source.
            // Production renders the headline through that custom
            // layout, which can omit `state_toggle` entirely. The
            // headline never appears as a state_toggle entity in the
            // resolved ViewModel, so ToggleState would time out.
            // EditViaViewModel/Indent/MoveUp etc. already exclude
            // layout blocks for the same reason.
            && !state.layout_blocks.contains(&self.block_id)
            // A custom entity profile for `block` can replace the
            // default render with anything (e.g. just an
            // `editable_text`) — losing the state_toggle widget.
            // The reference state doesn't introspect the active
            // variant's widget set, so conservatively skip
            // ToggleState whenever a custom block profile is loaded.
            && !state.has_blocks_profile()
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        state.apply_mutation(&crate::pbt::types::MutationEvent {
            source: crate::pbt::types::MutationSource::UI,
            mutation: crate::pbt::types::Mutation::Update {
                entity: "block".to_string(),
                id: self.block_id.clone(),
                fields: [(
                    "task_state".to_string(),
                    holon_api::Value::String(self.new_state.clone()),
                )]
                .into(),
            },
        });
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_toggle_state(&self.block_id, &self.new_state)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        expected_sql_for_kind(MutationKind::Update, watches, blocks, docs)
    }
}
