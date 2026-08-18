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
    use crate::block_ordering::MintedPosition;
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
        content: String,
        tags: holon_api::Tags,
        collapsed: bool,
    }

    impl BlockEntity for TestBlock {
        fn id(&self) -> &EntityUri {
            &self.id
        }
        fn parent_id(&self) -> Option<&EntityUri> {
            self.parent_id.as_ref()
        }
        fn content(&self) -> &str {
            &self.content
        }
        fn tags(&self) -> holon_api::Tags {
            self.tags.clone()
        }
        fn collapsed(&self) -> bool {
            self.collapsed
        }
    }

    /// In-memory block store for testing
    struct MemStore {
        blocks: Mutex<Vec<TestBlock>>,
        /// Child lists the POSITIONAL AUTHORITY reports, overriding the
        /// `parent_id`-derived ones. Production reads order from
        /// `BlockOrdering` (the Loro tree) and `collapsed`/`Page` from
        /// the block row — two authorities that can disagree, which is
        /// the only way a child CYCLE becomes reachable. Empty by
        /// default, so every other test is unaffected.
        forced_children: Mutex<HashMap<String, Vec<EntityUri>>>,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                blocks: Mutex::new(Vec::new()),
                forced_children: Mutex::new(HashMap::new()),
            }
        }

        fn force_children(&self, parent_id: &str, children: Vec<EntityUri>) {
            self.forced_children
                .lock()
                .unwrap()
                .insert(canon(parent_id), children);
        }

        fn insert(&self, block: TestBlock) {
            self.blocks.lock().unwrap().push(block);
        }

        fn set_collapsed(&self, id: &str, collapsed: bool) {
            let want = canon(id);
            let mut blocks = self.blocks.lock().unwrap();
            let block = blocks
                .iter_mut()
                .find(|b| b.id.as_str() == want)
                .unwrap_or_else(|| panic!("set_collapsed: block {want} not inserted"));
            block.collapsed = collapsed;
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
                "content" => Value::String(block.content.clone()),
                _ => Value::Null,
            };
            match field {
                // ALLOW(entity_uri_from_raw): test set_field boundary — parent_id value arrives as
                // a raw string.
                "parent_id" => block.parent_id = value.as_string().map(EntityUri::from_raw),
                "sort_key" => block.sort_key = value.as_string().unwrap().to_string(),
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
                content: fields
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string(),
                tags: holon_api::Tags::default(),
                collapsed: false,
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
        ) -> Result<MintedPosition> {
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
            // This double keeps every sibling key minted, so a position here
            // never displaces one.
            gen_key_between(prev_key.as_deref(), next_key.as_deref())
                .map(MintedPosition::alone)
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
            // This double keeps every sibling key minted, so a position never
            // displaces one and the re-key half is always empty.
            let (new_sort_key, rekeys) = self
                .new_child_anchor(parent_id, after_id)
                .await?
                .into_parts();
            assert!(
                rekeys.is_empty(),
                "MemStore never produces an unkeyed sibling, so a position must displace nothing"
            );
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
            if let Some(forced) = self
                .forced_children
                .lock()
                .unwrap()
                .get(&canon(parent_id.as_str()))
            {
                return Ok(forced.clone());
            }
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
                        content: params
                            .get("content")
                            .and_then(|v| v.as_string())
                            .unwrap_or_default()
                            .to_string(),
                        tags: holon_api::Tags::default(),
                        collapsed: false,
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
        store.insert(TestBlock {
            id: EntityUri::block(id),
            parent_id: parent_id.map(EntityUri::block),
            sort_key,
            content: format!("Content {}", id),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
    }

    /// Insert a `Page`-tagged block (an org-file root) at `parent_id`.
    fn insert_page(store: &MemStore, id: &str, parent_id: Option<&str>) {
        let sort_key = gen_key_between(None, None).unwrap();
        let mut tags = holon_api::Tags::default();
        tags.insert(holon_api::PAGE_TAG);
        store.insert(TestBlock {
            id: EntityUri::block(id),
            parent_id: parent_id.map(EntityUri::block),
            sort_key,
            content: format!("Page {}", id),
            tags,
            collapsed: false,
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
    }

    #[tokio::test]
    async fn outdent_moves_to_grandparent() {
        let store = MemStore::new();
        insert_block(&store, "GP", None, None);
        insert_block(&store, "P", Some("GP"), None);

        let b = TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "Content B".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        };
        store.insert(b);

        // Outdent B: should move to GP level, after P
        store.outdent(&EntityUri::block("B")).await.unwrap();

        let b = store.get("B").unwrap();
        assert_eq!(b.parent_id, Some(EntityUri::block("GP")));
    }

    #[tokio::test]
    async fn outdent_root_block_fails() {
        let store = MemStore::new();
        insert_block(&store, "R", None, None);

        let result = store.outdent(&EntityUri::block("R")).await;
        assert!(result.is_err());
    }

    /// Destructive-delete ruling 2026-07-21: `delete_keep_children` reparents
    /// the deleted block's children into ITS OWN sibling slot, preserving their
    /// relative order. P has [A, B, C]; B has [B1, B2]. Deleting B keeping
    /// children yields [A, B1, B2, C] — the children take B's position IN
    /// ORDER, not appended at the end (a naive append would give [A, C, B1,
    /// B2]).
    #[tokio::test]
    async fn delete_keep_children_reparents_into_slot_preserving_order() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));
        let key_b = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "C", Some("P"), Some(&key_b));
        insert_block(&store, "B1", Some("B"), None);
        let key_b1 = store.sorted_children("B").last().unwrap().sort_key.clone();
        insert_block(&store, "B2", Some("B"), Some(&key_b1));

        store
            .delete_keep_children(&EntityUri::block("B"))
            .await
            .unwrap();

        assert!(store.get("B").is_none(), "B itself must be deleted");
        let ids: Vec<EntityUri> = store
            .sorted_children("P")
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(
            ids,
            vec![
                EntityUri::block("A"),
                EntityUri::block("B1"),
                EntityUri::block("B2"),
                EntityUri::block("C"),
            ]
        );
    }

    /// `delete_subtree` removes the target AND every descendant, leaving
    /// siblings untouched. P has [A, B]; B has [B1, B2]. Deleting B's subtree
    /// leaves P with just [A].
    #[tokio::test]
    async fn delete_subtree_removes_target_and_all_descendants() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));
        insert_block(&store, "B1", Some("B"), None);
        let key_b1 = store.sorted_children("B").last().unwrap().sort_key.clone();
        insert_block(&store, "B2", Some("B"), Some(&key_b1));

        store.delete_subtree(&EntityUri::block("B")).await.unwrap();

        assert!(store.get("B").is_none());
        assert!(store.get("B1").is_none());
        assert!(store.get("B2").is_none());
        let remaining = store.sorted_children("P");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, EntityUri::block("A"));
    }

    /// ADR 0028 D1: outdenting a DIRECT PAGE CHILD is rejected — it would move
    /// the block out of its page container, escaping the page. Structurally a
    /// grandparent exists (GP), so the ONLY reason this must fail is the
    /// page-parent rule; before that rule this outdent succeeded (moved B to
    /// GP).
    #[tokio::test]
    async fn outdent_direct_page_child_is_rejected() {
        let store = MemStore::new();
        insert_block(&store, "GP", None, None);
        // P is a Page nested under GP, so B (child of P) HAS a grandparent —
        // the generic "no grandparent" guard does not apply here.
        insert_page(&store, "P", Some("GP"));

        let b = TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "Content B".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        };
        store.insert(b);

        let result = store.outdent(&EntityUri::block("B")).await;
        assert!(
            result.is_err(),
            "outdent of a direct page child must be rejected (ADR 0028 D1)"
        );

        // Rejection is inert: B stays under the page, unchanged.
        let b = store.get("B").unwrap();
        assert_eq!(b.parent_id, Some(EntityUri::block("P")));
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
            content: "Hello World".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
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
    /// At a position-0 split the id follows the text: `A` keeps the whole
    /// string and the minted block is the EMPTY one, inserted ABOVE it — so
    /// anything addressing `A` still resolves to the text.
    async fn split_block_at_start() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "Hello".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });

        store.split_block(&EntityUri::block("A"), 0).await.unwrap();

        let a = store.get("block:A").unwrap();
        assert_eq!(a.content, "Hello");

        let children = store.sorted_children("P");
        assert_eq!(children.len(), 2);
        assert_eq!(
            children[1].id,
            EntityUri::block("A"),
            "the minted empty block sits ABOVE the text-bearing original"
        );
        assert_eq!(children[0].content, "");
    }

    #[tokio::test]
    /// A position-0 split of a PARENTLESS block. Its predecessor is `None`
    /// (`get_prev_sibling` short-circuits on a null `parent_id`), so both
    /// create arms take the first-slot branch — a branch no keystone draw
    /// reaches. Identity routing must be the same as anywhere else: the
    /// text keeps the original id, the minted block is the empty one, and
    /// the inverse round- trips. Root ORDER is pinned where it is real —
    /// `SqlBlockOperations:: root_slot_anchor_sorts_before_the_first_root`
    /// and the Loro registry's
    /// `first_slot_position_among_roots_is_expressible_for_a_parentless_split`
    /// — because this in-memory store models parentless as a null `parent_id`
    /// rather than the `sentinel:no_parent` rows the real minter scans.
    async fn split_block_at_start_of_a_parentless_block_routes_identity_and_undoes() {
        let store = MemStore::new();
        store.insert(TestBlock {
            id: EntityUri::block("R"),
            parent_id: None,
            sort_key: gen_key_between(None, None).unwrap(),
            content: "Rooted".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let before = snapshot(&store);

        let split = store.split_block(&EntityUri::block("R"), 0).await.unwrap();
        assert_eq!(
            store.get("R").unwrap().content,
            "Rooted",
            "the text keeps the original id even at the top level"
        );
        let minted = store
            .get_all()
            .await
            .expect("read the store")
            .into_iter()
            .find(|b| b.id.as_str() != "block:R")
            .expect("the split minted a block");
        assert_eq!(minted.content, "", "the minted block is the empty one");
        assert_eq!(
            minted.parent_id, None,
            "the minted block stays parentless, like its origin"
        );

        let after_split = snapshot(&store);
        let undo_result = apply_inverse(&store, &split.undo).await;
        assert_eq!(snapshot(&store), before);
        apply_inverse(&store, &undo_result.undo).await;
        assert_eq!(
            snapshot(&store),
            after_split,
            "redo must re-apply the parentless position-0 split byte-identically"
        );
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
            content: "Hello".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
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
            content: "Hi".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
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
            content: "foo".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            content: "bar".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
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
            content: "parent ".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "child".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            content: "sib1".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_b = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("C"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_b), None).unwrap(),
            content: "sib2".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
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
            content: "parent ".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "child".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            content: "sib".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        store.insert(TestBlock {
            id: EntityUri::block("X"),
            parent_id: Some(EntityUri::block("A")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "x".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_x = store.sorted_children("A").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("Y"),
            parent_id: Some(EntityUri::block("A")),
            sort_key: gen_key_between(Some(&key_x), None).unwrap(),
            content: "y".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
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
            content: "alone".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
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
    async fn join_block_merges_into_the_visible_outline_predecessor_not_the_prev_sibling() {
        // Case A (prev sibling exists) with children: the merge target is the
        // row directly ABOVE B in the visible outline — A's last visible
        // descendant W, not A itself — and B's children X, Y append under W in
        // document order.
        // Layout:
        //   P
        //     A ("foo")
        //       W ("w")   <- merge target (the row above B)
        //     B ("bar")
        //       X ("x")
        //       Y ("y")
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "foo".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            content: "bar".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        store.insert(TestBlock {
            id: EntityUri::block("W"),
            parent_id: Some(EntityUri::block("A")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "w".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        store.insert(TestBlock {
            id: EntityUri::block("X"),
            parent_id: Some(EntityUri::block("B")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "x".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_x = store.sorted_children("B").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("Y"),
            parent_id: Some(EntityUri::block("B")),
            sort_key: gen_key_between(Some(&key_x), None).unwrap(),
            content: "y".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });

        store.join_block(&EntityUri::block("B"), 0).await.unwrap();

        assert_eq!(store.get("W").unwrap().content, "wbar");
        assert_eq!(
            store.get("A").unwrap().content,
            "foo",
            "A is two rows above B; its content must be untouched"
        );
        assert!(store.get("B").is_none(), "B must be deleted after join");
        assert_eq!(
            store
                .sorted_children("W")
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["block:X", "block:Y"],
            "B's children append under the merge target, in order"
        );
        assert_eq!(
            store
                .sorted_children("A")
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["block:W"]
        );
    }

    #[tokio::test]
    async fn join_block_stops_at_a_collapsed_previous_sibling() {
        // Same layout, but A is collapsed: W is not rendered, so the row above
        // B is A itself and the merge lands there.
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "foo".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        store.set_collapsed("A", true);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            content: "bar".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        insert_block(&store, "W", Some("A"), None);

        store.join_block(&EntityUri::block("B"), 0).await.unwrap();

        assert_eq!(store.get("A").unwrap().content, "foobar");
        assert_eq!(
            store.get("W").unwrap().content,
            "Content W",
            "the hidden child is not a merge target"
        );
    }

    /// The descent reads order from `BlockOrdering` and `collapsed`/`Page` from
    /// the block row. When those two authorities disagree the child graph can
    /// carry a cycle, and an unbounded walk hangs the op. It must fail loud
    /// instead, naming the start block and the tail of the walk.
    #[tokio::test]
    async fn join_block_descent_into_a_child_cycle_fails_loud() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        insert_block(&store, "A", Some("P"), None);
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        insert_block(&store, "B", Some("P"), Some(&key_a));
        // The positional authority claims A is its own last child.
        store.force_children("A", vec![EntityUri::block("A")]);

        let err = store
            .join_block(&EntityUri::block("B"), 0)
            .await
            .expect_err("a cyclic outline must fail loud, not hang");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("descent exceeded 4096 steps from block:B"),
            "error must name the limit and the start block; got: {msg}"
        );
        assert!(
            msg.contains("block:A"),
            "error must show the visited ids; got: {msg}"
        );
        assert_eq!(
            store.get("B").unwrap().content,
            "Content B",
            "the refused join must not have mutated anything"
        );
    }

    // ---- No-pages-under-non-pages op guard (interim ruling 2026-07-13) -------

    /// Red-first proof of the write-side guard in `move_block`: a `Page`-tagged
    /// block may NOT be reparented under a non-page block. Both the SQL and
    /// Loro providers use this default `BlockOperations::move_block`, so
    /// the single guard covers both. Without the guard the move lands (the
    /// prohibited topology surfaces only deep in writeback via
    /// `name_chain`); with it the op fails loud and the tree is untouched.
    #[tokio::test]
    async fn move_block_rejects_page_under_non_page() {
        let store = MemStore::new();
        // `folder` is a page (org-file root); `date` is a page nested under it —
        // the valid page-under-page topology the journal rule produces.
        insert_page(&store, "folder", None);
        insert_page(&store, "date", Some("folder"));
        // `text` is an ordinary non-page block, also under `folder`.
        insert_block(&store, "text", Some("folder"), None);

        let err = store
            .move_block(&EntityUri::block("date"), &EntityUri::block("text"), None)
            .await
            .expect_err("reparenting a page under a non-page block must fail loud");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("pages under non-pages are prohibited"),
            "error must name the ruling; got: {msg}"
        );

        // The rejected move left the tree untouched: `date` still parents `folder`.
        assert_eq!(
            store.get("date").unwrap().parent_id,
            Some(EntityUri::block("folder")),
        );
    }

    /// The guard is scoped to PAGES: a non-page block may still be reparented
    /// under a non-page block (the common outline-indent case), and a page may
    /// be reparented under another page.
    #[tokio::test]
    async fn move_block_allows_page_under_page_and_non_page_anywhere() {
        let store = MemStore::new();
        insert_page(&store, "folderA", None);
        insert_page(&store, "folderB", None);
        insert_page(&store, "date", Some("folderA"));
        insert_block(&store, "text1", Some("folderA"), None);
        insert_block(&store, "text2", Some("folderA"), None);

        // page under a different page: allowed.
        store
            .move_block(
                &EntityUri::block("date"),
                &EntityUri::block("folderB"),
                None,
            )
            .await
            .expect("a page may nest under another page");
        assert_eq!(
            store.get("date").unwrap().parent_id,
            Some(EntityUri::block("folderB")),
        );

        // non-page under a non-page: allowed.
        store
            .move_block(&EntityUri::block("text1"), &EntityUri::block("text2"), None)
            .await
            .expect("a non-page may nest under a non-page");
        assert_eq!(
            store.get("text1").unwrap().parent_id,
            Some(EntityUri::block("text2")),
        );
    }

    // ---- U4: split_block / join_block compound inverses ---------------------

    /// A byte-comparable snapshot of the whole block table: every block's
    /// (id, parent_id, sort_key, content), sorted by id. Undo must
    /// restore this EXACTLY (the U4 contract).
    fn snapshot(store: &MemStore) -> Vec<(String, Option<String>, String, String)> {
        let mut rows: Vec<_> = store
            .blocks
            .lock()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b.id.as_str().to_string(),
                    b.parent_id.as_ref().map(|p| p.as_str().to_string()),
                    b.sort_key.clone(),
                    b.content.clone(),
                )
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    /// Dispatch an inverse `Operation` (from `OperationResult::undo`) back
    /// through the generated block-operations dispatcher — the same path the
    /// production undo engine (`OperationEngine::undo`) uses to re-execute an
    /// inverse. Returns the executed inverse's OWN result (whose `.undo` is the
    /// redo operation).
    async fn apply_inverse(store: &MemStore, undo: &UndoAction) -> OperationResult {
        let op = match undo {
            UndoAction::Undo(op) => op,
            other => panic!("expected reversible op, got {other:?}"),
        };
        let params: crate::storage::types::StorageEntity = op
            .params
            .iter()
            .map(|(k, v)| (std::sync::Arc::from(k.as_str()), v.clone()))
            .collect();
        crate::__operations_block_operations::dispatch_operation::<MemStore, TestBlock>(
            store,
            &op.op_name,
            &params,
        )
        .await
        .expect("inverse dispatch failed")
    }

    #[tokio::test]
    async fn split_block_undo_restores_exact_pre_op_state_and_redo_reapplies() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            // Whitespace at the split point that the trim discards — the undo
            // must still restore the UNtrimmed original, proving the inverse
            // uses the recorded pre-split content, not `content_before`.
            content: "Hello World".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });

        let before = snapshot(&store);

        // Split "Hello World" at position 5 ("Hello| World"): A -> "Hello",
        // new block -> "World" (leading space trimmed).
        let split = store.split_block(&EntityUri::block("A"), 5).await.unwrap();
        assert_eq!(store.get("A").unwrap().content, "Hello");
        let after_split = snapshot(&store);
        assert_ne!(before, after_split, "split must change state");

        // Undo: delete the new block, restore A's exact original content.
        let undo_result = apply_inverse(&store, &split.undo).await;
        assert_eq!(
            snapshot(&store),
            before,
            "undo of split_block must restore byte-identical pre-op state"
        );

        // Redo: re-split deterministically (same new-block id, same content).
        apply_inverse(&store, &undo_result.undo).await;
        assert_eq!(
            snapshot(&store),
            after_split,
            "redo must re-apply the split byte-identically"
        );
    }

    #[tokio::test]
    async fn split_block_at_start_undo_restores_exact_pre_op_state() {
        // Boundary: split at 0 leaves all the content on A and inserts an empty
        // block above it. Undo must delete that block and leave A untouched —
        // including its sort_key, which the insert-above re-anchoring must not
        // have disturbed.
        //
        // The LEADING WHITESPACE is load-bearing. A position-0 split still
        // `trim_start`s the text, and that trim now lands on the id the caret
        // sits in, so the undo's content-restore leg has real work to do:
        // A must come back as "  Hello", not the trimmed "Hello". Without it
        // the origin already holds its final bytes and an inverse that never
        // wrote content would pass unnoticed.
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "  Hello".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let before = snapshot(&store);

        let split = store.split_block(&EntityUri::block("A"), 0).await.unwrap();
        assert_eq!(
            store.get("A").unwrap().content,
            "Hello",
            "the text stays on A, trimmed"
        );

        let after_split = snapshot(&store);
        let undo_result = apply_inverse(&store, &split.undo).await;
        assert_eq!(
            store.get("A").unwrap().content,
            "  Hello",
            "undo must restore the UNTRIMMED pre-split bytes onto A"
        );
        assert_eq!(snapshot(&store), before);

        // Redo re-splits deterministically: same minted id, same empty block in
        // the same slot above A.
        apply_inverse(&store, &undo_result.undo).await;
        assert_eq!(
            snapshot(&store),
            after_split,
            "redo must re-apply the position-0 split byte-identically"
        );
    }

    #[tokio::test]
    async fn join_block_into_prev_sibling_undo_restores_exact_pre_op_state_and_redo_reapplies() {
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "foo".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            // B minted directly after A between (key_a, None) — the same slot
            // `restore_split` re-mints on undo, so B's sort_key comes back
            // byte-identical (deterministic fractional index).
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            content: "bar".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let before = snapshot(&store);

        // Join B into A: A -> "foobar", B deleted.
        let join = store.join_block(&EntityUri::block("B"), 0).await.unwrap();
        assert_eq!(store.get("A").unwrap().content, "foobar");
        assert!(store.get("B").is_none());
        let after_join = snapshot(&store);

        // Undo: recreate B at its slot with its exact fields, restore A.
        let undo_result = apply_inverse(&store, &join.undo).await;
        assert_eq!(
            snapshot(&store),
            before,
            "undo of join_block must restore byte-identical pre-op state (incl. B's sort_key)"
        );

        // Redo: re-join deterministically.
        apply_inverse(&store, &undo_result.undo).await;
        assert_eq!(
            snapshot(&store),
            after_join,
            "redo must re-apply the join byte-identically"
        );
    }

    #[tokio::test]
    async fn join_block_into_parent_first_child_undo_restores_order_and_content() {
        // Child->parent join (B has no prev sibling): B merges into parent P,
        // B deleted. Undo recreates B as P's first child. In this case B's
        // sort_key is re-minted against different neighbours, so we assert the
        // observable contract the projection oracle enforces: sibling ORDER +
        // content + ids + parent, rather than the raw key bytes.
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "child".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_b = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("C"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_b), None).unwrap(),
            content: "sib".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        // Give the parent content so the join boundary is observable.
        store
            .set_field("block:P", "content", Value::String("parent".to_string()))
            .await
            .unwrap();

        let order_before: Vec<String> = store
            .sorted_children("P")
            .iter()
            .map(|b| b.content.clone())
            .collect();
        assert_eq!(order_before, vec!["child", "sib"]);

        let join = store.join_block(&EntityUri::block("B"), 0).await.unwrap();
        assert_eq!(store.get("P").unwrap().content, "parentchild");
        assert!(store.get("B").is_none());

        apply_inverse(&store, &join.undo).await;
        // P content restored, B back as first child, order preserved.
        assert_eq!(store.get("P").unwrap().content, "parent");
        let order_after: Vec<(String, String)> = store
            .sorted_children("P")
            .iter()
            .map(|b| (b.id.as_str().to_string(), b.content.clone()))
            .collect();
        assert_eq!(
            order_after,
            vec![
                ("block:B".to_string(), "child".to_string()),
                ("block:C".to_string(), "sib".to_string()),
            ]
        );
        assert_eq!(
            store.get("B").unwrap().parent_id,
            Some(EntityUri::block("P"))
        );
    }

    #[tokio::test]
    async fn join_block_with_children_stays_irreversible() {
        // A subtree join (B has its own child) re-parents the child under A;
        // one flat inverse cannot restore that placement, so the op must fail
        // loud as Irreversible rather than ship a lossy inverse.
        let store = MemStore::new();
        insert_block(&store, "P", None, None);
        store.insert(TestBlock {
            id: EntityUri::block("A"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "foo".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        let key_a = store.sorted_children("P").last().unwrap().sort_key.clone();
        store.insert(TestBlock {
            id: EntityUri::block("B"),
            parent_id: Some(EntityUri::block("P")),
            sort_key: gen_key_between(Some(&key_a), None).unwrap(),
            content: "bar".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });
        store.insert(TestBlock {
            id: EntityUri::block("B1"),
            parent_id: Some(EntityUri::block("B")),
            sort_key: gen_key_between(None, None).unwrap(),
            content: "grandchild".to_string(),
            tags: holon_api::Tags::default(),
            collapsed: false,
        });

        let join = store.join_block(&EntityUri::block("B"), 0).await.unwrap();
        assert!(
            matches!(join.undo, UndoAction::DeclaredIrreversible(_)),
            "join with re-parented children must stay DeclaredIrreversible, got {:?}",
            join.undo
        );
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
        let mk = |id: &str, parent: Option<&str>, key: &str| TestBlock {
            id: EntityUri::block(id),
            parent_id: parent.map(EntityUri::block),
            sort_key: key.to_string(),
            content: format!("Content {id}"),
            tags: holon_api::Tags::default(),
            collapsed: false,
        };
        DefaultHelpersStore {
            blocks: vec![
                mk("P", None, "A0"),
                mk("A", Some("P"), "A1"),
                mk("A1", Some("A"), "A1"),
                mk("B", Some("P"), "A2"),
                mk("C", Some("P"), "A3"),
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
