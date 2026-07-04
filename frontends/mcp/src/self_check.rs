//! The live self-check contract: the seam between the `run_self_checks` tool
//! and the invariant suite that answers it.
//!
//! The suite implementation lives ABOVE this crate (in
//! `holon-integration-tests`, where the composed invariant catalog lives) and
//! is registered into [`crate::server::DebugServices::self_check_suite`] by the
//! frontend. Only a `pbt`-featured build carries one, so an unregistered slot
//! is a wiring fact the tool states plainly — never an empty report that reads
//! like a pass.
//!
//! [`LiveSelfCheckSuite::run`] is SYNCHRONOUS and takes an owned
//! [`LiveSnapshot`]: the invariant machinery's futures are `!Send`, so the tool
//! captures the live state on the server's runtime (Send-safe, exactly as
//! `debug_pbt_snapshot` does) and hands the suite pure data to check.

use serde::Serialize;

/// The live state a self-check runs against, captured in one pass so every
/// invariant sees the same instant.
///
/// Fields are what the running app can answer TRUTHFULLY through the debug
/// handles. A capability with no live source here is not faked: the suite
/// reports the invariants needing it as [`CheckOutcome::Skipped`] naming the
/// missing source.
pub struct LiveSnapshot {
    /// The CDC-driven `LiveData<Block>` mirror — `debug_pbt_snapshot`'s
    /// `live_blocks`, as typed values.
    pub live_blocks: Vec<holon_api::Block>,
    /// The live `focus_roots` mirror as `(region, root_id)`.
    pub focus_roots: Vec<(String, String)>,
}

/// One invariant's disposition. `Skipped` always carries its reason — an
/// invariant that did not run must never be indistinguishable from one that
/// passed.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CheckOutcome {
    Pass,
    Fail { detail: String },
    Skipped { reason: String },
}

/// One row of the report.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub id: String,
    #[serde(flatten)]
    pub outcome: CheckOutcome,
}

/// The typed result of one `run_self_checks` call.
#[derive(Debug, Clone, Serialize)]
pub struct SelfCheckReport {
    /// Every class-1 invariant considered, in catalog order — passes, failures
    /// and skips alike. Nothing is omitted.
    pub checks: Vec<CheckReport>,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub wall_ms: u128,
    /// Blocks the snapshot carried — a zero here explains a report of all
    /// vacuous passes.
    pub live_block_count: usize,
}

impl SelfCheckReport {
    /// Build from the per-check rows, deriving the counts so they cannot
    /// disagree with the list.
    pub fn from_checks(checks: Vec<CheckReport>, wall_ms: u128, live_block_count: usize) -> Self {
        let passed = checks
            .iter()
            .filter(|c| matches!(c.outcome, CheckOutcome::Pass))
            .count();
        let failed = checks
            .iter()
            .filter(|c| matches!(c.outcome, CheckOutcome::Fail { .. }))
            .count();
        let skipped = checks
            .iter()
            .filter(|c| matches!(c.outcome, CheckOutcome::Skipped { .. }))
            .count();
        Self {
            total: checks.len(),
            passed,
            failed,
            skipped,
            wall_ms,
            live_block_count,
            checks,
        }
    }
}

/// The registered self-check suite. Implemented in `holon-integration-tests`
/// over the composed invariant catalog's class-1 set.
pub trait LiveSelfCheckSuite: Send + Sync {
    fn run(&self, snapshot: LiveSnapshot) -> Result<SelfCheckReport, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, outcome: CheckOutcome) -> CheckReport {
        CheckReport {
            id: id.to_string(),
            outcome,
        }
    }

    /// The counts are DERIVED, so a report can never claim a pass total its own
    /// rows do not support.
    #[test]
    fn counts_are_derived_from_the_rows() {
        let report = SelfCheckReport::from_checks(
            vec![
                row("a", CheckOutcome::Pass),
                row("b", CheckOutcome::Pass),
                row(
                    "c",
                    CheckOutcome::Fail {
                        detail: "orphan".to_string(),
                    },
                ),
                row(
                    "d",
                    CheckOutcome::Skipped {
                        reason: "no live source".to_string(),
                    },
                ),
            ],
            42,
            7,
        );
        assert_eq!(report.total, 4);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.passed + report.failed + report.skipped, report.total);
        assert_eq!(report.total, report.checks.len());
        assert_eq!(report.wall_ms, 42);
        assert_eq!(report.live_block_count, 7);
    }

    #[test]
    fn an_empty_report_counts_zero_rather_than_passing() {
        let report = SelfCheckReport::from_checks(Vec::new(), 0, 0);
        assert_eq!((report.total, report.passed, report.failed), (0, 0, 0));
    }

    /// A skip must carry its reason through serialization — a consumer that
    /// sees only `"skipped"` cannot tell an unavailable check from a passing
    /// one.
    #[test]
    fn a_skip_serializes_with_its_reason() {
        let report = SelfCheckReport::from_checks(
            vec![row(
                "inv-org-render-fixed-point",
                CheckOutcome::Skipped {
                    reason: "SUT cap absent live: SutOrgRender".to_string(),
                },
            )],
            0,
            0,
        );
        let json = serde_json::to_string(&report).expect("report serializes");
        assert!(json.contains("\"outcome\":\"skipped\""), "{json}");
        assert!(json.contains("SUT cap absent live: SutOrgRender"), "{json}");
    }
}
