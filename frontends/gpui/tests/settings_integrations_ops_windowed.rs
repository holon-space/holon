//! The Settings modal's integration rows paint their operations, in a REAL
//! window over a REAL booted engine.
//!
//! The integration row's `Configure…` affordance is layout data now
//! (`assets/default/types/integration_profile.yaml`: an `ops_of` collection
//! feeding `op_button`), and `ops_of` hands it over as a STREAMING collection.
//! Nothing in a VM snapshot or a `ReactiveFixtureView` mounts one — only the
//! production shell does. So this is the only tier that can answer "does the
//! user see the button at all", and until this rung existed nothing did.
//!
//! It also proves the toolbar gear is reachable: the modal has exactly one way
//! in, and a window test that cannot open it cannot see anything the modal
//! paints.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_integrations_ops_windowed -- --test-threads=1`
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

fn painted_op_buttons(bounds: &BoundsRegistry) -> Vec<String> {
    bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| {
            info.vm_node
                .as_ref()
                .is_some_and(|n| n.tag.as_ref() == "op_button")
        })
        .map(|(id, _)| id)
        .collect()
}

/// Every widget tag the window painted, with counts — the evidence a reader
/// needs to tell "the modal never opened" from "it opened and the operations
/// are missing".
fn painted_widget_census(bounds: &BoundsRegistry) -> String {
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for (_, info) in bounds.all_elements() {
        if let Some(node) = &info.vm_node {
            *tags.entry(node.tag.to_string()).or_default() += 1;
        }
    }
    format!("{tags:?}")
}

#[test]
fn the_settings_modal_paints_the_integration_rows_operations() {
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
                "Holon-SettingsIntegrations-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    assert!(
        painted_op_buttons(&bounds).is_empty(),
        "precondition: the Settings modal is closed, so no integration operations are painted"
    );

    let gear = bounds.element_info(SETTINGS_GEAR).unwrap_or_else(|| {
        panic!(
            "the toolbar gear is not registered as {SETTINGS_GEAR:?}, so no window test can open \
             Settings — the modal has no command and no keybinding either"
        )
    });
    let (gx, gy) = gear.center();
    let center = Point {
        x: Pixels::from(gx),
        y: Pixels::from(gy),
    };

    let window = rebind.window();
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
            .expect("window alive for the gear click");
    });

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let painted = painted_op_buttons(&bounds);
    assert!(
        !painted.is_empty(),
        "the open Settings modal must paint the integration rows' operations. The modal IS open \
         and the rows ARE rendered — what the window painted: {}",
        painted_widget_census(&bounds)
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
