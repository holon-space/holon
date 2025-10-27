//! `inv-editable-text-has-draggable`.
//!
//! Walks `WidgetSnapshot` trees per-block using `SutRenderer::widget_tree_for`,
//! BFS-discovered from `live_block` references.
//!
//! Asserts: in any per-block-subtree whose rendering contains ≥1
//! `draggable` widget (the block_profile-render signal), every
//! `editable_text` and `rendered_text` widget must be paired with a
//! `draggable` carrying the same entity_id. Drift means drag&drop
//! silently breaks for the unpaired blocks (production GPUI's draggable
//! short-circuits when row_id is None).
//!
//! Per-tree scoping matters: a custom non-block_profile render (e.g.
//! a sidebar list of editable_texts with no draggables) is intentional
//! and is skipped — only trees that have at least one draggable enforce
//! pairing.

use std::collections::BTreeSet;
use std::collections::HashSet;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvEditableTextHasDraggable;

impl InvEditableTextHasDraggable {
    pub const ID: InvariantId = InvariantId("inv-editable-text-has-draggable");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvEditableTextHasDraggable
where
    R: RefLayout,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // Test-environment alternate block profile renders bare
        // row(editable_text(...)) without draggable wrappers — that's
        // intentional and would produce false positives. Inline check
        // gates the same way.
        if ref_.has_blocks_profile() {
            return InvariantResult::Skipped(
                "test environment has registered an alternate block profile".into(),
            );
        }

        let root = sut.widget_tree_snapshot().await;
        let mut missing: BTreeSet<String> = BTreeSet::new();
        let mut visited: HashSet<EntityUri> = HashSet::new();

        // Inspect root first; harvest discovered live_block refs.
        let mut worklist: Vec<EntityUri> = Vec::new();
        inspect_tree(&root, &mut missing, &mut worklist);

        // BFS through discovered live_block trees. Each `widget_tree_for`
        // call returns a self-contained subtree; nested live_block refs
        // in it surface more block ids for the next iteration.
        while let Some(id) = worklist.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            let Some(snap) = sut.widget_tree_for(&id).await else {
                continue;
            };
            inspect_tree(&snap, &mut missing, &mut worklist);
        }

        if !missing.is_empty() {
            return InvariantResult::Fail(format!(
                "[inv-editable-text-has-draggable] {} editable_text widget(s) have no sibling \
                 draggable carrying the same row_id — drag&drop would silently break: {:?}",
                missing.len(),
                missing.iter().take(10).collect::<Vec<_>>(),
            ));
        }
        InvariantResult::Ok
    }
}

/// Gathers (draggable_ids, editable_ids) within `tree`, pushes any
/// nested `live_block` references into `worklist` for BFS continuation,
/// and records unpaired editable_text ids into `missing` ONLY when the
/// tree has ≥1 draggable (the block_profile signal).
fn inspect_tree(
    tree: &holon_pbt_core::capabilities::WidgetSnapshot,
    missing: &mut BTreeSet<String>,
    worklist: &mut Vec<EntityUri>,
) {
    let mut draggable: BTreeSet<String> = BTreeSet::new();
    let mut editable: BTreeSet<String> = BTreeSet::new();
    for node in tree.walk() {
        match node.kind.as_str() {
            "draggable" => {
                if let Some(id) = &node.entity_id {
                    draggable.insert(id.clone());
                }
            }
            "editable_text" | "rendered_text" => {
                if let Some(id) = &node.entity_id {
                    editable.insert(id.clone());
                }
            }
            "live_block" => {
                if let Some(id) = &node.entity_id {
                    worklist.push(
                        EntityUri::parse(id)
                            .expect("live_block entity_id must be a valid EntityUri"),
                    );
                }
            }
            _ => {}
        }
    }
    if !draggable.is_empty() {
        for id in editable.difference(&draggable) {
            missing.insert(id.clone());
        }
    }
}
