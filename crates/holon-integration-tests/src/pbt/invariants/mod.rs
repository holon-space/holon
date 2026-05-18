//! Invariant registry — Phase 3 of the testing strategy plan.
//!
//! Status: **scaffold only.** The 25 invariants live as inline assertions
//! in [`super::sut::E2ESut::check_invariants_async`]. This module declares
//! their *metadata* — id, name, min-SUT subsystem set, run mode
//! (strict/warn) — so PBTs that don't supply every subsystem can filter
//! the invariant set deterministically. Bodies move into closures during
//! Phase 3.2+, one invariant at a time, with a meta-test asserting the
//! wide PBT's selection still equals today's monolithic call.
//!
//! ## Layers
//!
//! - `Subsystem` — the dimensions a PBT's SUT either supplies or doesn't.
//!   Mirrors the audit in `docs/TESTING_INVARIANT_AUDIT.md`.
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
//! invariants exist in the wide-PBT harness today. When a new
//! `inv-...` label appears in `sut.rs`, register it here in the same PR.
//! The arch-test below catches the obvious drift case.

pub mod bodies;
pub mod registry;

pub use registry::{
    InvariantId, InvariantRegistry, InvariantSpec, PbtSuiteSpec, RunMode, Subsystem,
    register_default,
};
