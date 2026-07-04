//! `EntityCellRegistry` — per-entity-type lookup of [`Cell<T>`] handles.
//!
//! Each entity type (block, todoist-task, jira-issue, …) wires its own
//! registry. The registry decides how to construct the backing for a
//! given `(EntityUri, field)` (Loro container, `set_field`, external API,
//! …) and caches the resulting cell with `Weak`-keyed eviction so
//! consumer-released cells fall out automatically.
//!
//! Phase 1 only ships [`crate::cell::Cell<String>`] for `block.content`.
//! The trait surface is generic so Phase 2 can add scalar/meta backings
//! without breaking the API.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use holon_api::{BlockContent, EntityUri, Tags};

use crate::cell::{Cell, CellBacking};

/// Registry surface used by chord ops, frontend handlers, and sync
/// providers. Implementations live per entity type
/// (e.g. `BlockCellRegistry`).
#[async_trait]
pub trait EntityCellRegistry: Send + Sync {
    /// Type-erased cell lookup. Returns the cached cell if alive, or
    /// constructs a fresh one if expired/missing. The returned
    /// `Arc<dyn Any + Send + Sync>` actually holds an [`Cell<T>`] for the
    /// requested `T`; callers should use the [`EntityCellRegistryExt`]
    /// extension method [`live_field`](EntityCellRegistryExt::live_field)
    /// rather than calling this directly.
    fn live_field_any(
        &self,
        uri: &EntityUri,
        field: &str,
        type_id: TypeId,
    ) -> Result<Arc<dyn Any + Send + Sync>>;

    /// Drop cached cells for `uri`. Called by chord ops on `delete` /
    /// `join_block`-deletes-id to prevent a same-id re-create from
    /// observing a stale cell wrapping an orphaned backing.
    fn on_entity_deleted(&self, uri: &EntityUri);

    /// Typed positional write: place `uri` under `parent_id`, immediately
    /// after `after_id` (or first when `None`). Replaces the legacy
    /// `set_field("sort_key", gen_key_between(...))` round-trip — callers
    /// pass a typed predecessor block id directly and the registry routes
    /// to the backend's positioned-move primitive.
    ///
    /// Returns `Ok(true)` when handled (the registry executed the move).
    /// Returns `Ok(false)` when this registry can't satisfy the move (no
    /// backing tree, SqlOnly mode, etc.) — the caller is expected to
    /// fall back to the legacy compute + `set_field("sort_key")` path.
    ///
    /// Default impl returns `Ok(false)` so registries that don't model
    /// positional intent (most non-block entity types) opt out for free.
    async fn write_position(&self, _: &EntityUri, _: &str, _: Option<&str>) -> Result<bool> {
        Ok(false)
    }

    /// Create a new entity authoritatively through this registry's backing
    /// store (e.g. `LoroBackend::create_block`). The backend then emits an
    /// outbound `Loro`-origin SQL INSERT via the sync controller, so the
    /// inbound CDC gate sees `EventOrigin::Loro` and applies `EchoSuppress`
    /// instead of dropping the event as an unmigrated SQL-direct write.
    ///
    /// Returns `Ok(true)` when the create landed via the cell route;
    /// `Ok(false)` when this registry has no cell-routed create path
    /// (SqlOnly mode, synthetic test stores, non-block entity types).
    /// Callers fall back to `BlockOperations::create` on `Ok(false)`.
    ///
    /// Default impl returns `Ok(false)` so registries that don't support
    /// authoritative creates opt out for free.
    async fn create_entity(
        &self,
        _: &EntityUri,
        _: Option<&EntityUri>,
        _: &EntityUri,
        _: BlockContent,
        _: &std::collections::HashMap<String, holon_api::Value>,
        _: &Tags,
        _: &[EntityUri],
        _: &[EntityUri],
    ) -> Result<bool> {
        Ok(false)
    }

    /// Delete an entity authoritatively through this registry's backing
    /// store (e.g. `LoroBackend::delete_block`); the outbound projector then
    /// emits the SQL DELETE tagged `EventOrigin::Loro`. Mirrors
    /// [`create_entity`](Self::create_entity): deleting only from SQL leaves
    /// the still-present Loro node alive (observed: `join_block` merged the
    /// content via cells but the SQL-direct delete left the joined-away
    /// block in the Loro tree).
    ///
    /// Returns `Ok(true)` when the delete landed via the cell route;
    /// `Ok(false)` when this registry has no cell-routed delete path
    /// (SqlOnly mode, synthetic test stores). Callers fall back to the
    /// SQL delete on `Ok(false)`.
    async fn delete_entity(&self, _: &EntityUri) -> Result<bool> {
        Ok(false)
    }
}

/// Extension trait providing the typed `live_field<T>` surface on top of
/// [`EntityCellRegistry`]. Adds an `EntityCellRegistry`-shaped lookup with
/// compile-time `T` and a runtime type check via the `TypeId` round-trip.
pub trait EntityCellRegistryExt {
    fn live_field<T: 'static + Send + Sync + Clone>(
        &self,
        uri: &EntityUri,
        field: &str,
    ) -> Result<Cell<T>>;
}

