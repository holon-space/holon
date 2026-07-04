//! `inv-settle-budget` — per-transition interaction→projection-visible
//! latency budget.
//!
//! @pbt oracle latency — the wall time of ONE transition's dispatch + the
//!   3-projection convergence settle, checked against the p95 < 200ms
//!   interaction→projection-visible SLO (canonical id home; body dispatched
//!   via composed::settle_latency::InvComposedSettleBudget)
//! @pbt covers latency-slo — a transition whose projection becomes visible
//!   later than the SLO allows, at ANY vault scale
//! @pbt slips-if-removed a write path degrades from incremental to
//!   full-recompute (the 2026-07-28 turso IVM `Delta::consolidate` regime:
//!   ~9s per navigation focus at 24k blocks); the suite sits there and
//!   reports GREEN, because "correct" and "correct after 23 minutes" are
//!   indistinguishable without a clock in the oracle
//!
//! ## Why the threshold is NOT scaled by `soak_settle()`
//! [`soak_seed::soak_settle`] is the CONVERGENCE WAIT CAP — how long the
//! harness is willing to poll for quiescence before declaring
//! non-convergence. A scale lane raises it (`HOLON_SOAK_SETTLE_MS=30000`)
//! precisely so slow projections are allowed to *finish* and be *measured*.
//! Deriving the hard-fail threshold from it would make the scale lane
//! structurally incapable of firing this invariant — the exact regime it
//! exists to expose. So the threshold comes from the SLO instead, and the
//! machine-load allowance is a separate, explicit multiplier.
//!
//! ## Machine-load fairness
//! The suite runs on loaded dev machines, so the HARD-FAIL threshold is
//! `SLO × HOLON_PBT_LATENCY_SLACK` (default [`DEFAULT_SLACK`]).
//! `HOLON_PBT_LATENCY_STRICT=1` (quiet machine / dedicated soak host) drops
//! the slack to 1, i.e. the bare 200ms SLO. The RAW measurement is recorded
//! and reported either way — the slack changes when we FAIL, never what we
//! MEASURE.
//!
//! [`soak_seed::soak_settle`]: crate::pbt::composed::soak_seed::soak_settle

use std::time::Duration;

use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// Canonical id home. The dispatched body is
/// [`crate::pbt::composed::settle_latency::InvComposedSettleBudget`].
pub struct InvSettleBudget;

impl InvSettleBudget {
    pub const ID: InvariantId = InvariantId("inv-settle-budget");
}

/// The p95 interaction→projection-visible SLO (docs/Testing/BugFunnel.md).
pub const SLO: Duration = Duration::from_millis(200);

/// Loaded-dev-machine allowance on top of [`SLO`] for the hard-fail
/// threshold, i.e. 5s. Calibrated against the measured default-scale
/// keystone distribution on a loaded dev machine: max 2111ms (`Outdent`, the
/// org-writeback dominator), then 334ms (`NavigateFocus`) and below — so 5s
/// leaves ~2.4× headroom over the slowest honest transition while the
/// vault-scale IVM regime this exists to catch starts at ~9s. Strict mode
/// (a quiet host) drops it to the bare SLO.
pub const DEFAULT_SLACK: u32 = 25;

/// Bound on the wait itself. A transition that has not made its projection
/// visible within this is a WEDGE, not a slow transition: the harness stops
/// waiting and reds. Without it the 2026-07-28 IVM regime hangs the suite for
/// 23 minutes per transition. Generous on purpose — a scale run's genuinely
/// slow-but-progressing transitions (9–34s observed at 24k blocks) must be
/// MEASURED and reported with their real duration, not truncated to a wedge.
pub const DEFAULT_WEDGE_MS: u64 = 120_000;

/// Hard-fail threshold: `SLO × slack`. `HOLON_PBT_LATENCY_STRICT=1` ⇒ slack 1
/// (bare SLO); otherwise `HOLON_PBT_LATENCY_SLACK` (default
/// [`DEFAULT_SLACK`]).
pub fn hard_budget() -> Duration {
    SLO * slack()
}

