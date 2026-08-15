//! Unified logging via `HOLON_LOG` environment variable.
//!
//! Format: comma-separated destinations, optionally suffixed with `:json` for
//! structured JSON output:
//!
//! - `stderr` — human-readable `fmt` layer to stderr (default if unset)
//! - `stdout` — human-readable `fmt` layer to stdout
//! - `file:///path/to/log` — human-readable `fmt` layer to file (no ANSI)
//! - `stderr:json` — JSON lines to stderr
//! - `file:///path/to/log:json` — JSON lines to file
//! - `otlp` — OpenTelemetry OTLP exporter (reads `OTEL_EXPORTER_OTLP_ENDPOINT`,
//!   default `http://localhost:4318`)
//!
//! Examples:
//! ```text
//! HOLON_LOG=stderr                          # default
//! HOLON_LOG=stdout,otlp                     # human on stdout + structured to collector
//! HOLON_LOG=file:///tmp/holon.log,otlp      # file + collector
//! HOLON_LOG=file:///tmp/holon.json:json     # JSON to file (for analysis scripts)
//! HOLON_LOG=stderr:json                     # JSON to stderr
//! HOLON_LOG=otlp                            # collector only
//! ```
//!
//! `RUST_LOG` controls filtering for all destinations. The `holon_latency`
//! target is the one exception: its events are INFO-level (they must survive
//! `tracing/release_max_level_info` in release builds) and would otherwise be
//! printed by the workspace-wide `holon=info` directive, so the target is held
//! at `warn` unless `HOLON_LATENCY_SLO=1` or an explicit `RUST_LOG` directive
//! names it.
// TODO: Why is this in holon-frontend? Logging is not frontend-specific. Is
// there a duplicate implementation outside of holon-frontend?
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const HOLON_LOG_ENV: &str = "HOLON_LOG";
const DEFAULT_FILTER: &str = "holon_gpui=info,holon=info,holon_tui=info";
const LATENCY_TARGET: &str = "holon_latency";

/// Initialize tracing from `HOLON_LOG` env var.
///
/// Call once at startup. The returned guard keeps file handles and OTel
/// providers alive — drop it to flush and shut down.
pub fn init() -> LogGuard {
    let destinations = parse_destinations();
    init_with_destinations(&destinations)
}

/// Initialize with an explicit destination string (for programmatic use).
pub fn init_from(spec: &str) -> LogGuard {
    let destinations: Vec<LogDest> = spec.split(',').map(parse_single_dest).collect();
    init_with_destinations(&destinations)
}

pub struct LogGuard {
    #[cfg(not(target_arch = "wasm32"))]
    _file_guards: Vec<tracing_appender::non_blocking::WorkerGuard>,
    #[cfg(feature = "chrome-trace")]
    _chrome_trace_guard: Option<crate::memory_monitor::chrome_trace::FlushGuard>,
}

/// The per-stage `holon_latency` events are emitted at INFO so they survive
/// `tracing/release_max_level_info` (see [`latency_events_enabled`]). INFO also
/// means the workspace-wide `holon=info` directive would print all of them by
/// default — hundreds per boot — so the target is held at `warn` unless the
/// opt-in is on. An explicit `holon_latency` directive in `RUST_LOG` always
/// wins.
fn env_filter() -> EnvFilter {
    env_filter_with_default(DEFAULT_FILTER)
}

