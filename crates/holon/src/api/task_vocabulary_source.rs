//! SQL-backed [`TaskVocabularySource`]: the `#+TODO:` vocabulary that governs
//! a block, read off its nearest `Page`-tagged ancestor.
//!
//! ONE read per hop — parent, page tag and properties come back together — so
//! the common block-under-its-page shape costs two reads. A recursive CTE
//! would be one, but Turso answers a SELECT that JOINs a recursive CTE with
//! the CTE's own rows instead of the projection asked for.

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_org_format::OrgDocumentExt;
use holon_org_format::TaskKeywordVocabulary;

use crate::core::task_keyword_promotion::TaskVocabularySource;
use crate::storage::DbHandle;

/// Bound on the parent chain — a cycle must fail loud, not spin.
const MAX_HOPS: usize = 1024;

/// One hop of the walk: everything the next step needs, in one row.
struct Hop {
    parent_id: Option<String>,
    properties: Option<Value>,
    is_page: bool,
}

pub struct SqlTaskVocabularySource {
    db: DbHandle,
    table: String,
}

impl SqlTaskVocabularySource {
    pub fn new(db: DbHandle, table: impl Into<String>) -> Self {
        Self {
            db,
            table: table.into(),
        }
    }

    async fn hop(&self, id: &str) -> Result<Option<Hop>> {
        let sql = format!(
            "SELECT b.parent_id AS parent_id, b.properties AS properties, \
             (SELECT COUNT(*) FROM block_tags t WHERE t.block_id = b.id AND t.tag = 'Page') \
             AS page_tags FROM {} b WHERE b.id = '{}'",
            self.table,
            id.replace('\'', "''")
        );
        let mut rows = self
            .db
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("task vocabulary read for {id}: {e}"))?;
        let Some(mut row) = rows.drain(..).next() else {
            return Ok(None);
        };
        let parent_id = match row.remove("parent_id") {
            Some(Value::String(s)) if !s.is_empty() && s != EntityUri::no_parent().as_str() => {
                Some(s)
            }
            _ => None,
        };
        let is_page = matches!(row.get("page_tags"), Some(Value::Integer(n)) if *n > 0);
        Ok(Some(Hop {
            parent_id,
            properties: row.remove("properties"),
            is_page,
        }))
    }

    /// The `properties` of the nearest `Page`-tagged ancestor, `block_id`
    /// itself included — an editor can be open on a page row.
    async fn owning_page_properties(&self, block_id: &str) -> Result<Option<Value>> {
        let mut cursor = Some(block_id.to_string());
        for _ in 0..MAX_HOPS {
            let Some(id) = cursor else { return Ok(None) };
            let Some(hop) = self.hop(&id).await? else {
                return Ok(None);
            };
            if hop.is_page {
                return Ok(hop.properties);
            }
            cursor = hop.parent_id;
        }
        anyhow::bail!("task vocabulary: parent chain of {block_id} exceeds {MAX_HOPS} hops")
    }

    /// The raw `#+TODO:` declaration of the owning document, `None` when it
    /// declares none. The typed form the editor's source projection needs; the
    /// vocabulary below is the same fact with the parser's defaults applied.
    pub async fn declared_keywords(
        &self,
        block_id: &str,
    ) -> Result<Option<Vec<holon_api::TaskState>>> {
        let properties = match self.owning_page_properties(block_id).await? {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(s)) | Some(Value::Json(s)) if s.trim().is_empty() => {
                return Ok(None);
            }
            Some(other) => crate::api::operation_engine::properties_object(&other)?,
        };
        let mut doc = Block::new_text(EntityUri::no_parent(), EntityUri::no_parent(), "");
        doc.properties = properties;
        Ok(doc.todo_keywords())
    }
}

#[async_trait]
impl TaskVocabularySource for SqlTaskVocabularySource {
    async fn vocabulary_for_block(&self, block_id: &str) -> Result<TaskKeywordVocabulary> {
        Ok(TaskKeywordVocabulary::from_declared(
            self.declared_keywords(block_id).await?,
        ))
    }
}
