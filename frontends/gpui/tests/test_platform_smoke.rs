//! Spike: prove the Holon widget tree composes under gpui TestPlatform.
//!
//! Opens a Holon window inside `TestApp` with a real Turso backend,
//! verifies the BoundsRegistry is populated after the initial render,
//! and types keystrokes through the public API (`handle.update` +
//! `window.dispatch_keystroke`).

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::Keystroke;
use gpui::PlatformTextSystem;
use gpui::TestApp;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::test_environment::TestEnvironment;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    let platform = gpui_platform::current_platform(true);
    platform.text_system()
}

/// Cross-runtime fixed-point settle. Runs tokio between gpui pump cycles
/// so backend-to-frontend signals can deliver through the channel bridge.
fn settle(
    app: &mut TestApp,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    timeout: Duration,
) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable_iters = 0u32;
    while start.elapsed() < timeout {
        // Let the tokio runtime advance — delivers pending backend signals
        // into the channels the gpui pump reads. Real sleep (not yield) so
        // backend watchers get actual wall-clock time to run queries.
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        app.run_until_parked();
        // Fact 6: timer-based code (prewarm) needs clock advancement.
        app.advance_clock(Duration::from_secs(1));
        app.run_until_parked();
        bounds.flush();
        let count = bounds.all_elements().len();
        // Fixed point: element count unchanged for several iterations AND
        // no remaining "loading" placeholders.
        let still_loading = bounds
            .all_elements()
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count == last_count && count > 0 && !still_loading {
            stable_iters += 1;
            if stable_iters >= 5 {
                break;
            }
        } else {
            stable_iters = 0;
        }
        last_count = count;
    }
    // One more cycle for the cascade to settle.
    runtime.block_on(async { tokio::task::yield_now().await });
    app.run_until_parked();
    bounds.flush();
}

#[test]
fn holon_renders_under_test_platform() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    // Create a real Turso backend (temp dir + in-memory org FS).
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

    // Probe: poll the root layout signal directly on tokio, bypassing gpui.
    // Distinguishes "backend watcher never produces data" from "gpui-side
    // delivery broken".
    {
        use futures::StreamExt;
        use futures_signals::signal::SignalExt;
        let root_uri = holon_api::root_layout_block_uri();
        let sig = engine.watch_data_signal(&root_uri);
        let got = runtime.block_on(async move {
            let mut stream = sig.to_stream();
            tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(rvm) = stream.next().await {
                    eprintln!("[probe] root signal fired: widget={:?}", rvm.widget_name());
                    if rvm.widget_name().as_deref() != Some("loading") {
                        return true;
                    }
                }
                false
            })
            .await
            .unwrap_or(false)
        });
        eprintln!("[probe] root signal produced real data on tokio: {got}");
        assert!(got, "backend root layout watcher never produced real data");
    }

    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();

    // Open the window via the public rebindable launcher.
    let rebind_handle = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug_services.clone()),
                None,
                "Holon-TestPlatform-Spike",
                cx,
            )
        })
        .expect("window opened");
    let any_handle = rebind_handle.window();

    // Cross-runtime settle: the prewarm timeout and root-layout signal
    // pump need tokio advancement between gpui pump cycles.
    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let elements = bounds.all_elements();
    eprintln!(
        "[spike] BoundsRegistry has {} elements after settle",
        elements.len()
    );
    for (id, info) in &elements {
        eprintln!(
            "[spike]   {id}: widget={} entity={:?} pos=({:.0},{:.0} {:.0}x{:.0}) focused={:?}",
            info.widget_type, info.entity_id, info.x, info.y, info.width, info.height, info.focused,
        );
    }

    assert!(
        !elements.is_empty(),
        "BoundsRegistry must have entries after initial render"
    );

    // Try dispatching a keystroke through the public-API path.
    let ks = Keystroke::parse("h").unwrap();
    app.update(|cx| {
        any_handle
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(ks.clone(), cx);
            })
            .unwrap();
    });
    app.run_until_parked();
    bounds.flush();

    let after = bounds.all_elements().len();
    eprintln!(
        "[spike] elements after keystroke: {after} (was {} before)",
        elements.len()
    );

    // Shutdown clears windows, but detached pump tasks still hold entity
    // handles and gpui's leak detector runs before the dispatcher drops
    // them — leak the TestApp at test end (process exits right after).
    drop(rebind_handle);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}