/// The single filter-building entry point: `RUST_LOG` when set, else
/// `default_spec`, always with the `holon_latency` directive applied.
///
/// Every subscriber in the workspace must go through this — a self-built
/// `EnvFilter` inherits none of the above and floods its output with the INFO
/// stage events. Enforced by
/// `latency_target_is_suppressed_by_every_filter_builder`.
pub fn env_filter_with_default(default_spec: &str) -> EnvFilter {
    let base = std::env::var("RUST_LOG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default_spec.to_string());
    let spec = filter_spec(&base, latency_events_enabled());
    EnvFilter::builder()
        .parse(&spec)
        .unwrap_or_else(|e| panic!("invalid RUST_LOG filter '{spec}': {e}"))
}

fn filter_spec(base: &str, latency_on: bool) -> String {
    if names_latency_target(base) {
        return base.to_string();
    }
    // `warn` rather than `off`: the target also carries fail-loud disclosures
    // (`stage=e2e_expired`), which must stay visible without the opt-in.
    let level = if latency_on { "info" } else { "warn" };
    format!("{base},{LATENCY_TARGET}={level}")
}

/// Whether `spec` already carries a directive FOR the target — a per-directive
/// check, so a span or field name that merely contains the string does not
/// silently disable the default.
fn names_latency_target(spec: &str) -> bool {
    spec.split(',').any(|directive| {
        let target = directive.split('=').next().unwrap_or_default();
        let target = target.split('[').next().unwrap_or_default();
        target.trim() == LATENCY_TARGET
    })
}

/// Whether the `holon_latency` stage events reach the log. Opt-in via
/// `HOLON_LATENCY_SLO` in every profile — the events are INFO-level, so
/// gating them on `debug_assertions` (the way the oracle LAYER is gated)
/// would flood the default dev log with per-batch timing lines.
fn latency_events_enabled() -> bool {
    matches!(
        std::env::var("HOLON_LATENCY_SLO").as_deref(),
        Ok("1") | Ok("true") | Ok("on")
    )
}

/// Whether to install the live latency-SLO oracle layer. `HOLON_ORACLES=off`
/// opts out everywhere. Otherwise on by default in debug; in release it is
/// opt-in via `HOLON_LATENCY_SLO` (`1`/`true`/`on`) so dogfooding a release
/// build can still surface SLO breaches.
fn latency_slo_enabled() -> bool {
    if !holon_oracles::OracleMode::from_env().enabled() {
        return false;
    }
    if cfg!(debug_assertions) {
        return true;
    }
    matches!(
        std::env::var("HOLON_LATENCY_SLO").as_deref(),
        Ok("1") | Ok("true") | Ok("on")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFormat {
    Human,
    Json,
}

#[derive(Debug)]
enum LogDest {
    Stderr(LogFormat),
    Stdout(LogFormat),
    File(String, LogFormat),
    #[cfg(feature = "otel")]
    Otlp,
}

fn parse_destinations() -> Vec<LogDest> {
    match std::env::var(HOLON_LOG_ENV) {
        Ok(val) if !val.is_empty() => val.split(',').map(parse_single_dest).collect(),
        _ => vec![LogDest::Stderr(LogFormat::Human)],
    }
}

fn parse_single_dest(s: &str) -> LogDest {
    let s = s.trim();

    // Handle file:// destinations — the :json suffix comes after the path
    if let Some(rest) = s.strip_prefix("file://") {
        return if let Some(path) = rest.strip_suffix(":json") {
            LogDest::File(path.to_string(), LogFormat::Json)
        } else {
            LogDest::File(rest.to_string(), LogFormat::Human)
        };
    }

    match s {
        "stderr" => LogDest::Stderr(LogFormat::Human),
        "stderr:json" => LogDest::Stderr(LogFormat::Json),
        "stdout" => LogDest::Stdout(LogFormat::Human),
        "stdout:json" => LogDest::Stdout(LogFormat::Json),
        #[cfg(feature = "otel")]
        "otlp" => LogDest::Otlp,
        #[cfg(not(feature = "otel"))]
        "otlp" => panic!("HOLON_LOG=otlp requires the 'otel' cargo feature"),
        other => panic!("Unknown HOLON_LOG destination: '{other}'"),
    }
}

fn init_with_destinations(destinations: &[LogDest]) -> LogGuard {
    use tracing_subscriber::Layer;

    #[cfg(not(target_arch = "wasm32"))]
    let mut file_guards = Vec::new();
    let mut layers: Vec<Box<dyn Layer<tracing_subscriber::Registry> + Send + Sync>> = Vec::new();

    for dest in destinations {
        match dest {
            LogDest::Stderr(LogFormat::Human) => {
                layers.push(Box::new(
                    fmt::layer()
                        .with_writer(std::io::stderr)
                        .with_ansi(true)
                        .with_filter(env_filter()),
                ));
            }
            LogDest::Stderr(LogFormat::Json) => {
                layers.push(Box::new(
                    fmt::layer()
                        .json()
                        .with_span_list(true)
                        .with_writer(std::io::stderr)
                        .with_filter(env_filter()),
                ));
            }
            LogDest::Stdout(LogFormat::Human) => {
                layers.push(Box::new(
                    fmt::layer()
                        .with_writer(std::io::stdout)
                        .with_ansi(true)
                        .with_filter(env_filter()),
                ));
            }
            LogDest::Stdout(LogFormat::Json) => {
                layers.push(Box::new(
                    fmt::layer()
                        .json()
                        .with_span_list(true)
                        .with_writer(std::io::stdout)
                        .with_filter(env_filter()),
                ));
            }
            // wasm32 has no writer thread and no filesystem to write to, so the
            // request is unsatisfiable rather than degraded. Panicking matches
            // the two arms below (`otlp` without the feature, unknown
            // destination) and is the only disclosure that survives
            // wasm32-unknown-unknown, where stderr is a discard sink.
            #[cfg(target_arch = "wasm32")]
            LogDest::File(path, _) => {
                panic!("HOLON_LOG=file://{path} is unavailable on wasm32: no file logging");
            }
            #[cfg(not(target_arch = "wasm32"))]
            LogDest::File(path, format) => {
                let file = std::fs::File::create(path)
                    .unwrap_or_else(|e| panic!("Cannot create log file '{path}': {e}"));
                let (non_blocking, guard) = tracing_appender::non_blocking(file);
                file_guards.push(guard);
                match format {
                    LogFormat::Human => {
                        layers.push(Box::new(
                            fmt::layer()
                                .with_writer(non_blocking)
                                .with_ansi(false)
                                .with_filter(env_filter()),
                        ));
                    }
                    LogFormat::Json => {
                        layers.push(Box::new(
                            fmt::layer()
                                .json()
                                .with_span_list(true)
                                .with_writer(non_blocking)
                                .with_filter(env_filter()),
                        ));
                    }
                }
            }
            #[cfg(feature = "otel")]
            LogDest::Otlp => {
                layers.push(Box::new(init_otlp_layer().with_filter(env_filter())));
            }
        }
    }

    // tokio-console async-stall profiler. `spawn()` starts the gRPC aggregator
    // on its own background thread so the `tokio-console` CLI can attach. Only
    // records task busy/idle data when built with `--cfg tokio_unstable` (plus
    // tokio's `tracing` feature, pulled in by the `tokio-console` cargo
    // feature). Bind address overridable via `TOKIO_CONSOLE_BIND`.
    #[cfg(feature = "tokio-console")]
    layers.push(Box::new(
        console_subscriber::ConsoleLayer::builder()
            .with_default_env()
            .spawn(),
    ));

    #[cfg(feature = "chrome-trace")]
    let (chrome_layer, chrome_guard) = crate::memory_monitor::chrome_trace::layer();

    // Live-oracle latency SLO: watch the existing `holon_latency` stage events
    // (dispatch/rows always; projection when CRDT is on); a stage slower than
    // the SLO becomes a violation (banner + error log). Always compiled;
    // enabled by default in debug, opt-in in release via `HOLON_LATENCY_SLO=1`
    // so dogfooding a release build can still catch SLO breaches.
    // HOLON_ORACLES=off opts out everywhere; HOLON_ORACLES_SLO_MS tunes.
    if latency_slo_enabled() {
        layers.push(Box::new(holon_oracles::latency::LatencySloLayer::from_env()));
    }

    let registry = tracing_subscriber::registry().with(layers);

    #[cfg(feature = "chrome-trace")]
    registry.with(chrome_layer).init();

    #[cfg(not(feature = "chrome-trace"))]
    registry.init();

    install_panic_hook();

    LogGuard {
        #[cfg(not(target_arch = "wasm32"))]
        _file_guards: file_guards,
        #[cfg(feature = "chrome-trace")]
        _chrome_trace_guard: Some(chrome_guard),
    }
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };

        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        tracing::error!(panic.payload = %payload, panic.location = %location, "PANIC: {payload}");

        default_hook(info);
    }));
}

