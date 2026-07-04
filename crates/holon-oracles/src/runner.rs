//! Background oracle runner — the cheap tier on a fixed cadence.
//!
//! Runs as a plain tokio task (spawned by the frontend's main, off the GPUI
//! thread). Each cycle takes SQL snapshots through [`OracleStateAccess`]
//! (implemented by the frontend over `BackendEngine::execute_query` — the
//! same concurrency-safe read path the embedded MCP server uses), runs the
//! pure [`crate::checks`], and publishes findings to
//! [`crate::status::OracleStatus::global`].
//!
//! Violations are loud in both channels: `tracing::error!` AND the UI banner
//! (fed via the status watch channel). Cycle timing is logged under
//! `target="holon_oracles"` so overhead is measurable.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::checks::{self, ParentRow, SourceLanguageRow};
use crate::status::{OracleStatus, Violation};
use crate::OracleMode;

/// Live-state snapshot access, implemented by the frontend at the boundary
/// (SQL row → typed row parsing happens in the impl, fail-loud).
#[async_trait::async_trait]
pub trait OracleStateAccess: Send + Sync + 'static {
    /// `(id, parent_id)` rows from the `block` matview (read-side).
    async fn matview_parent_rows(&self) -> anyhow::Result<Vec<ParentRow>>;
    /// `(id, parent_id)` rows from `block_raw` (write-side truth).
    async fn raw_parent_rows(&self) -> anyhow::Result<Vec<ParentRow>>;
    /// Source-language rows from `block_raw`.
    async fn source_language_rows(&self) -> anyhow::Result<Vec<SourceLanguageRow>>;
}

pub struct OracleRunnerConfig {
    pub interval: Duration,
    /// Adaptive back-off: sleep at least `backoff_factor ×` the previous
    /// cycle's duration, capped at [`Self::max_interval`]. Bounds the oracle's
    /// duty cycle (work / (work + sleep)) to `1 / (1 + backoff_factor)`
    /// regardless of vault size — measured: a cycle is ~45ms at 1.1k blocks
    /// but ~5s at 8k blocks (Turso full-scan cost grows superlinearly).
    pub backoff_factor: u32,
    pub max_interval: Duration,
}

impl Default for OracleRunnerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            backoff_factor: 5,
            max_interval: Duration::from_secs(60),
        }
    }
}

/// Run the oracle loop forever. Spawn this on the tokio runtime.
pub async fn run_oracle_loop(
    db: Arc<dyn OracleStateAccess>,
    mode: OracleMode,
    config: OracleRunnerConfig,
) {
    assert!(mode.enabled(), "run_oracle_loop called with oracles off");
    tracing::info!(
        target: "holon_oracles",
        ?mode,
        interval_ms = config.interval.as_millis() as u64,
        "live oracles active (HOLON_ORACLES=off to disable)"
    );
    let mut sleep_for = config.interval;
    loop {
        tokio::time::sleep(sleep_for).await;
        let t0 = Instant::now();
        match run_cheap_cycle(db.as_ref()).await {
            Ok(cycle) => {
                let cycle_ms = t0.elapsed().as_millis() as u64;
                sleep_for = (t0.elapsed() * config.backoff_factor)
                    .clamp(config.interval, config.max_interval);
                for v in &cycle.violations {
                    tracing::error!(
                        target: "holon_oracles",
                        oracle = v.oracle,
                        "ORACLE VIOLATION: {}",
                        v.message
                    );
                }
                tracing::debug!(
                    target: "holon_oracles",
                    stage = "cycle",
                    ms = cycle_ms,
                    matview_rows = cycle.matview_rows,
                    raw_rows = cycle.raw_rows,
                    violations = cycle.violations.len(),
                    next_sleep_ms = sleep_for.as_millis() as u64,
                    "holon_oracles cycle"
                );
                OracleStatus::global().set_structural(cycle.violations);
            }
            Err(e) => {
                sleep_for = (t0.elapsed() * config.backoff_factor)
                    .clamp(config.interval, config.max_interval);
                // Snapshot failure is itself a violation — never silently
                // degrade to "no news is good news".
                let v = Violation {
                    oracle: "oracle-runner",
                    message: format!("oracle snapshot failed: {e:#}"),
                    at: SystemTime::now(),
                };
                tracing::error!(target: "holon_oracles", "ORACLE VIOLATION: {}", v.message);
                OracleStatus::global().set_structural(vec![v]);
            }
        }
    }
}

struct CycleOutcome {
    violations: Vec<Violation>,
    matview_rows: usize,
    raw_rows: usize,
}

async fn run_cheap_cycle(db: &dyn OracleStateAccess) -> anyhow::Result<CycleOutcome> {
    let matview = db.matview_parent_rows().await?;
    let raw = db.raw_parent_rows().await?;
    let source_rows = db.source_language_rows().await?;

    let now = SystemTime::now();
    let violations: Vec<Violation> = checks::find_orphans(&matview)
        .into_iter()
        .map(|m| ("inv-no-orphan-blocks", m))
        .chain(
            checks::find_parent_cycles(&raw)
                .into_iter()
                .map(|m| ("inv-no-parent-cycles", m)),
        )
        .chain(
            checks::find_source_language_violations(&source_rows)
                .into_iter()
                .map(|m| ("inv-source-language-iff-source", m)),
        )
        .map(|(oracle, message)| Violation {
            oracle,
            message,
            at: now,
        })
        .collect();

    Ok(CycleOutcome {
        matview_rows: matview.len(),
        raw_rows: raw.len(),
        violations,
    })
}
