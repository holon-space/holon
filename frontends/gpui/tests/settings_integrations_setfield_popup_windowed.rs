//! Clicking a MULTI-param op_button (`integration.set_field`) must open a
//! param-collection popup and, once the user picks the missing params, dispatch
//! the operation — in a REAL window over a REAL booted engine.
//!
//! `set_field` needs three params: `id` (resolved from the row), `field`
//! (`OneOf ["enabled"]`), and `value` (`Bool`). The op_button hands
//! `present_op` only `{ id }`, so two params are missing. Today `present_op`
//! has no way to collect them and takes its fail-loud `panic!` branch
//! (`crates/holon-frontend/src/reactive.rs`). That panic IS the red-for-the-
//! right-reason: the feature (a param-collection popup anchored at the button)
//! is missing, and the deliberate fail-loud says so. Once the popup exists the
//! click opens it, picking `field=enabled` then `value` dispatches, and the
//! enablement mirror flips.
//!
//! The single-param sibling (`begin_oauth`, "Configure…") is covered by
//! `settings_integrations_ops_windowed.rs`; this rung is the multi-param case
//! that was never driven.
//!
//! ENVIRONMENT CAVEAT: opening a second row's popup is a STRUCTURAL REBUILD
//! here, so the ephemeral-cache wipe closes the first popup on its own — the
//! two-row rung passes even with `on_mouse_down_out` disabled. The live app
//! stacks popups (pre-fix dogfood), i.e. that rebuild/wipe does NOT fire there,
//! so `on_mouse_down_out` is the load-bearing dismissal in production. The
//! `outside_click_on_inert_space_...` rung pins it directly: it clicks inert
//! space (no op_button → no rebuild → no wipe), so only the handler can close
//! the popup. The dogfood re-run must watch this windowed↔live divergence.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_integrations_setfield_popup_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::InputEvent;
use gpui::MouseButton;
use gpui::Pixels;
use gpui::Point;
use holon::storage::DbHandle;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The toolbar affordance that opens Settings — the modal's only door.
const SETTINGS_GEAR: &str = "settings-gear";

/// `gcal`'s generic `set_field` op_button. Every integration row renders one
/// per admitted operation (`ops_of` collection → `op_button`), and `set_field`
/// carries no guard, so it is always listed.
const SET_FIELD_GCAL: &str = "op-button-set_field-integration:gcal";

/// The param-collection overlay the click must open, and its per-choice items.
/// These ids are the contract between this test and the op_button overlay
/// (`frontends/gpui/src/render/builders/op_button.rs`).
const PARAM_POPUP_TAG: &str = "op_param_popup";
const FIELD_ITEM_ENABLED: &str = "op-param-item-field-enabled";
const VALUE_ITEM_ON: &str = "op-param-item-value-true";
const VALUE_ITEM_OFF: &str = "op-param-item-value-false";

const GCAL: &str = "gcal";

/// Dispatch a real left click at `center`.
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

fn center_of(info: &holon_frontend::geometry::ElementInfo) -> Point<Pixels> {
    let (x, y) = info.center();
    Point {
        x: Pixels::from(x),
        y: Pixels::from(y),
    }
}

fn painted_count(bounds: &BoundsRegistry, tag: &str) -> usize {
    bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.vm_node.as_ref().is_some_and(|n| n.tag.as_ref() == tag))
        .count()
}

/// Every widget tag the window painted, with counts — the evidence for telling
/// "the popup never opened" from "it opened without the choices".
fn painted_widget_census(bounds: &BoundsRegistry) -> String {
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for (_, info) in bounds.all_elements() {
        if let Some(node) = &info.vm_node {
            *tags.entry(node.tag.to_string()).or_default() += 1;
        }
    }
    format!("{tags:?}")
}

/// `enabled` for `provider`, read from the queryable mirror the projector owns.
fn mirror_enabled(runtime: &tokio::runtime::Runtime, db: &DbHandle, provider: &str) -> Option<i64> {
    runtime.block_on(async {
        db.query(
            "SELECT provider_name, enabled FROM integration_state",
            Default::default(),
        )
        .await
        .expect("read integration_state")
        .iter()
        .find(|r| r.get("provider_name").and_then(|v| v.as_string()) == Some(provider))
        .and_then(|r| r.get("enabled"))
        .and_then(|v| v.as_i64())
    })
}

