//! `inv-live-block-shell-present` wired into the composed catalog — a
//! **windowed** shell-faithfulness guard. Needs `SutLayout` (the real geometry
//! snapshot) + `SutFrontendEngine` (present only where a live gpui window's
//! engine is), so it is selected only by the windowed slice and deselected by
//! every headless slice (which has neither). It asserts the window routed panel
//! blocks through the production per-block `ReactiveShell` wrapper — the mount
//! path two dedicated fixtures omitted this week, hiding real bugs.
//!
//! `Strict`. The body `Skip`s on an empty snapshot (window still loading) and
//! otherwise fails loudly if no `live_block`-tagged `block:default-*` panel is
//! present (the bare-mount masking signature).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::live_block_shell_present::InvLiveBlockShellPresent;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLiveBlockShellPresent,
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
