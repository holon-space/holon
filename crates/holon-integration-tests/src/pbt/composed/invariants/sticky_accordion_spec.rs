//! `inv-sticky-accordion-spec` wired into the composed catalog — a **windowed**
//! sticky-overlay geometry guard. Needs `SutLayout` (real geometry) +
//! `SutFrontendEngine` (present only where a live gpui window's engine is), so
//! it is selected only by the windowed slice and deselected by every headless
//! slice. The body itself `Skip`s unless a sticky-accordion footer is on
//! screen, so it engages non-vacuously only on the Journals-shaped sticky
//! compositions (Inc E).
//!
//! `Strict`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::sticky_accordion_spec::InvStickyAccordionSpec;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvStickyAccordionSpec,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutLayout>(),
                CapId::of::<dyn SutFrontendEngine>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
