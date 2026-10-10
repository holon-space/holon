//! A guard that fails to RESOLVE must stop the boot, never degrade to the
//! permissive default the container uses when no guard is bound at all.
//!
//! "Nothing bound it" and "it is bound but its factory failed" are different
//! facts. The first is a legal container shape (SqlOnly, no vault, a test
//! container with no sharing policies); the second is a wiring fault. Taking
//! the second for the first installs `InertBoundaryEnforcer`, `InertNetGuard`,
//! no write-tier authority, or SQL write-authority reads — four permissive
//! defaults, silently.
//!
//! Each test binds ONE guard type to a factory that errors, so the resolve
//! fails with `FactoryFailed` rather than `ServiceNotProvided`, and asserts the
//! boot panics naming both the composition site and the type. The site matters:
//! `WriteAuthorityReads` is resolved loudly a second time while the engine is
//! built, so a test that only asks "did the boot panic" would pass without the
//! dispatcher's shape gate ever being fixed.

use std::panic::AssertUnwindSafe;

use fluxdi::Provider;
use fluxdi::Shared;
use futures::FutureExt;
use holon::api::net_guard::NetGuard;
use holon::di::test_helpers::create_test_engine_with_setup;
use holon_core::BoundaryEnforcer;
use holon_core::OperationObserver;
use holon_core::WriteAuthorityReads;
use holon_core::WriteTierAuthority;

const FACTORY_FAILURE: &str = "the guard factory failed on purpose";

/// A binding whose factory errors. The resolve reports `FactoryFailed`, which
/// is also the shape a panicking guard factory takes once fluxdi catches
/// factory panics at its head (#158).
fn failing_provider<T>() -> Provider<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    Provider::<T>::root_try_async(|_| async {
        Err::<Shared<T>, String>(FACTORY_FAILURE.to_string())
    })
}

/// Boots a container with `bind` applied to it and returns the panic message.
async fn boot_failure_message(bind: impl FnOnce(&fluxdi::Injector) + Send) -> String {
    // The boot is EXPECTED to panic, so its backtrace is noise. Restore the
    // hook this process had, not the default: under `cargo test` all tests
    // share one process, and swapping in the default would strip a sibling's
    // panic output.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = AssertUnwindSafe(create_test_engine_with_setup(
        ":memory:".into(),
        |injector| {
            bind(injector);
            Ok(())
        },
    ))
    .catch_unwind()
    .await;
    std::panic::set_hook(previous);
    match outcome {
        Ok(_) => panic!(
            "the boot survived the failed resolve, so it installed the permissive default \
             instead of failing loudly"
        ),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "a panic with a non-string payload".to_string()),
    }
}

/// Boots a container in which `T` is bound to a failing factory.
async fn guard_boot_failure_message<T>() -> String
where
    T: ?Sized + Send + Sync + 'static,
{
    boot_failure_message(|injector| {
        injector.provide::<T>(failing_provider::<T>());
    })
    .await
}

fn assert_names_site_and_type(message: &str, site: &str, type_name: &str) {
    assert!(
        message.contains(site),
        "the panic should name the composition site {site}, got: {message}"
    );
    assert!(
        message.contains(type_name),
        "the panic should name the guard type {type_name}, got: {message}"
    );
    assert!(
        message.contains(FACTORY_FAILURE),
        "the panic should carry the cause, got: {message}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_boundary_enforcer_factory_stops_the_boot() {
    let message = guard_boot_failure_message::<dyn BoundaryEnforcer>().await;
    assert_names_site_and_type(
        &message,
        "[OperationModule] boundary seam",
        "BoundaryEnforcer",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_net_guard_factory_stops_the_boot() {
    let message = guard_boot_failure_message::<dyn NetGuard>().await;
    assert_names_site_and_type(&message, "[OperationModule] net gate", "NetGuard");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_write_tier_authority_factory_stops_the_boot() {
    let message = guard_boot_failure_message::<dyn WriteTierAuthority>().await;
    assert_names_site_and_type(
        &message,
        "[OperationModule] write-tier gate",
        "WriteTierAuthority",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_write_authority_reads_factory_stops_the_boot() {
    let message = guard_boot_failure_message::<dyn WriteAuthorityReads>().await;
    assert_names_site_and_type(
        &message,
        "[OperationModule] shape gate",
        "WriteAuthorityReads",
    );
}

/// The operation log is an observer, and the observer SET took the same
/// shortcut: any resolve error gave zero observers. The only observer prod
/// binds is `OperationLogObserver`, so that silently dropped undo and the audit
/// trail.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_operation_observer_stops_the_boot() {
    let message = boot_failure_message(|injector| {
        injector.provide_into_set::<dyn OperationObserver>(failing_provider());
    })
    .await;
    assert_names_site_and_type(
        &message,
        "[OperationModule] operation observers",
        "OperationObserver",
    );
}

/// The other half of the rule (SEC-bound.a): a container that names no guard
/// still boots, because `crate::di::guard_bindings` binds the EXPLICIT no-op
/// guards for it before anything resolves the dispatcher. The dispatcher itself
/// has no permissive branch left to take.
#[tokio::test(flavor = "multi_thread")]
async fn a_container_that_names_no_guard_gets_the_explicit_no_ops() {
    let engine = create_test_engine_with_setup(":memory:".into(), |_| Ok(()))
        .await
        .expect("a container that names no guard boots on the explicit no-op guards");
    // A boot is the whole claim: the dispatcher resolved all four as required.
    drop(engine);
}
