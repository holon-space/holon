//! Per-transition performance budgets.
//!
//! SQL counts are **deterministic** — they depend on the number of active
//! watches, documents, blocks, etc. They are computed from `ReferenceState`,
//! not recorded.
//!
//! Timing is **non-deterministic** — wall-clock and query durations are checked
//! against generous hard limits only.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

// Budget formulas + inputs live in holon-pbt-core (Phase 1a Step 1); re-exported
// so existing `crate::pbt::transition_budgets::…` import sites keep resolving.
pub use holon_pbt_core::budget::{
    CACHE_EVENT_READS, CLICK_JITTER_TOLERANCE, ExpectedSql, JOURNAL_READS, MutationKind,
    NAV_DML_READS, NAV_RENDER_FAN_READS, OPEN_TAB_CLICK_RESOLVE_READS,
    PIN_BLOCK_CLICK_RESOLVE_READS, REACTIVE_BASE, READS_PER_WATCH, SqlBudget, cdc_tolerance,
    docs_tolerance, expected_sql_for_kind,
};
use holon_pbt_core::types::Mutation;

use super::reference_state::ReferenceState;
use crate::test_tracing::TransitionMetrics;

// ── SQL count model ───────────────────────────────────────────────
//
// Every post-startup transition has a "base" read overhead from the reactive
// engine checking what to re-render:
//
//   REACTIVE_BASE = 5:
//     1× SELECT ... FROM block (full block for render source)
//     1× SELECT region, block_id FROM current_focus
//     3× SELECT root_id AS id FROM focus_roots WHERE region = '{region}'
//
// UI mutations go through the operation journal (undo/redo tracking):
//
//   JOURNAL_READS = 2:
//     1× UPDATE operation SET status = ...     (clear redo stack)
//     1× INSERT INTO operation (...) RETURNING id  (insert + get ID in one
// query)   (COUNT(*) for trim is amortized to every 10th operation)
//
// Navigation operations execute DML tracked as "query" spans:
//
//   NAV_DML_READS = 5:
//     1× DELETE FROM navigation_history WHERE region = ... AND id > ...
//     1× INSERT INTO navigation_history (region, block_id) VALUES (...)
//     1× INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES
// (...)     1× SELECT MAX(id) FROM navigation_history WHERE region = ...
//     1× SELECT history_id FROM navigation_cursor WHERE region = ...
//
// Org sync CDC events trigger cache subscriber reads:
//
//   CACHE_EVENT_READS = 3:
//     2× SELECT id FROM block WHERE name IS NULL     (one per CDC event)
//     1× SELECT id, properties FROM block WHERE properties IS NOT NULL
//
// User watches (from SetupWatch) add matview existence checks:
//
//   READS_PER_WATCH = 2:
//     1× SELECT name FROM sqlite_master WHERE type='view' AND
// name='watch_view_...'     1× SELECT * FROM watch_view_...
//
// NOTE: Internal watches (region watches, all-blocks watch, structural
// watch_ui) use subscribe_sql → matview CDC broadcast and do NOT generate
// "query" spans during post-startup transitions. Only user watches from
// SetupWatch contribute.

/// Compute expected SQL counts for a transition given the current reference
/// state.
///
/// The formulas are derived from SQL span analysis (HOLON_PERF_DETAIL=1,
/// 2026-04-05). When a formula doesn't match reality, it means either:
/// 1. The code changed (update the formula), or
/// 2. There's an N+1 bug (fix the code).
///
/// **Tolerance** accounts for CDC-driven re-render cascades: when a mutation
/// triggers org sync (file re-write → file watcher → re-parse → CDC events),
/// each cascade cycle adds parent chain walks and property lookups proportional
/// to the number of blocks in the affected document.
pub fn expected_sql(
    transition: &crate::pbt::transitions::E2ETransition,
    ref_state: &ReferenceState,
) -> ExpectedSql {
    transition.expected_sql(ref_state)
}

