//! Inc 2 windowed PBT (backlinks-bounded-region plan §5) — the bounded-footer
//! invariants, now GREEN. Drives the PRODUCTION-shaped
//! `columns( column( outline-collection, divider, accordion(backlinks) ) )`
//! through the real `columns::render` / `column::render` accordion split (R8:
//! pinned footer) via `ReactiveFixtureView`:
//!
//!   I1 (bounded): the `accordion#seq` region's visible height ≤
//!       `max_height_fraction × viewport + ε`, for EVERY case.
//!   I2 (shrink-to-content): a near-empty accordion under a generous cap does
//!       NOT inflate to the cap — it shrinks to its content.
//!   I4 (aux reachability): a below-cap backlink row gains nonzero visible
//!       height after a wheel over the accordion body.
//!   I5 (main scroll preserved): a far outline row is reachable by a wheel over
//!       the main region — the outline scrolls INDEPENDENTLY of the pinned
//!       accordion footer.
//!
//! History: in Inc 1 (inert passthrough, no cap) I1 failed as an assertion
//! (red-run-inc1.log). Inc 2's flow-panel split + capped body turns it green.
//!
//! Run: `cargo test -p holon-gpui --test accordion_bounded_pbt`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::MouseMoveEvent;
use gpui::MouseUpEvent;
use gpui::Size;
use gpui::TestAppContext;
use gpui::point;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_frontend::LayoutHint;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::geometry::BoundsRegistry;
use support::ReactiveFixtureView;
use support::simulate_wheel_at;

const EPS: f32 = 1.5;

fn outline_id(ix: usize) -> String {
    format!("outline-{ix}")
}
fn backlink_id(ix: usize) -> String {
    format!("backlink-{ix}")
}

/// Production-faithful `text` row (see `builders/text.rs`): `data.id` +
/// `props.field == "content"` makes its bounds appear keyed by `entity_id`.
fn text_item(id: String, label: String) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".into(), Value::String(id));
    data.insert("content".into(), Value::String(label.clone()));

    let mut props = HashMap::new();
    props.insert("content".into(), Value::String(label));
    props.insert("field".into(), Value::String("content".into()));

    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = Mutable::new(Arc::new(data)).read_only();
    vm
}

/// `columns( column( <outline list>, divider, accordion(<backlinks>) ) )` — the
/// migrated seed shape (plan §2), built as a VM tree so the accordion is a
/// legitimately-placed direct column child.
fn root(outline_n: usize, backlink_n: usize, fraction: f64) -> Arc<ReactiveViewModel> {
    let outline_items: Vec<ReactiveViewModel> = (0..outline_n)
        .map(|i| text_item(outline_id(i), format!("outline {i}")))
        .collect();
    let outline_view = Arc::new(ReactiveView::new_static_with_layout(
        outline_items,
        CollectionVariant::list(0.0),
    ));
    let outline_child = Arc::new(ReactiveViewModel {
        collection: Some(outline_view),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    });

    let divider = Arc::new(ReactiveViewModel::from_widget("divider", HashMap::new()));

    let backlink_rows: Vec<Arc<ReactiveViewModel>> = (0..backlink_n)
        .map(|i| Arc::new(text_item(backlink_id(i), format!("backlink {i}"))))
        .collect();
    let mut acc_props = HashMap::new();
    acc_props.insert(
        "title".to_string(),
        Value::String("Linked references".into()),
    );
    acc_props.insert("max_height_fraction".to_string(), Value::Float(fraction));
    acc_props.insert("collapsible".to_string(), Value::Boolean(true));
    acc_props.insert("collapsed".to_string(), Value::Boolean(false));
    let accordion = Arc::new(ReactiveViewModel {
        children: backlink_rows,
        expanded: Some(Mutable::new(true)),
        // What the shadow builder stamps on a `pinned` accordion — the
        // declaration the split reads.
        layout_hint: LayoutHint::PinnedToEnd,
        ..ReactiveViewModel::from_widget("accordion", acc_props)
    });

    let mut column = ReactiveViewModel::from_widget("column", HashMap::new());
    column.children = vec![outline_child, divider, accordion];

    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![column],
        CollectionVariant::columns(4.0),
    ));
    Arc::new(ReactiveViewModel {
        collection: Some(columns_view),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    })
}

