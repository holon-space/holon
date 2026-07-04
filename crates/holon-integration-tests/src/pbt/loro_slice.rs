//! The Loro-storage composed PBT slice — a second slice over the **same**
//! shared [`crate::pbt::composed`] catalog, differing from `memory_slice` only
//! in its component: a real `holon-loro` `LoroBackend` CRDT instead of the
//! in-memory store. This is the design's central claim made concrete — adding a
//! storage realization re-runs the entire applicable catalog *for free*, by
//! capability presence, with zero catalog duplication (§6 payoff).
//!
//! Beyond the block-tree caps it shares with the memory slice, the
//! [`LoroBackendComponent`] also provides [`SutLoroLog`], unlocking the
//! Loro-specific invariants (`inv-loro-no-errors`,
//! `inv-loro-children-match-ref`) that cross-check the CRDT's fractional-index
//! sibling order against the reference.
//!
//! [`SutLoroLog`]: holon_pbt_core::capabilities::SutLoroLog
//! [`LoroBackendComponent`]: holon_loro_testing::LoroBackendComponent

pub mod builders;

#[cfg(test)]
mod integration_tests;

pub use builders::loro_wide;
// `LoroBackendComponent` co-located into the `holon-loro-testing` companion
// crate (Phase 1); re-exported so the historical slice path stays valid.
pub use holon_loro_testing::LoroBackendComponent;
