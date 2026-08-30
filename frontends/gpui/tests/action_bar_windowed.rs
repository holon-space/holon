//! The action bar docked above the soft keyboard (Martin, 2026-08-30): on a
//! phone-shaped window the bottom dock carries one `op_button` per operation
//! the FOCUSED entity offers, it appears only while the keyboard is up, and it
//! sits above the main panel rather than over it.
//!
//! The bar is assembled declaratively, not by hand: every perspective
//! synthesizes `if_space(600, bottom_dock(columns({narrow}), list(#{gap: 8,
//! horizontal: true, collection: chain_ops(0),
//! item_template: op_button(col("name"))})), …)`
//! (`crates/holon-api/src/perspective.rs`), the GPUI builder registry
//! dispatches `bottom_dock` / `op_button` by widget name, and `chain_ops(0)`
//! re-projects the focused block through the operation catalog. So these rungs
//! judge what reaches the SCREEN — which ops, in what order, under which inset
//! — and not whether some node exists in a view-model tree.
//!
//! Why WINDOWED and not the headless keystone: the inset, the dock's row band,
//! the panel's flex allocation and the horizontal overflow are all painted
//! geometry. The keystone has no window and structurally cannot see any of it.
//!
//! Run: `cargo nextest run -p holon-gpui --test action_bar_windowed --features
//! holon-gpui/pbt --no-fail-fast` (nextest gives each test its own process,
//! which the gpui test platform's thread-local state requires).

use std::sync::Arc;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::InputEvent;
use gpui::MouseButton;
use gpui::Pixels;
use gpui::Point;
use holon_api::EntityUri;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::RebindHandle;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::graft_displayed_text_tree;
use holon_integration_tests::test_environment::TestEnvironment;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// A phone-shaped window: narrower than the `if_space(600, …)` breakpoint, so
/// the synthesized layout takes its `bottom_dock` branch. Tall enough that the
/// outline still paints with the keyboard up (the geometry pinned by
/// `block_focus_keeps_outline_windowed`).
const PHONE_WINDOW: &str = "393x852";
/// A desktop window — above both breakpoints, so no dock branch at all.
const DESKTOP_WINDOW: &str = "1200x900";
/// [`PHONE_WINDOW`]'s width — the edge past which a bar button has scrolled out
/// of the first screenful and can no longer be tapped without a swipe.
const PHONE_WIDTH_PX: f32 = 393.0;

/// The soft keyboard's own height, in logical px — measured off Martin's
/// DN2103 (792 physical at ~2.75x).
const KEYBOARD_HEIGHT_PX: f32 = 288.0;
/// The bottom inset a phone has with the keyboard DOWN: the home indicator or
/// gesture area. Non-zero on every real device, which is the whole reason the
/// bar cannot be gated on the inset.
const RESTING_INSET_PX: f32 = 34.0;

/// The row `graft_displayed_text_tree` puts the caret in. A child, not the page
/// title, so the tap is the gesture that focuses an ordinary outline row.
const FOCUSED_ROW: &str = "c1";
/// Its sibling — `move_down` on `c1` must push it past this one.
const SIBLING_ROW: &str = "c2";

/// The main panel's live_block id in the shipped layout.
const MAIN_PANEL: &str = "block:default-main-panel";
/// Room the panel needs before an empty outline is the frontend's fault rather
/// than a fixture that left it no space. Three 36px rows need ~110px; the
/// keyboard-inset rung uses the same 200px margin.
const MIN_PANEL_BOX_PX: f32 = 200.0;

/// The op affordances the window is currently DRAWING, as
/// `(op_name, target_uri, box)`, left to right.
///
/// Read from the `op-button-{op}-{target}` trackers, the only record that names
/// WHICH op a box belongs to and WHAT it would act on. The builder registry
/// also wraps each button in a positional `op_button#{seq}` tracker carrying
/// the real width, but those cannot be paired back to an op reliably — a frame
/// draws more of them than there are buttons — so identity and order come from
/// the named tracker alone. Its `x`, `y` and `height` are the button's own;
/// only its width is absent, which is why clicks go through [`press_point`]
/// rather than a centre.
fn painted_op_buttons(bounds: &BoundsRegistry) -> Vec<(String, String, ElementInfo)> {
    let mut found: Vec<(String, String, ElementInfo)> = bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.height > 0.0)
        .filter_map(|(el_id, info)| {
            let rest = el_id.strip_prefix("op-button-")?;
            // `op-button-{op}-{scheme}:{id}` — the target always carries a
            // scheme, so the LAST `-` before the scheme separator splits it.
            let split = rest.rfind('-')?;
            let (op, target) = rest.split_at(split);
            Some((op.to_string(), target[1..].to_string(), info))
        })
        .collect();
    found.sort_by(|a, b| a.2.x.total_cmp(&b.2.x));
    found
}

