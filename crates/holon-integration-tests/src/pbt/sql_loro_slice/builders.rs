//! Compose the combined SQL+Loro slice's SUT `CapMap`. Unlike the single-store
//! slices this one registers two components, but deliberately *not* through two
//! `Config::with` calls: both provide `SutBackend` and the typemap keeps one
//! provider per cap, so a second `with` would silently shadow the first. Instead
//! SQL registers fully (its `register` provides `SutBackend` + `SutSqlProjection`)
//! and Loro contributes only `SutLoroTaskState` — see the slice module doc.

use std::sync::Arc;

use holon_pbt_core::capabilities::SutLoroTaskState;
use holon_pbt_core::composition::{CapMap, CapProvider};

use crate::pbt::loro_slice::components::LoroBackendComponent;
use crate::pbt::sql_slice::components::SqlProjectionComponent;

/// Build the combined SUT: SQL is the canonical block store, Loro the second
/// task_state oracle. The catalog selects every `SutBackend`/`SutSqlProjection`
/// block-tree and SQL-projection invariant over the SQL store, plus
/// `inv-task-state-storage-coherence` (the only invariant needing both
/// `SutSqlProjection` and `SutLoroTaskState`).
pub fn sql_loro_wide(sql: SqlProjectionComponent, loro: LoroBackendComponent) -> CapMap {
    let mut caps = CapMap::new();
    Arc::new(sql).register(&mut caps);
    caps.insert(Arc::new(loro) as Arc<dyn SutLoroTaskState>);
    caps
}