/// Extract a panic payload's message (the fail-loud text `present_op` emits).
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "<non-string panic payload>".to_string())
}

#[test]
fn clicking_multi_param_set_field_opens_param_popup_then_dispatches() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    // Empty HOME BEFORE anything reads it: the bundled gcal sidecar names
    // `~/.config/holon/gcal-client-*`; on a machine that has them a consent flow
    // could reach a real browser. `set_field` never opens a browser, but the
    // section boot resolves the sidecar either way.
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
                "Holon-SetFieldPopup-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");
    let window = rebind.window();

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Open Settings.
    let gear = bounds
        .element_info(SETTINGS_GEAR)
        .expect("the toolbar gear must be painted so Settings can be opened");
    click_at(&mut app, window, center_of(&gear), "gear");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // The multi-param button must be on screen; the whole test is about clicking
    // it, so its absence is a harness failure, not a feature red.
    let set_field_btn = bounds.element_info(SET_FIELD_GCAL).unwrap_or_else(|| {
        panic!(
            "the integration row must paint its generic {SET_FIELD_GCAL:?} op_button; \
             painted: {}",
            painted_widget_census(&bounds)
        )
    });

    let initial_enabled = mirror_enabled(&runtime, &db, GCAL)
        .expect("gcal has a row in the integration_state mirror");
    let target_enabled = if initial_enabled == 0 { 1 } else { 0 };

    // The click. Today it PANICS inside `present_op` (feature missing); catch it
    // so the red surfaces as a clean assertion failure carrying the fail-loud
    // message, never a hang. Once the popup exists this settles with it painted.
    let click_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        click_at(&mut app, window, center_of(&set_field_btn), "set_field");
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(10));
    }));

    let popup_painted = painted_count(&bounds, PARAM_POPUP_TAG) > 0;
    let field_item_painted = bounds.element_info(FIELD_ITEM_ENABLED).is_some();

    // Complete the popup only on the green path — pick the field, then the value
    // opposite to the current one, then read the mirror back.
    let mut final_enabled = initial_enabled;
    if click_result.is_ok() && popup_painted {
        let field_item = bounds
            .element_info(FIELD_ITEM_ENABLED)
            .expect("the popup's first step must offer the OneOf field choice 'enabled'");
        click_at(&mut app, window, center_of(&field_item), "field=enabled");
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(10));

        let value_id = if target_enabled == 1 {
            VALUE_ITEM_ON
        } else {
            VALUE_ITEM_OFF
        };
        let value_item = bounds.element_info(value_id).unwrap_or_else(|| {
            let op_param_ids: Vec<String> = bounds
                .all_elements()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| id.contains("op-param"))
                .collect();
            panic!(
                "the popup's value step must offer {value_id:?}. op-param ids painted now: \
                 {op_param_ids:?}. census: {}",
                painted_widget_census(&bounds)
            )
        });
        click_at(&mut app, window, center_of(&value_item), "value");

        // Give the dispatch → store write → projector mirror chain time to land.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(2));
            final_enabled = mirror_enabled(&runtime, &db, GCAL).unwrap_or(initial_enabled);
            if final_enabled == target_enabled {
                break;
            }
        }
    }

    // F2: once the mirror has flipped, the integration_state row is visible, so
    // the e2e latency interaction for this set_field must have CLOSED at the
    // projection. Left pending, it later expires as `e2e_expired` — WARN spam
    // and an unmeasured (SLO-blind) interaction. Captured here, asserted after
    // teardown alongside the rest.
    let interaction_still_pending = holon_api::latency_e2e::pending_targets()
        .iter()
        .any(|t| t == "integration:gcal");

    // Teardown BEFORE the assertions so a red does not also trip the gpui leak
    // detector and bury the real failure. Tolerate a poisoned teardown after a
    // caught panic — the assertion message is what must survive.
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        drop(rebind);
        app.update(|cx| cx.shutdown());
        app.run_until_parked();
    }));
    std::mem::forget(app);
    std::mem::forget(bundle);

    if let Err(payload) = &click_result {
        panic!(
            "clicking the multi-param {SET_FIELD_GCAL:?} op_button must open a param-collection \
             popup, not panic. It panicked: {}",
            panic_message(payload.as_ref())
        );
    }
    assert!(
        popup_painted,
        "clicking {SET_FIELD_GCAL:?} must open the param-collection popup ({PARAM_POPUP_TAG:?}); \
         nothing of that tag was painted. Census: {}",
        painted_widget_census(&bounds)
    );
    assert!(
        field_item_painted,
        "the popup's first step must offer the OneOf field choice {FIELD_ITEM_ENABLED:?}"
    );
    assert_eq!(
        final_enabled,
        target_enabled,
        "picking field=enabled then value={} must dispatch set_field and flip gcal's mirror \
         `enabled` from {initial_enabled} to {target_enabled}",
        target_enabled == 1
    );
    assert!(
        !interaction_still_pending,
        "the popup-driven set_field e2e latency interaction for \"integration:gcal\" must CLOSE \
         once the projection lands (the mirror already flipped) — left pending it expires as \
         `e2e_expired`: WARN spam and an unmeasured, SLO-blind interaction"
    );
}