/// A point inside an op button. Its tracked box has no width (see
/// [`painted_op_buttons`]), so the centre of that box sits on the button's left
/// edge, where a click can fall into the gap between two buttons. Nudging in by
/// a few px lands inside even the narrowest button the bar renders.
fn press_point(info: &ElementInfo) -> Point<Pixels> {
    Point {
        x: Pixels::from(info.x + 4.0),
        y: Pixels::from(info.y + info.height / 2.0),
    }
}

/// Human-readable inventory of what the frame painted — the evidence that tells
/// "the bar never rendered" apart from "it rendered the wrong ops".
fn op_button_census(bounds: &BoundsRegistry) -> String {
    let buttons = painted_op_buttons(bounds);
    if buttons.is_empty() {
        // Which widgets DID paint separates "the dock is there and empty" from
        // "the layout never took its dock branch".
        let mut tags: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for (_, info) in bounds.all_elements() {
            *tags.entry(info.widget_type.to_string()).or_default() += 1;
        }
        let mut raws: Vec<(String, ElementInfo)> = bounds
            .all_elements()
            .into_iter()
            .filter(|(_, i)| i.widget_type.as_ref() == "op_button")
            .collect();
        raws.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
        let raw: Vec<String> = raws
            .iter()
            .take(8)
            .map(|(el, i)| {
                format!(
                    "{el} x={:.1} y={:.1} w={:.1} h={:.1}",
                    i.x, i.y, i.width, i.height
                )
            })
            .collect();
        return format!(
            "<no op_button drawn; widgets: {tags:?}; {} op_button trackers, leftmost: {raw:?}>",
            raws.len()
        );
    }
    buttons
        .iter()
        .map(|(op, target, info)| {
            format!(
                "{op}@{target} x={:.1} y={:.1} h={:.1}",
                info.x, info.y, info.height
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The box of a painted entity, by URI. Tallest match wins — the entity's own
/// wrapper rather than one of the inner text runs.
fn entity_box(bounds: &BoundsRegistry, uri: &str) -> Option<ElementInfo> {
    bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.entity_id.as_deref() == Some(uri))
        .filter(|(_, info)| info.height > 0.0)
        .map(|(_, info)| info)
        .max_by(|a, b| a.height.total_cmp(&b.height))
}

/// Dispatch a real left click at `center` — the same three platform events a
/// finger produces, so the click goes through hit-testing rather than around
/// it.
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

/// What a rung gets to drive: the live window, its rebind seam (the inset), the
/// bounds registry it paints into, and a driver for real gestures.
struct Fixture<'a> {
    app: &'a mut HeadlessAppContext,
    rebind: &'a RebindHandle,
    bounds: BoundsRegistry,
    runtime: Arc<tokio::runtime::Runtime>,
    driver: SimUserDriver,
    window: gpui::AnyWindowHandle,
}

impl Fixture<'_> {
    fn settle(&mut self) {
        settle_to_fixed_point(
            self.app,
            &self.bounds,
            &self.runtime,
            Duration::from_secs(120),
        );
    }

    /// Raise the soft keyboard the way the platform does: the window does NOT
    /// resize, the bottom inset grows and the page container absorbs it. Both
    /// signals move, because they answer different questions — the inset is the
    /// whole unusable strip (resting chrome PLUS keyboard), the height is the
    /// keyboard alone.
    fn raise_keyboard(&mut self) {
        let rebind = self.rebind;
        self.app.update(|cx| {
            rebind.set_safe_area_bottom(RESTING_INSET_PX + KEYBOARD_HEIGHT_PX, cx);
            rebind.set_keyboard_height(KEYBOARD_HEIGHT_PX, cx);
        });
        self.settle();
    }

    /// A phone with the keyboard DOWN: a home indicator or gesture area still
    /// eats the bottom of the screen, so the inset is non-zero while the
    /// keyboard height is not. Desktop leaves both at 0 and cannot tell the two
    /// apart, which is exactly why this has to be set explicitly.
    fn rest_with_bottom_chrome(&mut self) {
        let rebind = self.rebind;
        self.app.update(|cx| {
            rebind.set_safe_area_bottom(RESTING_INSET_PX, cx);
            rebind.set_keyboard_height(0.0, cx);
        });
        self.settle();
    }

    /// Put the caret in an outline row — the gesture that raises the keyboard
    /// on a phone and the one that gives `chain_ops(0)` a focused entity.
    ///
    /// The grafted rows can land a settle or two after the window first reaches
    /// a fixed point (a stable element count with no `loading` placeholder is
    /// reachable before CDC has delivered them), so the row is waited for
    /// rather than assumed. Tapping a row that is not on screen yet reports
    /// "not in bounds", which reads like a missing affordance and is only a
    /// race.
    fn focus_row(&mut self, id: &str) {
        let uri = format!("block:{id}");
        for _ in 0..10 {
            if entity_box(&self.bounds, &uri).is_some() {
                break;
            }
            self.settle();
        }
        assert!(
            entity_box(&self.bounds, &uri).is_some(),
            "the grafted row {uri} never painted, so there is nothing to put the caret in"
        );
        let entity = EntityUri::block(id);
        self.runtime
            .clone()
            .block_on(async { self.driver.click_entity(&entity, "main").await })
            .unwrap_or_else(|e| panic!("tap the outline row {id} to put the caret in it: {e}"));
        self.settle();
    }
}

/// Boot a window over a seeded environment, graft the three-row outline, and
/// hand the live fixture to `run`. `app` stays pinned on this frame for the
/// whole call, so the `*const HeadlessAppContext` the driver holds stays valid.
fn with_action_bar_window(window_size: &str, run: impl FnOnce(&mut Fixture<'_>)) {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", window_size) };

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
                "Holon-Action-Bar",
                cx,
            )
        })
        .expect("window opened");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    runtime
        .block_on(graft_displayed_text_tree(&env))
        .expect("graft the outline the panel draws");
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let app_ptr: *const HeadlessAppContext = &app;
    let driver = SimUserDriver::new(
        app_ptr,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        debug_services
            .interaction_tx
            .get()
            .expect("interaction_tx set by the window interaction pump")
            .clone(),
    );

    let window = rebind.window();
    {
        let mut fixture = Fixture {
            app: &mut app,
            rebind: &rebind,
            bounds: bounds.clone(),
            runtime: runtime.clone(),
            driver,
            window,
        };
        run(&mut fixture);
    }

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

