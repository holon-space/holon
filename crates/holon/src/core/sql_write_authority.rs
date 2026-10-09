//! [`WriteAuthorityReads`] for a session whose SQL tables accept block writes
//! directly (no Loro): the write tables are the authority, so reading them
//! cannot race a projection.

use std::collections::HashMap;

use async_trait::async_trait;
use holon_api::Block;
use holon_api::EntityUri;
use holon_api::PAGE_TAG;
use holon_api::Value;
use holon_core::WriteAuthorityReads;

use super::traits::Result;
use crate::storage::BLOCK_WRITE_TABLE;
use crate::storage::DbHandle;
use crate::storage::HYDRATED_BLOCK_COLUMNS;

/// Deeper than any real outline; reaching it means a stored `parent_id` cycle.
const MAX_SUBTREE_DEPTH: usize = 512;

pub struct SqlWriteAuthority {
    db_handle: DbHandle,
}

impl SqlWriteAuthority {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }

    async fn blocks_where(&self, predicate: &str, bound: &str) -> Result<Vec<Block>> {
        let sql = format!(
            "SELECT {HYDRATED_BLOCK_COLUMNS} FROM {BLOCK_WRITE_TABLE} b WHERE b.{predicate} = \
             $bound ORDER BY b.sort_key, b.id"
        );
        let params = HashMap::from([("bound".to_string(), Value::String(bound.to_string()))]);
        let rows =
            self.db_handle.query(&sql, params).await.map_err(|e| {
                format!("{BLOCK_WRITE_TABLE} rows with {predicate} = '{bound}': {e}")
            })?;
        rows.into_iter()
            .map(|row| {
                Block::try_from(row).map_err(|e| {
                    format!("{BLOCK_WRITE_TABLE} row with {predicate} = '{bound}': {e:#}").into()
                })
            })
            .collect()
    }

    /// The sibling before `id` in its parent's order; `None` for a first
    /// child.
    async fn previous_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        // One seek on `idx_block_raw_parent_sort_id` to `id`'s slot, stepping
        // back over sort-key ties only.
        let t = |column: &str| format!("(SELECT {column} FROM {BLOCK_WRITE_TABLE} WHERE id = $id)");
        let sql = format!(
            "SELECT id FROM {BLOCK_WRITE_TABLE} WHERE parent_id = {parent} AND id != parent_id \
             AND sort_key <= {sort_key} AND (sort_key < {sort_key} OR id < $id) \
             ORDER BY sort_key DESC, id DESC LIMIT 1",
            parent = t("parent_id"),
            sort_key = t("sort_key"),
        );
        let params = HashMap::from([("id".to_string(), Value::String(id.to_string()))]);
        let rows = self
            .db_handle
            .query(&sql, params)
            .await
            .map_err(|e| format!("previous sibling of {id}: {e}"))?;
        match rows.first().map(|row| row.get("id")) {
            None => {
                if !self.block_exists(id).await? {
                    return Err(format!("previous sibling of {id}: no such block").into());
                }
                Ok(None)
            }
            Some(Some(Value::String(before))) => {
                Ok(Some(EntityUri::parse(before).map_err(|e| {
                    format!("previous sibling `{before}` of {id}: {e}")
                })?))
            }
            Some(other) => Err(format!("previous sibling of {id}: row id is {other:?}").into()),
        }
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

    async fn block(&self, id: &EntityUri) -> Result<Option<Block>> {
        Ok(self.blocks_where("id", id.as_str()).await?.pop())
    }

    async fn subtree(&self, root: &EntityUri) -> Result<Option<Vec<Block>>> {
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
                let parent = nodes[i].id.to_string();
                nodes.extend(self.blocks_where("parent_id", &parent).await?);
            }
            level = start..nodes.len();
        }
        Ok(Some(nodes))
    }

    async fn tagged_neighbourhood(
        &self,
        tags: &[&str],
        around: &[EntityUri],
        previous_of: Option<&EntityUri>,
    ) -> Result<Option<holon_core::TaggedNeighbourhood>> {
        let mut near = holon_core::TaggedNeighbourhood::default();
        let mut around = around.to_vec();
        if let Some(block) = previous_of
            && let Some(before) = self.previous_sibling(block).await?
        {
            around.push(before.clone());
            near.set_previous_sibling(before);
        }
        if around.is_empty() {
            return Ok(Some(near));
        }
        let ids = (0..around.len())
            .map(|i| format!("$id{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let tag_names = (0..tags.len())
            .map(|i| format!("$tag{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT b.id AS id, b.parent_id AS parent_id, p.id AS parent_found, \
             p.parent_id AS grandparent_id, tb.block_id AS tagged, tp.block_id AS parent_tagged, \
             tg.block_id AS grandparent_tagged \
             FROM {BLOCK_WRITE_TABLE} b \
             LEFT JOIN {BLOCK_WRITE_TABLE} p ON p.id = b.parent_id \
             LEFT JOIN block_tags tb ON tb.block_id = b.id AND tb.tag IN ({tag_names}) \
             LEFT JOIN block_tags tp ON tp.block_id = b.parent_id AND tp.tag IN ({tag_names}) \
             LEFT JOIN block_tags tg ON tg.block_id = p.parent_id AND tg.tag IN ({tag_names}) \
             WHERE b.id IN ({ids})"
        );
        let params = around
            .iter()
            .enumerate()
            .map(|(i, id)| (format!("id{i}"), Value::String(id.to_string())))
            .chain(
                tags.iter()
                    .enumerate()
                    .map(|(i, t)| (format!("tag{i}"), Value::String(t.to_string()))),
            )
            .collect();
        let rows = self
            .db_handle
            .query(&sql, params)
            .await
            .map_err(|e| format!("tagged neighbourhood of {around:?}: {e}"))?;
        for row in rows {
            let uri = |key: &str| match row.get(key) {
                Some(Value::Null) | None => Ok(None),
                Some(Value::String(raw)) => EntityUri::parse(raw)
                    .map(Some)
                    .map_err(|e| format!("tagged neighbourhood row {key} `{raw}`: {e}")),
                Some(other) => Err(format!("tagged neighbourhood row {key} is {other:?}")),
            };
            let id = uri("id")?.ok_or("tagged neighbourhood row without an id")?;
            let parent = uri("parent_id")?;
            if uri("tagged")?.is_some() {
                near.tagged(id.clone());
            }
            if let Some(parent) = &parent {
                if uri("parent_found")?.is_some() {
                    near.found(parent.clone(), uri("grandparent_id")?);
                }
                if uri("parent_tagged")?.is_some() {
                    near.tagged(parent.clone());
                }
                if let Some(grandparent) = uri("grandparent_id")?
                    && uri("grandparent_tagged")?.is_some()
                {
                    near.tagged(grandparent);
                }
            }
            near.found(id, parent);
        }
        for id in &around {
            if !near.covers(id) {
                near.missing(id.clone());
            }
        }
        let parents = around
            .iter()
            .map(|id| Ok(near.parent(id)?.cloned()))
            .collect::<Result<Vec<_>>>()?;
        for parent in parents.into_iter().flatten() {
            if !near.covers(&parent) {
                near.missing(parent);
            }
        }
        Ok(Some(near))
    }

    async fn children(&self, parent: &EntityUri) -> Result<Vec<EntityUri>> {
        // The root sentinel's own row names itself as its parent.
        let sql = format!(
            "SELECT id FROM {BLOCK_WRITE_TABLE} WHERE parent_id = $parent AND id != $parent \
             ORDER BY sort_key, id"
        );
        let params = HashMap::from([("parent".to_string(), Value::String(parent.to_string()))]);
        let rows = self
            .db_handle
            .query(&sql, params)
            .await
            .map_err(|e| format!("children of {parent}: {e}"))?;
        if rows.is_empty() && !parent.is_no_parent() && !self.block_exists(parent).await? {
            return Err(
                format!("children of {parent}: the write authority holds no such block").into(),
            );
        }
        rows.into_iter()
            .map(|row| match row.get("id") {
                Some(Value::String(id)) => EntityUri::parse(id)
                    .map_err(|e| format!("child `{id}` of {parent}: {e}").into()),
                other => Err(format!("children of {parent}: row id is {other:?}").into()),
            })
            .collect()
    }
}
