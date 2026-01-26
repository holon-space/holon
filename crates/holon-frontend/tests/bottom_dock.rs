//! Shadow-level tests for the mobile action bar primitives.
//!
//! Full mobile end-to-end validation lives in the GPUI layout proptest +
//! general_e2e_pbt. These tests cover only the shadow interpreter's
//! handling of `bottom_dock` + `op_button` — specifically that the built
//! tree has the expected structure for snapshot consumers (MCP,
//! layout-testing).

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_types::{Arg, RenderExpr};
use holon_api::widget_spec::DataRow;
use holon_api::Value;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::{AvailableSpace, ReactiveViewModel, RenderContext, StubBuilderServices};

fn lit(name: &str) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args: vec![],
    }
}

fn pos_string(value: &str) -> Arg {
    Arg {
        name: None,
        value: RenderExpr::Literal {
            value: Value::String(value.to_string()),
        },
    }
}

fn pos(value: RenderExpr) -> Arg {
    Arg { name: None, value }
}

fn interpret(expr: &RenderExpr, row: Option<Arc<DataRow>>) -> ReactiveViewModel {
    let services = StubBuilderServices::new();
    let mut ctx = RenderContext::default();
    if let Some(r) = row {
        ctx = ctx.with_row(r);
    }
    services.interpret(expr, &ctx)
}

#[test]
fn bottom_dock_has_two_named_slots() {
    let main = lit("drop_zone");
    let dock = lit("drop_zone");
    let expr = RenderExpr::FunctionCall {
        name: "bottom_dock".to_string(),
        args: vec![pos(main), pos(dock)],
    };

    let vm = interpret(&expr, None);
    assert_eq!(
        vm.widget_name().as_deref(),
        Some("bottom_dock"),
        "bottom_dock did not produce bottom_dock widget; got: {:?}",
        vm.widget_name()
    );
    assert_eq!(
        vm.children.len(),
        2,
        "bottom_dock must have exactly 2 children"
    );
    assert_eq!(vm.children[0].widget_name().as_deref(), Some("drop_zone"));
    assert_eq!(vm.children[1].widget_name().as_deref(), Some("drop_zone"));
}

#[test]
#[should_panic(expected = "bottom_dock requires exactly 2 positional args")]
fn bottom_dock_rejects_single_slot() {
    let expr = RenderExpr::FunctionCall {
        name: "bottom_dock".to_string(),
        args: vec![pos(lit("drop_zone"))],
    };
    let _ = interpret(&expr, None);
}

#[test]
fn op_button_reads_target_id_and_display_name_from_row() {
    let mut row: DataRow = HashMap::new();
    row.insert("name".into(), Value::String("cycle_task_state".into()));
    row.insert("target_id".into(), Value::String("block:abc-123".into()));
    row.insert(
        "display_name".into(),
        Value::String("Cycle Task State".into()),
    );
    let row = Arc::new(row);

    let expr = RenderExpr::FunctionCall {
        name: "op_button".to_string(),
        args: vec![pos_string("cycle_task_state")],
    };

    let vm = interpret(&expr, Some(row));
    assert_eq!(
        vm.widget_name().as_deref(),
        Some("op_button"),
        "op_button did not produce op_button widget; got: {:?}",
        vm.widget_name()
    );
    assert_eq!(vm.prop_str("op_name").as_deref(), Some("cycle_task_state"));
    assert_eq!(vm.prop_str("target_id").as_deref(), Some("block:abc-123"));
    assert_eq!(
        vm.prop_str("display_name").as_deref(),
        Some("Cycle Task State")
    );
}

#[test]
fn op_button_falls_back_display_name_to_op_name() {
    let mut row: DataRow = HashMap::new();
    row.insert("name".into(), Value::String("delete".into()));
    row.insert("target_id".into(), Value::String("block:xyz".into()));
    // Intentionally no display_name
    let row = Arc::new(row);

    let expr = RenderExpr::FunctionCall {
        name: "op_button".to_string(),
        args: vec![pos_string("delete")],
    };

    let vm = interpret(&expr, Some(row));
    assert_eq!(
        vm.widget_name().as_deref(),
        Some("op_button"),
        "op_button did not produce op_button widget"
    );
    assert_eq!(vm.prop_str("display_name").as_deref(), Some("delete"));
}

// ── Viewport-switching: root_layout DSL at mobile vs desktop ───────────