#[cfg(feature = "otel")]
fn init_otlp_layer() -> impl tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync {
    use opentelemetry::KeyValue;
    use opentelemetry::global;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::trace::SdkTracerProvider;

    let service_name = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "holon".to_string());

    let base_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:4318".to_string());
    let base_endpoint = base_endpoint.trim_end_matches('/').to_string();
    let traces_endpoint = format!("{base_endpoint}/v1/traces");

    let resource = Resource::builder_empty()
        .with_attributes(vec![KeyValue::new("service.name", service_name.clone())])
        .build();

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(traces_endpoint)
        .build()
        .expect("Failed to build OTLP trace exporter");

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    global::set_tracer_provider(provider);

    let tracer = global::tracer(Box::leak(service_name.into_boxed_str()) as &'static str);
    tracing_opentelemetry::OpenTelemetryLayer::new(tracer)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use tracing::Subscriber;
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::layer::Layer;
    use tracing_subscriber::prelude::*;

    use super::DEFAULT_FILTER;
    use super::EnvFilter;
    use super::filter_spec;

    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Seen {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            self.0
                .lock()
                .expect("seen lock")
                .push(event.metadata().target().to_string());
        }
    }

    /// Emit one INFO event per target through `spec`; report what survived.
    fn targets_passing(spec: &str, targets: &[&str]) -> Vec<String> {
        let seen = Seen::default();
        let subscriber =
            tracing_subscriber::registry().with(seen.clone().with_filter(EnvFilter::new(spec)));
        tracing::subscriber::with_default(subscriber, || {
            for t in targets {
                match *t {
                    "holon_latency" => tracing::info!(target: "holon_latency", stage = "x"),
                    "holon_latency@warn" => tracing::warn!(target: "holon_latency", stage = "x"),
                    "holon_core" => tracing::info!(target: "holon_core", "x"),
                    other => panic!("unhandled probe target {other}"),
                }
            }
        });
        let out = seen.0.lock().expect("seen lock").clone();
        out
    }

    /// The default (opt-out) log keeps its volume: the stage events are INFO
    /// now, and the workspace-wide `holon=info` directive would otherwise print
    /// them all. The target's fail-loud `warn!` disclosures still get through.
    #[test]
    fn default_drops_the_stage_events_but_keeps_warnings() {
        let spec = filter_spec(DEFAULT_FILTER, false);
        let probes = &["holon_latency", "holon_core", "holon_latency@warn"];
        let passed = targets_passing(&spec, probes);
        let expected = vec!["holon_core".to_string(), "holon_latency".to_string()];
        assert_eq!(passed, expected, "spec: {spec}");
    }

    /// With the opt-in the events reach the log — the point of
    /// `HOLON_LATENCY_SLO=1` on a release build.
    #[test]
    fn latency_target_passes_under_the_opt_in() {
        let spec = filter_spec(DEFAULT_FILTER, true);
        let passed = targets_passing(&spec, &["holon_latency"]);
        assert_eq!(passed, vec!["holon_latency".to_string()], "spec: {spec}");
    }

    /// An explicit directive is the user's call, opt-in or not.
    #[test]
    fn explicit_rust_log_directive_wins() {
        let base = "warn,holon_latency=info";
        assert_eq!(filter_spec(base, false), base);
        let passed = targets_passing(base, &["holon_latency"]);
        assert_eq!(passed, vec!["holon_latency".to_string()]);
    }

    /// Only a directive whose TARGET is the latency target counts as explicit —
    /// a span or field of that name, or a longer target, must not disable the
    /// default suppression.
    #[test]
    fn a_mere_mention_of_the_target_is_not_a_directive() {
        for base in ["info[holon_latency]=debug", "holon_latency_probe=debug"] {
            let spec = filter_spec(base, false);
            assert!(spec.ends_with("holon_latency=warn"), "spec: {spec}");
        }
    }
}
