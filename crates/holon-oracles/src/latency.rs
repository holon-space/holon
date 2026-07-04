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
//! No single prod span measures interaction→projection-visible end-to-end
//! (the p95 < 200ms SLO); these stages are sequential components of that
//! path, so ANY stage exceeding the budget proves the end-to-end SLO is
//! blown. The layer therefore monitors every `holon_latency` event carrying
//! an `ms` field and reports the stage name with the measured number.
//! Boundary disclosure: a violation here is sufficient, not necessary — an
//! end-to-end breach spread thinly across stages is not caught.
//!
//! Zero new instrumentation, zero hot-path cost beyond reading an already-
//! emitted event's fields. Threshold tunable via `HOLON_ORACLES_SLO_MS`.
//!
//! The events must stay at INFO or above: the turso fork's `workspace-hack`
//! enables `tracing/release_max_level_info`, which compiles every `debug!`
//! callsite out of release builds — the layer would then see nothing in the
//! build that is actually dogfooded. Guarded by
//! `latency_events_are_emitted_above_the_release_level_ceiling`.

use std::time::SystemTime;

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

pub struct LatencySloLayer {
    slo_ms: u64,
}

impl LatencySloLayer {
    pub fn new(slo_ms: u64) -> Self {
        Self { slo_ms }
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
        if ms <= self.slo_ms {
            return;
        }
        if stage == "e2e" {
            // PRIMARY SLO signal: true interaction -> PROJECTION-VISIBLE wall
            // time (see holon_api::latency_e2e). The `e2e` stage closes when a
            // CDC batch is applied to the reactive mirror — data available for
            // render — NOT at GPU frame-present. Anchoring the SLO verdict on
            // paint would let a backgrounded/occluded window (presents deferred
            // by the OS) manufacture multi-second false violations; only `e2e`
            // reaches this violation branch, so no paint/frame stage can ever
            // be the SLO verdict. Violation = banner + error.
            let message = format!(
                "[latency-slo] interaction '{}' on {} took {ms}ms end-to-end (SLO: p95 <{}ms)",
                fields.action.as_deref().unwrap_or("?"),
                fields.block.as_deref().unwrap_or("?"),
                self.slo_ms,
            );
            // Loud in the log channel too. Different target than the events
            // this layer filters on, so re-entry terminates immediately.
            tracing::error!(target: "holon_oracles", oracle = "latency-slo", "ORACLE VIOLATION: {message}");
            OracleStatus::global().push_latency(Violation {
                oracle: "latency-slo",
                message,
                at: SystemTime::now(),
            });
        } else {
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

    /// Drive one over-budget `holon_latency` event of `stage` through the layer
    /// and return the count of latency violations that carry `marker` (the
    /// unique `block` id) — parallel-safe against the process-global
    /// `OracleStatus`.
    fn violations_for(stage: &'static str, marker: &str) -> usize {
        let before = OracleStatus::global()
            .snapshot()
            .into_iter()
            .filter(|v| v.oracle == "latency-slo" && v.message.contains(marker))
            .count();
        let layer = LatencySloLayer::new(200);
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
        OracleStatus::global()
            .snapshot()
            .into_iter()
            .filter(|v| v.oracle == "latency-slo" && v.message.contains(marker))
            .count()
            - before
    }

    /// The SLO verdict is anchored on the projection-visible `e2e` stage: an
    /// over-budget `e2e` event records a sticky latency violation.
    #[test]
    fn e2e_over_budget_records_violation() {
        assert_eq!(
            violations_for("e2e", "block:oracle-e2e-marker"),
            1,
            "over-budget projection-visible e2e must fire the SLO verdict"
        );
    }

    /// A per-stage component (`rows`) over budget is a DIAGNOSTIC only — it
    /// must NOT record an SLO violation. The `e2e` stage carries the
    /// verdict.
    #[test]
    fn component_stage_over_budget_is_diagnostic_not_violation() {
        assert_eq!(
            violations_for("rows", "block:oracle-rows-marker"),
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
            violations_for("frame_present", "block:oracle-paint-marker"),
            0,
            "paint/frame-present must never be judged as the SLO endpoint"
        );
    }
}
