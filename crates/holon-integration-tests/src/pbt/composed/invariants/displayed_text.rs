//! `inv-displayed-text/{widget,viewmodel}` wired into the composed catalog — the
//! render-axis text-equivalence family, the same property at two layers (§
//! `bodies::displayed_text`). Both `Strict` and both reference-gated
//! (`RefEditorMirror + RefBlockTree`); a block the oracle doesn't know is
//! skipped, not failed.
//!
//! - `/widget` `Needs SutLayout` — the on-screen geometry text. The windowed
//!   slice supplies it; the headless slice (no `SutLayout`) deselects it.
//! - `/viewmodel` `Needs SutRenderer` — the frontend-agnostic ViewModel-tree
//!   text. Both the windowed `GpuiFrontendEngineComponent` and the headless
//!   `HeadlessFrontendComponent` supply `SutRenderer`, so it runs wherever a
//!   renderer **and** the ref editor/block caps are present.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefBlockTree, RefEditorMirror, SutLayout, SutRenderer};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::displayed_text::{
    InvDisplayedTextViewModel, InvDisplayedTextWidget,
};

/// `inv-displayed-text/widget` — geometry (FrontendBounds) layer.
pub fn wire_widget() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvDisplayedTextWidget,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutLayout>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefEditorMirror>(),
                CapId::of::<dyn RefBlockTree>(),
            ],
        },
    ))
}

/// `inv-displayed-text/viewmodel` — frontend-agnostic ViewModel-tree layer.
pub fn wire_viewmodel() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvDisplayedTextViewModel,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefEditorMirror>(),
                CapId::of::<dyn RefBlockTree>(),
            ],
        },
    ))
}
