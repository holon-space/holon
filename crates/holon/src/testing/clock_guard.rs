//! A clock that did not start degrades a production boot
//! (`clock-not-running`) but fails every test harness boot: a harness that
//! wants it injects the fault and asserts the condition itself.

use std::sync::Arc;

use holon_api::ConditionBus;
use holon_api::ConditionKind;

/// Panics if `bus` carries `clock-not-running`. Call once the harness boot
/// has returned.
pub fn assert_clock_running(bus: &ConditionBus) {
    let not_running: Vec<String> = bus
        .current()
        .into_iter()
        .filter(|c| c.reason.condition_kind() == ConditionKind::CLOCK_NOT_RUNNING)
        .map(|c| c.reason.detail(&c.subject).body.join(" | "))
        .collect();
    assert!(
        not_running.is_empty(),
        "[{}] the harness booted without a running clock, a fault no test declared: {}",
        ConditionKind::CLOCK_NOT_RUNNING,
        not_running.join("; ")
    );
}

/// [`assert_clock_running`] on the container's bus.
pub fn assert_clock_running_in(injector: &fluxdi::Injector) {
    let bus: Arc<ConditionBus> = (*injector
        .try_resolve::<Arc<ConditionBus>>()
        .expect("every container provides a ConditionBus"))
    .clone();
    assert_clock_running(&bus);
}
