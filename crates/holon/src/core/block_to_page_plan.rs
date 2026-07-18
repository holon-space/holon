//! The typed plan the `block_to_page_plan` read op hands the engine-level
//! `convert_block_to_page` compound.
//!
//! Parse-don't-validate: the provider reads live block state (origin content +
//! marks, ordered children, the destination page chain) exactly once and emits
//! a fully-resolved plan. The engine then executes each constituent write as an
//! ordinary dispatched op — collecting the op-level inverse of each — and
//! assembles ONE composite `UndoEntry`. No re-reading, no string re-validation
//! across the boundary: the plan carries the proof of every id it names.

use holon_api::Value;

/// One page that must be CREATED to materialize the destination path (a
/// `"Projects/FooBar"` input where `FooBar` does not yet exist). Ordered
/// root→leaf, each carrying the deterministic id `PageId::for_path` assigns and
/// its parent (the previous segment, or the last existing page / root).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanSegment {
    pub id: String,
    pub name: String,
    pub parent_id: String,
    /// `parent.depth + 1` — passed on the `create` so the page row carries a
    /// tree-consistent `depth` (which `move_block` later reads to depth its
    /// re-homed children; an unset depth would leave the moved subtree off by
    /// one AND desynchronize the undo/redo staleness fingerprint).
    pub depth: i64,
}

/// Fully-resolved plan for turning `origin_id` into a page under
/// `destination_parent_id`, minting page `page_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockToPagePlan {
    /// The block being converted (stays put, becomes a `[[page_id]]` link).
    pub origin_id: String,
    /// The origin's stripped-label content text — becomes the page title AND
    /// the link label left behind.
    pub origin_content: String,
    /// The origin's `marks` column value (JSON string or `Null`) — carried onto
    /// the new page so the moved content is byte-identical.
    pub origin_marks: Value,
    /// Deterministic id of the new page = `PageId::for_path(full path)`.
    pub page_id: String,
    /// `destination_parent.depth + 1` — the new page's tree-consistent depth.
    pub page_depth: i64,
    /// Existing (or to-be-created) page the new page lands under; the vault
    /// root sentinel when the destination path is empty.
    pub destination_parent_id: String,
    /// Pages that do not yet exist and must be created first, root→leaf.
    pub missing_segments: Vec<PlanSegment>,
    /// The origin's DIRECT children in `sort_key` order — each re-homed under
    /// the new page.
    pub child_ids: Vec<String>,
}

fn get_str(obj: &std::collections::HashMap<String, Value>, key: &str) -> Result<String, String> {
    match obj.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        other => Err(format!(
            "BlockToPagePlan: field '{key}' must be a string, got {other:?}"
        )),
    }
}

fn get_i64(obj: &std::collections::HashMap<String, Value>, key: &str) -> Result<i64, String> {
    match obj.get(key) {
        Some(Value::Integer(n)) => Ok(*n),
        other => Err(format!(
            "BlockToPagePlan: field '{key}' must be an integer, got {other:?}"
        )),
    }
}

impl BlockToPagePlan {
    pub fn to_value(&self) -> Value {
        let mut obj = std::collections::HashMap::new();
        obj.insert(
            "origin_id".to_string(),
            Value::String(self.origin_id.clone()),
        );
        obj.insert(
            "origin_content".to_string(),
            Value::String(self.origin_content.clone()),
        );
        obj.insert("origin_marks".to_string(), self.origin_marks.clone());
        obj.insert("page_id".to_string(), Value::String(self.page_id.clone()));
        obj.insert("page_depth".to_string(), Value::Integer(self.page_depth));
        obj.insert(
            "destination_parent_id".to_string(),
            Value::String(self.destination_parent_id.clone()),
        );
        obj.insert(
            "missing_segments".to_string(),
            Value::Array(
                self.missing_segments
                    .iter()
                    .map(|seg| {
                        let mut o = std::collections::HashMap::new();
                        o.insert("id".to_string(), Value::String(seg.id.clone()));
                        o.insert("name".to_string(), Value::String(seg.name.clone()));
                        o.insert(
                            "parent_id".to_string(),
                            Value::String(seg.parent_id.clone()),
                        );
                        o.insert("depth".to_string(), Value::Integer(seg.depth));
                        Value::Object(o)
                    })
                    .collect(),
            ),
        );
        obj.insert(
            "child_ids".to_string(),
            Value::Array(
                self.child_ids
                    .iter()
                    .map(|c| Value::String(c.clone()))
                    .collect(),
            ),
        );
        Value::Object(obj)
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let obj = match value {
            Value::Object(o) => o,
            other => {
                return Err(format!("BlockToPagePlan: expected Object, got {other:?}"));
            }
        };
        let missing_segments = match obj.get("missing_segments") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::Object(seg) => Ok(PlanSegment {
                        id: get_str(seg, "id")?,
                        name: get_str(seg, "name")?,
                        parent_id: get_str(seg, "parent_id")?,
                        depth: get_i64(seg, "depth")?,
                    }),
                    other => Err(format!(
                        "BlockToPagePlan: segment must be Object, got {other:?}"
                    )),
                })
                .collect::<Result<Vec<_>, String>>()?,
            other => {
                return Err(format!(
                    "BlockToPagePlan: 'missing_segments' must be an Array, got {other:?}"
                ));
            }
        };
        let child_ids = match obj.get("child_ids") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::String(s) => Ok(s.clone()),
                    other => Err(format!(
                        "BlockToPagePlan: child id must be String, got {other:?}"
                    )),
                })
                .collect::<Result<Vec<_>, String>>()?,
            other => {
                return Err(format!(
                    "BlockToPagePlan: 'child_ids' must be an Array, got {other:?}"
                ));
            }
        };
        Ok(Self {
            origin_id: get_str(obj, "origin_id")?,
            origin_content: get_str(obj, "origin_content")?,
            origin_marks: obj.get("origin_marks").cloned().unwrap_or(Value::Null),
            page_id: get_str(obj, "page_id")?,
            page_depth: get_i64(obj, "page_depth")?,
            destination_parent_id: get_str(obj, "destination_parent_id")?,
            missing_segments,
            child_ids,
        })
    }
}
