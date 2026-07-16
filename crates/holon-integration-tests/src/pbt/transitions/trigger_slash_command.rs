//! Transition: trigger slash command (delete) on the currently-focused block.
//!
//! @pbt rung input-pipeline
//!   `apply_trigger_slash_command_to_sut`: click + type '/' + command chars +
//!   Enter, every step a real UserDriver gesture.
//! @pbt covers slash-command — slash-menu keystroke sequence -> command
//! dispatch
//!
//! Mirrors the legacy logic split across `state_machine.rs:1057-1081`
//! (generator), `state_machine.rs:3263-3277` (precondition),
//! `state_machine.rs:2535-2545` (ref-state apply),
//! `sut.rs:3250-3362` (SUT apply), and
//! `transition_budgets.rs:284-286` (expected SQL).

use std::time::Duration;

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::capabilities::SutDriver;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

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
pub async fn apply_trigger_slash_command_to_sut<S: SutLayout + SutDriver>(sut: &S, id: &EntityUri) {
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
    let delete_id: EntityUri = EntityUri::block("delete");
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

impl<
    R: RefLifecycle + RefBlockTree + RefLayout + RefFocusRoots + RefLayoutInteract + RefLayoutMutate,
> TransitionFactory<R> for TriggerSlashCommand
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Loro)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates: Vec<EntityUri> = state
            .region_focused_entity(CapRegion::Main)
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

impl<
    R: RefLifecycle + RefBlockTree + RefLayout + RefFocusRoots + RefLayoutInteract + RefLayoutMutate,
> TransitionRef<R> for TriggerSlashCommand
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(CapRegion::Main);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            // Block-interaction transitions need the block to render as an
            // interactive widget (ops/draggable) reactively over the navigated
            // focus. Only the default layout does; custom `index.org` query
            // layouts don't (see RefLifecycle::renders_block_interactively).
            check(
                state.renders_block_interactively(&self.block_id),
                Reason::BlocksNotInteractiveUnderLayout,
            ),
            // Slash-command input is character typing through the editor's
            // `on_text_changed` pipeline, which is Loro/MutableText-backed.
            // SqlOnly has no MutableText, so gate this out exactly like the
            // other atomic-editor transitions (TypeChars, DeleteBackward, …).
            check(state.enable_loro(), Reason::LoroRequiredForAtomicEditor),
        ];

        checks.push(check(
            state.block_content(&self.block_id).is_some(),
            Reason::FocusedBlockMissing,
        ));
        checks.push(check(
            state.is_text_block(&self.block_id),
            Reason::FocusedNotText,
        ));
        checks.push(check(
            !state.is_layout_block(&self.block_id),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            !self.block_id.as_str().contains("default-"),
            Reason::BlockIsDefaultLayout,
        ));
        checks.push(check(
            state.all_block_ids().len() > 2,
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

    fn apply_to_ref(&self, state: &mut R) {
        // The whole slash-delete reference effect (undo snapshot + delete via the
        // shared mutation machinery + focus clear) lives in
        // `RefLayoutMutate::apply_slash_delete`.
        state.apply_slash_delete(&self.block_id);
    }
}

crate::cap_transition! {
    TriggerSlashCommand: SutBlockInteract,
    where R: [
        RefLifecycle + RefBlockTree + RefLayout + RefFocusRoots + RefLayoutInteract + RefLayoutMutate
    ],
    |me, _state, sut| {
        sut.trigger_slash_command(&me.block_id).await;
    }
    sql_budget: |_me, state| {
        expected_sql_for_kind(
            MutationKind::Delete,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        )
    }
}
