//! Inc C windowed dedicated PBT — the sticky / in-flow accordion variants.
//!
//! Drives a PRODUCTION-shaped `section_stack( <content section>, accordion(
//! sticky, …) )` through the real `builders::render` pipeline over a
//! definite-height window, then evaluates the SHARED observational spec
//! (`holon_frontend::sticky_accordion` checkers — the same pure functions the
//! shared-catalog PBT invariant bodies wrap, so promotion to the keystone is a
//! move, not a rewrite):
//!
//!   position-spec, exactly-one-footer, no-overlap, cap-under-sticky,
//!   overlay-bounds-committed, settle-stability.
//!
//! History: with the sticky overlay stubbed (feature missing) every footer
//! checker went red — no `accordion_sticky_footer` element committed
//! (red-run-stickyimpl.log). `render_sticky` (absolute `.occlude()` + px-cap +
//! definite-height fail-loud) turns them green.
//!
//! Run: `cargo test -p holon-gpui --test sticky_accordion_pbt`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::Entity;
use gpui::Size;
use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::sticky_accordion as sa;
use holon_gpui::geometry::BoundsRegistry;
use support::ReactiveFixtureView;

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

/// `section_stack( column(<content rows>), accordion(sticky, <footer rows>) )`.
/// `footer_rows` tall enough to exceed the cap so the px-cap engages.
fn sticky_root(content_rows: usize, footer_rows: usize, fraction: f64) -> Arc<ReactiveViewModel> {
    let content: Vec<Arc<ReactiveViewModel>> = (0..content_rows)
        .map(|i| Arc::new(text_item(format!("crow-{i}"), format!("content row {i}"))))
        .collect();
    let mut content_col = ReactiveViewModel::from_widget("column", HashMap::new());
    content_col.children = content;

    let footer_children: Vec<Arc<ReactiveViewModel>> = (0..footer_rows)
        .map(|i| Arc::new(text_item(format!("frow-{i}"), format!("footer row {i}"))))
        .collect();
    let mut acc_props = HashMap::new();
    acc_props.insert("title".to_string(), Value::String("Sticky footer".into()));
    acc_props.insert("max_height_fraction".to_string(), Value::Float(fraction));
    acc_props.insert("collapsible".to_string(), Value::Boolean(true));
    acc_props.insert("collapsed".to_string(), Value::Boolean(false));
    acc_props.insert("placement".to_string(), Value::String("sticky".into()));
    let accordion = Arc::new(ReactiveViewModel {
        children: footer_children,
        expanded: Some(Mutable::new(true)),
        ..ReactiveViewModel::from_widget("accordion", acc_props)
    });

    let mut props = HashMap::new();
    props.insert("section_stack".to_string(), Value::Boolean(true));
    let mut ss = ReactiveViewModel::from_widget("section_stack", props);
    ss.children = vec![Arc::new(content_col), accordion];
    Arc::new(ss)
}

fn observe(bounds: &BoundsRegistry) -> Vec<sa::ObservedRect> {
    bounds
        .all_elements()
        .into_iter()
        .map(|(_, i)| sa::ObservedRect {
            widget_type: i.widget_type.to_string(),
            entity_id: i.entity_id.as_deref().map(str::to_string),
            x: i.x,
            y: i.y,
            w: i.width,
            h: i.height,
        })
        .collect()
}

fn settle(
    entity: &Entity<ReactiveFixtureView>,
    vcx: &mut VisualTestContext,
    bounds: &BoundsRegistry,
) {
    for _ in 0..4 {
        entity.update(&mut vcx.cx.clone(), |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    bounds.flush();
}

fn open_stack<'a>(
    cx: &'a mut TestAppContext,
    vm: Arc<ReactiveViewModel>,
    viewport: Size<gpui::Pixels>,
) -> (
    Entity<ReactiveFixtureView>,
    &'a mut VisualTestContext,
    BoundsRegistry,
) {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let (entity, vcx) = cx.add_window_view({
        let (v, b) = (vm.clone(), bounds.clone());
        move |_, _| ReactiveFixtureView::with_bounds(v, viewport, b)
    });
    vcx.run_until_parked();
    settle(&entity, vcx, &bounds);
    (entity, vcx, bounds)
}

#[gpui::test]
fn sticky_accordion_overlay_holds_the_spec(cx: &mut TestAppContext) {
    let fraction = 0.4_f64;
    let viewport = size(px(420.0), px(320.0));
    let (entity, vcx, bounds) = open_stack(cx, sticky_root(12, 16, fraction), viewport);

    let snap_a = observe(&bounds);
    settle(&entity, vcx, &bounds);
    let snap_b = observe(&bounds);

    let mut failures = sa::check_all_single(&snap_a, fraction as f32);
    if let Err(e) = sa::check_settle_stability(&snap_a, &snap_b) {
        failures.push(e);
    }

    if !failures.is_empty() {
        eprintln!("[sticky_accordion_pbt] {} spec failure(s):", failures.len());
        for f in &failures {
            eprintln!("  RED {f}");
        }
    }
    assert!(
        failures.is_empty(),
        "sticky accordion spec RED: {} failure(s) (see stderr)",
        failures.len()
    );
    eprintln!("[sticky_accordion_pbt] GREEN — all 6 observational checks pass");
}

#[gpui::test]
fn in_flow_accordion_renders_inline_not_error(cx: &mut TestAppContext) {
    // pinned:false in-flow accordion inside a section stack renders a real
    // capped region, NOT the placement error widget.
    let viewport = size(px(420.0), px(320.0));
    let mut acc_props = HashMap::new();
    acc_props.insert("title".to_string(), Value::String("In-flow".into()));
    acc_props.insert("max_height_fraction".to_string(), Value::Float(0.4));
    acc_props.insert("placement".to_string(), Value::String("in_flow".into()));
    let accordion = Arc::new(ReactiveViewModel {
        children: vec![Arc::new(text_item("if-0".into(), "row 0".into()))],
        expanded: Some(Mutable::new(true)),
        ..ReactiveViewModel::from_widget("accordion", acc_props)
    });
    let mut props = HashMap::new();
    props.insert("section_stack".to_string(), Value::Boolean(true));
    let mut ss = ReactiveViewModel::from_widget("section_stack", props);
    ss.children = vec![accordion];
    let (_e, _vcx, bounds) = open_stack(cx, Arc::new(ss), viewport);

    let obs = observe(&bounds);
    let has_error = obs
        .iter()
        .any(|o| o.widget_type == "error" || o.widget_type == "accordion");
    // The in-flow accordion renders as a tracked section, never the misplaced
    // "accordion" error path.
    assert!(
        !has_error,
        "in-flow accordion rendered an error/misplaced widget: {:?}",
        obs.iter().map(|o| &o.widget_type).collect::<Vec<_>>()
    );
}