/// Expected SQL for a mutation via its `Mutation` value.
pub(crate) fn expected_mutation_sql(
    mutation: &Mutation,
    watches: usize,
    blocks: usize,
    docs: usize,
) -> ExpectedSql {
    let kind = match mutation {
        Mutation::Create { .. } => MutationKind::Create,
        Mutation::Update { .. } => MutationKind::Update,
        Mutation::Delete { .. } => MutationKind::Delete,
        Mutation::Move { .. } => MutationKind::Move,
        Mutation::RestartApp => MutationKind::RestartApp,
    };
    expected_sql_for_kind(kind, watches, blocks, docs)
}

// ── Transition key ────────────────────────────────────────────────

/// Human-readable name for a transition variant (for log output).
pub fn transition_key(transition: &crate::pbt::transitions::E2ETransition) -> String {
    use crate::pbt::transitions::E2ETransition as TV;
    match transition {
        TV::ApplyMutation(am) => match &am.event.mutation {
            Mutation::Create { .. } => "ApplyMutation::Create".into(),
            Mutation::Update { .. } => "ApplyMutation::Update".into(),
            Mutation::Delete { .. } => "ApplyMutation::Delete".into(),
            Mutation::Move { .. } => "ApplyMutation::Move".into(),
            Mutation::RestartApp => "ApplyMutation::RestartApp".into(),
        },
        other => other.variant_name().to_string(),
    }
}

// ── Render budget model ──────────────────────────────────────────
//
// Render counts are NON-DETERMINISTIC — they depend on GPUI's frame scheduling,
// signal coalescing, and CDC timing. Budgets are generous upper bounds.
//
// Start as Violation::Warning to collect calibration data. Promote to Error
// once the model is validated across ~50 PBT runs.

// ── Checking ──────────────────────────────────────────────────────

pub enum Violation {
    Warning(String),
    Error(String),
    /// A breach of a PINNED ceiling — a budget measured and fixed at the
    /// observed maximum. Unlike [`Violation::Error`] this fails the run whether
    /// or not `HOLON_PERF_BUDGET` enforces, so the pinned number is a real
    /// upper limit rather than a logged note.
    PinnedError(String),
}

// ── Generic NFR metric model (C2) ─────────────────────────────────
//
// Every non-functional dimension we budget per transition is one `Metric`.
// `build_samples` turns the raw `TransitionMetrics` / timing / memory into a
// uniform list of `MetricSample`s carrying the typed value plus the *verbatim*
// violation message (so existing `[inv-sql-budget]` log greps keep working).
// `evaluate` is the single comparator; adding a new budgeted dimension is one
// more `Metric` variant + one `push_sample` call — nothing else changes.

/// A measurable non-functional dimension checked once per transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Metric {
    SqlReads,
    SqlReadRepeat,
    SqlWrites,
    SqlDdl,
    MaxQueryMs,
    WallMs,
    SettleMs,
    RssDeltaBytes,
    RssCumulativeBytes,
}

impl Metric {
    /// Stable string key used in the committed baseline file. Independent of
    /// the human-readable message label so the baseline schema is decoupled
    /// from log text.
    pub fn key(self) -> &'static str {
        match self {
            Metric::SqlReads => "sql_reads",
            Metric::SqlReadRepeat => "sql_read_repeat",
            Metric::SqlWrites => "sql_writes",
            Metric::SqlDdl => "sql_ddl",
            Metric::MaxQueryMs => "max_query_ms",
            Metric::WallMs => "wall_ms",
            Metric::SettleMs => "settle_ms",
            Metric::RssDeltaBytes => "rss_delta_bytes",
            Metric::RssCumulativeBytes => "rss_cumulative_bytes",
        }
    }
}

/// Whether an absolute hard-cap breach is a soft warning or a hard error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Error,
    /// See [`Violation::PinnedError`].
    Pinned,
}

/// Transitions whose SQL-read ceiling is PINNED at a measured maximum and
/// therefore enforced unconditionally.
///
/// The click-driven navigation family: both reach navigation through the
/// rendered widget tree, so both pay a click-resolve cost — but each is
/// budgeted by its OWN measured constant, since a shared one would have to sit
/// at the larger and leave the cheaper transition slack to hide in. Every other
/// transition keeps the catalog-wide `HOLON_PERF_BUDGET` opt-in.
fn sql_reads_pinned(transition: &crate::pbt::transitions::E2ETransition) -> bool {
    use crate::pbt::transitions::E2ETransition as TV;
    matches!(transition, TV::PinBlock(_) | TV::OpenTabViaModifierClick(_))
}

