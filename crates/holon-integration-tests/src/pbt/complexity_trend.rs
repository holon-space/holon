//! Within-run complexity-class TREND accounting.
//!
//! `inv-sql-budget` answers "did THIS transition cost more than its formula
//! allows". It cannot answer "does this transition get more expensive the
//! longer the program runs" — a formula wide enough to hold at block 200 has
//! room for linear growth from block 3 to block 200 hidden inside it.
//!
//! This module is that second question. Every transition occurrence contributes
//! one [`Sample`] (deduplicated SQL reads + writes, plus the state cardinality
//! at the time). At check time, each transition kind DECLARED
//! [`ComplexityClass::Constant`] is fitted: the mean of its first third of
//! occurrences against the mean of its last third. Growth beyond the tolerance
//! is a violation.
//!
//! # Counters only, never wall time
//! Deduplicated SQL counts are byte-deterministic for a given (transition,
//! state) — the A/B budget triage measured them stable across revisions. Wall
//! time under a loaded machine is not, and has already produced two false gate
//! reds. Nothing here reads a clock.
//!
//! # Why the class is DERIVED, not declared separately
//! [`crate::pbt::transition_budgets::declared_complexity_class`] probes the
//! transition's own `expected_sql` formula at two state sizes. A formula that
//! is a state-blind constant already claims O(1) — restating that claim in a
//! second table would only create a surface for the two to disagree.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The complexity class a transition's SQL budget formula claims.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ComplexityClass {
    /// The budget formula is state-blind: the transition claims its cost does
    /// not depend on how much is in the store. The trend check ENFORCES this.
    Constant,
    /// The budget formula reads state cardinality. Recorded and tabulated, but
    /// out of the trend check's teeth — growth is exactly what it declares.
    StateDependent,
}

/// One occurrence of one transition kind.
#[derive(Clone, Debug)]
pub struct Sample {
    /// Position in the transition sequence (1-based), so a series can be read
    /// against the run log.
    pub seq: usize,
    pub reads: usize,
    pub writes: usize,
    /// Blocks in the reference state after the transition — the cardinality a
    /// growing counter would be tracking.
    pub blocks: usize,
}

/// Which counter grew.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Counter {
    Reads,
    Writes,
}

impl Counter {
    fn label(self) -> &'static str {
        match self {
            Counter::Reads => "reads",
            Counter::Writes => "writes",
        }
    }

    fn of(self, s: &Sample) -> f64 {
        match self {
            Counter::Reads => s.reads as f64,
            Counter::Writes => s.writes as f64,
        }
    }
}

/// Minimum occurrences of one transition kind before a trend can be fitted at
/// all: three per third. Below it the kind is UNDECIDED and says so — a fit
/// over two points is a line through noise, and reporting it as a pass would be
/// the silent-degradation this codebase forbids.
pub const MIN_OCCURRENCES: usize = 9;

/// Growth tolerance. A declared-constant transition may still fork on a
/// BOOLEAN (first-visit navigation, activate-vs-insert tab), which shifts the
/// mean between two constants; the multiplicative slack absorbs a fork whose
/// arms differ by up to 50%, and the additive slack keeps small absolute counts
/// (3 → 4 reads) from tripping it. Neither absorbs accumulation: a counter that
/// tracks corpus size runs several times its early value by the last third.
pub const GROWTH_FACTOR: f64 = 1.5;
/// See [`GROWTH_FACTOR`].
pub const GROWTH_SLACK: f64 = 2.0;

/// One transition kind's occurrences, with the class its budget formula claims.
pub struct KindSeries {
    pub class: ComplexityClass,
    pub samples: Vec<Sample>,
}

/// A fitted violation: a declared-O(1) transition whose counter grew across the
/// run.
#[derive(Clone, Debug)]
pub struct TrendViolation {
    pub kind: String,
    pub counter: Counter,
    pub early_mean: f64,
    pub late_mean: f64,
    pub occurrences: usize,
    /// Blocks at the first and last occurrence — the growth this trend tracks.
    pub blocks_first: usize,
    pub blocks_last: usize,
}

impl TrendViolation {
    pub fn message(&self) -> String {
        format!(
            "{kind}.{counter}: declared O(1) but grew {early:.1} → {late:.1} \
             (×{ratio:.2}) across {n} occurrences, blocks {bf}→{bl}",
            kind = self.kind,
            counter = self.counter.label(),
            early = self.early_mean,
            late = self.late_mean,
            ratio = if self.early_mean > 0.0 {
                self.late_mean / self.early_mean
            } else {
                f64::INFINITY
            },
            n = self.occurrences,
            bf = self.blocks_first,
            bl = self.blocks_last,
        )
    }
}

