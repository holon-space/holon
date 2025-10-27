//! Invariant bodies + the `Subsystem` seeding axis.
//!
//! The executable invariant logic lives in the `bodies/` `Invariant<R, S>`
//! impls. The composed catalog (`composed::catalog`) is the sole manifest of
//! which invariants run and in what mode; the native metadata registry that
//! used to live in [`registry`] was deleted in Phase 2 (2026-07-02). All that
//! survives there is [`Subsystem`], the shrinkable dimension a PBT's SUT
//! supplies — consumed by the composed reference-seeding path.

pub mod block_compare;
pub mod bodies;
pub mod registry;
pub mod text_compare;

pub use registry::Subsystem;
