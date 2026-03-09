
# Performance Profiling

Trace-level profiling for diagnosing UI latency (e.g., slow view switching).

Uses `tracing-chrome` to produce Chrome Trace Event JSON files, viewable in:
- **Firefox Profiler**: https://profiler.firefox.com/
- **Perfetto**: https://ui.perfetto.dev/
- **Chrome**: `chrome://tracing`

## Quick Start (Blinc)

```bash
cargo run -p holon-blinc --features chrome-trace
```

Use the app — switch views, navigate blocks, etc. When done, stop the app
(Ctrl+C or close window). A `trace-{timestamp}.json` file appears in the
working directory.

Open it in Firefox Profiler (drag and drop).

## What's Traced

Instrumented spans on the view-switching hot path:

| Span | Location | What it measures |
|---|---|---|
| `watch_ui` | `ui_watcher.rs` | Full watch setup (structural matview + initial render) |
| `do_render` | `ui_watcher.rs` | Each re-render cycle |
| `render_entity` | `block_domain.rs` | Block rendering (query compile + watch + profile) |
| `load_block_with_query_source` | `block_domain.rs` | DB lookup for block's query source |
| `lookup_block_path` | `block_domain.rs` | Block path resolution |
| `compile_to_sql` | `backend_engine.rs` | PRQL/GQL/SQL compilation |
| `query_and_watch` | `backend_engine.rs` | Matview creation + initial query + CDC subscribe |
| `execute_query` | `backend_engine.rs` | Any SQL query execution |
| `ensure_view` | `matview_manager.rs` | Matview existence check / creation |
| `query_view` | `matview_manager.rs` | SELECT * from matview |
| `watch` | `matview_manager.rs` | ensure_view + query + subscribe |
| `query` | `turso.rs` | DB actor round-trip for SELECT |
| `execute` | `turso.rs` | DB actor round-trip for INSERT/UPDATE/DELETE |
| `execute_ddl` | `turso.rs` | DB actor round-trip for DDL |
| `execute_ddl_with_deps` | `turso.rs` | DDL with dependency wait |

## Configuration

| Env var | Default | Description |
|---|---|---|
| `CHROME_TRACE_FILE` | `trace-{timestamp}.json` | Output file path |
| `RUST_LOG` | `holon_blinc=info,holon=info` | Filter — use `debug` for more spans |

## Reading the Trace

In Firefox Profiler:
1. Look at the **flame chart** — each horizontal bar is a span
2. Spans nest: `watch_ui` → `do_render` → `render_entity` → `query_and_watch` → `ensure_view` + `query`
3. Find the widest bar — that's where time is spent
4. DB operations (`query`, `execute_ddl`) show the SQL in the span's `sql` field

Common things to look for:
- `ensure_view` taking long = matview creation (DDL) is slow
- `query` taking long = SQLite query is slow (check SQL in span args)
- `execute_ddl_with_deps` taking long = waiting for dependency resources
- Gap between spans = time in async scheduling or channel communication

## Adding to Other Frontends

1. Add to the frontend's `Cargo.toml`:
   ```toml
   [features]
   chrome-trace = ["holon-frontend/chrome-trace"]
   ```

2. Add to the frontend's `main()`:
   ```rust
   #[cfg(feature = "chrome-trace")]
   let (_chrome_trace_guard, _) = {
       use tracing_subscriber::layer::SubscriberExt;
       let (chrome_layer, guard) = holon_frontend::memory_monitor::chrome_trace::layer();
       let subscriber = tracing_subscriber::Registry::default()
           .with(chrome_layer)
           .with(tracing_subscriber::fmt::layer())
           .with(tracing_subscriber::EnvFilter::try_from_default_env()
               .unwrap_or_else(|_| "info".into()));
       tracing::subscriber::set_global_default(subscriber).unwrap();
       (guard, true)
   };
   ```

## Implementation

- `crates/holon-frontend/src/memory_monitor.rs` — `chrome_trace` module
- Feature flag `chrome-trace` on `holon-frontend` enables `tracing-chrome` + `chrono` deps
- Zero overhead when `chrome-trace` is not enabled — spans still exist but produce no output
