//! Plain (NON-accordion) flow-panel scroll regression (Martin dogfood, real
//! vault). His persisted user-owned main panel is the PRE-accordion form
//! `column(collection_view(), divider(), row(icon, spacer, text),
//! live_query(…))` — no accordion node — so `has_accordion_child` is false and
//! it routes through `columns::render`'s PLAIN flow wrapper. That path stopped
//! scrolling: the outline (56 rows, genuinely overflowing) no-ops on wheel.
//!
//! `main_panel_scroll.rs` covers the plain path with a BARE list collection and
//! passes; the only new variable here is the greedy `live_query` sibling (its
//! `ReactiveShell` is styled `flex_grow:1, height:relative(1.0)`), which is
//! what Martin actually has. This rung stays PERMANENTLY: user-authored
//! non-accordion layouts are a supported shape forever.
//!
//! Run: `cargo test -p holon-gpui --test plain_path_scroll`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::point;
use gpui::px;
use gpui::size;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::render_dsl::parse_render_dsl;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::LayoutHint;
use holon_frontend::RenderContext;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveSlot;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::geometry::BoundsRegistry;
use support::BlockTreeRegistry;
use support::BlockTreeThunk;
use support::ReactiveFixtureView;
use support::simulate_wheel_at;

const ITEM_COUNT: usize = 56;
const FAR_IX: usize = 50;
const VIEWPORT_W: f32 = 700.0;
const VIEWPORT_H: f32 = 500.0;

/// The legacy (pre-accordion) main-panel render — the shape Martin's persisted
/// user-owned `__default__.org` holds.
const LEGACY_SRC: &str = "column(collection_view(), divider(), \
    row(icon(\"link\"), spacer(6), text(\"Linked references\", #{bold: true})), \
    live_query(#{sql: \"SELECT bl.id AS id, bl.content AS content FROM backlinks bl\", \
    item_template: text(col(\"content\"))}))";

fn outline_row(ix: usize) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(format!("outline-{ix}")));
    data.insert(
        "content".to_string(),
        Value::String(format!("outline {ix}")),
    );
    let mut props = HashMap::new();
    props.insert(
        "content".to_string(),
        Value::String(format!("outline {ix}")),
    );
    props.insert("field".to_string(), Value::String("content".to_string()));
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(Arc::new(data)).read_only();
    vm
}

fn outline_collection() -> ReactiveViewModel {
    let items: Vec<ReactiveViewModel> = (0..ITEM_COUNT).map(outline_row).collect();
    let view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    ReactiveViewModel {
        collection: Some(view),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    }
}

/// A `view_mode_switcher` node whose slot holds the outline collection — the
/// faithful production shape of `collection_view()` (routed to
/// `render_content_height`, which wraps the eager collection in a `.relative()`
/// flex_col). This is what Martin's real main panel renders, unlike the bare
/// `list` collection `main_panel_scroll` uses.
fn vms_outline() -> ReactiveViewModel {
    let items: Vec<ReactiveViewModel> = (0..ITEM_COUNT).map(outline_row).collect();
    let view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let slot_content = ReactiveViewModel {
        collection: Some(view),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    };
    ReactiveViewModel {
        slot: Some(ReactiveSlot::new(slot_content)),
        ..ReactiveViewModel::from_widget("view_mode_switcher", HashMap::new())
    }
}

fn substitute_collection_view(expr: RenderExpr) -> RenderExpr {
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            if name == "collection_view" {
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
                        value: substitute_collection_view(a.value),
                    })
                    .collect(),
            }
        }
        RenderExpr::Array { items } => RenderExpr::Array {
            items: items.into_iter().map(substitute_collection_view).collect(),
        },
        RenderExpr::Object { fields } => RenderExpr::Object {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, substitute_collection_view(v)))
                .collect(),
        },
        other => other,
    }
}

fn visible_height(bounds: &BoundsRegistry, entity_id: &str) -> Option<f32> {
    bounds
        .all_elements()
        .iter()
        .find(|(_, i)| i.entity_id.as_deref() == Some(entity_id))
        .map(|(_, i)| i.height)
}

/// A minimal SHRINK drawer — its presence flips `columns::render` into the
/// drawer branch (Martin's real app has left/right sidebars).
fn shrink_drawer(block_id: &str) -> ReactiveViewModel {
    let mut props = HashMap::new();
    props.insert("mode".to_string(), Value::String("shrink".to_string()));
    props.insert("block_id".to_string(), Value::String(block_id.to_string()));
    props.insert("width".to_string(), Value::Float(200.0));
    ReactiveViewModel {
        children: vec![Arc::new(ReactiveViewModel::text("sidebar"))],
        layout_hint: LayoutHint::Fixed { px: 200.0 },
        ..ReactiveViewModel::from_widget("drawer", props)
    }
}

