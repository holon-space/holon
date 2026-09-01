//! The Settings modal's integration rows paint their operations, in a REAL
//! window over a REAL booted engine.
//!
//! The integration row's `Configure…` affordance is layout data
//! (`assets/default/types/integration_profile.yaml`: an `ops_of` collection
//! feeding `op_button`), and `ops_of` hands it over as a STREAMING collection.
//! Only the production shell mounts one — neither a VM snapshot nor a
//! `ReactiveFixtureView` does — so this is the tier that answers "does the user
//! see the button at all".
//!
//! The modal's one way in is the toolbar gear, so the rung clicks it.
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
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::InputEvent;
use gpui::MouseButton;
use gpui::Pixels;
use gpui::Point;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::preferences::PrefKey;
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

/// `gcal` declares an OAuth2 consent flow; `todoist` authenticates with a
/// static token and must offer no way to start one.
const GCAL_CONFIGURE: &str = "op-button-begin_oauth-integration:gcal";
const TODOIST_CONFIGURE: &str = "op-button-begin_oauth-integration:todoist";

/// The shopping peer's list URL is a credential — the list's capability token
/// sits in a path segment — so the settings row for it must paint a mask, never
/// the value. Only a real window can answer that.
const SHOPPING_PREF_KEY: &str = "shopping.list_url";

/// Synthetic. A real list URL is a live credential and never belongs in a
/// fixture. Nothing else in this file contains this literal, so finding it in
/// painted text is a leak.
const LIST_URL_TOKEN: &str = "abc123SYNTHETICwindowedTOKENq7Wv";

/// What a user pastes into Settings. The token sits in an UNMARKED segment:
/// this rung is about masking on the way to the screen, not about the
/// redactor's separate `!`-marker rule.
const STORED_LIST_URL: &str = "https://shop.example/c/abc123SYNTHETICwindowedTOKENq7Wv/api";

/// The glyphs a masked secret paints.
const SECRET_MASK: &str = "••••••••";

/// Exported so the shopping row renders its LOCKED state: the environment
/// outranks the stored value, which is the other of the two paths that can put
/// a secret on screen (`build_locked_display` rather than `build_text_field`).
const EXPORTED_LIST_URL: &str = "https://shop.example/c/abc123SYNTHETICexportedTOKENz4Kd/api";
const EXPORTED_LIST_TOKEN: &str = "abc123SYNTHETICexportedTOKENz4Kd";

/// A second stored secret whose override is NOT exported, so one masked row is
/// locked and one is editable. Without both, reverting either mask stays green.
const TODOIST_PREF_KEY: &str = "todoist.api_key";
const TODOIST_TOKEN: &str = "abc123SYNTHETICtodoistTOKENm5Rt";

/// Every text the window painted, so a failure names what the user would have
/// seen instead of only that an assertion tripped.
fn painted_texts(bounds: &BoundsRegistry) -> Vec<String> {
    bounds
        .all_elements()
        .into_iter()
        .filter_map(|(_, info)| info.displayed_text.as_deref().map(str::to_string))
        .collect()
}

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