/// Redundancy ratchet: the largest number of times ONE read binding-set may
/// re-execute inside a single transition window.
///
/// Budgets are checked against the DEDUPLICATED read count, so the
/// re-execution defect (task #15) no longer inflates them — this is the
/// separate ceiling that keeps the defect from growing while it is unfixed.
///
/// Measured 2026-08-06 over 2063 transition samples / 2860 duplicate-read
/// rows: max 54 (`SimulateRestart`, the block-hydrate select under
/// `org.ingest_file`), then 36, 32, 26, 26, 24; 99.7% of rows sit at ≤14.
/// The ceiling is 64 rather than 54 because that top decile is ten rows
/// concentrated in the two full-reprojection transitions — a max drawn from
/// ten observations is not characterized, and a ratchet that reds on sampling
/// noise gets disarmed instead of fixed.
///
/// RATCHET DOWN as #15 progresses, NEVER UP. A breach means the re-execution
/// defect grew; the fix is the new consumer, never this number.
pub const MAX_READ_REPEAT_PER_BINDING: usize = 64;

/// Violation messages quote the offending statement; the `sql` span attribute
/// is already head…tail-fingerprinted (~440 chars), which is far more than a
/// one-line failure needs to identify it.
fn short_sql(sql: &str) -> String {
    const KEEP: usize = 90;
    let chars: Vec<char> = sql.chars().collect();
    if chars.len() <= KEEP {
        return sql.to_string();
    }
    format!("{}…", chars[..KEEP].iter().collect::<String>())
}

/// One metric's observed value for a transition, its absolute hard cap, and
/// the fully-formed violation message to emit if `actual > limit`.
pub struct MetricSample {
    pub metric: Metric,
    pub actual: f64,
    pub limit: f64,
    pub severity: Severity,
    /// Verbatim violation text, identical to the legacy per-metric format.
    pub message: String,
}