pub fn strict() -> bool {
    std::env::var("HOLON_PBT_LATENCY_STRICT").is_ok_and(|v| v == "1")
}

fn slack() -> u32 {
    if strict() {
        return 1;
    }
    std::env::var("HOLON_PBT_LATENCY_SLACK")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n: &u32| n > 0)
        .unwrap_or(DEFAULT_SLACK)
}

/// The wedge bound the harness arms around apply + settle.
/// `HOLON_PBT_LATENCY_WEDGE_MS` (default [`DEFAULT_WEDGE_MS`]).
pub fn wedge_deadline() -> Duration {
    Duration::from_millis(
        std::env::var("HOLON_PBT_LATENCY_WEDGE_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&ms: &u64| ms > 0)
            .unwrap_or(DEFAULT_WEDGE_MS),
    )
}

/// One transition's measured interaction→projection-visible duration:
/// dispatch through the production pipeline plus the 3-projection
/// convergence settle (everything except final GPU paint, which the headless
/// harness has none of).
#[derive(Clone, Debug)]
pub struct SettleSample {
    pub action: String,
    pub elapsed: Duration,
}

/// The verdict. Over the hard threshold ⇒ `Fail` with the RAW duration; under
/// it but over the bare SLO ⇒ `Ok` plus a recorded warning (the slack is a
/// fairness allowance, not a redefinition of the SLO). No sample yet (the
/// pre-first-transition check) ⇒ `Ok`.
pub fn verdict(sample: Option<&SettleSample>) -> InvariantResult {
    let Some(sample) = sample else {
        return InvariantResult::Ok;
    };
    let budget = hard_budget();
    let ms = sample.elapsed.as_millis();
    if sample.elapsed > budget {
        return InvariantResult::Fail(format!(
            "[inv-settle-budget] '{}' took {}ms — interaction→projection-visible past the \
             hard threshold {}ms (p95 SLO {}ms × slack {}{}). This is the latency SLO \
             failing, not a flake: the measurement covers dispatch + the 3-projection \
             convergence settle only.",
            sample.action,
            ms,
            budget.as_millis(),
            SLO.as_millis(),
            slack(),
            if strict() {
                ", HOLON_PBT_LATENCY_STRICT=1"
            } else {
                ""
            },
        ));
    }
    if sample.elapsed > SLO {
        tracing::warn!(
            target: "holon_latency",
            stage = "settle_budget",
            action = %sample.action,
            total_ms = ms as u64,
            slo_ms = SLO.as_millis() as u64,
            "over the p95 SLO but within the machine-load slack",
        );
    }
    InvariantResult::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ms: u64) -> SettleSample {
        SettleSample {
            action: "NavigateFocus".to_string(),
            elapsed: Duration::from_millis(ms),
        }
    }

    #[test]
    fn no_sample_is_ok() {
        assert!(matches!(verdict(None), InvariantResult::Ok));
    }

    #[test]
    fn under_slo_is_ok() {
        assert!(matches!(verdict(Some(&sample(20))), InvariantResult::Ok));
    }

    /// The tooth: the 2026-07-28 IVM regime (~9s per navigation focus) fails
    /// even at the loosest default slack, and the message carries the RAW
    /// duration.
    #[test]
    fn vault_scale_regime_fails_with_its_measured_duration() {
        let InvariantResult::Fail(msg) = verdict(Some(&sample(9_000))) else {
            panic!("9s per interaction must FAIL the settle budget");
        };
        assert!(
            msg.contains("9000ms"),
            "raw duration must be reported: {msg}"
        );
    }

    /// Between the SLO and the slack the invariant passes (loaded dev machine)
    /// but the raw number is still recorded — the slack moves the failure
    /// point, never the measurement.
    #[test]
    fn between_slo_and_slack_passes() {
        assert!(hard_budget() > SLO, "default slack must exceed 1");
        let just_over = SLO + Duration::from_millis(1);
        assert!(matches!(
            verdict(Some(&SettleSample {
                action: "Edit".to_string(),
                elapsed: just_over,
            })),
            InvariantResult::Ok
        ));
    }
}
