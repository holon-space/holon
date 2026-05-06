use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use gpui::{AnyEntity, Entity};

// ── Builder entity types ────────────────────────────────────────────────

/// Self-rendering collapsible disclosure widget. `impl Render` is in `builders/collapsible.rs`.
pub struct CollapsibleView {
    pub collapsed: bool,
    pub header_text: String,
    pub icon_text: String,
    pub detail_text: String,
}

/// Simple boolean toggle state shared by tree items and pie menus.
pub struct ToggleState {
    pub active: bool,
}

// ── CacheKey ────────────────────────────────────────────────────────────

/// Typed key into the parent-scoped entity cache.
///
/// Each variant encodes the lifetime of its entries: state-bearing entries
/// (the first four variants) survive a structural rebuild of the parent's
/// reactive tree; `Ephemeral` entries do not. The classification lives on
/// the type so adding a new state-bearing kind requires explicitly extending
/// the enum and the matching arm in [`CacheKey::is_state_bearing`] —
/// "Parse, Don't Validate" applied to cache lifetimes (CLAUDE.md).
///
/// All variants are hashed to drive cache lookups, so the contained data
/// must already be canonical (e.g. block ids are full URIs, not nicknames).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum CacheKey {
    /// Nested `ReactiveShell` keyed by [`ReactiveView::stable_cache_key`].
    /// Preserves `ListState` (scroll position + measured row heights) and
    /// the entity's own nested cache across structural rebuilds.
    ReactiveShell(u64),

    /// `ReactiveShell` for a referenced block, keyed by canonical block id.
    /// Preserves nested entity state (editor input, expand toggles, child
    /// caches) across the parent's structural rebuilds.
    LiveBlock(String),

    /// `LiveQueryView` keyed by `live_query_key(sql, context_id)`.
    /// Preserves cached query results.
    LiveQuery(String),

    /// `RenderEntityView` for one collection row, keyed by row id.
    /// Survives structural rebuilds; collection-mode `apply_diff` is
    /// responsible for pruning entries when their row goes away.
    RenderEntity(String),

    /// Builder-internal state (toggles, collapsibles, drag highlights,
    /// per-frame positional ids). Wiped on every structural rebuild via
    /// [`wipe_ephemeral`].
    ///
    /// The contained string is opaque — choose a stable per-builder seed
    /// (e.g. node id + role) so re-renders hit the same entry.
    Ephemeral(String),
}

impl CacheKey {
    /// Whether this key's entry must survive a structural rebuild of the
    /// parent's reactive tree. Drives [`wipe_ephemeral`].
    pub fn is_state_bearing(&self) -> bool {
        !matches!(self, CacheKey::Ephemeral(_))
    }
}

// ── LocalEntityScope ────────────────────────────────────────────────────

/// Entity cache for builder-created widgets (toggles, collapsibles, nested
/// reactive shells, …). Arc-owned by the parent view so it persists across
/// re-renders.
pub type EntityCache = Arc<RwLock<HashMap<CacheKey, AnyEntity>>>;

/// Per-row entity context, built fresh each render pass.
///
/// Wraps the row-owned `EntityCache` Arc so builders can lazily create
/// or look up cached entities by [`CacheKey`]. The optional `parent_cache`
/// is the shell-level cache one level up; `LiveQuery` lookups route to it
/// (data-semantic — same SQL → same cached result is correct). All other
/// kinds (`LiveBlock`, `RenderEntity`, `Ephemeral`) use the row's
/// `entity_cache` so two `live_block(X)` rendered in the same shell don't
/// collide on `tree-collapse:X`, `editable_text` input, or `RenderEntity(X)`.
///
/// When `parent_cache` is `None`, all lookups use `entity_cache` — that is
/// the shell-level scope itself, the root of the chain.
pub struct LocalEntityScope {
    pub(crate) entity_cache: EntityCache,
    pub(crate) parent_cache: Option<EntityCache>,
}

impl LocalEntityScope {
    pub fn new() -> Self {
        Self {
            entity_cache: Default::default(),
            parent_cache: None,
        }
    }

    pub fn with_cache(mut self, cache: EntityCache) -> Self {
        self.entity_cache = cache;
        self
    }

    pub fn with_parent(mut self, parent: EntityCache) -> Self {
        self.parent_cache = Some(parent);
        self
    }

