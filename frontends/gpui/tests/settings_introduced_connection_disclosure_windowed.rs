//! An introduced connection's row PAINTS where it came from and who it calls.
//!
//! This is the landing condition the `user-connections` disclosure was declared
//! against, and the one the dogfood pass found unmet. `IntegrationRow` carried
//! `origin` and `hosts`, the row-data test asserting them was green, and the
//! Settings modal showed an introduced connection with exactly the five columns
//! a bundled one has. The data was right and nothing read it — which is why the
//! covering test has to be a WINDOWED one: a row-data test cannot go red for a
//! missing render.
//!
//! The disclosure matters because the other two things on the row — the name
//! and the icon — are the file's own choice. A hostile connection can call
//! itself "Calendar", wear a calendar glyph, and ask for the same click as a
//! bundled one. Its path on disk and the hosts its manual calls are what it
//! cannot choose away from.
//!
//! The values are written into `integration_state` directly rather than by
//! installing a connection file: the mirror IS what the Settings `live_query`
//! reads, the projector's half is pinned headlessly
//! (`holon-app/tests/introduced_connection_disclosure_reaches_the_mirror.rs`),
//! and writing them here keeps this rung about the RENDER, which is the half
//! that was missing.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_introduced_connection_disclosure_windowed -- --test-threads=1`
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
use gpui::InputEvent;
use gpui::MouseButton;
use gpui::Pixels;
use gpui::Point;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The toolbar affordance that opens Settings. The modal has no command and no
/// keybinding — this is the only door.
const SETTINGS_GEAR: &str = "settings-gear";

/// The connection whose row is given a disclosure. Any bundled provider serves:
/// the row is the render target, and the mirror is what says it was introduced.
const SUBJECT: &str = "todoist";

/// Chosen to be unmistakable in painted text and impossible to produce by
/// accident from anything else the modal renders.
const ORIGIN: &str = "/tmp/holon-fixture/introduced-calendar.yaml";
const HOSTS: &str = "api.evil.example, cdn.evil.example";

fn click_at(
    app: &mut HeadlessAppContext,
    window: gpui::AnyWindowHandle,
    center: Point<Pixels>,
    what: &str,
) {
    app.update(|cx| {
        window
            .update(cx, |_, win, cx| {
                win.dispatch_event(
                    gpui::MouseMoveEvent {
                        position: center,
                        pressed_button: None,
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::MouseDownEvent {
                        position: center,
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::MouseUpEvent {
                        position: center,
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap_or_else(|e| panic!("window alive for the {what} click: {e}"));
    });
}

fn center_of(info: &ElementInfo) -> Point<Pixels> {
    let (x, y) = info.center();
    Point {
        x: Pixels::from(x),
        y: Pixels::from(y),
    }
}

/// Every non-empty string the window painted. The red log needs the whole set:
/// "the modal never opened" and "it opened and says nothing about the origin"
/// are different failures with the same assertion.
fn painted_text(bounds: &BoundsRegistry) -> Vec<String> {
    let mut out: Vec<String> = bounds
        .all_elements()
        .into_iter()
        .filter_map(|(_, info)| info.displayed_text)
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn an_introduced_connections_row_paints_its_origin_and_its_hosts() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    // Point HOME at an empty dir before anything reads it: the bundled gcal
    // sidecar names `~/.config/holon/gcal-client-*`, and on a machine that has
    // those files a consent flow could reach a real browser.
    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: single-threaded test binary (`--test-threads=1`), set before the
    // app boots and before any thread reads the environment.
    unsafe { std::env::set_var("HOME", home.path()) };

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
    let db = bundle
        .engine
        .as_ref()
        .expect("full_headless -> a Turso engine")
        .db_handle()
        .clone();

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
                None,
                "Holon-IntroducedConnectionDisclosure-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let gear = bounds.element_info(SETTINGS_GEAR).unwrap_or_else(|| {
        panic!(
            "the toolbar gear is not registered as {SETTINGS_GEAR:?}, so no window test can open \
             Settings — the modal has no command and no keybinding either"
        )
    });
    let window = rebind.window();
    click_at(&mut app, window, center_of(&gear), "gear");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Make one row an INTRODUCED one, after the modal is already open: the
    // update has to reach the screen through the section's own `live_query`,
    // which is the path a real scan would use.
    runtime.block_on(async {
        db.execute_values(
            "UPDATE integration_state SET origin = ?, hosts = ? WHERE provider_name = ?",
            vec![
                holon_api::Value::String(ORIGIN.to_string()),
                holon_api::Value::String(HOSTS.to_string()),
                holon_api::Value::String(SUBJECT.to_string()),
            ],
        )
        .await
        .expect("the mirror must carry the disclosure columns");
    });
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let painted = painted_text(&bounds);

    // Teardown BEFORE the assertions so a red does not also trip the gpui leak
    // detector, which would bury the real failure.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    assert!(
        painted.iter().any(|t| t.contains(ORIGIN)),
        "the open Settings modal must paint the FILE an introduced connection came from — the \
         name and the icon beside it are the file's own choice and disclose nothing. Painted \
         text: {painted:#?}"
    );
    assert!(
        painted.iter().any(|t| t.contains("api.evil.example")),
        "the row must also paint the hosts the connection calls: switching it on is consent to \
         reach them with the credential named for it, and that is not readable from a display \
         name. Painted text: {painted:#?}"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
