//! `inv-embedded-page-collapsed-lazy`.
//!
//! @pbt oracle correspondence
//! @pbt covers lazy embedded-page gating — expand_toggle presence and
//!   collapsed→no-descendants-in-panel vs the ref expansion state
//! @pbt slips-if-removed a collapsed embedded page leaks its children into the
//!   main panel (lazy gate bypassed → unbounded eager load) or renders with no
//!   expand_toggle affordance
//!
//! Embedded page-blocks (non-seed pages that are strict descendants of the Main
//! region's current focus root) must render with an `expand_toggle` widget node
//! targeting them. When collapsed (default / ref not in `expanded_toggles`):
//! `expanded=="false"` AND no ref-known strict descendant of the page may
//! appear in the main-panel widget tree (children are lazy-loaded on expand).
//! When expanded (ref in `expanded_toggles`): `expanded=="true"` OR descendants
//! MAY appear (settle-tolerant — the lazy live_query fires asynchronously).
//!
//! Skip semantics: returns Skip when the widget tree / main-panel is not ready
//! (same gating as `inv-main-panel-rows-match-focus`) or when the ref contains
//! no non-seed page that is a strict descendant of the main focus root (the
//! journals companion closure is the only keystone draw that satisfies this
//! today). Vacuously green on the default structural-page topology.

use std::collections::BTreeSet;

use holon_api::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefToggle;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvEmbeddedPageCollapsedLazy;

impl InvEmbeddedPageCollapsedLazy {
    pub const ID: InvariantId = InvariantId("inv-embedded-page-collapsed-lazy");
}