impl EntityCellRegistryExt for dyn EntityCellRegistry + '_ {
    fn live_field<T: 'static + Send + Sync + Clone>(
        &self,
        uri: &EntityUri,
        field: &str,
    ) -> Result<Cell<T>> {
        let any_arc = self.live_field_any(uri, field, TypeId::of::<T>())?;
        // The registry stored an `Arc<Cell<T>>` for the requested `T`;
        // downcast to recover. If the registry returned a different
        // concrete type for this (uri, field, TypeId), that's a
        // registry-implementation bug and we surface it loudly.
        any_arc
            .downcast::<Cell<T>>()
            .map(|arc_cell| (*arc_cell).clone())
            .map_err(|_| {
                anyhow!(
                    "EntityCellRegistry returned a cell of an unexpected type for \
                     ({uri}, {field}). Expected Cell<{}>; this is a registry-impl bug.",
                    std::any::type_name::<T>()
                )
            })
    }
}

// ── Generic Weak-keyed cache helper ─────────────────────────────────────

/// Slot stored in the cache. Holds a `Weak<dyn CellBacking<T>>` for some
/// concrete `T`; we keep `T` opaque via `Box<dyn AnyCellSlot>` so the cache
/// is heterogeneous over `T`.
trait AnyCellSlot: Send + Sync {
    fn is_alive(&self) -> bool;
    fn upgrade_as_any(&self) -> Option<Arc<dyn Any + Send + Sync>>;
}

struct CellSlot<T: 'static + Send + Sync + Clone> {
    backing: Weak<dyn CellBacking<T>>,
}

impl<T: 'static + Send + Sync + Clone> AnyCellSlot for CellSlot<T> {
    fn is_alive(&self) -> bool {
        self.backing.strong_count() > 0
    }

    fn upgrade_as_any(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        let backing = self.backing.upgrade()?;
        let cell = Cell::from_backing(backing);
        Some(Arc::new(cell) as Arc<dyn Any + Send + Sync>)
    }
}

/// Reusable Weak-keyed cell cache. Registries embed one of these and
/// supply a per-`(field, T)` constructor; the cache handles upgrade,
/// eviction, and same-id pruning.
type CellCacheKey = (EntityUri, String, TypeId);
type CellCacheMap = HashMap<CellCacheKey, Box<dyn AnyCellSlot>>;

pub struct CellCache {
    inner: Mutex<CellCacheMap>,
}

impl Default for CellCache {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl CellCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up an existing live cell for `(uri, field, T)`, or construct
    /// one via `make_backing` and cache its `Weak` reference. The returned
    /// `Arc<dyn Any>` wraps a `Cell<T>` (so [`EntityCellRegistryExt::live_field`]
    /// can downcast).
    pub fn get_or_construct<T, F>(
        &self,
        uri: &EntityUri,
        field: &str,
        make_backing: F,
    ) -> Result<Arc<dyn Any + Send + Sync>>
    where
        T: 'static + Send + Sync + Clone,
        F: FnOnce() -> Result<Arc<dyn CellBacking<T>>>,
    {
        let key = (uri.clone(), field.to_string(), TypeId::of::<T>());
        {
            let map = self.inner.lock().unwrap();
            if let Some(slot) = map.get(&key) {
                if let Some(arc_any) = slot.upgrade_as_any() {
                    return Ok(arc_any);
                }
            }
        }
        // Either no entry, or its Weak expired. Construct fresh.
        let backing = make_backing()?;
        let weak = Arc::downgrade(&backing);
        let cell = Cell::from_backing(backing);
        let arc_any: Arc<dyn Any + Send + Sync> = Arc::new(cell);
        let slot: Box<dyn AnyCellSlot> = Box::new(CellSlot::<T> { backing: weak });
        let mut map = self.inner.lock().unwrap();
        map.insert(key, slot);
        Ok(arc_any)
    }

    /// Drop every cached entry for `uri`. Called by registries from their
    /// `on_entity_deleted` impl. Returns the number of entries pruned.
    pub fn evict_uri(&self, uri: &EntityUri) -> usize {
        let mut map = self.inner.lock().unwrap();
        let before = map.len();
        map.retain(|(k_uri, _, _), _| k_uri != uri);
        before - map.len()
    }

