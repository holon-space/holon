//! The page a block belongs to: its nearest `Page`-tagged ancestor, the block
//! itself included.

use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use holon_api::EntityUri;
use holon_api::Value;

use crate::storage::DbHandle;

/// Base tables only, never a matview, so a block written a moment ago already
/// resolves. The self-parented no-parent sentinel ends the walk.
const NEAREST_PAGE_SQL: &str = "WITH RECURSIVE up(id, parent_id, depth) AS ( \
     SELECT id, parent_id, 0 FROM block_raw WHERE id = $start \
     UNION ALL \
     SELECT b.id, b.parent_id, up.depth + 1 FROM block_raw b JOIN up ON b.id = up.parent_id \
     WHERE up.id != up.parent_id AND up.depth < 1024 \
   ) \
   SELECT up.id AS id FROM up \
   WHERE EXISTS (SELECT 1 FROM block_tags t WHERE t.block_id = up.id AND t.tag = 'Page') \
   ORDER BY up.depth LIMIT 1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRef(EntityUri);

impl PageRef {
    /// The nearest page ancestor of `start`. Fails when `start` has none.
    pub async fn nearest(db: &DbHandle, start: &EntityUri) -> Result<Self> {
        let params = HashMap::from([(
            "start".to_string(),
            Value::String(start.as_str().to_string()),
        )]);
        let rows = db
            .query(NEAREST_PAGE_SQL, params)
            .await
            .with_context(|| format!("nearest page of {start}: read failed"))?;
        let id = rows
            .first()
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_string())
            .with_context(|| format!("{start} has no page ancestor"))?;
        Ok(Self(EntityUri::parse(id)?))
    }

    pub fn id(&self) -> &EntityUri {
        &self.0
    }
}