/// Find the first node in pre-order whose entity_id equals `id`.
fn find_by_entity_id<'a>(root: &'a WidgetSnapshot, id: &str) -> Option<&'a WidgetSnapshot> {
    root.walk().find(|n| n.entity_id.as_deref() == Some(id))
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvEmbeddedPageCollapsedLazy
where
    R: RefBlockTree + RefLayout + RefViewSelection + RefFocus + RefToggle,
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
        // overwhelming common case — the panel is settled and the prong passes,
        // so it adds no latency and re-uses the tick-shared recompute.
        match Self::evaluate(ref_, &sut.widget_tree_snapshot().await) {
            InvariantResult::Ok => return InvariantResult::Ok,
            InvariantResult::Skipped(s) => return InvariantResult::Skipped(s),
            InvariantResult::Fail(_) => {}
        }

        // A FAIL here is NOT automatically a real leak / missing affordance.
        // After a BlockToPage the one-shot STRICT snapshot can catch a LEGAL
        // transient frame: the `focus_descendants` recursive-CTE matview's
        // prune delta (which drops the just-collapsed page's children from the
        // main panel) lands a frame AFTER the snapshot, and the `expand_toggle`
        // affordance for the newly-minted page appears on the same async CDC
        // beat. Both self-heal within a settle. So model prod like the sibling
        // fixed-point invariants: bounded-wait for a STABLE pass, re-sampling
        // via the NON-cached `widget_tree_snapshot_fresh` (the cached snapshot
        // is frozen per tick and can never observe the heal). Fail only if a
        // violation PERSISTS.
        use std::time::Duration;
        use std::time::Instant;
        let budget = Duration::from_secs(5);
        let stable_for = Duration::from_millis(200);
        let deadline = Instant::now() + budget;
        let mut stable_since: Option<Instant> = None;
        let mut last_fail = String::new();
        loop {
            if Instant::now() >= deadline {
                return InvariantResult::Fail(format!(
                    "[inv-embedded-page-collapsed-lazy] violation PERSISTED for {budget:?} — not a \
                     transient focus_descendants prune-delta / affordance-appearance frame but a \
                     real leak / missing affordance.\n{last_fail}"
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

impl InvEmbeddedPageCollapsedLazy {
    /// Pure, snapshot-in / result-out evaluation of both prongs (affordance +
    /// collapsed-lazy descendants) against ONE widget-tree snapshot. Split out
    /// so the bounded-wait loop can re-run it over fresh re-samples. All
    /// reference reads are sync; only the SUT snapshot is async (obtained by
    /// the caller).
    fn evaluate<R>(ref_: &R, root: &WidgetSnapshot) -> InvariantResult
    where
        R: RefBlockTree + RefLayout + RefViewSelection + RefFocus + RefToggle,
    {
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

        let main_focus_roots: BTreeSet<EntityUri> = ref_
            .expected_focus_root_rows()
            .into_iter()
            .filter(|(region, _)| region == "main")
            .flat_map(|(_, ids)| ids)
            .filter_map(|id| EntityUri::parse(&id).ok())
            .collect();

        let non_seed = ref_.all_non_seed_block_ids();

        let panel_ids: BTreeSet<String> = panel.collect_canonical_entity_ids();

        // Raw embedded-page set: every non-seed page strictly under the main
        // focus root.
        let raw_embedded: Vec<&EntityUri> = non_seed
            .iter()
            .filter(|id| {
                ref_.is_page_block(id)
                    && ref_.is_descendant_of_any(id, &main_focus_roots)
                    && !main_focus_roots.contains(id)
            })
            .collect();

        // Oracle narrowing (verdict A, 2026-07-25): a page owes an
        // `expand_toggle` ONLY if its ENTIRE embedded-page ancestor chain
        // (below the focus root) is expanded. A page nested under a COLLAPSED
        // embedded page is collapse-hidden BY DESIGN — the composed render
        // correctly emits no toggle for it, and the descendants prong already
        // (correctly) requires it ABSENT from the panel. Demanding a toggle for
        // such a hidden page was a SELF-CONTRADICTION in the affordance prong
        // (deeper than the slice's single-level topology exercises). Only pages
        // whose whole embedded-ancestor chain is expanded are actually rendered
        // and owe a toggle.
        let embedded_pages: Vec<&EntityUri> = raw_embedded
            .iter()
            .copied()
            .filter(|page| {
                !raw_embedded.iter().any(|ancestor| {
                    **ancestor != **page
                        && ref_.is_descendant_of_any(page, &{
                            let mut s = BTreeSet::new();
                            s.insert((*ancestor).clone());
                            s
                        })
                        && !ref_.is_expanded(ancestor)
                })
            })
            .collect();

        if embedded_pages.is_empty() {
            return InvariantResult::Skipped(
                "no non-seed page under the main focus root owes an expand_toggle (none exist, or \
                 all are collapse-hidden under a collapsed embedded-page ancestor); nothing to \
                 check"
                    .into(),
            );
        }

        let focus_root_strs: Vec<String> = main_focus_roots
            .iter()
            .map(|r| r.as_str().to_string())
            .collect();
        let expand_toggles: Vec<&WidgetSnapshot> = panel.collect_by_kind("expand_toggle");

        for page in &embedded_pages {
            let page_str = page.as_str().to_string();
            let ref_expanded = ref_.is_expanded(page);

            let toggle = expand_toggles
                .iter()
                .find(|n| n.props.get("target_id").map(String::as_str) == Some(page_str.as_str()));
            let toggle_expanded = toggle
                .and_then(|t| t.props.get("expanded"))
                .map(String::as_str)
                == Some("true");

            let descendants_in_panel: Vec<String> = non_seed
                .iter()
                .filter(|id| {
                    let id_str = id.as_str().to_string();
                    id_str != page_str
                        && ref_.is_descendant_of_any(id, &{
                            let mut s = BTreeSet::new();
                            s.insert((*page).clone());
                            s
                        })
                        && panel_ids.contains(&id_str)
                })
                .map(|u| u.as_str().to_string())
                .collect();

            // MUST have a toggle for every embedded page.
            if toggle.is_none() {
                return InvariantResult::Fail(format!(
                    "[inv-embedded-page-collapsed-lazy] embedded page {page_str} (strict \
                     descendant of main focus root{roots}) has NO expand_toggle in the main-panel \
                     widget tree. Affordance prong FAILS — the embedded_page profile variant must \
                     wrap the page in an expand_toggle.",
                    roots = focus_root_strs
                        .iter()
                        .map(|s| format!(" {s}"))
                        .collect::<Vec<_>>()
                        .join(""),
                ));
            }

            if ref_expanded {
                // Expanded: descendants are allowed (the lazy live_query fired).
                // Settle-tolerant — descendants may or may not have appeared yet.
                // Consistency check: widget toggle should also report expanded.
                if !toggle_expanded {
                    return InvariantResult::Fail(format!(
                        "[inv-embedded-page-collapsed-lazy] embedded page {page_str} is expanded \
                         in the ref model but the widget tree toggle reports expanded=false. \
                         Ref-model vs SUT consistency FAILS.",
                    ));
                }
            } else {
                // Collapsed: widget toggle must report collapsed AND no descendants
                // may appear in the panel.
                if toggle_expanded {
                    return InvariantResult::Fail(format!(
                        "[inv-embedded-page-collapsed-lazy] embedded page {page_str} is collapsed \
                         in the ref model but the widget tree toggle reports expanded=true. \
                         Ref-model vs SUT consistency FAILS.",
                    ));
                }
                if !descendants_in_panel.is_empty() {
                    return InvariantResult::Fail(format!(
                        "[inv-embedded-page-collapsed-lazy] embedded page {page_str} has a \
                         collapsed expand_toggle (affordance prong PASSES) but its ref-known \
                         strict descendants {descendants_in_panel:?} are present in the \
                         main-panel widget tree — descendants prong FAILS (children leaked past \
                         the lazy gate).",
                    ));
                }
            }
        }

        InvariantResult::Ok
    }
}
