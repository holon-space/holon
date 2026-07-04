//! `inv-focus-matches-ref` wired into the composed catalog — the **windowed**
//! differential focus check: the reactive/frontend engine's global focus
//! matches the reference model's global focus after every focus-changing
//! transition. `Needs SutDriver` (the engine focus + `resolve_ref_block_id`
//! bridge) and `RefGlobalFocus + RefEditorMirror` (the ref focus, plus the
//! editor-open guard — editor focus is authoritative while editing). Distinct
//! from `inv-window-focus` (SUT-internal window↔engine coherence, no ref): this
//! one cross-checks against the reference oracle.
//!
//! Selected only by the windowed slice (`window_slice::window_focus_wide`
//! supplies `SutDriver`); storage/headless slices lack `SutDriver` and deselect
//! — disclosed, not faked. The body internally `Skip`s with no global focus /
//! an open editor / no engine, so it is non-vacuous only on focus transitions.
//! Teeth come from the real `run_windowed_composed_check` every tick (no
//! fixture triad — see the `pbt-composition` skill, testing philosophy);
//! `cargo-mutants` is the deferred detection gate.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::SutDriver;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::focus_matches_ref::InvFocusMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFocusMatchesRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutDriver>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefGlobalFocus>(),
                CapId::of::<dyn RefEditorMirror>(),
            ],
        },
        Attribution::at(Layer::Render, file!()),
    ))
}