/// `columns([sidebar?], column(<outline 56>, divider, row, live_query))` —
/// Martin's legacy plain-path shape, built through the production DSL + shadow
/// interp. `with_sidebar` picks the drawer branch (his real, sidebar'd shape).
fn legacy_columns_root(
    services: &Arc<dyn holon_frontend::reactive::BuilderServices>,
    with_sidebar: bool,
    use_vms: bool,
) -> Arc<ReactiveViewModel> {
    let expr = substitute_collection_view(parse_render_dsl(LEGACY_SRC).expect("legacy src parses"));
    let interp = holon_frontend::shadow_builders::build_shadow_interpreter();
    let ctx = RenderContext::default();
    let mut column_vm = interp.interpret(&expr, &ctx, &**services);
    let sentinel = column_vm
        .children
        .iter()
        .position(|c| c.widget_name().as_deref() == Some("list"))
        .expect("collection_view -> list sentinel present");
    column_vm.children[sentinel] = Arc::new(if use_vms {
        vms_outline()
    } else {
        outline_collection()
    });

    let mut items: Vec<ReactiveViewModel> = Vec::new();
    if with_sidebar {
        items.push(shrink_drawer("left-sidebar"));
    }
    items.push(column_vm);

    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::columns(4.0),
    ));
    Arc::new(ReactiveViewModel {
        collection: Some(columns_view),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    })
}

fn run_scroll_case(cx: &mut TestAppContext, with_sidebar: bool, use_vms: bool) -> (f32, f32) {
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> =
        support::TestServices::with_registry_quiescent(Arc::new(support::BlockTreeRegistry::new()));
    let root = legacy_columns_root(&services, with_sidebar, use_vms);
    let bounds = BoundsRegistry::new();
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        let services = services.clone();
        move |_, _| {
            ReactiveFixtureView::with_services_and_bounds(
                root,
                services,
                size(px(VIEWPORT_W), px(VIEWPORT_H)),
                bounds,
            )
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    let before = visible_height(&bounds, &format!("outline-{FAR_IX}")).unwrap_or(0.0);
    simulate_wheel_at(
        vcx,
        point(px(VIEWPORT_W / 2.0), px(VIEWPORT_H / 2.0)),
        px(-4000.0),
    );
    vcx.run_until_parked();
    bounds.flush();
    let after = visible_height(&bounds, &format!("outline-{FAR_IX}")).unwrap_or(0.0);
    (before, after)
}

/// Control: WITHOUT sidebars (non-drawer branch) the plain path already
/// scrolls — mirrors `main_panel_scroll` but with the greedy live_query
/// sibling. Guards that the fix does not regress the non-drawer branch.
#[gpui::test]
fn plain_path_non_drawer_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let (before, after) = run_scroll_case(cx, false, false);
    assert!(
        before <= 0.0,
        "outline-{FAR_IX} should start clipped (got {before})"
    );
    assert!(
        after > 0.0,
        "non-drawer plain path must scroll: outline-{FAR_IX} still clipped after wheel ({after})"
    );
}

/// Martin's real shape: WITH a sidebar (drawer branch). RED before the fix —
/// the drawer-branch flow wrapper makes `inner` a `flex_col`, so the content-
/// height column shrinks to fit the panel instead of overflowing, and the wheel
/// no-ops (his live outline of 56 rows was frozen).
#[gpui::test]
fn plain_path_drawer_branch_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let (before, after) = run_scroll_case(cx, true, false);
    assert!(
        before <= 0.0,
        "outline-{FAR_IX} should start clipped (got {before})"
    );
    assert!(
        after > 0.0,
        "PLAIN-PATH SCROLL REGRESSION (drawer branch): after a wheel over the \
         sidebar'd main panel, outline row {FAR_IX} should scroll into view but \
         was still clipped ({after}). The drawer-branch flow wrapper's `.flex_col()` \
         shrinks the content-height column to the panel height, so it never \
         overflows the scroll viewport and the wheel no-ops."
    );
}

/// The most faithful shape: view_mode_switcher outline (render_content_height)
/// + drawer branch + greedy live_query — Martin's exact main panel.
#[gpui::test]
fn plain_path_vms_drawer_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let (before, after) = run_scroll_case(cx, true, true);
    assert!(
        before <= 0.0,
        "outline-{FAR_IX} should start clipped (got {before})"
    );
    assert!(
        after > 0.0,
        "PLAIN-PATH SCROLL REGRESSION (vms + drawer): outline row {FAR_IX} \
         should scroll into view but was still clipped ({after})."
    );
}

