//! Inc 1 windowed PBT (backlinks-bounded-region plan §5) — RED-for-the-right-
//! reason. Drives the PRODUCTION-shaped
//! `columns( column( outline-collection, divider, accordion(backlinks) ) )`
//! through the real `columns::render` / `column::render` path (via
//! `ReactiveFixtureView`) and checks the bounded-region invariants:
//!
//!   I1 (bounded): the `accordion#seq` region's visible height ≤
//!       `max_height_fraction × viewport + ε`, for EVERY generated case.
//!   I2 (shrink-to-content): a nearly-empty accordion under a generous cap
//!       does NOT inflate to the cap — it shrinks to its content.
//!
//! In Inc 1 the accordion gpui builder is an inert passthrough (header +
//! children, NO cap). So for small-outline / large-backlink cases the region's
//! visible height is the whole area below the outline — far larger than the
//! cap — and **I1 fails as an ASSERTION** (not a missing symbol: the widget
//! parses and renders; only the bounding is unimplemented). That is the
//! captured RED. Inc 2 (the flow-panel split + capped body) turns it GREEN.
//!
//! The case matrix is the plan's explicit generator space enumerated
//! exhaustively (stronger than sampling over these finite sets).
//!
//! Run: `cargo test -p holon-gpui --test accordion_bounded_pbt`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::Size;
use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::geometry::BoundsRegistry;
use support::ReactiveFixtureView;

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
/// migrated seed shape (plan §2 example), built as a VM tree so the accordion
/// is a legitimately-placed direct column child (bypasses the DSL placement
/// guard on purpose: the bounding, not the placement, is what's under test).
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

fn accordion_visible_height(bounds: &BoundsRegistry) -> Option<f32> {
    bounds
        .all_elements()
        .iter()
        .find(|(_, info)| info.widget_type.as_ref() == "accordion")
        .map(|(_, info)| info.height)
}

fn measure(
    cx: &mut TestAppContext,
    outline_n: usize,
    backlink_n: usize,
    fraction: f64,
    viewport: Size<gpui::Pixels>,
) -> f32 {
    let bounds = BoundsRegistry::new();
    let root = root(outline_n, backlink_n, fraction);
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, viewport, bounds)
    });
    vcx.run_until_parked();
    bounds.flush();
    accordion_visible_height(&bounds).expect("accordion region must be tagged in BoundsRegistry")
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

                    // I1 — the region must never exceed the fraction cap.
                    assert!(
                        h <= cap + EPS,
                        "I1 bounded VIOLATED: accordion region visible height {h} > \
                         cap {cap} (fraction {fraction} × viewport {vh}) for \
                         outline={outline_n} backlinks={backlink_n}. The accordion \
                         is not bounded — the flow-panel split + capped body \
                         (Inc 2) is unimplemented, so the region takes all space \
                         below the outline."
                    );

                    // I2 — shrink-to-content: a near-empty accordion under a
                    // generous (full-viewport) cap must not inflate to the cap.
                    if fraction == 1.0 && backlink_n <= 1 && outline_n <= 3 {
                        assert!(
                            h < vh * 0.5,
                            "I2 shrink-to-content VIOLATED: near-empty accordion \
                             height {h} inflated toward the cap (viewport {vh}) \
                             instead of shrinking to its content"
                        );
                    }
                }
            }
        }
    }
}
