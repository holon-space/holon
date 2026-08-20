//! Each integration row's op_buttons must read as belonging to THAT row — in a
//! REAL window over a REAL booted engine.
//!
//! The dogfood hazard (2026-08-20): the Settings → Integrations op_buttons sit
//! in a far-right column and drift vertically off their row's baseline, growing
//! down the list. A drifted button of one row creeps into the vertical band of
//! the NEXT row, so a click aimed at (say) gmail's `set_field` lands on
//! jsonplaceholder's — an unintended dispatch on the WRONG provider. The
//! bounds-driven windowed rungs never mis-click (they click a button's
//! registered centre), so the geometry escaped every existing assertion: this
//! rung supplies the missing oracle.
//!
//! The oracle joins each op_button to its own row through the row's
//! `state_toggle`, which `tag_node` binds to the row entity
//! (`vm_node.entity == "integration:<provider>"`). The toggle is one-per-row
//! and sits on the row's vertical centre, so it is the row's baseline. Two
//! things must hold for EVERY provider row:
//!   1. the op_button's vertical centre tracks its row's baseline (no drift);
//!   2. the row whose baseline is nearest the button is the button's OWN row —
//!      the direct formalization of "this click cannot land on another row".
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! settings_integrations_row_op_alignment_windowed -- --test-threads=1`
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

const SETTINGS_GEAR: &str = "settings-gear";

/// Every seeded integration renders a row, and `set_field` carries no guard, so
/// each row paints exactly this button. These are the rows the alignment
/// oracle sweeps.
const PROVIDERS: &[&str] = &[
    "gcal",
    "gmail",
    "todoist",
    "jsonplaceholder",
    "claude-history",
];

/// A drifted button belongs to its row while its centre stays within this many
/// pixels of the row's `state_toggle` baseline. Comfortably above sub-pixel
/// layout rounding of a correctly-centred row, far below the row pitch — so a
/// button that has drifted toward the next row is red.
const MAX_BASELINE_DRIFT_PX: f32 = 6.0;

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

/// Every element bound to a provider row (`vm_node.entity ==
/// "integration:<p>"`), with real on-screen extent. `tag_node` stamps that
/// entity on each row node, so this is the row's full painted contents.
fn row_elements(bounds: &BoundsRegistry, provider: &str) -> Vec<(String, ElementInfo)> {
    let entity = format!("integration:{provider}");
    bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| {
            info.has_visible_area()
                && info
                    .vm_node
                    .as_ref()
                    .is_some_and(|n| n.entity.as_deref() == Some(entity.as_str()))
        })
        .collect()
}

/// The row's label baseline: the leftmost painted, row-bound element that is
/// not itself an op affordance. That is the provider label / status text the
/// user reads the row by, and the line the op_buttons must sit on.
fn row_label(bounds: &BoundsRegistry, provider: &str) -> Option<ElementInfo> {
    row_elements(bounds, provider)
        .into_iter()
        .filter(|(_, info)| {
            info.vm_node
                .as_ref()
                .is_none_or(|n| n.tag.as_ref() != "op_button")
                && info.width > 0.0
        })
        .min_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap())
        .map(|(_, info)| info)
}

