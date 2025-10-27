//! Frontend-owned PBT SUT capability traits (the cap home-rule).
//!
//! A cap lives in the crate that owns the types it names. `NavDirection` is a
//! `holon-frontend` type referenced across the frontend, so the
//! arrow-navigation cap lives here rather than forcing that type down into
//! `holon-pbt-core`.
//!
//! `#[holon_macros::capmap_adapter]` generates the typemap key impl and the
//! forwarding `impl … for holon_pbt_core::composition::CapMap`, so a composed
//! `CapMap` that holds an `Arc<dyn SutArrowNavigate>` is itself a
//! `SutArrowNavigate`. The macro references `::holon_pbt_core::composition::*`
//! by absolute path, which is what lets it expand correctly here (not just
//! inside `holon-pbt-core`).

use holon_pbt_core::capabilities::CapRegion;

use crate::navigation::NavDirection;

/// SUT capability: arrow-key navigation from the focused block within a region,
/// `steps` presses in `direction`. Driven by the `ArrowNavigate` PBT
/// transition.
///
/// `&self` so it is hostable on the composed `CapMap` via `#[capmap_adapter]`
/// (the macro emits `unimplemented!` for `&mut self`). No `ref_state`
/// parameter: any reference-model coupling lives in the harness
/// reconcile/settle seam, not on the action (cap home-rule + the
/// `ref_state`-off-the-cap principle).
#[holon_macros::capmap_adapter]
pub trait SutArrowNavigate {
    async fn apply_arrow_navigate(&self, region: CapRegion, direction: NavDirection, steps: u8);
}
