//! `wheel_occlusion_routing` wired into the composed catalog — a **windowed**
//! WheelScroll postcondition guard. Needs `SutLayout` + `SutFrontendEngine`
//! (windowed only); the body Skips unless a sticky footer is on screen, so it
//! engages non-vacuously only on the Journals-shaped sticky compositions (Inc
//! E).
//!
//! `Strict`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::wheel_occlusion_routing::InvWheelOcclusionRouting;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvWheelOcclusionRouting,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutLayout>(),
                CapId::of::<dyn SutFrontendEngine>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::Render, file!()),
    ))
}
