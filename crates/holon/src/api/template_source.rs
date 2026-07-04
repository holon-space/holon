//! Reading a template subtree for `instantiate_template`
//! (docs/Proposals/Templating-2026-07-12.md §5).
//!
//! The bare operation dispatcher has no read capability, so the engine carries
//! an optional [`TemplateSource`]. The Turso implementation reads the SQL
//! projection (total by invariant); a wiring without one fails loud at the
//! operation, never silently.

use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use async_trait::async_trait;
use holon_api::Value;
use holon_api::template_instantiation::TemplateNode;

use crate::storage::turso::DbHandle;

/// Loads a template's block subtree: root first, parent-before-child, siblings
/// in `sort_key` order — the exact shape
/// [`plan_instantiation`](holon_api::template_instantiation::plan_instantiation)
/// consumes.
#[async_trait]
pub trait TemplateSource: Send + Sync {
    async fn load_subtree(&self, root_id: &str) -> Result<Vec<TemplateNode>>;
    /// Verify a block id exists in the backing store. `instantiate_template`
    /// calls this to fail loud on a bogus `target_parent` before planning.
    async fn exists(&self, id: &str) -> Result<bool>;
}

/// Guard against a corrupted parent chain (a `parent_id` cycle would loop the
/// BFS forever). Loud panic-by-error at this depth — no real vault nests this
/// deep.
const MAX_TEMPLATE_DEPTH: usize = 512;

/// Turso-backed [`TemplateSource`] over the base block table.
pub struct TursoTemplateSource {
    db_handle: DbHandle,
    table_name: &'static str,
}

impl TursoTemplateSource {
    pub fn new(db_handle: DbHandle) -> Self {
        Self {
            db_handle,
            table_name: crate::storage::BLOCK_WRITE_TABLE,
        }
    }

    async fn fetch_children(&self, parent_id: &str) -> Result<Vec<TemplateNode>> {
        let sql = format!(
            "SELECT id, parent_id, content, content_type, block_type, sort_key, collapsed, \
             widget_only, completed, source_language, source_name, properties, marks FROM {} WHERE parent_id = \
             '{}' ORDER BY sort_key, id",
            self.table_name,
            parent_id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("template children of '{parent_id}': {e}"))?;
        rows.into_iter().map(row_to_node).collect()
    }
}

#[async_trait]
impl TemplateSource for TursoTemplateSource {
    async fn exists(&self, id: &str) -> Result<bool> {
        let sql = format!(
            "SELECT 1 FROM {} WHERE id = '{}' LIMIT 1",
            self.table_name,
            id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("existence check for '{id}': {e}"))?;
        Ok(!rows.is_empty())
    }

    async fn load_subtree(&self, root_id: &str) -> Result<Vec<TemplateNode>> {
        let sql = format!(
            "SELECT id, parent_id, content, content_type, block_type, sort_key, collapsed, \
             widget_only, completed, source_language, source_name, properties, marks FROM {} WHERE id = '{}'",
            self.table_name,
            root_id.replace('\'', "''"),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("template root '{root_id}': {e}"))?;
        let root = rows
            .into_iter()
            .next()
            .with_context(|| format!("template '{root_id}' not found"))?;

        let mut nodes = vec![row_to_node(root)?];
        // BFS, parent-before-child; each level's siblings arrive sort_key-ordered.
        let mut frontier: Vec<String> = vec![root_id.to_string()];
        let mut depth = 0usize;
        while !frontier.is_empty() {
            depth += 1;
            if depth > MAX_TEMPLATE_DEPTH {
                bail!(
                    "template '{root_id}': subtree deeper than {MAX_TEMPLATE_DEPTH} — refusing \
                     (parent cycle?)"
                );
            }
            let mut next = Vec::new();
            for parent in &frontier {
                for child in self.fetch_children(parent).await? {
                    next.push(child.id.clone());
                    nodes.push(child);
                }
            }
            frontier = next;
        }
        Ok(nodes)
    }
}

fn row_to_node(
    row: impl IntoIterator<Item = (std::sync::Arc<str>, Value)>,
) -> Result<TemplateNode> {
    let mut node = TemplateNode::default();
    let mut saw_id = false;
    for (key, value) in row {
        match (key.as_ref(), value) {
            ("id", Value::String(s)) => {
                node.id = s;
                saw_id = true;
            }
            ("parent_id", Value::String(s)) => node.parent_id = s,
            ("content", Value::String(s)) => node.content = s,
            ("content_type", Value::String(s)) => node.content_type = s,
            ("block_type", Value::String(s)) => node.block_type = s,
            ("sort_key", Value::String(s)) => node.sort_key = s,
            ("collapsed", v) => node.collapsed = truthy(&v),
            ("widget_only", v) => node.widget_only = truthy(&v),
            ("completed", v) => node.completed = truthy(&v),
            ("source_language", Value::String(s)) => node.source_language = Some(s),
            ("source_name", Value::String(s)) => node.source_name = Some(s),
            // `DbHandle::query` deserializes JSON TEXT columns into structured
            // `Value`s (Object/Array), so `properties`/`marks` arrive parsed —
            // re-serialize to the JSON string the planner re-parses. A plain
            // TEXT/Json value is passed through verbatim.
            ("properties", v) => node.properties = json_column_to_string(v),
            ("marks", v) => node.marks = json_column_to_string(v),
            // Nullable columns arrive as Null; unmatched shapes for the
            // required TEXT columns would be a projection bug caught by the
            // planner's typed parse with the node id in the message.
            _ => {}
        }
    }
    if !saw_id {
        bail!("template row without an 'id' column — projection shape changed?");
    }
    Ok(node)
}

/// Normalize a JSON-bearing column to its string form. `query` may hand back
/// either the raw TEXT (`String`/`Json`) or an already-parsed structured value
/// (`Object`/`Array`); the planner re-parses a JSON string, so structured
/// values are re-serialized. `Null` (and any scalar, which the schema never
/// stores here) yields `None`.
fn json_column_to_string(v: Value) -> Option<String> {
    match v {
        Value::String(s) | Value::Json(s) => Some(s),
        Value::Null => None,
        obj @ (Value::Object(_) | Value::Array(_)) => {
            Some(serde_json::to_string(&obj).expect("Value serialization is total"))
        }
        // A scalar in a JSON column would be a projection bug; surface it.
        other => panic!("template JSON column held an unexpected scalar: {other:?}"),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        _ => false,
    }
}
