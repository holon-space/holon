//! A secret held only in the keychain paints "Stored in the keychain", in a
//! REAL window.
//!
//! Settings now writes credentials to the OS keychain instead of to plaintext
//! `holon.toml`, so on the next boot the preference map is EMPTY for that key
//! while the integration is fully configured and working. The settings row
//! renders from that map, so without a disclosure it paints "Not set" about a
//! value that is in force — a working integration reading as unconfigured,
//! which is the silent degradation the error policy forbids outright.
//!
//! Only a window can answer what the row actually says, which is why this sits
//! at the windowed tier rather than beside the row-data test in
//! `crates/holon-frontend/tests/stored_secret_row.rs`.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_stored_secret_windowed -- --test-threads=1`
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

/// The toolbar affordance that opens Settings — the modal's only door.
const SETTINGS_GEAR: &str = "settings-gear";

/// The account the Todoist key is filed under. Keyed by the normalized
/// preference key, which is what `${TODOIST_API_KEY}` also resolves to.
const TODOIST_ACCOUNT: &str = "todoist_api_key";

/// Synthetic. It must never be painted, so it is distinctive enough that a
/// substring search over every painted string cannot miss it.
const TODOIST_TOKEN: &str = "SYNTHETIC-TODOIST-TOKEN-MUST-NOT-PAINT";

/// What the row must say. Kept in step with `pref_field.rs`'s `SECRET_STORED`
/// by this test failing if that string changes.
const STORED_DISCLOSURE: &str = "Stored in the keychain";

/// What it must NOT say for a configured secret.
const NOT_SET: &str = "Not set";

/// The registered element carrying the Todoist row's VALUE, so an assertion
/// reads that row rather than scanning every string in the window.
const TODOIST_VALUE_ELEMENT: &str = "pref-value-todoist.api_key";

/// The control row: a secret with nothing stored anywhere.
const SHOPPING_VALUE_ELEMENT: &str = "pref-value-shopping.list_url";

fn painted_texts(bounds: &BoundsRegistry) -> Vec<String> {
    bounds
        .all_elements()
        .into_iter()
        .filter_map(|(_, info)| info.displayed_text.as_deref().map(str::to_string))
        .collect()
}

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

fn census(bounds: &BoundsRegistry) -> String {
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for (_, info) in bounds.all_elements() {
        if let Some(node) = &info.vm_node {
            *tags.entry(node.tag.to_string()).or_default() += 1;
        }
    }
    format!("{tags:?}")
}

#[test]
fn a_secret_held_only_in_the_keychain_paints_as_stored() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: single-threaded test binary (`--test-threads=1`), set before the
    // app boots and before any thread reads the environment.
    unsafe { std::env::set_var("HOME", home.path()) };
    // The environment must NOT supply this one, or the row renders through the
    // locked path and says "Set by CLI/environment" instead.
    // SAFETY: as above.
    unsafe { std::env::remove_var("TODOIST_API_KEY") };

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

    // The whole point: the secret is in the STORE and nowhere else. No
    // `set_preference` call, so `holon.toml` stays empty for this key, exactly
    // as it is on the boot after the user typed the value in.
    //
    // SEEDED into the store the fixture already injected, rather than injecting
    // a second one: the test harness binds an in-memory store to every session
    // it latches, so nothing here can reach the machine's real keychain even if
    // this line were deleted.
    assert!(
        holon_frontend::platform_keychain_forbidden(),
        "the fixture must refuse the OS keychain before this test runs"
    );
    session
        .seed_secret_for_test(TODOIST_ACCOUNT, TODOIST_TOKEN.as_bytes())
        .expect("the injected store accepts the secret");

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
                "Holon-StoredSecret-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let gear = bounds
        .element_info(SETTINGS_GEAR)
        .unwrap_or_else(|| panic!("the toolbar gear is the only door into Settings"));
    let window = rebind.window();
    click_at(&mut app, window, center_of(&gear), "gear");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let texts = painted_texts(&bounds);
    let widgets = census(&bounds);

    // Read everything the assertions need BEFORE teardown, and tear down
    // BEFORE asserting: a red inside a live window also trips gpui's leak
    // detector, which buries the real failure under a handle dump.
    let todoist_painted = bounds
        .element_info(TODOIST_VALUE_ELEMENT)
        .and_then(|i| i.displayed_text.clone());
    let shopping_painted = bounds
        .element_info(SHOPPING_VALUE_ELEMENT)
        .and_then(|i| i.displayed_text.clone());
    let leaked = texts.iter().find(|t| t.contains(TODOIST_TOKEN)).cloned();

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    assert!(
        !texts.is_empty(),
        "the Settings modal painted nothing: {widgets}"
    );
    if let Some(leak) = leaked {
        panic!("the settings window painted a stored credential: {leak:?}");
    }

    // Scoped to THIS row's own element, not to the whole window: another
    // secret row is legitimately unconfigured and says "Not set", so a
    // window-wide scan would either miss the defect or forbid a correct row.
    let painted = todoist_painted.unwrap_or_else(|| {
        panic!(
            "the Todoist row's value element {TODOIST_VALUE_ELEMENT:?} painted nothing: {widgets}"
        )
    });
    assert!(
        painted.contains(STORED_DISCLOSURE),
        "the Todoist key is held in the keychain, so its row must disclose that rather than \
         leaving the user to guess. That row painted {painted:?}"
    );
    assert_ne!(
        painted.trim(),
        NOT_SET,
        "a configured secret must not read as unconfigured — that is the silent-degradation \
         wording this test exists to forbid"
    );

    // The control: a secret that genuinely is NOT stored must still say so, or
    // the disclosure above would be unconditional and mean nothing.
    let unset = shopping_painted
        .unwrap_or_else(|| panic!("the shopping row's value element painted nothing: {widgets}"));
    assert_eq!(
        unset.trim(),
        NOT_SET,
        "nothing is stored for the shopping list URL, so its row must still read {NOT_SET:?}"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
