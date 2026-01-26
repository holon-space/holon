# GPUI Memory Analysis — 2026-04-08

## Summary

At 1 GB RSS (dhat capture at `HOLON_RSS_ABORT_MB=2000`), heap is **1059 MB** across 914K live blocks.
Total churn: 121 GB allocated over the session lifetime (338M allocation events).

## Breakdown by Category

| Category | MB | % | Live Blocks | Avg Size |
|----------|---:|--:|------------:|---------:|
| GPUI `Arc<[ElementId]>` | 540 | 51% | ~26K | ~22 KB |
| holon `Arc<ReactiveViewModel>` | 210 | 20% | 242K+ | 272 B |
| Taffy layout `SlotMap<NodeData>` | 112 | 11% | 1 | 78 MB |
| GPUI arena/element | 75 | 7% | ~25 | 1 MB |
| holon Value/RenderTypes clone | 41 | 4% | ~16K | 2.6 KB |
| GPUI profiler `CircularBuffer` | 40 | 4% | 2-4 | 20 MB |
| GPUI other | 28 | 3% | — | — |
| Turso/limbo | 2 | 0.2% | — | — |

## Top Allocation Sites (at peak)

### #1 — `Arc<[ElementId]>` path slices (540 MB, 51%)

Multiple call sites, all `allocate_for_slice<gpui::window::ElementId>`.
Each GPUI element stores its identity as an `Arc<[ElementId]>` path from root.
With ~3000 elements and repeated render passes, these accumulate.
~26K live allocations averaging 22 KB each.

### #2 — `Arc<ReactiveViewModel>` (210 MB, 20%)

129K+ live `Arc<ReactiveViewModel>` at 272 bytes each (33.5 MB),
plus 113K more at similar size (29.4 MB), plus Vec storage (27.6 MB).
These are items in `ReactiveView::items` MutableVec and structural tree nodes.
Never freed because `ReactiveEngine::unwatch()` is never called —
watchers and their associated ViewModels accumulate indefinitely.

### #3 — Taffy `SlotMap<NodeData>` (112 MB, 11%)

Single 78 MB allocation: `SlotMap::with_capacity_and_key<DefaultKey, NodeData>`.
Taffy's layout node pool grows monotonically and never shrinks.
Every element that was ever laid out keeps a slot.

### #4 — GPUI arena (75 MB, 7%)

Arena chunks of 1 MB each (`gpui::arena::Arena`).
Used for render tree element storage (`AnyElement::new`).
~25 live arenas — likely one per render pass, not all freed.

### #5 — holon Value clones (41 MB, 4%)

`HashMap<String, holon_api::Value>` clones during CDC sync and profile resolution.
8148 live hashmap allocations at 2.6 KB avg. Driven by cc_task sync
(876 tasks re-diffed every ~10s, each requiring full row clone).

## Key Observations

1. **Turso is NOT the problem** — only 2 MB (0.2%). DBSP circuits are lightweight.
2. **GPUI element identity is the #1 issue** — half of all memory.
3. **ReactiveViewModel accumulation is the #2 issue** — `unwatch()` never called.
4. **Taffy never shrinks** — 112 MB fixed cost after layout.
5. **Massive churn** — 121 GB total allocated for 1 GB live. Fragmentation is significant
   (RSS 1.3 GB for 1.06 GB heap = ~20% overhead from allocator).

## Reproduction

```bash
rm -f ~/.config/holon/holon.db ~/.config/holon/holon.db-wal ~/.config/holon/holon.db-shm
HOLON_RSS_ABORT_MB=2000 cargo run -p holon-gpui --features heap-profile --release
# Wait for RSS to grow; auto-captures dhat-heap.json on threshold
# Open: https://nnethercote.github.io/dh_view/dh_view.html
```

Non-deterministic — sometimes stays at 150 MB, sometimes grows to GB.
Likely depends on timing of claude-history MCP sync notifications during startup.

## Actionable Fixes (priority order)

1. **GPUI ElementId paths** — investigate why 22 KB path arrays accumulate; likely needs GPUI-side fix
2. **Call `unwatch()`** — add lifecycle cleanup so ReactiveViewModels are freed on navigation
3. **Taffy pool cap** — consider resetting Taffy tree between navigations
4. **QueryableCache diff** — avoid full-row clone when diffing unchanged cc_task records
