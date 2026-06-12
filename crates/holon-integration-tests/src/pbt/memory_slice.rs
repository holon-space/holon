//! The pure-memory composed PBT slice (doc §6 "memory-wide") — a slice is just a
//! component list. This module owns only what is *specific to this slice*: its
//! SUT [`components`] (`MemoryBackend` block store + an in-memory editor) and the
//! [`builders`] that compose them. The invariants it runs are **not** listed
//! here — they live in the shared [`crate::pbt::composed`] catalog, and selection
//! (`run_selected(&composed_invariant_catalog(), &these_components, &ref)`) picks
//! the subset whose `Needs` these components satisfy.
//!
//! Why a pure-memory slice exists and §8.5's `min_sut` reclassification does
//! **not** (doc §8.6): the structural invariants' *E2ESut* realization reads
//! Turso SQL, so reclassifying `min_sut → [BlockTree]` would pull them into a
//! Turso-free wiring (the `{Loro}` manifest — once the pinned `loro_backend_pbt`
//! slice, now a generated draw of the convergence harness) and break it. The γ
//! path sidesteps that: this slice provides its **own** `SutBackend` realization
//! over `MemoryBackend` and selects by hosted caps, leaving the legacy
//! `min_sut`/`Subsystem` selection untouched.
//!
//! The cross-cutting / real-`MemoryBackend` tests (selection counts, the
//! mutation- and editor-op-sequence proptests) live in `integration_tests`; the
//! per-invariant catch triads live with their invariant in
//! [`crate::pbt::composed::invariants`], driven by hand-crafted doubles.

pub mod builders;
pub mod components;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod structural_pbt;

pub use builders::{memory_wide, memory_wide_with_editor};
pub use components::{InMemEditorComponent, MemoryBackendComponent};