/// How many elements of `tag` the window painted.
fn painted_count(bounds: &BoundsRegistry, tag: &str) -> usize {
    bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.vm_node.as_ref().is_some_and(|n| n.tag.as_ref() == tag))
        .count()
}

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

    // Point HOME at an empty dir BEFORE anything reads it: the bundled gcal
    // sidecar names `~/.config/holon/gcal-client-*`, and on a machine that has
    // those files the consent flow would reach a real browser.
    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: single-threaded test binary (`--test-threads=1`), set before the
    // app boots and before any thread reads the environment.
    unsafe { std::env::set_var("HOME", home.path()) };
    // One override exported and one removed, so the modal paints a LOCKED
    // secret row and an EDITABLE one in the same window.
    // SAFETY: as above — single-threaded, before the app boots.
    unsafe {
        std::env::set_var("SHOPPING_LIST_URL", EXPORTED_LIST_URL);
        std::env::remove_var("TODOIST_API_KEY");
    }

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

    // Stored before the first render, so the rows the modal paints hold
    // credentials rather than empty fields.
    for (key, value) in [
        (SHOPPING_PREF_KEY, STORED_LIST_URL),
        (TODOIST_PREF_KEY, TODOIST_TOKEN),
    ] {
        session
            .set_preference(&PrefKey::new(key), toml::Value::String(value.into()))
            .unwrap_or_else(|e| panic!("{key} must persist into the test profile: {e:#}"));
    }

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
    click_at(&mut app, window, center_of(&gear), "gear");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let painted = painted_op_buttons(&bounds);
    let census = painted_widget_census(&bounds);
    // The modal hosts the preferences half too, and it renders through the same
    // `interpret_and_render` path as the integrations section.
    let prefs = painted_count(&bounds, "pref_field");
    let gcal = bounds.element_info(GCAL_CONFIGURE);
    let todoist = bounds.element_info(TODOIST_CONFIGURE);

    assert!(
        !painted.is_empty(),
        "the open Settings modal must paint the integration rows' operations. The modal IS open \
         and the rows ARE rendered — what the window painted: {census}"
    );
    assert!(
        prefs > 0,
        "the modal's preferences half must still render alongside the integrations section: \
         {census}"
    );

    // What the user's eyes get. The stored list URL is a credential, so the row
    // must paint the mask and nothing that contains the token.
    let texts = painted_texts(&bounds);
    for token in [LIST_URL_TOKEN, EXPORTED_LIST_TOKEN, TODOIST_TOKEN] {
        if let Some(leak) = texts.iter().find(|t| t.contains(token)) {
            panic!(
                "the settings window painted a credential: {leak:?} — neither a stored secret nor \
                 an exported one may reach the screen"
            );
        }
    }
    // Two masked rows: the shopping row is locked (its override is exported, so
    // it renders through `build_locked_display`) and the todoist row is
    // editable (`build_text_field`). Requiring both is what gives each mask its
    // own teeth — with one row, reverting the other mask stays green.
    let masked = texts.iter().filter(|t| *t == SECRET_MASK).count();
    assert!(
        masked >= 2,
        "expected a masked LOCKED row and a masked EDITABLE row, saw {masked} mask(s). Without \
         both, the leak check above can pass by finding no text at all. painted: {texts:?}"
    );
    let gcal = gcal.unwrap_or_else(|| {
        panic!("gcal has an unrun consent flow, so its row must paint {GCAL_CONFIGURE}: {census}")
    });
    assert!(
        todoist.is_none(),
        "todoist authenticates without a consent flow, so its row must paint no \
         {TODOIST_CONFIGURE}"
    );
    assert_eq!(
        gcal.displayed_text.as_deref(),
        Some("Configure…"),
        "the button must carry the words the user reads"
    );

    // The click, through the real mouse path. The flow it starts cannot reach a
    // browser: `HOME` points at an empty tempdir, so the sidecar's
    // `~/.config/holon/gcal-client-*` credentials resolve to nothing and the
    // flow fails before `BrowserOpener::open`.
    click_at(&mut app, window, center_of(&gcal), "Configure…");

    // The click's effect, read where the guard reads it: the mirror column the
    // projector writes when the view model's progress signal fires. Non-empty
    // means the whole chain ran — present_op, the dispatcher, the operation
    // provider, the consent flow, the view model cell, the projector.
    let db = bundle
        .engine
        .as_ref()
        .expect("full_headless -> a Turso engine")
        .db_handle()
        .clone();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut progress = String::new();
    while Instant::now() < deadline {
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(2));
        progress = runtime.block_on(async {
            db.query(
                "SELECT provider_name, configure_progress FROM integration_state",
                Default::default(),
            )
            .await
            .expect("read the mirror")
            .iter()
            .find(|r| r.get("provider_name").and_then(|v| v.as_string()) == Some("gcal"))
            .and_then(|r| r.get("configure_progress"))
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string()
        });
        if !progress.is_empty() {
            break;
        }
    }
    let withdrew = bounds.element_info(GCAL_CONFIGURE).is_none();

    // Close and reopen the modal, which re-interprets the section from scratch.
    // If the button is gone THEN, the row data and the guard were right all
    // along and only the repaint was missing — the difference between "stale
    // data" and "stale pixels".
    let gear = bounds
        .element_info(SETTINGS_GEAR)
        .expect("the gear is still painted");
    click_at(&mut app, window, center_of(&gear), "gear (close)");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(5));
    let gear = bounds
        .element_info(SETTINGS_GEAR)
        .expect("the gear is still painted");
    click_at(&mut app, window, center_of(&gear), "gear (reopen)");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(5));
    let withdrew_on_reopen = bounds.element_info(GCAL_CONFIGURE).is_none();

    // Teardown BEFORE the assertions so a red does not also trip the gpui leak
    // detector, which would bury the real failure.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    assert!(
        !progress.is_empty(),
        "clicking {GCAL_CONFIGURE} must START the consent flow and the projector must carry what \
         it says onto the row; `configure_progress` is still empty"
    );
    assert!(
        withdrew,
        "the guard reads `configure_progress`, and it is now {progress:?} — so the row must stop \
         offering {GCAL_CONFIGURE} while the user is looking at it, with no reopen. (Reopening \
         withdraws it: {withdrew_on_reopen}.)"
    );
    assert!(
        withdrew_on_reopen,
        "closing and reopening the modal must not resurrect {GCAL_CONFIGURE} — a reopened modal \
         reuses the cached shell, and a stale one would offer an operation the guard refuses"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
