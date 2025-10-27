//! Transition: trigger the [[ doc-link popup and validate the InsertText pipeline.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1083-1100` (generator),
//! `state_machine.rs:3278-3295` (precondition),
//! `state_machine.rs:2547-2550` (ref-state apply — read-only),
//! `sut.rs:3364-3522` (SUT apply), and
//! `transition_budgets.rs:288-294` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

use holon_api::{ContentType, EntityUri};

/// Trigger the `[[` doc-link popup on a text block and validate the full
/// `[[ trigger → EditorViewModel → PopupMenu → InsertText` pipeline.
/// Read-only transition: no reference-model state changes.
#[derive(Clone, Debug)]
pub struct TriggerDocLink {
    pub block_id: EntityUri,
    pub target_block_id: EntityUri,
}

impl E2ETransitionFactory for TriggerDocLink {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main).cloned();
        let editable_block_ids: Vec<EntityUri> =
            if state.is_properly_setup() && focused_in_main.is_some() {
                let focused = focused_in_main.as_ref().unwrap();
                let no_content_update: std::collections::HashSet<EntityUri> = state
                    .layout_blocks
                    .render_source_ids
                    .iter()
                    .chain(state.layout_blocks.query_source_ids.iter())
                    .chain(state.profile_block_ids.iter())
                    .cloned()
                    .collect();
                let valid = state
                    .block_state
                    .blocks
                    .get(focused)
                    .is_some_and(|b| b.content_type == ContentType::Text && !b.is_page())
                    && state.layout_blocks.is_focusable(focused)
                    && !no_content_update.contains(focused)
                    && state.is_descendant_of_any(focused, &focus_roots);
                if valid { vec![focused.clone()] } else { vec![] }
            } else {
                vec![]
            };
        if editable_block_ids.len() < 2 {
            // Widening vs. legacy: the legacy generator computed
            // `editable_block_ids` from `focused_in_main` (always at most one
            // element), so `len() < 2` was unconditionally true and the
            // variant was effectively unreachable. Falling back to "any two
            // text blocks in the focusable tree" makes generation actually
            // exercise the [[ doc-link pipeline. The precondition below
            // already requires `block_id` to be in the focus tree + text +
            // non-layout, so the fallback can't produce invalid pairs.
            let all_text: Vec<EntityUri> = state
                .block_state
                .blocks
                .values()
                .filter(|b| b.content_type == ContentType::Text && !b.is_page())
                .map(|b| b.id.clone())
                .collect();
            if all_text.len() < 2 {
                return None;
            }
            let ids = all_text.clone();
            let target_ids = all_text.clone();
            let strat = (
                proptest::sample::select(ids),
                proptest::sample::select(target_ids),
            )
                .prop_filter("block and target must differ", |(a, b)| a != b)
                .prop_map(|(block_id, target_block_id)| TriggerDocLink {
                    block_id,
                    target_block_id,
                })
                .boxed();
            return Some((1, strat));
        }
        let ids = editable_block_ids.clone();
        let target_ids = editable_block_ids.clone();
        let strat = (
            proptest::sample::select(ids),
            proptest::sample::select(target_ids),
        )
            .prop_filter("block and target must differ", |(a, b)| a != b)
            .prop_map(|(block_id, target_block_id)| TriggerDocLink {
                block_id,
                target_block_id,
            })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for TriggerDocLink {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        state.app_started
            && state.is_properly_setup()
            && state.block_state.blocks.contains_key(&self.block_id)
            && state.block_state.blocks.contains_key(&self.target_block_id)
            && self.block_id != self.target_block_id
            && state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| b.content_type == ContentType::Text)
            && !state.layout_blocks.contains(&self.block_id)
            && state.is_descendant_of_any(&self.block_id, &focus_roots)
    }

    fn apply_to_ref(&self, _state: &mut ReferenceState) {
        // Read-only: validates the [[ trigger → InsertText pipeline.
        // No state change in the reference model.
    }

    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_trigger_doc_link(&self.block_id, &self.target_block_id, state)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}
