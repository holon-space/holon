//! `inv-viewmodel-editable-text-triggers`.
//!
//! Relies on `props["trigger_count"]` in the `WidgetSnapshot` translation
//! for `EditableText`.
//!
//! ## Contract
//!
//! Every `editable_text` widget with non-empty `operations` must also
//! carry non-empty triggers. Operations are bound actions (e.g.
//! `set_field:content:`, `delete::id`); triggers are the input-trigger
//! patterns (`/` for slash menu, `[[` for wiki links, etc.) that fire
//! the bound actions in response to typing. An EditableText with
//! operations but no triggers is a render-DSL regression: the user can
//! type but no `on_text_changed` event ever activates the popup → the
//! bound delete / set_field ops are unreachable from real user input.
//!
//! This is the runtime guard for the assertion the deleted
//! `apply_edit_via_view_model` body used to inline at sut_handle.rs:806
//! ("EditableText for {block_id} has no triggers").
//!
//! ## Bound
//!
//! `S: SutRenderer`. No ref-side dependency — the contract is on the
//! rendered widget tree alone.

use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvViewmodelEditableTextTriggers;

impl InvViewmodelEditableTextTriggers {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-editable-text-triggers");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelEditableTextTriggers
where
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let root = sut.widget_tree_snapshot().await;
        let mut missing: Vec<String> = Vec::new();
        for node in root.walk() {
            if node.kind != "editable_text" {
                continue;
            }
            if node.operations.is_empty() {
                // Operations-less editor — typically a stub render or a
                // read-only context. Triggers are optional there.
                continue;
            }
            // `trigger_count` is encoded by `view_model_to_snapshot`
            // (sut_capabilities.rs) as the decimal string of
            // `vm.triggers.len()`. Absent → translator regression;
            // treat as 0 so the failure surfaces here.
            let trigger_count: usize = node
                .props
                .get("trigger_count")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if trigger_count == 0 {
                let id = node.entity_id.as_deref().unwrap_or("<no-entity-id>");
                missing.push(format!(
                    "  {id}: operations={:?} but trigger_count=0",
                    node.operations
                ));
            }
        }

        if missing.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-viewmodel-editable-text-triggers] {count} editable_text \
             widget(s) have bound operations but no input triggers — \
             user input can never reach the bound actions:\n{details}",
            count = missing.len(),
            details = missing
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        ))
    }
}
