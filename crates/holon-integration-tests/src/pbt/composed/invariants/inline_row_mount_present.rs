//! `inv-inline-row-mount-present` wired into the composed catalog — the
//! **windowed** mount-faithfulness guard for frontends that resolve doc-block
//! rows inline. Needs `SutLayout` (the real geometry snapshot) +
//! `SutInlineRowMount` (registered only by the TUI overlay), so it is selected
//! only by the TUI windowed slice and deselected by GPUI (which registers
//! `SutPerBlockShellMount` and gets `inv-live-block-shell-present` instead) and
//! by every headless slice (which has neither).
//!
//! `Strict`. The body `Skip`s on an empty snapshot (window still loading) and
//! otherwise fails loudly if no `render_entity`-tagged `block:*` row is
//! present.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutInlineRowMount;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::inline_row_mount_present::InvInlineRowMountPresent;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvInlineRowMountPresent,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutLayout>(),
                CapId::of::<dyn SutInlineRowMount>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::Render, file!()),
    ))
}
