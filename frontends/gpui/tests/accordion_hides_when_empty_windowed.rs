//! Windowed PBT — an accordion carrying `hide_when_empty` paints NOTHING while
//! its content has no rows, and comes back the moment a row arrives.
//!
//!   R1 hidden:    zero backlink rows ⇒ no accordion region, no reserved space.
//!   R2 appears:   a row pushed into the section's collection paints the
//!                 region again, with that row in it.
//!   R3 hides:     removing the last row hides it again, and the reader's
//!                 expand state survives the hidden interval.
//!   R4 opt-in:    an accordion WITHOUT the flag keeps its title row.
//!
//! R1/R4 drive the seeded shape (parsed DSL → query-source switcher wrap →
//! registered block tree → `columns`) whose rows come from a `live_query`
//! shell. R2/R3 hold ONE window open and move rows through the section
//! collection's `MutableVec` — the seam the streaming driver writes to — so the
//! transitions are observed live rather than across two fresh renders.
//!
//! Run: `cargo nextest run -p holon-gpui --test
//! accordion_hides_when_empty_windowed`

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
use holon_api::render_dsl::parse_render_dsl;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::LayoutHint;
use holon_frontend::RenderContext;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::geometry::BoundsRegistry;
use support::BoundsSnapshot;
use support::ReactiveFixtureView;

const WINDOW_W: f32 = 1000.0;
const WINDOW_H: f32 = 900.0;
/// Rows in the outline above the footer — several viewports tall.
const OUTLINE_N: usize = 40;

fn desktop() -> Size<gpui::Pixels> {
    size(px(WINDOW_W), px(WINDOW_H))
}

// ── The seeded shape (R1, R4) ──────────────────────────────────────────────

/// The seed's main-panel shape, parameterized on the flag under test.
/// `assets/default/index.org` carries `hide_when_empty: true` on its
/// Linked-references accordion; the `false` arm is the pre-existing contract.
fn panel_dsl(hide_when_empty: bool) -> String {
    format!(
        "column(\
           columns(#{{item_template: live_block()}}), \
           divider(), \
           accordion(#{{title: \"Linked references\", icon: \"link\", \
             max_height_fraction: 0.33, hide_when_empty: {hide_when_empty}}}, \
             live_query(#{{sql: \"SELECT * FROM backlinks\", \
               item_template: list(#{{item_template: text(col(\"content\"))}})}})))"
    )
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

/// A production-faithful `text` row (see `builders/text.rs`): `data.id` plus
/// `props.field == "content"` puts its bounds in the registry keyed by
/// `entity_id`, so the rungs can count what is actually painted.
fn text_row(id: String, label: String) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(id));
    data.insert("content".to_string(), Value::String(label.clone()));
    let mut props = HashMap::new();
    props.insert("content".to_string(), Value::String(label));
    props.insert("field".to_string(), Value::String("content".to_string()));
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = Mutable::new(Arc::new(data)).read_only();
    vm
}

fn outline_collection() -> ReactiveViewModel {
    let items: Vec<ReactiveViewModel> = (0..OUTLINE_N)
        .map(|i| text_row(format!("outline-{i}"), format!("outline {i}")))
        .collect();
    ReactiveViewModel {
        collection: Some(Arc::new(ReactiveView::new_static_with_layout(
            items,
            CollectionVariant::list(0.0),
        ))),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    }
}

