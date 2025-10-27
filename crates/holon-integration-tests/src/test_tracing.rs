//! In-memory OpenTelemetry span collection for integration tests.
//!
//! The tracing subscriber is global (per-process), initialized once via `SpanCollector::global()`.
//! Each PBT transition calls `reset()` to get per-transition isolation.
//!
//! Span names come from `#[tracing::instrument]` on SQL operations in `turso.rs`:
//! - `"query"` — SQL SELECT
//! - `"execute"` — SQL INSERT/UPDATE/DELETE
//! - `"execute_ddl"` — DDL statements
//! - `"execute_ddl_with_deps"` — DDL with dependency tracking

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::global;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};

/// Thread-safe handle to the in-memory span exporter.
/// Clone is cheap — `InMemorySpanExporter` wraps `Arc<Mutex<Vec<SpanData>>>`.
#[derive(Clone)]
pub struct SpanCollector {
    exporter: InMemorySpanExporter,
}

static GLOBAL_COLLECTOR: OnceLock<SpanCollector> = OnceLock::new();

/// Holds the `tracing-chrome` flush guard for the lifetime of the
/// process. Dropping it flushes the trace file. We park it in a static
/// so it lives until process exit — `_log_guard`-style stack guards
/// don't survive across `SpanCollector::global()`'s `OnceLock`.
///
/// `FlushGuard` isn't `Sync` (its inner `Cell<Option<JoinHandle>>`
/// blocks it), so we wrap it in a `Mutex` to make the static safe to
/// share. We never lock the mutex after init — the guard exists only
/// to be dropped at process exit.
#[cfg(feature = "chrome-trace")]
static CHROME_TRACE_GUARD: OnceLock<std::sync::Mutex<Option<tracing_chrome::FlushGuard>>> =
    OnceLock::new();

/// Flush the chrome trace file. Call before `std::process::exit` —
/// `OnceLock`-stored guards aren't dropped at process exit and the
/// chrome trace JSON is left truncated (no closing `]`).
///
/// No-op when the `chrome-trace` feature is disabled or no trace has
/// been started.
pub fn flush_chrome_trace() {
    #[cfg(feature = "chrome-trace")]
    if let Some(slot) = CHROME_TRACE_GUARD.get() {
        if let Ok(mut guard) = slot.lock() {
            if let Some(flush_guard) = guard.take() {
                drop(flush_guard);
                eprintln!("[test_tracing] Chrome trace flushed");
            }
        }
    }
}

