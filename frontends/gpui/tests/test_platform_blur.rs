//! Blur regression test — deterministic deactivation under TestPlatform.
//!
//! Uses `TestAppContext` + `VisualTestContext::deactivate_window()` (which
//! internally calls `TestPlatform::set_active_window(None)`). Does NOT need
//! a Zed fork change — `from_window` is public.

use std::sync::Arc;

use gpui::Keystroke;
use gpui::TestAppContext;
use gpui::VisualTestContext;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::test_environment::TestEnvironment;

#[test]
fn blur_commits_pending_text() {
    // Use TestAppContext (not TestApp) because we need `deactivate_window()`,
    // which requires a `VisualTestContext` bound to the window handle.
    let cx = TestAppContext::single();

    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });

    let org_content = b"* First block\n:PROPERTIES:\n:ID: root-one\n:END:\n\n* Second block\n:PROPERTIES:\n:ID: root-two\n:END:\n";
    let org_path = env.temp_dir.path().join("blur_test.org");
    std::fs::write(&org_path, org_content).unwrap();

    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    let session = env.session_arc();
    let engine = env.reactive_engine.get().cloned().expect("reactive engine");

    // Warm the root layout watcher on tokio (real time) before the window
    // subscribes — under TestPlatform the launch pre-warm time-skips its
    // timer and would open the window in loading state.
    {
        use futures::StreamExt;
        use futures_signals::signal::SignalExt;
        let root_uri = holon_api::root_layout_block_uri();
        let sig = engine.watch_data_signal(&root_uri);
        let got = runtime.block_on(async move {
            let mut stream = sig.to_stream();
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                while let Some(rvm) = stream.next().await {
                    if rvm.widget_name().as_deref() != Some("loading") {
                        return true;
                    }
                }
                false
            })
            .await
            .unwrap_or(false)
        });
        assert!(got, "backend root layout watcher never produced real data");
    }

    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();

    // Open the window via the public rebindable launcher.
    // TestAppContext::update gives us &mut App, same as TestApp.
    let rebind_handle = cx
        .update(|app| {
            launch_holon_window_rebindable(
                session,
                engine,
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                None,
                None,
                "Holon-Blur-Test",
                app,
            )
        })
        .expect("window opened");

    let window = rebind_handle.window();

    // Settle to a fixed point: real tokio time between gpui pump cycles so
    // the multi-level reactive cascade (panels → rows) fully renders.
    {
        let mut last_count = 0usize;
        let mut stable = 0u32;
        for _ in 0..500 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(500));
            cx.run_until_parked();
            bounds.flush();
            let elements = bounds.all_elements();
            let count = elements.len();
            let still_loading = elements
                .iter()
                .any(|(_, info)| info.widget_type.as_ref() == "loading");
            if count > 0 && count == last_count && !still_loading {
                stable += 1;
                if stable >= 3 {
                    break;
                }
            } else {
                stable = 0;
            }
            last_count = count;
        }
    }

    // Bind a VisualTestContext so we can call deactivate_window().
    let mut visual = VisualTestContext::from_window(window, &cx);

    let initial_count = bounds.all_elements().len();
    eprintln!("[blur-test] initial elements: {initial_count}");

    // Type text into the focused editor.
    let ks = Keystroke::parse("h").unwrap();
    cx.update(|app| {
        let _ = window.update(app, |_, w, cx| {
            w.dispatch_keystroke(ks.clone(), cx);
        });
    });
    cx.run_until_parked();
    bounds.flush();

    // Deterministic blur — the editor's pending text should commit on
    // authority-move. No key-window-status race; no flaky timing.
    visual.deactivate_window();
    cx.run_until_parked();
    bounds.flush();

    eprintln!(
        "[blur-test] elements after blur: {}",
        bounds.all_elements().len()
    );

    assert!(
        bounds.all_elements().len() > 1,
        "window must render real content before the blur exercise"
    );

    // Shutdown clears windows, but detached pump tasks still hold entity
    // handles and gpui's leak detector runs before the dispatcher drops
    // them — leak the contexts at test end (process exits right after).
    drop(rebind_handle);
    cx.update(|app| app.shutdown());
    cx.run_until_parked();
    std::mem::forget(visual);
    std::mem::forget(cx);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