/// The root_layout render expression from `block_profile.yaml`. Kept in
/// sync with the mobile-bar PR — breaking this test means the profile
/// branches no longer produce a `bottom_dock` on narrow viewports, or
/// the desktop branch accidentally picked one up.
const ROOT_LAYOUT_DSL: &str = r#"
if_space(600,
  bottom_dock(
    columns(drawer("block:default-left-sidebar", live_block("block:default-left-sidebar"), #{mode: "overlay"}), live_block("block:default-main-panel"), drawer("block:default-right-sidebar", live_block("block:default-right-sidebar"), #{mode: "overlay"})),
    columns(#{gap: 8, collection: chain_ops(0), item_template: op_button(col("name"))})),
  if_space(1000,
    columns(drawer("block:default-left-sidebar", live_block("block:default-left-sidebar"), #{mode: "shrink"}), live_block("block:default-main-panel"), drawer("block:default-right-sidebar", live_block("block:default-right-sidebar"), #{mode: "overlay"})),
    columns(drawer("block:default-left-sidebar", live_block("block:default-left-sidebar"), #{mode: "shrink"}), live_block("block:default-main-panel"), drawer("block:default-right-sidebar", live_block("block:default-right-sidebar"), #{mode: "shrink"}))))
"#;

fn space(w: f32, h: f32) -> AvailableSpace {
    AvailableSpace {
        width_px: w,
        height_px: h,
        width_physical_px: w,
        height_physical_px: h,
        scale_factor: 1.0,
    }
}

fn interpret_at(expr: &RenderExpr, w: f32, h: f32) -> ReactiveViewModel {
    let services = StubBuilderServices::new();
    let ctx = RenderContext {
        available_space: Some(space(w, h)),
        ..Default::default()
    };
    services.interpret(expr, &ctx)
}

/// Recursively search for any `bottom_dock` node in the shadow tree.
/// Returns the first match found depth-first. Only walks static children
/// (sufficient for layout-level tests where bottom_dock is structural).
fn find_bottom_dock(node: &ReactiveViewModel) -> Option<&ReactiveViewModel> {
    if node.widget_name().as_deref() == Some("bottom_dock") {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_bottom_dock(child) {
            return Some(found);
        }
    }
    None
}

fn parse(dsl: &str) -> RenderExpr {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    holon::render_dsl::parse_render_dsl(dsl).expect("root_layout DSL parses")
}

/// Verification #2 (shadow/snapshot, bar present on mobile only).
#[test]
fn root_layout_mobile_viewport_has_bottom_dock() {
    let expr = parse(ROOT_LAYOUT_DSL);
    let vm = interpret_at(&expr, 400.0, 800.0);
    let dock = find_bottom_dock(&vm)
        .expect("root_layout at 400px viewport must contain a BottomDock node");

    assert_eq!(
        dock.children.len(),
        2,
        "BottomDock must have main + dock slot"
    );

    // Dock slot should be the streaming columns produced from chain_ops(0).
    // With no focus seeded, chain_ops yields an empty row set, so the
    // slot's streaming collection has 0 items — but it's still a collection
    // view with Columns layout (not, say, a snapshot fallback).
    let dock_slot = &dock.children[1];
    let is_streaming_columns = dock_slot.collection.as_ref().map_or(false, |view| {
        view.layout()
            .as_ref()
            .map(|l| l.name() == "columns")
            .unwrap_or(false)
    });
    assert!(
        is_streaming_columns,
        "dock slot should be a streaming Columns collection view; got {:?}",
        dock_slot.widget_name()
    );
}

/// Verification #8 (MCP regression, desktop viewport).
#[test]
fn root_layout_desktop_viewport_has_no_bottom_dock() {
    let expr = parse(ROOT_LAYOUT_DSL);
    let vm = interpret_at(&expr, 1200.0, 900.0);
    assert!(
        find_bottom_dock(&vm).is_none(),
        "root_layout at 1200px viewport must NOT contain a BottomDock node \
         (bar is gated on if_space(<600))"
    );
}

/// Mid-tier viewport (between 600 and 1000): still desktop-style, still no bar.
#[test]
fn root_layout_mid_viewport_has_no_bottom_dock() {
    let expr = parse(ROOT_LAYOUT_DSL);
    let vm = interpret_at(&expr, 800.0, 900.0);
    assert!(
        find_bottom_dock(&vm).is_none(),
        "root_layout at 800px viewport must NOT contain a BottomDock node"
    );
}
