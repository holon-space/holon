//! The soft keyboard must stay up while a text input still owns window focus.
//!
//! gpui derives its focus-out EVENT from the rendered frame's focus PATH
//! (`Window::focus_path` → `DispatchTree::focus_path`), which goes empty when
//! the focused element is absent from that frame's dispatch tree or when the
//! window is inactive — while `window.focus` keeps naming the same element.
//! Holon's editor treated that event as "the user left the editor" and
//! scheduled the platform keyboard hide.
//!
//! On Android that misread is user-visible: raising the IME resizes the
//! content rect (`adjustResize`), the relayout drops the focused row out of
//! the frame, the synthesized blur fires and ~150ms later the keyboard the
//! user just opened closes itself (device-measured 2026-08-27, OnePlus
//! DN2103: `safe_area_insets updated: bottom=48 -> bottom=792` followed by
//! `hide_keyboard_android` with no tap in between).
//!
//! `VisualTestContext::deactivate_window` produces the SAME class of
//! synthesized blur deterministically under `TestPlatform` — empty
//! `current_focus_path`, untouched `window.focus` — so the invariant is
//! testable on the desktop:
//!
//!   INV: a focus-out event never advances `keyboard_hide_requests()` while
//!   the input still holds `window.focus`.
//!
//! Run: cargo test -p holon-gpui --features pbt --test
//! soft_keyboard_survives_spurious_blur

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::TestAppContext;
use gpui::VisualTestContext;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::soft_keyboard;
use holon_integration_tests::pbt::window_slice::seed::graft_undo_blur_pair;
use holon_integration_tests::test_environment::TestEnvironment;

#[test]
fn spurious_blur_does_not_dismiss_the_soft_keyboard() {
    let cx = TestAppContext::single();

    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let env = runtime.block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    // Two real rows under the Main focus root, so there is something to tap.
    let (edit_id, _sibling_id) = runtime
        .block_on(graft_undo_blur_pair(&env, "softkbd"))
        .expect("graft the row pair");

    let session = env.session_arc();
    let engine = env.reactive_engine.get().cloned().expect("reactive engine");

    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();

    let rebind_handle = cx
        .update(|app| {
            launch_holon_window_rebindable(
                session,
                engine,
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                None,
                None,
                "Holon-SoftKeyboard-Test",
                app,
            )
        })
        .expect("window opened");

    let window = rebind_handle.window();
    let mut visual = VisualTestContext::from_window(window, &cx);

    // Precondition, not a timing guess: the row must be PAINTED before a tap
    // can focus its editor.
    let row_element = format!("block:{edit_id}");
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut target = None;
    while Instant::now() < deadline {
        settle(&cx, &bounds);
        target = row_center(&bounds, &row_element);
        if target.is_some() {
            break;
        }
        eprintln!(
            "[soft-kbd] row {row_element} not painted yet; {} elements",
            bounds.all_elements().len()
        );
    }
    let target = target.expect("boot precondition: the grafted row must paint before it is tapped");

    // A phone app is the active window while the user types in it; under
    // `TestPlatform` a launched window starts inactive, and an inactive window
    // never reports a focus path at all.
    cx.update(|app| {
        let _ = window.update(app, |_, w, _| w.activate_window());
    });
    cx.run_until_parked();

    // The tap that raises the keyboard on the phone.
    visual.simulate_mouse_move(target, None, Default::default());
    visual.simulate_click(target, Default::default());
    settle(&cx, &bounds);

    let focused_before = cx
        .update(|app| window.update(app, |_, w, cx| w.focused(cx)))
        .expect("window alive");
    assert!(
        focused_before.is_some(),
        "precondition: tapping the row must give an input window focus — otherwise this test \
         cannot observe the keyboard lifecycle at all"
    );
    assert!(
        soft_keyboard::keyboard_show_requests() > 0,
        "precondition: the focused input must have raised the soft keyboard (focus-gain never \
         reached the keyboard lifecycle)"
    );

    let hide_before = soft_keyboard::keyboard_hide_requests();

    // NON-VACUITY: `deactivate_window` is a no-op unless this window is the
    // platform's active one, and an inactive window emits no focus event at
    // all — the exercise below would then prove nothing.
    assert!(
        cx.update(|app| window.update(app, |_, w, _| w.is_window_active()))
            .expect("window alive"),
        "precondition: the window must be ACTIVE before it can be deactivated"
    );

    // The spurious blur: the window goes inactive, so gpui reports an empty
    // focus path and every focus-out listener fires — `window.focus` itself is
    // untouched, exactly as on the Android inset relayout.
    visual.deactivate_window();
    cx.run_until_parked();
    // A focus event is derived at DRAW time from the rendered frame, so the
    // deactivation only reaches the listeners once the window redraws.
    cx.update(|app| {
        let _ = window.update(app, |_, w, _| w.refresh());
    });
    cx.run_until_parked();
    assert!(
        !cx.update(|app| window.update(app, |_, w, _| w.is_window_active()))
            .expect("window alive"),
        "the deactivation must actually land — otherwise no focus-out event fires"
    );
    // Past the deferred-hide grace, so a scheduled hide has really fired.
    cx.executor()
        .advance_clock(soft_keyboard::KEYBOARD_HIDE_GRACE * 4);
    cx.run_until_parked();
    bounds.flush();

    let focused_after = cx
        .update(|app| window.update(app, |_, w, cx| w.focused(cx)))
        .expect("window alive");
    assert_eq!(
        focused_after, focused_before,
        "a focus-out event that is only a rendering/activation artefact must leave the \
         authoritative window focus alone"
    );

    assert_eq!(
        soft_keyboard::keyboard_hide_requests(),
        hide_before,
        "the soft keyboard was dismissed although the input still holds window focus — this is \
         the Android IME-inset flash: geometry (or deactivation) must not clear focus state"
    );

    // Shutdown clears windows, but detached pump tasks still hold entity
    // handles and gpui's leak detector runs before the dispatcher drops
    // them — leak the contexts at test end (process exits right after).
    drop(rebind_handle);
    cx.update(|app| app.shutdown());
    cx.run_until_parked();
    std::mem::forget(visual);
    std::mem::forget(cx);
}

/// Pump gpui and the backend to a rendering fixed point: real tokio time
/// between pump cycles so the multi-level reactive cascade (panels → rows)
/// fully renders.
fn settle(cx: &TestAppContext, bounds: &BoundsRegistry) {
    let mut last_count = 0usize;
    let mut stable = 0u32;
    for _ in 0..500 {
        std::thread::sleep(Duration::from_millis(10));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        bounds.flush();
        let elements = bounds.all_elements();
        let count = elements.len();
        let still_loading = elements
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count > 0 && count == last_count && !still_loading {
            stable += 1;
            if stable >= 3 {
                break;
            }
        } else {
            stable = 0;
        }
        last_count = count;
    }
}

/// Centre of the painted text of `entity` in the main region, if it is
/// rendered at all.
fn row_center(bounds: &BoundsRegistry, entity: &str) -> Option<gpui::Point<gpui::Pixels>> {
    bounds.flush();
    bounds
        .all_elements()
        .into_iter()
        .map(|(_, info)| info)
        .filter(|info| info.entity_id.as_deref() == Some(entity))
        .find(|info| matches!(info.widget_type.as_ref(), "rendered_text" | "editable_text"))
        .map(|info| {
            gpui::point(
                gpui::px(info.x + info.width / 2.0),
                gpui::px(info.y + info.height / 2.0),
            )
        })
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
