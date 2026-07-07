//! Latency SLO oracle — a `tracing_subscriber` Layer over the existing
//! `holon_latency` instrumentation.
//!
//! The pipeline already emits per-stage timing events
//! `tracing::debug!(target: "holon_latency", stage = "...", ms = ...)`:
//!
//! - `dispatch` (holon-frontend `reactive.rs`) — user action enters the op
//!   pipeline. Fires in every configuration.
//! - `rows` (holon-api `live_data.rs`) — a CDC batch from the matview lands
//!   in the reactive mirror (change becomes visible to the view model).
//!   Fires in every configuration.
//! - `projection` (holon-loro `LoroSyncController`) — full Loro→SQL
//!   projection pass. Fires ONLY when CRDT sync is enabled
//!   (`crdt_enabled`); the default desktop config never emits it.
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

use std::time::SystemTime;

use tracing::field::{Field, Visit};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::status::{OracleStatus, Violation};

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
            // PRIMARY SLO signal: true interaction -> visible-to-render wall
            // time (see holon_api::latency_e2e). Violation = banner + error.
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
