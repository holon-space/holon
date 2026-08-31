//! Capture the prod `stage="e2e"` stream so a gate rung can score the latency
//! SLO in-process.
//!
//! The correlator emits its measurement from the tokio CDC actor, not from the
//! driving thread, so a thread-local subscriber never sees it. This layer rides
//! the ONE global registry [`crate::test_tracing::SpanCollector`] installs,
//! filtered to `holon_latency` exactly like
//! [`super::reseed_observer::ReseedObserverLayer`], and files each measurement
//! into a process-global [`holon_api::latency_slo::SloWindow`].
//!
//! It is DISARMED by default: an unarmed run drops every event after one atomic
//! load, so the keystone pays nothing for it. A rung arms the probe around the
//! stretch it means to measure — boot and setup samples are never in the window
//! it judges.
//!
//! Scoring goes through the same `SloWindow` the runtime `latency-slo` oracle
//! uses (`holon_oracles::latency`), which is the point: the banner the app
//! paints and the gate that fails the build are the same two numbers.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Instant;

use holon_api::latency_slo::E2eSample;
use holon_api::latency_slo::SERVICE_TIME_SLO_MS;
use holon_api::latency_slo::SloWindow;
use holon_api::latency_slo::THROUGHPUT_FLOOR_WRITES_PER_SEC;

/// Retention for a gate rung: past any rung's write count, so an armed stretch
/// keeps every sample it produced rather than judging a suffix of itself.
const PROBE_CAPACITY: usize = 4096;

/// Mean boot `matview_ddl` above which a run is refused as unjudgeable.
///
/// Wall-clock latency on this host moves more with how busy the machine is
/// than with any code change. `docs/Testing/latency-ceilings.txt` measured the
/// covariate — boot DDL against a contended storage layer tracks per-action
/// latency with Spearman +0.94 over a 19x span of it — and calibrated 30ms as
/// the cut. These rungs reuse both, so the two latency gates agree about when
/// the host was too busy to judge.
pub const MAX_CONTENTION_MS: f64 = 30.0;

static ARMED: AtomicBool = AtomicBool::new(false);
static WINDOW: OnceLock<Mutex<SloWindow>> = OnceLock::new();
/// Every `matview_ddl` duration seen this process, armed or not — the covariate
/// is a BOOT measurement, so it is collected before any rung arms the window.
static DDL_MS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn window() -> &'static Mutex<SloWindow> {
    WINDOW.get_or_init(|| {
        Mutex::new(SloWindow::new(
            PROBE_CAPACITY,
            SERVICE_TIME_SLO_MS,
            THROUGHPUT_FLOOR_WRITES_PER_SEC,
        ))
    })
}

/// Mean boot `matview_ddl`, or `None` when the process emitted none.
pub fn contention_ms() -> Option<f64> {
    let v = DDL_MS.lock().expect("slo probe ddl log poisoned");
    (!v.is_empty()).then(|| v.iter().sum::<u64>() as f64 / v.len() as f64)
}

/// The armed measurement window. Dropping it disarms the probe, so a rung that
/// panics mid-measurement cannot leave the probe recording into the next one.
pub struct SloProbe {
    _private: (),
}

impl SloProbe {
    /// Start recording into an EMPTY window. The caller must already have
    /// installed the global subscriber (any `ComposedSut` boot does).
    pub fn arm() -> Self {
        window().lock().expect("slo probe window poisoned").clear();
        ARMED.store(true, Ordering::Release);
        Self { _private: () }
    }

    /// The samples recorded so far, scored as the two SLO rungs.
    pub fn snapshot(&self) -> SloWindow {
        let live = window().lock().expect("slo probe window poisoned");
        let mut out = SloWindow::new(
            PROBE_CAPACITY,
            SERVICE_TIME_SLO_MS,
            THROUGHPUT_FLOOR_WRITES_PER_SEC,
        );
        for s in live.samples() {
            out.record(s.clone());
        }
        out
    }
}

impl Drop for SloProbe {
    fn drop(&mut self) {
        ARMED.store(false, Ordering::Release);
    }
}

/// The `holon_latency` layer. Register once, alongside the reseed observer.
pub struct SloProbeLayer;

#[derive(Default)]
struct E2eVisitor {
    stage: Option<String>,
    action: Option<String>,
    ms: Option<u64>,
    in_flight: Option<u64>,
    backlog: Option<u64>,
}

impl tracing::field::Visit for E2eVisitor {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        match field.name() {
            "ms" => self.ms = Some(value),
            "in_flight" => self.in_flight = Some(value),
            "backlog" => self.backlog = Some(value),
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        if value >= 0 {
            self.record_u64(field, value as u64);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "stage" => self.stage = Some(value.to_string()),
            "action" => self.action = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let raw = format!("{value:?}");
        self.record_str(field, raw.trim_matches('"'));
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SloProbeLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "holon_latency" {
            return;
        }
        let mut v = E2eVisitor::default();
        event.record(&mut v);
        // The contention covariate is a BOOT measurement, so it is collected
        // whether or not a rung has armed the window yet.
        if v.stage.as_deref() == Some("matview_ddl") {
            if let Some(ms) = v.ms {
                DDL_MS.lock().expect("slo probe ddl log poisoned").push(ms);
            }
            return;
        }
        if !ARMED.load(Ordering::Acquire) || v.stage.as_deref() != Some("e2e") {
            return;
        }
        // A rung that silently scored an unscoreable sample would be the exact
        // vacuous-pass this gate exists to prevent, so a missing queue depth is
        // a panic on the emitting thread rather than a defaulted `1`.
        let (Some(ms), Some(in_flight), Some(backlog)) = (v.ms, v.in_flight, v.backlog) else {
            panic!(
                "slo probe: an `e2e` event lacked ms/in_flight/backlog (ms={:?} in_flight={:?} \
                 backlog={:?}) — the correlator's emission and this probe have diverged",
                v.ms, v.in_flight, v.backlog
            );
        };
        window()
            .lock()
            .expect("slo probe window poisoned")
            .record(E2eSample {
                action: v.action.unwrap_or_else(|| "?".to_string()),
                ms,
                in_flight: in_flight as usize,
                backlog: backlog as usize,
                delivered_at: Instant::now(),
            });
    }
}
