//! `inv-paint-text-styling` wired into the composed catalog — the read-mode
//! inline-styling paint check (§ `bodies::paint_text_styling`).
//!
//! `Needs` both `SutLayout` (the painted styled-run fingerprint) and
//! `SutBackend` (the write-side `(content, marks)` the paint must honour);
//! ref-independent. The headless slice supplies no `SutLayout`, so it is
//! deselected there — only the windowed composed slice
//! (`window_slice::window_wide`) selects it, and an empty geometry snapshot is
//! `Skipped` rather than failed.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::paint_text_styling::InvPaintTextStyling;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvPaintTextStyling,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutLayout>(), CapId::of::<dyn SutBackend>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
