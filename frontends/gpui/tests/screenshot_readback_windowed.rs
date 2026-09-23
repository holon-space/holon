//! The in-app screenshot path: an `InteractionEvent::CaptureScreenshot` sent
//! through the window's interaction pump must come back as the painted frame.
//!
//! This is the path the MCP `screenshot` tool takes on Android, where there is
//! no OS-level window capture. It relies on `Window::render_to_image` being
//! available outside gpui's `test-support` feature, which only the holon-space
//! zed fork provides; a fork rebase that loses it breaks this binary.
//!
//! Run: cargo nextest run -p holon-gpui --features holon-gpui/pbt --test
//! screenshot_readback_windowed

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::graft_undo_blur_pair;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_mcp::server::CapturedImage;
use holon_mcp::server::InteractionCommand;
use holon_mcp::server::InteractionEvent;

fn real_text_system() -> Arc<dyn gpui::PlatformTextSystem> {
    gpui_platform::current_text_system()
}

fn settle(app: &mut HeadlessAppContext, bounds: &BoundsRegistry, timeout: Duration) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable_iters = 0u32;
    while start.elapsed() < timeout {
        futures::executor::block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
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
    futures::executor::block_on(async { tokio::task::yield_now().await });
    app.run_until_parked();
    bounds.flush();
}

fn distinct_colors_in(img: &CapturedImage, x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
    let mut colors = HashSet::new();
    for y in y0..y1.min(img.height) {
        for x in x0..x1.min(img.width) {
            let i = ((y * img.width + x) * 4) as usize;
            colors.insert(u32::from_le_bytes([
                img.rgba[i],
                img.rgba[i + 1],
                img.rgba[i + 2],
                img.rgba[i + 3],
            ]));
        }
    }
    colors.len()
}

#[test]
fn capture_screenshot_through_the_pump_returns_the_painted_frame() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let _reactor = runtime.enter();
    let env = futures::executor::block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    futures::executor::block_on(async { env.start_app(true).await.expect("start_app") });
    let (_first_id, second_id) =
        futures::executor::block_on(graft_undo_blur_pair(&env, "readback"))
            .expect("graft the sibling row pair");

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

    let bounds = BoundsRegistry::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                NavigationState::new(),
                bounds.clone(),
                Some(debug_services.clone()),
                None,
                "screenshot-readback-windowed",
                cx,
            )
        })
        .expect("window opened");

    let row_element = format!("block:{second_id}");
    let boot_deadline = Instant::now() + Duration::from_secs(180);
    let row = loop {
        settle(&mut app, &bounds, Duration::from_secs(30));
        let row = bounds.all_elements().into_iter().find(|(_, info)| {
            info.entity_id.as_deref() == Some(row_element.as_str())
                && info.widget_type.as_ref() == "rendered_text"
        });
        if let Some((_, info)) = row {
            break info;
        }
        assert!(
            Instant::now() < boot_deadline,
            "boot precondition: the seeded row {row_element} never painted"
        );
    };

    let (viewport, scale) = app
        .update_window(rebind.window(), |_, window, _| {
            (window.viewport_size(), window.scale_factor())
        })
        .expect("read the window's viewport");

    let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
    debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone()
        .try_send(InteractionCommand {
            event: InteractionEvent::CaptureScreenshot,
            response_tx,
        })
        .expect("the interaction pump accepts a command");
    let response_deadline = Instant::now() + Duration::from_secs(30);
    let response = loop {
        app.run_until_parked();
        match response_rx.try_recv() {
            Ok(response) => break response,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                assert!(
                    Instant::now() < response_deadline,
                    "the pump never answered CaptureScreenshot"
                );
                futures::executor::block_on(async {
                    tokio::time::sleep(Duration::from_millis(20)).await
                });
            }
            Err(e) => panic!("the pump dropped the CaptureScreenshot reply: {e}"),
        }
    };

    assert!(
        response.handled,
        "CaptureScreenshot must be handled, detail: {:?}",
        response.detail
    );
    let img = response
        .screenshot
        .expect("a handled CaptureScreenshot carries an image");
    let expected_w = (f32::from(viewport.width) * scale).round() as u32;
    let expected_h = (f32::from(viewport.height) * scale).round() as u32;
    assert_eq!(
        (img.width, img.height),
        (expected_w, expected_h),
        "the image must cover the whole window in device pixels"
    );
    assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);

    let x0 = (row.x * scale) as u32;
    let y0 = (row.y * scale) as u32;
    let x1 = ((row.x + row.width) * scale) as u32;
    let y1 = ((row.y + row.height) * scale) as u32;
    let row_colors = distinct_colors_in(&img, x0, y0, x1, y1);
    assert!(
        row_colors >= 8,
        "the seeded row's rect ({x0},{y0})-({x1},{y1}) must show anti-aliased glyphs, \
         found {row_colors} distinct colours"
    );

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
