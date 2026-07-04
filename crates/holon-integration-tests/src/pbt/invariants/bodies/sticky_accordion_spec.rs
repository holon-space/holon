//! `inv-sticky-accordion-spec`.
//!
//! @pbt oracle internal-consistency
//! @pbt covers sticky-overlay-geometry — a painted sticky-accordion footer
//!   obeys the spike position law, is the unique active footer, overlaps
//!   nothing, caps under its px-cap, and commits non-degenerate overlay bounds
//! @pbt slips-if-removed a sticky footer that mispositions (drifts off the
//!   viewport-bottom / next-section handoff), paints twice, spills past its
//!   viewport, blows its cap, or fails to register overlay bounds ships a
//!   broken pinned region that VM-only checks cannot see
//!
//! ENGAGEMENT-GATED on sticky-bearing compositions: the body `Skip`s unless the
//! rendered tree contains at least one `accordion_sticky_footer` element, so it
//! is non-vacuous only when a sticky accordion is actually on screen (the
//! Journals-shaped generator arm, Inc E). It wraps the SHARED pure checkers in
//! [`holon_frontend::sticky_accordion`] — the exact functions the windowed
//! dedicated test (`sticky_accordion_pbt`) evaluates — so promotion of that
//! dedicated proof into this keystone is a move, not a rewrite.
//!
//! Single-snapshot checks only (position-spec, exactly-one-footer, no-overlap,
//! cap-under-sticky, overlay-bounds-committed). Settle-stability is a
//! two-snapshot check owned by the dedicated windowed test.

use holon_frontend::sticky_accordion as sa;
use holon_pbt_core::capabilities::RenderedElement;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// Cap fraction the Journals-shaped generator seeds sticky accordions with
/// (Inc E). Matches the shadow builder's `DEFAULT_MAX_HEIGHT_FRACTION`.
const STICKY_FRACTION: f32 = 0.4;

pub struct InvStickyAccordionSpec;

impl InvStickyAccordionSpec {
    pub const ID: InvariantId = InvariantId("inv-sticky-accordion-spec");
    const LABEL: &'static str = "inv-sticky-accordion-spec";
}

fn to_observed(elements: &[RenderedElement]) -> Vec<sa::ObservedRect> {
    elements
        .iter()
        .map(|e| sa::ObservedRect {
            widget_type: e.widget_type.clone(),
            entity_id: e.entity_id.as_ref().map(|u| u.to_string()),
            x: e.x,
            y: e.y,
            w: e.width,
            h: e.height,
        })
        .collect()
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvStickyAccordionSpec
where
    S: SutLayout,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let elements = sut.rendered_elements().await;
        let obs = to_observed(&elements);

        // Engagement gate: only sticky-bearing compositions.
        let has_footer = obs
            .iter()
            .any(|o| o.widget_type == sa::STICKY_FOOTER_WIDGET);
        if !has_footer {
            return InvariantResult::Skipped(format!(
                "[{}] no sticky-accordion footer on screen",
                Self::LABEL
            ));
        }

        let fails = sa::check_all_single(&obs, STICKY_FRACTION);
        if fails.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[{}] {} sticky-accordion spec violation(s): {}",
                Self::LABEL,
                fails.len(),
                fails.join(" | "),
            ))
        }
    }
}
