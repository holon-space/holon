//! `describe_ui` reports STRUCTURE; an agent reading it cannot tell a row
//! that is on screen from one laid out at zero height or clipped out of the
//! viewport. These tests pin the geometry annotation: measured rects joined
//! from the frontend's `GeometryProvider`, and — the part that matters for
//! trust — an EXPLICIT absence marker wherever no measurement exists.
//!
//! @pbt kind example
//! @pbt covers describe-ui-geometry — measured layout rects on describe_ui
//! nodes, with fail-loud absence markers

use std::sync::Arc;

use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::geometry::SharedBoundsRegistry;
use holon_mcp::describe_ui_geometry::annotate_json;
use holon_mcp::describe_ui_geometry::geometry_text_report;
use serde_json::Value;
use serde_json::json;

fn info(widget: &str, entity: &str, rect: (f32, f32, f32, f32)) -> ElementInfo {
    ElementInfo {
        x: rect.0,
        y: rect.1,
        width: rect.2,
        height: rect.3,
        widget_type: Arc::from(widget),
        entity_id: Some(Arc::from(entity)),
        has_content: true,
        parent_id: None,
        displayed_text: None,
        focused: None,
        styled_runs: None,
        opacity: None,
        expected_size: Default::default(),
    }
}

/// A two-node `describe_ui` JSON tree: a painted editor row and a sibling
/// that the registry never saw.
fn tree() -> Value {
    json!({
        "widget": "list",
        "entity": { "id": "block:panel" },
        "children": {
            "items": [
                {
                    "widget": "editable_text",
                    "entity": { "id": "block:painted" },
                    "content": "hello",
                },
                {
                    "widget": "rendered_text",
                    "entity": { "id": "block:never-painted" },
                    "content": "offscreen",
                },
            ]
        }
    })
}

fn registry() -> SharedBoundsRegistry {
    let reg = SharedBoundsRegistry::new();
    reg.record(
        "panel-el".into(),
        info("list", "block:panel", (0.0, 0.0, 800.0, 600.0)),
    );
    reg.record(
        "editor-el".into(),
        info("editable_text", "block:painted", (12.0, 340.5, 600.0, 22.0)),
    );
    reg
}

fn node_at(tree: &Value, idx: usize) -> &Value {
    &tree["children"]["items"][idx]
}

/// A painted widget reports its REAL rect, joined on `(widget_type, entity)`.
#[test]
fn a_painted_node_carries_its_measured_rect() {
    let reg = registry();
    let out = annotate_json(&tree(), Some(&reg as &dyn GeometryProvider));

    let geo = &node_at(&out, 0)["geometry"];
    assert_eq!(
        geo["x"], 12.0,
        "painted editable_text must report the registry's x. geometry={geo}"
    );
    assert_eq!(geo["y"], 340.5, "y must come from the registry. geo={geo}");
    assert_eq!(geo["width"], 600.0, "width from the registry. geo={geo}");
    assert_eq!(geo["height"], 22.0, "height from the registry. geo={geo}");
    assert_eq!(
        geo["has_visible_area"], true,
        "a 600x22 rect has visible area. geo={geo}"
    );
    assert_eq!(
        geo["el_id"], "editor-el",
        "the joined element id must be disclosed so a reader can re-query it. geo={geo}"
    );

    // The root node is itself entity-bound and painted.
    assert_eq!(
        out["geometry"]["width"], 800.0,
        "the root list node joins too. out={out}"
    );
    assert_eq!(
        out["geometry_source"]["available"], true,
        "a provider-backed run must say so. out={out}"
    );
}

/// A node the registry never saw gets an explicit marker — NOT a zero rect.
#[test]
fn an_unpainted_node_is_marked_absent_not_zeroed() {
    let reg = registry();
    let out = annotate_json(&tree(), Some(&reg as &dyn GeometryProvider));

    let geo = &node_at(&out, 1)["geometry"];
    assert_eq!(
        geo["absent"], "not_painted",
        "a never-painted node must carry the absence marker. geo={geo}"
    );
    assert!(
        geo.get("x").is_none() && geo.get("width").is_none(),
        "an absent node must carry NO coordinates at all (zeros would read as a real \
         degenerate rect). geo={geo}"
    );
}

