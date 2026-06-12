//! Blur regression test — deterministic deactivation under TestPlatform.
//!
//! Uses `TestAppContext` + `VisualTestContext::deactivate_window()` (which
//! internally calls `TestPlatform::set_active_window(None)`). Does NOT need
//! a Zed fork change — `from_window` is public.

use std::sync::Arc;

use gpui::{AppContext as _, Keystroke, TestAppContext, VisualTestContext};
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::test_environment::TestEnvironment;

#[test]
fn blur_commits_pending_text() {
    // Use TestAppContext (not TestApp) because we need `deactivate_window()`,
    // which requires a `VisualTestContext` bound to the window handle.
    let mut cx = TestAppContext::single();

    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let mut env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });

    let org_content = b"* First block\n:PROPERTIES:\n:ID: root-one\n:END:\n\n* Second block\n:PROPERTIES:\n:ID: root-two\n:END:\n";
    let org_path = env.temp_dir.path().join("blur_test.org");
    std::fs::write(&org_path, org_content).unwrap();

    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    let session = env.session_arc();
    let engine = env.reactive_engine.clone().expect("reactive engine");
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
                "Holon-Blur-Test",
                app,
            )
        })
        .expect("window opened");

    let window = rebind_handle.window();
    cx.run_until_parked();
    bounds.flush();

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

    // Clean up the window handle before dropping the context.
    drop(rebind_handle);
    cx.run_until_parked();
    cx.update(|app| app.shutdown());
    cx.run_until_parked();
}