/// Per-kind counter series accumulated across one PBT case.
#[derive(Default)]
pub struct TrendAccumulator {
    next_seq: usize,
    kinds: BTreeMap<String, KindSeries>,
}

impl TrendAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one transition occurrence. Called once per tick by the harness's
    /// span-metrics host, from the same freeze point `inv-sql-budget` reads —
    /// one collection pipeline, two consumers.
    pub fn record(
        &mut self,
        kind: String,
        class: ComplexityClass,
        reads: usize,
        writes: usize,
        blocks: usize,
    ) {
        self.next_seq += 1;
        let seq = self.next_seq;
        let entry = self.kinds.entry(kind).or_insert(KindSeries {
            class,
            samples: Vec::new(),
        });
        assert_eq!(
            entry.class, class,
            "a transition kind's declared complexity class must not change \
             within a run — the class is derived from a state-blind probe of \
             its budget formula",
        );
        entry.samples.push(Sample {
            seq,
            reads,
            writes,
            blocks,
        });
    }

    /// Fit every declared-constant kind with enough occurrences.
    pub fn report(&self) -> TrendReport {
        let mut violations = Vec::new();
        let mut undecided = Vec::new();
        for (kind, series) in &self.kinds {
            if series.class != ComplexityClass::Constant {
                continue;
            }
            if series.samples.len() < MIN_OCCURRENCES {
                undecided.push((kind.clone(), series.samples.len()));
                continue;
            }
            for counter in [Counter::Reads, Counter::Writes] {
                if let Some(v) = fit(kind, counter, &series.samples) {
                    violations.push(v);
                }
            }
        }
        TrendReport {
            // The accumulator does not read the environment: the HOST decides
            // whether the gate is armed, exactly as `SqlBudgetReport` carries
            // `enforce` rather than the body sampling `HOLON_PERF_BUDGET`.
            enforce: false,
            violations,
            undecided,
            table: self.table(),
        }
    }

    /// The full per-kind counter series. A trend is evidence only as a SERIES;
    /// a shrunk counterexample destroys exactly the accumulation that proves
    /// it, so the report carries every sample rather than a minimal one.
    pub fn table(&self) -> String {
        let mut out = String::new();
        for (kind, series) in &self.kinds {
            let class = match series.class {
                ComplexityClass::Constant => "O(1)",
                ComplexityClass::StateDependent => "O(state)",
            };
            let _ = writeln!(
                out,
                "  {kind} [declared {class}] {} occurrence(s):",
                series.samples.len()
            );
            for s in &series.samples {
                let _ = writeln!(
                    out,
                    "      #{seq:<4} reads={reads:<5} writes={writes:<4} blocks={blocks}",
                    seq = s.seq,
                    reads = s.reads,
                    writes = s.writes,
                    blocks = s.blocks,
                );
            }
        }
        out
    }
}

/// Robust-enough fit: mean of the first third against the mean of the last
/// third, in occurrence order. Deliberately not a regression — the question is
/// "is the late regime more expensive than the early one", and thirds answer it
/// without a slope estimate that a single outlier can dominate.
fn fit(kind: &str, counter: Counter, samples: &[Sample]) -> Option<TrendViolation> {
    let third = samples.len() / 3;
    let early: Vec<&Sample> = samples[..third].iter().collect();
    let late: Vec<&Sample> = samples[samples.len() - third..].iter().collect();
    let mean =
        |xs: &[&Sample]| -> f64 { xs.iter().map(|s| counter.of(s)).sum::<f64>() / xs.len() as f64 };
    let early_mean = mean(&early);
    let late_mean = mean(&late);
    if late_mean <= early_mean * GROWTH_FACTOR + GROWTH_SLACK {
        return None;
    }
    Some(TrendViolation {
        kind: kind.to_string(),
        counter,
        early_mean,
        late_mean,
        occurrences: samples.len(),
        blocks_first: samples[0].blocks,
        blocks_last: samples[samples.len() - 1].blocks,
    })
}

