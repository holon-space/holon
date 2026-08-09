//! Ordering for the SqlOnly block-CRUD authority.
//!
//! In SqlOnly mode the block content-write ops (`create` / `set_field` /
//! `delete`) are served by a bare [`SqlOperationProvider`], which knows nothing
//! about sibling order: `SqlBlockOperations` advertises only the STRUCTURAL ops
//! (see the provider-split note on `OperationDispatcher`). A `block.create`
//! therefore left `sort_key` on the column default, so two creation-slot
//! commits under one parent tied and the visible order collapsed to the id
//! tiebreak — the block you typed second appeared first. A
//! `set_field("parent_id")` re-parent had the matching hole: the moved block
//! kept the key it was minted in its OLD parent's sequence, which says nothing
//! about where it belongs among its new siblings.
//!
//! [`OrderedBlockCrud`] closes both at the composition seam rather than per-op:
//! it wraps the CRUD provider and gives every content write that changes a
//! block's placement a real position, delegating everything else untouched.
//! Under Loro this decorator is not installed — the tree owns order there and
//! `apply_create` derives the index from `position_after_block_id`.
//!
//! `delete` needs nothing: removing a row leaves its siblings' keys distinct
//! and correctly ordered.

use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::block_ordering::BlockOrdering;
use holon_core::block_ordering::MintedPosition;
use holon_core::block_ordering::OrderKeyMinting;

/// Wraps the SqlOnly block-CRUD provider so placement-changing writes carry a
/// real `sort_key`. `order_owner` is the SqlOnly order owner — the same
/// `SqlBlockOperations` minting path the structural ops use, so both agree on
/// what a position is.
pub struct OrderedBlockCrud {
    inner: Arc<dyn OperationProvider>,
    order_owner: Arc<SqlBlockOperations>,
    /// The SAME concrete provider as `inner`, kept typed so placement writes
    /// hand a `MintedPosition` to `create_row` / `place_row` directly — the
    /// re-keys travel TYPED, never as an `_order_rekeys` params key (Ruling B).
    sql: Arc<SqlOperationProvider>,
}

impl OrderedBlockCrud {
    pub fn new(
        inner: Arc<dyn OperationProvider>,
        order_owner: Arc<SqlBlockOperations>,
        sql: Arc<SqlOperationProvider>,
    ) -> Self {
        Self {
            inner,
            order_owner,
            sql,
        }
    }

    /// The position a block appended as `parent`'s last child must carry.
    /// `moving` is excluded from the anchor search so a re-parent does not
    /// try to anchor after itself.
    async fn append_key(
        &self,
        parent: &EntityUri,
        moving: Option<&EntityUri>,
    ) -> Result<MintedPosition> {
        let children = self.order_owner.children(parent).await?;
        let last = children
            .iter()
            .rev()
            .find(|id| Some(*id) != moving.as_ref().copied());
        // ALLOW(order_minting): routed through the SqlOnly order owner's own
        // OrderKeyMinting seam (the same call split_block makes), not minted
        // locally — this decorator holds no keyspace of its own.
        self.order_owner.new_child_anchor(parent, last).await
    }
}

#[async_trait::async_trait]
impl OperationProvider for OrderedBlockCrud {
    fn operations(&self) -> Vec<OperationDescriptor> {
        self.inner.operations()
    }

    fn get_last_created_id(&self) -> Option<String> {
        self.inner.get_last_created_id()
    }

    fn identity_minter(&self) -> Option<&dyn holon_api::identity_minting::IdentityMinting> {
        self.inner.identity_minter()
    }