/// Build the per-metric samples for a transition. The messages are
/// byte-for-byte what `check_budget` emitted before the generic refactor.
pub fn build_samples(
    transition: &crate::pbt::transitions::E2ETransition,
    ref_state: &ReferenceState,
    metrics: &TransitionMetrics,
    wall_time: Duration,
    memory: Option<&MemoryMetrics>,
) -> Vec<MetricSample> {
    let key = transition_key(transition);
    let expected = expected_sql(transition, ref_state);
    let mut samples = Vec::new();

    let reads_limit = expected.reads + expected.tolerance;
    let dedup_reads = metrics.dedup_read_count();
    let raw_reads = metrics.sql_read_count;
    // PINNED ceilings compare RAW reads; every other budget compares dedup.
    //
    // The pins exist to catch identical-binding redundancy GROWTH — entry 14's
    // defect is literally "41 redundant re-snapshots of two `watch_view`
    // SELECTs" — so measuring them after dedup would subtract exactly the thing
    // being pinned. Their values were also measured on raw reads and are not
    // re-derived here.
    let pinned = sql_reads_pinned(transition);
    samples.push(MetricSample {
        metric: Metric::SqlReads,
        actual: if pinned { raw_reads } else { dedup_reads } as f64,
        limit: reads_limit as f64,
        severity: if pinned {
            Severity::Pinned
        } else {
            Severity::Error
        },
        // The pinned arm keeps the historical wording verbatim up to the
        // `(watches=…)` tail — `docs/Testing/KeystoneKnownReds.md` matches
        // `pinblock-unrendered-target` on that prefix.
        message: if pinned {
            format!(
                "{key}.sql_reads: {raw_reads} exceeds expected {expected} + tolerance {tol} = \
                 {limit} (watches={w}, docs={d}) [PINNED ceilings gate RAW reads; dedup was \
                 {dedup_reads}]",
                expected = expected.reads,
                tol = expected.tolerance,
                limit = reads_limit,
                w = ref_state.mcp.active_watches.len(),
                d = ref_state.files.documents.len(),
            )
        } else {
            format!(
                "{key}.sql_reads: {dedup_reads} dedup (raw {raw_reads}, {excess} redundant \
                 re-executions) exceeds expected {expected} + tolerance {tol} = {limit} \
                 (watches={w}, docs={d})",
                excess = metrics.redundant_read_excess(),
                expected = expected.reads,
                tol = expected.tolerance,
                limit = reads_limit,
                w = ref_state.mcp.active_watches.len(),
                d = ref_state.files.documents.len(),
            )
        },
    });

    if let Some((sql, repeats)) = metrics.worst_read_repeat() {
        samples.push(MetricSample {
            metric: Metric::SqlReadRepeat,
            actual: repeats as f64,
            limit: MAX_READ_REPEAT_PER_BINDING as f64,
            severity: Severity::Error,
            message: format!(
                "{key}.sql_read_repeat: one binding-set of `{sql}` re-executed {repeats}x, \
                 over the redundancy ratchet {MAX_READ_REPEAT_PER_BINDING} — the re-execution \
                 defect GREW; find the new consumer, do not raise the ratchet",
                sql = short_sql(sql),
            ),
        });
    }

    let writes_limit = expected.writes + expected.tolerance;
    samples.push(MetricSample {
        metric: Metric::SqlWrites,
        actual: metrics.sql_write_count as f64,
        limit: writes_limit as f64,
        severity: Severity::Error,
        message: format!(
            "{key}.sql_writes: {actual} exceeds expected {expected} + tolerance {tol} = {limit}",
            actual = metrics.sql_write_count,
            expected = expected.writes,
            tol = expected.tolerance,
            limit = writes_limit,
        ),
    });

    let ddl_limit = expected.ddl + expected.tolerance;
    samples.push(MetricSample {
        metric: Metric::SqlDdl,
        actual: metrics.sql_ddl_count as f64,
        limit: ddl_limit as f64,
        severity: Severity::Error,
        message: format!(
            "{key}.sql_ddl: {actual} exceeds expected {expected} + tolerance {tol} = {limit}",
            actual = metrics.sql_ddl_count,
            expected = expected.ddl,
            tol = expected.tolerance,
            limit = ddl_limit,
        ),
    });

    let max_single_query = Duration::from_secs(2);
    samples.push(MetricSample {
        metric: Metric::MaxQueryMs,
        actual: metrics.max_query_duration.as_millis() as f64,
        limit: max_single_query.as_millis() as f64,
        severity: Severity::Error,
        message: format!(
            "{key}.single_query: {}ms exceeds limit {}ms",
            metrics.max_query_duration.as_millis(),
            max_single_query.as_millis(),
        ),
    });

    let max_wall = Duration::from_secs(30);
    samples.push(MetricSample {
        metric: Metric::WallMs,
        actual: wall_time.as_millis() as f64,
        limit: max_wall.as_millis() as f64,
        severity: Severity::Error,
        message: format!(
            "{key}.wall_time: {}ms exceeds limit {}ms",
            wall_time.as_millis(),
            max_wall.as_millis(),
        ),
    });

    // Loro→SQL convergence latency. The poll self-bounds (~3s), so the absolute
    // cap is a generous Warn ("settle nearly timed out → convergence failing");
    // the real signal is baseline regression. `Warn` so non-deterministic sync
    // timing never fails a run.
    let settle_limit_ms = 2500.0;
    samples.push(MetricSample {
        metric: Metric::SettleMs,
        actual: metrics.settle_total.as_millis() as f64,
        limit: settle_limit_ms,
        severity: Severity::Warn,
        message: format!(
            "{key}.settle: {}ms exceeds limit {}ms (Loro→SQL convergence slow)",
            metrics.settle_total.as_millis(),
            settle_limit_ms as u64,
        ),
    });

    if let Some(mem) = memory {
        let delta_limit = max_rss_delta_bytes(transition) as f64 * memory_multiplier();
        samples.push(MetricSample {
            metric: Metric::RssDeltaBytes,
            actual: mem.rss_delta_bytes() as f64,
            limit: delta_limit,
            severity: Severity::Error,
            message: format!(
                "{key}.rss_delta: {delta_mb:+.1}MB exceeds limit {limit_mb:.0}MB \
                 (before={before_mb:.0}MB, after={after_mb:.0}MB)",
                delta_mb = mem.rss_delta_mb(),
                limit_mb = delta_limit / (1024.0 * 1024.0),
                before_mb = mem.rss_before as f64 / (1024.0 * 1024.0),
                after_mb = mem.rss_after as f64 / (1024.0 * 1024.0),
            ),
        });

        let cumulative_limit = MAX_CUMULATIVE_RSS_GROWTH as f64 * memory_multiplier();
        samples.push(MetricSample {
            metric: Metric::RssCumulativeBytes,
            actual: mem.cumulative_growth_bytes() as f64,
            limit: cumulative_limit,
            severity: Severity::Error,
            message: format!(
                "{key}.rss_cumulative: {cum_mb:+.1}MB total growth exceeds limit {limit_mb:.0}MB \
                 (baseline={base_mb:.0}MB, current={cur_mb:.0}MB)",
                cum_mb = mem.cumulative_growth_mb(),
                limit_mb = MAX_CUMULATIVE_RSS_GROWTH as f64 / (1024.0 * 1024.0),
                base_mb = mem.rss_baseline as f64 / (1024.0 * 1024.0),
                cur_mb = mem.rss_after as f64 / (1024.0 * 1024.0),
            ),
        });
    }

    samples
}