/// MARTIN'S ASK (2026-08-30), first half: with the caret in a block and the
/// keyboard up, the bar offers that block's operations.
///
/// The bar's op set is the focused entity's — `chain_ops(0)` re-projects
/// `UiState.focused_block` through the operation catalog on every focus change
/// — so every button the dock paints must target the row the user is editing.
/// A button aimed anywhere else is an op the user would dispatch against a
/// block they are not looking at.
#[test]
fn the_focused_blocks_ops_paint_in_the_dock_above_the_keyboard() {
    with_action_bar_window(PHONE_WINDOW, |f| {
        f.focus_row(FOCUSED_ROW);
        f.raise_keyboard();

        let focused_uri = format!("block:{FOCUSED_ROW}");
        let census = op_button_census(&f.bounds);
        assert!(
            !painted_op_buttons(&f.bounds).is_empty(),
            "with the caret in {focused_uri} and the keyboard up, the dock must offer that \
             block's operations; painted: {census}"
        );

        // The ENTITY tier must aim at the caret's row. The global tier
        // deliberately does not (it targets `navigation:main`), so it is
        // excluded here rather than silently widening the claim.
        let stray: Vec<(String, String)> = painted_op_buttons(&f.bounds)
            .into_iter()
            .filter(|(op, _, _)| !GLOBAL_OPS.contains(&op.as_str()))
            .filter(|(_, target, _)| *target != focused_uri)
            .map(|(op, target, _)| (op, target))
            .collect();
        assert!(
            stray.is_empty(),
            "every entity op the bar offers must target the focused block ({focused_uri}); these \
             do not: {stray:?}; painted: {census}"
        );
    });
}

/// The three app-level ops the bar's global tier offers. History and home have
/// no keyboard chord on a phone, which is why they earn a permanent slot.
const GLOBAL_OPS: [&str; 3] = ["go_back", "go_forward", "go_home"];

