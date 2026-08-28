//! The open-tabs strip must show the tabs that are open, from the first frame.
//!
//! Found alongside the dogfood escape
//! `2026-08-28-short-window-empties-main-outline`: on the device, tapping a
//! block made a tab chip and a breadcrumb appear out of nowhere, which read as
//! the app navigating somewhere. It had not navigated. The tab was already
//! open — it had been open since before the app restarted — and the strip
//! simply had not drawn it yet.
//!
//! `HolonApp::render` re-resolves the strip only when the focused block
//! CHANGES (`frontends/gpui/src/lib.rs`, the `last_tab_strip_focus` block), and
//! `last_tab_strip_focus` starts as `None` next to a focus that is also `None`.
//! The two are equal on the first frame, so the re-resolve never fires and the
//! strip stays empty until something unrelated moves the focus. The latch
//! conflates "nothing is focused" with "never resolved yet", which are not the
//! same state.
//!
//! The user-visible cost is not only the missing chip: the strip and the
//! breadcrumb are chrome bars ABOVE the content, so resolving them late
//! reflows the main panel mid-gesture, on the very frame the soft keyboard is
//! also taking ~290px away. That is what makes the sibling defect's row
//! disappearance look like a page switch.
//!
//! Run: `cargo nextest run -p holon-gpui --test
//! tab_strip_resolves_at_boot_windowed       --features holon-gpui/pbt`

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::PlatformTextSystem;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::graft_displayed_text_tree;
use holon_integration_tests::test_environment::TestEnvironment;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Cross-runtime fixed-point settle (the shared windowed pattern): pump until
/// the element count is stable and no `"loading"` placeholders remain.
fn settle_to_fixed_point(
    app: &mut HeadlessAppContext,
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
        let elements = bounds.all_elements();
        let count = elements.len();
        let still_loading = elements
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count == last_count && count > 0 && !still_loading {
            stable_iters += 1;
            if stable_iters >= 5 {
                return;
            }
        } else {
            stable_iters = 0;
        }
        last_count = count;
    }
    panic!(
        "window never reached a fixed point within {timeout:?}: {} elements",
        bounds.all_elements().len(),
    );
}

#[test]
fn the_tab_strip_draws_an_open_tab_without_being_prodded() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime
        .block_on(async { TestEnvironment::new(runtime.clone()) })
        .expect("test environment");
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
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug_services.clone()),
                None,
                "Holon-Tab-Strip-Boot",
                cx,
            )
        })
        .expect("window opened");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Open a tab, the way navigating to a page opens one. Nothing here touches
    // the editor, so the focused block never moves — which is the whole point:
    // the strip must not need a focus change to notice.
    runtime
        .block_on(graft_displayed_text_tree(&env))
        .expect("graft a page and navigate Main onto it");
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    // VACUITY GUARD, read from the table the strip is a view of. Without this a
    // failure below could just mean no tab was ever opened.
    let open_tabs = runtime
        .block_on(
            env.query_sql("SELECT fr.history_id FROM focus_roots fr WHERE fr.region = 'main'"),
        )
        .expect("read the open Main tabs");
    assert!(
        !open_tabs.is_empty(),
        "the fixture must leave at least one tab open in the navigation state, else the claim \
         below is vacuous",
    );

    let drawn = app
        .update(|cx| rebind.drawn_tab_count(cx))
        .expect("the window built a root view");

    eprintln!(
        "[tab-strip-boot] tabs open in navigation state={} drawn by the strip={drawn}",
        open_tabs.len(),
    );

    assert_eq!(
        drawn,
        open_tabs.len(),
        "the tab strip is drawing {drawn} of the {} tab(s) that are open. Nothing else will \
         prompt it: it re-resolves only on a focus CHANGE, and the focus has not moved. On the \
         device the strip stayed empty until the user's first tap on a block, and then appeared \
         all at once — which reflows the main panel underneath, mid-gesture, on the same frame \
         the soft keyboard is shrinking it.",
        open_tabs.len(),
    );

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
#[path = "test_init/mod.rs"]
mod test_init;
