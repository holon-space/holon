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
//!            default `http://localhost:4318`)
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
//! `RUST_LOG` controls filtering for all destinations.

use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

const HOLON_LOG_ENV: &str = "HOLON_LOG";
const DEFAULT_FILTER: &str = "holon_gpui=info,holon=info,holon_tui=info";

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
    _file_guards: Vec<tracing_appender::non_blocking::WorkerGuard>,
    #[cfg(feature = "chrome-trace")]
    _chrome_trace_guard: Option<crate::memory_monitor::chrome_trace::FlushGuard>,
}

fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_FILTER.into())
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
    if s.starts_with("file://") {
        let rest = &s["file://".len()..];
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

    #[cfg(feature = "chrome-trace")]
    let (chrome_layer, chrome_guard) = crate::memory_monitor::chrome_trace::layer();

    let registry = tracing_subscriber::registry().with(layers);

    #[cfg(feature = "chrome-trace")]
    registry.with(chrome_layer).init();

    #[cfg(not(feature = "chrome-trace"))]
    registry.init();

    install_panic_hook();

    LogGuard {
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
    use opentelemetry::global;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;

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
