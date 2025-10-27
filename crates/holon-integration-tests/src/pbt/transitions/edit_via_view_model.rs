//! Transition: edit a block's content via the view model (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:915-927` (generator),
//! `state_machine.rs:3208-3220` (precondition),
//! `state_machine.rs:2502-2517` (ref-state apply),
//! `sut.rs:2059-2174` (SUT apply), and
//! `transition_budgets.rs:279-281` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use holon_api::{ContentType, EntityUri};

/// Edit a block's content via the ViewModel path.
#[derive(Clone, Debug)]
pub struct EditViaViewModel {
    pub block_id: EntityUri,
    pub new_content: String,
}

impl E2ETransitionFactory for EditViaViewModel {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        // `EditViaViewModel` bypasses the keyboard pipeline (queries the
        // DB directly, calls `on_blur` on a synthetically-rendered
        // editor). When the atomic editor is enabled, the proper
        // UI-driven primitives — `FocusEditableText`, `TypeChars`,
        // `DeleteBackward`, `Blur` — cover the same surface through
        // real keystrokes. Disable the bypass path in that mode so
        // the PBT can't accidentally rely on it. See
        // `frontends/tui/TODO.md` item A6.
        if ReferenceState::atomic_editor_enabled() {
            return None;
        }

        let no_content_update: std::collections::HashSet<EntityUri> = state
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(state.layout_blocks.query_source_ids.iter())
            .chain(state.profile_block_ids.iter())
            .cloned()
            .collect();

        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main).cloned();

        let editable_block_ids: Vec<EntityUri> =
            if state.is_properly_setup() && focused_in_main.is_some() {
                let focused = focused_in_main.as_ref().unwrap();
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

        if editable_block_ids.is_empty() {
            return None;
        }

        let strat = (prop::sample::select(editable_block_ids), "[a-z ]{3,20}")
            .prop_map(|(block_id, new_content)| EditViaViewModel {
                block_id,
                new_content,
            })
            .boxed();
        Some((5, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for EditViaViewModel {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        state.app_started
            && state.is_properly_setup()
            && state.block_state.blocks.contains_key(&self.block_id)
            && state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| b.content_type == ContentType::Text)
            && !state.layout_blocks.contains(&self.block_id)
            && state.is_descendant_of_any(&self.block_id, &focus_roots)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        state.apply_mutation(&crate::pbt::types::MutationEvent {
            source: crate::pbt::types::MutationSource::UI,
            mutation: crate::pbt::types::Mutation::Update {
                entity: "block".to_string(),
                id: self.block_id.clone(),
                fields: [(
                    "content".to_string(),
                    holon_api::Value::String(self.new_content.clone()),
                )]
                .into(),
            },
        });
        state.reset_cursor_if_focused(&self.block_id);
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_edit_via_view_model(&self.block_id, &self.new_content)
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