fn visible_height(bounds: &BoundsRegistry, entity_id: &str) -> Option<f32> {
    bounds
        .all_elements()
        .iter()
        .find(|(_, info)| info.entity_id.as_deref() == Some(entity_id))
        .map(|(_, info)| info.height)
}

/// Accordion region height for one case. Opens a window sized to `viewport`
/// (a DEFINITE-height parent) so the accordion's `max_h(relative(f))` resolves
/// against `viewport` — the production-faithful reference (a real window has a
/// definite height; `add_window_view` uses a fixed default that would swamp the
/// chosen fraction).
fn measure(
    cx: &mut TestAppContext,
    outline_n: usize,
    backlink_n: usize,
    fraction: f64,
    viewport: Size<gpui::Pixels>,
) -> f32 {
    let snap =
        support::render_reactive_fixture_sized(cx, root(outline_n, backlink_n, fraction), viewport);
    let h = snap
        .of_type("accordion")
        .next()
        .map(|info| info.height)
        .expect("accordion region must be tagged");
    h
}

#[gpui::test]
fn accordion_region_is_bounded(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    // The plan's explicit generator space (§5), enumerated exhaustively.
    let outlines = [0usize, 3, 80];
    let backlinks = [0usize, 1, 5, 200];
    let fractions = [0.1f64, 0.33, 1.0];
    let viewports = [
        size(px(400.0), px(300.0)),
        size(px(600.0), px(400.0)),
        size(px(1200.0), px(900.0)),
    ];

    for &vp in &viewports {
        let vh = f32::from(vp.height);
        for &fraction in &fractions {
            let cap = fraction as f32 * vh;
            for &outline_n in &outlines {
                for &backlink_n in &backlinks {
                    let h = measure(cx, outline_n, backlink_n, fraction, vp);

                    // I1 — the region never exceeds the fraction cap.
                    assert!(
                        h <= cap + EPS,
                        "I1 bounded VIOLATED: accordion region visible height {h} > \
                         cap {cap} (fraction {fraction} × viewport {vh}) for \
                         outline={outline_n} backlinks={backlink_n}"
                    );

                    // I2 — shrink-to-content: a near-empty accordion under a
                    // generous (full-viewport) cap must not inflate to the cap.
                    if fraction == 1.0 && backlink_n <= 1 {
                        assert!(
                            h < vh * 0.5,
                            "I2 shrink-to-content VIOLATED: near-empty accordion \
                             height {h} inflated toward the cap (viewport {vh}) \
                             instead of shrinking to content (outline={outline_n})"
                        );
                    }
                }
            }
        }
    }
}

#[gpui::test]
fn wheel_over_accordion_body_reveals_backlink(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    // fraction 1.0: the accordion body is (about) the full viewport; the last
    // backlink row starts far below the body fold. outline=0 so the body owns
    // the whole panel (structurally like `main_panel_scroll`).
    let vp = size(px(600.0), px(400.0));
    let backlinks = 80usize;
    let far = backlinks - 1;

    let bounds = BoundsRegistry::new();
    let root = root(0, backlinks, 1.0);
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, vp, bounds)
    });
    vcx.run_until_parked();
    bounds.flush();

    let before = visible_height(&bounds, &backlink_id(far)).unwrap_or(0.0);
    assert!(
        before <= 0.0,
        "I4 setup: backlink {far} should start below the accordion body fold \
         (got {before})"
    );

    // Wheel over the accordion body centre (~viewport centre when it fills).
    simulate_wheel_at(vcx, point(px(300.0), px(220.0)), px(-100000.0));
    vcx.run_until_parked();
    bounds.flush();

    let after = visible_height(&bounds, &backlink_id(far)).unwrap_or(0.0);
    assert!(
        after > 0.0,
        "I4 aux reachability VIOLATED: after a wheel over the capped accordion \
         body, backlink {far} should scroll into view (nonzero visible height) \
         but was still clipped ({after}). The body's `min_h_0 + overflow_y_scroll` \
         internal viewport is not scrolling."
    );
}

