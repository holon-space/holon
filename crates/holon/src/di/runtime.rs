//! Runtime utilities for DI factories.

use fluxdi::Injector;
use holon_api::entity::{IntoEntity, TryFromEntity};

use crate::core::queryable_cache::QueryableCache;

use super::DbHandleProvider;

/// Creates a QueryableCache for a given type using the DbHandle from DI (async version).
///
/// Use this in `Provider::root_async` factories.
pub async fn create_queryable_cache_async<T>(injector: &Injector) -> QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    tracing::debug!(
        "[DI] create_queryable_cache_async<{}> called",
        std::any::type_name::<T>()
    );
    let provider = injector.resolve::<dyn DbHandleProvider>();
    let db_handle = provider.handle();

    QueryableCache::new(db_handle, T::type_definition())
        .await
        .expect("Failed to create QueryableCache")
}
