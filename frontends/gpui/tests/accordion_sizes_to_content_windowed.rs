//! Windowed PBT — an accordion holding a `live_query` takes only the height its
//! CONTENT needs, capped at `max_height_fraction`, and starts collapsed on a
//! phone-width panel.
//!
//! `accordion_bounded_pbt` builds the accordion's children as plain `text` VMs,
//! whose intrinsic height any container can measure. Production's child is a
//! `live_query` — a slot node whose `ReactiveShell` claims
//! `height: relative(1.0)` and so reports a height only against a definite
//! parent. These rungs drive that production shape (parsed DSL → query-source
//! switcher wrap → registered block tree → `columns`), fed by `TestServices`'
//! canned `watch_query_live`:
//!
//!   R1 empty:     zero query rows ⇒ region is the header row only.
//!   R2 few rows:  region grows with content and stays well under the cap.
//!   R3 many rows: region saturates AT `max_height_fraction × panel`.
//!   R4 phone:     below `ACCORDION_MIN_EXPANDED_WIDTH_PX` of available width
//!                 the accordion starts collapsed; above it, expanded.
//!   R5 survival:  a space change does not discard the reader's own expand.
//!
//! Run: `cargo nextest run -p holon-gpui --test
//! accordion_sizes_to_content_windowed`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::Size;
use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::render_dsl::parse_render_dsl;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::shadow_builders::ACCORDION_MIN_EXPANDED_WIDTH_PX;

const WINDOW_W: f32 = 1000.0;
const WINDOW_H: f32 = 900.0;
const FRACTION: f32 = 0.33;
const EPS: f32 = 2.0;
/// Rows in the outline stand-in above the footer — several viewports tall.
const OUTLINE_N: usize = 40;

/// The seed's main-panel shape, parameterized on the accordion's `collapsed`
/// prop. `assets/default/index.org` is pinned byte-for-byte by
/// `seeded_accordion_panel_smoke`; this mirrors its structure so the row count
/// and the collapse seed can be varied per rung.
fn panel_dsl() -> String {
    "column(\
       columns(#{item_template: live_block()}), \
       divider(), \
       accordion(#{title: \"Linked references\", icon: \"link\", max_height_fraction: 0.33}, \
         live_query(#{sql: \"SELECT * FROM backlinks\", item_template: text(col(\"content\"))})))"
        .to_string()
}

/// Replace the outline call — `columns(#{item_template: live_block()})`, which
/// production feeds from the focused root's children — with an empty `list()`
/// sentinel, populated at the VM level below.
fn substitute_outline(expr: RenderExpr) -> RenderExpr {
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            if name == "columns" {
                return RenderExpr::FunctionCall {
                    name: "list".to_string(),
                    args: vec![],
                };
            }
            RenderExpr::FunctionCall {
                name,
                args: args
                    .into_iter()
                    .map(|a| Arg {
                        name: a.name,
                        value: substitute_outline(a.value),
                    })
                    .collect(),
            }
        }
        RenderExpr::Array { items } => RenderExpr::Array {
            items: items.into_iter().map(substitute_outline).collect(),
        },
        RenderExpr::Object { fields } => RenderExpr::Object {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, substitute_outline(v)))
                .collect(),
        },
        other => other,
    }
}

fn outline_row(ix: usize) -> ReactiveViewModel {
    let label = format!("outline {ix}");
    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(format!("outline-{ix}")));
    data.insert("content".to_string(), Value::String(label.clone()));
    let mut props = HashMap::new();
    props.insert("content".to_string(), Value::String(label));
    props.insert("field".to_string(), Value::String("content".to_string()));
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(Arc::new(data)).read_only();
    vm
}

fn outline_collection() -> ReactiveViewModel {
    let items: Vec<ReactiveViewModel> = (0..OUTLINE_N).map(outline_row).collect();
    ReactiveViewModel {
        collection: Some(Arc::new(ReactiveView::new_static_with_layout(
            items,
            CollectionVariant::list(0.0),
        ))),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    }
}

struct Rung {
    /// Height of the tagged accordion footer region.
    region_h: f32,
    /// Backlink rows the canned watcher produced that are actually painted.
    visible_rows: usize,
}