#[gpui::test]
fn wheel_over_main_region_reveals_outline(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    // fraction 0.1: the accordion is a tiny pinned footer; the main region owns
    // (almost) the whole panel and must still scroll the outline independently.
    let vp = size(px(600.0), px(400.0));
    let outlines = 80usize;
    let far = outlines - 1;

    let bounds = BoundsRegistry::new();
    let root = root(outlines, 1, 0.1);
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, vp, bounds)
    });
    vcx.run_until_parked();
    bounds.flush();

    let before = visible_height(&bounds, &outline_id(far)).unwrap_or(0.0);
    assert!(
        before <= 0.0,
        "I5 setup: outline {far} should start below the main-region fold (got {before})"
    );

    // Wheel over the main region centre (well above the tiny footer).
    simulate_wheel_at(vcx, point(px(300.0), px(160.0)), px(-100000.0));
    vcx.run_until_parked();
    bounds.flush();

    let after = visible_height(&bounds, &outline_id(far)).unwrap_or(0.0);
    assert!(
        after > 0.0,
        "I5 main scroll VIOLATED: after a wheel over the main region, outline \
         {far} should scroll into view but was still clipped ({after}). The \
         accordion split's main region is not an independent scroll viewport."
    );
}

/// Left-click at `pos` (MouseMove → MouseDown → MouseUp), mirroring
/// `layout_editor::click`.
fn click_at(vcx: &mut gpui::VisualTestContext, pos: gpui::Point<gpui::Pixels>) {
    vcx.simulate_event(MouseMoveEvent {
        position: pos,
        ..Default::default()
    });
    vcx.simulate_event(MouseDownEvent {
        position: pos,
        button: MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent {
        position: pos,
        button: MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
    });
}

fn header_center(bounds: &BoundsRegistry) -> gpui::Point<gpui::Pixels> {
    // The header sits at the top of the tagged `accordion` region.
    let (_, info) = bounds
        .all_elements()
        .into_iter()
        .find(|(_, info)| info.widget_type.as_ref() == "accordion")
        .expect("accordion region must be tagged");
    point(px(info.x + info.width / 2.0), px(info.y + 8.0))
}

#[gpui::test]
fn clicking_header_collapses_and_frees_height(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    // fraction 1.0, outline 0: the accordion fills the panel and its body shows
    // the first backlink rows — so `backlink 0` is visible before collapse.
    let vp = size(px(600.0), px(400.0));
    let backlinks = 80usize;

    let bounds = BoundsRegistry::new();
    let root = root(0, backlinks, 1.0);
    let (_e, mut vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, vp, bounds)
    });
    vcx.run_until_parked();
    bounds.flush();

    let region_before = accordion_visible_height(&bounds).expect("accordion tagged");
    let backlink_before = visible_height(&bounds, &backlink_id(0)).unwrap_or(0.0);
    assert!(
        backlink_before > 0.0,
        "I3 setup: backlink 0 should be visible while expanded (got {backlink_before})"
    );

    // Click the header to collapse.
    let target = header_center(&bounds);
    click_at(&mut vcx, target);
    vcx.run_until_parked();
    bounds.flush();

    let backlink_after = visible_height(&bounds, &backlink_id(0)).unwrap_or(0.0);
    let region_after = accordion_visible_height(&bounds).expect("accordion tagged");
    assert!(
        backlink_after <= 0.0,
        "I3 collapse VIOLATED: after clicking the header the backlink body should \
         be unrendered (backlink 0 visible height 0) but it was still {backlink_after}. \
         The header chevron is inert — its click does not flip the `expanded` Mutable."
    );
    assert!(
        region_after < region_before,
        "I3 collapse VIOLATED: collapsing should free height — accordion region \
         shrank from {region_before} to {region_after} (expected strictly smaller)"
    );
}

fn accordion_visible_height(bounds: &BoundsRegistry) -> Option<f32> {
    bounds
        .all_elements()
        .iter()
        .find(|(_, info)| info.widget_type.as_ref() == "accordion")
        .map(|(_, info)| info.height)
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
