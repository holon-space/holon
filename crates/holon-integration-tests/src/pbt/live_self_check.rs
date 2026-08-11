//! The class-1 invariant suite the running app's `run_self_checks` MCP tool
//! answers with.
//!
//! It runs the SAME [`composed_invariant_catalog`] the keystone PBT runs, over
//! a `CapMap` whose only SUT capability is a snapshot of the live app's CDC
//! mirrors. The reference side is
//! [`holon_pbt_core::null_ref::null_ref_caps`]: a class-1 invariant never reads
//! it, and one that does self-reports as a loud `Skipped` (the `NullRef` panic
//! names the method) instead of silently comparing against nothing.
//!
//! Every catalog entry the suite considers appears in the report. An invariant
//! whose live source does not exist is `Skipped` WITH the missing cap named —
//! never absent, never a vacuous `Pass`.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use holon_mcp::self_check::CheckOutcome;
use holon_mcp::self_check::CheckReport;
use holon_mcp::self_check::LiveSelfCheckSuite;
use holon_mcp::self_check::LiveSnapshot;
use holon_mcp::self_check::SelfCheckReport;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapSet;
use holon_pbt_core::composition::Needs;
use holon_pbt_core::invariant::InvariantResult;
use holon_pbt_core::null_ref::null_ref_caps;

use super::composed::catalog::composed_invariant_catalog;

/// Class-1 by the catalog's criterion (they declare no `Ref*` cap) but excluded
/// here: these are the dogfood-recorder plan's class-3 temporal/budget checks.
/// They score a per-tick accounting window (SQL counts, settle time, matview
/// deltas since the last transition), which a one-shot sweep of a running app
/// simply does not have — running them would report an accounting artifact as a
/// product defect.
const CLASS_THREE_EXCLUDED: &[&str] = &[
    "inv-sql-budget",
    "inv-settle-budget",
    "inv-complexity-class-trend",
    "inv-matview-consistent-with-recompute",
    "inv-no-steady-reseed-leak",
];

/// The registered suite. Stateless — the snapshot arrives per call.
pub struct LiveSelfCheck;

/// Answers `SutBackend` from the captured live mirrors, and ONLY from them.
struct SnapshotBackend(LiveSnapshot);

#[async_trait::async_trait(?Send)]
impl SutBackend for SnapshotBackend {
    async fn live_block_snapshot(&self) -> Vec<holon_api::Block> {
        self.0.live_blocks.clone()
    }

    async fn block_raw_snapshot(&self) -> Vec<holon_api::Block> {
        panic!(
            "no live source: SutBackend::block_raw_snapshot (the running app \
             exposes the CDC mirror, not the block_raw write side)"
        )
    }

    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        self.0.focus_roots.clone()
    }
}

impl LiveSelfCheckSuite for LiveSelfCheck {
    fn run(&self, snapshot: LiveSnapshot) -> Result<SelfCheckReport, String> {
        let started = Instant::now();
        let live_block_count = snapshot.live_blocks.len();

        // The invariant futures are `!Send`; the caller runs us on
        // `spawn_blocking`, so a private current-thread runtime is ours alone.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("live self-check: cannot build the check runtime: {e}"))?;

        let mut sut = CapMap::new();
        sut.insert(Arc::new(SnapshotBackend(snapshot)) as Arc<dyn SutBackend>);
        let ref_ = null_ref_caps();
        let have_sut = sut.cap_set();
        let have_ref = ref_.cap_set();

        // Built BEFORE the hook guard: a panic in catalog construction must
        // reach the app's own hook, not a filter this sweep installed.
        let catalog = composed_invariant_catalog();

        // `NullRef` panics and the `block_raw_snapshot` refusal are this
        // suite's DATA, not failures. The guard filters exactly those two
        // payloads, forwards every other panic (a sibling thread's included),
        // holds the process-wide sweep lock, and restores on every exit.
        let _hook = holon_pbt_core::panic_filter::SweepPanicHook::install();

        let mut checks = Vec::new();
        for inv in &catalog {
            let id = inv.id().0;
            let needs = inv.needs();

            // Class 2: it declares a reference model we do not have here. Not
            // this suite's subject at all — the class-1 suite does not report
            // on invariants outside its class.
            if !needs.ref_present.is_empty() {
                continue;
            }

            if CLASS_THREE_EXCLUDED.contains(&id) {
                checks.push(skip(
                    id,
                    "class-3 temporal/budget check: scores a per-tick accounting \
                     window a one-shot live sweep does not have",
                ));
                continue;
            }

            if !needs.selected_against(&have_sut, &have_ref) {
                checks.push(skip(id, &unselected_reason(&needs, &have_sut)));
                continue;
            }

            let caught = std::panic::catch_unwind(AssertUnwindSafe(|| {
                rt.block_on(inv.check_boxed(&sut, &ref_))
            }));
            let outcome = match caught {
                Ok(InvariantResult::Ok) => CheckOutcome::Pass,
                Ok(InvariantResult::Fail(detail)) => CheckOutcome::Fail { detail },
                Ok(InvariantResult::Skipped(reason)) => CheckOutcome::Skipped { reason },
                Err(payload) => {
                    let message = panic_message(payload);
                    // Only `NullRef` and `SnapshotBackend` may mint these two
                    // prefixes: they are how a cap says "ask me and you get a
                    // skip". Minting one anywhere else would launder a real
                    // panic into a skip.
                    if message.starts_with(holon_pbt_core::panic_filter::NO_LIVE_SOURCE_PREFIX)
                        || message.starts_with(holon_pbt_core::panic_filter::CLASS_TWO_PREFIX)
                    {
                        CheckOutcome::Skipped { reason: message }
                    } else {
                        CheckOutcome::Fail {
                            detail: format!("invariant body panicked: {message}"),
                        }
                    }
                }
            };
            checks.push(CheckReport {
                id: id.to_string(),
                outcome,
            });
        }

        Ok(SelfCheckReport::from_checks(
            checks,
            started.elapsed().as_millis(),
            live_block_count,
        ))
    }
}

fn skip(id: &str, reason: &str) -> CheckReport {
    CheckReport {
        id: id.to_string(),
        outcome: CheckOutcome::Skipped {
            reason: reason.to_string(),
        },
    }
}

/// Name the caps that actually blocked selection, so the report says WHICH live
/// source is missing rather than "not selected".
fn unselected_reason(needs: &Needs, have_sut: &CapSet) -> String {
    let missing: Vec<&str> = needs
        .sut_present
        .iter()
        .filter(|c| !have_sut.contains(c))
        .map(CapId::name)
        .collect();
    let forbidden: Vec<&str> = needs
        .sut_absent
        .iter()
        .filter(|c| have_sut.contains(c))
        .map(CapId::name)
        .collect();
    match (missing.is_empty(), forbidden.is_empty()) {
        (false, true) => format!(
            "no live source for SUT capability {} — the live snapshot hosts only SutBackend",
            missing.join(", ")
        ),
        (true, false) => format!(
            "requires SUT capability {} to be ABSENT, but the live snapshot hosts it",
            forbidden.join(", ")
        ),
        (false, false) => format!(
            "no live source for SUT capability {}; also requires {} to be absent",
            missing.join(", "),
            forbidden.join(", ")
        ),
        (true, true) => unreachable!("selection failed but every declared SUT cap is satisfied"),
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    "<non-string panic payload>".to_string()
}
