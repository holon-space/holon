//! The window says the vault is loading until the default layout is seeded,
//! and says why when the seed cannot happen (D98.b).
//!
//! The seed runs after the org initial scan (D83.a), in the background of a
//! production boot, while the window is already open. Before it lands there is
//! no `block:root-layout`, so the shell would paint no sidebars and no main
//! panel — an empty window that looks like an empty vault. The boot ledger
//! (`BootReport`, step `seed-default-layout`) is the signal: in the background
//! means loading, failed means the vault did not load.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! vault_loading_placeholder_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe), and the failure case arms a process-global crash point.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::platform::BootStep;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_integration_tests::test_environment::TestEnvironmentBuilder;
use pbt_harness::windowed_wide::real_text_system;

const LOADING: &str = "Loading vault…";
const FAILED: &str = "The vault could not be loaded";
const SCAN_CRASH: &str = "[crash-injection] walking the vault fails";
const CONTROLLER_DEAD: &str = "ended before it reported the initial scan";
const MAIN_PANEL: &str = "block:default-main-panel";

const NOTES_ORG: &str = "* A note\n:PROPERTIES:\n:ID: vault-loading-note\n:END:\n";

fn painted_text(bounds: &BoundsRegistry) -> Vec<String> {
    let mut out: Vec<String> = bounds
        .all_elements()
        .into_iter()
        .filter_map(|(_, info)| info.displayed_text)
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn paints_main_panel(bounds: &BoundsRegistry) -> bool {
    bounds
        .all_elements()
        .into_iter()
        .any(|(_, info)| info.entity_id.as_deref() == Some(MAIN_PANEL))
}

fn headless_app() -> HeadlessAppContext {
    let assets: Arc<dyn AssetSource> = Arc::new(());
    HeadlessAppContext::with_platform(real_text_system(), assets, || {
        gpui_platform::current_headless_renderer()
    })
}

fn pump(app: &mut HeadlessAppContext, bounds: &BoundsRegistry, runtime: &tokio::runtime::Runtime) {
    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
    app.run_until_parked();
    app.advance_clock(Duration::from_secs(1));
    app.run_until_parked();
    bounds.flush();
}

/// Pump frames until `done` holds; panic with the painted text after `timeout`.
fn pump_until(
    app: &mut HeadlessAppContext,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    timeout: Duration,
    what: &str,
    done: impl Fn(&BoundsRegistry) -> bool,
) {
    let start = Instant::now();
    while !done(bounds) {
        assert!(
            start.elapsed() < timeout,
            "the window did not reach `{what}` within {timeout:?}. Painted text: {:#?}",
            painted_text(bounds)
        );
        pump(app, bounds, runtime);
    }
}

fn launch(
    app: &mut HeadlessAppContext,
    env: &TestEnvironment,
    runtime: &tokio::runtime::Runtime,
    bounds: &BoundsRegistry,
    title: &str,
) -> holon_gpui::RebindHandle {
    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after boot");
    app.update(|cx| {
        launch_holon_window_rebindable(
            session,
            engine,
            runtime.handle().clone(),
            NavigationState::new(),
            bounds.clone(),
            None,
            None,
            title,
            cx,
        )
    })
    .expect("window opened over the booted session")
}

fn shut_down(app: HeadlessAppContext, rebind: holon_gpui::RebindHandle) {
    let mut app = app;
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}

#[test]
fn the_window_says_loading_until_the_layout_seed_lands() {
    let mut app = headless_app();
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime
        .block_on(
            TestEnvironmentBuilder::new()
                .with_org_file("notes.org", NOTES_ORG)
                .wait_for_file_watcher(false)
                .hold_initial_scan()
                .build(runtime.clone()),
        )
        .expect("a production (no-wait) boot returns while the scan is held");
    let report = env.session().boot_report().clone();
    assert_eq!(
        report.in_background(),
        vec![BootStep::SeedDefaultLayout],
        "precondition: the held scan keeps the layout seed in the background"
    );

    let bounds = BoundsRegistry::new();
    let rebind = launch(
        &mut app,
        &env,
        &runtime,
        &bounds,
        "Holon-VaultLoadingPlaceholder-Windowed",
    );
    pump_until(
        &mut app,
        &bounds,
        &runtime,
        Duration::from_secs(20),
        LOADING,
        |b| painted_text(b).iter().any(|t| t.contains(LOADING)),
    );
    assert_eq!(
        report.in_background(),
        vec![BootStep::SeedDefaultLayout],
        "precondition: the placeholder was looked at while the seed was still pending"
    );

    env.org_fs.release_scans();
    pump_until(
        &mut app,
        &bounds,
        &runtime,
        Duration::from_secs(30),
        "the seeded layout",
        |b| paints_main_panel(b) && !painted_text(b).iter().any(|t| t.contains(LOADING)),
    );
    let failed = report.failed();
    shut_down(app, rebind);
    assert_eq!(failed, Vec::new(), "the seed succeeded once the scan ran");
}

#[test]
fn the_window_says_the_vault_failed_when_the_seed_cannot_run() {
    let mut app = headless_app();
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    holon_filesystem::crash_injection::arm("scan_vault_files");
    let env = runtime
        .block_on(
            TestEnvironmentBuilder::new()
                .with_org_file("notes.org", NOTES_ORG)
                .wait_for_file_watcher(false)
                .build(runtime.clone()),
        )
        .expect("a failed scan is disclosed, not a boot failure");
    let report = env.session().boot_report().clone();

    let bounds = BoundsRegistry::new();
    let rebind = launch(
        &mut app,
        &env,
        &runtime,
        &bounds,
        "Holon-VaultLoadFailed-Windowed",
    );
    pump_until(
        &mut app,
        &bounds,
        &runtime,
        Duration::from_secs(30),
        FAILED,
        |b| {
            let painted = painted_text(b);
            painted
                .iter()
                .any(|t| t.contains(FAILED) && t.contains(SCAN_CRASH))
                && !painted.iter().any(|t| t.contains(LOADING))
        },
    );
    let failed = report.failed();
    shut_down(app, rebind);
    assert!(
        failed
            .iter()
            .any(|(step, why)| *step == BootStep::SeedDefaultLayout && why.contains(SCAN_CRASH)),
        "the window shows the ledger's failure, so the ledger must hold it: {failed:?}"
    );
}

/// The window is already open on the placeholder when the seed fails. No
/// `block:root-layout` is ever written, so only the boot-report pump can
/// repaint the error.
#[test]
fn the_window_turns_from_loading_to_failed_when_the_seed_fails_after_launch() {
    let mut app = headless_app();
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime
        .block_on(
            TestEnvironmentBuilder::new()
                .with_org_file("notes.org", NOTES_ORG)
                .wait_for_file_watcher(false)
                .hold_initial_scan()
                .build(runtime.clone()),
        )
        .expect("a production (no-wait) boot returns while the scan is held");
    let report = env.session().boot_report().clone();

    let bounds = BoundsRegistry::new();
    let rebind = launch(
        &mut app,
        &env,
        &runtime,
        &bounds,
        "Holon-VaultLoadFailedAfterLaunch-Windowed",
    );
    pump_until(
        &mut app,
        &bounds,
        &runtime,
        Duration::from_secs(20),
        LOADING,
        |b| painted_text(b).iter().any(|t| t.contains(LOADING)),
    );

    holon_filesystem::crash_injection::arm("initial_scan_ingest");
    env.org_fs.release_scans();
    pump_until(
        &mut app,
        &bounds,
        &runtime,
        Duration::from_secs(30),
        FAILED,
        |b| {
            let painted = painted_text(b);
            painted
                .iter()
                .any(|t| t.contains(FAILED) && t.contains(CONTROLLER_DEAD))
                && !painted.iter().any(|t| t.contains(LOADING))
        },
    );
    let failed = report.failed();
    let painted_main = paints_main_panel(&bounds);
    shut_down(app, rebind);
    assert!(
        failed.iter().any(
            |(step, why)| *step == BootStep::SeedDefaultLayout && why.contains(CONTROLLER_DEAD)
        ),
        "the window shows the ledger's failure, so the ledger must hold it: {failed:?}"
    );
    assert!(
        !painted_main,
        "no layout was seeded, so no main panel paints"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