/// `todoist`'s generic `set_field` op_button — the "other row" a dismissal test
/// clicks to prove opening a second menu closes the first.
const SET_FIELD_TODOIST: &str = "op-button-set_field-integration:todoist";

/// Dispatch one Escape keystroke to the focused node.
fn press_escape(app: &mut HeadlessAppContext, window: gpui::AnyWindowHandle) {
    app.update(|cx| {
        window
            .update(cx, |_, win, cx| {
                let ks = gpui::Keystroke::parse("escape").expect("parse escape keystroke");
                win.dispatch_keystroke(ks, cx);
            })
            .expect("window alive for the escape keystroke");
    });
}

/// Boot a windowed Holon over a seeded engine, open the Settings modal, and run
/// `body` with the live handles. Tears down (and forgets) whether or not `body`
/// panics, so an assertion failure surfaces cleanly instead of tripping the
/// gpui leak detector.
fn with_settings_open(
    body: impl FnOnce(
        &mut HeadlessAppContext,
        gpui::AnyWindowHandle,
        &BoundsRegistry,
        &Arc<tokio::runtime::Runtime>,
        &DbHandle,
    ),
) {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });
    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: single-threaded test binary, set before the app boots.
    unsafe { std::env::set_var("HOME", home.path()) };

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
    let set = ComponentSet::full_headless();
    let bundle = runtime
        .block_on(async { compose_sut_windowed_base_seeded(&set, &resolver, &[], &[]).await });
    let session = bundle.session.clone().expect("full_headless -> session");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> reactive engine");
    let db = bundle
        .engine
        .as_ref()
        .expect("full_headless -> turso engine")
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
                "Holon-SetFieldPopup-Dismissal-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");
    let window = rebind.window();

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));
    let gear = bounds
        .element_info(SETTINGS_GEAR)
        .expect("the toolbar gear must be painted so Settings can be opened");
    click_at(&mut app, window, center_of(&gear), "gear");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        body(&mut app, window, &bounds, &runtime, &db);
    }));

    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        drop(rebind);
        app.update(|cx| cx.shutdown());
        app.run_until_parked();
    }));
    std::mem::forget(app);
    std::mem::forget(bundle);

    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn outside_click_on_inert_space_closes_the_popup_without_dispatching() {
    with_settings_open(|app, window, bounds, runtime, db| {
        let initial = mirror_enabled(runtime, db, GCAL).expect("gcal has a mirror row");
        let btn = bounds
            .element_info(SET_FIELD_GCAL)
            .expect("gcal's set_field op_button must be painted");
        click_at(app, window, center_of(&btn), "gcal set_field");
        settle_to_fixed_point(app, bounds, runtime, Duration::from_secs(10));
        assert!(
            painted_count(bounds, PARAM_POPUP_TAG) > 0,
            "precondition: clicking set_field opens the popup"
        );

        // Click INERT space on gcal's OWN row — its provider/status text, left of
        // the state_toggle and of the op-button column, so NO op_button and no
        // toggle is hit. That means no operation dispatch AND no structural
        // rebuild, so the ephemeral wipe cannot fire — the ONLY thing that can
        // close the popup here is its own `on_mouse_down_out`. This is the rung
        // that actually pins that handler (the two-row test below is closed by
        // the wipe in this environment; see the file header). Anchored off the
        // button's top-left, which the deterministic headless layout fixes.
        let btn = bounds
            .element_info(SET_FIELD_GCAL)
            .expect("gcal's set_field op_button must be painted");
        let inert = Point {
            x: Pixels::from(btn.x - 150.0),
            y: Pixels::from(btn.y + 60.0),
        };
        click_at(app, window, inert, "inert space on gcal's row");
        settle_to_fixed_point(app, bounds, runtime, Duration::from_secs(10));

        assert_eq!(
            painted_count(bounds, PARAM_POPUP_TAG),
            0,
            "clicking outside the popup on inert space (no op_button → no structural rebuild) must \
             close it via on_mouse_down_out"
        );
        assert_eq!(
            mirror_enabled(runtime, db, GCAL).expect("gcal row"),
            initial,
            "an abandoned popup must dispatch NO set_field — gcal's `enabled` must be unchanged"
        );
    });
}

