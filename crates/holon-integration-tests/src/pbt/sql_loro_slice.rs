//! The combined **SQL + Loro** composed PBT slice — the only non-redundant
//! consumer of [`SutLoroTaskState`]. It composes the existing
//! [`SqlProjectionComponent`] (a real Turso `BackendEngine`) and
//! [`LoroBackendComponent`] (a real `LoroBackend` CRDT) into one SUT so that
//! `inv-task-state-storage-coherence` can cross-check a block's `task_state`
//! across **two independent storage realizations** — the SQL
//! `json_extract(properties,'$.task_state')` projection vs the Loro
//! `properties["task_state"]` projection.
//!
//! Composition choice (honesty): both components provide `SutBackend`, and
//! [`CapMap`] is a typemap (one provider per cap) — registering both fully
//! would *silently* drop one block store. So this slice registers **SQL as the
//! canonical block store** (`SutBackend` + `SutSqlProjection`) and **Loro for
//! only its task_state projection** (`SutLoroTaskState`). The block-tree
//! invariants then run unambiguously over the SQL store (Loro's structural
//! correctness is already covered by `loro_slice`); Loro is purely the second
//! task_state oracle the coherence invariant compares against.
//!
//! [`SutLoroTaskState`]: holon_pbt_core::capabilities::SutLoroTaskState
//! [`CapMap`]: holon_pbt_core::composition::CapMap
//! [`SqlProjectionComponent`]: crate::pbt::sql_slice::components::SqlProjectionComponent
//! [`LoroBackendComponent`]: holon_loro_testing::LoroBackendComponent

pub mod builders;

#[cfg(test)]
mod integration_tests;

pub use builders::sql_loro_wide;
