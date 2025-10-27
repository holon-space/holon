//! `inv-frontend-bounds-rendered`.
//!
//! The geometry compound that lived inside the inline `inv-frontend-engine`
//! block: verifies the gpui window actually laid out the elements the
//! frontend ViewModel emitted. Reads [`SutLayout::rendered_elements`] (the
//! geometry snapshot, with `expected_size`/error verdicts pre-computed
//! SUT-side), the frontend VM's ordered entity ids
//! ([`SutFrontendEngine::frontend_root_vm`]), the screenshot
//! [`SutLayout::visual_content_fraction`], and the reference's
//! [`RefLayout`]. `Skipped` when the frontend root isn't resolved
//! (headless / SqlOnly / still loading).
//!
//! Strict sub-checks (first failure → `Fail`, in inline source order):
//! expected-size-satisfied, no-error-widgets-rendered,
//! vm-y-order-and-contiguity, non-wrapper-content-when-docs,
//! not-visually-empty, vm-data-tracked-as-content. Warn sub-checks (logged,
//! never fail): bounds-registry-not-empty, vm-entities-have-bounds,
//! known-widget-type, element-has-content.
//!
//! Layout blocks (`block:default-*` region wrappers) are excluded from the
//! coverage/order checks — `live_block` deliberately doesn't `tracked()` them,
//! so they're never in the geometry snapshot and would create false gaps.

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RenderedElement;
use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

const LAYOUT_BLOCK_IDS: [&str; 3] = [
    "block:default-main-panel",
    "block:default-left-sidebar",
    "block:default-right-sidebar",
];

pub struct InvFrontendBoundsRendered;

impl InvFrontendBoundsRendered {
    pub const ID: InvariantId = InvariantId("inv-frontend-bounds-rendered");
    const LABEL: &'static str = "inv-frontend-bounds-rendered";
}

