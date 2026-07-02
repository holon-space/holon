//! Storage-agnostic cache seam: [`EntityCache`] + [`CacheFactory`].
//!
//! Provider crates (holon-mcp-client, …) consume per-entity caches without
//! naming the concrete backend (`QueryableCache` / Turso `DbHandle`). The
//! composition root implements [`CacheFactory`] over the real storage and
//! registers it in DI; consumers resolve `dyn CacheFactory` and hold
//! `Arc<dyn EntityCache<_>>`.
//!
//! Surface is deliberately minimal — exactly what today's consumers call
//! (`get_all` / `get_by_id` via the [`DataSource`] supertrait, plus
//! `get_all_ids` and `apply_batch`). Grow it only when a consumer needs more.

use async_trait::async_trait;
use std::sync::Arc;

use crate::traits::{DataSource, MaybeSendSync, Result};
use holon_api::{Change, DynamicEntity, EntityUri, SyncTokenUpdate, TypeDefinition};

/// A per-entity cache: readable as a [`DataSource`], plus the write-side
/// batch ingestion used by external-sync engines.
#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
pub trait EntityCache<T>: DataSource<T>
where
    T: Clone + MaybeSendSync + 'static,
{
    /// All primary-key values, without deserializing full rows.
    async fn get_all_ids(&self) -> Result<Vec<EntityUri>>;

    /// Apply a batch of changes transactionally, optionally persisting the
    /// provider's sync token in the same transaction.
    async fn apply_batch(
        &self,
        changes: &[Change<T>],
        sync_token: Option<&SyncTokenUpdate>,
    ) -> Result<()>;

    /// Insert-or-replace a single item (keyed by its primary-key field).
    async fn upsert(&self, item: &T) -> Result<()>;

    /// Delete a single item by primary key.
    async fn delete(&self, id: &str) -> Result<()>;
}

/// Mints [`EntityCache`]s for runtime-described entity types.
///
/// The dynamic (schema-at-runtime) shape covers MCP sidecar entities; typed
/// consumers pass `T::type_definition()` and convert at the boundary.
#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
pub trait CacheFactory: Send + Sync {
    /// Create (or open) the cache backing `type_def`, running any schema DDL
    /// the backend requires.
    async fn create_dynamic_cache(
        &self,
        type_def: TypeDefinition,
    ) -> Result<Arc<dyn EntityCache<DynamicEntity>>>;
}
