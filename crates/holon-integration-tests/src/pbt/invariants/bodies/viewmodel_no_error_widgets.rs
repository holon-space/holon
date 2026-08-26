//! `inv-viewmodel-no-error-widgets`.
//!
//! @pbt oracle internal-consistency
//! @pbt covers error-widget-in-tree — render-pipeline fault (matview fault,
//!   CDC delivery bug, shadow-interp panic) leaves Error nodes in the tree
//! @pbt slips-if-removed a CDC/interpret failure renders Error placeholders
//!   in the user-visible tree; the app looks structurally fine but shows
//!   error boxes and no oracle flags it
//!
//! Walks the headless `ReactiveEngine`'s rendered FOREST — the root layout
//! tree PLUS every per-block live tree reachable from it — and asserts no
//! `Error` widget nodes exist anywhere. Catches render-pipeline failures
//! (matview fault, CDC delivery bug, shadow-interpretation panic) that leave
//! Error widgets in the user-visible tree.
//!
//! Two capabilities, because the render pipeline produces a forest, not a
//! tree, and `live_block` nodes are REFERENCES rather than inlined subtrees:
//!
//! - `SutViewSelection::headless_error_node_count` counts the ROOT layout's
//!   nodes. `Some(n)`, or `None` when the headless engine isn't installed or
//!   its tree isn't yet ready (loading / placeholder / interpretation
//!   panicked). `None` → `Skipped`, and the per-block walk is not attempted: an
//!   unready root is no evidence about anything.
//! - `SutRenderer::widget_tree_for` resolves one block's own tree. BFS from the
//!   root's `live_block` references, the enumeration
//!   `inv-editable-text-has-draggable` established. Without it a failed block
//!   render — `ui_watcher`'s `error_render_expr` fallback — is outside the
//!   invariant's reach entirely
//!   (`2026-08-26-render-failure-invisible-warn-and-root-only-oracle`).

use std::collections::BTreeSet;
use std::collections::HashSet;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::SutViewSelection;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvViewmodelNoErrorWidgets;

impl InvViewmodelNoErrorWidgets {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-no-error-widgets");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelNoErrorWidgets
where
    S: SutViewSelection + SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let Some(root_count) = sut.headless_error_node_count().await else {
            return InvariantResult::Skipped(
                "headless engine not installed or tree not ready".into(),
            );
        };

        let per_block = per_block_errors(sut).await;
        if root_count == 0 && per_block.is_empty() {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "[inv-viewmodel-no-error-widgets] {root_count} error node(s) in the root ViewModel \
             tree, {} in per-block live trees{}",
            per_block.len(),
            if per_block.is_empty() {
                String::new()
            } else {
                format!(": {:?}", per_block.iter().take(10).collect::<Vec<_>>())
            },
        ))
    }
}

/// `"<block id>: <error message>"` for every error widget in a per-block live
/// tree, BFS-discovered from the root's `live_block` references. Blocks that
/// don't resolve (`widget_tree_for` → `None`) are skipped: not-yet-watchable
/// is the loading state, not a render failure.
async fn per_block_errors<S: SutRenderer>(sut: &S) -> BTreeSet<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut visited: HashSet<EntityUri> = HashSet::new();
    let mut worklist: Vec<EntityUri> = live_block_refs(&sut.widget_tree_snapshot().await);

    while let Some(id) = worklist.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(snap) = sut.widget_tree_for(&id).await else {
            continue;
        };
        for node in snap.walk().filter(|n| n.kind == "error") {
            let message = node
                .props
                .get("message")
                .cloned()
                .unwrap_or_else(|| "<error widget with no message prop>".to_string());
            found.insert(format!("{id}: {message}"));
        }
        worklist.extend(live_block_refs(&snap));
    }
    found
}

