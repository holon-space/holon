//! OTel / performance-metrics component for the PBT SUT.
//!
//! @pbt kind sut-arm
//! @pbt covers all-slices (span-metrics) — non-functional SUT arm:
//! per-transition   span/RSS/wall metrics feeding `inv-sql-budget` (latency SLO
//! teeth).
//!
//! Owns the per-transition span collector and RSS sampling. Hosted by the
//! composed `span_metrics` component (`composed/span_metrics.rs`), which
//! forwards the lifecycle hooks — `on_transition_start` (reset before each
//! transition) and `sql_budget_report` (the `inv-sql-budget` body) — so no
//! other module reads the raw span/RSS state directly. (The whole-case
//! `PBT_MATVIEW_METRICS` drop report died with the `E2ESut` monolith; re-host
//! it on the span_metrics component if case-level dumps are wanted again.)
//!
//! Most state and behaviour is gated on `otel-testing`; without that feature
//! the lifecycle hooks become no-ops.

use std::cell::RefCell;
use std::time::Duration;

#[cfg(feature = "otel-testing")]
use super::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::invariants::bodies::sql_budget::SqlBudgetReport;
#[cfg(feature = "otel-testing")]
use crate::pbt::transitions::E2ETransition;

/// Per-transition performance metrics owned by the SUT.
pub(super) struct MetricsSut {
    /// In-memory OTel span collector for non-functional invariants.
    #[cfg(feature = "otel-testing")]
    span_collector: crate::test_tracing::SpanCollector,
    /// Wall-clock start of the last transition (for wall-time budget checks).
    #[cfg(feature = "otel-testing")]
    last_transition_start: Option<std::time::Instant>,
    /// RSS (bytes) captured before the last transition started.
    #[cfg(feature = "otel-testing")]
    rss_before: usize,
    /// RSS (bytes) at the very start of the PBT run, for cumulative growth.
    #[cfg(feature = "otel-testing")]
    rss_baseline: usize,
    /// Span metrics + wall/RSS frozen at invariant-check start
    /// ([`Self::freeze_at_check_start`]), so the budget measures the
    /// transition itself (apply + settle) — NOT the invariant bodies' own
    /// SQL reads, which used to leak into the window because
    /// `inv-sql-budget` dispatches after every other invariant body and
    /// made the measured counts nondeterministic.
    #[cfg(feature = "otel-testing")]
    frozen_at_check: RefCell<Option<FrozenCheckMetrics>>,
}

/// See [`MetricsSut::freeze_at_check_start`].
#[cfg(feature = "otel-testing")]
struct FrozenCheckMetrics {
    metrics: crate::test_tracing::TransitionMetrics,
    wall: Duration,
    rss_after: usize,
}

impl MetricsSut {
    pub(super) fn new() -> Self {
        Self {
            #[cfg(feature = "otel-testing")]
            span_collector: crate::test_tracing::SpanCollector::global().clone(),
            #[cfg(feature = "otel-testing")]
            last_transition_start: None,
            #[cfg(feature = "otel-testing")]
            rss_before: 0,
            #[cfg(feature = "otel-testing")]
            rss_baseline: 0,
            #[cfg(feature = "otel-testing")]
            frozen_at_check: RefCell::new(None),
        }
    }

    /// Freeze span metrics, wall time, and RSS at the start of invariant
    /// checking. `sql_budget_report` consumes this snapshot instead of
    /// sampling live, so the SQL the other invariant bodies issue (full
    /// `block_raw` scans, fresh-tree re-renders) cannot pollute the
    /// transition's budget window. Called by `run_invariant_registry_gated`
    /// right after the shared settle.
    #[cfg(feature = "otel-testing")]
    pub(super) fn freeze_at_check_start(&self) {
        let metrics = self.span_collector.snapshot();
        let wall = self
            .last_transition_start
            .map(|t| t.elapsed())
            .unwrap_or_default();
        let rss_after = crate::test_tracing::current_rss_bytes();
        *self.frozen_at_check.borrow_mut() = Some(FrozenCheckMetrics {
            metrics,
            wall,
            rss_after,
        });
    }

    /// Snapshot the previous transition's `query` ancestor chains into the
    /// case-level accumulator (when `PBT_MATVIEW_METRICS=1`), then reset the
    /// span collector and re-sample wall-clock/RSS for the transition that is
    /// about to run. Called from both the `proptest-state-machine` `apply`
    /// hook and the `phased` harness so both paths get per-transition
    /// metric isolation. No-op without `otel-testing`.
    #[cfg(feature = "otel-testing")]
    pub(super) fn on_transition_start(&mut self) {
        self.span_collector.reset();
        *self.frozen_at_check.borrow_mut() = None;
        self.last_transition_start = Some(std::time::Instant::now());
        let rss_now = crate::test_tracing::current_rss_bytes();
        self.rss_before = rss_now;
        if self.rss_baseline == 0 {
            self.rss_baseline = rss_now;
        }
    }

    #[cfg(not(feature = "otel-testing"))]
    pub(super) fn on_transition_start(&mut self) {}

