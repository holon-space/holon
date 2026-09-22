//! [`WriteAuthorityReads`] for a session whose SQL tables accept block writes
//! directly (no Loro): the write tables are the authority, so reading them
//! cannot race a projection.

use std::collections::HashMap;

use async_trait::async_trait;
use holon_api::Block;
use holon_api::DynamicEntity;
use holon_api::EntityUri;
use holon_api::PAGE_TAG;
use holon_api::StoredBlock;
use holon_api::Value;
use holon_core::WriteAuthorityReads;

use super::traits::Result;
use super::traits::TryFromEntity;
use crate::storage::BLOCK_WRITE_TABLE;
use crate::storage::DbHandle;

/// Deeper than any real outline; reaching it means a stored `parent_id` cycle.
const MAX_SUBTREE_DEPTH: usize = 512;

pub struct SqlWriteAuthority {
    db_handle: DbHandle,
}

impl SqlWriteAuthority {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }

    async fn blocks_where(&self, predicate: &str, bound: &str) -> Result<Vec<StoredBlock>> {
        let sql = format!(
            "SELECT * FROM {BLOCK_WRITE_TABLE} WHERE {predicate} = $bound ORDER BY sort_key, id"
        );
        let params = HashMap::from([("bound".to_string(), Value::String(bound.to_string()))]);
        let rows =
            self.db_handle.query(&sql, params).await.map_err(|e| {
                format!("{BLOCK_WRITE_TABLE} rows with {predicate} = '{bound}': {e}")
            })?;
        rows.into_iter()
            .map(|row| {
                let context = || format!("{BLOCK_WRITE_TABLE} row with {predicate} = '{bound}'");
                let block_type = match row.get("block_type") {
                    Some(Value::String(s)) => s.clone(),
                    other => {
                        return Err(format!(
                            "{}: block_type is not a string: {other:?}",
                            context()
                        )
                        .into());
                    }
                };
                let completed = match row.get("completed") {
                    Some(Value::Boolean(b)) => *b,
                    Some(Value::Integer(0)) => false,
                    Some(Value::Integer(1)) => true,
                    other => {
                        return Err(format!(
                            "{}: completed is not a boolean: {other:?}",
                            context()
                        )
                        .into());
                    }
                };
                let mut entity = DynamicEntity::new("block");
                for (key, value) in row {
                    entity.set(key, value);
                }
                let block =
                    Block::from_entity(entity).map_err(|e| format!("{}: {e}", context()))?;
                Ok(StoredBlock {
                    block,
                    block_type: Some(block_type),
                    completed: Some(completed),
                })
            })
            .collect()
    }
}

#[async_trait]
impl WriteAuthorityReads for SqlWriteAuthority {
    async fn block_exists(&self, id: &EntityUri) -> Result<bool> {
        let sql = format!("SELECT 1 FROM {BLOCK_WRITE_TABLE} WHERE id = $id LIMIT 1");
        let params = HashMap::from([("id".to_string(), Value::String(id.to_string()))]);
        let rows = self
            .db_handle
            .query(&sql, params)
            .await
            .map_err(|e| format!("existence of {id}: {e}"))?;
        Ok(!rows.is_empty())
    }

    async fn block_is_page(&self, id: &EntityUri) -> Result<bool> {
        let sql = "SELECT 1 FROM block_tags WHERE block_id = $id AND tag = $tag LIMIT 1";
        let params = HashMap::from([
            ("id".to_string(), Value::String(id.to_string())),
            ("tag".to_string(), Value::String(PAGE_TAG.to_string())),
        ]);
        let rows = self
            .db_handle
            .query(sql, params)
            .await
            .map_err(|e| format!("page tag of {id}: {e}"))?;
        Ok(!rows.is_empty())
    }

    async fn subtree(&self, root: &EntityUri) -> Result<Option<Vec<StoredBlock>>> {
        let Some(root_block) = self.blocks_where("id", root.as_str()).await?.pop() else {
            return Ok(None);
        };
        let mut nodes = vec![root_block];
        let mut level = 0..1;
        for depth in 0.. {
            if level.is_empty() {
                break;
            }
            if depth == MAX_SUBTREE_DEPTH {
                return Err(format!(
                    "subtree of {root} is deeper than {MAX_SUBTREE_DEPTH} levels — a stored \
                     parent_id cycle?"
                )
                .into());
            }
            let start = nodes.len();
            for i in level {
                let parent = nodes[i].block.id.to_string();
                nodes.extend(self.blocks_where("parent_id", &parent).await?);
            }
            level = start..nodes.len();
        }
        Ok(Some(nodes))
    }
}
