//! `inv-window-focus-matches-engine-focus` wired into the composed catalog — the
//! windowed focus-coherence invariant (ADR 0010). `Needs SutDriver + SutLayout`
//! and **no** reference cap: it compares the SUT against itself (the engine's
//! `focused_block` vs the per-frame `RenderedElement::focused` window authority).
//!
//! Selected only by a windowed slice that supplies BOTH `SutLayout` (geometry)
//! and `SutDriver` (the `engine_focused_block` read). The headless
//! `frontend_slice` has neither and the storage slices have neither, so they
//! deselect it — disclosed, not faked. This is the increment-4 catalog wire that
//! becomes reachable once `SutDriver` is `#[capmap_adapter]`-hosted on `CapMap`.
//!
//! `Strict`. The body internally `Skip`s when no geometry is installed, when the
//! engine has no focus, or when engine focus lands on a row with no mounted
//! `editable_text` — so it only bites on a *settled* steal-back / lost-handoff.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{SutDriver, SutLayout};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::window_focus_matches_engine_focus::InvWindowFocusMatchesEngineFocus;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvWindowFocusMatchesEngineFocus,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutDriver>(), CapId::of::<dyn SutLayout>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
