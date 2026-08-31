//! Latency SLO oracle — a `tracing_subscriber` Layer over the existing
//! `holon_latency` instrumentation.
//!
//! The pipeline already emits per-stage timing events
//! `tracing::info!(target: "holon_latency", stage = "...", ms = ...)`:
//!
//! - `dispatch` (holon-frontend `reactive.rs`) — user action enters the op
//!   pipeline. Fires in every configuration.
//! - `rows` (holon-api `live_data.rs`) — a CDC batch from the matview lands in
//!   the reactive mirror (change becomes visible to the view model). Fires in
//!   every configuration.
//! - `projection` (holon-loro `LoroSyncController`) — full Loro→SQL projection
//!   pass. Fires ONLY when CRDT sync is enabled (`crdt_enabled`); the default
//!   desktop config never emits it.
//!
//! The `e2e` stage (holon-api `latency_e2e.rs`) closes the interaction on its
//! delivered row and carries the verdict; the others are diagnostic components.
//!
//! # What this layer judges — and why not a single `ms`
//!
//! Firing on any one over-budget `e2e` event scores the wrong quantity: `ms` is
//! service time PLUS the wait behind everything queued ahead, so a pipeline
//! driven faster than it drains paints a violation per keystroke while being
//! perfectly healthy (five banners off one queue ramp, BugFunnel 2026-08-31).
//! Per Martin's ruling D50.a the layer accumulates events into a
//! [`holon_api::latency_slo::SloWindow`] and reports its two rungs — service
//! p95 and saturated drain rate. That is the SAME type the land gate scores
//! (`crates/holon-integration-tests/tests/latency_slo_gate.rs`), so the banner
//! and the gate cannot disagree about either number.
//!
//! Violations are edge-triggered: one banner when a rung turns red, not one per
//! event. Boundary disclosure: a rung below its sample floor is `Unjudged` and
//! paints nothing — the window has not seen enough to accuse the pipeline.
//!
//! Zero new instrumentation, zero hot-path cost beyond reading an already-
//! emitted event's fields. Threshold tunable via `HOLON_ORACLES_SLO_MS`.
//!
//! The events must stay at INFO or above: the turso fork's `workspace-hack`
//! enables `tracing/release_max_level_info`, which compiles every `debug!`
//! callsite out of release builds — the layer would then see nothing in the
//! build that is actually dogfooded. Guarded by
//! `latency_events_are_emitted_above_the_release_level_ceiling`.

use std::sync::Mutex;
use std::time::Instant;
use std::time::SystemTime;

use holon_api::latency_slo::E2eSample;
use holon_api::latency_slo::RungVerdict;
use holon_api::latency_slo::SloWindow;
use holon_api::latency_slo::THROUGHPUT_FLOOR_WRITES_PER_SEC;
use tracing::Event;
use tracing::Metadata;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::Layer;

use crate::status::OracleStatus;
use crate::status::Violation;

pub const DEFAULT_SLO_MS: u64 = 200;
const LATENCY_TARGET: &str = "holon_latency";

/// Deliveries the running app judges over. A few hundred keystrokes is recent
/// behaviour; a whole session's worth would let an hour-old stall keep the
/// banner up after the pipeline recovered.
const WINDOW_CAPACITY: usize = 512;

pub struct LatencySloLayer {
    slo_ms: u64,
    window: Mutex<SloWindow>,
    /// Which rungs were red at the last evaluation, so a sustained breach
    /// paints one banner rather than one per delivered row.
    red: Mutex<RedRungs>,
}

#[derive(Default, Clone, Copy, PartialEq)]
struct RedRungs {
    service: bool,
    throughput: bool,
}

impl LatencySloLayer {
    pub fn new(slo_ms: u64) -> Self {
        Self {
            slo_ms,
            window: Mutex::new(SloWindow::new(
                WINDOW_CAPACITY,
                slo_ms,
                THROUGHPUT_FLOOR_WRITES_PER_SEC,
            )),
            red: Mutex::new(RedRungs::default()),
        }
    }