/// MARTIN'S ASK, second half: entity ops FIRST, then global ones.
///
/// `TargetScope` orders operations by narrowness (`Block < Page < Global`), and
/// the bar makes that order structural: what the thumb reaches first acts on
/// the block being edited, and app-level navigation sits past it. The boundary
/// is the claim — every entity op left of every global one — not merely that
/// both kinds appear.
#[test]
fn entity_ops_come_before_global_ops_in_the_bar() {
    with_action_bar_window(PHONE_WINDOW, |f| {
        f.focus_row(FOCUSED_ROW);
        f.raise_keyboard();

        let census = op_button_census(&f.bounds);
        let painted = painted_op_buttons(&f.bounds);
        let is_global = |op: &str| GLOBAL_OPS.contains(&op);

        let last_entity = painted.iter().rposition(|(op, _, _)| !is_global(op));
        let first_global = painted.iter().position(|(op, _, _)| is_global(op));

        let last_entity = last_entity.unwrap_or_else(|| {
            panic!("the bar must offer the focused block's own ops; painted: {census}")
        });
        let first_global = first_global.unwrap_or_else(|| {
            panic!(
                "the bar must offer the global tier ({GLOBAL_OPS:?}) after the entity ops; \
                 painted: {census}"
            )
        });

        assert!(
            last_entity < first_global,
            "every entity op must sit left of every global op: the last entity op is at index \
             {last_entity} and the first global one at {first_global}; painted: {census}"
        );
    });
}

/// The global tier does not depend on focus, so it must still be there with
/// nothing focused — otherwise the bar would flicker its whole contents in and
/// out as the caret moves, and "go back" would be unreachable exactly when a
/// user is lost.
///
/// Reachability note: the keyboard-up-with-no-focus state IS reachable here
/// because the inset is driven through `RebindHandle::set_safe_area_bottom`
/// rather than by focusing a row. On a real phone the keyboard is normally
/// raised BY focusing something, so this is the harness reaching a state the
/// device reaches only transiently (the caret leaving a row while the IME is
/// still up). The claim it pins — globals are not focus-derived — holds either
/// way.
#[test]
fn the_global_tier_renders_with_nothing_focused() {
    with_action_bar_window(PHONE_WINDOW, |f| {
        f.raise_keyboard();

        let census = op_button_census(&f.bounds);
        let painted = painted_op_buttons(&f.bounds);
        let globals: Vec<&String> = painted
            .iter()
            .map(|(op, _, _)| op)
            .filter(|op| GLOBAL_OPS.contains(&op.as_str()))
            .collect();

        assert_eq!(
            globals.len(),
            GLOBAL_OPS.len(),
            "with nothing focused the bar must still offer all {} global ops — they are app-level, \
             not derived from the caret; painted: {census}",
            GLOBAL_OPS.len()
        );

        let entity_ops: Vec<&String> = painted
            .iter()
            .map(|(op, _, _)| op)
            .filter(|op| !GLOBAL_OPS.contains(&op.as_str()))
            .collect();
        assert!(
            entity_ops.is_empty(),
            "with nothing focused there is no entity to act on, so the bar must offer no \
             entity ops; it offered {entity_ops:?}"
        );
    });
}

/// A global button must DISPATCH, not just paint. Tapping `go_home` clears the
/// main region's focused block, which is the effect the op exists to produce —
/// and the one that would silently not happen if the tap opened a region picker
/// instead (the navigation ops carry a `region` param that only their
/// `bound_params` satisfy).
#[test]
fn tapping_a_global_op_button_dispatches_it() {
    with_action_bar_window(PHONE_WINDOW, |f| {
        f.focus_row(FOCUSED_ROW);
        f.raise_keyboard();

        let census = op_button_census(&f.bounds);
        let button = painted_op_buttons(&f.bounds)
            .into_iter()
            .find(|(op, _, _)| op == "go_home")
            .unwrap_or_else(|| panic!("the bar must offer `go_home`; painted: {census}"))
            .2;

        click_at(f.app, f.window, press_point(&button), "go_home");
        f.settle();

        // A param popup means the tap resolved nothing and asked the user which
        // region they meant — the failure mode `bound_params` exists to prevent.
        let popup_open = f
            .bounds
            .all_elements()
            .into_iter()
            .any(|(_, i)| i.widget_type.as_ref() == "op_param_popup" && i.height > 0.0);
        assert!(
            !popup_open,
            "tapping `go_home` opened a param-collection popup: its `region` param was not \
             satisfied from the descriptor's bound_params, so the bar asked the user which \
             region they meant instead of just going home"
        );
    });
}