fn lookup<'a>(elements: &'a [RenderedElement], eid: &EntityUri) -> Option<&'a RenderedElement> {
    elements.iter().find(|e| e.entity_id.as_ref() == Some(eid))
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvFrontendBoundsRendered
where
    R: RefLayout,
    S: SutLayout + SutFrontendEngine,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let Some(vm) = sut.frontend_root_vm().await else {
            return InvariantResult::Skipped(format!(
                "[{}] no frontend engine / root still loading",
                Self::LABEL
            ));
        };
        let elements = sut.rendered_elements().await;

        // bounds-registry-not-empty (WARN): a layout-time snapshot can be
        // transiently empty during restarts; not-visually-empty is the
        // authoritative empty-UI check.
        if elements.is_empty() {
            tracing::warn!(
                target: "pbt_invariant",
                "[{}/bounds-registry-not-empty] BoundsRegistry is empty — GPUI may not have rendered yet",
                Self::LABEL,
            );
        }

        // expected-size-satisfied (STRICT) — verdict pre-computed SUT-side.
        for el in &elements {
            if let Some(violation) = &el.expected_size_violation {
                return InvariantResult::Fail(format!(
                    "[{}/expected-size-satisfied] Element '{}' violates expected_size: {violation}",
                    Self::LABEL,
                    el.el_id,
                ));
            }
        }

        // no-error-widgets-rendered (STRICT).
        for el in &elements {
            if el.is_error_widget {
                return InvariantResult::Fail(format!(
                    "[{}/no-error-widgets-rendered] BoundsRegistry contains error widget '{}'",
                    Self::LABEL,
                    el.el_id,
                ));
            }
        }

        // vm-entities-have-bounds (WARN): VM entity ids without geometry —
        // uniform_list virtualizes, so not all are rendered.
        let missing: Vec<&EntityUri> = vm
            .entity_ids
            .iter()
            .filter(|eid| !LAYOUT_BLOCK_IDS.contains(&eid.as_str()))
            .filter(|eid| lookup(&elements, eid).is_none())
            .collect();
        if !missing.is_empty() {
            tracing::warn!(
                target: "pbt_invariant",
                "[{}/vm-entities-have-bounds] {} VM entity id(s) have no BoundsRegistry entry: {:?}",
                Self::LABEL,
                missing.len(),
                &missing[..missing.len().min(5)],
            );
        }

        // known-widget-type (WARN): entity ids present in both VM and bounds
        // should carry a known rendering wrapper.
        for el in &elements {
            if let Some(eid) = &el.entity_id
                && vm.entity_ids.contains(eid)
                && !matches!(
                    el.widget_type.as_str(),
                    "render_entity"
                        | "live_block"
                        | "editable_text"
                        | "rendered_text"
                        | "selectable"
                )
            {
                tracing::warn!(
                    target: "pbt_invariant",
                    "[{}/known-widget-type] Element '{}' entity={eid} has unexpected widget_type='{}'",
                    Self::LABEL,
                    el.el_id,
                    el.widget_type,
                );
            }
        }

        // element-has-content (WARN).
        for el in &elements {
            if !el.has_content {
                tracing::warn!(
                    target: "pbt_invariant",
                    "[{}/element-has-content] Element '{}' (widget_type='{}') has has_content=false",
                    Self::LABEL,
                    el.el_id,
                    el.widget_type,
                );
            }
        }

        // vm-y-order-and-contiguity (STRICT): rendered elements that map to VM
        // entity ids must appear in the same y order and form a contiguous
        // subsequence of the VM's (layout-block-excluded) entity list.
        let contiguity_ids: Vec<&EntityUri> = vm
            .entity_ids
            .iter()
            .filter(|eid| !LAYOUT_BLOCK_IDS.contains(&eid.as_str()))
            .collect();
        let rendered: Vec<(usize, &EntityUri, f32)> = contiguity_ids
            .iter()
            .enumerate()
            .filter_map(|(vm_idx, eid)| lookup(&elements, eid).map(|el| (vm_idx, *eid, el.y)))
            .collect();
        if rendered.len() >= 2 {
            for pair in rendered.windows(2) {
                let (_, id_a, y_a) = pair[0];
                let (_, id_b, y_b) = pair[1];
                if y_b < y_a {
                    return InvariantResult::Fail(format!(
                        "[{}/vm-y-order-and-contiguity] Y-order violation: '{id_a}' at y={y_a:.0} \
                         appears before '{id_b}' at y={y_b:.0}",
                        Self::LABEL,
                    ));
                }
            }
            for pair in rendered.windows(2) {
                let (idx_a, id_a, _) = pair[0];
                let (idx_b, id_b, _) = pair[1];
                if idx_b != idx_a + 1 {
                    return InvariantResult::Fail(format!(
                        "[{}/vm-y-order-and-contiguity] Non-contiguous rendering: '{id_a}' at VM \
                         index {idx_a} and '{id_b}' at VM index {idx_b} — gap of {} entities",
                        Self::LABEL,
                        idx_b - idx_a - 1,
                    ));
                }
            }
        }

        // `root_kind == "table"` means the render-expr matview hasn't delivered
        // the columns() expression yet (transient loading) — gate the
        // content-presence checks off it to avoid false positives.
        let layout_ready = vm.root_kind != "table";
        let has_documents = ref_.has_user_documents();
        let has_content_widget = elements.iter().any(|e| e.widget_type != "live_block");

        // non-wrapper-content-when-docs (STRICT).
        if has_documents && layout_ready && !elements.is_empty() && !has_content_widget {
            return InvariantResult::Fail(format!(
                "[{}/non-wrapper-content-when-docs] reference has user document(s) and \
                 BoundsRegistry has {} elements, but all are live_block wrappers — no content \
                 widgets rendered",
                Self::LABEL,
                elements.len(),
            ));
        }

        // not-visually-empty (STRICT): pixel-level backstop when bounds report
        // no content widget. Weaker threshold when the main panel isn't focused
        // (a sidebar-only UI is legitimately sparse).
        if has_documents && layout_ready && !has_content_widget {
            let main_focused =
                ref_.region_entity_focused(holon_pbt_core::capabilities::CapRegion::Main);
            let min_content = if main_focused { 0.003 } else { 0.001 };
            if let Some(content_fraction) = sut.visual_content_fraction().await
                && content_fraction <= min_content
            {
                return InvariantResult::Fail(format!(
                    "[{}/not-visually-empty] UI is visually empty: \
                     content_fraction={content_fraction:.4} <= {min_content:.4} (reference has \
                     documents, main_focused={main_focused}, bounds have no content widget)",
                    Self::LABEL,
                ));
            }
        }

        // vm-data-tracked-as-content (STRICT, with a loading/propagation WARN
        // downgrade): non-region-wrapper VM entity ids represent real data; at
        // least one must be tracked as a non-`live_block` content widget.
        let data_entity_ids: Vec<&EntityUri> = vm
            .entity_ids
            .iter()
            .filter(|eid| !eid.as_str().starts_with("block:default-"))
            .collect();
        if !data_entity_ids.is_empty() {
            let content_match_count = data_entity_ids
                .iter()
                .filter(|eid| {
                    lookup(&elements, eid)
                        .map(|el| el.widget_type != "live_block")
                        .unwrap_or(false)
                })
                .count();
            let all_have_some_widget = data_entity_ids
                .iter()
                .all(|eid| lookup(&elements, eid).is_some());
            let has_loading = elements.iter().any(|e| e.widget_type == "loading");
            if content_match_count == 0 {
                if has_loading || !all_have_some_widget {
                    tracing::warn!(
                        target: "pbt_invariant",
                        "[{}/vm-data-tracked-as-content] {} data entity id(s) not yet tracked as \
                         content widgets (loading={has_loading}, all_have_widget={all_have_some_widget}): {:?}",
                        Self::LABEL,
                        data_entity_ids.len(),
                        &data_entity_ids[..data_entity_ids.len().min(5)],
                    );
                } else {
                    return InvariantResult::Fail(format!(
                        "[{}/vm-data-tracked-as-content] ViewModel has {} data entity id(s) but \
                         none are tracked as content widgets \
                         (render_entity/editable_text/selectable): {:?}",
                        Self::LABEL,
                        data_entity_ids.len(),
                        &data_entity_ids[..data_entity_ids.len().min(5)],
                    ));
                }
            }
        }

        InvariantResult::Ok
    }
}
