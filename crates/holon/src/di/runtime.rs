//! Runtime utilities for DI factories.

use std::sync::Arc;

use async_trait::async_trait;
use fluxdi::Injector;
use holon_api::entity::{IntoEntity, TryFromEntity};
use holon_api::{DynamicEntity, TypeDefinition};
use holon_core::{CacheFactory, EntityCache};

use crate::core::queryable_cache::QueryableCache;
use crate::storage::DbHandle;

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

/// The Turso composition-root implementation of the storage-agnostic
/// [`CacheFactory`] seam (holon-core): mints `QueryableCache`s over the
/// shared [`DbHandle`]. Registered as `dyn CacheFactory` in
/// `di::registration` so provider crates resolve the seam, not this type.
pub struct DbHandleCacheFactory {
    db_handle: DbHandle,
}

impl DbHandleCacheFactory {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }
}

#[async_trait]
impl CacheFactory for DbHandleCacheFactory {
    async fn create_dynamic_cache(
        &self,
        type_def: TypeDefinition,
    ) -> holon_core::Result<Arc<dyn EntityCache<DynamicEntity>>> {
        let cache = QueryableCache::<DynamicEntity>::new(self.db_handle.clone(), type_def).await?;
        Ok(Arc::new(cache))
    }
}
