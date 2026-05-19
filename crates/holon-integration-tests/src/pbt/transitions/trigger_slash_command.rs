//! Transition: trigger slash command (delete) on the currently-focused block.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1057-1081` (generator),
//! `state_machine.rs:3263-3277` (precondition),
//! `state_machine.rs:2535-2545` (ref-state apply),
//! `sut.rs:3250-3362` (SUT apply), and
//! `transition_budgets.rs:284-286` (expected SQL).

use holon_pbt_core::capabilities::{CapBlockId, SutDriver, SutLayout};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use std::time::Duration;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use holon_api::{ContentType, EntityUri};

// ── Capability-bound free function (Phase C, Option A — real user input) ──
//
// Replaces the previous body that built the EditorViewModel apparatus from
// SQL + interpret_pure and dispatched the resulting intent via apply_intent.
// The user-faithful path: click the target editor, type "/", type "delete"
// to filter the popup, then press Enter.
//
// The popup renders each item as a tracked widget (editor_view.rs:889) with
// `widget_type` = "popup_item" or "popup_item_selected" and entity_id =
// op.id. The selected-item assertion is the precondition for pressing
// Enter — if the filter didn't land on "delete" (a future op-name
// collision, a popup-filter regression), the test fails loudly with
// "delete not selected" instead of silently dispatching the wrong op.
//
// In headless mode (no geometry installed) `wait_for_widget_kind` returns
// `Ok(())` and the EditorViewModel's popup state advances internally via
// `on_text_changed`. Enter routes through the popup_handler.

/// SUT-side body of `TriggerSlashCommand`. Bound on `SutLayout + SutDriver`.
/// Clicks the target editor, opens the slash menu via "/", filters to
/// "delete" by typing the op name, asserts the popup_item_selected is
/// the delete op, then presses Enter.
pub async fn apply_trigger_slash_command_to_sut<S: SutLayout + SutDriver>(
    sut: &mut S,
    id: &CapBlockId,
) {
    sut.wait_for_widget_kind(
        id,
        &["editable_text", "rendered_text"],
        Duration::from_secs(2),
    )
    .await
    .unwrap_or_else(|e| panic!("[TriggerSlashCommand] target {id} not rendered: {e}"));
    sut.click_entity(id, "main")
        .await
        .unwrap_or_else(|e| panic!("[TriggerSlashCommand] click_entity failed for {id}: {e}"));
    sut.wait_for_engine_focus(id, Duration::from_secs(1))
        .await
        .unwrap_or_else(|e| panic!("[TriggerSlashCommand] focus did not propagate for {id}: {e}"));
    // Open the slash menu.
    sut.send_raw_keystroke("/", &[])
        .await
        .unwrap_or_else(|e| panic!("[TriggerSlashCommand] '/' keystroke failed: {e}"));
    // Type the op name to narrow the popup's filter. Each keystroke
    // re-fires `on_text_changed` with the cumulative input line, which
    // updates the popup state.
    for ch in "delete".chars() {
        sut.send_raw_keystroke(&ch.to_string(), &[])
            .await
            .unwrap_or_else(|e| {
                panic!("[TriggerSlashCommand] filter char {ch:?} keystroke failed: {e}")
            });
    }
    // Confirm Enter would actually fire `delete` — i.e. the filter
    // narrowed the popup to a state where the selected item is "delete".
    // Headless: no-op (popup doesn't reach BoundsRegistry).
    let delete_id: CapBlockId = "delete".to_string();
    sut.wait_for_widget_kind(&delete_id, &["popup_item_selected"], Duration::from_secs(2))
        .await
        .unwrap_or_else(|e| {
            panic!(
                "[TriggerSlashCommand] 'delete' popup item not selected after typing filter: {e}"
            )
        });
    sut.send_raw_keystroke("enter", &[])
        .await
        .unwrap_or_else(|e| panic!("[TriggerSlashCommand] Enter keystroke failed: {e}"));
}

/// Trigger the "/" slash-command menu on the focused block and select "delete".
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TriggerSlashCommand {
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for TriggerSlashCommand {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let focused_in_main = state.focused_entity(holon_api::Region::Main).cloned();
        let candidates: Vec<EntityUri> = focused_in_main
            .into_iter()
            .filter(|uri| {
                TriggerSlashCommand {
                    block_id: uri.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();

        check(!candidates.is_empty(), Reason::InsufficientBlocksForDelete).map(|_| {
            let strat = proptest::sample::select(candidates)
                .prop_map(|block_id| TriggerSlashCommand { block_id })
                .boxed();
            (1, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for TriggerSlashCommand {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
        ];

        checks.push(check(
            state.block_state.blocks.contains_key(&self.block_id),
            Reason::FocusedBlockMissing,
        ));
        checks.push(check(
            state
                .block_state
                .blocks
                .get(&self.block_id)
                .is_some_and(|b| b.content_type == ContentType::Text),
            Reason::FocusedNotText,
        ));
        checks.push(check(
            !state.layout_blocks.contains(&self.block_id),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            !self.block_id.as_str().contains("default-"),
            Reason::BlockIsDefaultLayout,
        ));
        checks.push(check(
            state.block_state.blocks.len() > 2,
            Reason::InsufficientBlocksForDelete,
        ));
        checks.push(check(
            state.is_descendant_of_any(&self.block_id, &focus_roots),
            Reason::FocusedNotDescendantOfFocusRoot,
        ));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        use crate::pbt::types::{Mutation, MutationEvent, MutationSource};
        state.push_undo_snapshot();
        state.apply_mutation(&MutationEvent {
            source: MutationSource::UI,
            mutation: Mutation::Delete {
                entity: "block".to_string(),
                id: self.block_id.clone(),
            },
        });
        state.clear_focus_if_deleted(&self.block_id);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_trigger_slash_command(&self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        expected_sql_for_kind(
            MutationKind::Delete,
            state.active_watches.len(),
            state.block_state.blocks.len(),
            state.documents.len(),
        )
    }
}
