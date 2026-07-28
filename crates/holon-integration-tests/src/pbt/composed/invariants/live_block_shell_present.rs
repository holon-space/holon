//! `inv-live-block-shell-present` wired into the composed catalog — a
//! **windowed** shell-faithfulness guard. Needs `SutLayout` (the real geometry
//! snapshot) + `SutFrontendEngine` (present only where a live window's engine
//! is) + `SutPerBlockShellMount` (registered only by the GPUI overlay), so it
//! is selected only by the GPUI windowed slice and deselected by every
//! headless slice and by the TUI. It asserts the window routed panel blocks
//! through the production per-block `ReactiveShell` wrapper — the mount path
//! two dedicated fixtures omitted this week, hiding real bugs.
//!
//! The mount cap is load-bearing, not decoration: the `live_block` tag is a
//! GPUI-only observable (the TUI resolves rows inline and registers them as
//! `render_entity`), so without it this invariant was a permanent red on the
//! TUI windowed slice. Its TUI counterpart is
//! `inv-inline-row-mount-present`.
//!
//! `Strict`. The body `Skip`s on an empty snapshot (window still loading) and
//! otherwise fails loudly if no `live_block`-tagged `block:default-*` panel is
//! present (the bare-mount masking signature).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::SutPerBlockShellMount;
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
                CapId::of::<dyn SutPerBlockShellMount>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
