//! The SQL-storage composed PBT slice — a third slice over the **same** shared
//! [`crate::pbt::composed`] catalog, after `memory_slice` (`MemoryBackend`) and
//! `loro_slice` (`LoroBackend`). Its component is a real Turso `BackendEngine`
//! (the production storage + IVM matview layer) driven through the production
//! block operations, but with **none** of the reactive engine / frontend /
//! navigation / CDC-mirror machinery the monolithic `E2ESut` carries.
//!
//! @pbt kind slice-mod
//! @pbt covers sql-slice — component list + builders for the Turso storage arm.
//!
//! This is deliberately the storage-layer slice of the *future `E2ESut`
//! replacement* (the §F2 convergence): it reuses `E2ESut`'s own SQL realization
//! so that when the monolith is retired the composed components already cover
//! its storage surface.
//!
//! Beyond the block-tree caps it shares with the other slices, the
//! [`SqlProjectionComponent`] also provides [`SutSqlProjection`], unlocking the
//! SQL-projection invariants (e.g. `inv-block-content/sql`) that read the
//! `block_raw`/`block`/`block_tags` projections. The navigation/focus/watch
//! members of that cap are honest-empty here (no reactive engine drives them).
//!
//! [`SutSqlProjection`]: holon_pbt_core::capabilities::SutSqlProjection
//! [`SqlProjectionComponent`]: components::SqlProjectionComponent

pub mod builders;
pub mod components;

#[cfg(test)]
mod integration_tests;

pub use builders::sql_wide;
pub use components::SqlProjectionComponent;