/// The panel ViewModel, built the way production builds it: the shell publishes
/// a viewport, and the root `RenderContext` takes its `available_space` from
/// `services.viewport_snapshot()` — `interpret_pure`'s own body
/// (`reactive.rs`). Nothing here hands `available_space` to the accordion
/// directly, and the snapshot is asserted present, so a seam that stops
/// supplying it reds these rungs instead of silently falling back to the
/// desktop-first default.
fn interpret_panel(
    services: &Arc<support::TestServices>,
    window: Size<gpui::Pixels>,
) -> ReactiveViewModel {
    use holon_frontend::reactive::BuilderServices;

    services.set_viewport_size(f32::from(window.width), f32::from(window.height));
    let space = services
        .viewport_snapshot()
        .expect("the published viewport must reach the interpreter through viewport_snapshot");
    assert_eq!(space.width_px, f32::from(window.width));

    let panel_expr = holon::api::block_domain::BlockDomain::wrap_in_query_source_switcher(
        &holon_api::EntityUri::block("default-main-panel"),
        substitute_outline(parse_render_dsl(&panel_dsl()).expect("panel DSL must parse")),
        "SELECT * FROM blocks",
        holon_api::QueryLanguage::HolonSql,
    );
    let ctx = RenderContext {
        available_space: Some(space),
        ..Default::default()
    };
    let interp = holon_frontend::shadow_builders::build_shadow_interpreter();
    let panel_vm = interp.interpret(&panel_expr, &ctx, &**services);

    {
        let slot = panel_vm.slot.as_ref().expect("the wrap gives it a slot");
        let mut content = slot.content.lock_mut();
        let column = Arc::get_mut(&mut content)
            .expect("slot content is uniquely held immediately after interpret");
        let sentinel = column
            .children
            .iter()
            .position(|c| c.widget_name().as_deref() == Some("list"))
            .expect("substituted outline -> `list` sentinel must be a direct column child");
        column.children[sentinel] = Arc::new(outline_collection());
    }
    panel_vm
}

/// The accordion node inside an interpreted panel — the node whose `expanded`
/// Mutable carries the collapse state a click flips.
fn accordion_of(panel_vm: &ReactiveViewModel) -> Arc<ReactiveViewModel> {
    let content = panel_vm
        .slot
        .as_ref()
        .expect("the wrap gives it a slot")
        .content
        .get_cloned();
    content
        .children
        .iter()
        .find(|c| c.widget_name().as_deref() == Some("accordion"))
        .expect("the panel column must hold the accordion")
        .clone()
}

/// Render the production-shaped main panel with `backlink_rows` query rows at
/// `window`.
fn render_panel(cx: &mut TestAppContext, backlink_rows: usize, window: Size<gpui::Pixels>) -> Rung {
    cx.update(|cx| gpui_component::init(cx));

    let registry = Arc::new(support::BlockTreeRegistry::new());
    let services = support::TestServices::with_registry_quiescent(registry.clone());
    services.set_live_query_rows(backlink_rows);
    let panel_vm = interpret_panel(&services, window);
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = services;

    let column_slot = std::sync::Mutex::new(Some(panel_vm));
    let thunk: support::BlockTreeThunk = Arc::new(move || {
        column_slot
            .lock()
            .unwrap()
            .take()
            .expect("watch_live called more than once")
    });
    registry.register(
        "block:default-main-panel",
        vec![("default".to_string(), thunk)],
        0,
    );
    let root = Arc::new(ReactiveViewModel {
        collection: Some(Arc::new(ReactiveView::new_static_with_layout(
            vec![ReactiveViewModel::live_block(holon_api::EntityUri::block(
                "default-main-panel",
            ))],
            CollectionVariant::columns(4.0),
        ))),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    });

    let snap =
        support::render_reactive_fixture_quiescent_sized_with_services(cx, root, window, services);

    let region_h = snap
        .of_type("accordion")
        .map(|i| i.height)
        .fold(0.0f32, f32::max);
    let visible_rows = snap
        .of_type("text")
        .filter(|i| {
            i.height > 0.0
                && i.entity_id
                    .as_deref()
                    .is_some_and(|e| e.starts_with("backlink-"))
        })
        .count();
    Rung {
        region_h,
        visible_rows,
    }
}

fn desktop() -> Size<gpui::Pixels> {
    size(px(WINDOW_W), px(WINDOW_H))
}

/// Martin's DN2103 in portrait — the geometry
/// `block_focus_keeps_outline_windowed` pins as the device viewport.
fn phone_portrait() -> Size<gpui::Pixels> {
    size(px(393.0), px(852.0))
}

#[gpui::test]
fn empty_accordion_is_header_height_only(cx: &mut TestAppContext) {
    let empty = render_panel(cx, 0, desktop());
    let few = render_panel(cx, 3, desktop());
    let cap = FRACTION * WINDOW_H;

    assert!(
        empty.region_h > 0.0,
        "R1 setup: the accordion footer must render its header"
    );
    assert_eq!(
        empty.visible_rows, 0,
        "R1 setup: an empty query yields no backlink rows"
    );
    assert!(
        empty.region_h < few.region_h,
        "R1 content-sized VIOLATED: an EMPTY accordion is {} tall, no shorter than the \
         3-row one ({}) - the region is fixed at its cap ({cap}) instead of sizing to \
         its content, so an empty Linked-references section reserves space it does \
         not need",
        empty.region_h,
        few.region_h
    );
}

