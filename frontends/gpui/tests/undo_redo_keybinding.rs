//! Regression test for the dogfooded "cmd+z / cmd+shift+z is completely
//! unbound" bug: before this fix, `gpui_component::input::{Undo, Redo}` (the
//! only cmd-z/cmd-shift-z bindings in the app) are scoped to the `"Input"`
//! key context, so the chord resolves to *nothing* — "No handler matched the
//! key chord" — whenever no editor has focus. A fresh window boot has no
//! focused editor, so this is the simplest real reproduction.
//!
//! `TriggerUndo`/`TriggerRedo` (bound with `context: None` in
//! `launch_holon_window_impl`) close that gap; the page-level `capture_action`
//! handlers in `HolonApp::render` route both them and the `Input`-scoped
//! actions to `FrontendSession::undo`/`redo` via `share_ui::dispatch_undo`/
//! `dispatch_redo`.
//!
//! Same TestPlatform boot recipe as `test_platform_smoke.rs`
//! (`holon_renders_under_test_platform`): a real Turso backend, a window
//! opened via the public `launch_holon_window_rebindable`, settled to a
//! fixed point. `window.dispatch_keystroke` returning `true` means some
//! handler consumed the chord — the exact thing that was `false` (falling
//! through to "no handler matched") before this fix.
//!
//! What this test does NOT cover (documented rather than faked): it asserts
//! the chord *resolves*, not that `FrontendSession::undo()` observably
//! reverted a prior edit — that would additionally require an operation to
//! undo and a settle loop over the projection, which is exactly the keystone
//! composed PBT's job (`general_e2e_composed_pbt.rs`) once an undo-driving
//! transition exists there. It also does not exercise the `UndoFailed` toast
//! path (see `share_ui.rs`'s `dispatch_undo_redo`) — that requires a
//! `FrontendSession` with no operation engine wired, which the
//! `TestEnvironment` builder does not currently offer as a fixture;
//! `share_ui.rs`'s unit test `push_toast` exercises the toast-plumbing side
//! directly instead.

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
    gpui_platform::current_platform(true).text_system()
}

/// Same cross-runtime fixed-point settle as `test_platform_smoke.rs`.
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
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        app.run_until_parked();
        app.advance_clock(Duration::from_secs(1));
        app.run_until_parked();
        bounds.flush();
        let count = bounds.all_elements().len();
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
    runtime.block_on(async { tokio::task::yield_now().await });
    app.run_until_parked();
    bounds.flush();
}

#[test]
fn undo_redo_chords_resolve_without_editor_focus() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

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

    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();

    let rebind_handle = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug_services.clone()),
                "Holon-TestPlatform-UndoRedo",
                cx,
            )
        })
        .expect("window opened");
    let any_handle = rebind_handle.window();

    settle(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // A fresh boot has no focused editor — exactly the state that used to
    // make cmd-z fall all the way through to "No handler matched the key
    // chord". Both chords must resolve to a handler regardless.
    #[cfg(target_os = "macos")]
    let (undo_chord, redo_chord) = ("cmd-z", "cmd-shift-z");
    #[cfg(not(target_os = "macos"))]
    let (undo_chord, redo_chord) = ("ctrl-z", "ctrl-y");

    let undo_ks = Keystroke::parse(undo_chord).unwrap();
    let handled_undo = app.update(|cx| {
        any_handle
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(undo_ks.clone(), cx)
            })
            .unwrap()
    });
    assert!(
        handled_undo,
        "{undo_chord} was not handled with no editor focused — regression of the dogfooded 'No \
         handler matched the key chord' bug"
    );

    let redo_ks = Keystroke::parse(redo_chord).unwrap();
    let handled_redo = app.update(|cx| {
        any_handle
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(redo_ks.clone(), cx)
            })
            .unwrap()
    });
    assert!(
        handled_redo,
        "{redo_chord} was not handled with no editor focused"
    );

    drop(rebind_handle);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}
