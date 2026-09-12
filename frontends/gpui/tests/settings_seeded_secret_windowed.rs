//! A secret planted by a SEED FILE paints as stored, masked, in a REAL window.
//!
//! This is the rung the two `user-connections` dogfood passes could not stand
//! in for. Storing a credential through the UI opens a native masked dialog
//! (macOS `display dialog`), which needs System Events — an automation
//! permission an agent session does not have — so both passes reported the
//! "Stored in the keychain" row as UNVERIFIED and the flow shipped on the word
//! of a row-data test.
//!
//! `HOLON_SECRETS_MEMORY_SEED` is the seam that closes it: a throwaway session
//! is handed its fixture secrets at boot. What that seam has to be worth is
//! exactly this — the account the FILE names reaches the painted row. So the
//! accounts here come from a real seed file through
//! `holon_secrets::parse_seed_file`, not from a spelling written out by hand:
//! the file says `TODOIST_API_KEY`, the row is keyed `todoist.api_key`, and a
//! folding rule restated in a fixture would agree with itself and with nothing
//! else.
//!
//! The admission half of the seam — that a seed is refused unless the in-memory
//! backend is selected — is pinned headlessly (`holon-secrets`'
//! `a_seed_is_refused_outside_the_in_memory_backend`,
//! `holon-app/tests/in_memory_secret_backend_boot.rs`). This rung is about what
//! is PAINTED, which is the half no headless test can answer.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_seeded_secret_windowed -- --test-threads=1`
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

/// How the seed file spells the account. Deliberately the `${VAR}` form, which
/// is NOT how the preference row is keyed: the folding is the property.
const SEED_KEY: &str = "TODOIST_API_KEY";

/// Synthetic, and distinctive enough that a substring search over every painted
/// string cannot miss it if the mask ever fails.
const SEEDED_SECRET: &str = "SYNTHETIC-SEEDED-TOKEN-MUST-NOT-PAINT";

/// The registered element carrying the Todoist row's VALUE.
const TODOIST_VALUE_ELEMENT: &str = "pref-value-todoist.api_key";

/// The control row: a secret nothing seeded.
const SHOPPING_VALUE_ELEMENT: &str = "pref-value-shopping.list_url";

/// What the row must say, and the mask it must say it behind. Kept in step with
/// `pref_field.rs`'s `SECRET_STORED` by this test failing if either changes.
const STORED_DISCLOSURE: &str = "Stored in the keychain";
const MASK: &str = "••••••••";
const NOT_SET: &str = "Not set";

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
fn a_secret_planted_by_a_seed_file_paints_as_stored_and_masked() {
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

    let seed_file = home.path().join("fixture-secrets.toml");
    std::fs::write(&seed_file, format!("{SEED_KEY} = \"{SEEDED_SECRET}\"\n"))
        .expect("write the seed file");
    let seeded = holon_secrets::parse_seed_file(&seed_file)
        .expect("the seed file this test just wrote must parse");

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

    // Nothing here can reach the machine's real keychain: the harness binds an
    // in-memory store to every session it latches and refuses the OS one.
    assert!(
        holon_frontend::platform_keychain_forbidden(),
        "the fixture must refuse the OS keychain before this test runs"
    );
    assert_eq!(
        seeded.len(),
        1,
        "the fixture seeds exactly one account, or the control row below is not a control"
    );
    for (account, secret) in &seeded {
        session
            .seed_secret_for_test(account, secret)
            .expect("the injected store accepts the seeded secret");
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
                "Holon-SeededSecret-Windowed",
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
    let todoist_painted = bounds
        .element_info(TODOIST_VALUE_ELEMENT)
        .and_then(|i| i.displayed_text.clone());
    let shopping_painted = bounds
        .element_info(SHOPPING_VALUE_ELEMENT)
        .and_then(|i| i.displayed_text.clone());
    let leaked = texts.iter().find(|t| t.contains(SEEDED_SECRET)).cloned();

    // Teardown BEFORE the assertions so a red does not also trip the gpui leak
    // detector, which would bury the real failure.
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
        panic!("the settings window painted the seeded credential: {leak:?}");
    }

    let painted = todoist_painted.unwrap_or_else(|| {
        panic!(
            "the Todoist row's value element {TODOIST_VALUE_ELEMENT:?} painted nothing: {widgets}"
        )
    });
    assert!(
        painted.contains(STORED_DISCLOSURE),
        "a credential the seed file planted under {SEED_KEY:?} must reach the row keyed \
         'todoist.api_key' and paint as stored — if the folding differs, the row says the \
         credential is absent while the resolver uses it. That row painted {painted:?}"
    );
    assert!(
        painted.contains(MASK),
        "and it must say so behind the mask, never in the clear: {painted:?}"
    );

    // The control: a secret nothing seeded must still say so, or the assertion
    // above would be unconditional and mean nothing.
    let unset = shopping_painted
        .unwrap_or_else(|| panic!("the shopping row's value element painted nothing: {widgets}"));
    assert_eq!(
        unset.trim(),
        NOT_SET,
        "the seed file names one account, so every other secret row must still read {NOT_SET:?}"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
