//! Tests for BlockOperations default implementations (indent, outdent,
//! move_block, etc.)

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use holon_api::EntityUri;
    use holon_api::Value;

    use crate::block_ordering::BlockOrdering;
    use crate::block_ordering::OrderKeyMinting;
    use crate::fractional_index::gen_key_between;
    use crate::traits::*;

    /// Canonical block-id form used for matching inside the synthetic store.
    /// Production ids are scheme-qualified (`block:UUID`); `BlockOperations`
    /// now hands the store `EntityUri` values whose `as_str()` carries that
    /// scheme. Test fixtures still spell ids bare (`"A"`), so we compare on
    /// the canonical `block:`-prefixed form to accept either spelling.
    fn canon(id: &str) -> String {
        // ALLOW(entity_uri_from_raw): canon() parses bare test-fixture id literal into
        // canonical block form
        EntityUri::from_raw(id).as_str().to_string()
    }

    #[derive(Debug, Clone)]
    struct TestBlock {
        id: EntityUri,
        parent_id: Option<EntityUri>,
        sort_key: String,
        depth: i64,
        content: String,
    }

    impl BlockEntity for TestBlock {
        fn id(&self) -> &EntityUri {
            &self.id
        }
        fn parent_id(&self) -> Option<&EntityUri> {
            self.parent_id.as_ref()
        }
        fn depth(&self) -> i64 {
            self.depth
        }
        fn content(&self) -> &str {
            &self.content
        }
        fn tags(&self) -> holon_api::Tags {
            holon_api::Tags::default()
        }
    }

    /// In-memory block store for testing
    struct MemStore {
        blocks: Mutex<Vec<TestBlock>>,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                blocks: Mutex::new(Vec::new()),
            }
        }

        fn insert(&self, block: TestBlock) {
            self.blocks.lock().unwrap().push(block);
        }

        fn get(&self, id: &str) -> Option<TestBlock> {
            let want = canon(id);
            self.blocks
                .lock()
                .unwrap()
                .iter()
                .find(|b| b.id.as_str() == want)
                .cloned()
        }

        fn sorted_children(&self, parent_id: &str) -> Vec<TestBlock> {
            let want = canon(parent_id);
            let blocks = self.blocks.lock().unwrap();
            let mut children: Vec<TestBlock> = blocks
                .iter()
                .filter(|b| {
                    b.parent_id.as_ref().map(|p| p.as_str().to_string()) == Some(want.clone())
                })
                .cloned()
                .collect();
            children.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
            children
        }
    }

    #[async_trait]
    impl DataSource<TestBlock> for MemStore {
        async fn get_all(&self) -> Result<Vec<TestBlock>> {
            Ok(self.blocks.lock().unwrap().clone())
        }
        async fn get_by_id(&self, id: &str) -> Result<Option<TestBlock>> {
            Ok(self.get(id))
        }
        async fn get_children(&self, parent_id: &EntityUri) -> Result<Vec<TestBlock>> {
            // Override the default (exact-string-match) filter so the
            // synthetic store's bare fixture ids match a scheme-qualified
            // `parent_id` via `canon`, mirroring `sorted_children`.
            Ok(self.sorted_children(parent_id.as_str()))
        }
    }

    #[async_trait]
    impl CrudOperations<TestBlock> for MemStore {
        async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
            let want = canon(id);
            let mut blocks = self.blocks.lock().unwrap();
            let block = blocks.iter_mut().find(|b| b.id.as_str() == want).unwrap();
            let old_value = match field {
                "parent_id" => block
                    .parent_id
                    .as_ref()
                    .map_or(Value::Null, |v| Value::String(v.as_str().to_string())),
                "sort_key" => Value::String(block.sort_key.clone()),
                "depth" => Value::Integer(block.depth),
                "content" => Value::String(block.content.clone()),
                _ => Value::Null,
            };
            match field {
                // ALLOW(entity_uri_from_raw): test set_field boundary — parent_id value arrives as
                // a raw string.
                "parent_id" => block.parent_id = value.as_string().map(EntityUri::from_raw),
                "sort_key" => block.sort_key = value.as_string().unwrap().to_string(),
                "depth" => block.depth = value.as_i64().unwrap(),
                "content" => block.content = value.as_string().unwrap().to_string(),
                _ => {}
            }
            Ok(OperationResult::new(
                vec![FieldDelta::new(id, field, old_value, value)],
                holon_api::Operation::new("test", "set_field", "set_field", HashMap::new()),
            ))
        }

        async fn create(
            &self,
            fields: crate::storage::types::StorageEntity,
        ) -> Result<(String, OperationResult)> {
            let id = fields
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap()
                .to_string();
            let block = TestBlock {
                // ALLOW(entity_uri_from_raw): test create() boundary — id/parent_id arrive as raw
                // fixture strings.
                id: EntityUri::from_raw(&id),
                parent_id: fields
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    // ALLOW(entity_uri_from_raw): test create() boundary — see `id` above.
                    .map(EntityUri::from_raw),
                sort_key: fields
                    .get("sort_key")
                    .and_then(|v| v.as_string())
                    .unwrap_or("A0")
                    .to_string(),
                depth: fields.get("depth").and_then(|v| v.as_i64()).unwrap_or(0),
                content: fields
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
            };
            self.blocks.lock().unwrap().push(block);
            Ok((id, OperationResult::irreversible(vec![])))
        }

        async fn delete(&self, id: &str) -> Result<OperationResult> {
            let want = canon(id);
            self.blocks
                .lock()
                .unwrap()
                .retain(|b| b.id.as_str() != want);
            Ok(OperationResult::irreversible(vec![]))
        }
    }

    #[async_trait]
    impl BlockQueryHelpers<TestBlock> for MemStore {
        async fn children_ordered(&self, parent_id: &EntityUri) -> Result<Vec<TestBlock>> {
            Ok(self.sorted_children(parent_id.as_str()))
        }

        async fn get_prev_sibling(&self, block_id: &EntityUri) -> Result<Option<TestBlock>> {
            match <Self as BlockOrdering>::prev_sibling(self, block_id).await? {
                Some(id) => self.get_by_id(id.as_str()).await,
                None => Ok(None),
            }
        }

        async fn get_next_sibling(&self, block_id: &EntityUri) -> Result<Option<TestBlock>> {
            match <Self as BlockOrdering>::next_sibling(self, block_id).await? {
                Some(id) => self.get_by_id(id.as_str()).await,
                None => Ok(None),
            }
        }

        async fn get_first_child(
            &self,
            parent_id: Option<&EntityUri>,
        ) -> Result<Option<TestBlock>> {
            let Some(pid) = parent_id else {
                return Ok(None);
            };
            match <Self as BlockOrdering>::first_child(self, pid).await? {
                Some(id) => self.get_by_id(id.as_str()).await,
                None => Ok(None),
            }
        }

        async fn get_last_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<TestBlock>> {
            let Some(pid) = parent_id else {
                return Ok(None);
            };
            match <Self as BlockOrdering>::last_child(self, pid).await? {
                Some(id) => self.get_by_id(id.as_str()).await,
                None => Ok(None),
            }
        }
    }
    impl BlockMaintenanceHelpers<TestBlock> for MemStore {}
    impl BlockDataSourceHelpers<TestBlock> for MemStore {}
    impl BlockOperations<TestBlock> for MemStore {
        fn ordering(&self) -> Option<&dyn BlockOrdering> {
            Some(self as &dyn BlockOrdering)
        }
        fn order_key_minter(&self) -> Option<&dyn OrderKeyMinting> {
            Some(self as &dyn OrderKeyMinting)
        }
    }

    #[async_trait]
    impl OrderKeyMinting for MemStore {
        async fn new_child_anchor(
            &self,
            parent_id: &EntityUri,
            after_id: Option<&EntityUri>,
        ) -> Result<String> {
            let (prev_key, next_key) = match after_id {
                None => {
                    let first = self.sorted_children(parent_id.as_str()).into_iter().next();
                    (None, first.map(|b| b.sort_key))
                }
                Some(after) => {
                    let after_block = self
                        .get(after.as_str())
                        .ok_or_else(|| anyhow::anyhow!("MemStore: after block {after} missing"))?;
                    let after_parent = after_block.parent_id.clone();
                    let next_sib = self
                        .sorted_children(after_parent.as_ref().map(|u| u.as_str()).unwrap_or(""))
                        .into_iter()
                        .find(|b| b.sort_key > after_block.sort_key);
                    (Some(after_block.sort_key), next_sib.map(|b| b.sort_key))
                }
            };
            gen_key_between(prev_key.as_deref(), next_key.as_deref())
                .map_err(|e| format!("{e:#}").into())
        }
    }

    #[async_trait]
    impl BlockOrdering for MemStore {
        async fn place(
            &self,
            uri: &EntityUri,
            parent_id: &EntityUri,
            after_id: Option<&EntityUri>,
        ) -> Result<()> {
            let new_sort_key = self.new_child_anchor(parent_id, after_id).await?;
            let want = canon(uri.as_str());
            let mut blocks = self.blocks.lock().unwrap();
            let block = blocks
                .iter_mut()
                .find(|b| b.id.as_str() == want)
                .ok_or_else(|| anyhow::anyhow!("MemStore::place: block {want} not found"))?;
            block.parent_id = Some(parent_id.clone());
            block.sort_key = new_sort_key;
            Ok(())
        }

        async fn prev_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
            let block = self
                .get(id.as_str())
                .ok_or_else(|| anyhow::anyhow!("prev_sibling: block {id} missing"))?;
            let Some(parent_id) = block.parent_id.as_ref() else {
                return Ok(None);
            };
            Ok(self
                .sorted_children(parent_id.as_str())
                .into_iter()
                .rfind(|b| b.sort_key < block.sort_key)
                .map(|b| b.id.clone()))
        }

        async fn next_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
            let block = self
                .get(id.as_str())
                .ok_or_else(|| anyhow::anyhow!("next_sibling: block {id} missing"))?;
            let Some(parent_id) = block.parent_id.as_ref() else {
                return Ok(None);
            };
            Ok(self
                .sorted_children(parent_id.as_str())
                .into_iter()
                .find(|b| b.sort_key > block.sort_key)
                .map(|b| b.id.clone()))
        }

        async fn first_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
            Ok(self
                .sorted_children(parent_id.as_str())
                .into_iter()
                .next()
                .map(|b| b.id.clone()))
        }

        async fn last_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
            Ok(self
                .sorted_children(parent_id.as_str())
                .into_iter()
                .last()
                .map(|b| b.id.clone()))
        }

        async fn children(&self, parent_id: &EntityUri) -> Result<Vec<EntityUri>> {
            Ok(self
                .sorted_children(parent_id.as_str())
                .into_iter()
                .map(|b| b.id.clone())
                .collect())
        }

        async fn update_in_tree(&self, params: holon_api::StorageEntity) -> Result<()> {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .ok_or_else(|| anyhow::anyhow!("MemStore::update_in_tree: missing id"))?
                .to_string();
            let mut blocks = self.blocks.lock().unwrap();
            let block = match blocks.iter_mut().find(|b| b.id.as_str() == id) {
                Some(b) => b,
                None => {
                    drop(blocks);
                    self.insert(TestBlock {
                        // ALLOW(entity_uri_from_raw): test update_in_tree boundary — id/parent_id
                        // arrive as raw strings.
                        id: EntityUri::from_raw(&id),
                        parent_id: params
                            .get("parent_id")
                            .and_then(|v| v.as_string())
                            // ALLOW(entity_uri_from_raw): test update_in_tree boundary — see `id`
                            // above.
                            .map(EntityUri::from_raw),
                        sort_key: gen_key_between(None, None).map_err(|e| format!("{e:#}"))?,
                        depth: 0,
                        content: params
                            .get("content")
                            .and_then(|v| v.as_string())
                            .unwrap_or_default()
                            .to_string(),
                    });
                    return Ok(());
                }
            };
            if let Some(c) = params.get("content").and_then(|v| v.as_string()) {
                block.content = c.to_string();
            }
            if let Some(p) = params.get("parent_id").and_then(|v| v.as_string()) {
                // ALLOW(entity_uri_from_raw): test update_in_tree boundary — parent_id arrives
                // as a raw string.
                block.parent_id = Some(EntityUri::from_raw(p));
            }
            Ok(())
        }

        async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> Result<()> {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .ok_or_else(|| anyhow::anyhow!("MemStore::delete_in_tree: missing id"))?
                .to_string();
            self.blocks.lock().unwrap().retain(|b| b.id.as_str() != id);
            Ok(())
        }
    }

    fn insert_block(store: &MemStore, id: &str, parent_id: Option<&str>, prev_key: Option<&str>) {
        let sort_key = gen_key_between(prev_key, None).unwrap();
        let depth: i64 = if parent_id.is_some() { 1 } else { 0 };
        store.insert(TestBlock {
            id: EntityUri::block(id),
            parent_id: parent_id.map(EntityUri::block),
            sort_key,
            depth,
            content: format!("Content {}", id),
        });
    }

    #[tokio::test]
    async fn move_block_to_beginning() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_b;
        {
            let children = store.sorted_children("P");
            key_b = children.last().map(|c| c.sort_key.clone());
        }
        insert_block(&store, "B", Some("P"), key_b.as_deref());
        insert_block(&store, "C", Some("P"), {
            let children = store.sorted_children("P");
            children
                .last()
                .map(|c| c.sort_key.as_str().to_string())
                .as_deref()
        });

        // Move C to beginning
        store
            .move_block(&EntityUri::block("C"), &EntityUri::block("P"), None)
            .await
            .unwrap();

        let children = store.sorted_children("P");
        assert_eq!(children[0].id, EntityUri::block("C"));
        assert_eq!(children[1].id, EntityUri::block("A"));
        assert_eq!(children[2].id, EntityUri::block("B"));
    }

    #[tokio::test]
    async fn move_block_after_specific() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));
        let key_b = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "C", Some("P"), Some(&key_b));

        // Move A after B
        store
            .move_block(
                &EntityUri::block("A"),
                &EntityUri::block("P"),
                Some(&EntityUri::block("B")),
            )
            .await
            .unwrap();

        let children = store.sorted_children("P");
        assert_eq!(children[0].id, EntityUri::block("B"));
        assert_eq!(children[1].id, EntityUri::block("A"));
        assert_eq!(children[2].id, EntityUri::block("C"));
    }

    #[tokio::test]
    async fn indent_moves_under_prev_sibling() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));

        // Indent B (resolves previous sibling A as new parent)
        store.indent(&EntityUri::block("B")).await.unwrap();

        let b = store.get("B").unwrap();
        assert_eq!(b.parent_id, Some(EntityUri::block("A")));
        assert_eq!(b.depth, 2); // A is depth 1, so B becomes depth 2
    }

    #[tokio::test]
    async fn outdent_moves_to_grandparent() {
        let store = MemStore::new();
        insert_block(&store, "GP", None, None);
        insert_block(&store, "P", Some("GP"), None);

        // B is child of P, depth 2
        let b = TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 2,
            content: "Content B".to_string(),
        };
        store.insert(b);

        // Outdent B: should move to GP level, after P
        store.outdent(&EntityUri::block("B")).await.unwrap();

        let b = store.get("B").unwrap();
        assert_eq!(b.parent_id, Some(EntityUri::block("GP")));
        assert_eq!(b.depth, 1); // GP is depth 0, so B becomes depth 1
    }

    #[tokio::test]
    async fn outdent_root_block_fails() {
        let store = MemStore::new();
        insert_block(&store, "R", None, None);

        let result = store.outdent(&EntityUri::block("R")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn move_up_swaps_with_prev_sibling() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));

        // Move B up (before A)
        store.move_up(&EntityUri::block("B")).await.unwrap();

        let children = store.sorted_children("P");
        assert_eq!(children[0].id, EntityUri::block("B"));
        assert_eq!(children[1].id, EntityUri::block("A"));
    }

    #[tokio::test]
    async fn move_up_first_child_fails() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);

        let result = store.move_up(&EntityUri::block("A")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn move_down_swaps_with_next_sibling() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));

        // Move A down (after B)
        store.move_down(&EntityUri::block("A")).await.unwrap();

        let children = store.sorted_children("P");
        assert_eq!(children[0].id, EntityUri::block("B"));
        assert_eq!(children[1].id, EntityUri::block("A"));
    }

    #[tokio::test]
    async fn move_down_last_child_fails() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);

        let result = store.move_down(&EntityUri::block("A")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn split_block_divides_content() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "Hello World".to_string(),
        });

        store.split_block(&EntityUri::block("A"), 5).await.unwrap();

        let a = store.get("block:A").unwrap();
        assert_eq!(a.content, "Hello");

        // Find the new block (not A, not P, child of P)
        let children = store.sorted_children("P");
        assert_eq!(children.len(), 2); // A and the new block
        let new_block = children
            .iter()
            .find(|b| b.id.as_str() != "block:A")
            .unwrap();
        assert_eq!(new_block.content, "World");
    }

    #[tokio::test]
    async fn split_block_at_start() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "Hello".to_string(),
        });

        store.split_block(&EntityUri::block("A"), 0).await.unwrap();

        let a = store.get("block:A").unwrap();
        assert_eq!(a.content, "");

        let children = store.sorted_children("P");
        let new_block = children
            .iter()
            .find(|b| b.id.as_str() != "block:A")
            .unwrap();
        assert_eq!(new_block.content, "Hello");
    }

    #[tokio::test]
    async fn split_block_at_end_of_content_succeeds() {
        // Enter at end-of-line: position == content.len() is the most common
        // split in practice and must be accepted (only position > len errors).
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "Hello".to_string(),
        });

        store.split_block(&EntityUri::block("A"), 5).await.unwrap();

        let a = store.get("A").unwrap();
        assert_eq!(a.content, "Hello");
        let children = store.sorted_children("P");
        assert_eq!(children.len(), 2);
        let new_block = children
            .iter()
            .find(|b| b.id.as_str() != "block:A")
            .unwrap();
        assert_eq!(new_block.content, "");
    }

    #[tokio::test]
    async fn split_block_invalid_position_fails() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "Hi".to_string(),
        });

        let result = store.split_block(&EntityUri::block("A"), 10).await;
        assert!(result.is_err());

        let result = store.split_block(&EntityUri::block("A"), -1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn join_block_into_prev_sibling_concatenates_content() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "foo".to_string(),
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            depth: 1,
            content: "bar".to_string(),
        });

        store.join_block(&EntityUri::block("B"), 0).await.unwrap();

        let a = store.get("A").unwrap();
        assert_eq!(a.content, "foobar");
        assert!(store.get("B").is_none());
        let children = store.sorted_children("P");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, EntityUri::block("A"));
    }

    #[tokio::test]
    async fn join_block_into_parent_when_first_child() {
        // Layout:
        //   P (content "parent ")
        //     A (content "child")  <- first child, no prev sibling
        //     B (content "sib1")
        //     C (content "sib2")
        // After `join_block("A", 0)`:
        //   P (content "parent child")
        //     B (content "sib1")
        //     C (content "sib2")
        let store = MemStore::new();
        store.insert(TestBlock {
            id: EntityUri::block("P"),
            parent_id: None,
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 0,
            content: "parent ".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "child".to_string(),
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            depth: 1,
            content: "sib1".to_string(),
        });
        let key_b = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("C"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_b), None).unwrap(),
            depth: 1,
            content: "sib2".to_string(),
        });

        store.join_block(&EntityUri::block("A"), 0).await.unwrap();

        let p = store.get("P").unwrap();
        assert_eq!(p.content, "parent child");
        assert!(store.get("A").is_none());
        let children = store.sorted_children("P");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].id, EntityUri::block("B"));
        assert_eq!(children[1].id, EntityUri::block("C"));
    }

    #[tokio::test]
    async fn join_block_into_parent_with_grandchildren_is_refused() {
        // Phase 3.5: matches LogSeq's behavior — a first-child block with
        // its own subtree cannot be joined into its parent (Backspace at
        // the start of `A` is a no-op when `A` has children). Previously
        // we re-parented grandchildren to A's slot; now we refuse.
        //
        // Layout:
        //   P (content "parent ")
        //     A (content "child")  <- first child, has its own children X, Y
        //       X (content "x")
        //       Y (content "y")
        //     B (content "sib")
        // After `join_block("A", 0)`: unchanged.
        let store = MemStore::new();
        store.insert(TestBlock {
            id: EntityUri::block("P"),
            parent_id: None,
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 0,
            content: "parent ".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "child".to_string(),
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            depth: 1,
            content: "sib".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("X"),
            parent_id: Some(EntityUri::block("A")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 2,
            content: "x".to_string(),
        });
        let key_x = store.sorted_children("A").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("Y"),
            parent_id: Some(EntityUri::block("A")),
            sort_key: gen_key_between(Some(&key_x), None).unwrap(),
            depth: 2,
            content: "y".to_string(),
        });

        store.join_block(&EntityUri::block("A"), 0).await.unwrap();

        // Parent content unchanged, A still alive, grandchildren still
        // under A — nothing moved.
        let p = store.get("P").unwrap();
        assert_eq!(p.content, "parent ");
        assert!(store.get("A").is_some(), "A must remain present");
        let p_children = store.sorted_children("P");
        assert_eq!(
            p_children.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["block:A", "block:B"],
            "A and B remain as P's children in original order"
        );
        let a_children = store.sorted_children("A");
        assert_eq!(
            a_children.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["block:X", "block:Y"],
            "X and Y remain under A"
        );
    }

    #[tokio::test]
    async fn join_block_root_has_no_target_fails() {
        // Block with no prev sibling AND no parent → both fallbacks unavailable.
        let store = MemStore::new();
        store.insert(TestBlock {
            id: EntityUri::block("Root"),
            parent_id: None,
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 0,
            content: "alone".to_string(),
        });

        let result = store.join_block(&EntityUri::block("Root"), 0).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn move_block_returns_inverse() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));
        let key_b = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "C", Some("P"), Some(&key_b));

        let result = store
            .move_block(&EntityUri::block("C"), &EntityUri::block("P"), None)
            .await
            .unwrap();
        assert!(result.undo.is_reversible());
    }

    #[tokio::test]
    async fn indent_returns_inverse() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));

        let result = store.indent(&EntityUri::block("B")).await.unwrap();
        assert!(result.undo.is_reversible());
    }

    #[tokio::test]
    async fn descendant_depth_update_on_move() {
        let store = MemStore::new();
        insert_block(&store, "P1", None, None); // depth 0
        insert_block(&store, "P2", None, None); // depth 0

        // A is child of P1, depth 1
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P1")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "A".to_string(),
        });
        // B is child of A, depth 2
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("A")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 2,
            content: "B".to_string(),
        });

        // Move A under P2 (depth doesn't change since both parents are depth 0)
        store
            .move_block(&EntityUri::block("A"), &EntityUri::block("P2"), None)
            .await
            .unwrap();

        let a = store.get("A").unwrap();
        assert_eq!(a.parent_id, Some(EntityUri::block("P2")));
        assert_eq!(a.depth, 1);

        let b = store.get("B").unwrap();
        assert_eq!(b.depth, 2); // unchanged since depth delta is 0
    }

    #[tokio::test]
    async fn join_block_with_children_reparents_them_in_order_into_prev_sibling() {
        // Case A (prev sibling exists) with children: B's children X, Y must
        // be appended under A AFTER A's existing child W, in document order.
        // Layout:
        //   P
        //     A ("foo")
        //       W ("w")
        //     B ("bar")   <- join target
        //       X ("x")
        //       Y ("y")
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "foo".to_string(),
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            depth: 1,
            content: "bar".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("W"),
            parent_id: Some(EntityUri::block("A")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 2,
            content: "w".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("X"),
            parent_id: Some(EntityUri::block("B")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 2,
            content: "x".to_string(),
        });
        let key_x = store.sorted_children("B").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("Y"),
            parent_id: Some(EntityUri::block("B")),
            sort_key: gen_key_between(Some(&key_x), None).unwrap(),
            depth: 2,
            content: "y".to_string(),
        });

        store.join_block(&EntityUri::block("B"), 0).await.unwrap();

        let a = store.get("A").unwrap();
        assert_eq!(a.content, "foobar");
        assert!(store.get("B").is_none(), "B must be deleted after join");
        let a_children = store.sorted_children("A");
        assert_eq!(
            a_children.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["block:W", "block:X", "block:Y"],
            "B's children append after A's existing child, in order"
        );
    }

    #[tokio::test]
    async fn move_block_deeper_updates_descendant_depths_exactly() {
        // Moving A (depth 1, subtree B depth 2, C depth 3) under E (depth 2)
        // gives depth_delta = +2: A -> 3, B -> 4, C -> 5. Exact values kill
        // the +/- and +/* arithmetic mutations and the delta != 0 gate flip.
        let store = MemStore::new();
        insert_block(&store, "P1", None, None); // depth 0
        insert_block(&store, "P2", None, None); // depth 0
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P1")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "A".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("A")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 2,
            content: "B".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("C"),
            parent_id: Some(EntityUri::block("B")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 3,
            content: "C".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("D"),
            parent_id: Some(EntityUri::block("P2")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 1,
            content: "D".to_string(),
        });
        store.insert(TestBlock {
            id: EntityUri::block("E"),
            parent_id: Some(EntityUri::block("D")),
            sort_key: gen_key_between(None, None).unwrap(),
            depth: 2,
            content: "E".to_string(),
        });

        store
            .move_block(&EntityUri::block("A"), &EntityUri::block("E"), None)
            .await
            .unwrap();

        assert_eq!(store.get("A").unwrap().depth, 3);
        assert_eq!(store.get("B").unwrap().depth, 4);
        assert_eq!(store.get("C").unwrap().depth, 5);
        // Blocks outside the moved subtree are untouched.
        assert_eq!(store.get("D").unwrap().depth, 1);
        assert_eq!(store.get("E").unwrap().depth, 2);
    }

    /// Store that relies on the DEFAULT `DataSource`/`BlockQueryHelpers`
    /// impls (only `children_ordered` provided, as required) so the default
    /// sibling-navigation logic in traits.rs is exercised, not overridden.
    struct DefaultHelpersStore {
        blocks: Vec<TestBlock>,
    }

    #[async_trait]
    impl DataSource<TestBlock> for DefaultHelpersStore {
        async fn get_all(&self) -> Result<Vec<TestBlock>> {
            Ok(self.blocks.clone())
        }
        async fn get_by_id(&self, id: &str) -> Result<Option<TestBlock>> {
            Ok(self.blocks.iter().find(|b| b.id.as_str() == id).cloned())
        }
    }

    #[async_trait]
    impl BlockQueryHelpers<TestBlock> for DefaultHelpersStore {
        async fn children_ordered(&self, parent_id: &EntityUri) -> Result<Vec<TestBlock>> {
            let mut children: Vec<TestBlock> = self
                .blocks
                .iter()
                .filter(|b| b.parent_id.as_ref() == Some(parent_id))
                .cloned()
                .collect();
            children.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
            Ok(children)
        }
    }

    fn default_helpers_fixture() -> DefaultHelpersStore {
        // P
        //   A
        //     A1
        //   B
        //   C
        let mk = |id: &str, parent: Option<&str>, key: &str, depth: i64| TestBlock {
            id: EntityUri::block(id),
            parent_id: parent.map(EntityUri::block),
            sort_key: key.to_string(),
            depth,
            content: format!("Content {id}"),
        };
        DefaultHelpersStore {
            blocks: vec![
                mk("P", None, "A0", 0),
                mk("A", Some("P"), "A1", 1),
                mk("A1", Some("A"), "A1", 2),
                mk("B", Some("P"), "A2", 1),
                mk("C", Some("P"), "A3", 1),
            ],
        }
    }

    fn ids(blocks: &[TestBlock]) -> Vec<&str> {
        blocks.iter().map(|b| b.id.as_str()).collect()
    }

    #[tokio::test]
    async fn default_data_source_children_and_descendants() {
        let store = default_helpers_fixture();
        let p = EntityUri::block("P");

        let children = DataSource::get_children(&store, &p).await.unwrap();
        let mut child_ids = ids(&children);
        child_ids.sort();
        assert_eq!(child_ids, vec!["block:A", "block:B", "block:C"]);

        let descendants = store.get_descendants(&p).await.unwrap();
        let mut desc_ids = ids(&descendants);
        desc_ids.sort();
        assert_eq!(
            desc_ids,
            vec!["block:A", "block:A1", "block:B", "block:C"],
            "descendants include grandchildren, not P itself"
        );
    }

    #[tokio::test]
    async fn default_sibling_navigation_helpers() {
        let store = default_helpers_fixture();
        let a = EntityUri::block("A");
        let b = EntityUri::block("B");
        let c = EntityUri::block("C");
        let p = EntityUri::block("P");

        let sibs = store.get_siblings(&b).await.unwrap();
        assert_eq!(ids(&sibs), vec!["block:A", "block:C"]);

        assert_eq!(
            store.get_prev_sibling(&b).await.unwrap().unwrap().id,
            a,
            "prev sibling of B is A"
        );
        assert!(store.get_prev_sibling(&a).await.unwrap().is_none());
        assert_eq!(
            store.get_next_sibling(&b).await.unwrap().unwrap().id,
            c,
            "next sibling of B is C"
        );
        assert!(store.get_next_sibling(&c).await.unwrap().is_none());

        assert_eq!(
            store.get_first_child(Some(&p)).await.unwrap().unwrap().id,
            a
        );
        assert_eq!(store.get_last_child(Some(&p)).await.unwrap().unwrap().id, c);
        assert!(store.get_first_child(None).await.unwrap().is_none());
        assert!(store.get_last_child(None).await.unwrap().is_none());
    }
}