impl SpanCollector {
    /// Get the global SpanCollector, initializing the tracing subscriber on first call.
    ///
    /// Uses `OnceLock` because proptest runs many cases sequentially in one process
    /// and `set_global_default` can only be called once.
    pub fn global() -> &'static SpanCollector {
        GLOBAL_COLLECTOR.get_or_init(|| {
            // Install a panic hook that flushes the chrome trace before
            // the panic propagates. The PBT thread regularly panics
            // (intentional, on invariant violations) and never reaches
            // the explicit `flush_chrome_trace()` call in test main(),
            // so without a hook the trace JSON is left truncated.
            #[cfg(feature = "chrome-trace")]
            {
                let prev_hook = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    flush_chrome_trace();
                    prev_hook(info);
                }));
            }

            let exporter = InMemorySpanExporter::default();
            let collector = SpanCollector {
                exporter: exporter.clone(),
            };

            let provider = SdkTracerProvider::builder()
                .with_simple_exporter(exporter)
                .build();
            global::set_tracer_provider(provider);

            let otel_layer =
                tracing_opentelemetry::OpenTelemetryLayer::new(global::tracer("holon-pbt"));

            use tracing_subscriber::EnvFilter;
            use tracing_subscriber::Layer as _;
            use tracing_subscriber::layer::SubscriberExt;
            use tracing_subscriber::util::SubscriberInitExt;

            let registry = tracing_subscriber::registry().with(otel_layer).with(
                tracing_subscriber::fmt::layer()
                    .with_test_writer()
                    .with_filter(
                        EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
                    ),
            );

            #[cfg(feature = "chrome-trace")]
            {
                let file_path = std::env::var("CHROME_TRACE_FILE").unwrap_or_else(|_| {
                    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                    format!("trace-{ts}.json")
                });
                // Recording every TRACE-level span produces an
                // unusably large file (>200 MB / 30 s) and slows the
                // app enough to miss its 120 s window-ready deadline.
                // Default to a curated filter that captures spans
                // relevant for click-to-render latency: PBT
                // transitions (info), GPUI render/reconcile (debug),
                // UiWatcher fan-out (debug), Turso queries (debug).
                // Anything else stays at WARN. Override with
                // `CHROME_TRACE_FILTER` (any `EnvFilter` syntax).
                let filter_spec = std::env::var("CHROME_TRACE_FILTER").unwrap_or_else(|_| {
                    [
                        "warn",
                        "holon=info",
                        "holon::api=debug",
                        "holon_frontend=debug",
                        "holon_gpui=debug",
                        "holon_integration_tests=info",
                    ]
                    .join(",")
                });
                let chrome_filter = EnvFilter::new(&filter_spec);
                let (chrome_layer, chrome_guard) = tracing_chrome::ChromeLayerBuilder::new()
                    .file(file_path.clone())
                    .include_args(true)
                    .include_locations(false)
                    .build();
                CHROME_TRACE_GUARD
                    .set(std::sync::Mutex::new(Some(chrome_guard)))
                    .map_err(|_| ())
                    .expect("CHROME_TRACE_GUARD must only be set once");
                eprintln!(
                    "[test_tracing] Recording Chrome trace to {file_path} (filter={filter_spec})"
                );
                registry
                    .with(chrome_layer.with_filter(chrome_filter))
                    .init();
            }

            #[cfg(not(feature = "chrome-trace"))]
            registry.init();

            collector
        })
    }

    /// Clear all collected spans. Call at the start of each transition.
    pub fn reset(&self) {
        self.exporter.reset();
    }

    /// Get all spans collected since last reset.
    pub fn finished_spans(&self) -> Vec<SpanData> {
        self.exporter
            .get_finished_spans()
            .expect("InMemorySpanExporter lock poisoned")
    }

    /// Count spans whose name exactly matches.
    pub fn count_spans(&self, name: &str) -> usize {
        self.finished_spans()
            .iter()
            .filter(|s| s.name.as_ref() == name)
            .count()
    }

    /// Get spans matching a name, sorted by start time.
    pub fn spans_named(&self, name: &str) -> Vec<SpanData> {
        let mut spans: Vec<_> = self
            .finished_spans()
            .into_iter()
            .filter(|s| s.name.as_ref() == name)
            .collect();
        spans.sort_by_key(|s| s.start_time);
        spans
    }

    /// Maximum duration of any span matching the given name.
    /// Returns `Duration::ZERO` if no matching spans.
    pub fn max_duration_of(&self, name: &str) -> Duration {
        self.spans_named(name)
            .iter()
            .map(|s| span_duration(s))
            .max()
            .unwrap_or(Duration::ZERO)
    }

    /// Structured snapshot of all collected spans for assertion + persistence.
    pub fn snapshot(&self) -> TransitionMetrics {
        let spans = self.finished_spans();

        let sql_read_count = spans.iter().filter(|s| s.name.as_ref() == "query").count();
        let sql_write_count = spans
            .iter()
            .filter(|s| s.name.as_ref() == "execute")
            .count();
        let sql_ddl_count = spans
            .iter()
            .filter(|s| {
                s.name.as_ref() == "execute_ddl" || s.name.as_ref() == "execute_ddl_with_deps"
            })
            .count();

        let sql_spans = spans.iter().filter(|s| {
            matches!(
                s.name.as_ref(),
                "query" | "execute" | "execute_ddl" | "execute_ddl_with_deps"
            )
        });

        let max_query_duration = sql_spans
            .clone()
            .map(span_duration)
            .max()
            .unwrap_or(Duration::ZERO);

        let total_query_duration: Duration = sql_spans.clone().map(span_duration).sum();

        // Duplicate SQL detection: count identical SQL texts fired multiple times.
        // The `sql` attribute is set by turso.rs #[tracing::instrument(fields(sql = ...))].
        let duplicate_sql = find_duplicate_sql(sql_spans);

        // ── Render metrics ───────────────────────────────────────
        let render_spans: Vec<_> = spans
            .iter()
            .filter(|s| s.name.as_ref() == "frontend.render")
            .collect();
        let render_count = render_spans.len();
        let max_render_duration = render_spans
            .iter()
            .map(|s| span_duration(s))
            .max()
            .unwrap_or(Duration::ZERO);
        let total_render_duration: Duration = render_spans.iter().map(|s| span_duration(s)).sum();

        let mut component_counts: HashMap<String, usize> = HashMap::new();
        for span in &render_spans {
            let component = span_attr(span, "component").unwrap_or_else(|| "unknown".into());
            *component_counts.entry(component).or_default() += 1;
        }
        let mut render_by_component: Vec<_> = component_counts.into_iter().collect();
        render_by_component.sort_by(|a, b| b.1.cmp(&a.1));

        // ── CDC metrics ──────────────────────────────────────────
        let cdc_ingest_count = spans
            .iter()
            .filter(|s| s.name.as_ref() == "queryable_cache.ingest_batch")
            .count();
        let cdc_emission_count = spans
            .iter()
            .filter(|s| s.name.as_ref() == "queryable_cache.cdc_emission")
            .count();

        // ── PBT perf attribution (HOLON_PERF investigation) ──────
        let sum_span = |name: &str| -> Duration {
            spans
                .iter()
                .filter(|s| s.name.as_ref() == name)
                .map(span_duration)
                .sum()
        };
        let inv10_watch_drain = sum_span("pbt.inv10_watch_drain");
        let wait_files_stable = sum_span("pbt.wait_for_org_files_stable");
        let wait_file_sync = sum_span("pbt.wait_for_org_file_sync");
        let mark_processed_total = sum_span("events.mark_processed");
        let mark_processed_count = spans
            .iter()
            .filter(|s| s.name.as_ref() == "events.mark_processed")
            .count();
        let apply_transition_total = sum_span("pbt.apply_transition");
        let check_invariants_total = sum_span("pbt.check_invariants");
        let drain_cdc_total =
            sum_span("pbt.drain_cdc_events") + sum_span("pbt.drain_region_cdc_events");

        TransitionMetrics {
            sql_read_count,
            sql_write_count,
            sql_ddl_count,
            max_query_duration,
            total_query_duration,
            total_span_count: spans.len(),
            duplicate_sql,
            render_count,
            render_by_component,
            max_render_duration,
            total_render_duration,
            cdc_ingest_count,
            cdc_emission_count,
            inv10_watch_drain,
            wait_files_stable,
            wait_file_sync,
            mark_processed_total,
            mark_processed_count,
            apply_transition_total,
            check_invariants_total,
            drain_cdc_total,
        }
    }
}