#[gpui::test]
fn few_rows_size_below_the_cap(cx: &mut TestAppContext) {
    let few = render_panel(cx, 3, desktop());
    let cap = FRACTION * WINDOW_H;

    assert_eq!(
        few.visible_rows, 3,
        "R2 setup: the canned watcher must paint all 3 backlink rows"
    );
    assert!(
        few.region_h < cap * 0.75,
        "R2 content-sized VIOLATED: a 3-row accordion is {} tall against a cap of {cap} \
         - it inflated toward the cap instead of shrinking to its content",
        few.region_h
    );
}

#[gpui::test]
fn many_rows_saturate_the_cap(cx: &mut TestAppContext) {
    let many = render_panel(cx, 200, desktop());
    let cap = FRACTION * WINDOW_H;

    assert!(
        (many.region_h - cap).abs() <= EPS,
        "R3 capped VIOLATED: a 200-row accordion is {} tall; it must saturate AT the \
         cap {cap} (0.33 x {WINDOW_H})",
        many.region_h
    );
}

#[gpui::test]
fn a_phone_width_panel_starts_collapsed(cx: &mut TestAppContext) {
    let phone = render_panel(cx, 40, phone_portrait());
    let wide = render_panel(cx, 40, desktop());

    assert!(
        f32::from(phone_portrait().width) < ACCORDION_MIN_EXPANDED_WIDTH_PX
            && f32::from(desktop().width) >= ACCORDION_MIN_EXPANDED_WIDTH_PX,
        "R4 setup: the two fixtures must straddle {ACCORDION_MIN_EXPANDED_WIDTH_PX}px"
    );
    assert!(
        wide.visible_rows > 0,
        "R4 control: at {}px wide (>= {ACCORDION_MIN_EXPANDED_WIDTH_PX}) the accordion \
         must start EXPANDED and paint backlink rows",
        f32::from(desktop().width)
    );
    assert_eq!(
        phone.visible_rows,
        0,
        "R4 default-collapsed VIOLATED: at {}px wide (< {ACCORDION_MIN_EXPANDED_WIDTH_PX}) \
         the accordion must start COLLAPSED, but it painted {} backlink rows",
        f32::from(phone_portrait().width),
        phone.visible_rows
    );
    assert!(
        phone.region_h < wide.region_h,
        "R4 default-collapsed VIOLATED: the collapsed footer ({}) must be shorter than \
         the expanded one ({}) - collapsing must free the body's height",
        phone.region_h,
        wide.region_h
    );
}

/// A rotation or resize re-interprets the tree against the fresh viewport. The
/// width-derived default seeds each FRESH node, so on a panel that is still
/// narrow it would collapse again — the reader's own expand has to win.
#[gpui::test]
fn a_space_change_keeps_the_readers_choice(_: &mut TestAppContext) {
    let registry = Arc::new(support::BlockTreeRegistry::new());
    let services = support::TestServices::with_registry_quiescent(registry);

    let mounted = interpret_panel(&services, phone_portrait());
    let expanded = accordion_of(&mounted)
        .expanded
        .as_ref()
        .expect("a collapsible accordion carries an `expanded` handle")
        .clone();
    assert!(
        !expanded.get(),
        "R5 setup: at phone width the accordion starts collapsed"
    );

    // The reader opens it, then the panel resizes — still narrow, so the fresh
    // default is collapsed and has something to overwrite.
    expanded.set(true);
    let rebuilt = interpret_panel(&services, size(px(412.0), px(915.0)));
    assert!(
        !accordion_of(&rebuilt)
            .expanded
            .as_ref()
            .expect("gate")
            .get(),
        "R5 setup: the rebuilt tree must default to COLLAPSED, otherwise this rung \
         cannot tell a preserved choice from a fresh default"
    );

    let reconciled = mounted.with_update(&rebuilt);
    assert!(
        accordion_of(&reconciled)
            .expanded
            .as_ref()
            .expect("the reconciled accordion keeps its `expanded` handle")
            .get(),
        "R5 survival VIOLATED: the reader expanded the accordion, then a resize \
         re-interpreted the tree and it snapped back to collapsed - the rebuild adopted \
         the fresh default instead of keeping the mounted node's state"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