    /// Drop every cached entry whose backing has been released by all
    /// consumers. Useful for periodic maintenance; not required for
    /// correctness (lookups already check `is_alive`).
    pub fn evict_dead(&self) -> usize {
        let mut map = self.inner.lock().unwrap();
        let before = map.len();
        map.retain(|_, slot| slot.is_alive());
        before - map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::LwwTextCellBacking;
    use futures::future::BoxFuture;
    use futures::stream::BoxStream;

    fn dummy_text_backing() -> Arc<dyn CellBacking<String>> {
        Arc::new(LwwTextCellBacking::new(
            Arc::new(String::new),
            Arc::new(|_| -> BoxFuture<'static, Result<()>> { Box::pin(async { Ok(()) }) }),
            Arc::new(|| -> BoxStream<'static, String> { Box::pin(futures::stream::empty()) }),
        ))
    }

    #[test]
    fn cache_returns_same_cell_when_alive() {
        let cache = CellCache::new();
        let uri = EntityUri::block("a");
        let arc1 = cache
            .get_or_construct::<String, _>(&uri, "content", || Ok(dummy_text_backing()))
            .unwrap();
        let arc2 = cache
            .get_or_construct::<String, _>(&uri, "content", || {
                panic!("constructor should not be called when cache is warm")
            })
            .unwrap();
        // Both Arcs point to a Cell<String>; downcast and verify they're
        // the same backing (same Arc::ptr_eq via the inner Arc).
        let cell1 = arc1.downcast::<Cell<String>>().unwrap();
        let cell2 = arc2.downcast::<Cell<String>>().unwrap();
        assert!(Arc::ptr_eq(cell1.inner_arc(), cell2.inner_arc()));
    }

    #[test]
    fn cache_constructs_fresh_when_weak_expired() {
        let cache = CellCache::new();
        let uri = EntityUri::block("b");
        // First insert and immediately drop the cell
        let _ = cache
            .get_or_construct::<String, _>(&uri, "content", || Ok(dummy_text_backing()))
            .unwrap();
        // The Arc<Cell<String>> from above was dropped at the end of the
        // statement (no binding kept it alive), so the inner backing's
        // strong count is 0 now; the slot's Weak should fail upgrade.
        let mut constructed_again = false;
        let _arc = cache
            .get_or_construct::<String, _>(&uri, "content", || {
                constructed_again = true;
                Ok(dummy_text_backing())
            })
            .unwrap();
        assert!(constructed_again);
    }

    #[test]
    fn evict_uri_drops_entries_for_that_uri() {
        let cache = CellCache::new();
        let uri_a = EntityUri::block("c");
        let uri_b = EntityUri::block("d");
        let _kept_a = cache
            .get_or_construct::<String, _>(&uri_a, "content", || Ok(dummy_text_backing()))
            .unwrap();
        let _kept_b = cache
            .get_or_construct::<String, _>(&uri_b, "content", || Ok(dummy_text_backing()))
            .unwrap();
        let pruned = cache.evict_uri(&uri_a);
        assert_eq!(pruned, 1);
        // After eviction, looking up uri_a triggers a fresh construct;
        // uri_b's existing entry is still cached.
        let mut a_constructed = false;
        let _ = cache
            .get_or_construct::<String, _>(&uri_a, "content", || {
                a_constructed = true;
                Ok(dummy_text_backing())
            })
            .unwrap();
        assert!(a_constructed);
        let mut b_constructed = false;
        let _ = cache
            .get_or_construct::<String, _>(&uri_b, "content", || {
                b_constructed = true;
                Ok(dummy_text_backing())
            })
            .unwrap();
        assert!(!b_constructed);
    }

    #[test]
    fn evict_dead_removes_only_dead_slots() {
        let cache = CellCache::new();
        let live_uri = EntityUri::block("live");
        let dead_uri = EntityUri::block("dead");
        let kept = cache
            .get_or_construct::<String, _>(&live_uri, "content", || Ok(dummy_text_backing()))
            .unwrap();
        // Construct and immediately drop: this slot's backing dies.
        let _ = cache
            .get_or_construct::<String, _>(&dead_uri, "content", || Ok(dummy_text_backing()))
            .unwrap();

        assert_eq!(cache.evict_dead(), 1, "exactly the dead slot is pruned");
        assert_eq!(cache.evict_dead(), 0, "second pass has nothing to prune");

        // The live slot survives eviction: a warm lookup must not construct.
        let again = cache
            .get_or_construct::<String, _>(&live_uri, "content", || {
                panic!("live slot must survive evict_dead")
            })
            .unwrap();
        let cell_kept = kept.downcast::<Cell<String>>().unwrap();
        let cell_again = again.downcast::<Cell<String>>().unwrap();
        assert!(Arc::ptr_eq(cell_kept.inner_arc(), cell_again.inner_arc()));
    }

    /// Registry that implements only the required surface, to pin the
    /// default `Ok(false)` ("not handled — caller must use its SQL path")
    /// contracts of the optional write seams.
    struct MinimalRegistry;

    #[async_trait::async_trait]
    impl EntityCellRegistry for MinimalRegistry {
        fn live_field_any(
            &self,
            _uri: &EntityUri,
            _field: &str,
            _type_id: TypeId,
        ) -> Result<Arc<dyn Any + Send + Sync>> {
            unimplemented!("not exercised")
        }
        fn on_entity_deleted(&self, _uri: &EntityUri) {}
    }

    #[tokio::test]
    async fn optional_write_seams_default_to_not_handled() {
        let registry = MinimalRegistry;
        let uri = EntityUri::block("a");
        let parent = EntityUri::block("p");

        assert!(!registry.write_position(&uri, "p", None).await.unwrap());
        assert!(!registry
            .create_entity(
                &parent,
                None,
                &uri,
                holon_api::BlockContent::text("x"),
                &std::collections::HashMap::new(),
                &holon_api::Tags::default(),
                &[],
                &[],
            )
            .await
            .unwrap());
        assert!(!registry.delete_entity(&uri).await.unwrap());
    }
}
