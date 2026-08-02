//! Measured-layout annotation for `describe_ui`.
//!
//! `describe_ui` answers "what would an agent see?" structurally. Structure
//! alone cannot tell a row that is on screen from one laid out at zero height
//! or scrolled out of the clip mask, so this module joins each node against
//! the frontend's [`GeometryProvider`] — the SAME painted rects the click
//! resolver uses — and annotates the node with the real rect.
//!
//! Absence is always explicit: a node that was never painted, an ambiguous
//! join, and a run with no provider at all each get a distinct marker. No
//! coordinate in this output is ever synthesized.

use std::collections::HashMap;
use std::fmt::Write;

use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use serde_json::Value;
use serde_json::json;

/// Registry elements grouped by the entity they render.
pub struct GeometryIndex {
    by_entity: HashMap<String, Vec<(String, ElementInfo)>>,
    total: usize,
}

/// Strip one leading URI scheme so both sides of the join key alike: renderers
/// tag with the schemed row id (`block:foo`) while the view-model may carry
/// either form.
fn bare_id(id: &str) -> &str {
    for scheme in ["block:", "doc:", "page:"] {
        if let Some(rest) = id.strip_prefix(scheme) {
            return rest;
        }
    }
    id
}

impl GeometryIndex {
    pub fn build(provider: &dyn GeometryProvider) -> Self {
        let mut by_entity: HashMap<String, Vec<(String, ElementInfo)>> = HashMap::new();
        let elements = provider.all_elements();
        let total = elements.len();
        for (el_id, info) in elements {
            if let Some(entity) = info.entity_id.clone() {
                by_entity
                    .entry(bare_id(&entity).to_string())
                    .or_default()
                    .push((el_id, info));
            }
        }
        Self { by_entity, total }
    }

    pub fn total_elements(&self) -> usize {
        self.total
    }

    /// Candidates for a `(widget_type, entity_id)` pair. The pair is NOT
    /// guaranteed unique (the registry itself warns when a pair migrates
    /// el_id), so the caller decides what to do with >1 rather than picking.
    fn candidates(
        &self,
        widget: Option<&str>,
        entity: &str,
    ) -> (Vec<&(String, ElementInfo)>, bool) {
        let all = match self.by_entity.get(bare_id(entity)) {
            Some(v) => v,
            None => return (Vec::new(), true),
        };
        if let Some(widget) = widget {
            let exact: Vec<_> = all
                .iter()
                .filter(|(_, info)| &*info.widget_type == widget)
                .collect();
            if !exact.is_empty() {
                return (exact, true);
            }
        }
        (all.iter().collect(), false)
    }
}

