//! Per-tick memoisation wrapper for SUT capability reads.
//!
//! Multiple invariants evaluated in the same tick often need the same
//! SUT snapshot. `drain_vm_emissions` is particularly tricky: it has
//! drain-once semantics — calling it twice returns an empty Vec the
//! second time. The proxy solves this by draining EAGERLY at construction
//! and serving the cached Vec on every subsequent call.
//!
//! ## Lifetime + tick boundary
//!
//! Build with [`cached`] (read-only, no eager drain) or [`cached_with_drain`]
//! (drains VM emissions up front; needs `&mut S`). Each call starts a
//! fresh tick; cache state never bleeds across ticks.
//!
//! ## `SutCdc::drain_cdc` is intentionally NOT forwarded
//!
//! `drain_cdc` flushes the CDC pipeline — a persistent SUT side effect
//! that belongs to the transition executor, not invariant evaluation.
//! Flush before constructing the proxy:
//!
//! ```text
//! sut.drain_cdc().await;          // flush
//! let proxy = cached(&sut);       // snapshot
//! invariants.check(&ref_, &proxy).await;
//! ```

use std::cell::RefCell;
use std::collections::BTreeSet;

use crate::capabilities::{CapBlockId, SutCdc, SutSqlProjection, SutViewModel};

/// A per-tick caching view over a SUT.
///
/// Construct with [`cached`] (no eager drain) or [`cached_with_drain`]
/// (drains VM emissions eagerly so subsequent `drain_vm_emissions` calls
/// serve the cache).
pub struct CachingProxy<'a, S> {
    inner: &'a S,
    /// Pre-drained VM emissions. `None` = the proxy was built without
    /// eager drain, and `drain_vm_emissions` will return an empty Vec
    /// (the caller already drained, or there's nothing to drain).
    vm_emissions_cache: Option<Vec<String>>,
    cdc_in_flight_cache: RefCell<Option<bool>>,
    all_block_ids_cache: RefCell<Option<BTreeSet<CapBlockId>>>,
}

impl<'a, S> CachingProxy<'a, S> {
    /// Access the underlying SUT for methods the proxy does not wrap.
    pub fn inner(&self) -> &S {
        self.inner
    }
}

/// Build a fresh read-only `CachingProxy`. VM emissions are NOT drained;
/// `drain_vm_emissions` will return an empty Vec. Use when the slice has
/// no ViewModel-touching invariants, or when the caller drained earlier.
pub fn cached<S>(sut: &S) -> CachingProxy<'_, S> {
    CachingProxy {
        inner: sut,
        vm_emissions_cache: None,
        cdc_in_flight_cache: RefCell::new(None),
        all_block_ids_cache: RefCell::new(None),
    }
}

/// Build a `CachingProxy` after eagerly draining VM emissions from `sut`.
/// Subsequent `drain_vm_emissions` calls serve the cached Vec.
pub async fn cached_with_drain<S: SutViewModel>(sut: &mut S) -> CachingProxy<'_, S> {
    let emissions = sut.drain_vm_emissions().await;
    CachingProxy {
        inner: sut,
        vm_emissions_cache: Some(emissions),
        cdc_in_flight_cache: RefCell::new(None),
        all_block_ids_cache: RefCell::new(None),
    }
}

// ─── SutViewModel ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<'a, S: SutViewModel> SutViewModel for CachingProxy<'a, S> {
    /// Returns the eagerly-drained snapshot if the proxy was built with
    /// [`cached_with_drain`], else an empty Vec.
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        self.vm_emissions_cache.clone().unwrap_or_default()
    }

    async fn frontend_root_is_error(&self) -> bool {
        self.inner.frontend_root_is_error().await
    }

    async fn headless_error_node_count(&self) -> Option<usize> {
        self.inner.headless_error_node_count().await
    }
}

// ─── SutCdc (read-only subset only) ───────────────────────────────────

impl<'a, S: SutCdc> CachingProxy<'a, S> {
    /// Memoised `cdc_in_flight`. The proxy intentionally does NOT
    /// implement `SutCdc::drain_cdc` — see module-level doc.
    pub async fn cdc_in_flight_cached(&self) -> bool {
        {
            let guard = self.cdc_in_flight_cache.borrow();
            if let Some(v) = *guard {
                return v;
            }
        }
        let v = self.inner.cdc_in_flight().await;
        *self.cdc_in_flight_cache.borrow_mut() = Some(v);
        v
    }
}

// ─── SutSqlProjection ─────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<'a, S: SutSqlProjection> SutSqlProjection for CachingProxy<'a, S> {
    /// Per-id read — no drain-once issue, uncached.
    async fn block_row(&self, id: &CapBlockId) -> Option<Vec<String>> {
        self.inner.block_row(id).await
    }

    /// Memoised set snapshot.
    async fn all_block_ids(&self) -> BTreeSet<CapBlockId> {
        {
            let guard = self.all_block_ids_cache.borrow();
            if let Some(ids) = guard.clone() {
                return ids;
            }
        }
        let ids = self.inner.all_block_ids().await;
        *self.all_block_ids_cache.borrow_mut() = Some(ids.clone());
        ids
    }

    async fn watch_row_count(&self, query_id: &str) -> Option<usize> {
        self.inner.watch_row_count(query_id).await
    }

    async fn block_raw_row(&self, id: &CapBlockId) -> Option<Vec<String>> {
        self.inner.block_raw_row(id).await
    }

    async fn block_tag_block_ids(&self) -> BTreeSet<CapBlockId> {
        self.inner.block_tag_block_ids().await
    }

    async fn block_task_state(&self, id: &CapBlockId) -> Option<String> {
        self.inner.block_task_state(id).await
    }

    async fn block_content(&self, id: &CapBlockId) -> Option<String> {
        self.inner.block_content(id).await
    }
}
