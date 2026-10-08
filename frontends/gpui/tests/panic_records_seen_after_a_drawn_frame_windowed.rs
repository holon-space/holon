//! A previous run's panic record counts as seen only after a frame that drew
//! it, so a run that dies before that shows it again at the next start.
//!
//! @pbt kind windowed
//! @pbt covers panic-record-seen-after-draw — the window acknowledges the
//! records `install` showed only on the frame after the one that painted their
//! toast (BugFunnel 2026-10-08-arm-consumes-the-previous-panic-record-unshown)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone is headless and
//! draws no frame

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui::AnyWindowHandle;
use gpui::AssetSource;
use gpui::TestApp;
use holon_frontend::panic_record::PanicRecord;
use holon_frontend::panic_record::SEEN_RECORD_FILE;
use holon_frontend::panic_record::UNSHOWN_DIR;
use holon_gpui::launch_holon_window_with_engine_and_share;
use holon_integration_tests::test_environment::TestEnvironment;

fn unshown(dir: &Path) -> bool {
    dir.join(UNSHOWN_DIR).join("1.json").exists()
}

fn the_window(app: &mut TestApp) -> AnyWindowHandle {
    let windows = app.update(|cx| cx.windows());
    assert_eq!(windows.len(), 1, "the launch opens one window");
    windows[0]
}

#[test]
fn a_previous_run_is_seen_on_the_frame_after_the_one_that_drew_it() {
    let mut app = TestApp::with_text_system_and_assets(
        gpui_platform::current_text_system(),
        Arc::new(()) as Arc<dyn AssetSource>,
    );
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    env.set_enable_loro(false);
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });
    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

    let config = tempfile::tempdir().expect("temp config dir");
    PanicRecord {
        message: "the previous run".to_string(),
        location: "prior.rs:1:1".to_string(),
        thread: "main".to_string(),
    }
    .write_to(config.path())
    .expect("seed the previous run's record");
    let bus = holon_frontend::panic_record::install(config.path());
    assert!(
        unshown(config.path()),
        "install shows the record on its bus"
    );

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
    let window = the_window(&mut app);
    assert!(
        unshown(config.path()),
        "the window holds the toast but has drawn no frame with it"
    );

    app.update(|cx| {
        window
            .update(cx, |_, window, cx| window.draw(cx).clear(cx))
            .expect("draw the window");
    });
    assert!(
        unshown(config.path()),
        "the frame that draws the toast is not yet presented"
    );

    let ran = app.update(|cx| {
        window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .expect("deliver the next frame")
    });
    assert!(ran > 0, "the drawn frame scheduled its acknowledgement");
    assert!(
        !unshown(config.path()) && config.path().join(SEEN_RECORD_FILE).exists(),
        "the next frame marks the record seen"
    );

    std::mem::forget(app);
    std::mem::forget(env);
}

mod test_init;
