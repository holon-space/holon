//! Blinc UI PBT — geometry-based PBT test for the Blinc frontend.
//!
//! Launches the Blinc app in-process, extracts the ElementRegistry from the
//! render context, and uses BlincGeometry + GeometryDriver for element lookups.
//!
//! Requires a display — not headless. Stays `#[ignore]` for CI.
//!
//! Run with: cargo test -p holon-blinc --test blinc_ui_pbt -- --ignored --nocapture

use holon_blinc::geometry::BlincGeometry;
use holon_integration_tests::pbt::phased::run_pbt_with_driver_sync;
use holon_integration_tests::{GeometryDriver, XcapBackend};

#[test]
#[ignore = "requires display + Blinc app in-process — run manually"]
fn blinc_geometry_pbt() {
    let (registry_tx, registry_rx) = std::sync::mpsc::sync_channel(1);
    let mut sent = false;

    // Spawn the Blinc app on a dedicated thread — WindowedApp::run() blocks.
    std::thread::spawn(move || {
        blinc_theme::ThemeState::init_default();
        blinc_app::windowed::WindowedApp::run(blinc_app::WindowConfig::default(), move |ctx| {
            if !sent {
                let _ = registry_tx.send(ctx.element_registry().clone());
                sent = true;
            }
            blinc_app::prelude::div()
                .w(ctx.width)
                .h(ctx.height)
                .child(blinc_app::prelude::text("Blinc PBT test"))
        })
        .expect("WindowedApp::run failed");
    });

    // Wait for the registry from the first frame
    let registry = registry_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("Timed out waiting for Blinc ElementRegistry");

    let screenshot_dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("pbt-screenshots")
        .join("blinc");

    let backend = XcapBackend::new("Blinc");
    let geometry = BlincGeometry::new(registry);
    let mut driver = GeometryDriver::new(Box::new(geometry))
        .with_screenshots(Box::new(backend), screenshot_dir.clone());

    match run_pbt_with_driver_sync(15, &mut driver) {
        Ok(summary) => {
            eprintln!("[blinc_ui_pbt] {summary}");
            eprintln!("[blinc_ui_pbt] screenshots at {}", screenshot_dir.display());
        }
        Err(e) => panic!("Blinc UI PBT failed: {e:?}"),
    }
}