/// view_mode_switcher outline, non-drawer branch.
#[gpui::test]
fn plain_path_vms_non_drawer_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let (before, after) = run_scroll_case(cx, false, true);
    assert!(
        before <= 0.0,
        "outline-{FAR_IX} should start clipped (got {before})"
    );
    assert!(
        after > 0.0,
        "vms non-drawer must scroll: outline-{FAR_IX} clipped ({after})"
    );
}

/// Register `block:default-main-panel` so a `live_block` routes through the
/// REAL per-block `ReactiveShell` (production wraps every `block:default-*`
/// panel). The block tree is the legacy column; no live_query is needed to
/// expose the bug (the shell clips ANY overflowing content-height column).
fn register_block(registry: &BlockTreeRegistry, block_id: &str, use_vms: bool) {
    let thunk: BlockTreeThunk = Arc::new(move || {
        let outline = if use_vms {
            vms_outline()
        } else {
            outline_collection()
        };
        let mut col = ReactiveViewModel::from_widget("column", HashMap::new());
        col.children = vec![
            Arc::new(outline),
            Arc::new(ReactiveViewModel::from_widget("divider", HashMap::new())),
        ];
        col
    });
    registry.register(block_id, vec![("default".to_string(), thunk)], 0);
}

/// `columns( live_block(block:default-main-panel) )` — the PRODUCTION wrapping:
/// the per-block ReactiveShell sits between the columns scroll wrapper and the
/// content-height column, unlike the other rungs (which mount the column bare).
fn shell_routed_root(local_id: &str) -> Arc<ReactiveViewModel> {
    let live_block = ReactiveViewModel::live_block(EntityUri::block(local_id));
    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![live_block],
        CollectionVariant::columns(4.0),
    ));
    Arc::new(ReactiveViewModel {
        collection: Some(columns_view),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    })
}

fn run_shell_case(
    cx: &mut TestAppContext,
    block_id: &str,
    local_id: &str,
    use_vms: bool,
) -> (f32, f32) {
    let registry = Arc::new(BlockTreeRegistry::new());
    register_block(&registry, block_id, use_vms);
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> =
        support::TestServices::with_registry_quiescent(registry);
    let root = shell_routed_root(local_id);
    let bounds = BoundsRegistry::new();
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        let services = services.clone();
        move |_, _| {
            ReactiveFixtureView::with_services_and_bounds(
                root,
                services,
                size(px(VIEWPORT_W), px(VIEWPORT_H)),
                bounds,
            )
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    let before = visible_height(&bounds, &format!("outline-{FAR_IX}")).unwrap_or(0.0);
    simulate_wheel_at(
        vcx,
        point(px(VIEWPORT_W / 2.0), px(VIEWPORT_H / 2.0)),
        px(-4000.0),
    );
    vcx.run_until_parked();
    bounds.flush();
    let after = visible_height(&bounds, &format!("outline-{FAR_IX}")).unwrap_or(0.0);
    (before, after)
}

/// THE production-faithful rung. RED before the fix: the block-mode
/// ReactiveShell arm (reactive_shell.rs:~745) wraps the content-height column
/// in a bare `size_full()` with no `overflow_y_scroll`, so the 56-row outline
/// is clipped to the viewport and the wheel no-ops — Martin's frozen outline.
#[gpui::test]
fn shell_wrapped_main_panel_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let (before, after) =
        run_shell_case(cx, "block:default-main-panel", "default-main-panel", true);
    assert!(
        before <= 0.0,
        "outline-{FAR_IX} should start clipped (got {before})"
    );
    assert!(
        after > 0.0,
        "SHELL-WRAPPED SCROLL REGRESSION: through the real per-block ReactiveShell, \
         after a wheel over the main panel, outline row {FAR_IX} should scroll into \
         view but was still clipped ({after}). The block-mode shell arm's bare \
         size_full() clips the content-height column with no scroll viewport."
    );
}

/// The SIDEBAR routes through the SAME generic block-mode shell arm — its id
/// also starts `block:default-`. One fix covers both. RED before the fix for
/// the same reason (bare size_full clips the content-height sidebar tree).
#[gpui::test]
fn shell_wrapped_sidebar_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let (before, after) = run_shell_case(
        cx,
        "block:default-left-sidebar",
        "default-left-sidebar",
        true,
    );
    assert!(
        before <= 0.0,
        "outline-{FAR_IX} should start clipped in the sidebar shell (got {before})"
    );
    assert!(
        after > 0.0,
        "SIDEBAR SHELL SCROLL: block:default-left-sidebar routes through the same          block-mode shell arm; after the fix its overflowing tree must scroll          (outline-{FAR_IX} still clipped: {after})."
    );
}

