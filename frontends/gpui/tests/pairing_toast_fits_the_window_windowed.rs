//! A toast the user can read: every line inside the window.
//!
//! The pairing disclosure's lines carry an absolute archive path and the query
//! that finds the conflict copies (D93.a). Both are longer than the toast box,
//! and neither can be recovered from anywhere else in the UI, so a line that
//! runs past the window edge is the disclosure not arriving at all.
//!
//! `share_ui`'s own tests judge the STRINGS `toast_lines` returns; only a real
//! window judges where those strings land.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! pairing_toast_fits_the_window_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::share_ui::TOAST_LINE;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_loro::ShareDegraded;
use holon_loro::ShareDegradedReason;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

const WINDOW: &str = "1512x900";
const WINDOW_W: f32 = 1512.0;
const WINDOW_H: f32 = 900.0;

/// The stack's own inset from the right window edge (`share_ui.rs`). A line
/// clipped at the window border reports its right edge AT the border, so a
/// bound of exactly the window width would accept the cut.
const STACK_INSET: f32 = 16.0;

/// The archive the pair writes into: an absolute path under the user's
/// Application Support, which is where the length comes from.
const ARCHIVE: &str =
    "/Users/martin/Library/Application Support/holon/loro-store/archive/20260908-011500";

#[test]
fn every_pairing_toast_line_is_inside_the_window() {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", WINDOW) };
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));

    let set = ComponentSet::full_headless();
    let bundle = runtime
        .block_on(async { compose_sut_windowed_base_seeded(&set, &resolver, &[], &[]).await });
    let session = bundle
        .session
        .clone()
        .expect("full_headless -> booted FrontendSession");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> booted ReactiveEngine");
    let frontend = bundle
        .frontend
        .clone()
        .expect("full_headless -> booted frontend component");
    let bus = frontend
        .degraded_bus()
        .expect("the booted session's degraded bus");

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
                None,
                Some(bus.clone()),
                "Holon-PairingToastFits-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // The disclosure a real pair raises (`device_pairing_op`), carried over the
    // session's own bus so the toast is built by the production bridge.
    bus.emit(ShareDegraded {
        shared_tree_id: "device".into(),
        reason: ShareDegradedReason::PairingReimportedLocalContent {
            blocks: 4,
            conflict_copies: 1,
            archive: ARCHIVE.into(),
        },
    });

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let lines: Vec<(String, ElementInfo)> = bounds
        .all_elements()
        .into_iter()
        .filter(|(id, _)| id.starts_with(TOAST_LINE))
        .collect();

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    let query = holon_loro::device_pairing_op::conflict_copies_query();
    let painted: Vec<String> = lines
        .iter()
        .filter_map(|(_, i)| i.displayed_text.as_deref().map(str::to_string))
        .collect();
    assert!(
        painted.iter().any(|l| l.contains(&query)),
        "precondition: the disclosure must have painted its query line, else the bounds below \
         judge a toast that never opened. painted: {painted:?}"
    );

    for (id, info) in &lines {
        assert!(
            info.width > 0.0
                && info.height > 0.0
                && info.x >= 0.0
                && info.y >= 0.0
                && info.x + info.width <= WINDOW_W - STACK_INSET
                && info.y + info.height <= WINDOW_H - STACK_INSET,
            "the toast line {id} must lie inside the {WINDOW} window: what falls off the right \
             edge is the archive path and the predicate of the query, and neither is reachable \
             anywhere else. It sits at x={} y={} w={} h={} and reads {:?}",
            info.x,
            info.y,
            info.width,
            info.height,
            info.displayed_text
        );
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
