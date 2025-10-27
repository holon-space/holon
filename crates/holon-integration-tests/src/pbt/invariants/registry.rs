//! The `Subsystem` axis — the shrinkable dimensions a PBT's SUT supplies.
//!
//! Phase 2 (2026-07-02) deleted the native invariant registry that used to live
//! here (`InvariantSpec`/`InvariantRegistry`/`PbtSuiteSpec`/`register_default`/
//! the `HOLON_PBT_INVARIANTS` parser). The composed catalog
//! (`composed::catalog`) is the sole invariant manifest now, and mode overrides
//! are parsed by `invariant_mode_override.rs`. Only the `Subsystem` enum — the
//! reference-seeding axis consumed by `composed::subsystem_seed` /
//! `composed::wide_e2e` / `frontend_slice` — survives.

use std::collections::BTreeSet;

/// Dimensions a PBT's SUT can supply. Composed reference seeding
/// (`build_started_ref`) derives its `Wiring` from a `BTreeSet<Subsystem>`.
///
/// Mirrors `docs/Testing/TESTING_INVARIANT_AUDIT.md`. Keep in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subsystem {
    /// In-memory block + tree state (no Loro, no Turso).
    BlockTree,
    /// LoroSyncController + Loro doc.
    Loro,
    /// Turso storage + matviews (final state).
    TursoProjection,
    /// CDC stream ordering / delivery semantics (a sub-aspect of
    /// `TursoProjection` called out when an invariant depends on the
    /// stream, not just the final state).
    Cdc,
    /// ReactiveEngine ViewModel tree.
    ViewModel,
    /// Render-expr → ViewModel rendering pipeline (sub-aspect of
    /// `ViewModel` when the renderer specifically is the SUT).
    Renderer,
    /// `InputState` + active-editor mirror.
    EditorState,
    /// Real GPUI/TUI window with a populated `BoundsRegistry`.
    FrontendBounds,
    /// `UserDriver` impl that synthesises interactions.
    Driver,
}

impl Subsystem {
    /// Convenience: every subsystem — what the windowed composed harnesses
    /// (the 4b loop / `gpui_compose_sut_windowed`) supply.
    pub fn all() -> BTreeSet<Subsystem> {
        use Subsystem::*;
        [
            BlockTree,
            Loro,
            TursoProjection,
            Cdc,
            ViewModel,
            Renderer,
            EditorState,
            FrontendBounds,
            Driver,
        ]
        .into_iter()
        .collect()
    }

    /// Subsystems supplied by the headless composed keystone (no real window).
    pub fn headless_wide() -> BTreeSet<Subsystem> {
        let mut s = Self::all();
        s.remove(&Subsystem::FrontendBounds);
        s
    }
}