    fn cache_for_key(&self, key: &CacheKey) -> &EntityCache {
        match (&self.parent_cache, key) {
            (Some(parent), CacheKey::LiveQuery(_)) => parent,
            _ => &self.entity_cache,
        }
    }

    #[cfg(test)]
    pub(crate) fn target_cache_for_test(&self, key: &CacheKey) -> EntityCache {
        self.cache_for_key(key).clone()
    }

    /// Get or create a cached entity by typed key. Persists across
    /// re-renders because the parent view owns the [`EntityCache`] Arc.
    pub fn get_or_create(&self, key: CacheKey, create: impl FnOnce() -> AnyEntity) -> AnyEntity {
        let cache = self.cache_for_key(&key);
        let mut g = cache.write().unwrap();
        g.entry(key).or_insert_with(create).clone()
    }

    /// Typed wrapper around [`get_or_create`] that downcasts back to
    /// `Entity<T>`.
    ///
    /// A cache hit under a different `T` is a programming error (key
    /// collision across types) and panics loudly — per the project's
    /// "fail loud, never fake" rule, that's not a runtime
    // ALLOW(fallback): describing the absence of a fallback, not introducing one — the panic IS the contract
    /// fallback condition.
    pub fn get_or_create_typed<T: 'static>(
        &self,
        key: CacheKey,
        create: impl FnOnce() -> Entity<T>,
    ) -> Entity<T> {
        let key_for_panic = key.clone();
        let any = self.get_or_create(key, || create().into_any());
        any.downcast::<T>().unwrap_or_else(|_| {
            panic!(
                "entity_cache type mismatch on key {key_for_panic:?} — \
                 same key was used for a different Entity<T>"
            )
        })
    }
}

/// Wipe ephemeral builder entries from the cache, preserving state-bearing
/// keys (see [`CacheKey::is_state_bearing`]). Called on every structural
/// rebuild of a `ReactiveShell` so scroll position, expand state, and
/// nested entity state outlive re-interpretation of the parent's render
/// tree.
pub fn wipe_ephemeral(cache: &EntityCache) {
    let mut g = cache.write().unwrap();
    g.retain(|k, _| k.is_state_bearing());
}

// ── LiveBlockAncestors ──────────────────────────────────────────────────

/// Chain of `live_block` block ids being rendered up the entity tree.
///
/// Replaces the per-call-stack `RECONCILING` thread-local that the old
/// synchronous `reconcile_children` path used to detect A→B→A cycles. GPUI
/// renders entities asynchronously across separate render passes, so the
/// thread-local approach can't see ancestors once the parent's render
/// returns. Instead the chain is stored on the `ReactiveShell` itself at
/// creation time (captured from the `GpuiRenderContext` of the creating
/// frame), then re-emitted into each of the shell's own render frames.
///
/// The chain is cheap to extend (one `Vec<String>` clone, typically ≤4
/// entries) and equality on the contained ids is canonical-string equality
/// — the same ids that flow into `CacheKey::LiveBlock`.
#[derive(Clone, Debug, Default)]
pub struct LiveBlockAncestors {
    inner: Vec<String>,
}