fn live_block_refs(tree: &WidgetSnapshot) -> Vec<EntityUri> {
    tree.walk()
        .filter(|n| n.kind == "live_block")
        .filter_map(|n| n.entity_id.as_deref())
        .map(|id| EntityUri::parse(id).expect("live_block entity_id must be a valid EntityUri"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str, entity_id: Option<&str>, children: Vec<WidgetSnapshot>) -> WidgetSnapshot {
        WidgetSnapshot {
            kind: kind.to_string(),
            entity_id: entity_id.map(str::to_string),
            props: Default::default(),
            operations: Vec::new(),
            children,
        }
    }

    fn error_node(message: &str) -> WidgetSnapshot {
        let mut n = node("error", None, Vec::new());
        n.props.insert("message".to_string(), message.to_string());
        n
    }

    /// Root layout references one `live_block`; that block's own tree carries
    /// an error widget. `headless_error_node_count` reports the ROOT count
    /// only — the shape a refused `live_query` DDL produces in production.
    struct ForestSut {
        root_error_count: usize,
        root: WidgetSnapshot,
        per_block: Vec<(&'static str, WidgetSnapshot)>,
    }

    #[async_trait::async_trait(?Send)]
    impl SutViewSelection for ForestSut {
        async fn headless_error_node_count(&self) -> Option<usize> {
            Some(self.root_error_count)
        }
        async fn drain_vm_emissions(&mut self) -> Vec<String> {
            Vec::new()
        }
        async fn current_view(&self) -> String {
            "all".to_string()
        }
    }

    #[async_trait::async_trait(?Send)]
    impl SutRenderer for ForestSut {
        async fn collection_row_ids(
            &self,
            _: &EntityUri,
        ) -> Option<std::collections::BTreeSet<EntityUri>> {
            None
        }
        async fn widget_tree_snapshot(&self) -> WidgetSnapshot {
            self.root.clone()
        }
        async fn widget_tree_snapshot_fresh(&self) -> WidgetSnapshot {
            self.root.clone()
        }
        async fn widget_tree_for(&self, block_id: &EntityUri) -> Option<WidgetSnapshot> {
            self.per_block
                .iter()
                .find(|(id, _)| *id == block_id.as_str())
                .map(|(_, snap)| snap.clone())
        }
        async fn render_tree_of(&self, _: &EntityUri) -> Option<String> {
            None
        }
        async fn root_data_row_ids(&self) -> BTreeSet<EntityUri> {
            BTreeSet::new()
        }
        async fn root_content_comparison(
            &self,
            _: &[String],
        ) -> Option<(Vec<String>, Vec<String>)> {
            None
        }
        async fn root_render_ready(&self) -> bool {
            true
        }
        async fn root_render_kind(&self) -> Option<String> {
            None
        }
    }

    /// The escape this invariant was blind to
    /// (`2026-08-26-render-failure-invisible-warn-and-root-only-oracle`): a
    /// clean root, an error widget inside a per-block live tree.
    #[tokio::test]
    async fn catches_error_widget_inside_a_per_block_live_tree() {
        let sut = ForestSut {
            root_error_count: 0,
            root: node(
                "column",
                None,
                vec![node("live_block", Some("block:query-host"), Vec::new())],
            ),
            per_block: vec![(
                "block:query-host",
                node(
                    "column",
                    Some("block:query-host"),
                    vec![error_node("matview DDL refused: no such table: nope")],
                ),
            )],
        };

        let result = InvViewmodelNoErrorWidgets.check(&(), &sut).await;

        let InvariantResult::Fail(msg) = result else {
            panic!("an error widget in a per-block live tree must FAIL; got {result:?}");
        };
        assert!(
            msg.contains("block:query-host") && msg.contains("matview DDL refused"),
            "the failure must name the failing block and quote its error message; got {msg:?}",
        );
    }

    /// BFS reaches trees nested more than one live_block deep, and a cycle in
    /// the references terminates.
    #[tokio::test]
    async fn walks_nested_live_blocks_and_terminates_on_cycles() {
        let sut = ForestSut {
            root_error_count: 0,
            root: node(
                "column",
                None,
                vec![node("live_block", Some("block:outer"), Vec::new())],
            ),
            per_block: vec![
                (
                    "block:outer",
                    node(
                        "column",
                        Some("block:outer"),
                        vec![
                            node("live_block", Some("block:inner"), Vec::new()),
                            node("live_block", Some("block:outer"), Vec::new()),
                        ],
                    ),
                ),
                (
                    "block:inner",
                    node("column", Some("block:inner"), vec![error_node("boom")]),
                ),
            ],
        };

        let result = InvViewmodelNoErrorWidgets.check(&(), &sut).await;

        assert!(
            matches!(&result, InvariantResult::Fail(m) if m.contains("block:inner")),
            "a two-hop live_block reference must still be walked; got {result:?}",
        );
    }

    #[tokio::test]
    async fn clean_forest_passes() {
        let sut = ForestSut {
            root_error_count: 0,
            root: node(
                "column",
                None,
                vec![node("live_block", Some("block:a"), Vec::new())],
            ),
            per_block: vec![(
                "block:a",
                node(
                    "column",
                    Some("block:a"),
                    vec![node("editable_text", Some("block:a"), Vec::new())],
                ),
            )],
        };

        assert!(matches!(
            InvViewmodelNoErrorWidgets.check(&(), &sut).await,
            InvariantResult::Ok
        ));
    }

    /// `None` from the root capability still means "tree not ready" — the
    /// per-block walk must not upgrade a Skip into a verdict.
    #[tokio::test]
    async fn unready_root_still_skips() {
        struct Unready;
        #[async_trait::async_trait(?Send)]
        impl SutViewSelection for Unready {
            async fn headless_error_node_count(&self) -> Option<usize> {
                None
            }
            async fn drain_vm_emissions(&mut self) -> Vec<String> {
                Vec::new()
            }
            async fn current_view(&self) -> String {
                "all".to_string()
            }
        }
        #[async_trait::async_trait(?Send)]
        impl SutRenderer for Unready {
            async fn collection_row_ids(
                &self,
                _: &EntityUri,
            ) -> Option<std::collections::BTreeSet<EntityUri>> {
                None
            }
            async fn widget_tree_snapshot(&self) -> WidgetSnapshot {
                node("column", None, vec![error_node("must not be read")])
            }
            async fn widget_tree_snapshot_fresh(&self) -> WidgetSnapshot {
                self.widget_tree_snapshot().await
            }
            async fn widget_tree_for(&self, _: &EntityUri) -> Option<WidgetSnapshot> {
                None
            }
            async fn render_tree_of(&self, _: &EntityUri) -> Option<String> {
                None
            }
            async fn root_data_row_ids(&self) -> BTreeSet<EntityUri> {
                BTreeSet::new()
            }
            async fn root_content_comparison(
                &self,
                _: &[String],
            ) -> Option<(Vec<String>, Vec<String>)> {
                None
            }
            async fn root_render_ready(&self) -> bool {
                false
            }
            async fn root_render_kind(&self) -> Option<String> {
                None
            }
        }

        assert!(matches!(
            InvViewmodelNoErrorWidgets.check(&(), &Unready).await,
            InvariantResult::Skipped(_)
        ));
    }
}