fn span_duration(span: &SpanData) -> Duration {
    span.end_time
        .duration_since(span.start_time)
        .unwrap_or(Duration::ZERO)
}

/// Extract a string attribute from a span's attributes by key.
fn span_attr(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.to_string())
}

/// Extract the `sql` attribute from a span's attributes.
fn sql_attr(span: &SpanData) -> Option<String> {
    span_attr(span, "sql")
}

/// Find SQL texts that appear more than once (potential N+1 pattern).
/// Returns (sql_text, count) pairs sorted by count descending.
fn find_duplicate_sql<'a>(sql_spans: impl Iterator<Item = &'a SpanData>) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for span in sql_spans {
        if let Some(sql) = sql_attr(span) {
            *counts.entry(sql).or_default() += 1;
        }
    }
    let mut duplicates: Vec<_> = counts.into_iter().filter(|(_, count)| *count > 1).collect();
    duplicates.sort_by(|a, b| b.1.cmp(&a.1));
    duplicates
}

/// Structured metrics from a single transition's span collection.
#[derive(Debug, Clone)]
pub struct TransitionMetrics {
    /// SQL SELECT queries (`"query"` spans from turso.rs)
    pub sql_read_count: usize,
    /// SQL INSERT/UPDATE/DELETE (`"execute"` spans from turso.rs)
    pub sql_write_count: usize,
    /// DDL statements (`"execute_ddl"` + `"execute_ddl_with_deps"`)
    pub sql_ddl_count: usize,
    /// Slowest individual SQL operation
    pub max_query_duration: Duration,
    /// Sum of all SQL operation durations
    pub total_query_duration: Duration,
    /// Total OTel spans emitted (all types)
    pub total_span_count: usize,
    /// SQL texts fired more than once: (sql_text, count). Potential N+1 patterns.
    pub duplicate_sql: Vec<(String, usize)>,

