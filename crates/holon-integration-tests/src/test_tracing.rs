//! In-memory OpenTelemetry span collection for integration tests.
//!
//! The tracing subscriber is global (per-process), initialized once via
//! `SpanCollector::global()`. Each PBT transition calls `reset()` to get
//! per-transition isolation.
//!
//! Span names come from `#[tracing::instrument]` on SQL operations in
//! `turso.rs`:
//! - `"query"` — SQL SELECT
//! - `"execute"` — SQL INSERT/UPDATE/DELETE
//! - `"execute_ddl"` — DDL statements
//! - `"execute_ddl_with_deps"` — DDL with dependency tracking

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::global;
use opentelemetry_sdk::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::SpanData;

/// Thread-safe handle to the in-memory span exporter.
/// Clone is cheap — `InMemorySpanExporter` wraps `Arc<Mutex<Vec<SpanData>>>`.
#[derive(Clone)]
pub struct SpanCollector {
    exporter: InMemorySpanExporter,
    /// Swallowed problems (ERROR events + panics) captured since the last
    /// [`SpanCollector::reset`]. Shared `Arc` — clones observe the same sink.
    problems: ProblemSink,
}

/// A problem captured during a test run that would otherwise be SWALLOWED: an
/// ERROR-level tracing event, or a panic on ANY thread — including spawned
/// background tokio workers, whose panics kill only that task and never fail
/// the test thread (exactly how the advice `No id found` panic hid behind a
/// green deterministic test). Drained per-case by the observability invariant.
#[derive(Clone, Debug)]
pub struct CapturedProblem {
    pub kind: ProblemKind,
    pub target: String,
    pub message: String,
    pub location: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    ErrorLog,
    Panic,
}

impl std::fmt::Display for CapturedProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            ProblemKind::ErrorLog => "ERROR",
            ProblemKind::Panic => "PANIC",
        };
        let loc = self.location.as_deref().unwrap_or("?");
        write!(f, "[{kind}] {} ({loc}): {}", self.target, self.message)
    }
}

/// Shared, resettable sink for captured problems.
type ProblemSink = Arc<Mutex<Vec<CapturedProblem>>>;