/// Single comparator: a sample violates iff `actual > limit`.
pub fn evaluate(samples: &[MetricSample]) -> Vec<Violation> {
    samples
        .iter()
        .filter(|s| s.actual > s.limit)
        .map(|s| match s.severity {
            Severity::Warn => Violation::Warning(s.message.clone()),
            Severity::Error => Violation::Error(s.message.clone()),
            Severity::Pinned => Violation::PinnedError(s.message.clone()),
        })
        .collect()
}

/// Check observed metrics against computed expected SQL counts + timing +
/// memory limits, plus baseline-relative regressions (C3). Absolute hard-cap
/// breaches keep their original `Warning`/`Error` severity; regressions are
/// `Warning`s.
pub fn check_budget(
    transition: &crate::pbt::transitions::E2ETransition,
    ref_state: &ReferenceState,
    metrics: &TransitionMetrics,
    wall_time: Duration,
    memory: Option<&MemoryMetrics>,
) -> Vec<Violation> {
    let key = transition_key(transition);
    let samples = build_samples(transition, ref_state, metrics, wall_time, memory);
    record_for_baseline(&key, &samples);
    let mut violations = evaluate(&samples);
    violations.extend(baseline_regressions(&key, &samples));
    violations
}

// ── Baseline-relative regression (C3) ─────────────────────────────
//
// Absolute hard caps (above) are generous floors that only catch gross
// blow-ups. The committed baseline catches *regressions*: a transition that
// used to do N reads / grow M bytes and now does measurably more, even while
// still under the hard cap. The baseline is keyed `transition_key -> metric ->
// value`; a sample regresses when `actual > baseline * (1 + tol)`.
//
// The mechanism ships DORMANT: with no baseline file, regression checking is
// simply inactive (no fabricated numbers). Generate one deliberately with a
// single calibration run:
//
//   HOLON_NFR_BASELINE_UPDATE=1 cargo nextest run -p holon-integration-tests \
//       --features pbt <pbt-test> --no-capture -j1
//
// which records the per-(transition,metric) maxima observed and writes the
// file. Commit it; thereafter every run warns on regressions beyond tolerance.

/// `transition_key -> (metric.key() -> value)`.
type BaselineMap = BTreeMap<String, BTreeMap<String, f64>>;