    /// Add one delivery to the window and paint a banner for each rung that
    /// has just turned red. A rung already red stays red silently; a rung that
    /// recovers clears its edge so a later breach speaks again.
    fn record_and_judge(&self, sample: E2eSample) {
        let (service, throughput, report) = {
            let mut window = self.window.lock().expect("latency-slo window poisoned");
            window.record(sample);
            (
                window.service_verdict(),
                window.throughput_verdict(),
                window.report(),
            )
        };
        let now = RedRungs {
            service: service.is_fail(),
            throughput: throughput.is_fail(),
        };
        let mut red = self.red.lock().expect("latency-slo edge state poisoned");
        let was = *red;
        *red = now;
        drop(red);

        if now.service && !was.service {
            let RungVerdict::Fail { measured, n } = service else {
                unreachable!("is_fail() implies Fail")
            };
            self.raise(format!(
                "[latency-slo] SERVICE TIME p95 {measured:.0}ms over n={n} interactions \
                 dispatched with an empty queue (SLO: p95 <{}ms). {report}",
                self.slo_ms,
            ));
        }
        if now.throughput && !was.throughput {
            let RungVerdict::Fail { measured, n } = throughput else {
                unreachable!("is_fail() implies Fail")
            };
            self.raise(format!(
                "[latency-slo] THROUGHPUT {measured:.1} writes/s while saturated over {n} \
                 intervals (floor: {THROUGHPUT_FLOOR_WRITES_PER_SEC:.1}/s). {report}",
            ));
        }
    }

    fn raise(&self, message: String) {
        // Loud in the log channel too. Different target than the events this
        // layer filters on, so re-entry terminates immediately.
        tracing::error!(target: "holon_oracles", oracle = "latency-slo", "ORACLE VIOLATION: {message}");
        OracleStatus::global().push_latency(Violation {
            oracle: "latency-slo",
            message,
            at: SystemTime::now(),
        });
    }

    /// Threshold from `HOLON_ORACLES_SLO_MS` (default [`DEFAULT_SLO_MS`]).
    pub fn from_env() -> Self {
        let slo_ms = match std::env::var("HOLON_ORACLES_SLO_MS") {
            Ok(s) => s
                .parse()
                .unwrap_or_else(|e| panic!("HOLON_ORACLES_SLO_MS must be a number of ms: {e}")),
            Err(_) => DEFAULT_SLO_MS,
        };
        Self::new(slo_ms)
    }
}

impl Default for LatencySloLayer {
    fn default() -> Self {
        Self::new(DEFAULT_SLO_MS)
    }
}

#[derive(Default)]
struct LatencyFields {
    stage: Option<String>,
    ms: Option<u64>,
    ops: Option<u64>,
    blocks: Option<u64>,
    action: Option<String>,
    block: Option<String>,
    in_flight: Option<u64>,
    backlog: Option<u64>,
}

impl LatencyFields {
    fn set_str(&mut self, name: &str, value: String) {
        match name {
            "stage" => self.stage = Some(value),
            "action" => self.action = Some(value),
            "block" => self.block = Some(value),
            _ => {}
        }
    }
}

