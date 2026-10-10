//! The composition roots OUTSIDE `OperationModule` fail loud too.
//!
//! `crates/holon/tests/guard_resolve_fails_loud.rs` pins the five sites of
//! `OperationModule`, but its container loads neither `LoroModule`,
//! `EventInfraModule` nor `OrgModeModule`. Each test here binds ONE type in the
//! real app wiring to a factory that fails, and asserts the boot dies.
//!
//! Two of the four pin a composition site of this lane:
//!
//! - the block cell registry, which pins `SessionCapabilities`
//!   (`holon-loro-wiring/src/event_infra_module.rs:161`)
//! - the block-CRUD authority, which picks the Loro or the SqlOnly write leg
//!   (`holon-app/src/turso_seams.rs:955`)
//!
//! The other two pin that the app boot refuses to proceed, without naming one
//! site: several sites resolve the same type and the first one to run is the
//! one that panics. For `WriteTierAuthority` every site is now REQUIRED
//! (SEC-bound.a), and `wiring.rs:448` requires `ReadOnlyDocuments`, so there
//! is no permissive arm left behind either of them.
//!
//! @pbt kind harness
//! @pbt covers guard-resolve-fails-loud-outside-the-dispatcher — a failed
//! resolve at an event-infra or org-mode composition site stops the boot naming
//! that site, instead of silently degrading the guard it installs
//! @pbt covers guard-resolve-never-boots-degraded — a failed resolve of a
//! write-tier authority or of the read-only document authority never yields a
//! booted app
//!
//! Not covered here: the device-pairing write-tier resolve
//! (`loro_module.rs:705`) is a lazy closure the pairing operation calls, so
//! booting cannot reach it at all.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Provider;
use fluxdi::Shared;
use futures::FutureExt;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const FACTORY_FAILURE: &str = "the guard factory failed on purpose";

/// A binding whose async factory errors: the resolve reports `FactoryFailed`,
/// which is also the shape a panicking factory takes once fluxdi catches
/// factory panics (#158).
fn failing_provider<T>() -> Provider<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    Provider::<T>::root_try_async(|_| async {
        Err::<Shared<T>, String>(FACTORY_FAILURE.to_string())
    })
}

/// Boots the shipped app wiring with `bind` applied to the container, and
/// returns the panic message.
async fn app_boot_failure_message(
    dir: &std::path::Path,
    bind: impl FnOnce(&Injector) + Send + 'static,
) -> String {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("guard.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };

    // The boot is EXPECTED to panic, so its backtrace is noise. Restore the
    // hook this process had, not the default: under `cargo test` all tests
    // share one process, and swapping in the default would strip a sibling's
    // panic output.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = AssertUnwindSafe(holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
        dir.to_path_buf(),
        HashSet::new(),
        Arc::new(holon_api::ConditionBus::new()),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            bind(injector);
            Ok(())
        },
        |_| (),
    ))
    .catch_unwind()
    .await;
    std::panic::set_hook(previous);

    match outcome {
        Ok(_) => panic!(
            "the app boot survived the failed resolve, so a composition root took the failure \
             for an absence and installed its permissive default"
        ),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "a panic with a non-string payload".to_string()),
    }
}

fn boot_and_assert(bind: impl FnOnce(&Injector) + Send + 'static, site: &str, type_name: &str) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let dir = tempfile::tempdir().expect("create tempdir for the vault");
    let message = rt.block_on(app_boot_failure_message(dir.path(), bind));

    assert!(
        message.contains(site),
        "the panic should name the composition site {site}, got: {message}"
    );
    assert!(
        message.contains(type_name),
        "the panic should name the type {type_name}, got: {message}"
    );
    assert!(
        message.contains(FACTORY_FAILURE),
        "the panic should carry the cause, got: {message}"
    );
}

/// Every site that wants the write-tier authority now requires it, so which
/// one panics depends on resolve order and the test does not pin a site. What
/// it pins is the guarantee: an app never boots with a `WriteTierAuthority`
/// that failed to resolve.
#[test]
fn a_failing_write_tier_authority_stops_the_app_boot() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let dir = tempfile::tempdir().expect("create tempdir for the vault");
    let message = rt.block_on(app_boot_failure_message(dir.path(), |injector| {
        injector.override_provider::<dyn holon_core::WriteTierAuthority>(failing_provider());
    }));
    assert!(
        message.contains("WriteTierAuthority"),
        "the panic should name the guard type, got: {message}"
    );
    assert!(
        message.contains(FACTORY_FAILURE),
        "the panic should carry the cause, got: {message}"
    );
}

/// A failed registry resolve read as absence would pin reduced
/// `SessionCapabilities` for the whole session and route cell writes through
/// SQL instead of Loro.
#[test]
fn a_failing_block_cell_registry_stops_the_event_infra_wiring() {
    boot_and_assert(
        |injector| {
            injector.override_provider::<holon_loro::block_cell_registry::BlockCellRegistry>(
                failing_provider(),
            );
        },
        "[EventInfraModule] block cell registry for the operation provider",
        "BlockCellRegistry",
    );
}

/// `ReadOnlyDocuments` IS the vault's write-tier authority: absence makes every
/// block in a read-only-format document editable again. `wiring.rs:448`
/// requires it, so the loud failure comes from there; the org-mode arm this
/// lane made loud (`holon-orgmode/src/di.rs:808`) bites in a container that
/// requires none, which this harness cannot build — the org-standalone one.
/// Pinned here: the app boot never proceeds with an authority that failed to
/// resolve.
#[test]
fn a_failing_read_only_document_authority_stops_the_app_boot() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let dir = tempfile::tempdir().expect("create tempdir for the vault");
    let message = rt.block_on(app_boot_failure_message(dir.path(), |injector| {
        injector.override_provider::<Arc<holon_core::ReadOnlyDocuments>>(failing_provider());
    }));
    assert!(
        message.contains("ReadOnlyDocuments"),
        "the panic should name the authority type, got: {message}"
    );
}

/// The block-CRUD authority picks the write leg. Registered here with an ASYNC
/// factory while the seam resolves it synchronously — a real wiring fault
/// (`AsyncFactoryRequiresAsyncResolve`), and NOT "no Loro in this session".
#[test]
fn a_wrongly_registered_crud_authority_stops_the_org_seam() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let dir = tempfile::tempdir().expect("create tempdir for the vault");
    let message = rt.block_on(app_boot_failure_message(dir.path(), |injector| {
        injector.override_provider::<holon_core::CrudAuthority>(Provider::root_async(|_| async {
            unreachable!("a sync resolve must refuse this provider before it runs")
        }));
    }));

    assert!(
        message.contains("[OrgModeSeam] block-CRUD authority"),
        "the panic should name the org-mode seam, got: {message}"
    );
    assert!(
        message.contains("CrudAuthority"),
        "the panic should name the type, got: {message}"
    );
}
