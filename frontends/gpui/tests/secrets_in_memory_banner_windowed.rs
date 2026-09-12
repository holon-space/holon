//! A session that saves no credential says so ON SCREEN.
//!
//! `HOLON_SECRETS_BACKEND=memory` raises a sticky `SecretsHeldInMemory`
//! condition, and `holon-app/tests/in_memory_secret_backend_boot.rs` asserts it
//! reaches the bus. That is not the same claim as the user seeing it: a
//! condition raised on a bus nobody renders is the silent degradation this mode
//! exists to avoid, and the whole justification for admitting an in-memory
//! credential store is that the user is TOLD their token is not being saved.
//!
//! So this rung asserts the words, painted, in a real window over a real boot —
//! the half a headless bus assertion structurally cannot see.
//!
//! The bus is handed to the launcher explicitly. `launch_holon_window_*` takes
//! it as an `Option`, and passing `None` (which the other windowed rungs in
//! this lane do, having nothing to disclose) renders no banner at all — so a
//! test that forgot it would pass on an empty window.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! secrets_in_memory_banner_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe), and this binary sets a process-global environment variable.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::sync::Arc;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_loro::DegradedSignalBus;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The sentence the banner must carry. Taken from the fact that changes what a
/// user does — that the credential does not survive the session — rather than
/// from the mode's name, which tells them nothing.
const MUST_SAY: &str = "gone when Holon exits";

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

#[test]
fn an_in_memory_secret_session_paints_the_banner_that_admits_it() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: single-threaded test binary (`--test-threads=1`), both set before
    // the app boots and before any thread reads the environment. The backend
    // variable is what this rung is about; `TestEnvironment`'s config dir is a
    // `TempDir`, so the admission rule accepts it.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var(holon_secrets::BACKEND_ENV, "memory");
    }

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime
        .block_on(async { TestEnvironment::new(runtime.clone()) })
        .expect("test environment");
    runtime.block_on(async {
        env.start_app(true)
            .await
            .expect("the app must boot on the in-memory secret backend")
    });

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let bus: Arc<DegradedSignalBus> = (*env
        .injector()
        .expect("injector after start_app")
        .resolve::<Arc<DegradedSignalBus>>())
    .clone();

    // The condition is raised during boot DI, before any window exists. It is
    // STICKY precisely so a window opened afterwards still receives it; if that
    // stopped being true this rung would go red for the right reason.
    assert!(
        bus.subscribe().current.iter().any(|c| matches!(
            c.reason,
            holon_loro::ShareDegradedReason::SecretsHeldInMemory { .. }
        )),
        "precondition: the boot raised the condition — without it this rung would be asserting \
         that a window paints a banner nobody sent"
    );

    let bounds = BoundsRegistry::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session,
                engine,
                runtime.handle().clone(),
                NavigationState::new(),
                bounds.clone(),
                None,
                Some(bus),
                "Holon-SecretsInMemoryBanner-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let painted = painted_text(&bounds);

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);

    assert!(
        painted.iter().any(|t| t.contains(MUST_SAY)),
        "a session holding every secret in RAM must SAY so on screen — that disclosure is the \
         whole reason the mode is allowed to exist, and a user who types a token into a field \
         that discards it has been told nothing. Painted text: {painted:#?}"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