impl Visit for LatencyFields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "ms" => self.ms = Some(value),
            "ops" => self.ops = Some(value),
            "blocks" => self.blocks = Some(value),
            "in_flight" => self.in_flight = Some(value),
            "backlog" => self.backlog = Some(value),
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if value >= 0 {
            self.record_u64(field, value as u64);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.set_str(field.name(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.set_str(
            field.name(),
            format!("{value:?}").trim_matches('"').to_string(),
        );
    }
}

impl<S: Subscriber> Layer<S> for LatencySloLayer {
    /// Declare interest in `holon_latency` events even at DEBUG level, so
    /// they are dispatched regardless of the fmt layers' env filters.
    fn enabled(&self, metadata: &Metadata<'_>, _: Context<'_, S>) -> bool {
        metadata.target() == LATENCY_TARGET
    }

    /// The layer accepts any level, so it must never let a sibling layer's
    /// filter lower the subscriber's global max level below its interest.
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }

    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != LATENCY_TARGET {
            return;
        }
        let mut fields = LatencyFields::default();
        event.record(&mut fields);
        let (Some(stage), Some(ms)) = (fields.stage.as_deref(), fields.ms) else {
            return;
        };
        if stage == "e2e" {
            // PRIMARY SLO signal: true interaction -> PROJECTION-VISIBLE wall
            // time (see holon_api::latency_e2e). The `e2e` stage closes when a
            // CDC batch is applied to the reactive mirror — data available for
            // render — NOT at GPU frame-present. Anchoring the SLO verdict on
            // paint would let a backgrounded/occluded window (presents deferred
            // by the OS) manufacture multi-second false violations; only `e2e`
            // reaches this violation branch, so no paint/frame stage can ever
            // be the SLO verdict.
            //
            // A pre-D50.a build emits no queue depth. Assuming `1` would file
            // every queued sample as service time and restore the exact
            // false-banner behaviour this rung replaces, so an event without
            // the fields is dropped and disclosed rather than guessed at.
            let (Some(in_flight), Some(backlog)) = (fields.in_flight, fields.backlog) else {
                tracing::warn!(
                    target: "holon_oracles",
                    oracle = "latency-slo",
                    "[latency-slo] an `e2e` event carried no in_flight/backlog — this sample is \
                     unscoreable and the SLO rungs are running on partial evidence",
                );
                return;
            };
            self.record_and_judge(E2eSample {
                action: fields.action.unwrap_or_else(|| "?".to_string()),
                ms,
                in_flight: in_flight as usize,
                backlog: backlog as usize,
                delivered_at: Instant::now(),
            });
        } else if ms > self.slo_ms {
            // Diagnostic attribution: which pipeline stage ate the budget.
            // Warn-level (no banner) — the e2e stage carries the verdict.
            tracing::warn!(
                target: "holon_oracles",
                oracle = "latency-slo",
                "[latency-slo diagnostic] '{stage}' stage took {ms}ms (> {}ms budget){}",
                self.slo_ms,
                match (fields.ops, fields.blocks) {
                    (Some(o), Some(b)) => format!(" — {o} op(s), {b} block(s)"),
                    _ => String::new(),
                }
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::prelude::*;

    use super::*;

    /// `OracleStatus` is process-global and these tests measure it by
    /// difference, so two of them running at once would each see the other's
    /// pushes. They take turns.
    static ORACLE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Count the latency violations carrying `marker`, around a body that
    /// drives events through a fresh layer.
    fn violations_around(marker: &str, drive: impl FnOnce(LatencySloLayer)) -> usize {
        let _turn = ORACLE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = || {
            OracleStatus::global()
                .snapshot()
                .into_iter()
                .filter(|v| v.oracle == "latency-slo" && v.message.contains(marker))
                .count()
        };
        let before = count();
        drive(LatencySloLayer::new(SLO_MS));
        count() - before
    }

    /// One threshold for every test here; each test separates its violations
    /// from the others' by the unique `action` name it emits, which the rung
    /// banners carry through the window report.
    const SLO_MS: u64 = 200;

    /// Drive `n` `e2e` events through `layer`, each `ms` with the given queue
    /// depths. `in_flight = 1, backlog = 0` is the paced (service-time) shape.
    fn drive_e2e(layer: LatencySloLayer, n: usize, ms: u64, in_flight: usize, backlog: usize) {
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..n {
                tracing::info!(
                    target: "holon_latency",
                    stage = "e2e",
                    action = "set_field",
                    block = "block:x",
                    ms = ms,
                    in_flight = in_flight as u64,
                    backlog = backlog as u64,
                    "holon_latency",
                );
            }
        });
    }

    /// Drive one over-budget event of a NON-e2e stage and count its violations.
    fn violations_for_stage(stage: &'static str, marker: &str) -> usize {
        violations_around(marker, |layer| {
            let subscriber = tracing_subscriber::registry().with(layer);
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(
                    target: "holon_latency",
                    stage = stage,
                    action = "navigate",
                    block = marker,
                    ms = 5000u64,
                    "holon_latency",
                );
            });
        })
    }

    /// **D50.a, the behaviour change.** A single over-budget `e2e` event used
    /// to paint a banner. It no longer can: one sample is not a p95, and
    /// the sample that produced the loudest historical banners was a queued
    /// one.
    #[test]
    fn a_single_over_budget_e2e_event_does_not_fire() {
        let fired = violations_around("service p95", |layer| {
            drive_e2e(layer, 1, 5000, 1, 0);
        });
        assert_eq!(
            fired, 0,
            "one sample cannot be a p95 verdict — the rung needs its sample floor"
        );
    }

    /// **The dogfood false-banner case.** A healthy pipeline (10ms of real work
    /// per write) driven faster than it drains produces a queue ramp whose `ms`
    /// climbs past the SLO. The service rung must stay silent: not one of those
    /// samples was dispatched with an empty queue.
    #[test]
    fn a_queue_ramp_paints_no_service_violation() {
        let fired = violations_around("SERVICE TIME", |layer| {
            let subscriber = tracing_subscriber::registry().with(layer);
            tracing::subscriber::with_default(subscriber, || {
                for i in 0..60u64 {
                    tracing::info!(
                        target: "holon_latency",
                        stage = "e2e",
                        action = "set_field",
                        block = "block:ramp",
                        ms = 10 + 10 * i,
                        in_flight = i + 1,
                        backlog = 59 - i,
                        "holon_latency",
                    );
                }
            });
        });
        assert_eq!(
            fired, 0,
            "a queue-depth ramp is not a service-time breach — this is the false banner D50.a removes"
        );
    }

    /// A genuinely slow pipeline, driven one interaction at a time, DOES fire —
    /// and fires ONCE, not once per delivery, because the banner is edge-
    /// triggered on the rung turning red.
    #[test]
    fn sustained_slow_service_paints_exactly_one_banner() {
        let fired = violations_around("SERVICE TIME", |layer| {
            drive_e2e(layer, 40, 250, 1, 0);
        });
        assert_eq!(
            fired, 1,
            "40 over-budget paced deliveries are ONE sustained breach, not 40"
        );
    }

    /// An `e2e` event without queue depth cannot be scored. Assuming it was
    /// paced would file every queued sample as service time and restore the
    /// false banners, so it is dropped and disclosed instead.
    #[test]
    fn an_e2e_event_without_queue_depth_is_not_scored() {
        let fired = violations_around("service p95", |layer| {
            let subscriber = tracing_subscriber::registry().with(layer);
            tracing::subscriber::with_default(subscriber, || {
                for _ in 0..40 {
                    tracing::info!(
                        target: "holon_latency",
                        stage = "e2e",
                        action = "set_field",
                        block = "block:legacy",
                        ms = 5000u64,
                        "holon_latency",
                    );
                }
            });
        });
        assert_eq!(fired, 0, "unscoreable samples must not reach a verdict");
    }

    /// **The THROUGHPUT banner branch** (`record_and_judge`'s second arm).
    /// Every other test here exercises the service branch, so without this one
    /// the drain-rate half of the oracle could stop raising entirely and no
    /// test would notice.
    ///
    /// Drives a saturated stretch retiring one delivery per 150ms — ~6.7
    /// writes/s, under the floor — as batches that each leave a backlog.
    #[test]
    fn a_slow_drain_paints_a_throughput_banner() {
        let fired = violations_around("THROUGHPUT", |layer| {
            let subscriber = tracing_subscriber::registry().with(layer);
            tracing::subscriber::with_default(subscriber, || {
                for i in 0..20u64 {
                    tracing::info!(
                        target: "holon_latency",
                        stage = "e2e",
                        action = "set_field",
                        block = "block:slow-drain",
                        ms = 40u64,
                        in_flight = i + 2,
                        // Never reaches zero: the queue stays non-empty, so
                        // every gap is a saturated interval.
                        backlog = 20 - i,
                        "holon_latency",
                    );
                    // Real wall time — the drain rate is measured against the
                    // clock, so the test has to spend it.
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            });
        });
        assert_eq!(
            fired, 1,
            "a sustained slow drain must raise the THROUGHPUT banner exactly once"
        );
    }

    /// A per-stage component (`rows`) over budget is a DIAGNOSTIC only — it
    /// must NOT record an SLO violation. The `e2e` stage carries the verdict.
    #[test]
    fn component_stage_over_budget_is_diagnostic_not_violation() {
        assert_eq!(
            violations_for_stage("rows", "block:oracle-rows-marker"),
            0,
            "component stages warn-diagnose; only e2e is the SLO verdict"
        );
    }

    /// Guard against the exact regression this lane prevents: if someone
    /// re-anchored the SLO onto a GPU paint/frame stage, that stage would have
    /// to reach the violation branch. It must not — a `frame_present` event is
    /// diagnostic-only, so paint can never manufacture an SLO violation.
    #[test]
    fn frame_present_stage_cannot_be_the_slo_verdict() {
        assert_eq!(
            violations_for_stage("frame_present", "block:oracle-paint-marker"),
            0,
            "paint/frame-present must never be judged as the SLO endpoint"
        );
    }
}
