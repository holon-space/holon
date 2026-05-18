//! Phase 7 — `inv-focus-matches-ref` (DEFERRED).
//!
//! Inline body lives at `sut.rs:6709–6773`.
//!
//! # Why deferred
//!
//! The body requires:
//! - `self.frontend_engine` (pub, but needs `engine.focused_block()` — not
//!   exposed via `SutDriver::driver_current_focus` which reads SQL, not the engine)
//! - `self.resolve_uri` (private) — to translate ref-state synthetic IDs to real UUIDs
//! - `ref_state.focused_block` — the global focus (not per-region), no Ref* cap today
//! - `ref_state.active_editor` — to skip the check while an editor is open
//!
//! `SutDriver::driver_current_focus` reads `current_focus` SQL, which is the
//! navigation-focus matview, not the engine's global `focused_block` field
//! (which the reactive click handler sets). These are deliberately different
//! sources; mixing them produces false positives. Wire once a `RefFocus`
//! capability exposes the global focus field and a `SutDriver::engine_focused_block`
//! method reads `ReactiveEngine::focused_block()`.

pub struct InvFocusMatchesRef;

impl InvFocusMatchesRef {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-focus-matches-ref");
}

// `Invariant` impl intentionally omitted — body migration deferred.
