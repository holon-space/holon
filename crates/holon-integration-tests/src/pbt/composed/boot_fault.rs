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

/// The site is `#[track_caller]`, so its panic and its `Location::caller`
/// both report the call below: the model expects the panic's own location.
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
        BootFault::PriorCrash(prior) => state.conditions.prior_crash(&Panicked {
            location: prior.location.clone(),
            message: prior.message.clone(),
        }),
        BootFault::TaskPanic => state.conditions.task_panicked(Panicked {
            location: injected_task_panic(false),
            message: TASK_PANIC_MESSAGE.to_string(),
        }),
        BootFault::MissingVaultDir => {}
    }
    state.boot_fault = fault;
}

/// The SUT side of a [`BootFault::TaskPanic`], run once the app has started:
/// a spawned task panics, and the boot waits (bounded) for its disclosure so
/// the first invariant check sees it. A disclosure that never comes is left
/// for `inv-conditions-match-ref` to report.
pub async fn inject_task_panic(bus: Arc<holon_conditions::ConditionBus>) {
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
            .any(|c| c.reason.condition_kind() == holon_conditions::ConditionKind::TASK_PANICKED)
    {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The drawn fault, shrinking toward `None`. `HOLON_PBT_BOOT_FAULT=prior`,
/// `=task` or `=missing-vault` pins one variant (`prior` still draws its
/// record).
pub fn boot_fault_strategy() -> proptest::strategy::BoxedStrategy<BootFault> {
    use proptest::strategy::Strategy;
    match std::env::var("HOLON_PBT_BOOT_FAULT").as_deref() {
        Ok("prior") => holon_pbt_core::any_boot_fault()
            .prop_filter("pinned to PriorCrash", |f| {
                matches!(f, BootFault::PriorCrash(_))
            })
            .boxed(),
        Ok("task") => proptest::strategy::Just(BootFault::TaskPanic).boxed(),
        Ok("missing-vault") => proptest::strategy::Just(BootFault::MissingVaultDir).boxed(),
        Ok(other) => {
            panic!("HOLON_PBT_BOOT_FAULT={other:?}: expected `prior`, `task` or `missing-vault`")
        }
        Err(_) => holon_pbt_core::any_boot_fault(),
    }
}

/// The error text an armed [`holon_pbt_core::BootStepFault`] fails its step
/// with.
pub fn boot_step_fault_error(fault: holon_pbt_core::BootStepFault) -> String {
    use holon_app::wiring::step_failure;
    use holon_pbt_core::BootStepFault;
    let point = match fault {
        BootStepFault::IntegrationProjection => step_failure::INTEGRATION_STATE_PROJECTION,
        BootStepFault::ActionWatchers => step_failure::START_ACTION_WATCHERS,
        BootStepFault::LayoutSeed => step_failure::SEED_DEFAULT_LAYOUT,
        BootStepFault::SyncController => return INJECTED_FACTORY_ERROR.to_string(),
        BootStepFault::IntegrationSupervisor => return INJECTED_FACTORY_ERROR.to_string(),
        BootStepFault::VaultWatcher => return INJECTED_WATCHER_BUILD_ERROR.to_string(),
        BootStepFault::VaultWatchRegistration => return INJECTED_WATCH_ERROR.to_string(),
    };
    format!("[crash-injection] boot step `{point}` fails")
}

const INJECTED_FACTORY_ERROR: &str = "boot step fault: injected factory error";
const INJECTED_WATCHER_BUILD_ERROR: &str = "[crash-injection] the OS refuses a file watcher";
const INJECTED_WATCH_ERROR: &str = "[crash-injection] arming the watcher fails";

/// Arms `fault` on a container whose frontend is already registered (the
/// boot's extra setup): a step fails at its injection point, a factory by
/// override. A watcher fault needs the production change source
/// ([`BootStepFault::fires_in_os_watcher`]).
pub fn arm_boot_step_fault(fault: holon_pbt_core::BootStepFault, injector: &fluxdi::Injector) {
    use holon_app::wiring::step_failure;
    use holon_filesystem::crash_injection::arm;
    use holon_pbt_core::BootStepFault;
    match fault {
        BootStepFault::IntegrationProjection => arm(step_failure::INTEGRATION_STATE_PROJECTION),
        BootStepFault::ActionWatchers => arm(step_failure::START_ACTION_WATCHERS),
        BootStepFault::LayoutSeed => arm(step_failure::SEED_DEFAULT_LAYOUT),
        BootStepFault::SyncController => {
            injector.override_provider::<holon_loro::LoroSyncControllerHandle>(
                fluxdi::Provider::root_try_async(|_| async {
                    Err::<fluxdi::Shared<holon_loro::LoroSyncControllerHandle>, _>(
                        INJECTED_FACTORY_ERROR,
                    )
                }),
            );
        }
        BootStepFault::IntegrationSupervisor => {
            injector.override_provider::<holon_app::IntegrationSupervisor>(
                fluxdi::Provider::root_try_async(|_| async {
                    Err::<fluxdi::Shared<holon_app::IntegrationSupervisor>, _>(
                        INJECTED_FACTORY_ERROR,
                    )
                }),
            );
        }
        BootStepFault::VaultWatcher => arm("build_watcher"),
        BootStepFault::VaultWatchRegistration => arm("arm_watcher"),
    }
}
