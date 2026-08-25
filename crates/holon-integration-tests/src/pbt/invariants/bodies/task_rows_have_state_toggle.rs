//! `inv-viewmodel-task-rows-have-state-toggle`.
//!
//! @pbt oracle correspondence
//! @pbt covers task-row-degrades-to-text — a rendered collection row backed
//!   by a task block carries no `state_toggle` in its own row scope (the
//!   flat-live-query page_title misfire: bugfunnel
//!   2026-08-25-flat-query-task-rows-render-as-page-title-blobs)
//! @pbt slips-if-removed a flat live-query task list (the vault's Now list)
//!   renders every task as one bare text blob with no TODO chip, and
//!   `inv-viewmodel-state-toggle-correct` stays green because it only judges
//!   the toggles that DO exist
//!
//! The blind-side twin of `viewmodel_state_toggle_correct`: that invariant
//! verifies every rendered `state_toggle`; this one requires the toggle to be
//! rendered at all. For every `tree_item` row whose `entity_id` is a ref
//! block with a non-empty task_state, the row's OWN scope (its subtree,
//! stopping at nested `tree_item`s — a parent row must not borrow its
//! children's toggles) must contain a `state_toggle`.
//!
//! Exemptions — rows that legitimately render as a title/header without a
//! toggle, each derived ref-side so the check cannot be fooled by the very
//! `role: "page_title"` misfire it guards against:
//! - focus roots of Main and Sidebar (the zoomed-into block and pinned sidebar
//!   heads render as page-title headers by design),
//! - `Page` blocks (embedded-page headers render title-only),
//! - layout blocks (scaffolding never renders as a task row).

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// True iff `node`'s own row scope contains a `state_toggle`. The scope is
/// the subtree below `node` truncated at nested `tree_item`s: a nested row
/// is another block's row, and crediting its toggle to the parent would let
/// a degraded parent row pass on its child's chrome.
fn row_scope_has_state_toggle(node: &WidgetSnapshot) -> bool {
    fn scan(children: &[WidgetSnapshot]) -> bool {
        children.iter().any(|c| {
            if c.kind == "tree_item" {
                return false;
            }
            c.kind == "state_toggle" || scan(&c.children)
        })
    }
    scan(&node.children)
}

/// The ids of every `tree_item` row for which `is_checked_task(entity_id)`
/// holds but whose row scope renders NO `state_toggle`. Shared by the
/// composed catalog invariant and the dedicated Now-list rung
/// (`tests/frontend_suite/now_query_task_rows_render_structured.rs`), so
/// both judge the identical property.
pub fn task_tree_rows_missing_state_toggle(
    root: &WidgetSnapshot,
    is_checked_task: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut missing = Vec::new();
    for node in root.walk() {
        if node.kind != "tree_item" {
            continue;
        }
        let Some(id) = node.entity_id.as_deref() else {
            continue;
        };
        if is_checked_task(id) && !row_scope_has_state_toggle(node) {
            missing.push(id.to_string());
        }
    }
    missing
}

pub struct InvViewmodelTaskRowsHaveStateToggle;

impl InvViewmodelTaskRowsHaveStateToggle {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-task-rows-have-state-toggle");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelTaskRowsHaveStateToggle
where
    R: RefBlockTree + RefTaskState,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let root = sut.widget_tree_snapshot().await;
        let exempt: BTreeSet<EntityUri> = ref_
            .focus_root_ids(CapRegion::Main)
            .into_iter()
            .chain(ref_.focus_root_ids(CapRegion::Sidebar))
            .collect();
        let is_checked_task = |id: &str| {
            let Ok(uri) = EntityUri::parse(id) else {
                return false;
            };
            // Only ref-known blocks are judged (phantom ids are
            // `inv-viewmodel-entity-ids-subset-of-data`'s concern).
            if ref_.block_content(&uri).is_none() {
                return false;
            }
            let is_task = ref_
                .task_state_of(&uri)
                .is_some_and(|state| !state.is_empty());
            is_task
                && !exempt.contains(&uri)
                && !ref_.is_page_block(&uri)
                && !ref_.is_layout_block(&uri)
        };
        let missing = task_tree_rows_missing_state_toggle(&root, &is_checked_task);
        if missing.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[inv-viewmodel-task-rows-have-state-toggle] {} task-backed row(s) render NO \
                 state_toggle in their own row scope: {missing:?}. The row degraded to a bare \
                 text/title rendering (the flat-live-query page_title misfire — bugfunnel \
                 2026-08-25-flat-query-task-rows-render-as-page-title-blobs).",
                missing.len()
            ))
        }
    }
}