/// The geometry facts for one node, or the reason there are none.
fn node_geometry(index: &GeometryIndex, widget: Option<&str>, entity: &str) -> Value {
    let (candidates, exact_pair) = index.candidates(widget, entity);
    match candidates.len() {
        0 => json!({ "absent": "not_painted" }),
        1 => {
            let (el_id, info) = candidates[0];
            let mut obj = json!({
                "el_id": el_id,
                "x": round1(info.x),
                "y": round1(info.y),
                "width": round1(info.width),
                "height": round1(info.height),
                "has_visible_area": info.has_visible_area(),
            });
            let map = obj.as_object_mut().expect("geometry object");
            if !exact_pair {
                map.insert("match".into(), json!("entity_only"));
                map.insert("painted_as".into(), json!(&*info.widget_type));
            }
            if let Some(focused) = info.focused {
                map.insert("focused".into(), json!(focused));
            }
            if let Some(opacity) = info.opacity {
                map.insert("opacity".into(), json!(round1(opacity)));
            }
            obj
        }
        n => json!({
            "absent": "ambiguous",
            "candidates": n,
            "el_ids": candidates
                .iter()
                .take(4)
                .map(|(el_id, _)| el_id.clone())
                .collect::<Vec<_>>(),
        }),
    }
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// Why a run carries no measured geometry at all.
const NO_PROVIDER: &str = "no geometry provider registered (headless run, or a frontend with no \
                           painted window) — no coordinates are being guessed";

/// A node's identity for the geometry join: the serde widget tag and the
/// entity URI it renders. Nodes without an entity are not joinable at all.
fn node_key(node: &serde_json::Map<String, Value>) -> Option<(Option<&str>, &str)> {
    let entity = node.get("entity")?.get("id")?.as_str()?;
    Some((node.get("widget").and_then(Value::as_str), entity))
}

/// Walk every object in the tree, annotating the ones that look like widget
/// nodes. Recursion is structure-agnostic (it does not hard-code
/// `children.items`) so a render-schema change cannot silently drop geometry.
fn annotate_in_place(value: &mut Value, index: &GeometryIndex) {
    match value {
        Value::Object(map) => {
            let geo = node_key(map).map(|(widget, entity)| node_geometry(index, widget, entity));
            if let Some(geo) = geo {
                map.insert("geometry".into(), geo);
            }
            for child in map.values_mut() {
                annotate_in_place(child, index);
            }
        }
        Value::Array(items) => {
            for item in items {
                annotate_in_place(item, index);
            }
        }
        _ => {}
    }
}

fn collect_nodes<'a>(value: &'a Value, out: &mut Vec<&'a serde_json::Map<String, Value>>) {
    match value {
        Value::Object(map) => {
            if node_key(map).is_some() {
                out.push(map);
            }
            for child in map.values() {
                collect_nodes(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_nodes(item, out);
            }
        }
        _ => {}
    }
}

/// Annotate every entity-bound node of a serialized `describe_ui` tree with
/// its measured rect. With no provider the tree carries ONE root-level marker
/// instead of a per-node repetition of the same absence.
pub fn annotate_json(tree: &Value, geometry: Option<&dyn GeometryProvider>) -> Value {
    let mut out = tree.clone();
    let source = match geometry {
        None => json!({ "available": false, "reason": NO_PROVIDER }),
        Some(provider) => {
            let index = GeometryIndex::build(provider);
            annotate_in_place(&mut out, &index);
            json!({ "available": true, "elements_recorded": index.total_elements() })
        }
    };
    if let Some(map) = out.as_object_mut() {
        map.insert("geometry_source".into(), source);
    }
    out
}

/// Compact geometry report appended under the `text` rendering of the tree —
/// one line per entity-bound node, measured or explicitly absent.
pub fn geometry_text_report(tree: &Value, geometry: Option<&dyn GeometryProvider>) -> String {
    let Some(provider) = geometry else {
        return format!("geometry: UNAVAILABLE — {NO_PROVIDER}\n");
    };
    let index = GeometryIndex::build(provider);
    let mut nodes = Vec::new();
    collect_nodes(tree, &mut nodes);

    let mut out = format!(
        "geometry (measured, {} elements recorded):\n",
        index.total_elements()
    );
    for node in nodes {
        let (widget, entity) = node_key(node).expect("collect_nodes keeps only joinable nodes");
        let geo = node_geometry(&index, widget, entity);
        let widget = widget.unwrap_or("?");
        let detail = match geo.get("absent").and_then(Value::as_str) {
            Some("ambiguous") => format!("ABSENT: ambiguous ({} candidates)", geo["candidates"]),
            Some(reason) => format!("ABSENT: {reason}"),
            None => format!(
                "x={} y={} w={} h={}{}{}",
                geo["x"],
                geo["y"],
                geo["width"],
                geo["height"],
                if geo["has_visible_area"] == json!(true) {
                    " visible"
                } else {
                    " NO-VISIBLE-AREA"
                },
                match geo.get("painted_as").and_then(Value::as_str) {
                    Some(painted) => format!(" (via {painted})"),
                    None => String::new(),
                }
            ),
        };
        let _ = writeln!(out, "  {widget} {entity}  {detail}");
    }
    out
}