    /// Forwarded like every other non-ordering method: this decorator only
    /// takes a position on placement. Defaulting instead would answer `None`
    /// for a provider that can read the row, silently blinding the
    /// dispatcher's mark-extraction to the CRUD authority underneath.
    async fn read_block_content_marks(&self, id: &str) -> Result<Option<(String, Value)>> {
        self.inner.read_block_content_marks(id).await
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name.as_str() != "block" {
            return self
                .inner
                .execute_operation(entity_name, op_name, params)
                .await;
        }

        match op_name {
            // A create that names its parent and does not already carry an
            // explicit position is appended as that parent's last child — the
            // creation slot's contract ("type here" adds to the end).
            "create" if !params.contains_key("sort_key") => {
                let position = if let Some(parent) = params
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
                {
                    // ALLOW(entity_uri_from_raw): op-dispatch parent id string → EntityUri
                    let parent = EntityUri::from_raw(&parent);
                    // Key AND any sibling re-key it is expressed against, carried
                    // TYPED into the create's transaction (ADR 0030 D1) — never
                    // an `_order_rekeys` params key (Ruling B).
                    Some(self.append_key(&parent, None).await?)
                } else {
                    None
                };
                self.sql.create_row(params, position).await
            }
            // A re-parent moves the block into a sequence its old key has no
            // meaning in, so it is appended to the new parent. The position is
            // decided while the mover is still absent from the new sibling set,
            // then parent, key and sibling re-keys are written as ONE placement
            // — split across transactions, a mid-failure left the block
            // re-parented under a key from its old parent's sequence
            // (ADR 0030 D1).
            "set_field" if params.get("field").and_then(|v| v.as_string()) == Some("parent_id") => {
                let (Some(id), Some(new_parent)) = (
                    params.get("id").and_then(|v| v.as_string()),
                    params.get("value").and_then(|v| v.as_string()),
                ) else {
                    return self
                        .inner
                        .execute_operation(entity_name, op_name, params)
                        .await;
                };
                // ALLOW(entity_uri_from_raw): op-dispatch id strings → EntityUri
                let uri = EntityUri::from_raw(id);
                // ALLOW(entity_uri_from_raw): op-dispatch id strings → EntityUri
                let parent = EntityUri::from_raw(new_parent);
                let position = self.append_key(&parent, Some(&uri)).await?;
                // Parent + key + sibling re-keys are ONE placement in ONE
                // transaction; `place_row` takes the `MintedPosition` whole, so
                // the re-keys travel TYPED (ADR 0030 D1/D4, amended).
                self.sql
                    .place_row(uri.as_str(), parent.as_str(), position)
                    .await
            }
            _ => {
                self.inner
                    .execute_operation(entity_name, op_name, params)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use holon::core::queryable_cache::QueryableCache;
    use holon::core::sql_operation_provider::SqlOperationProvider;
    use holon::storage::BLOCK_WRITE_TABLE;
    use holon::storage::schema_module::SchemaModule;
    use holon::storage::turso::DbHandle;
    use holon::storage::turso::TursoBackend;
    use holon_api::block::Block;
    use holon_turso::schema_modules::BlockSchemaModule;

    use super::*;

    async fn setup() -> (DbHandle, OrderedBlockCrud) {
        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        holon_turso::schema_modules::CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("core schema");
        BlockSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("block schema");

        let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
            handle.clone(),
            BLOCK_WRITE_TABLE.to_string(),
            "block".to_string(),
            "block".to_string(),
            BlockSchemaModule.edge_fields(),
        ));
        let mut block_raw_type_def = Block::type_definition();
        block_raw_type_def.name = "block_raw".to_string();
        let cache = Arc::new(
            QueryableCache::<Block>::new(handle.clone(), block_raw_type_def)
                .await
                .expect("cache"),
        );
        let order_owner = Arc::new(SqlBlockOperations::new(sql_ops.clone(), cache));
        let provider = OrderedBlockCrud::new(
            sql_ops.clone() as Arc<dyn OperationProvider>,
            order_owner,
            sql_ops,
        );
        (handle, provider)
    }

    async fn insert_raw(handle: &DbHandle, id: &str, parent: &str, sort_key: &str) {
        handle
            .execute(
                &format!(
                    // ALLOW(sole_block_writer): test fixture seeding rows the
                    // decorator then reads as pre-existing siblings (#[cfg(test)]).
                    "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                     created_at, updated_at) VALUES ('{id}', '{parent}', '{sort_key}', '{id}', \
                     'text', 0, 0)"
                ),
                vec![],
            )
            .await
            .unwrap_or_else(|e| panic!("insert {id}: {e}"));
    }

    async fn order_under(handle: &DbHandle, parent: &str) -> Vec<String> {
        handle
            .query(
                &format!(
                    "SELECT id FROM block_raw WHERE parent_id = '{parent}' ORDER BY sort_key, id"
                ),
                std::collections::HashMap::new(),
            )
            .await
            .expect("read order")
            .into_iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string().map(str::to_string)))
            .collect()
    }

    /// The re-parent hole: a bare CRUD provider writes `parent_id` and nothing
    /// else, so the moved block keeps the key it was minted in its OLD parent's
    /// sequence — a value that says nothing about where it belongs among its
    /// new siblings. Here the old key sorts BEFORE both new siblings, so
    /// without the decorator the moved block jumps to the front of a parent it
    /// has just joined.
    #[tokio::test]
    async fn a_re_parent_appends_the_moved_block_to_its_new_siblings() {
        let (handle, provider) = setup().await;

        insert_raw(&handle, "block:old-parent", "sentinel:no_parent", "80").await;
        insert_raw(&handle, "block:new-parent", "sentinel:no_parent", "8180").await;
        // Minted first in the old parent, so it carries the LOWEST key.
        insert_raw(&handle, "block:mover", "block:old-parent", "80").await;
        insert_raw(&handle, "block:kept-a", "block:new-parent", "8180").await;
        insert_raw(&handle, "block:kept-b", "block:new-parent", "8280").await;

        let entity = EntityName::new("block");
        let mut params: StorageEntity = StorageEntity::new();
        params.insert("id".into(), Value::String("block:mover".to_string()));
        params.insert("field".into(), Value::String("parent_id".to_string()));
        params.insert(
            "value".into(),
            Value::String("block:new-parent".to_string()),
        );
        provider
            .execute_operation(&entity, "set_field", params)
            .await
            .expect("re-parent the mover");

        assert_eq!(
            order_under(&handle, "block:new-parent").await,
            vec![
                "block:kept-a".to_string(),
                "block:kept-b".to_string(),
                "block:mover".to_string()
            ],
            "a re-parented block must be appended to its new siblings, not carry its old \
             parent's key into their sequence"
        );
    }

    /// An inner provider that answers `read_block_content_marks` and nothing
    /// else — enough to tell a forward from the trait's `None` default.
    struct MarksReader;

    #[async_trait::async_trait]
    impl OperationProvider for MarksReader {
        fn operations(&self) -> Vec<OperationDescriptor> {
            Vec::new()
        }

        async fn execute_operation(
            &self,
            _: &EntityName,
            _: &str,
            _: StorageEntity,
        ) -> Result<OperationResult> {
            unreachable!("this fixture only answers reads")
        }

        async fn read_block_content_marks(&self, id: &str) -> Result<Option<(String, Value)>> {
            Ok(Some((
                format!("content of {id}"),
                Value::String("[marks]".to_string()),
            )))
        }
    }

    /// The decorator must not answer for a method it has no opinion about.
    /// Taking the trait's `None` default here would tell the dispatcher the
    /// CRUD authority cannot read block marks, when in fact it can.
    #[tokio::test]
    async fn read_block_content_marks_reaches_the_inner_provider() {
        let (_handle, sql_backed) = setup().await;
        let decorated = OrderedBlockCrud::new(
            Arc::new(MarksReader),
            sql_backed.order_owner,
            sql_backed.sql,
        );

        assert_eq!(
            decorated
                .read_block_content_marks("block:x")
                .await
                .expect("forwarded read"),
            Some((
                "content of block:x".to_string(),
                Value::String("[marks]".to_string())
            )),
            "the decorator must forward reads to the provider it wraps, not answer None"
        );
    }

    /// The create hole: `block.create` reaches the bare CRUD provider with no
    /// `sort_key`, so two consecutive creates under one parent both take the
    /// column default, tie, and the visible order falls through to the id
    /// tiebreak — the block created SECOND sorts first whenever its id happens
    /// to be lexicographically smaller. This is the `page-id-rename-collision`
    /// divergence in miniature.
    #[tokio::test]
    async fn consecutive_creates_are_ordered_by_creation_not_by_id() {
        let (handle, provider) = setup().await;

        insert_raw(&handle, "block:page", "sentinel:no_parent", "80").await;

        let entity = EntityName::new("block");
        // "zzz" is created FIRST but sorts after "aaa" by id, so an id
        // tiebreak would invert them.
        for id in ["block:zzz", "block:aaa"] {
            let mut params: StorageEntity = StorageEntity::new();
            params.insert("id".into(), Value::String(id.to_string()));
            params.insert("parent_id".into(), Value::String("block:page".to_string()));
            params.insert("content".into(), Value::String(id.to_string()));
            params.insert("content_type".into(), Value::String("text".to_string()));
            provider
                .execute_operation(&entity, "create", params)
                .await
                .unwrap_or_else(|e| panic!("create {id}: {e}"));
        }

        assert_eq!(
            order_under(&handle, "block:page").await,
            vec!["block:zzz".to_string(), "block:aaa".to_string()],
            "creation order must win; an id tiebreak means both rows took the column default"
        );
    }

    /// The re-key half of a create, end to end through this decorator.
    ///
    /// Appending under a parent whose children are NOT an insertable sequence
    /// (here the SQL column default) has to re-mint them, and those re-keys
    /// ride the create into its own transaction (ADR 0030 D1). Without them
    /// the new block ties with the unkeyed row and the order falls through
    /// to the id tiebreak — so this test fails if the re-keys are dropped
    /// anywhere between the mint and the SQL writer.
    #[tokio::test]
    async fn a_create_carries_its_sibling_rekeys_all_the_way_into_the_row() {
        let (handle, provider) = setup().await;
        let entity: EntityName = EntityName::new("block");

        insert_raw(&handle, "block:page", "sentinel:no_parent", "80").await;
        // Unkeyed: not a position, so the append must re-key it in place.
        insert_raw(&handle, "block:unkeyed", "block:page", "A0").await;

        let mut params: StorageEntity = StorageEntity::new();
        params.insert("id".into(), Value::String("block:appended".to_string()));
        params.insert("parent_id".into(), Value::String("block:page".to_string()));
        params.insert("content".into(), Value::String("appended".to_string()));
        params.insert("content_type".into(), Value::String("text".to_string()));
        provider
            .execute_operation(&entity, "create", params)
            .await
            .expect("append under an unkeyed sibling set");

        assert_eq!(
            order_under(&handle, "block:page").await,
            vec!["block:unkeyed".to_string(), "block:appended".to_string()],
            "the appended block must sort LAST, which requires the unkeyed sibling to have been \
             re-keyed by the same write"
        );
        let keys = sort_keys_under(&handle, "block:page").await;
        assert!(
            keys.iter().all(|k| k != "A0"),
            "every sibling must hold a real position after the append, got {keys:?}"
        );
    }

    /// The same for a RE-PARENT, which this decorator turns into one `place`.
    ///
    /// The mover joins a sibling set that is not an insertable sequence, so the
    /// placement carries re-keys too — and parent, key and re-keys must all
    /// land together.
    #[tokio::test]
    async fn a_reparent_carries_its_sibling_rekeys_into_one_placement() {
        let (handle, provider) = setup().await;
        let entity: EntityName = EntityName::new("block");

        insert_raw(&handle, "block:from", "sentinel:no_parent", "80").await;
        insert_raw(&handle, "block:to", "sentinel:no_parent", "8180").await;
        insert_raw(&handle, "block:mover", "block:from", "80").await;
        insert_raw(&handle, "block:unkeyed", "block:to", "A0").await;

        let mut params: StorageEntity = StorageEntity::new();
        params.insert("id".into(), Value::String("block:mover".to_string()));
        params.insert("field".into(), Value::String("parent_id".to_string()));
        params.insert("value".into(), Value::String("block:to".to_string()));
        provider
            .execute_operation(&entity, "set_field", params)
            .await
            .expect("re-parent into an unkeyed sibling set");

        assert_eq!(
            order_under(&handle, "block:to").await,
            vec!["block:unkeyed".to_string(), "block:mover".to_string()],
            "the moved block is appended after its new siblings, which requires the unkeyed one \
             to have been re-keyed by the same placement"
        );
        assert!(
            order_under(&handle, "block:from").await.is_empty(),
            "the mover left its old parent"
        );
        let keys = sort_keys_under(&handle, "block:to").await;
        assert!(
            keys.iter().all(|k| k != "A0"),
            "every sibling must hold a real position after the move, got {keys:?}"
        );
    }

    async fn sort_keys_under(handle: &DbHandle, parent: &str) -> Vec<String> {
        handle
            .query(
                &format!(
                    "SELECT sort_key FROM block_raw WHERE parent_id = '{parent}' ORDER BY \
                     sort_key, id"
                ),
                std::collections::HashMap::new(),
            )
            .await
            .expect("read keys")
            .into_iter()
            .filter_map(|r| {
                r.get("sort_key")
                    .and_then(|v| v.as_string().map(str::to_string))
            })
            .collect()
    }
}