/// Headless / no frontend: one honest root marker, and not a single
/// fabricated coordinate anywhere in the tree.
#[test]
fn a_run_without_a_geometry_provider_says_so_once() {
    let out = annotate_json(&tree(), None);

    assert_eq!(
        out["geometry_source"]["available"], false,
        "with no provider the output must declare geometry unavailable. out={out}"
    );
    let reason = out["geometry_source"]["reason"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        reason.contains("no geometry provider"),
        "the reason must name the actual cause. reason={reason:?} out={out}"
    );
    assert!(
        !serde_json::to_string(&out)
            .expect("serialize")
            .contains("\"x\""),
        "no node may carry coordinates when nothing was measured. out={out}"
    );

    let report = geometry_text_report(&tree(), None);
    assert!(
        report.contains("UNAVAILABLE"),
        "the text rendering must disclose the absence too. report={report:?}"
    );
}

/// The `(widget_type, entity_id)` pair is NOT unique. Two candidates must be
/// disclosed as ambiguous, never silently resolved to one.
#[test]
fn an_ambiguous_join_is_disclosed_not_resolved() {
    let reg = registry();
    reg.record(
        "editor-el-duplicate".into(),
        info("editable_text", "block:painted", (12.0, 900.0, 600.0, 22.0)),
    );

    let out = annotate_json(&tree(), Some(&reg as &dyn GeometryProvider));
    let geo = &node_at(&out, 0)["geometry"];
    assert_eq!(
        geo["absent"], "ambiguous",
        "two elements share the pair — picking one would be a guess. geo={geo}"
    );
    assert_eq!(
        geo["candidates"], 2,
        "the candidate count must be disclosed. geo={geo}"
    );
}

/// Registry widget types are the frontend's own tracker names and only
/// sometimes equal the view-model's widget tag, so most nodes join on the
/// entity alone. Such a rect belongs to a SIBLING element of the same entity —
/// attributing it to this node silently would be the fabrication this whole
/// annotation exists to avoid, so both renderings must say "via <widget>".
#[test]
fn an_entity_only_join_discloses_which_widget_was_measured() {
    let reg = registry();
    reg.record(
        "sibling-el".into(),
        info(
            "selectable",
            "block:never-painted",
            (4.0, 500.0, 200.0, 30.0),
        ),
    );

    let out = annotate_json(&tree(), Some(&reg as &dyn GeometryProvider));
    let geo = &node_at(&out, 1)["geometry"];
    assert_eq!(
        geo["match"], "entity_only",
        "a rendered_text node measured from a `selectable` element must say so. geo={geo}"
    );
    assert_eq!(geo["painted_as"], "selectable", "geo={geo}");

    let report = geometry_text_report(&tree(), Some(&reg as &dyn GeometryProvider));
    let line = report
        .lines()
        .find(|l| l.contains("block:never-painted"))
        .unwrap_or_default();
    assert!(
        line.contains("via selectable"),
        "the text line must disclose that the rect came from another widget of the same \
         entity, not from this node. line={line:?}"
    );
}

/// `McpUserDriver::refresh_ui` parses `describe_ui`'s JSON straight back into
/// a `ViewModel`. Geometry rides ALONGSIDE that contract — an annotated tree
/// must still deserialize, or every MCP-driven test rung breaks.
#[test]
fn an_annotated_tree_still_deserializes_as_a_view_model() {
    use holon_frontend::view_model::ViewModel;

    let vm = ViewModel::collection(
        "list",
        vec![ViewModel::element(
            "table_row",
            Arc::new(std::collections::HashMap::from([(
                "id".to_string(),
                holon_frontend::Value::String("block:painted".into()),
            )])),
            vec![],
        )],
    );
    let serialized = serde_json::to_value(&vm).expect("serialize view model");
    let annotated = annotate_json(&serialized, Some(&registry() as &dyn GeometryProvider));

    serde_json::from_value::<ViewModel>(annotated.clone()).unwrap_or_else(|e| {
        panic!(
            "annotated describe_ui JSON must still parse as a ViewModel \
                                    (McpUserDriver depends on it): {e}. json={annotated}"
        )
    });
}

/// The text rendering carries the same facts in a compact per-node form.
#[test]
fn the_text_report_lists_measured_and_absent_nodes() {
    let reg = registry();
    let report = geometry_text_report(&tree(), Some(&reg as &dyn GeometryProvider));

    assert!(
        report.contains("block:painted") && report.contains("340.5"),
        "the painted row's rect must appear. report={report}"
    );
    assert!(
        report.contains("block:never-painted") && report.contains("not_painted"),
        "the unpainted row's absence must appear. report={report}"
    );
}
