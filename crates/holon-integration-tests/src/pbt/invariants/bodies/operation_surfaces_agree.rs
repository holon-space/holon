//! The comparison every operation-surface invariant shares: the dispatcher
//! catalog carries the required operations, and the profile resolver and the
//! last re-render offer exactly the catalog.

use std::collections::BTreeSet;
use std::time::Duration;
use std::time::Instant;

use holon_pbt_core::capabilities::OperationSurfaces;

/// Poll `read` until the surfaces agree. The profile resolver learns of a
/// registration asynchronously, so a disagreement counts only once it outlives
/// the deadline; the last surfaces read are the error.
pub async fn operation_surfaces_agree(
    required: &BTreeSet<String>,
    mut read: impl AsyncFnMut() -> OperationSurfaces,
) -> Result<(), OperationSurfaces> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let surfaces = read().await;
        if surfaces.dispatcher.is_superset(required)
            && surfaces.profile == surfaces.dispatcher
            && surfaces
                .rerendered
                .as_ref()
                .is_none_or(|rerendered| rerendered == &surfaces.dispatcher)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(surfaces);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