/// The panel ViewModel, built the way production builds it: the shell publishes
/// a viewport and the root `RenderContext` takes its `available_space` from
/// `services.viewport_snapshot()`, so a seam that stops supplying it reds these
/// rungs instead of silently falling back to the desktop-first default.
fn interpret_panel(
    services: &Arc<support::TestServices>,
    hide_when_empty: bool,
    window: Size<gpui::Pixels>,
) -> ReactiveViewModel {
    use holon_frontend::reactive::BuilderServices;

    services.set_viewport_size(f32::from(window.width), f32::from(window.height));
    let space = services
        .viewport_snapshot()
        .expect("the published viewport must reach the interpreter through viewport_snapshot");

    let panel_expr = holon::api::block_domain::BlockDomain::wrap_in_query_source_switcher(
        &holon_api::EntityUri::block("default-main-panel"),
        substitute_outline(
            parse_render_dsl(&panel_dsl(hide_when_empty)).expect("panel DSL must parse"),
        ),
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

/// Mount `panel_vm` as `block:default-main-panel` under a `columns` root — the
/// production composition (`live_block`, not a bare column), which is what
/// makes the flow-panel accordion split fire.
fn mount(
    registry: &Arc<support::BlockTreeRegistry>,
    panel_vm: ReactiveViewModel,
) -> Arc<ReactiveViewModel> {
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
    Arc::new(ReactiveViewModel {
        collection: Some(Arc::new(ReactiveView::new_static_with_layout(
            vec![ReactiveViewModel::live_block(holon_api::EntityUri::block(
                "default-main-panel",
            ))],
            CollectionVariant::columns(4.0),
        ))),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    })
}

/// Height of the tagged accordion footer region (0.0 when nothing is tagged).
fn region_height(snap: &BoundsSnapshot) -> f32 {
    snap.of_type("accordion")
        .map(|i| i.height)
        .fold(0.0f32, f32::max)
}

/// Heights of the rules painted between the outline and the section.
fn dividers(snap: &BoundsSnapshot) -> Vec<f32> {
    snap.of_type("divider")
        .filter(|i| i.height > 0.0)
        .map(|i| i.height)
        .collect()
}

/// Error widgets the frame painted. The tracker records no text for them, so
/// this counts what the no-error-widget oracles count.
fn error_widgets(snap: &BoundsSnapshot) -> usize {
    snap.of_type("error").count()
}

/// Backlink rows the section actually painted.
fn painted_backlinks(snap: &BoundsSnapshot) -> usize {
    snap.of_type("text")
        .filter(|i| {
            i.height > 0.0
                && i.entity_id
                    .as_deref()
                    .is_some_and(|e| e.starts_with("backlink-"))
        })
        .count()
}

/// Render the seeded panel with `backlink_rows` query rows.
fn render_seeded(
    cx: &mut TestAppContext,
    hide_when_empty: bool,
    backlink_rows: usize,
) -> BoundsSnapshot {
    cx.update(|cx| gpui_component::init(cx));

    let registry = Arc::new(support::BlockTreeRegistry::new());
    let services = support::TestServices::with_registry_quiescent(registry.clone());
    services.set_live_query_rows(backlink_rows);
    let panel_vm = interpret_panel(&services, hide_when_empty, desktop());
    let root = mount(&registry, panel_vm);
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = services;

    support::render_reactive_fixture_quiescent_sized_with_services(cx, root, desktop(), services)
}

#[gpui::test]
fn an_empty_hide_when_empty_accordion_paints_nothing(cx: &mut TestAppContext) {
    let empty = render_seeded(cx, true, 0);
    let populated = render_seeded(cx, true, 3);

    assert_eq!(
        painted_backlinks(&populated),
        3,
        "R1 setup: with 3 query rows the section must paint them"
    );
    assert!(
        region_height(&populated) > 0.0,
        "R1 setup: a populated hide_when_empty accordion must still paint"
    );
    assert_eq!(
        region_height(&empty),
        0.0,
        "R1 hide-when-empty VIOLATED: with zero backlinks the accordion still \
         occupies {} px (a populated one is {} px) - an empty Linked-references \
         section must not paint its title row or reserve any space",
        region_height(&empty),
        region_height(&populated)
    );
    assert!(
        !dividers(&populated).is_empty(),
        "R1 setup: the rule above a painted section must itself paint"
    );
    assert_eq!(
        dividers(&empty),
        Vec::<f32>::new(),
        "R1 hide-when-empty VIOLATED: the section is gone but the rule that \
         introduces it still paints ({:?}) - a full-width line with nothing under \
         it is exactly the chrome the reader was told would disappear",
        dividers(&empty)
    );
}

#[gpui::test]
fn without_the_flag_an_empty_accordion_keeps_its_title(cx: &mut TestAppContext) {
    let empty = render_seeded(cx, false, 0);

    assert_eq!(
        painted_backlinks(&empty),
        0,
        "R4 setup: an empty query yields no backlink rows"
    );
    assert!(
        region_height(&empty) > 0.0,
        "R4 opt-in VIOLATED: an accordion WITHOUT hide_when_empty vanished when \
         empty - hiding must be opt-in, not the default"
    );
}

// ── The live shape (R2, R3) ────────────────────────────────────────────────

fn backlink_row(ix: usize) -> Arc<ReactiveViewModel> {
    Arc::new(text_row(format!("backlink-{ix}"), format!("backlink {ix}")))
}

/// `column( <outline>, divider, accordion(hide_when_empty, <content>) )` — the
/// shipped shape as a VM tree, so the accordion is a legitimately-placed direct
/// column child and the flow-panel split fires.
fn panel_over(content: Arc<ReactiveViewModel>) -> (ReactiveViewModel, Mutable<bool>) {
    let mut acc_props = HashMap::new();
    acc_props.insert(
        "title".to_string(),
        Value::String("Linked references".into()),
    );
    acc_props.insert("max_height_fraction".to_string(), Value::Float(0.33));
    acc_props.insert("collapsible".to_string(), Value::Boolean(true));
    acc_props.insert("collapsed".to_string(), Value::Boolean(false));
    acc_props.insert("hide_when_empty".to_string(), Value::Boolean(true));
    let expanded = Mutable::new(true);
    let accordion = Arc::new(ReactiveViewModel {
        children: vec![content],
        expanded: Some(expanded.clone()),
        layout_hint: LayoutHint::PinnedToEnd,
        ..ReactiveViewModel::from_widget("accordion", acc_props)
    });

    let mut column = ReactiveViewModel::from_widget("column", HashMap::new());
    column.children = vec![
        Arc::new(outline_collection()),
        Arc::new(ReactiveViewModel::from_widget("divider", HashMap::new())),
        accordion,
    ];
    (column, expanded)
}

/// The section's collection is handed back to the caller, so a rung can move
/// rows through the `MutableVec` the streaming driver writes to.
fn live_panel() -> (ReactiveViewModel, Arc<ReactiveView>, Mutable<bool>) {
    let section = Arc::new(ReactiveView::new_static_with_layout(
        Vec::new(),
        CollectionVariant::list(0.0),
    ));
    let section_node = Arc::new(ReactiveViewModel {
        collection: Some(section.clone()),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    });
    let (column, expanded) = panel_over(section_node);
    (column, section, expanded)
}

/// The same panel over a plain text child: nothing beneath the accordion can
/// ever report a row count.
fn static_content_panel() -> ReactiveViewModel {
    panel_over(Arc::new(text_row(
        "static-note".to_string(),
        "a note".to_string(),
    )))
    .0
}

#[gpui::test]
fn a_row_arriving_shows_the_section_and_its_removal_hides_it_again(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    let registry = Arc::new(support::BlockTreeRegistry::new());
    let services = support::TestServices::with_registry_quiescent(registry.clone());
    services.set_viewport_size(WINDOW_W, WINDOW_H);
    let (panel, section, expanded) = live_panel();
    let root = mount(&registry, panel);
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = services;

    let bounds = BoundsRegistry::new();
    let (_view, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        let services = services.clone();
        move |_, _| ReactiveFixtureView::with_services_and_bounds(root, services, desktop(), bounds)
    });
    let settle = |vcx: &mut gpui::VisualTestContext, bounds: &BoundsRegistry| -> BoundsSnapshot {
        vcx.run_until_parked();
        bounds.flush();
        holon_layout_testing::snapshot::snapshot_from_provider(bounds)
    };

    let hidden = settle(vcx, &bounds);
    assert_eq!(
        region_height(&hidden),
        0.0,
        "R2 setup: an empty hide_when_empty section starts hidden, got {} px",
        region_height(&hidden)
    );
    assert_eq!(
        dividers(&hidden),
        Vec::<f32>::new(),
        "R2 setup: the rule above a hidden section must be gone too, got {:?}",
        dividers(&hidden)
    );

    section.items.lock_mut().push_cloned(backlink_row(0));
    let shown = settle(vcx, &bounds);
    assert_eq!(
        painted_backlinks(&shown),
        1,
        "R2 appear VIOLATED: a backlink arrived but the section painted {} rows",
        painted_backlinks(&shown)
    );
    assert!(
        region_height(&shown) > 0.0,
        "R2 appear VIOLATED: a backlink arrived and the section is still 0 px \
         tall - a hidden accordion must come back when its content does"
    );
    assert!(
        !dividers(&shown).is_empty(),
        "R2 appear VIOLATED: the section came back without the rule that \
         introduces it"
    );

    section.items.lock_mut().clear();
    let hidden_again = settle(vcx, &bounds);
    assert_eq!(
        region_height(&hidden_again),
        0.0,
        "R3 re-hide VIOLATED: the last backlink went away but the section still \
         occupies {} px",
        region_height(&hidden_again)
    );
    assert_eq!(
        dividers(&hidden_again),
        Vec::<f32>::new(),
        "R3 re-hide VIOLATED: the section went away but its rule stayed ({:?})",
        dividers(&hidden_again)
    );
    assert!(
        expanded.get(),
        "R3 state survival VIOLATED: hiding is a paint decision, but the \
         accordion's expand state was reset while it was hidden"
    );

    // The other leg: a reader who COLLAPSED the section must find it collapsed
    // when its rows come back, not reopened by the round trip through hidden.
    expanded.set(false);
    section.items.lock_mut().push_cloned(backlink_row(1));
    let shown_again = settle(vcx, &bounds);
    assert!(
        !expanded.get(),
        "R3 state survival VIOLATED: the reader had collapsed the section, and \
         re-showing it reopened the body"
    );
    assert_eq!(
        painted_backlinks(&shown_again),
        0,
        "R3 state survival VIOLATED: the section is collapsed, so its body must \
         paint no rows; it painted {}",
        painted_backlinks(&shown_again)
    );
}

/// R6 — `hide_when_empty` over content that can never report a row count.
///
/// Hiding such an accordion would be permanent and invisible: nothing beneath
/// it will ever produce the row that brings it back. The frame says so through
/// the NAMED error widget, which is what the no-error-widget oracles read; an
/// error-coloured anonymous div would leave them blind to it.
#[gpui::test]
fn hide_when_empty_over_static_content_paints_a_named_error(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    let registry = Arc::new(support::BlockTreeRegistry::new());
    let services = support::TestServices::with_registry_quiescent(registry.clone());
    services.set_viewport_size(WINDOW_W, WINDOW_H);
    let root = mount(&registry, static_content_panel());
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = services;

    let snap = support::render_reactive_fixture_quiescent_sized_with_services(
        cx,
        root,
        desktop(),
        services,
    );

    assert_eq!(
        error_widgets(&snap),
        1,
        "R6 loud VIOLATED: hide_when_empty over static content must paint the NAMED \
         error widget so the no-error-widget oracles can see it; painted {} of them",
        error_widgets(&snap)
    );
    assert!(
        region_height(&snap) > 0.0,
        "R6 loud VIOLATED: the error takes the section's place on screen, so the \
         slot must have height; it collapsed to 0 px and the failure is invisible \
         after all"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