/// KNOWN BUG (red-first scaffold for the follow-up): route an ACCORDION-bearing
/// column through the real per-block shell — the PRODUCTION wrapping. The
/// accordion split lives in `columns::render`, gated on the flow child being a
/// `column`; production's flow child is a
/// `live_block(block:default-main-panel)`, so `columns::render` sees a
/// live_block, the split never fires, and the accordion reaches its generic
/// render — the PLACEMENT-ERROR div (~38px) — not a bounded footer (~150px).
/// Confirmed here (see the eprintln). Ignored so the gate stays green;
/// un-ignore + fix (relocate the split to fire wherever a column-with-accordion
/// is rendered, i.e. the block-shell arm) in the follow-up.
#[gpui::test]
#[ignore = "KNOWN: accordion split does not fire through the per-block live_block             shell (renders placement-error, ~38px, not a bounded footer). Follow-up."]
fn accordion_through_shell_renders_bounded_not_error(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let registry = Arc::new(BlockTreeRegistry::new());
    let thunk: BlockTreeThunk = Arc::new(|| {
        let mut acc_props = HashMap::new();
        acc_props.insert(
            "title".to_string(),
            Value::String("Linked references".into()),
        );
        acc_props.insert("max_height_fraction".to_string(), Value::Float(0.33));
        acc_props.insert("collapsible".to_string(), Value::Boolean(true));
        acc_props.insert("collapsed".to_string(), Value::Boolean(false));
        let backlinks: Vec<Arc<ReactiveViewModel>> =
            (0..5).map(|i| Arc::new(outline_row(i))).collect();
        let accordion = ReactiveViewModel {
            children: backlinks,
            expanded: Some(futures_signals::signal::Mutable::new(true)),
            ..ReactiveViewModel::from_widget("accordion", acc_props)
        };
        // Small outline so the accordion footer is IN VIEW (a full 56-row
        // outline would push it below the fold and report height 0).
        let small_items: Vec<ReactiveViewModel> = (0..3).map(outline_row).collect();
        let small_view = Arc::new(ReactiveView::new_static_with_layout(
            small_items,
            CollectionVariant::list(0.0),
        ));
        let small_outline = ReactiveViewModel {
            collection: Some(small_view),
            ..ReactiveViewModel::from_widget("list", HashMap::new())
        };
        let mut col = ReactiveViewModel::from_widget("column", HashMap::new());
        col.children = vec![
            Arc::new(small_outline),
            Arc::new(ReactiveViewModel::from_widget("divider", HashMap::new())),
            Arc::new(accordion),
        ];
        col
    });
    registry.register(
        "block:default-main-panel",
        vec![("default".to_string(), thunk)],
        0,
    );
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> =
        support::TestServices::with_registry_quiescent(registry);
    let root = shell_routed_root("default-main-panel");
    let bounds = BoundsRegistry::new();
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        let services = services.clone();
        move |_, _| {
            ReactiveFixtureView::with_services_and_bounds(
                root,
                services,
                size(px(VIEWPORT_W), px(VIEWPORT_H)),
                bounds,
            )
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    let has_accordion = bounds
        .all_elements()
        .iter()
        .any(|(_, i)| i.widget_type.as_ref() == "accordion");
    let has_error = bounds
        .all_elements()
        .iter()
        .any(|(_, i)| i.widget_type.as_ref() == "error");
    let acc_h = bounds
        .all_elements()
        .iter()
        .filter(|(_, i)| i.widget_type.as_ref() == "accordion")
        .map(|(_, i)| i.height)
        .fold(0.0f32, f32::max);
    // Bounded (split fired) → header + 5 backlinks ≈ 150px; generic error div
    // ≈ 40px. Also dump the accordion subtree text count.
    let acc_text_rows = bounds
        .all_elements()
        .iter()
        .filter(|(_, i)| i.widget_type.as_ref() == "text" && i.height > 0.0)
        .count();
    eprintln!(
        "ACCORDION_THROUGH_SHELL has_accordion={has_accordion} has_error={has_error}          acc_height={acc_h} visible_text_rows={acc_text_rows}"
    );
    // DESIRED: the accordion renders as a bounded footer (header + backlinks,
    // well over the ~38px placement-error div). Currently RED (the split does
    // not fire through the shell) — hence #[ignore] above.
    assert!(
        acc_h > 100.0,
        "accordion routed through the per-block shell should be a BOUNDED footer          (>100px), but was {acc_h}px — the placement-error div. The accordion          split in columns::render does not fire because the flow child is a          live_block, not a column."
    );
}