/// The bar is docked ABOVE THE KEYBOARD, so with the keyboard down there is
/// nothing to dock above. A permanently-present bar would spend a phone's
/// scarcest resource — vertical room — on affordances for a block the user is
/// not editing.
#[test]
fn with_the_keyboard_down_the_bar_is_not_painted() {
    with_action_bar_window(PHONE_WINDOW, |f| {
        f.focus_row(FOCUSED_ROW);
        // No `raise_keyboard()`: the inset stays at its desktop 0.0.

        let census = op_button_census(&f.bounds);
        assert!(
            painted_op_buttons(&f.bounds).is_empty(),
            "with the keyboard DOWN the action bar must not be painted — it is docked above the \
             keyboard, and there is no keyboard; painted: {census}"
        );
    });
}

/// A REAL PHONE never has a zero bottom inset. A home indicator, a nav bar or a
/// gesture area always eats the bottom strip, so a bar gated on "the bottom
/// inset is non-zero" is a bar that is permanently on screen — the
/// keyboard-down rung above passes only because a desktop window happens to
/// rest at 0.
///
/// This is the device condition: resting chrome present, keyboard down. The bar
/// must be absent, and must appear when the keyboard actually rises.
#[test]
fn resting_bottom_chrome_alone_does_not_raise_the_bar() {
    with_action_bar_window(PHONE_WINDOW, |f| {
        f.focus_row(FOCUSED_ROW);
        f.rest_with_bottom_chrome();

        let census = op_button_census(&f.bounds);
        assert!(
            painted_op_buttons(&f.bounds).is_empty(),
            "with {RESTING_INSET_PX}px of resting bottom chrome and the keyboard DOWN the bar \
             must not be painted — every real phone sits in this state all the time, so a bar \
             here is a bar that never goes away; painted: {census}"
        );

        // The control: the same window WITH the keyboard shows the bar, so the
        // absence above is the keyboard's doing and not a broken fixture.
        f.raise_keyboard();
        let census_up = op_button_census(&f.bounds);
        assert!(
            !painted_op_buttons(&f.bounds).is_empty(),
            "raising the keyboard from that same resting state must bring the bar up, else the \
             assertion above proves only that the bar is broken; painted: {census_up}"
        );
    });
}

/// A button that does not dispatch is a lie. Tapping `move_down` on the focused
/// row must reorder it past its sibling, observed as painted geometry (the row
/// the user was editing is now BELOW the one it was above), not as a click that
/// was merely received.
#[test]
fn tapping_a_dock_op_button_dispatches_the_operation() {
    with_action_bar_window(PHONE_WINDOW, |f| {
        f.focus_row(FOCUSED_ROW);
        f.raise_keyboard();

        let focused_uri = format!("block:{FOCUSED_ROW}");
        let sibling_uri = format!("block:{SIBLING_ROW}");

        let before_focused = entity_box(&f.bounds, &focused_uri)
            .unwrap_or_else(|| panic!("{focused_uri} must be painted before the tap"));
        let before_sibling = entity_box(&f.bounds, &sibling_uri)
            .unwrap_or_else(|| panic!("{sibling_uri} must be painted before the tap"));
        assert!(
            before_focused.y < before_sibling.y,
            "the fixture must start with {focused_uri} above {sibling_uri}, else moving it down \
             proves nothing: y={:.1} vs {:.1}",
            before_focused.y,
            before_sibling.y
        );

        let census = op_button_census(&f.bounds);
        assert!(
            painted_op_buttons(&f.bounds)
                .iter()
                .any(|(op, target, _)| op == "move_down" && *target == focused_uri),
            "the bar's `move_down` button must target the focused block ({focused_uri}); \
             painted: {census}"
        );
        let button = painted_op_buttons(&f.bounds)
            .into_iter()
            .find(|(op, _, _)| op == "move_down")
            .unwrap_or_else(|| {
                panic!(
                    "the bar must offer `move_down` for the focused block — it is a block-scoped \
                     reorder with no params beyond the target, the archetypal action-bar op; \
                     painted: {census}"
                )
            })
            .2;

        assert!(
            button.x + 4.0 < PHONE_WIDTH_PX,
            "`move_down` sits at x={:.1}, past the {PHONE_WIDTH_PX:.0}px viewport — it has \
             scrolled off the first screenful, so a tap here would land on nothing; painted: \
             {census}",
            button.x
        );
        let center = press_point(&button);
        click_at(f.app, f.window, center, "move_down");
        f.settle();

        let after_focused = entity_box(&f.bounds, &focused_uri)
            .unwrap_or_else(|| panic!("{focused_uri} must still be painted after the tap"));
        let after_sibling = entity_box(&f.bounds, &sibling_uri)
            .unwrap_or_else(|| panic!("{sibling_uri} must still be painted after the tap"));

        assert!(
            after_focused.y > after_sibling.y,
            "tapping `move_down` must move {focused_uri} below {sibling_uri}; it is still at \
             y={:.1} against the sibling's y={:.1}, so the tap reached the button but no \
             operation was dispatched",
            after_focused.y,
            after_sibling.y
        );
    });
}