/// Fraction over baseline that counts as a regression.
/// `HOLON_NFR_REGRESSION_TOL` overrides (e.g. `0.5` = 50%). Default 25% —
/// RSS/wall are noisy.
fn regression_tolerance() -> f64 {
    static TOL: OnceLock<f64> = OnceLock::new();
    *TOL.get_or_init(|| {
        std::env::var("HOLON_NFR_REGRESSION_TOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.25)
    })
}

/// Committed baseline location. `HOLON_NFR_BASELINE` overrides; default is
/// `nfr_baseline.json` at the crate root.
fn baseline_path() -> PathBuf {
    std::env::var("HOLON_NFR_BASELINE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("nfr_baseline.json"))
}

/// Loaded baseline, or `None` when the file is absent (feature dormant). A
/// present-but-malformed file fails loud — a corrupt baseline is a real error,
/// not a reason to silently skip the check.
fn load_baseline() -> &'static Option<BaselineMap> {
    static BASELINE: OnceLock<Option<BaselineMap>> = OnceLock::new();
    BASELINE.get_or_init(|| {
        let path = baseline_path();
        if !path.exists() {
            return None;
        }
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read NFR baseline {}: {e}", path.display()));
        Some(parse_baseline(&raw))
    })
}

/// Parse a baseline document. Separated from `load_baseline` so the parse is
/// testable without touching the process-global `OnceLock` cache.
fn parse_baseline(raw: &str) -> BaselineMap {
    serde_json::from_str(raw).expect("malformed NFR baseline document")
}

/// Pure regression comparator: a sample regresses when its `actual` exceeds the
/// recorded baseline by more than `tol`. Zero/absent baselines are skipped.
fn compute_regressions(
    key: &str,
    baseline_metrics: &BTreeMap<String, f64>,
    samples: &[MetricSample],
    tol: f64,
) -> Vec<Violation> {
    samples
        .iter()
        .filter_map(|s| {
            let base = *baseline_metrics.get(s.metric.key())?;
            if base <= 0.0 {
                return None;
            }
            let threshold = base * (1.0 + tol);
            (s.actual > threshold).then(|| {
                Violation::Warning(format!(
                    "{key}.{metric} regression: {actual:.1} vs baseline {base:.1} (+{pct:.0}%, \
                     threshold {threshold:.1} at {tol_pct:.0}% tolerance)",
                    metric = s.metric.key(),
                    actual = s.actual,
                    pct = (s.actual - base) / base * 100.0,
                    tol_pct = tol * 100.0,
                ))
            })
        })
        .collect()
}

/// Per-transition regressions vs the committed baseline. Always `Warning`s —
/// regressions inform, they don't fail the run.
pub fn baseline_regressions(key: &str, samples: &[MetricSample]) -> Vec<Violation> {
    let Some(baseline) = load_baseline() else {
        return Vec::new();
    };
    let Some(metrics) = baseline.get(key) else {
        return Vec::new();
    };
    compute_regressions(key, metrics, samples, regression_tolerance())
}

/// Whether `HOLON_NFR_BASELINE_UPDATE` is set (and not `0`) — turns on baseline
/// generation: each transition's maxima are accumulated and the file rewritten.
fn baseline_update_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("HOLON_NFR_BASELINE_UPDATE")
            .map(|v| v != "0")
            .unwrap_or(false)
    })
}

/// Process-global accumulator of per-(transition,metric) maxima, used only in
/// baseline-generation mode.
fn baseline_accumulator() -> &'static Mutex<BaselineMap> {
    static ACC: OnceLock<Mutex<BaselineMap>> = OnceLock::new();
    ACC.get_or_init(|| Mutex::new(BaselineMap::new()))
}

/// In generation mode, fold this transition's samples into the running maxima
/// and rewrite the baseline file. No-op otherwise. Rewriting on every
/// transition is wasteful but only happens during a deliberate calibration run,
/// and keeps the file correct without an end-of-run flush hook.
pub fn record_for_baseline(key: &str, samples: &[MetricSample]) {
    if !baseline_update_enabled() {
        return;
    }
    let mut acc = baseline_accumulator()
        .lock()
        .expect("baseline accumulator poisoned");
    let entry = acc.entry(key.to_string()).or_default();
    for s in samples {
        let slot = entry.entry(s.metric.key().to_string()).or_insert(f64::MIN);
        *slot = slot.max(s.actual);
    }
    let path = baseline_path();
    let json = serde_json::to_string_pretty(&*acc).expect("serialize NFR baseline");
    std::fs::write(&path, json)
        .unwrap_or_else(|e| panic!("failed to write NFR baseline {}: {e}", path.display()));
}

// ── Memory budget model ─────────────────────────────────────────
//
// RSS (Resident Set Size) is the OS-visible memory footprint. It's
// non-deterministic (page-granular, affected by OS reclaim) but it's
// what users see when memory bloats from 250MB to 4GB.
//
// Per-transition limits are generous hard caps. The cumulative limit
// catches slow leaks that stay under per-transition thresholds.
//
// When a limit is breached, sut.rs dumps system-level allocation stats
// to help identify the bloated subsystem.

