//! GPUI UI PBT — geometry-based PBT test with a real GPUI window and xcap screenshots.
//!
//! Launches a real GPUI window sharing the same Turso DB as the PBT engine.
//! BoundsRegistry populates during GPUI render passes, enabling the GeometryDriver
//! to look up element positions and xcap to capture window screenshots.
//!
//! Thread model:
//! - Main thread: GPUI event loop (required on macOS — `Application` must be on main thread)
//! - Background thread: PBT state machine + driver
//!
//! Synchronization:
//! 1. PBT runs pre-startup steps → on_ready sends engine → blocks waiting for window
//! 2. Main thread receives engine → launches GPUI window → signals "window ready"
//! 3. PBT thread unblocks → runs post-startup steps with real window + xcap screenshots
//!
//! This is a `harness = false` test binary (GPUI requires main thread).
//! Run with: cargo test -p holon-gpui --test gpui_ui_pbt

use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::Duration;

use gpui::Application;
use holon_api::EntityUri;
use holon_frontend::FrontendSession;
use holon_gpui::geometry::BoundsRegistry;
use holon_integration_tests::pbt::phased::run_pbt_with_driver_sync_callback;
use holon_integration_tests::{GeometryDriver, XcapBackend};

fn main() {
    // Channels for cross-thread coordination
    let (engine_tx, engine_rx) = sync_channel(1);
    let (window_ready_tx, window_ready_rx) = sync_channel::<()>(1);

    let screenshot_dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("pbt-screenshots")
        .join("gpui");

    let bounds_registry = BoundsRegistry::new();

    // PBT runs on background thread (GPUI needs main thread on macOS)
    let pbt_registry = bounds_registry.clone();
    let pbt_handle = std::thread::spawn(move || {
        let backend = XcapBackend::new("Holon");
        let mut driver = GeometryDriver::new(Box::new(pbt_registry))
            .with_screenshots(Box::new(backend), screenshot_dir.clone());

        let result = run_pbt_with_driver_sync_callback(15, &mut driver, |engine| {
            // Send engine to main thread for GPUI window creation
            engine_tx
                .send(engine.clone())
                .expect("failed to send engine to main thread");

            // Block until GPUI window is open and has rendered at least once
            eprintln!("[gpui_ui_pbt] PBT waiting for GPUI window to be ready...");
            window_ready_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("timed out waiting for GPUI window to become ready");
            eprintln!("[gpui_ui_pbt] GPUI window ready, continuing PBT steps");
        });

        match result {
            Ok(summary) => {
                eprintln!("[gpui_ui_pbt] {summary}");
                eprintln!("[gpui_ui_pbt] screenshots at {}", screenshot_dir.display());
            }
            Err(e) => {
                eprintln!("GPUI UI PBT failed: {e:?}");
                std::process::exit(1);
            }
        }
    });

    // Main thread: wait for engine from PBT, then launch GPUI
    let engine = engine_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("timed out waiting for BackendEngine from PBT thread");

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let rt_handle = rt.handle().clone();

    let session = Arc::new(FrontendSession::from_engine(engine));
    let root_id = EntityUri::doc_root();

    let app_state = rt.block_on(async {
        let watch = session
            .watch_ui(&root_id, true)
            .await
            .expect("watch_ui failed");
        holon_frontend::cdc::spawn_ui_listener(watch)
    });

    // Keep runtime alive on a background thread
    let _rt_guard = std::thread::spawn(move || {
        rt.block_on(std::future::pending::<()>());
    });

    let app = Application::with_platform(gpui_platform::current_platform(false));

    app.run(move |cx| {
        cx.activate(true);

        holon_gpui::launch_holon_window_with_registry(
            session,
            app_state,
            rt_handle,
            bounds_registry,
            cx,
        );

        // Signal PBT thread after a short delay to let the first render complete
        cx.spawn(async move |_| {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = window_ready_tx.send(());
            eprintln!("[gpui_ui_pbt] Window ready signal sent");
            Ok::<_, anyhow::Error>(())
        })
        .detach();

        // When PBT thread finishes, quit the GPUI app
        let pbt_handle_opt = std::sync::Mutex::new(Some(pbt_handle));
        cx.spawn(async move |cx| {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let finished = {
                    let guard = pbt_handle_opt.lock().unwrap();
                    guard.as_ref().map_or(true, |h| h.is_finished())
                };
                if finished {
                    if let Some(handle) = pbt_handle_opt.lock().unwrap().take() {
                        handle.join().expect("PBT thread panicked");
                    }
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