#[test]
fn escape_closes_the_param_popup_without_dispatching() {
    with_settings_open(|app, window, bounds, runtime, db| {
        let initial = mirror_enabled(runtime, db, GCAL).expect("gcal has a mirror row");
        let btn = bounds
            .element_info(SET_FIELD_GCAL)
            .expect("gcal's set_field op_button must be painted");
        click_at(app, window, center_of(&btn), "set_field");
        settle_to_fixed_point(app, bounds, runtime, Duration::from_secs(10));
        assert!(
            painted_count(bounds, PARAM_POPUP_TAG) > 0,
            "precondition: clicking set_field opens the popup"
        );

        // Escape must close it. Retry a few settles to absorb a focus-mount race;
        // before the fix nothing consumes Escape, so the popup never closes and
        // the loop simply runs out — the red.
        for _ in 0..5 {
            press_escape(app, window);
            settle_to_fixed_point(app, bounds, runtime, Duration::from_secs(2));
            if painted_count(bounds, PARAM_POPUP_TAG) == 0 {
                break;
            }
        }

        assert_eq!(
            painted_count(bounds, PARAM_POPUP_TAG),
            0,
            "Escape must close the param-collection popup"
        );
        assert_eq!(
            mirror_enabled(runtime, db, GCAL).expect("gcal row"),
            initial,
            "an abandoned popup must dispatch NO set_field — gcal's `enabled` must be unchanged"
        );
    });
}

#[test]
fn opening_a_second_rows_op_button_closes_the_first() {
    with_settings_open(|app, window, bounds, runtime, db| {
        let gcal_initial = mirror_enabled(runtime, db, GCAL).expect("gcal has a mirror row");
        let gcal_btn = bounds
            .element_info(SET_FIELD_GCAL)
            .expect("gcal's set_field op_button must be painted");
        click_at(app, window, center_of(&gcal_btn), "gcal set_field");
        settle_to_fixed_point(app, bounds, runtime, Duration::from_secs(10));
        assert_eq!(
            painted_count(bounds, PARAM_POPUP_TAG),
            1,
            "precondition: exactly gcal's popup is open"
        );

        // Opening another row's op_button must never leave two popups stacked.
        // NOTE: in this windowed environment the first popup is closed by the
        // ephemeral-cache WIPE on the structural rebuild that opening the second
        // triggers — NOT by `on_mouse_down_out` (this rung stays green with that
        // handler disabled; see the file header and
        // `outside_click_on_inert_space_...`, which pins the handler directly).
        // It is kept as an end-to-end guard on the "never two stacked" invariant.
        let todoist_btn = bounds
            .element_info(SET_FIELD_TODOIST)
            .expect("todoist's set_field op_button must be painted");
        click_at(app, window, center_of(&todoist_btn), "todoist set_field");
        settle_to_fixed_point(app, bounds, runtime, Duration::from_secs(10));

        // gcal's popup must be gone — the click landed outside it (on another
        // row), so its `on_mouse_down_out` dismissed it.
        assert!(
            bounds
                .element_info("op-param-popup-set_field-integration:gcal")
                .is_none(),
            "clicking another row's op_button is a click outside gcal's popup and must close it"
        );
        // And the app must never show two popups stacked (the dogfood defect).
        assert!(
            painted_count(bounds, PARAM_POPUP_TAG) <= 1,
            "opening another row's op_button must never leave two popups stacked; painted {}",
            painted_count(bounds, PARAM_POPUP_TAG)
        );
        assert_eq!(
            mirror_enabled(runtime, db, GCAL).expect("gcal row"),
            gcal_initial,
            "dismissing gcal's popup by clicking away must dispatch NO set_field on gcal"
        );
    });
}
