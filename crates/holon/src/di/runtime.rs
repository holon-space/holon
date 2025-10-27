//! Runtime utilities for bridging async/sync boundaries in DI factories.

use std::future::Future;

use ferrous_di::Resolver;

use crate::core::queryable_cache::QueryableCache;
use crate::core::traits::HasSchema;
use crate::storage::DbHandle;

use super::DbHandleProvider;

/// Runs an async operation in a synchronous DI factory context.
///
/// DI factories are synchronous, but many services require async initialization.
/// This helper tries to stay on the current runtime (to communicate with actors
/// spawned there) but falls back to a new thread if no runtime is available.
///
/// Strategy:
/// 1. If inside a multi-threaded tokio runtime: use block_in_place + Handle::current()
/// 2. If inside a current-thread runtime: use Handle::current() directly (may block)
/// 3. If no runtime: spawn a new thread with its own runtime
///
/// IMPORTANT for Flutter: When called during DI resolution within an async context
/// (like FrontendSession::new), we're already on FRB's runtime where the actor lives,
/// so we MUST stay on that runtime to communicate with the actor.
pub fn run_async_in_sync_factory<F, T>(future: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            tokio::runtime::RuntimeFlavor::CurrentThread => handle.block_on(future),
            _ => tokio::task::block_in_place(|| handle.block_on(future)),
        },
        Err(_) => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                std::thread::spawn(move || {
                    let rt =
                        tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                    rt.block_on(future)
                })
                .join()
                .expect("Thread panicked during async initialization")
            }

            #[cfg(target_arch = "wasm32")]
            {
                panic!("run_async_in_sync_factory called outside async context on WASM");
            }
        }
    }
}

/// Creates a QueryableCache for a given type using the DbHandle from DI.
///
/// This function uses trait-based resolution (`DbHandleProvider`) to avoid
/// TypeId mismatches when called from different compilation units.
pub fn create_queryable_cache<T, R: Resolver>(resolver: &R) -> QueryableCache<T>
where
    T: HasSchema + Send + Sync + 'static,
{
    eprintln!(
        "[DI] create_queryable_cache<{}> called",
        std::any::type_name::<T>()
    );
    let provider = resolver.get_required_trait::<dyn DbHandleProvider>();
    let db_handle = provider.handle();

    create_queryable_cache_with_db_handle(db_handle)
}

/// Creates a QueryableCache for a given type using a pre-resolved DbHandle.
pub fn create_queryable_cache_with_db_handle<T>(db_handle: DbHandle) -> QueryableCache<T>
where
    T: HasSchema + Send + Sync + 'static,
{
    eprintln!(
        "[DI] create_queryable_cache_with_db_handle<{}> called",
        std::any::type_name::<T>()
    );

    run_async_in_sync_factory(async move {
        QueryableCache::for_entity(db_handle)
            .await
            .expect("Failed to create QueryableCache")
    })
}
