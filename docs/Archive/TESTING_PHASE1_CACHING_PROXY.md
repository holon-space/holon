# Phase 1 P1.4 — `CachingProxy<&Sut>` walkthrough

**Goal**: confirm the proxy design at Phase 7's `check_invariants_async` migration is sound. Source: `crates/holon-integration-tests/src/pbt/sut.rs`.

## Shared-state bindings in `check_invariants_async`

Three load-bearing pieces of shared state. Each is read by multiple `[inv-…]` blocks; the WARN/SKIP classifier depends on at least the first and third.

### `live_blocks_cell` — `RefCell<Option<Arc<LiveData<Block>>>>` (sut.rs:341)

- **Hydrated**: lazily via `Self::live_blocks(&self) -> Vec<Block>` (sut.rs:3863-3897). First call subscribes a `LiveData<Block>`; subsequent calls return cached.
- **Read by**: 9 `[inv-…]` blocks via `let live_blocks = self.live_blocks().await;` (line 4121 and similar).
- **Staleness**: never invalidated within a `check_invariants_async` call. Fresh `LiveData` per call.

**Proxy mapping**: `CachingProxy::live_blocks(&self) -> &Vec<Block>` — populates on first call via the inner `Sut`, stores in `OnceCell`. Subsequent calls in the same tick return the cached Vec. ✅ Trivial cache.

### `vm_emissions` — `Arc<Mutex<Vec<ViewModel>>>` (sut.rs:295)

- **Drained**: `std::mem::take(&mut *self.vm_emissions.lock().unwrap())` at sut.rs:5846. **Drain-once**.
- **Read by**: `inv-viewmodel-snapshot`, `inv-viewmodel-no-error-widgets`, several others.
- **Filled by**: `vm_emissions` is populated by a background task that watches the reactive root (sut.rs:3626).

**Proxy mapping**: `CachingProxy::vm_snapshots(&self) -> &[ViewModel]` — first call drains the Mutex into a stored Vec, subsequent calls return slice. ✅ Drain-once semantics solved by the proxy owning the drained Vec.

**Hazard**: late emissions arriving mid-tick (after the proxy drains) are invisible to invariants in this tick. They appear in the NEXT tick's drain. Document this in the proxy's rustdoc; matches today's implicit behaviour (the inline drain at line 5846 also doesn't pick up late emissions in the same call).

### `live_blocks_stale` — `bool` (sut.rs:4225)

- **Computed**: at sut.rs:4225 inside the `inv-backend-blocks-match-ref` block, after comparing backend live_blocks against ref state.
- **Read later**: sut.rs:4647 inside `inv-watch-rows-match-ref` ("Skip when inv-backend-blocks-match-ref detected the live_blocks mirror is stale").

This is the **WARN/SKIP classifier** in raw form. A boolean set early and consulted later by a different invariant block.

**Proxy mapping**: `CachingProxy::is_live_blocks_stale(&self, ref: &impl RefBlockTree) -> bool` — explicit async method, computed on first call, cached. The `inv-watch-rows-match-ref` body calls it; the body doesn't need to know which other invariant set it.

The plan's stated contract holds: tick = `cached(&sut)` call boundary; classifier surfaces as proxy methods.

## Per-invariant body shared-state reads — summary

Cross-tabulated from the explore-agent's body-range table:

| Invariant | live_blocks | vm_emissions | live_blocks_stale | Reads block_raw | Notes |
|---|:-:|:-:|:-:|:-:|---|
| inv-loro-no-errors | | | | | Reads loro_sut only |
| inv-backend-blocks-match-ref | ✓ | | sets ✓ | ✓ | Strict (Warn fallback) |
| inv-org-render-fixed-point | | | | | Reads org renderer + ref block tree |
| inv-watch-rows-match-ref | ✓ | | gates ✓ | ✓ | Warn mode |
| inv-focus-roots | | | | ✓ | Warn mode |
| inv-viewmodel-snapshot | | drains ✓ | | | First VM consumer |
| inv-viewmodel-no-error-widgets | | reads ✓ | | | Reads from drained VM |
| inv-viewmodel-root-matches-render-expr | | reads ✓ | | | |
| inv-viewmodel-entity-ids-subset-of-data | | reads ✓ | | ✓ | |
| inv-viewmodel-decompiled-rows-match-query | | reads ✓ | | ✓ | |
| inv-viewmodel-editable-text-triggers | | reads ✓ | | | |
| inv-viewmodel-state-toggle-correct | | reads ✓ | | | Plus ref block tree |
| inv-viewmodel-tree-virtual-slots | | reads ✓ | | | |
| inv-matview-consistent-with-ref | ✓ | | | | Plus matview query |
| inv-value-fn-provider-* | | reads ✓ | | | |
| inv-sql-budget | | | | | Reads OTel spans |
| inv-frontend-engine | | reads ✓ | | | Reads bounds + VM |
| inv-frontend-bounds-rendered | | | | | Reads BoundsRegistry only |
| inv-displayed-text | | reads ✓ | | | Reads bounds + editor mirror |
| inv-focus-matches-ref | | | | | Reads driver state |
| inv-live-children-match-ref | ✓ | | | | |