/// What the invariant decides on, plus the evidence it prints.
pub struct TrendReport {
    /// `HOLON_TREND_BUDGET` enforcement is on (else violations are DISCLOSED as
    /// a skip carrying the evidence, not failed). Set by the host.
    pub enforce: bool,
    pub violations: Vec<TrendViolation>,
    /// Declared-constant kinds with too few occurrences to fit: disclosed as
    /// unchecked, never counted as a pass.
    pub undecided: Vec<(String, usize)>,
    /// See [`TrendAccumulator::table`].
    pub table: String,
}

impl TrendReport {
    /// The full disclosure text: fitted trends, then the whole series table,
    /// then the kinds that could not be fitted.
    pub fn evidence(&self) -> String {
        let mut out = String::new();
        for v in &self.violations {
            let _ = writeln!(out, "  {}", v.message());
        }
        let _ = writeln!(out, "counter series (all kinds):\n{}", self.table);
        if !self.undecided.is_empty() {
            let list: Vec<String> = self
                .undecided
                .iter()
                .map(|(k, n)| format!("{k}={n}"))
                .collect();
            let _ = writeln!(
                out,
                "UNFITTED (declared O(1), fewer than {MIN_OCCURRENCES} occurrences): {}",
                list.join(" "),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(class: ComplexityClass, reads: &[usize]) -> TrendAccumulator {
        let mut acc = TrendAccumulator::new();
        for (i, r) in reads.iter().enumerate() {
            acc.record("K".to_string(), class, *r, 2, i + 1);
        }
        acc
    }

    /// TEETH: a planted linear trend on a declared-O(1) kind must be flagged.
    #[test]
    fn planted_linear_trend_is_flagged() {
        let reads: Vec<usize> = (0..12).map(|i| 10 + i * 3).collect();
        let report = series(ComplexityClass::Constant, &reads).report();
        assert_eq!(
            report.violations.len(),
            1,
            "one reads violation expected; got {:?}",
            report.violations,
        );
        let v = &report.violations[0];
        assert_eq!(v.counter, Counter::Reads);
        assert!(
            v.late_mean > v.early_mean,
            "the fit must record growth: {v:?}",
        );
        assert!(
            report.evidence().contains("#12"),
            "the evidence must carry the WHOLE series, not a minimal case:\n{}",
            report.evidence(),
        );
    }

    /// A flat series passes — the check must not red on a constant transition.
    #[test]
    fn flat_series_passes() {
        let report = series(ComplexityClass::Constant, &[7; 12]).report();
        assert!(
            report.violations.is_empty(),
            "a flat series must pass: {:?}",
            report.violations,
        );
    }

    /// Bounded jitter (a boolean fork between two constants) is inside the
    /// tolerance — the noise budget the comment claims, pinned.
    #[test]
    fn boolean_fork_jitter_passes() {
        let reads = [8, 11, 8, 11, 8, 8, 11, 8, 11, 11, 8, 11];
        let report = series(ComplexityClass::Constant, &reads).report();
        assert!(
            report.violations.is_empty(),
            "a two-constant fork must not be read as a trend: {:?}",
            report.violations,
        );
    }

    /// A declared-O(state) kind growing is its DECLARATION, not a violation.
    #[test]
    fn state_dependent_growth_is_not_a_violation() {
        let reads: Vec<usize> = (0..12).map(|i| 10 + i * 3).collect();
        let report = series(ComplexityClass::StateDependent, &reads).report();
        assert!(
            report.violations.is_empty(),
            "an O(state) declaration must be out of the teeth: {:?}",
            report.violations,
        );
        assert!(
            report.table.contains("declared O(state)"),
            "…but still tabulated:\n{}",
            report.table,
        );
    }

    /// Too few occurrences ⇒ UNDECIDED and disclosed, never a silent pass.
    #[test]
    fn short_series_is_disclosed_as_unfitted() {
        let report = series(ComplexityClass::Constant, &[1, 2, 3, 40]).report();
        assert!(report.violations.is_empty());
        assert_eq!(report.undecided, vec![("K".to_string(), 4)]);
        assert!(
            report.evidence().contains("UNFITTED"),
            "the shortfall must reach the report:\n{}",
            report.evidence(),
        );
    }

    /// Growth in WRITES is caught on the same footing as reads.
    #[test]
    fn write_growth_is_flagged() {
        let mut acc = TrendAccumulator::new();
        for i in 0..12 {
            acc.record(
                "K".to_string(),
                ComplexityClass::Constant,
                5,
                1 + i * 2,
                i + 1,
            );
        }
        let report = acc.report();
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.counter == Counter::Writes),
            "a growing write counter must be flagged: {:?}",
            report.violations,
        );
    }
}
