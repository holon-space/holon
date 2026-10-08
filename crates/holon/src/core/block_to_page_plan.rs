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
}

/// Fully-resolved plan for turning `origin_id` into a page under
/// `destination_parent_id`, minting page `page_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockToPagePlan {
    /// The block being converted (stays put, becomes a `[[page_id]]` link).
    pub origin_id: String,
    /// The sanitized page title derived from the origin's content.
    pub page_title: String,
    /// The origin's stored content text; its first line is what the link left
    /// behind covers.
    pub origin_content: String,
    /// The origin's tags, written as the tag group after its title.
    pub origin_tags: holon_api::Tags,
    /// The origin's `marks` column value (JSON string or `Null`) — carried onto
    /// the new page so the moved content is byte-identical.
    pub origin_marks: Value,
    /// Deterministic id of the new page = `PageId::for_path(full path)`.
    pub page_id: String,
    /// Existing (or to-be-created) page the new page lands under; the vault
    /// root sentinel when the destination path is empty.
    pub destination_parent_id: String,
    /// Pages that do not yet exist and must be created first, root→leaf.
    pub missing_segments: Vec<PlanSegment>,
    /// The origin's DIRECT children in `sort_key` order — each re-homed under
    /// the new page.
    pub child_ids: Vec<String>,
}

impl PlanSegment {
    pub fn list_to_value(segments: &[PlanSegment]) -> Value {
        Value::Array(
            segments
                .iter()
                .map(|seg| {
                    let mut o = std::collections::HashMap::new();
                    o.insert("id".to_string(), Value::String(seg.id.clone()));
                    o.insert("name".to_string(), Value::String(seg.name.clone()));
                    o.insert(
                        "parent_id".to_string(),
                        Value::String(seg.parent_id.clone()),
                    );
                    Value::Object(o)
                })
                .collect(),
        )
    }

    pub fn list_from_value(value: Option<&Value>) -> Result<Vec<PlanSegment>, String> {
        match value {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::Object(seg) => Ok(PlanSegment {
                        id: get_str(seg, "id")?,
                        name: get_str(seg, "name")?,
                        parent_id: get_str(seg, "parent_id")?,
                    }),
                    other => Err(format!("a segment must be an Object, got {other:?}")),
                })
                .collect(),
            other => Err(format!("expected an Array of segments, got {other:?}")),
        }
    }
}

pub(crate) fn get_str(
    obj: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Result<String, String> {
    match obj.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        other => Err(format!(
            "plan field '{key}' must be a string, got {other:?}"
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
            "page_title".to_string(),
            Value::String(self.page_title.clone()),
        );
        obj.insert(
            "origin_content".to_string(),
            Value::String(self.origin_content.clone()),
        );
        obj.insert(
            "origin_tags".to_string(),
            Value::Array(
                self.origin_tags
                    .iter()
                    .map(|t| Value::String(t.clone()))
                    .collect(),
            ),
        );
        obj.insert("origin_marks".to_string(), self.origin_marks.clone());
        obj.insert("page_id".to_string(), Value::String(self.page_id.clone()));
        obj.insert(
            "destination_parent_id".to_string(),
            Value::String(self.destination_parent_id.clone()),
        );
        obj.insert(
            "missing_segments".to_string(),
            PlanSegment::list_to_value(&self.missing_segments),
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
        let missing_segments = PlanSegment::list_from_value(obj.get("missing_segments"))
            .map_err(|e| format!("BlockToPagePlan: 'missing_segments': {e}"))?;
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
            page_title: get_str(obj, "page_title")?,
            origin_content: get_str(obj, "origin_content")?,
            origin_tags: match obj.get("origin_tags") {
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| match item {
                        Value::String(s) => Ok(s.clone()),
                        other => Err(format!(
                            "BlockToPagePlan: tag must be String, got {other:?}"
                        )),
                    })
                    .collect::<Result<holon_api::Tags, String>>()?,
                other => {
                    return Err(format!(
                        "BlockToPagePlan: 'origin_tags' must be an Array, got {other:?}"
                    ));
                }
            },
            origin_marks: obj.get("origin_marks").cloned().unwrap_or(Value::Null),
            page_id: get_str(obj, "page_id")?,
            destination_parent_id: get_str(obj, "destination_parent_id")?,
            missing_segments,
            child_ids,
        })
    }
}