    // ── Render metrics (from "frontend.render" spans) ────────────
    /// Total frontend render spans
    pub render_count: usize,
    /// Per-component render counts: (component_name, count), sorted by count descending
    pub render_by_component: Vec<(String, usize)>,
    /// Slowest individual render span
    pub max_render_duration: Duration,
    /// Sum of all render span durations
    pub total_render_duration: Duration,

    // ── CDC metrics (from existing queryable_cache spans) ────────
    /// CDC batch ingestion spans ("queryable_cache.ingest_batch")
    pub cdc_ingest_count: usize,
    /// CDC emission spans ("queryable_cache.cdc_emission")
    pub cdc_emission_count: usize,

    // ── PBT perf attribution (HOLON_PERF investigation) ──────────
    /// Time spent inside the inv10 reactive.watch + drain block (sut.rs:2820).
    pub inv10_watch_drain: Duration,
    /// Time spent inside `wait_for_org_files_stable` (called from both apply and check).
    pub wait_files_stable: Duration,
    /// Time spent inside `wait_for_org_file_sync` (apply path only — hits 5s timeouts).
    pub wait_file_sync: Duration,
    /// Cumulative time inside `events.mark_processed` (the suspected N+1 update).
    pub mark_processed_total: Duration,
    /// Number of `events.mark_processed` calls in this transition.
    pub mark_processed_count: usize,
    /// Total time inside `apply_transition_async` (the SUT-side of a transition).
    pub apply_transition_total: Duration,
    /// Total time inside `check_invariants_async` (post-transition assertions).
    pub check_invariants_total: Duration,
    /// Total time inside `drain_cdc_events` + `drain_region_cdc_events` (1s/200ms timeouts).
    pub drain_cdc_total: Duration,
}

impl TransitionMetrics {
    /// Total SQL operations (reads + writes + DDL).
    pub fn sql_total(&self) -> usize {
        self.sql_read_count + self.sql_write_count + self.sql_ddl_count
    }
}

/// Detailed per-category SQL breakdown for a transition.
/// Groups SQL statements by span type and deduplicates.
#[derive(Debug)]
pub struct SqlBreakdown {
    /// (sql_text_truncated, count) for "query" spans
    pub reads: Vec<(String, usize)>,
    /// (sql_text_truncated, count) for "execute" spans
    pub writes: Vec<(String, usize)>,
    /// (sql_text_truncated, count) for "execute_ddl"/"execute_ddl_with_deps" spans
    pub ddl: Vec<(String, usize)>,
}

impl SpanCollector {
    /// Detailed SQL breakdown grouped by type, with deduplication.
    pub fn sql_breakdown(&self) -> SqlBreakdown {
        let spans = self.finished_spans();

        fn group(spans: &[SpanData], names: &[&str]) -> Vec<(String, usize)> {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for span in spans.iter().filter(|s| names.contains(&s.name.as_ref())) {
                let sql = sql_attr(span).unwrap_or_else(|| "<no sql attr>".into());
                *counts.entry(sql).or_default() += 1;
            }
            let mut items: Vec<_> = counts.into_iter().collect();
            items.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            items
        }

        SqlBreakdown {
            reads: group(&spans, &["query"]),
            writes: group(&spans, &["execute"]),
            ddl: group(&spans, &["execute_ddl", "execute_ddl_with_deps"]),
        }
    }
}

