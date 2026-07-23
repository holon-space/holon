//! `inv-wheel-occlusion-routing`.
//!
//! @pbt oracle internal-consistency
//! @pbt covers wheel-occlusion_routing — occlusion routing — a wheel over the
//! footer leaves the outer offset unchanged and vice versa
//! @pbt slips-if-removed a wheel that mis-routes or moves the pinned footer by
//!   the wrong amount ships a sticky region that jumps or double-scrolls, which
//!   VM-only checks cannot see
//!
//! ENGAGEMENT-GATED on sticky-bearing compositions (Skips unless an
//! `accordion_sticky_footer` is on screen — the Journals-shaped generator arm,
//! Inc E). It wraps the SHARED pure checker
//! [`holon_frontend::sticky_accordion::check_occlusion_routing`] (unit-tested
//! there), so the metamorphic `WheelObservation` the windowed harness captures
//! before/after a `WheelScroll` (Inc E) flows straight in — promotion is a
//! move, not a rewrite.
//!
//! Inc D: the transition + its windowed-only gate land here; the before/after
//! `WheelObservation` capture is the Inc E windowed-harness seam, so the body
//! Skips until then (the disclosed C/E boundary — vacuity resolves in Inc E).

use holon_frontend::sticky_accordion as sa;
use holon_pbt_core::capabilities::RenderedElement;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvWheelOcclusionRouting;

impl InvWheelOcclusionRouting {
    pub const ID: InvariantId = InvariantId("inv-wheel-occlusion-routing");
    const LABEL: &'static str = "inv-wheel-occlusion-routing";
}

fn has_footer(elements: &[RenderedElement]) -> bool {
    elements
        .iter()
        .any(|e| e.widget_type == sa::STICKY_FOOTER_WIDGET)
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvWheelOcclusionRouting
where
    S: SutLayout,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let elements = sut.rendered_elements().await;
        if !has_footer(&elements) {
            return InvariantResult::Skipped(format!(
                "[{}] no sticky-accordion footer on screen",
                Self::LABEL
            ));
        }
        // Read the WheelObservation the harness/driver captured before/after
        // the last WheelScroll. None ⇒ no wheel fired this tick (the composed
        // catalog never sets it) ⇒ Skip.
        let Some(obs) = sa::take_wheel_observation() else {
            return InvariantResult::Skipped(format!(
                "[{}] no WheelScroll observation this tick",
                Self::LABEL
            ));
        };
        match sa::check_occlusion_routing(&obs) {
            Ok(()) => InvariantResult::Ok,
            Err(e) => InvariantResult::Fail(format!("[{}] {e}", Self::LABEL)),
        }
    }
}
