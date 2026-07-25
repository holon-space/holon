//! `inv-main-panel-rows-match-focus`.
//!
//! @pbt oracle correspondence
//! @pbt covers stale-rows-on-nav — ref-known main-panel rows must ⊆ Main's
//!   current focus-root subtree (ref.expected_visible_content_ids)
//! @pbt slips-if-removed a CDC delete that fails to propagate through the
//!   focus_roots→main-panel chained matview leaves the previous root's rows
//!   in the panel after navigation (dioxus w4-web-05, GPUI stale sidebar)
//!
//! **No stale rows after navigation**: every reference-known block rendered
//! inside the MAIN PANEL widget subtree must belong to the Main region's
//! *current* focus-root subtree (`RefLayout::expected_visible_content_ids`,
//! which is "descendants of the region's expected focus roots" — including
//! source blocks, which render as visible rule-card / query-result rows per the
//! fork-A program-rendering ruling) or be layout/profile scaffolding. A block
//! that is ref-known but outside that
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
//!   panel with an out-of-subtree ref-known block that PERSISTS Fails.
//! - Settle semantics (2026-07-25): the cached widget snapshot is frozen per
//!   tick, so a one-shot read judges an asynchronously-settling projection at a
//!   single instant. Re-parenting (Indent/Outdent) makes a subtree depart the
//!   Main focus root and the `focus_descendants` recursive-CTE matview's prune
//!   delta lands a frame later. The check therefore bounded-waits for a STABLE
//!   pass over NON-cached re-samples (5s budget, 200ms stable window),
//!   mirroring `inv-embedded-page-collapsed-lazy`. A genuine stale row never
//!   heals, so it still Fails — the teeth are unchanged, only instantaneity is
//!   no longer asserted.
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
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityUri;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

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
    R: RefViewSelection + RefLayout + RefFocus + RefBlockTree,
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

        // Fast path: evaluate the current (cached) snapshot. This is the
        // overwhelming common case — the panel is settled and the check
        // passes, so it adds no latency and re-uses the tick-shared recompute.
        match Self::evaluate(ref_, &sut.widget_tree_snapshot().await) {
            InvariantResult::Ok => return InvariantResult::Ok,
            InvariantResult::Skipped(s) => return InvariantResult::Skipped(s),
            InvariantResult::Fail(_) => {}
        }

        // A FAIL here is NOT automatically a real stale row. Re-parenting
        // (Indent/Outdent) makes a subtree DEPART the Main focus root; the
        // `focus_descendants` recursive-CTE matview's PRUNE delta lands a
        // frame AFTER this one-shot snapshot, so a mid-prune frame renders
        // rows that are already out of the focus subtree in the ref. That
        // heals within a settle. So model prod like the sibling fixed-point
        // invariants: bounded-wait for a STABLE pass, re-sampling via the
        // NON-cached `widget_tree_snapshot_fresh` (the cached snapshot is
        // frozen per tick and can never observe the heal). Fail only if the
        // violation PERSISTS — a genuine stale row never heals.
        let budget = Duration::from_secs(5);
        let stable_for = Duration::from_millis(200);
        let deadline = Instant::now() + budget;
        let mut stable_since: Option<Instant> = None;
        let mut last_fail = String::new();
        loop {
            if Instant::now() >= deadline {
                return InvariantResult::Fail(format!(
                    "[inv-main-panel-rows-match-focus] violation PERSISTED for {budget:?} — not a \
                     transient focus_descendants prune-delta frame but a real stale \
                     row.\n{last_fail}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            match Self::evaluate(ref_, &sut.widget_tree_snapshot_fresh().await) {
                InvariantResult::Ok => match stable_since {
                    Some(t) if t.elapsed() >= stable_for => return InvariantResult::Ok,
                    Some(_) => {}
                    None => stable_since = Some(Instant::now()),
                },
                // A transient Skip mid-wait (panel dropped out of this frame)
                // just resets the stability window — keep waiting for a stable
                // pass rather than treating it as terminal.
                InvariantResult::Skipped(s) => {
                    stable_since = None;
                    last_fail = format!("(transient skip) {s}");
                }
                InvariantResult::Fail(f) => {
                    stable_since = None;
                    last_fail = f;
                }
            }
        }
    }
}

impl InvMainPanelRowsMatchFocus {
    /// Pure, snapshot-in / result-out evaluation against ONE widget-tree
    /// snapshot. Split out so the bounded-wait loop can re-run it over fresh
    /// re-samples. All reference reads are sync; only the SUT snapshot is
    /// async (obtained by the caller).
    fn evaluate<R>(ref_: &R, root: &WidgetSnapshot) -> InvariantResult
    where
        R: RefViewSelection + RefLayout + RefFocus + RefBlockTree,
    {
        // Layout-less mode: no main panel exists, so there is no per-region
        // row set to judge — the whole tree is "the content".
        let Some(main_panel_id) = ref_.main_panel_block_id() else {
            return InvariantResult::Ok;
        };

        let Some(panel) = find_by_entity_id(root, main_panel_id.as_str()) else {
            return InvariantResult::Skipped(format!(
                "main-panel node (entity_id '{}') not yet present under root '{}' (not rendered \
                 in this snapshot tick)",
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

        let panel_ids = panel.collect_canonical_entity_ids();
        let stale: Vec<&String> = panel_ids
            .iter()
            .filter(|id| ref_known.contains(*id) && !allowed.contains(*id))
            .collect();

        if stale.is_empty() {
            return InvariantResult::Ok;
        }

        let focus_roots: Vec<(String, Vec<String>)> = ref_.expected_focus_root_rows();

        // The REF ancestor chain of each stale id is what separates a
        // transient prune-delta frame (chain leads to a subtree that just
        // DEPARTED the focus root via Indent/Outdent) from a real stale row
        // (chain leads to a previously-focused root). Without it the red is
        // unreadable.
        let chains: Vec<String> = stale
            .iter()
            .map(|id| {
                let Ok(uri) = EntityUri::parse(id) else {
                    return format!("{id} -> <unparseable EntityUri>");
                };
                let mut chain = vec![uri.as_str().to_string()];
                let mut cursor = uri;
                // Bounded by the ref's block count: a cycle in `parent_of`
                // would be a ref-model corruption, so cap and report it.
                for _ in 0..ref_known.len() + 1 {
                    match ref_.parent_of(&cursor) {
                        Some(parent) => {
                            chain.push(parent.as_str().to_string());
                            cursor = parent;
                        }
                        None => return chain.join(" < "),
                    }
                }
                panic!(
                    "[inv-main-panel-rows-match-focus] ref `parent_of` chain for {id} exceeded \
                     the ref block count — cycle in the reference block tree: {chain:?}"
                );
            })
            .collect();

        InvariantResult::Fail(format!(
            "[inv-main-panel-rows-match-focus] STALE ROW(S) IN MAIN PANEL — ref-known blocks \
             rendered inside the main-panel subtree that are NOT in the current Main focus-root \
             subtree (previous root's rows lingering after navigation / focus_roots \
             chained-matview delete not propagated?).\n  stale ids: {stale:?}\n  stale REF \
             ancestor chains (child < parent < …):\n{}\n  expected focus roots (per region): \
             {focus_roots:?}\n  allowed set ({} ids), panel rendered ids ({}): {panel_ids:?}",
            chains
                .iter()
                .map(|c| format!("    {c}"))
                .collect::<Vec<_>>()
                .join("\n"),
            allowed.len(),
            panel_ids.len(),
        ))
    }
}
