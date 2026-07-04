//! `inv-main-panel-rows-match-focus`.
//!
//! **No stale rows after navigation**: every reference-known block rendered
//! inside the MAIN PANEL widget subtree must belong to the Main region's
//! *current* focus-root subtree (`RefLayout::expectedvisible_content_ids`,
//! which is "non-source descendants of the region's expected focus roots") or
//! be layout/profile scaffolding. A block that is ref-known but outside that
//! set is a STALE ROW — the cross-frontend "previous root's row lingers after
//! navigating" prod bug (dioxus-web `w4-web-05`, GPUI stale sidebar family):
//! the main panel's watch view is a matview chained on the `focus_roots`
//! matview, so a CDC delete that fails to propagate through the chain leaves
//! the previously-focused root's rows in the panel.
//!
//! Scoping and honesty:
//! - Scoped to the main-panel subtree (located semantically via
//!   `RefViewSelection::main_panel_block_id`, like
//!   `inv-viewmodel-root-matches-render-expr`) — the sidebars legitimately
//!   render pages outside Main's focus subtree.
//! - Only REF-KNOWN block ids are judged: ids the reference doesn't know are
//!   `inv-viewmodel-entity-ids-subset-of-data`'s phantom check, not staleness.
//! - Layout-less mode (no main-panel id) and not-ready snapshots Skip/Ok with
//!   the same gating as `inv-viewmodel-root-matches-render-expr`; a rendered
//!   panel with an out-of-subtree ref-known block always Fails — never
//!   weakened.
//!
//! NOTE — no `has_root_render_expr()` gate: in the wide composed run the ref
//! tracks no ROOT render-source (the default 3-column layout keeps its render
//! sources on the panels, not on `root-layout`), so that gate makes an
//! invariant permanently vacuous there (it silently blinds
//! `inv-viewmodel-root-matches-render-expr` and
//! `inv-viewmodel-entity-ids-subset-of-data` today). This body only needs the
//! main-panel id + the widget snapshot.
//!
//! Status: functional; non-vacuity proven by inversion (emptying the expected
//! set reds the keystone on the boot-focus page within 1 case). Added
//! 2026-07-05 to reproduce the cross-frontend stale-row-on-nav prod bug per
//! project rule. Empirical outcome: the keystone is GREEN at the SETTLED
//! state — deletes DO propagate through the chained watch-view matview in
//! this environment (the in-repo `turso_ivm_chained_matview_stale_rows`
//! example no longer reproduces either). The prod linger is therefore either
//! (a) a transient-duration bug (delete lags insert by seconds at vault
//! scale) which a settled-state invariant cannot see — needs a
//! staleness-latency budget probe, or (b) an embedder-wiring gap (e.g. the
//! web worker missing the CRUD provider, so the `closed_at` UPDATE never
//! lands). Divergences recorded in the WS-STALEROW report.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::{
    CapRegion, RefFocus, RefLayout, RefViewSelection, SutRenderer, WidgetSnapshot,
};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvMainPanelRowsMatchFocus;

impl InvMainPanelRowsMatchFocus {
    pub const ID: InvariantId = InvariantId("inv-main-panel-rows-match-focus");
}

/// Find the first node in pre-order whose `entity_id` equals `id`.
fn find_by_entity_id<'a>(root: &'a WidgetSnapshot, id: &str) -> Option<&'a WidgetSnapshot> {
    root.walk().find(|n| n.entity_id.as_deref() == Some(id))
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvMainPanelRowsMatchFocus
where
    R: RefViewSelection + RefLayout + RefFocus,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        if !sut.root_render_ready().await {
            return InvariantResult::Skipped(
                "root render not ready (loading / spacer / not watchable / interpret panic)".into(),
            );
        }
        // Layout-less mode: no main panel exists, so there is no per-region
        // row set to judge — the whole tree is "the content".
        let Some(main_panel_id) = ref_.main_panel_block_id() else {
            return InvariantResult::Ok;
        };

        let root = sut.widget_tree_snapshot().await;
        let Some(panel) = find_by_entity_id(&root, main_panel_id.as_str()) else {
            return InvariantResult::Skipped(format!(
                "main-panel node (entity_id '{}') not yet present under root '{}' \
                 (not rendered in this snapshot tick)",
                main_panel_id.as_str(),
                root.kind,
            ));
        };

        // Ids the panel may legitimately render: the current focus-root
        // subtree for Main + layout scaffolding (panel containers, query/render
        // sources) + profile blocks. All already resolved into SUT id space by
        // the runner's `with_resolved_doc_uris` view.
        let allowed: BTreeSet<String> = ref_
            .expected_visible_content_ids(CapRegion::Main)
            .iter()
            .chain(ref_.layout_block_ids().iter())
            .chain(ref_.profile_block_ids().iter())
            .map(|u| u.as_str().to_string())
            .collect();

        // Judge only ref-known blocks: an unknown id is a phantom
        // (`inv-viewmodel-entity-ids-subset-of-data`'s job), not staleness.
        let ref_known: BTreeSet<String> = ref_
            .all_block_ids()
            .iter()
            .map(|u| u.as_str().to_string())
            .collect();

        let panel_ids = panel.collect_entity_ids();
        let stale: Vec<&String> = panel_ids
            .iter()
            .filter(|id| ref_known.contains(*id) && !allowed.contains(*id))
            .collect();

        if stale.is_empty() {
            return InvariantResult::Ok;
        }

        let focus_roots: Vec<(String, Vec<String>)> = ref_.expected_focus_root_rows();
        InvariantResult::Fail(format!(
            "[inv-main-panel-rows-match-focus] STALE ROW(S) IN MAIN PANEL — ref-known blocks \
             rendered inside the main-panel subtree that are NOT in the current Main focus-root \
             subtree (previous root's rows lingering after navigation / focus_roots chained-matview \
             delete not propagated?).\n  stale ids: {stale:?}\n  \
             expected focus roots (per region): {focus_roots:?}\n  \
             allowed set ({} ids), panel rendered ids ({}): {panel_ids:?}",
            allowed.len(),
            panel_ids.len(),
        ))
    }
}