**Proxy methods needed (synthesised from above)**:

- `live_blocks()` — 4 readers
- `vm_snapshots()` — 9 readers (heaviest)
- `is_live_blocks_stale()` — 1 setter + 1 reader (cross-invariant)
- `block_raw(query)` — 4 readers (truth-check pattern)
- `matview_query(name)` — 1 reader (inv-matview-consistent-with-ref)
- `bounds_registry()` — 3 readers
- `otel_spans()` — 1 reader
- `loro_log()` — 1 reader
- `driver_state()` — 1 reader

**Verdict — H6 PASS structurally.** The proxy methods needed are bounded and each maps cleanly to a single capability:

- `live_blocks()`, `block_raw()`, `matview_query()` → `SutSqlProjection`
- `vm_snapshots()` → `SutViewModel`
- `bounds_registry()` → `SutLayout`
- `loro_log()` → `SutLoro`
- `otel_spans()` → `SutSqlProjection` (or a separate `SutObservability` if we want)
- `driver_state()` → `SutDriver`
- `is_live_blocks_stale()` → cross-cut helper on the proxy itself (composes `live_blocks()` + ref state)

This means **`CachingProxy<&Sut>` is itself a forwarding wrapper** that adds memoization on top of the existing capability traits' read methods. No new trait family needed.

## Risks to flag for Phase 7

1. **`vm_emissions` drain semantics are subtle**. The current code at sut.rs:5846 does ONE drain. The proxy maintains this. But: if Phase 7 splits invariant bodies into multiple closures called from multiple wrapper functions, each call site must use the same proxy instance to share the drained Vec. Document the singleton-per-tick contract.

2. **`live_blocks_cell` is a `RefCell<Option<Arc<LiveData<…>>>>`**. The proxy must hold an `Arc` clone of the `LiveData`, not a borrow of the RefCell's contents — otherwise the proxy outlives the RefCell guard.

3. **OTel span span-collector lifetime**. `inv-sql-budget` reads spans that accumulate during the previous transition. If the proxy caches spans, it must drain them *before* the next transition starts. Today this is implicit (each transition's spans go to the next invariant pass).

4. **`block_raw()` shows up in 4 readers but with different SQL queries**. The proxy can't cache `block_raw(arbitrary_query)` results — the query is the cache key. Either: per-query memoization (HashMap keyed on query string) or no caching for this method. Recommend no caching — it's a query-by-query truth check, latency is acceptable.

## Sketch for the proxy crate

```rust
// In holon-pbt-core/src/proxy.rs (Phase 7 — not Phase 1):
pub struct CachingProxy<S> {
    inner: S,
    live_blocks: OnceCell<Vec<Block>>,
    vm_snapshots: OnceCell<Vec<ViewModel>>,
    live_blocks_stale: OnceCell<bool>,
    // …
}

impl<S> CachingProxy<S> {
    pub fn new(sut: S) -> Self { … }
}

impl<S: SutSqlProjection> SutSqlProjection for CachingProxy<&S> {
    fn live_blocks(&self) -> &[Block] {
        self.live_blocks
            .get_or_init(|| futures::executor::block_on(self.inner.live_blocks()))
    }
    // block_raw passes through without caching (per-query)
}

impl<S: SutViewModel> SutViewModel for CachingProxy<&S> {
    fn vm_snapshots(&self) -> &[ViewModel] {
        self.vm_snapshots.get_or_init(|| self.inner.drain_vm_emissions())
    }
}
```

Async/sync interaction needs care — `block_on` inside `get_or_init` is the wrong shape if the proxy is called from async contexts. The real implementation will use `async-once-cell` or similar. Phase 7 to finalise.