impl std::fmt::Display for SqlBreakdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.reads.is_empty() {
            writeln!(
                f,
                "  READS ({} unique, {} total):",
                self.reads.len(),
                self.reads.iter().map(|r| r.1).sum::<usize>()
            )?;
            for (sql, count) in &self.reads {
                writeln!(f, "    {count:>3}x {sql}")?;
            }
        }
        if !self.writes.is_empty() {
            writeln!(
                f,
                "  WRITES ({} unique, {} total):",
                self.writes.len(),
                self.writes.iter().map(|r| r.1).sum::<usize>()
            )?;
            for (sql, count) in &self.writes {
                writeln!(f, "    {count:>3}x {sql}")?;
            }
        }
        if !self.ddl.is_empty() {
            writeln!(
                f,
                "  DDL ({} unique, {} total):",
                self.ddl.len(),
                self.ddl.iter().map(|r| r.1).sum::<usize>()
            )?;
            for (sql, count) in &self.ddl {
                writeln!(f, "    {count:>3}x {sql}")?;
            }
        }
        Ok(())
    }
}

/// Read current RSS (Resident Set Size) in bytes. Returns 0 if unavailable.
pub fn current_rss_bytes() -> usize {
    memory_stats::memory_stats()
        .map(|s| s.physical_mem)
        .unwrap_or(0)
}

// ── Folded-stack flamegraph generation ────────────────────────────

/// Write collected spans as folded stacks (compatible with flamegraph.pl / inferno).
///
/// Each line: `ancestor;parent;span_name duration_us`
/// Open the output with `inferno-flamegraph` or `speedscope` for visualization.
pub fn write_folded_stacks(spans: &[SpanData], path: &std::path::Path) {
    use opentelemetry::trace::SpanId;
    use std::io::Write;

    // Index spans by their span_id for parent lookup
    let by_id: HashMap<SpanId, &SpanData> = spans
        .iter()
        .map(|s| (s.span_context.span_id(), s))
        .collect();

    let mut lines: Vec<String> = Vec::new();

    for span in spans {
        // Build the stack from leaf to root
        let mut stack = vec![span.name.as_ref().to_string()];
        let mut current = span;
        while current.parent_span_id != SpanId::INVALID {
            if let Some(parent) = by_id.get(&current.parent_span_id) {
                stack.push(parent.name.as_ref().to_string());
                current = parent;
            } else {
                break;
            }
        }
        stack.reverse();
        let duration_us = span_duration(span).as_micros();
        if duration_us > 0 {
            lines.push(format!("{} {duration_us}", stack.join(";")));
        }
    }

    let mut file = std::fs::File::create(path)
        .unwrap_or_else(|e| panic!("failed to create flamegraph file {}: {e}", path.display()));
    for line in &lines {
        writeln!(file, "{line}").expect("failed to write flamegraph line");
    }
}

/// Generate a folded stacks file for spans from the heaviest transition
/// in the current collector. Call after the test run completes.
///
/// Only writes if `HOLON_PERF_FLAMEGRAPH` env var is set (to a directory path).
/// File is named `{transition_key}.folded`.
pub fn maybe_write_flamegraph(collector: &SpanCollector, transition_key: &str) {
    let dir = match std::env::var("HOLON_PERF_FLAMEGRAPH") {
        Ok(d) if !d.is_empty() => std::path::PathBuf::from(d),
        _ => return,
    };

    std::fs::create_dir_all(&dir).expect("failed to create flamegraph output dir");

    let spans = collector.finished_spans();
    if spans.is_empty() {
        return;
    }

    // Write SQL + render + CDC spans for a complete performance picture
    let perf_spans: Vec<_> = spans
        .into_iter()
        .filter(|s| {
            matches!(
                s.name.as_ref(),
                "query"
                    | "execute"
                    | "execute_ddl"
                    | "execute_ddl_with_deps"
                    | "compile_to_sql"
                    | "execute_query"
                    | "query_and_watch"
                    | "frontend.render"
                    | "queryable_cache.ingest_batch"
                    | "queryable_cache.cdc_emission"
            )
        })
        .collect();

    let path = dir.join(format!("{transition_key}.folded"));
    write_folded_stacks(&perf_spans, &path);
    eprintln!(
        "[flamegraph] Written {} spans to {}",
        perf_spans.len(),
        path.display()
    );
}
