# Memory Profiling

Two layers for detecting and diagnosing memory leaks.

## Layer 1: RSS Monitor (always-on)

Every `FrontendSession` automatically starts a background task that logs RSS
every 30 seconds via `tracing::info`. No setup needed.

```
[MemoryMonitor] Baseline RSS: 221.2MB
[MemoryMonitor] RSS 224.1MB (delta +2.9MB, +2.9MB since start)
[MemoryMonitor] ALERT: RSS 1247.1MB (+1000.0MB in 30s, +1001.8MB since start)
```

Thresholds:
- `INFO` — normal delta
- `WARN` — delta > 100MB between samples
- `ERROR` — delta > 500MB between samples

Make sure `RUST_LOG=info` (or more verbose) is set so the messages show up.

## Layer 2: Heap Profiling (on-demand)

Uses [dhat](https://docs.rs/dhat) behind a `heap-profile` feature flag.
Produces a JSON file showing **which code allocated how much memory**.

### Step 1: Build with profiling

```bash
# For Dioxus
cargo build -p holon-dioxus --features heap-profile

# For other frontends, forward the feature:
cargo build -p <frontend> --features holon-frontend/heap-profile
```

### Step 2: Run and reproduce the leak

```bash
HOLON_ORGMODE_ROOT=/path/to/org/files RUST_LOG=info ./target/debug/holon-dioxus
```

Let the app run until memory growth is visible in the monitor logs.

### Step 3: Stop with Ctrl+C

Press **Ctrl+C** (SIGINT). The signal handler writes `dhat-heap.json` to the
working directory before exiting:

```
[HeapProfile] Caught signal, writing dhat-heap.json...
[HeapProfile] dhat-heap.json written
```

### Step 4: Open the viewer

Open https://nnethercote.github.io/dh_view/dh_view.html and drag
`dhat-heap.json` into it.

The viewer shows a tree of allocation sites. Sort by **Total (bytes)** to find
the leak — the top entries point directly to the code path responsible.

Key columns:
- **Total (bytes)** — cumulative bytes allocated at this call site
- **At t-end (bytes)** — bytes still live when the profiler stopped (the leak)
- **Max (bytes)** — peak live bytes at this call site
- **Allocations** — number of allocations (high count + high total = hot loop)

### Adding heap-profile support to a new frontend

1. Add to the frontend's `Cargo.toml`:
   ```toml
   [features]
   heap-profile = ["holon-frontend/heap-profile"]
   ```

2. Add to the frontend's `main()`, **before any other code**:
   ```rust
   #[cfg(feature = "heap-profile")]
   let _profiler = holon_frontend::memory_monitor::heap_profile::start();
   ```

The `_profiler` guard installs a Ctrl+C handler. When the guard is dropped
(normal exit) or Ctrl+C is pressed, `dhat-heap.json` is written.

## Implementation

- `crates/holon-frontend/src/memory_monitor.rs` — both layers
- Feature flag `heap-profile` on `holon-frontend` enables `dhat` + `ctrlc` deps
- RSS monitoring uses the `memory-stats` crate (reads OS memory counters)
- Zero overhead when `heap-profile` is not enabled
