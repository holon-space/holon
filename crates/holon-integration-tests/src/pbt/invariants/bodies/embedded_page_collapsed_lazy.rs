//! `inv-embedded-page-collapsed-lazy`.
//!
//! Embedded page-blocks (non-seed pages that are strict descendants of the Main
//! region's current focus root) must render COLLAPSED by default — marked by an
//! `expand_toggle` widget node with `target_id==P` and `expanded=="false"`, and
//! with NO ref-known strict descendant of P present in the main-panel widget
//! tree (children are lazy-loaded on expand).
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
    R: RefBlockTree + RefLayout + RefViewSelection + RefFocus,
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
        let Some(main_panel_id) = ref_.main_panel_block_id() else {
            return InvariantResult::Ok;
        };

        let root = sut.widget_tree_snapshot().await;
        let Some(panel) = find_by_entity_id(&root, main_panel_id.as_str()) else {
            return InvariantResult::Skipped(format!(
                "main-panel node (entity_id '{}') not yet present under root '{}' (not rendered \
                 in this snapshot tick)",
                main_panel_id.as_str(),
                root.kind,
            ));
        };

        // Derive main focus roots from RefFocus::expected_focus_root_rows (available
        // on CapMap via capability_pair! macro, unlike RefFocusRoots).
        let main_focus_roots: BTreeSet<EntityUri> = ref_
            .expected_focus_root_rows()
            .into_iter()
            .filter(|(region, _)| region == "main")
            .flat_map(|(_, ids)| ids)
            .filter_map(|id| EntityUri::parse(&id).ok())
            .collect();

        let non_seed = ref_.all_non_seed_block_ids();

        // Collect the set of ref-known entity_ids visible in the main panel.
        let panel_ids: BTreeSet<String> = panel.collect_canonical_entity_ids();

        // Non-seed pages that are strict descendants of the main focus root.
        let embedded_pages: Vec<&EntityUri> = non_seed
            .iter()
            .filter(|id| {
                ref_.is_page_block(id)
                    && ref_.is_descendant_of_any(id, &main_focus_roots)
                    && !main_focus_roots.contains(id) // strict descendant (not the root itself)
            })
            .collect();

        if embedded_pages.is_empty() {
            return InvariantResult::Skipped(
                "no non-seed page is a strict descendant of the main focus root; nothing to check"
                    .into(),
            );
        }

        let focus_root_strs: Vec<String> = main_focus_roots
            .iter()
            .map(|r| r.as_str().to_string())
            .collect();

        for page in &embedded_pages {
            let page_str = page.as_str().to_string();

            // Look for an expand_toggle node targeting this page with expanded=="false".
            let expand_toggles: Vec<&WidgetSnapshot> = panel.collect_by_kind("expand_toggle");
            let has_collapsed_toggle = expand_toggles.iter().any(|n| {
                n.props.get("target_id").map(String::as_str) == Some(page_str.as_str())
                    && n.props.get("expanded").map(String::as_str) == Some("false")
            });

            if has_collapsed_toggle {
                continue;
            }

            // Check whether any ref-known strict descendant of this page appears in
            // the panel's entity ids — the signal of eager/un-collapsed rendering.
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

            if !descendants_in_panel.is_empty() {
                return InvariantResult::Fail(format!(
                    "[inv-embedded-page-collapsed-lazy] embedded page {page_str} (strict descendant \
                     of main focus root{roots}) is rendered EAGER/EXPANDED: no collapsed \
                     expand_toggle found for it and its ref-known descendants \
                     {descendants_in_panel:?} are present in the main-panel widget tree, but \
                     expanded_toggles = {{}}. Embedded pages must render collapsed by default \
                     with children lazy-loaded on expand.",
                    roots = focus_root_strs
                        .iter()
                        .map(|s| format!(" {s}"))
                        .collect::<Vec<_>>()
                        .join(""),
                ));
            }
        }

        InvariantResult::Ok
    }
}
