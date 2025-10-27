//! Reproduce the inbound parent_id bug: a SQL `FieldsChanged` event with a
//! `parent_id` delta is not applied to the LoroTree's structural parent — it
//! gets written into the property map instead, leaving the tree pointing at
//! the OLD parent. Concurrent peer indents/moves can't merge as tree CRDT
//! moves under that condition.
//!
//! On `main` (before the fix) this test fails: the structural `parent_id`
//! returned by `LoroBackend::get_block` still points at the original parent
//! after `apply_fields_changed` runs.

#[cfg(test)]
mod tests {
    use crate::api::LoroBackend;
    use crate::api::repository::CoreOperations;
    use crate::sync::LoroDocumentStore;
    use crate::sync::loro_sync_controller::apply_fields_changed;
    use holon_api::EntityUri;
    use holon_api::block::BlockContent;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Build an isolated `LoroBackend` against a fresh on-disk doc store.
    async fn fresh_backend() -> (LoroBackend, TempDir) {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let storage_dir: PathBuf = tempdir.path().to_path_buf();
        let store = LoroDocumentStore::new(storage_dir);
        let collab = store
            .get_global_doc()
            .await
            .expect("get_global_doc must succeed in test setup");
        (LoroBackend::from_document(collab), tempdir)
    }

    #[tokio::test]
    async fn inbound_parent_id_event_moves_block_in_loro_tree() {
        let (backend, _tempdir) = fresh_backend().await;

        // Seed: A under root, B under A.
        let block_a = backend
            .create_block(EntityUri::no_parent(), BlockContent::text("A"), None)
            .await
            .expect("create A");
        let block_b = backend
            .create_block(
                EntityUri::block(block_a.id.id()),
                BlockContent::text("B"),
                None,
            )
            .await
            .expect("create B");

        // Sanity: structurally, B is under A.
        let b_before = backend
            .get_block(block_b.id.as_str())
            .await
            .expect("get B before");
        assert_eq!(
            b_before.parent_id,
            EntityUri::block(block_a.id.id()),
            "precondition: B's structural parent is A"
        );

        // Synthetic SQL FieldsChanged: parent_id of B changed from A to root.
        // Format mirrors what `SqlOperationProvider` emits via
        // `build_event_payload`: array of [field, old, new] tuples.
        let new_parent = EntityUri::no_parent();
        let fields =
            serde_json::json!([["parent_id", block_a.id.to_string(), new_parent.to_string()]]);

        apply_fields_changed(&backend, block_b.id.as_str(), &fields)
            .await
            .expect("apply_fields_changed must not error");

        // After the fix: B's structural parent in the LoroTree is `no_parent`.
        // Before the fix: B's structural parent stays as A (the parent_id
        // delta only landed in the property map), so this assertion fails.
        let b_after = backend
            .get_block(block_b.id.as_str())
            .await
            .expect("get B after");
        assert_eq!(
            b_after.parent_id, new_parent,
            "inbound parent_id event must move B in the LoroTree, not just \
             update a property"
        );
    }
}
