use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::task::JoinHandle;

static MONITOR_ACTIVE: AtomicBool = AtomicBool::new(false);

const INTERVAL_SECS: u64 = 30;
const GROWTH_WARN_MB: f64 = 100.0;
const GROWTH_ALERT_MB: f64 = 500.0;

pub struct MemoryMonitorHandle {
    _task: JoinHandle<()>,
}

impl MemoryMonitorHandle {
    /// Start a background task that logs RSS every 30 seconds.
    ///
    /// Detects sustained growth and logs warnings when memory increases
    /// by >100MB between samples, or alerts at >500MB growth.
    /// Only one monitor runs at a time (idempotent).
    pub fn start() -> Option<Self> {
        if MONITOR_ACTIVE.swap(true, Ordering::SeqCst) {
            tracing::debug!("[MemoryMonitor] Already running, skipping");
            return None;
        }

        let task = tokio::spawn(async move {
            let mut prev_mb: Option<f64> = None;
            let mut baseline_mb: Option<f64> = None;

            loop {
                tokio::time::sleep(Duration::from_secs(INTERVAL_SECS)).await;

                let current_mb = match memory_stats::memory_stats() {
                    Some(stats) => stats.physical_mem as f64 / (1024.0 * 1024.0),
                    None => {
                        tracing::warn!("[MemoryMonitor] Failed to read memory stats");
                        continue;
                    }
                };

                if baseline_mb.is_none() {
                    baseline_mb = Some(current_mb);
                }
                let since_baseline = current_mb - baseline_mb.unwrap();

                if let Some(prev) = prev_mb {
                    let delta = current_mb - prev;
                    if delta > GROWTH_ALERT_MB {
                        tracing::error!(
                            "[MemoryMonitor] ALERT: RSS {current_mb:.1}MB (+{delta:.1}MB in {INTERVAL_SECS}s, +{since_baseline:.1}MB since start)"
                        );
                    } else if delta > GROWTH_WARN_MB {
                        tracing::warn!(
                            "[MemoryMonitor] RSS {current_mb:.1}MB (+{delta:.1}MB in {INTERVAL_SECS}s, +{since_baseline:.1}MB since start)"
                        );
                    } else {
                        tracing::info!(
                            "[MemoryMonitor] RSS {current_mb:.1}MB (delta {delta:+.1}MB, +{since_baseline:.1}MB since start)"
                        );
                    }
                } else {
                    tracing::info!("[MemoryMonitor] Baseline RSS: {current_mb:.1}MB");
                }

                prev_mb = Some(current_mb);
            }
        });

        Some(Self { _task: task })
    }
}

impl Drop for MemoryMonitorHandle {
    fn drop(&mut self) {
        MONITOR_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// Chrome Trace Event profiler. Enable with `--features chrome-trace`.
///
/// Produces a JSON file in Chrome Trace Event format, viewable in:
/// - Firefox Profiler: https://profiler.firefox.com/
/// - chrome://tracing
/// - Perfetto: https://ui.perfetto.dev/
///
/// Usage in main.rs:
/// ```rust,ignore
/// fn main() {
///     #[cfg(feature = "chrome-trace")]
///     let _trace_guard = holon_frontend::memory_monitor::chrome_trace::layer();
///     // ... set up tracing subscriber with the layer ...
/// }
/// ```
#[cfg(feature = "chrome-trace")]
pub mod chrome_trace {
    use tracing_chrome::ChromeLayerBuilder;
    pub use tracing_chrome::FlushGuard;
    use tracing_subscriber::registry::LookupSpan;

    /// Create a chrome trace layer and flush guard.
    ///
    /// The layer should be added to your tracing subscriber. The guard MUST
    /// be held alive — when dropped it flushes and writes the trace file.
    ///
    /// Output path defaults to `./trace-{timestamp}.json`, override with
    /// `CHROME_TRACE_FILE` env var.
    pub fn layer<S>() -> (tracing_chrome::ChromeLayer<S>, FlushGuard)
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
    {
        let file_path = std::env::var("CHROME_TRACE_FILE").unwrap_or_else(|_| {
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            format!("trace-{ts}.json")
        });

        let (layer, guard) = ChromeLayerBuilder::new()
            .file(file_path.clone())
            .include_args(true)
            .build();

        eprintln!("[ChromeTrace] Recording to {file_path} — stop the app to flush");
        (layer, guard)
    }
}

/// dhat heap profiler. Enable with `--features heap-profile`.
///
/// Usage in main.rs:
/// ```rust,ignore
/// fn main() {
///     #[cfg(feature = "heap-profile")]
///     let _profiler = holon_frontend::memory_monitor::heap_profile::start();
///     // ... rest of app ...
/// }
/// ```
///
/// The profiler writes `dhat-heap.json` when:
/// - The guard is dropped (normal main() return), OR
/// - The process receives Ctrl+C / SIGINT
///
/// Open the output at: https://nnethercote.github.io/dh_view/dh_view.html
#[cfg(feature = "heap-profile")]
pub mod heap_profile {
    use std::sync::Mutex;

    #[global_allocator]
    static ALLOC: dhat::Alloc = dhat::Alloc;

    static PROFILER: Mutex<Option<dhat::Profiler>> = Mutex::new(None);

    pub struct ProfilerGuard;

    impl Drop for ProfilerGuard {
        fn drop(&mut self) {
            if let Ok(mut lock) = PROFILER.lock() {
                if let Some(profiler) = lock.take() {
                    drop(profiler);
                    eprintln!("[HeapProfile] dhat-heap.json written");
                }
            }
        }
    }

    /// Start the dhat heap profiler. Returns a guard — when dropped, writes
    /// `dhat-heap.json`. Also installs a Ctrl+C handler to ensure the file
    /// is written even if the app doesn't return from main() cleanly.
    pub fn start() -> ProfilerGuard {
        let profiler = dhat::Profiler::new_heap();
        *PROFILER.lock().unwrap() = Some(profiler);

        // Ensure dhat writes output even when the process is killed with Ctrl+C
        // or when a GUI framework calls exit() without returning from main().
        ctrlc::set_handler(|| {
            eprintln!("[HeapProfile] Caught signal, writing dhat-heap.json...");
            if let Ok(mut lock) = PROFILER.lock() {
                if let Some(profiler) = lock.take() {
                    drop(profiler);
                    eprintln!("[HeapProfile] dhat-heap.json written");
                }
            }
            std::process::exit(0);
        })
        .expect("Failed to set Ctrl+C handler");

        eprintln!("[HeapProfile] dhat profiler active — Ctrl+C to write dhat-heap.json");
        ProfilerGuard
    }
}
