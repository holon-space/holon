//! A clock whose instant or zone is out of range must not take the boot with
//! it.
//!
//! The clock seed already degrades: `start_clock`'s `Err` becomes a
//! `ClockNotRunning` condition and the boot continues. Two clock paths bypassed
//! that degrade by panicking instead of returning an `Err` — the
//! `from_timestamp_millis` expect in `Grain::sample` and the out-of-range
//! `FixedOffset` panic in `local_offset`. A clock out of range therefore
//! aborted the boot past its own degrade.
//!
//! What is asserted: the session boots and discloses `clock-not-running` for
//! an out-of-range instant AND for an out-of-range zone.
//!
//! @pbt kind harness
//! @pbt covers clock-range-boots-degraded — an out-of-range clock instant or
//! zone is disclosed, not fatal

use std::collections::HashSet;
use std::sync::Arc;

use holon_api::Clock;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
}

/// Boot the shipped wiring with `clock` injected and hand back the degraded
/// conditions in effect. Returning at all is half the assertion: the panic this
/// file was written against never got here.
async fn boot_with_clock(clock: Arc<dyn Clock>) -> Vec<holon_api::Condition> {
    let dir = tempfile::tempdir().expect("tempdir");
    let holon_config = HolonConfig {
        db_path: Some(dir.path().join("degraded.db")),
        vault: VaultConfig {
            root: Some(dir.path().to_path_buf()),
        },
        ..Default::default()
    };
    let (session, _engine, bus) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
        dir.path().to_path_buf(),
        HashSet::new(),
        Arc::new(ConditionBus::new()),
        move |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            let injected = holon_api::InjectedClock(clock.clone());
            injector.provide::<holon_api::InjectedClock>(fluxdi::Provider::root(move |_| {
                injected.clone().into()
            }));
            Ok(())
        },
        |injector| {
            injector
                .try_resolve::<Arc<ConditionBus>>()
                .map(|b| (*b).clone())
        },
    )
    .await
    .expect(
        "the shipped wiring must boot a session even when the clock's instant or zone is out of \
         range — the clock seed has a degrade and a panic jumps past it",
    );
    let bus = bus.expect("the composition root registers a ConditionBus");
    let current = bus.subscribe().current;
    drop(session);
    current
}

fn clock_not_running_error(current: &[holon_api::Condition]) -> String {
    current
        .iter()
        .find_map(|c| match &c.reason {
            ConditionKind::ClockNotRunning { error } => Some(error.clone()),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "an unusable clock must be disclosed as `clock-not-running` so the user learns \
                 time-dependent views are stale; the bus carries {:?}",
                current
                    .iter()
                    .map(|c| (&c.subject, c.reason.condition_kind()))
                    .collect::<Vec<_>>()
            )
        })
}

/// `i64::MAX` milliseconds is past `DateTime`'s range, so every conversion of
/// the instant fails. This is the shape a corrupt or hostile clock source has.
#[test]
fn an_out_of_range_clock_instant_is_disclosed_rather_than_fatal() {
    let clock: Arc<dyn Clock> = Arc::new(holon_api::TestClock::with_utc_offset(i64::MAX, 0));
    let current = runtime().block_on(boot_with_clock(clock));

    let error = clock_not_running_error(&current);
    assert!(
        error.contains("range"),
        "the disclosure must say the clock reading is out of range, which is what distinguishes \
         it from a store failure; got {error:?}"
    );
    assert_eq!(
        current
            .iter()
            .filter(|c| c.subject == holon_api::condition_bus::CLOCK_SUBJECT)
            .count(),
        1,
        "exactly one clock disclosure stands; the bus carries {:?}",
        current
            .iter()
            .map(|c| (&c.subject, c.reason.condition_kind()))
            .collect::<Vec<_>>()
    );
}

/// A valid instant in a zone `FixedOffset` cannot represent (its range is
/// ±24h exclusive). An injected offset is a plain `i32` from the host or an
/// embedder, so nothing upstream proves it fits.
#[test]
fn an_out_of_range_clock_zone_is_disclosed_rather_than_fatal() {
    let millis = chrono::NaiveDate::from_ymd_opt(2026, 10, 14)
        .expect("a real date")
        .and_hms_opt(12, 0, 0)
        .expect("a real time")
        .and_utc()
        .timestamp_millis();
    let clock: Arc<dyn Clock> = Arc::new(holon_api::TestClock::with_utc_offset(millis, 90_000));
    let current = runtime().block_on(boot_with_clock(clock));

    let error = clock_not_running_error(&current);
    assert!(
        error.contains("90000"),
        "the disclosure must name the offset the clock reported, since that is the value the \
         embedder has to fix; got {error:?}"
    );
}