/// A `tracing` layer that records every ERROR-level EVENT into the shared sink.
/// Events (not spans) are what `tracing::error!(...)` emits; the OTel layer
/// only captures spans, so a bare `error!` would otherwise be invisible to
/// assertions.
struct ErrorCaptureLayer {
    sink: ProblemSink,
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    extra: Vec<String>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.extra.push(format!("{}={value:?}", field.name()));
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ErrorCaptureLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() != tracing::Level::ERROR {
            return;
        }
        let mut v = MessageVisitor::default();
        event.record(&mut v);
        let meta = event.metadata();
        let message = if v.message.is_empty() {
            v.extra.join(" ")
        } else if v.extra.is_empty() {
            v.message
        } else {
            format!("{} ({})", v.message, v.extra.join(" "))
        };
        let location = match (meta.file(), meta.line()) {
            (Some(file), Some(line)) => Some(format!("{file}:{line}")),
            _ => None,
        };
        if let Ok(mut g) = self.sink.lock() {
            g.push(CapturedProblem {
                kind: ProblemKind::ErrorLog,
                target: meta.target().to_string(),
                message,
                location,
            });
        }
    }
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
    /// Get the global SpanCollector, initializing the tracing subscriber on
    /// first call.
    ///
    /// Uses `OnceLock` because proptest runs many cases sequentially in one
    /// process and `set_global_default` can only be called once.
    pub fn global() -> &'static SpanCollector {
        GLOBAL_COLLECTOR.get_or_init(|| {
            let problems: ProblemSink = Arc::new(Mutex::new(Vec::new()));

            // Record panics from EVERY thread — including spawned background tokio
            // workers, whose panics kill only that task and never fail the test
            // thread (exactly how the advice `No id found` panic hid behind a green
            // test) — into the shared sink so `inv-no-observed-errors` fails on a
            // swallowed panic. Then flush the chrome trace (the intentionally-
            // panicking PBT thread would otherwise leave it truncated) and chain
            // the previous hook.
            {
                let sink = problems.clone();
                let prev_hook = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    let message = info
                        .payload()
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| info.payload().downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "<non-string panic payload>".to_string());
                    let location = info
                        .location()
                        .map(|l| format!("{}:{}", l.file(), l.line()));
                    let target = std::thread::current()
                        .name()
                        .unwrap_or("<unnamed>")
                        .to_string();
                    if let Ok(mut g) = sink.lock() {
                        g.push(CapturedProblem {
                            kind: ProblemKind::Panic,
                            target,
                            message,
                            location,
                        });
                    }
                    flush_chrome_trace();
                    prev_hook(info);
                }));
            }

            let exporter = InMemorySpanExporter::default();
            let collector = SpanCollector {
                exporter: exporter.clone(),
                problems: problems.clone(),
            };

            let provider = SdkTracerProvider::builder()
                .with_simple_exporter(exporter)
                .build();
            global::set_tracer_provider(provider);

            use tracing_subscriber::EnvFilter;
            use tracing_subscriber::Layer as _;
            use tracing_subscriber::layer::SubscriberExt;
            use tracing_subscriber::util::SubscriberInitExt;

            // PERF (2026-06-10): the OTel layer used to record EVERY span at
            // EVERY level (no filter). The render interpreter's recursive
            // DEBUG-level `interpret` span is emitted 100k+ times per run
            // (~93% of all spans) and is consumed by no invariant or budget —
            // recording it via the synchronous in-memory exporter dominated
            // wall time (filtering it cut per-transition `apply` 34–69% and
            // even sped up unrelated SQL 6× by relieving CPU/mutex pressure).
            //
            // Default `info` keeps every span the invariants/budgets read
            // (SQL `query`/`execute`/`execute_ddl`, `pbt.*`, `queryable_cache.*`
            // are all INFO) and drops the DEBUG render-tree noise. Override
            // with `HOLON_OTEL_FILTER` (any `EnvFilter` syntax) to record the
            // hot-path spans back, e.g. `HOLON_OTEL_FILTER=trace` for a full
            // chrome-trace render investigation.
            let otel_filter = EnvFilter::new(
                std::env::var("HOLON_OTEL_FILTER").unwrap_or_else(|_| "info".into()),
            );
            let otel_layer =
                tracing_opentelemetry::OpenTelemetryLayer::new(global::tracer("holon-pbt"))
                    .with_filter(otel_filter);

            // ERROR-only filter: registers interest in ERROR alone, so it does
            // NOT raise the registry's global max-level and resurrect the hot
            // DEBUG `interpret`-span cost the OTel filter above deliberately drops.
            let error_capture = ErrorCaptureLayer {
                sink: problems.clone(),
            }
            .with_filter(tracing_subscriber::filter::LevelFilter::ERROR);

            let registry = tracing_subscriber::registry()
                .with(otel_layer)
                .with(error_capture)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_test_writer()
                        .with_filter(
                            EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
                        ),
                );

            // tokio-console async-wait profiler. `spawn()` starts the gRPC
            // aggregator on its own background thread+runtime (no ambient
            // runtime required) so the `tokio-console` TUI can attach. Only
            // collects task busy/idle data when the binary is built with
            // `RUSTFLAGS="--cfg tokio_unstable"` (and tokio's `tracing`
            // feature, pulled in by the `tokio-console` cargo feature).
            // Bind address overridable via `TOKIO_CONSOLE_BIND`.
            #[cfg(feature = "tokio-console")]
            let registry = registry.with(
                console_subscriber::ConsoleLayer::builder()
                    .with_default_env()
                    .spawn(),
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
                    .trace_style(tracing_chrome::TraceStyle::Async)
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

    /// Clear all collected spans + captured problems. Call at the start of each
    /// transition (per-transition isolation for both spans and error/panic
    /// capture).
    pub fn reset(&self) {
        self.exporter.reset();
        if let Ok(mut g) = self.problems.lock() {
            g.clear();
        }
    }

    /// Problems (ERROR-level tracing events + panics on any thread) captured
    /// since the last [`SpanCollector::reset`]. Read by the observability
    /// invariant so a SWALLOWED error/panic fails the run.
    pub fn captured_problems(&self) -> Vec<CapturedProblem> {
        self.problems.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Count of problems captured since the last [`SpanCollector::reset`].
    pub fn problem_count(&self) -> usize {
        self.problems.lock().map(|g| g.len()).unwrap_or(0)
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
            .map(span_duration)
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
        // The `sql` attribute is set by turso.rs #[tracing::instrument(fields(sql =
        // ...))].
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
        let mark_processed_total = sum_span("events.mark_processed");
        let mark_processed_count = spans
            .iter()
            .filter(|s| s.name.as_ref() == "events.mark_processed")
            .count();
        let apply_transition_total = sum_span("pbt.apply_transition");
        let check_invariants_total = sum_span("pbt.check_invariants");
        let settle_total = sum_span("pbt.settle");
        let drain_cdc_total =
            sum_span("pbt.drain_cdc_events") + sum_span("pbt.drain_region_cdc_events");
        let pre_inv16_settle_total = sum_span("pbt.pre_inv16_settle");
        let live_mirrors_total = sum_span("pbt.wait_for_live_data_mirrors");
        let assert_quiescent_total = sum_span("pbt.assert_cdc_quiescent");

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
            mark_processed_total,
            mark_processed_count,
            apply_transition_total,
            check_invariants_total,
            settle_total,
            drain_cdc_total,
            pre_inv16_settle_total,
            live_mirrors_total,
            assert_quiescent_total,
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

/// One SQL text fired more than once in a transition window.
///
/// `distinct_bindings` (from the `params_fp` span attribute) separates the
/// two very different smells that share a SQL text:
/// - `distinct_bindings == 1`: the same statement + bindings ran twice —
///   definitely redundant work.
/// - `distinct_bindings > 1`: a parameterized statement fanned out over
///   different bindings (e.g. one render per sidebar) — possibly a real N+1,
///   possibly legitimate; judge by the count.
#[derive(Debug, Clone)]
pub struct DuplicateSql {
    pub sql: String,
    pub count: usize,
    pub distinct_bindings: usize,
}

/// Find SQL texts that appear more than once (potential N+1 pattern),
/// sorted by count descending.
fn find_duplicate_sql<'a>(sql_spans: impl Iterator<Item = &'a SpanData>) -> Vec<DuplicateSql> {
    let mut counts: HashMap<String, (usize, std::collections::HashSet<String>)> = HashMap::new();
    for span in sql_spans {
        if let Some(sql) = sql_attr(span) {
            let entry = counts.entry(sql).or_default();
            entry.0 += 1;
            // DDL spans don't carry params_fp; treat them as one binding.
            let fp = span_attr(span, "params_fp").unwrap_or_else(|| "-".into());
            entry.1.insert(fp);
        }
    }
    let mut duplicates: Vec<DuplicateSql> = counts
        .into_iter()
        .filter(|(_, (count, _))| *count > 1)
        .map(|(sql, (count, fps))| DuplicateSql {
            sql,
            count,
            distinct_bindings: fps.len(),
        })
        .collect();
    duplicates.sort_by(|a, b| b.count.cmp(&a.count));
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
    /// SQL texts fired more than once. Potential N+1 patterns — see
    /// [`DuplicateSql`] for how `distinct_bindings` separates redundant
    /// re-execution from parameterized fan-out.
    pub duplicate_sql: Vec<DuplicateSql>,

    // ── Render metrics (from "frontend.render" spans) ────────────
    /// Total frontend render spans
    pub render_count: usize,
    /// Per-component render counts: (component_name, count), sorted by count
    /// descending
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
    /// Time spent inside the inv-viewmodel-snapshot reactive.watch + drain
    /// block (sut.rs:2820).
    pub inv10_watch_drain: Duration,
    /// Time spent inside `wait_for_org_files_stable` (called from both apply
    /// and check).
    pub wait_files_stable: Duration,
    /// Cumulative time inside `events.mark_processed` (the suspected N+1
    /// update).
    pub mark_processed_total: Duration,
    /// Number of `events.mark_processed` calls in this transition.
    pub mark_processed_count: usize,
    /// Total time inside `apply_transition_async` (the SUT-side of a
    /// transition).
    pub apply_transition_total: Duration,
    /// Total time inside `run_invariant_registry` (post-transition assertions).
    pub check_invariants_total: Duration,
    /// Time inside `settle_before_invariants` — the Loro→SQL convergence poll
    /// (`pbt.settle` span). Budgeted as the `settle_ms` NFR metric.
    pub settle_total: Duration,
    /// Total time inside `drain_cdc_events` + `drain_region_cdc_events`
    /// (1s/200ms timeouts).
    pub drain_cdc_total: Duration,
    /// Time inside the `pbt.pre_inv16_settle` block in `apply_transition_async`
    /// (loro/cdc quiescence + mirror drain). The bulk of `apply` for non-file
    /// txns.
    pub pre_inv16_settle_total: Duration,
    /// Time inside `wait_for_live_data_mirrors` (drains the SUT block+focus
    /// mirrors; child of `pre_inv16_settle`). Two mirrors at 50ms quiet each.
    pub live_mirrors_total: Duration,
    /// Time inside `assert_cdc_quiescent` (post-settle no-churn guard).
    pub assert_quiescent_total: Duration,
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
    /// (sql_text_truncated, count) for "execute_ddl"/"execute_ddl_with_deps"
    /// spans
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

/// One entry in a `QueryOriginBreakdown`: a span ancestor chain and the
/// number of `query` spans whose call path matches it (plus their cumulative
/// wall-clock cost). Chains are stored leaf-last (`outermost ▸ … ▸ query`)
/// so the output reads top-down.
#[derive(Debug, Clone)]
pub struct QueryOriginRow {
    pub chain: Vec<String>,
    pub count: usize,
    pub total_duration: Duration,
}

/// Per-origin breakdown of `query` spans. Each row groups queries by their
/// full ancestor chain so it's obvious which subsystem entered the SQL path.
///
/// `unparented` is the count of `query` spans whose `parent_span_id ==
/// SpanId::INVALID` *and* are not themselves named differently — i.e.
/// fired from a context with no enclosing instrumented span. That bucket is
/// the prime suspect for the "1600 mystery queries" investigation: any
/// background task started without `.instrument(..)` lands here.
#[derive(Debug, Clone)]
pub struct QueryOriginBreakdown {
    pub rows: Vec<QueryOriginRow>,
    pub total_queries: usize,
    pub total_duration: Duration,
}

impl SpanCollector {
    /// Group `query` spans by their full ancestor chain.
    ///
    /// For every span named `"query"`, walk `parent_span_id` upward until the
    /// chain terminates (root span or unknown parent), build the
    /// outermost-first chain of span names, and bucket the (count, duration)
    /// pair under that chain. Rows are sorted by descending total duration —
    /// the top entries are the highest-leverage targets for follow-up 5.
    pub fn queries_by_origin(&self) -> QueryOriginBreakdown {
        use opentelemetry::trace::SpanId;

        let spans = self.finished_spans();
        let by_id: HashMap<SpanId, &SpanData> = spans
            .iter()
            .map(|s| (s.span_context.span_id(), s))
            .collect();

        let mut buckets: HashMap<Vec<String>, (usize, Duration)> = HashMap::new();
        let mut total_queries = 0usize;
        let mut total_duration = Duration::ZERO;

        for span in spans.iter().filter(|s| s.name.as_ref() == "query") {
            // Walk to the root (or an unknown parent) building the chain.
            // Skip the leaf "query" itself — every row in the breakdown
            // corresponds to query spans, so it's redundant.
            let mut chain: Vec<String> = Vec::new();
            let mut current = span;
            while current.parent_span_id != SpanId::INVALID {
                if let Some(parent) = by_id.get(&current.parent_span_id) {
                    chain.push(parent.name.as_ref().to_string());
                    current = parent;
                } else {
                    chain.push("<unknown-parent>".into());
                    break;
                }
            }
            // chain is currently leaf-first; reverse to outermost-first.
            chain.reverse();
            if chain.is_empty() {
                chain.push("<no-parent>".into());
            }

            let duration = span_duration(span);
            total_queries += 1;
            total_duration += duration;
            let entry = buckets.entry(chain).or_insert((0, Duration::ZERO));
            entry.0 += 1;
            entry.1 += duration;
        }

        let mut rows: Vec<QueryOriginRow> = buckets
            .into_iter()
            .map(|(chain, (count, total_duration))| QueryOriginRow {
                chain,
                count,
                total_duration,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.total_duration
                .cmp(&a.total_duration)
                .then(b.count.cmp(&a.count))
        });

        QueryOriginBreakdown {
            rows,
            total_queries,
            total_duration,
        }
    }
}

impl std::fmt::Display for QueryOriginBreakdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "  QUERIES BY ORIGIN ({} chains, {} total queries, {:.2?} total):",
            self.rows.len(),
            self.total_queries,
            self.total_duration
        )?;
        for row in &self.rows {
            let chain = if row.chain.is_empty() {
                "<empty-chain>".to_string()
            } else {
                row.chain.join(" ▸ ")
            };
            writeln!(
                f,
                "    {count:>4}× ({dur:>9.2?})  {chain}",
                count = row.count,
                dur = row.total_duration,
                chain = chain,
            )?;
        }
        Ok(())
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

/// Write collected spans as folded stacks (compatible with flamegraph.pl /
/// inferno).
///
/// Each line: `ancestor;parent;span_name duration_us`
/// Open the output with `inferno-flamegraph` or `speedscope` for visualization.
pub fn write_folded_stacks(spans: &[SpanData], path: &std::path::Path) {
    use std::io::Write;

    use opentelemetry::trace::SpanId;

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

#[cfg(test)]
mod capture_tests {
    use tracing_subscriber::Layer as _;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    /// The ERROR-capture layer records ERROR events and IGNORES lower levels —
    /// verified against a THREAD-LOCAL subscriber + local sink
    /// (`with_default`), so it never touches the process-global collector
    /// other tests read.
    #[test]
    fn error_capture_layer_records_only_error_events() {
        let sink: ProblemSink = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(
            ErrorCaptureLayer { sink: sink.clone() }
                .with_filter(tracing_subscriber::filter::LevelFilter::ERROR),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("ignored warning");
            tracing::error!("captured boom");
        });
        let got = sink.lock().unwrap();
        assert_eq!(
            got.len(),
            1,
            "only the ERROR event is recorded; got {got:?}"
        );
        assert_eq!(got[0].kind, ProblemKind::ErrorLog);
        assert!(
            got[0].message.contains("captured boom"),
            "message captured: {:?}",
            got[0].message
        );
    }
}