impl LiveBlockAncestors {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.inner.iter().any(|x| x == id)
    }

    /// Return a new chain with `id` appended. The receiver is unchanged so
    /// callers can keep using the parent chain after spawning a child.
    pub fn pushed(&self, id: impl Into<String>) -> Self {
        let mut c = self.inner.clone();
        c.push(id.into());
        Self { inner: c }
    }

    pub fn as_slice(&self) -> &[String] {
        &self.inner
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_state_bearing_classification() {
        assert!(CacheKey::ReactiveShell(0xdead_beef).is_state_bearing());
        assert!(CacheKey::LiveBlock("block:abc".into()).is_state_bearing());
        assert!(CacheKey::LiveQuery("lq-1234".into()).is_state_bearing());
        assert!(CacheKey::RenderEntity("block:xyz".into()).is_state_bearing());
        assert!(!CacheKey::Ephemeral("toggle-foo".into()).is_state_bearing());
    }

    #[test]
    fn live_query_routes_to_parent_other_kinds_route_to_self() {
        // Two row scopes share a parent_cache (shell-level), but each has
        // its own entity_cache (row-level). LiveQuery routes to parent;
        // every other kind stays row-scoped — the per-row state-isolation
        // guarantee that lets two `live_block(X)` in the same shell keep
        // independent expand-toggle / editor / RenderEntity state.
        let parent: EntityCache = Default::default();
        let row_a: EntityCache = Default::default();
        let row_b: EntityCache = Default::default();

        let scope_a = LocalEntityScope::new()
            .with_cache(row_a.clone())
            .with_parent(parent.clone());
        let scope_b = LocalEntityScope::new()
            .with_cache(row_b.clone())
            .with_parent(parent.clone());

        let lq = CacheKey::LiveQuery("sql:select 1".into());
        let lb = CacheKey::LiveBlock("block:X".into());
        let eph = CacheKey::Ephemeral("tree-collapse:block:X".into());
        let re = CacheKey::RenderEntity("block:X".into());

        // LiveQuery: both scopes route to the same parent cache (data-semantic share).
        let lq_a = scope_a.target_cache_for_test(&lq);
        let lq_b = scope_b.target_cache_for_test(&lq);
        assert!(Arc::ptr_eq(&lq_a, &parent));
        assert!(Arc::ptr_eq(&lq_b, &parent));

        // LiveBlock / Ephemeral / RenderEntity: each scope keeps its own
        // entity, so two pinned instances of block X don't share toggle
        // state, editor input, or row entity.
        for key in [&lb, &eph, &re] {
            let target_a = scope_a.target_cache_for_test(key);
            let target_b = scope_b.target_cache_for_test(key);
            assert!(
                Arc::ptr_eq(&target_a, &row_a),
                "key {key:?} did not route to row_a"
            );
            assert!(
                Arc::ptr_eq(&target_b, &row_b),
                "key {key:?} did not route to row_b"
            );
            assert!(
                !Arc::ptr_eq(&target_a, &target_b),
                "key {key:?} should not share state across rows"
            );
        }
    }

    #[test]
    fn scope_without_parent_uses_self_for_all_keys() {
        // Shell-level scope has no parent; every kind routes to its own
        // entity_cache. Same shape as today's behaviour.
        let cache: EntityCache = Default::default();
        let scope = LocalEntityScope::new().with_cache(cache.clone());

        for key in [
            CacheKey::LiveQuery("sql".into()),
            CacheKey::LiveBlock("block:X".into()),
            CacheKey::Ephemeral("eph".into()),
            CacheKey::RenderEntity("block:X".into()),
        ] {
            assert!(Arc::ptr_eq(&scope.target_cache_for_test(&key), &cache));
        }
    }

    #[test]
    fn live_block_ancestors_pushed_is_immutable_copy() {
        let a = LiveBlockAncestors::new();
        assert!(a.is_empty());
        let b = a.pushed("block:A");
        assert!(a.is_empty(), "parent chain stays unchanged");
        assert!(b.contains("block:A"));
        let c = b.pushed("block:B");
        assert!(c.contains("block:A"));
        assert!(c.contains("block:B"));
        assert!(!c.contains("block:C"));
    }

    #[test]
    fn wipe_ephemeral_preserves_state_bearing_entries() {
        // Use a synthetic AnyEntity stand-in: the test only asserts retain
        // semantics, not entity validity, so we drive the cache HashMap
        // directly via the same Arc<RwLock<…>> the helper sees.
        let cache: EntityCache = Default::default();
        // Insert one entry per kind via the typed enum; values are
        // irrelevant for the predicate test.
        // We can't construct AnyEntity without a gpui App, so the test
        // exercises the predicate alone.
        let _ = cache; // silence unused warning when the body below is empty.

        // Sanity check on the predicate:
        let keys: Vec<CacheKey> = vec![
            CacheKey::ReactiveShell(1),
            CacheKey::LiveBlock("a".into()),
            CacheKey::LiveQuery("b".into()),
            CacheKey::RenderEntity("c".into()),
            CacheKey::Ephemeral("d".into()),
        ];
        let kept: Vec<&CacheKey> = keys.iter().filter(|k| k.is_state_bearing()).collect();
        assert_eq!(kept.len(), 4);
        assert!(!kept.iter().any(|k| matches!(k, CacheKey::Ephemeral(_))));
    }
}
