//! The **single-sourced** E4 windowed composition-path check.
//!
//! Both windowed drivers — the `E2ESut` per-step hook
//! (`invariant_runner::run_windowed_composed_check`) and the gpui TestPlatform
//! sim's per-tick hook (`sim_windowed_replay.rs`) — funnel here so there is one
//! copy of "build the windowed `CapMap`, run the composed catalog, assert no
//! divergence + non-vacuity". Previously this logic was duplicated in both call
//! sites; relocating it removes that copy (§8.10: scaffolding goes DOWN).
//!
//! The SUT is the windowed composed `CapMap` ([`window_input_wide`]): the live
//! window's geometry + frontend engine + production `UserDriver`. Building it
//! through `window_input_wide` (not `window_focus_wide`) means its `cap_set()`
//! carries `SutBlockInteract` + `SutArrowNavigate`, so the input transitions
//! are cap-admitted on this path. Selection still picks exactly the windowed
//! *invariants* (the block/storage families deselect — no `SutBackend` here);
//! the extra input caps add no new selected invariant (no invariant body binds
//! them), so the check stays green while the alphabet widens.

use std::sync::Arc;

use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_pbt_core::ComponentSet;
use holon_pbt_core::composition::RunReport;
use holon_pbt_core::composition::run_selected;

use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::reference_capabilities::reference_state_ref_caps;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::reference_state::Resolved;
use crate::pbt::window_slice::builders::compose_windowed_sut;

/// Run the composed catalog over the windowed `CapMap` and assert.
///
/// - `geometry` / `engine` / `driver`: the live window's surfaces (a
///   `BoundsRegistry` clone, the frontend `ReactiveEngine`, and the production
///   `UserDriver` — `SimUserDriver` under TestPlatform, `GpuiUserDriver` under
///   a real window).
/// - `ref_state`: the **live** per-tick reference oracle (not a fresh/seeded
///   one) so the displayed-text / bounds checks compare against what the run
///   expects — load-bearing, not vacuous.
/// - `context`: a caller label embedded in the failure message (e.g. the last
///   transition name, or the tick index).
///
/// Panics on a composed failure (a windowed divergence the composition path
/// catches is a real bug) or if the non-vacuity floor isn't met. Returns the
/// [`RunReport`] for callers that want to inspect timing / selection.
pub async fn run_windowed_composed_check(
    geometry: Box<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
    driver: Arc<dyn UserDriver>,
    ref_state: &ReferenceState,
    context: &str,
) -> RunReport {
    // The windowed CapMap is the `Actor::UI` arm of the ONE composition — built
    // from the live window's handles, described by `ComponentSet::full_gpui()`
    // (the windowed dual of `full_headless`). `compose_sut` cannot build this
    // (thread affinity); `compose_windowed_sut` is its sibling entry.
    let set = ComponentSet::full_gpui();
    let sut_caps = compose_windowed_sut(&set, geometry, engine, driver);
    let ref_caps = reference_state_ref_caps(Resolved::identity(ref_state.clone()).map(Arc::new));
    let report = run_selected(&composed_invariant_catalog(), &sut_caps, &ref_caps).await;

    // Non-vacuity floor: the core windowed geometry invariant must SELECT + run
    // (it binds `SutLayout + SutViewSelection` on the SUT and `RefLayout` on the
    // ref, all present here). A future regression that silently deselects the
    // windowed family — turning "green" into "ran nothing" — fails HERE rather
    // than passing vacuously.
    assert!(
        report.ran_ids().contains(&"inv-frontend-bounds-rendered"),
        "[windowed composed check @ {context}] non-vacuity: inv-frontend-bounds-rendered must run \
         on the composition path (ran: {:?}, deselected: {:?})",
        report.ran_ids(),
        report.deselected.iter().map(|d| d.0).collect::<Vec<_>>(),
    );
    let failures = report.failures();
    assert!(
        failures.is_empty(),
        "[windowed composed check @ {context}] run_selected over the windowed CapMap diverged — \
         the windowed invariants fail on the COMPOSITION path:\n{failures:#?}",
    );
    report
}
