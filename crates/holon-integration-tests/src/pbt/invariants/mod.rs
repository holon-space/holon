//! Invariant registry.
//!
//! This module declares each invariant's *metadata* — id, name, min-SUT
//! subsystem set, run mode (strict/warn) — so PBTs that don't supply every
//! subsystem can filter the invariant set deterministically. The executable
//! logic lives in the `bodies/` `Invariant<R, S>` impls, run by
//! [`super::invariant_runner`].
//!
//! ## Layers
//!
//! - `Subsystem` — the dimensions a PBT's SUT either supplies or doesn't.
//!   Mirrors the audit in `docs/Testing/TESTING_INVARIANT_AUDIT.md`.
//! - `InvariantId`, `InvariantSpec` — addressable metadata.
//! - `InvariantRegistry` — `Vec<InvariantSpec>`, constructed once.
//! - `PbtSuiteSpec` — the subsystems a PBT's SUT *does* supply; supports
//!   `select(...)` over a registry.
//! - `RunMode` — preserves the warn/error distinction documented in the
//!   audit (three of today's invariants downgrade to a log line under
//!   CDC-lag conditions; strict-only enforcement would re-introduce the
//!   flakes those WARN paths were added to handle).
//!
//! ## Source of truth
//!
//! The registry built by [`register_default`] is *the* manifest of which
//! invariants exist in the wide-PBT harness. When a new invariant body is
//! added under `bodies/`, register it here in the same PR. The arch-test
//! below catches the obvious drift case.

pub mod block_compare;
pub mod bodies;
pub mod registry;
pub mod text_compare;

pub use registry::{
    InvariantId, InvariantRegistry, InvariantSpec, PbtSuiteSpec, RunMode, Subsystem,
    register_default,
};