const MB: usize = 1024 * 1024;

/// Linear multiplier for all memory budgets.
/// Set `PBT_MEMORY_MULTIPLIER=1.5` to relax limits by 50% (e.g. for
/// debug builds with full debug info, extra tracing subscribers, etc.).
/// Defaults to 1.0.
fn memory_multiplier() -> f64 {
    static MUL: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *MUL.get_or_init(|| {
        std::env::var("PBT_MEMORY_MULTIPLIER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0)
    })
}

/// Cumulative RSS growth limit across the entire PBT run.
/// If the process grows by more than this from the first transition,
/// something is leaking.
pub const MAX_CUMULATIVE_RSS_GROWTH: usize = 2000 * MB;

/// Per-transition RSS delta limit in bytes.
///
/// Calibrated from PBT runs (2026-04-06, sql_only variant, 2 cases):
///   StartApp (1st): +613MB (226K OTel spans + Turso schema + matviews)
///   StartApp (2nd): +59MB  (schema already cached, fewer spans)
///   BulkExternalAdd: +32MB (org sync + CDC cascades)
///   ApplyMutation:   +9MB  (single block mutation)
///   Navigation/View: <1MB
///
/// Limits are ~2x observed max to avoid flaky failures from OS page reclaim
/// jitter.
pub fn max_rss_delta_bytes(transition: &crate::pbt::transitions::E2ETransition) -> usize {
    match transition.variant_name() {
        "StartApp" => 1500 * MB,
        "ConcurrentSchemaInit" => 1500 * MB,
        "WriteOrgFile" | "CreateDirectory" | "GitInit" | "JjGitInit" | "CreateStaleLoro" => 5 * MB,
        // Builds a full DI container + opens a second Turso connection before the
        // wiring guard rejects the flipped consolidator (no matviews/CDC/spans of a
        // full boot, but a fresh backend + DI allocations); budget alongside StartApp.
        "EpochFlipRejected" => 1500 * MB,
        "BulkExternalAdd" | "CreateDocument" | "DeleteDocument" => 200 * MB,
        "SimulateRestart" => 80 * MB,
        "ApplyMutation" => 50 * MB,
        "SetupWatch" => 15 * MB,
        _ => 10 * MB,
    }
}

/// Memory metrics for a single transition.
#[derive(Debug, Clone)]
pub struct MemoryMetrics {
    /// RSS before the transition (bytes).
    pub rss_before: usize,
    /// RSS after the transition (bytes).
    pub rss_after: usize,
    /// RSS at the very start of the PBT run (first transition), for cumulative
    /// tracking.
    pub rss_baseline: usize,
}

impl MemoryMetrics {
    pub fn rss_delta_bytes(&self) -> isize {
        self.rss_after as isize - self.rss_before as isize
    }

    pub fn rss_delta_mb(&self) -> f64 {
        self.rss_delta_bytes() as f64 / (1024.0 * 1024.0)
    }

    pub fn cumulative_growth_bytes(&self) -> isize {
        self.rss_after as isize - self.rss_baseline as isize
    }

    pub fn cumulative_growth_mb(&self) -> f64 {
        self.cumulative_growth_bytes() as f64 / (1024.0 * 1024.0)
    }
}

/// Dump system-level memory diagnostics to stderr.
/// Called when an RSS budget is breached to help identify what's consuming
/// memory.
#[cfg(target_os = "macos")]
pub fn diagnose_memory(key: &str) {
    eprintln!("[MEMORY DIAG] {key}: dumping macOS memory stats...");

    if let Ok(output) = std::process::Command::new("footprint")
        .arg("-j")
        .arg(std::process::id().to_string())
        .output()
    {
        if output.status.success() {
            let json = String::from_utf8_lossy(&output.stdout);
            // Extract top-level categories from footprint JSON
            eprintln!(
                "[MEMORY DIAG] {key}: footprint output ({} bytes):",
                json.len()
            );
            // Print first 2000 chars — enough to see the major categories
            for line in json.lines().take(60) {
                eprintln!("  {line}");
            }
        } else {
            eprintln!(
                "[MEMORY DIAG] {key}: footprint failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    // Also dump RSS breakdown via ps
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "pid,rss,vsz,command", "-p"])
        .arg(std::process::id().to_string())
        .output()
        && output.status.success()
    {
        eprintln!(
            "[MEMORY DIAG] {key}: ps:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub fn diagnose_memory(key: &str) {
    eprintln!("[MEMORY DIAG] {key}: dumping /proc/self/status memory info...");
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("Vm") || line.starts_with("Rss") || line.starts_with("Hugetlb") {
                eprintln!("  {line}");
            }
        }
    }
    if let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps_rollup") {
        eprintln!("[MEMORY DIAG] {key}: smaps_rollup:");
        for line in smaps.lines() {
            eprintln!("  {line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(metric: Metric, actual: f64, limit: f64, severity: Severity) -> MetricSample {
        MetricSample {
            metric,
            actual,
            limit,
            severity,
            message: format!("{}: {actual} > {limit}", metric.key()),
        }
    }

    #[test]
    fn metric_keys_are_unique() {
        let all = [
            Metric::SqlReads,
            Metric::SqlWrites,
            Metric::SqlDdl,
            Metric::MaxQueryMs,
            Metric::WallMs,
            Metric::SettleMs,
            Metric::RssDeltaBytes,
            Metric::RssCumulativeBytes,
        ];
        let keys: BTreeMap<&str, ()> = all.iter().map(|m| (m.key(), ())).collect();
        assert_eq!(keys.len(), all.len(), "Metric::key() collision");
    }

    #[test]
    fn evaluate_flags_only_breaches_with_correct_severity() {
        let samples = vec![
            sample(Metric::SqlReads, 10.0, 12.0, Severity::Error), // under cap → ok
            sample(Metric::SqlReads, 12.0, 12.0, Severity::Error), /* exactly at cap → ok
                                                                    * (strict >) */
            sample(Metric::SqlWrites, 13.0, 12.0, Severity::Error), // over → Error
            sample(Metric::WallMs, 99.0, 30.0, Severity::Warn),     // over → Warning
        ];
        let v = evaluate(&samples);
        assert_eq!(v.len(), 2, "only the two over-cap samples should fire");
        assert!(matches!(v[0], Violation::Error(_)));
        assert!(matches!(v[1], Violation::Warning(_)));
    }

    #[test]
    fn compute_regressions_is_relative_to_baseline() {
        let mut base = BTreeMap::new();
        base.insert(Metric::SqlReads.key().to_string(), 100.0);
        base.insert(Metric::RssDeltaBytes.key().to_string(), 1000.0);

        // 20% over a 100 baseline at 25% tolerance → no regression.
        let within = vec![sample(Metric::SqlReads, 120.0, f64::MAX, Severity::Error)];
        assert!(compute_regressions("Key", &base, &within, 0.25).is_empty());

        // 30% over → regression warning.
        let over = vec![sample(Metric::SqlReads, 130.0, f64::MAX, Severity::Error)];
        let v = compute_regressions("Key", &base, &over, 0.25);
        assert_eq!(v.len(), 1);
        assert!(matches!(&v[0], Violation::Warning(m) if m.contains("sql_reads regression")));
    }

    #[test]
    fn compute_regressions_skips_absent_and_zero_baselines() {
        let mut base = BTreeMap::new();
        base.insert(Metric::SqlWrites.key().to_string(), 0.0); // zero → skip

        let samples = vec![
            sample(Metric::SqlReads, 9999.0, f64::MAX, Severity::Error), /* absent in baseline →
                                                                          * skip */
            sample(Metric::SqlWrites, 9999.0, f64::MAX, Severity::Error), // zero baseline → skip
        ];
        assert!(compute_regressions("Key", &base, &samples, 0.25).is_empty());
    }

    #[test]
    fn parse_baseline_round_trips() {
        let mut map: BaselineMap = BTreeMap::new();
        let mut inner = BTreeMap::new();
        inner.insert(Metric::SqlReads.key().to_string(), 42.0);
        inner.insert(Metric::WallMs.key().to_string(), 1234.5);
        map.insert("ApplyMutation::Create".to_string(), inner);

        let json = serde_json::to_string_pretty(&map).unwrap();
        let parsed = parse_baseline(&json);
        assert_eq!(parsed, map);
    }
}