fn main_test(app: &mut HeadlessAppContext, window: gpui::AnyWindowHandle, bounds: &BoundsRegistry) {
    // Diagnostic first: the full painted contents of every provider row, so a
    // failure to anchor is legible rather than an opaque panic.
    let mut inventory = String::from("\n=== painted row-bound elements ===\n");
    for &p in PROVIDERS {
        inventory.push_str(&format!("--- integration:{p} ---\n"));
        let mut els = row_elements(bounds, p);
        els.sort_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap());
        for (id, info) in &els {
            let tag = info.vm_node.as_ref().map(|n| n.tag.as_ref()).unwrap_or("?");
            let (cx, cy) = info.center();
            inventory.push_str(&format!(
                "  tag={tag:<14} c=({cx:7.1},{cy:7.1}) wh=({:5.1},{:5.1}) id={id}\n",
                info.width, info.height,
            ));
        }
    }
    eprintln!("{inventory}");

    // Collect the geometry to judge. Each entry: (provider, op_button rect, that
    // row's label-baseline rect). The Settings integration list overdraws rows
    // just outside its viewport, which register with a degenerate (clipped) rect
    // (`ElementInfo::has_visible_area`); those rows cannot be clicked and carry
    // no meaningful geometry, so the oracle judges the rows actually on screen —
    // which include the multi-op rows where the drift is worst.
    let mut rows: Vec<(&str, ElementInfo, ElementInfo)> = Vec::new();
    for &p in PROVIDERS {
        let Some(btn) = bounds
            .element_info(&format!("op-button-set_field-integration:{p}"))
            .filter(ElementInfo::has_visible_area)
        else {
            continue;
        };
        let label = row_label(bounds, p).unwrap_or_else(|| {
            panic!("{p}'s row paints a visible op_button but no label element to anchor its baseline{inventory}")
        });
        rows.push((p, btn, label));
    }
    assert!(
        rows.len() >= 2,
        "expected at least two integration rows on screen to judge cross-row alignment; \
         found {}{inventory}",
        rows.len()
    );

    // The evidence a reader needs: the whole table of button vs. row baselines
    // and the resulting drift, printed whether the run is red or green.
    let mut table = String::from(
        "\nprovider          btn(cx,cy,w,h)                 label_cy     drift_dy    nearest_row\n",
    );
    for (p, btn, label) in &rows {
        let (bcx, bcy) = btn.center();
        let (_, tcy) = label.center();
        let nearest = rows
            .iter()
            .min_by(|a, b| {
                let da = (a.2.center().1 - bcy).abs();
                let db = (b.2.center().1 - bcy).abs();
                da.partial_cmp(&db).unwrap()
            })
            .map(|(np, _, _)| *np)
            .unwrap();
        table.push_str(&format!(
            "{p:<17} ({bcx:7.1},{bcy:7.1},{:5.1},{:5.1})   {tcy:7.1}   {:+8.1}    {nearest}\n",
            btn.width,
            btn.height,
            bcy - tcy,
        ));
    }
    eprintln!("{table}");

    // 1. No drift: each button's centre tracks its own row's baseline.
    for (p, btn, label) in &rows {
        let (_, bcy) = btn.center();
        let (_, tcy) = label.center();
        let drift = (bcy - tcy).abs();
        assert!(
            drift <= MAX_BASELINE_DRIFT_PX,
            "{p}'s set_field op_button drifts {drift:.1}px off its row's label baseline \
             (button centre_y {bcy:.1}, label baseline {tcy:.1}); a button that does not sit on \
             its row's line reads as unbound from the row and invites a mis-click. {table}"
        );
    }

    // 2. No mis-click: the row whose baseline is nearest each button is the
    // button's OWN row. This is the geometric form of "this click cannot land
    // on another provider".
    for (p, btn, _) in &rows {
        let (_, bcy) = btn.center();
        let nearest = rows
            .iter()
            .min_by(|a, b| {
                let da = (a.2.center().1 - bcy).abs();
                let db = (b.2.center().1 - bcy).abs();
                da.partial_cmp(&db).unwrap()
            })
            .map(|(np, _, _)| *np)
            .unwrap();
        assert_eq!(
            nearest, *p,
            "{p}'s set_field op_button is vertically nearer to {nearest}'s row baseline than to \
             its own — a click on it can land on the wrong provider. {table}"
        );
    }

    // Sanity: the buttons are to the RIGHT of their row label (the far-right
    // op column), so the oracle is judging the real column, not a degenerate
    // overlap.
    for (p, btn, label) in &rows {
        assert!(
            btn.x >= label.x,
            "{p}'s op_button ({:.1}) must sit at or right of its row label ({:.1})",
            btn.x,
            label.x
        );
    }

    let _ = (app, window);
}

#[test]
fn every_row_op_button_stays_on_its_own_row() {
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
                "Holon-RowOpAlignment-Windowed",
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

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        main_test(&mut app, window, &bounds);
    }));

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
