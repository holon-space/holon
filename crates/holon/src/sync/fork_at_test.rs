//! Targeted test: does `doc.fork_at()` correctly rewind `LoroText`
//! sub-containers after a cross-peer import?

#[cfg(test)]
mod tests {
    use loro::{ExportMode, LoroDoc, LoroText};

    const TREE_NAME: &str = "holon_tree";

    fn read_content(doc: &LoroDoc) -> String {
        let tree = doc.get_tree(TREE_NAME);
        for node in tree.get_nodes(false) {
            if matches!(
                node.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            ) {
                continue;
            }
            let meta = tree.get_meta(node.id).unwrap();
            if let Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) =
                meta.get("content_raw")
            {
                return t.to_string();
            }
        }
        String::new()
    }

    #[test]
    fn fork_at_rewinds_lorotext_after_local_update() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let tree = doc.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        let text: LoroText = meta
            .insert_container("content_raw", LoroText::new())
            .unwrap();
        text.insert(0, "original").unwrap();
        doc.commit();

        let before_frontiers = doc.oplog_frontiers();
        assert_eq!(read_content(&doc), "original");

        // Local update
        let text: LoroText = meta
            .get_or_create_container("content_raw", LoroText::new())
            .unwrap();
        text.delete(0, text.len_unicode()).unwrap();
        text.insert(0, "updated").unwrap();
        doc.commit();

        assert_eq!(read_content(&doc), "updated");

        // Fork at the pre-update frontiers
        let fork = doc
            .fork_at(&before_frontiers)
            .expect("fork_at must succeed");
        let fork_content = read_content(&fork);
        assert_eq!(
            fork_content, "original",
            "fork_at should rewind LoroText to pre-update state (local)"
        );
    }

    #[test]
    fn fork_at_rewinds_lorotext_after_peer_import() {
        // Primary: create a block with content "original"
        let primary = LoroDoc::new();
        primary.set_peer_id(1).unwrap();
        let tree = primary.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        let text: LoroText = meta
            .insert_container("content_raw", LoroText::new())
            .unwrap();
        text.insert(0, "original").unwrap();
        primary.commit();

        // Peer: fork from primary, then update content
        let peer = LoroDoc::new();
        peer.set_peer_id(2).unwrap();
        let snapshot = primary
            .export(ExportMode::Snapshot)
            .expect("snapshot export");
        peer.import(&snapshot).expect("peer import snapshot");

        // Verify peer has "original"
        assert_eq!(read_content(&peer), "original");

        // Update on peer
        let peer_tree = peer.get_tree(TREE_NAME);
        for peer_node in peer_tree.get_nodes(false) {
            if matches!(
                peer_node.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            ) {
                continue;
            }
            let peer_meta = peer_tree.get_meta(peer_node.id).unwrap();
            let t: LoroText = peer_meta
                .get_or_create_container("content_raw", LoroText::new())
                .unwrap();
            let old_len = t.len_unicode();
            if old_len > 0 {
                t.delete(0, old_len).unwrap();
            }
            t.insert(0, "from_peer").unwrap();
            peer.commit();
            break;
        }
        assert_eq!(read_content(&peer), "from_peer");

        // Record primary's frontiers BEFORE importing peer delta
        let before_frontiers = primary.oplog_frontiers();
        assert_eq!(read_content(&primary), "original");

        // Import peer's delta into primary
        let primary_vv = primary.oplog_vv();
        let delta = peer
            .export(ExportMode::updates(&primary_vv))
            .expect("export delta");
        assert!(!delta.is_empty(), "peer should have changes to export");
        primary.import(&delta).expect("primary import delta");

        // Primary should now have "from_peer"
        let current_content = read_content(&primary);
        assert_eq!(
            current_content, "from_peer",
            "primary should reflect peer's update after import"
        );

        // THE CRITICAL CHECK: fork_at should show the OLD content
        let fork = primary
            .fork_at(&before_frontiers)
            .expect("fork_at must succeed");
        let fork_content = read_content(&fork);
        assert_eq!(
            fork_content, "original",
            "fork_at should rewind LoroText to pre-import state (cross-peer)"
        );
    }

    #[test]
    fn subscribe_root_fires_after_sync_docs_direct() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let primary = LoroDoc::new();
        primary.set_peer_id(1).unwrap();
        let tree = primary.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);

        let callback_count = Arc::new(AtomicUsize::new(0));
        let count_clone = callback_count.clone();
        let _sub = primary.subscribe_root(Arc::new(move |_| {
            count_clone.fetch_add(1, Ordering::SeqCst);
        }));

        // Create a peer with a block
        let peer = LoroDoc::new();
        peer.set_peer_id(2).unwrap();
        let peer_tree = peer.get_tree(TREE_NAME);
        peer_tree.enable_fractional_index(0);
        let _node = peer_tree.create(None).unwrap();
        peer.commit();

        assert_eq!(
            callback_count.load(Ordering::SeqCst),
            0,
            "no callback before sync"
        );

        // sync_docs_direct: peer → primary should fire subscribe_root on primary
        crate::sync::multi_peer::sync_docs_direct(&primary, &peer);

        assert!(
            callback_count.load(Ordering::SeqCst) > 0,
            "subscribe_root should fire after sync_docs_direct imports peer data"
        );
    }

    #[test]
    fn subscribe_root_fires_after_rwlock_wrapped_sync() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let primary = Arc::new(LoroDoc::new());
            {
                let doc = &*primary;
                doc.set_peer_id(1).unwrap();
                let tree = doc.get_tree(TREE_NAME);
                tree.enable_fractional_index(0);
            }

            let callback_count = Arc::new(AtomicUsize::new(0));
            let count_clone = callback_count.clone();
            let _sub = {
                let doc = &*primary;
                doc.subscribe_root(Arc::new(move |_| {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                }))
            };

            // Peer with a block
            let peer = LoroDoc::new();
            peer.set_peer_id(2).unwrap();
            let peer_tree = peer.get_tree(TREE_NAME);
            peer_tree.enable_fractional_index(0);
            let _node = peer_tree.create(None).unwrap();
            peer.commit();

            // Sync under write lock (same as StubSut)
            {
                let doc = &*primary;
                crate::sync::multi_peer::sync_docs_direct(&doc, &peer);
            }

            assert!(
                callback_count.load(Ordering::SeqCst) > 0,
                "subscribe_root should fire after sync under RwLock write guard"
            );

            // Sync under read lock (same as E2E SyncWithPeer)
            let count_before = callback_count.load(Ordering::SeqCst);
            let peer2 = LoroDoc::new();
            peer2.set_peer_id(3).unwrap();
            let peer2_tree = peer2.get_tree(TREE_NAME);
            peer2_tree.enable_fractional_index(0);
            let _n2 = peer2_tree.create(None).unwrap();
            peer2.commit();

            {
                let doc = &*primary;
                crate::sync::multi_peer::sync_docs_direct(&doc, &peer2);
            }

            assert!(
                callback_count.load(Ordering::SeqCst) > count_before,
                "subscribe_root should fire after sync under RwLock read guard"
            );
        });
    }
}
