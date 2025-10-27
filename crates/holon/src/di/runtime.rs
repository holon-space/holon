//! Runtime utilities for DI factories.

use std::future::Future;

use fluxdi::Injector;

use crate::core::queryable_cache::QueryableCache;
use holon_api::entity::{IntoEntity, TryFromEntity};

use super::DbHandleProvider;

/// Runs an async operation in a synchronous context.
///
/// With fluxdi's native async factories (`Provider::root_async`), this is only
/// needed outside of DI — e.g. in tests or standalone helpers that must bridge
/// sync → async.
pub fn run_async_in_sync<F, T>(future: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        // Native — full multi-thread runtime supports block_in_place.
        #[cfg(not(target_arch = "wasm32"))]
        Ok(handle) => match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            tokio::runtime::RuntimeFlavor::CurrentThread => handle.block_on(future),
            _ => tokio::task::block_in_place(|| handle.block_on(future)),
        },
        // wasi-threads (holon-worker) — only current-thread is buildable.
        #[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
        Ok(handle) => handle.block_on(future),
        // wasm32-unknown (dioxus-web) — no tokio runtime at all.
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        Ok(_) => panic!("run_async_in_sync called from inside async context on WASM"),
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
                panic!("run_async_in_sync called outside async context on WASM/WASI");
            }
        }
    }
}

/// Creates a QueryableCache for a given type using the DbHandle from DI (sync version).
///
/// Prefer `create_queryable_cache_async` in `Provider::root_async` factories.
pub fn create_queryable_cache<T>(injector: &Injector) -> QueryableCache<T>
where
    T: IntoEntity + TryFromEntity + Send + Sync + 'static,
{
    tracing::debug!(
        "[DI] create_queryable_cache<{}> called",
        std::any::type_name::<T>()
    );
    let provider = injector.resolve::<dyn DbHandleProvider>();
    let db_handle = provider.handle();

    run_async_in_sync(async move {
        QueryableCache::new(db_handle, T::type_definition())
            .await
            .expect("Failed to create QueryableCache")
    })
}

/// Creates a QueryableCache for a given type using the DbHandle from DI (async version).
///
/// Use this in `Provider::root_async` factories to avoid `run_async_in_sync`.
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
