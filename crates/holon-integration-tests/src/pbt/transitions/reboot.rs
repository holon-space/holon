//! Transition: reboot the app over its retained store.
//!
//! @pbt rung external
//!   drop the storage engine and boot again over the SAME on-disk store
//!   (`SutAppLifecycle::reboot`).
//! @pbt covers reboot-persistence — what survives a real restart

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReboot;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Restart the app: shut the storage engine down and boot again over the same
/// on-disk store. The persistence-across-restart class `SimulateRestart` cannot
/// reach — it only touches org files so the RUNNING controller re-parses them,
/// which keeps the engine and therefore can never observe what the store itself
/// restored.
///
/// The oracle is the split between the two halves of the app's state: the store
/// (blocks, edges, focus, navigation history, widget-open bits) survives
/// untouched, while everything the process held in memory (the editor buffer
/// and its caret) is gone. Nothing is re-seeded — the SUT asserts the block set
/// is byte-identical across the boot, so a reboot that silently re-ingested the
/// vault fails loud rather than doubling it.
///
/// **Reproducing anything about a reboot needs the WEIGHT, not just the gate.**
/// `HOLON_PBT_REBOOT=1` alone leaves the weight at 1 against ~70 other
/// transitions, so a short run (`just keystone-smoke`, 4 cases) draws zero
/// reboots and passes — a green that says nothing. The repro is
/// `HOLON_PBT_REBOOT=1 HOLON_PBT_FORCE_FULL=1 HOLON_PBT_REBOOT_WEIGHT=40
/// just pbt general 8`: `FORCE_FULL` because the Turso wiring is required, the
/// weight because otherwise reboots are the rarest draw in the alphabet.
///
/// **Open oracle gap:** `drawer_open` is the one `RefReboot` claim no run has
/// checked. The registered draw-dependent `drawer-open-matches-ref` known red
/// ends a weighted run on its first case, so the only configuration that
/// actually draws a `Reboot` softens `inv-drawer-open-matches-ref` to `warn` —
/// which is exactly the invariant that would judge the claim. It is asserted by
/// the model and unverified in practice until that known red is fixed.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the app is rebooted")]
pub struct Reboot;

/// Draw weight, `HOLON_PBT_REBOOT_WEIGHT` (default 1). A reboot costs a whole
/// boot — seconds, not milliseconds — so the default keeps it rare; the red
/// hunt raises it to make reboots the dominant transition of a run.
fn reboot_weight() -> u32 {
    std::env::var("HOLON_PBT_REBOOT_WEIGHT")
        .ok()
        .map(|s| {
            s.parse().unwrap_or_else(|e| {
                panic!("HOLON_PBT_REBOOT_WEIGHT={s:?} is not a u32: {e}");
            })
        })
        .unwrap_or(1)
}

impl<R: RefLifecycle + RefLayout + RefReboot> TransitionFactory<R> for Reboot {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only, like `SimulateRestart`: the reboot re-opens `test.db` and
        // re-runs `ensure_schema` over persisted matview/DBSP state, which is
        // the whole observation. A Loro-only draw has no such store to re-open.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // OFF by default (`HOLON_PBT_REBOOT=1` opts in), same shape as
        // `HOLON_PBT_DOC_RENAME`. Not a hedge about the reference model: a
        // reboot shuts the Turso actor down while the PREVIOUS boot's watcher
        // tasks (org-writeback supervisor, `UiWatcher`, `holon_rule_watcher`,
        // the debounced org re-render) are still running, so every reboot
        // raises a burst of `Actor channel closed` ERRORs and reds
        // `inv-no-observed-errors`. Holon has no orderly session shutdown that
        // stops those tasks — only ending the process does — so closing this
        // needs a production change, not a harness one. Until then the random
        // alphabet stays unchanged and the gate is the reproducer.
        // See `docs/Testing/bugfunnel/entries/2026-09-08-reboot-orphans-watcher-tasks.
        // md`.
        let enabled = std::env::var("HOLON_PBT_REBOOT").is_ok();
        let checks: Vec<Validated<(), Reason>> = vec![
            check(enabled, Reason::PreconditionFailed),
            Reboot.preconditions(state),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| (reboot_weight(), Just(Reboot).boxed()))
    }
}

impl<R: RefLifecycle + RefLayout + RefReboot> TransitionRef<R> for Reboot {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!state.all_block_ids().is_empty(), Reason::BlockStateEmpty),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.reboot_drops_in_memory_state();
    }
}

crate::cap_transition! {
    Reboot: SutAppLifecycle,
    where R: [ RefLifecycle + RefLayout + RefReboot ],
    |_me, _state, sut| {
        // The COMPOSED harness never reaches this body: a reboot replaces the
        // whole `CapMap` and the slice handle, which `apply_transition` cannot
        // do, so `ComposedSut::apply` intercepts it via
        // `ComposedSlice::is_reboot` (harness.rs). This arm serves the
        // non-composed harnesses that host the cap directly.
        sut.reboot().await;
    }
    sql_budget: |_me, state| {
        // The boot's own cost is OUTSIDE this window: the composed harness opens
        // a reboot tick's measurement window after the rebuild
        // (`ComposedSlice::note_tick_start`, called last in `ComposedSut::
        // rebooted`), exactly as the FIRST boot sits outside every budget window.
        // What remains inside is the post-window bookkeeping only —
        // `foreign_ids` + `align_ids` — which measured 0/0/0
        // (`reads=0 (dedup 0) writes=0 ddl=0`, lane-logs/rev2-redhunt-51601.log).
        // Set from that measurement; n=1, so re-measure and widen with numbers,
        // never pre-emptively.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4 + docs_tolerance(state),
        }
    }
}