/// The bar scrolls sideways; it must never grow downward or sit on top of the
/// content. Two things go wrong otherwise: the buttons wrap onto a second row
/// and eat the panel, or the dock and the page container BOTH apply the
/// keyboard inset and the bar is pushed a keyboard's height off the screen.
#[test]
fn the_dock_stays_one_row_below_the_main_panel() {
    with_action_bar_window(PHONE_WINDOW, |f| {
        f.focus_row(FOCUSED_ROW);
        f.raise_keyboard();

        let census = op_button_census(&f.bounds);
        let buttons = painted_op_buttons(&f.bounds);
        assert!(
            !buttons.is_empty(),
            "this rung judges the dock's geometry and needs a dock; painted: {census}"
        );

        // ONE ROW: every button shares a y band. A wrapped bar puts some
        // buttons a full button-height below the others.
        let top = buttons
            .iter()
            .map(|(_, _, i)| i.y)
            .fold(f32::INFINITY, f32::min);
        let bottom = buttons
            .iter()
            .map(|(_, _, i)| i.y + i.height)
            .fold(f32::NEG_INFINITY, f32::max);
        let tallest = buttons
            .iter()
            .map(|(_, _, i)| i.height)
            .fold(0.0f32, f32::max);
        assert!(
            bottom - top <= tallest * 1.5,
            "the action bar must stay ONE row — it scrolls horizontally instead of wrapping; its \
             buttons span {:.1}px against a tallest button of {tallest:.1}px; painted: {census}",
            bottom - top
        );

        // BELOW THE PANEL: the dock must not overlay the content it acts on.
        let panel = entity_box(&f.bounds, MAIN_PANEL)
            .unwrap_or_else(|| panic!("the main panel must be painted; painted: {census}"));
        assert!(
            top >= panel.y + panel.height - 1.0,
            "the action bar must sit BELOW the main panel, not over it: the bar's top is \
             {top:.1} while the panel runs to {:.1}",
            panel.y + panel.height
        );

        // VERTICAL BUDGET: the bar is the newest claimant on a phone's scarcest
        // resource, after the tab strip, the breadcrumb and the keyboard inset —
        // the same stack that once starved the panel to zero rows
        // (`block_focus_keeps_outline_windowed`). Adding a bar must not be what
        // tips it over.
        assert!(
            panel.height >= MIN_PANEL_BOX_PX,
            "with the action bar up the main panel is down to {:.1}px, under the {MIN_PANEL_BOX_PX:.1}px \
             three rows need — the chrome above it plus the bar has eaten the outline",
            panel.height
        );

        // ON SCREEN: a dock that applied the keyboard inset a second time (the
        // page container already did) lands a keyboard's height below the
        // viewport.
        let window_h = f
            .bounds
            .all_elements()
            .into_iter()
            .map(|(_, i)| i.y + i.height)
            .fold(0.0f32, f32::max);
        assert!(
            bottom <= window_h + 1.0,
            "the action bar is drawn at y={bottom:.1}, past the {window_h:.1}px the window has — \
             the keyboard inset is being applied twice, once by the page container and once by \
             the dock; painted: {census}"
        );
    });
}

/// NO REGRESSION on desktop: above the `if_space(600, …)` breakpoint the
/// synthesized layout takes a branch with no `bottom_dock` at all, so a desktop
/// window must paint no op affordance even with an inset set.
#[test]
fn a_desktop_window_paints_no_action_bar() {
    with_action_bar_window(DESKTOP_WINDOW, |f| {
        f.focus_row(FOCUSED_ROW);
        f.raise_keyboard();

        let census = op_button_census(&f.bounds);
        assert!(
            painted_op_buttons(&f.bounds).is_empty(),
            "a desktop-width window has no bottom dock branch, so it must paint no action-bar op \
             buttons; painted: {census}"
        );
    });
}
