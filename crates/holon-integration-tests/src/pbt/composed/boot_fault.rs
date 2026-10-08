//! The keystone's `BootFault` axis: what the model expects each drawn fault to
//! disclose, and the SUT side that injects it.

use std::sync::Arc;
use std::time::Duration;

use holon_pbt_core::BootFault;

use crate::pbt::conditions_state::Panicked;
use crate::pbt::reference_state::ReferenceState;

/// The payload of the task panic a [`BootFault::TaskPanic`] injects.
pub const TASK_PANIC_MESSAGE: &str = "keystone boot fault: injected task panic";

/// Patience for the panic's condition to reach the bus, which it does through
/// the hook's forwarder thread.
const TASK_PANIC_DISCLOSURE: Duration = Duration::from_secs(5);

/// One call site for both the model and the injection, so the location the
/// model expects is the location the panic reports.
fn injected_task_panic(panic_now: bool) -> String {
    injected_task_panic_site(panic_now)
}

#[track_caller]
fn injected_task_panic_site(panic_now: bool) -> String {
    if panic_now {
        panic!("{TASK_PANIC_MESSAGE}");
    }
    let site = std::panic::Location::caller();
    format!("{}:{}:{}", site.file(), site.line(), site.column())
}

/// The model of `fault` at boot: the case starts with these disclosures.
pub fn model_boot_fault(state: &mut ReferenceState, fault: BootFault) {
    match &fault {
        BootFault::None => {}
        BootFault::PriorCrash(prior) => state.conditions.previous_run_panicked(&Panicked {
            location: prior.location.clone(),
            message: prior.message.clone(),
        }),
        BootFault::TaskPanic => state.conditions.task_panicked(Panicked {
            location: injected_task_panic(false),
            message: TASK_PANIC_MESSAGE.to_string(),
        }),
    }
    state.boot_fault = fault;
}

/// The SUT side of a [`BootFault::TaskPanic`], run once the app has started:
/// a spawned task panics, and the boot waits (bounded) for its disclosure so
/// the first invariant check sees it. A disclosure that never comes is left
/// for `inv-conditions-match-ref` to report.
pub async fn inject_task_panic(bus: Arc<holon_api::ConditionBus>) {
    let joined = tokio::spawn(async { injected_task_panic(true) }).await;
    let error = joined.expect_err("the injected task must panic");
    assert!(
        error.is_panic(),
        "the injected task ended without a panic: {error}"
    );
    crate::test_tracing::SpanCollector::global().take_injected_panic(TASK_PANIC_MESSAGE);
    let deadline = tokio::time::Instant::now() + TASK_PANIC_DISCLOSURE;
    while tokio::time::Instant::now() < deadline
        && !bus
            .current()
            .iter()
            .any(|c| c.reason.condition_kind() == holon_api::ConditionKind::TASK_PANICKED)
    {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The drawn fault, shrinking toward `None`. `HOLON_PBT_BOOT_FAULT=prior` or
/// `=task` pins one variant (`prior` still draws its record).
pub fn boot_fault_strategy() -> proptest::strategy::BoxedStrategy<BootFault> {
    use proptest::strategy::Strategy;
    match std::env::var("HOLON_PBT_BOOT_FAULT").as_deref() {
        Ok("prior") => holon_pbt_core::any_boot_fault()
            .prop_filter("pinned to PriorCrash", |f| {
                matches!(f, BootFault::PriorCrash(_))
            })
            .boxed(),
        Ok("task") => proptest::strategy::Just(BootFault::TaskPanic).boxed(),
        Ok(other) => panic!("HOLON_PBT_BOOT_FAULT={other:?}: expected `prior` or `task`"),
        Err(_) => holon_pbt_core::any_boot_fault(),
    }
}
