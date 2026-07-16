//! `inv-viewmodel-state-toggle-correct`.
//!
//! @pbt oracle correspondence
//! @pbt covers state-toggle-divergence — rendered StateToggle
//!   current/label/field/ops disagree with the ref block's task_state
//! @pbt slips-if-removed a task block's toggle shows the wrong state or
//!   label, or lacks a task_state-affecting op; clicking the checkbox can't
//!   cycle state or displays a stale value
//!
//! Expressed against the frontend-agnostic `WidgetSnapshot` IR so it runs in
//! any slice whose SUT implements `SutRenderer`.
//!
//! Asserts, for every `state_toggle` widget in the snapshot whose
//! `entity_id` matches a block in the reference model:
//! - `props["field"]` == "task_state"
//! - `props["current"]` matches the reference block's task_state
//! - `props["label"]` == `state_display(current).0` (displayed label matches
//!   the state-display of the current value)
//! - For task blocks (those with a non-empty task_state): an operation whose
//!   `affected_fields` include `task_state` is bound (the typed
//!   `set_state`/`cycle_task_state` ops — NOT the generic `set_field`, whose
//!   affected-fields are empty)
//! - States list is non-empty for task blocks

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvViewmodelStateToggleCorrect;

impl InvViewmodelStateToggleCorrect {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-state-toggle-correct");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelStateToggleCorrect
where
    R: RefBlockTree + RefTaskState,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let root = sut.widget_tree_snapshot().await;
        for node in root.walk() {
            if node.kind != "state_toggle" {
                continue;
            }

            let field = node.props.get("field").map(String::as_str).unwrap_or("");
            if field != "task_state" {
                return InvariantResult::Fail(format!(
                    "[inv-viewmodel-state-toggle-correct] unexpected field '{field}' in \
                     StateToggle"
                ));
            }

            let Some(block_id) = node.entity_id.as_ref() else {
                return InvariantResult::Fail(
                    "[inv-viewmodel-state-toggle-correct] StateToggle has no entity id".into(),
                );
            };
            let cap_id: EntityUri = EntityUri::parse(block_id)
                .expect("StateToggle entity_id must be a valid EntityUri");

            // Skip widgets for blocks not in the ref model (transient
            // pre-merge peer rows, etc.).
            if ref_.block_content(&cap_id).is_none() {
                continue;
            }

            let expected_state = ref_.task_state_of(&cap_id).unwrap_or_default();
            let current = node.props.get("current").map(String::as_str).unwrap_or("");

            if current != expected_state {
                return InvariantResult::Fail(format!(
                    "[inv-viewmodel-state-toggle-correct] StateToggle current '{current}' != \
                     reference '{expected_state}' for block {block_id}"
                ));
            }

            // Displayed label must match the state-display of the current
            // value: `let (expected_label, _) = state_display(current); label
            // == expected_label`. Applies to all ref blocks (task or not).
            // The label is carried in `props["label"]`, populated by
            // `view_model_to_snapshot` from `ViewKind::StateToggle.label`.
            let label = node.props.get("label").map(String::as_str).unwrap_or("");
            let (expected_label, _) = holon_api::render_eval::state_display(current);
            if label != expected_label {
                return InvariantResult::Fail(format!(
                    "[inv-viewmodel-state-toggle-correct] StateToggle label '{label}' != expected \
                     '{expected_label}' for block {block_id}"
                ));
            }

            // Task blocks (non-empty task_state) must have an operation that can
            // change the block's task_state, bound + non-empty states. The bound
            // ops are the typed `set_state`/`cycle_task_state` (whose
            // `affected_fields` is `task_state`), NOT the generic `set_field` (whose
            // `affected_fields` is empty — `field` is a runtime param). Match by the
            // affected-field, not a hardcoded op name, so this stays render-faithful.
            let is_task = !expected_state.is_empty();
            if is_task {
                let affects_task_state = node.operations.iter().any(|op| {
                    // Canonical op form: `<name>:<affected_fields_csv>:<params_csv>`.
                    op.split(':')
                        .nth(1)
                        .is_some_and(|fields| fields.split(',').any(|f| f == "task_state"))
                });
                if !affects_task_state {
                    return InvariantResult::Fail(format!(
                        "[inv-viewmodel-state-toggle-correct] No operation affecting 'task_state' \
                         on {block_id} (ops: {:?})",
                        node.operations
                    ));
                }
                let states = node.props.get("states").map(String::as_str).unwrap_or("");
                if states.is_empty() {
                    return InvariantResult::Fail(format!(
                        "[inv-viewmodel-state-toggle-correct] StateToggle for {block_id} has \
                         empty states"
                    ));
                }
            }
        }
        InvariantResult::Ok
    }
}
