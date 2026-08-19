//! Somebody must actually be LISTENING to the degraded bus.
//!
//! Registering the bus (see `degraded_signal_bus_container.rs`) only creates a
//! channel. The shipped desktop build additionally has to subscribe to it, or
//! a dead MCP integration still renders a blank page with no banner — the bus
//! is merely written to and nobody reads.
//!
//! The subscriber is `share_ui::spawn_degraded_bus_bridge`, spawned by
//! `launch_holon_window_impl`. Its spawn used to be nested inside
//! `if let Some(backend) = share_backend`, and `share_backend` is
//! `Arc<LoroShareBackend>` — unregistered whenever `crdt.enabled` is unset,
//! i.e. in the SHIPPED DEFAULT. This test opens a real window over a SqlOnly
//! container through the production launcher and asserts the bus has a
//! subscriber afterwards.
//!
//! @pbt kind windowed
//! @pbt covers degraded-disclosure-subscriber — after the production GPUI
//! window launch over a SqlOnly (shipped-default) container, the
//! `DegradedSignalBus` has a live subscriber, so a raised condition reaches
//! `ShareUiState` and can be rendered as a banner (BugFunnel 2026-08-04
//! ENVIRONMENT)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone is headless and
//! never runs a window launch, so it cannot see a missing subscriber

use std::sync::Arc;
use std::time::Duration;

use gpui::AssetSource;
use gpui::PlatformTextSystem;
use gpui::TestApp;
use holon_gpui::launch_holon_window_with_engine_and_share;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_loro::DegradedSignalBus;
use holon_loro::ShareDegraded;
use holon_loro::ShareDegradedReason;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    let platform = gpui_platform::current_platform(true);
    platform.text_system()
}

#[test]
fn shipped_window_launch_subscribes_to_the_degraded_bus_in_sql_only_mode() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    // The shipped default: `crdt.enabled` unset ⇒ no LoroModule ⇒ no
    // `Arc<LoroShareBackend>`. This is the configuration the dogfood boot ran
    // in and the one the old `share_backend` guard silenced.
    env.set_enable_loro(false);
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });
    assert!(
        !env.loro_enabled(),
        "this test is only meaningful in the SqlOnly (shipped-default) mode"
    );

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");
    let bus: Arc<DegradedSignalBus> = (*env
        .injector()
        .expect("injector after start_app")
        .resolve::<Arc<DegradedSignalBus>>())
    .clone();

    assert_eq!(
        bus.subscriber_count(),
        0,
        "nothing should be subscribed before the window opens — a non-zero baseline would make \
         the post-launch assertion vacuous"
    );

    // The production launcher. `share_backend` is `None` (SqlOnly), exactly as
    // in `main.rs` for this configuration.
    app.update(|cx| {
        launch_holon_window_with_engine_and_share(
            session,
            engine,
            debug_services,
            None,
            bus.clone(),
            runtime.handle().clone(),
            cx,
        )
    });
    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
    app.run_until_parked();

    assert!(
        bus.subscriber_count() > 0,
        "the production window launch must subscribe to the DegradedSignalBus in SqlOnly mode — \
         with no subscriber every raised condition (dead MCP integration, failed org ingest) is \
         written to a channel nobody reads, and the page renders blank with no banner"
    );

    // The subscriber must survive delivery, not unsubscribe on the first event.
    bus.emit(ShareDegraded {
        shared_tree_id: "integration:probe".to_string(),
        reason: ShareDegradedReason::IntegrationConnectFailed {
            integration: "probe".to_string(),
            error: "sidecar not found".to_string(),
        },
    });
    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
    app.run_until_parked();
    assert!(
        bus.subscriber_count() > 0,
        "the bridge must stay subscribed after delivering a condition"
    );

    // The detached bridge pump still holds an `Entity<ShareUiState>` when
    // gpui's leak detector runs on drop, so leak the TestApp; the process
    // exits right after. No `cx.shutdown()` first — this window was opened by
    // the non-rebindable launcher, so there is no handle to release and the
    // shutdown never parks.
    std::mem::forget(app);
    std::mem::forget(env);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