    /// Port of the inline `inv-sql-budget` block: snapshot span metrics for
    /// the last transition, emit all telemetry side-effects (summary line,
    /// N+1 list, flamegraph, detail, memory diagnosis), and return the budget
    /// pass/fail decision. Error violations are returned only when
    /// `HOLON_PERF_BUDGET` enforcement is on; otherwise they're logged as
    /// `BUDGET OFF`. `last_transition` is owned by `E2ESut` (it's not a
    /// metric) and passed in.
    #[cfg(feature = "otel-testing")]
    pub(super) fn sql_budget_report(
        &self,
        last_transition: &E2ETransition,
        ref_state: &ReferenceState,
    ) -> SqlBudgetReport {
        use super::transition_budgets;

        // Consume the snapshot frozen at check-start: the budget must measure
        // the transition (apply + settle), not the SQL the other invariant
        // bodies issued while running before this one.
        let FrozenCheckMetrics {
            metrics,
            wall: wall_time,
            rss_after,
        } = self
            .frozen_at_check
            .borrow_mut()
            .take()
            .expect("freeze_at_check_start must run before sql_budget_report");
        let key = transition_budgets::transition_key(last_transition);

        let memory = transition_budgets::MemoryMetrics {
            rss_before: self.rss_before,
            rss_after,
            rss_baseline: self.rss_baseline,
        };

        let expected = transition_budgets::expected_sql(last_transition, ref_state);
        let render_summary: String = if metrics.render_count > 0 {
            let components: Vec<_> = metrics
                .render_by_component
                .iter()
                .map(|(c, n)| format!("{c}={n}"))
                .collect();
            format!(
                " renders={} [{}]",
                metrics.render_count,
                components.join(",")
            )
        } else {
            String::new()
        };
        let cdc_summary: String = if metrics.cdc_ingest_count > 0 || metrics.cdc_emission_count > 0
        {
            format!(
                " cdc_in={} cdc_out={}",
                metrics.cdc_ingest_count, metrics.cdc_emission_count
            )
        } else {
            String::new()
        };
        let perf_summary = format!(
            " apply={}ms check={}ms settle={}ms pre_inv16={}ms live_mirrors={}ms \
             assert_quiet={}ms drain_cdc={}ms inv10_drain={}ms files_stable={}ms mark_proc={}ms×{}",
            metrics.apply_transition_total.as_millis(),
            metrics.check_invariants_total.as_millis(),
            metrics.settle_total.as_millis(),
            metrics.pre_inv16_settle_total.as_millis(),
            metrics.live_mirrors_total.as_millis(),
            metrics.assert_quiescent_total.as_millis(),
            metrics.drain_cdc_total.as_millis(),
            metrics.inv10_watch_drain.as_millis(),
            metrics.wait_files_stable.as_millis(),
            metrics.mark_processed_total.as_millis(),
            metrics.mark_processed_count,
        );
        eprintln!(
            "[inv-sql-budget] {key}: reads={}/{} writes={}/{} ddl={}/{} tol={} max_q={}ms \
             wall={}ms spans={} rss={delta:+.1}MB \
             (cum={cum:+.1}MB){render_summary}{cdc_summary}{perf_summary}",
            metrics.sql_read_count,
            expected.reads,
            metrics.sql_write_count,
            expected.writes,
            metrics.sql_ddl_count,
            expected.ddl,
            expected.tolerance,
            metrics.max_query_duration.as_millis(),
            wall_time.as_millis(),
            metrics.total_span_count,
            delta = memory.rss_delta_mb(),
            cum = memory.cumulative_growth_mb(),
        );

        let violations = transition_budgets::check_budget(
            last_transition,
            ref_state,
            &metrics,
            wall_time,
            Some(&memory),
        );
        let enforce = std::env::var("HOLON_PERF_BUDGET")
            .map(|v| v != "0")
            .unwrap_or(false);

        let has_memory_violation = violations.iter().any(
            |v| matches!(v, transition_budgets::Violation::Error(msg) if msg.contains("rss_")),
        );
        if has_memory_violation {
            transition_budgets::diagnose_memory(&key);
        }

        let mut errors = Vec::new();
        for v in &violations {
            match v {
                transition_budgets::Violation::Warning(msg) => {
                    eprintln!("[inv-sql-budget WARN] {msg}");
                }
                transition_budgets::Violation::Error(msg) => {
                    if enforce {
                        errors.push(msg.clone());
                    } else {
                        eprintln!("[inv-sql-budget BUDGET OFF] {msg}");
                    }
                }
            }
        }

        if !metrics.duplicate_sql.is_empty() {
            eprintln!(
                "[inv-sql-budget N+1] {key}: {} distinct SQL texts fired multiple times:",
                metrics.duplicate_sql.len()
            );
            for dup in &metrics.duplicate_sql {
                // Same statement + same bindings re-executed is redundant work;
                // same statement over distinct bindings is fan-out (one render
                // per sidebar, per-row lookups, …) — only an N+1 if the count
                // is out of line.
                let bindings = if dup.distinct_bindings <= 1 {
                    "identical bindings — redundant".to_string()
                } else {
                    format!("{} distinct bindings — fan-out", dup.distinct_bindings)
                };
                eprintln!("  {}x ({bindings}): {}", dup.count, dup.sql);
            }
        }

        crate::test_tracing::maybe_write_flamegraph(&self.span_collector, &key);
        if std::env::var("HOLON_PERF_DETAIL").is_ok() {
            let breakdown = self.span_collector.sql_breakdown();
            eprintln!("[inv-sql-budget DETAIL] {key}:\n{breakdown}");
        }

        SqlBudgetReport { enforce, errors }
    }
}
