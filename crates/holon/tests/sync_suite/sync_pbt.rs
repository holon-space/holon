//! Multi-peer sync property-based tests with ephemeral Loro oracle.
//!
//! Two modes:
//! - **Direct** (`test_multi_peer_sync_direct`): Uses Loro export/import
//!   directly. Fast, deterministic.
//! - **Iroh** (`test_multi_peer_sync_iroh`): Uses real Iroh QUIC transport.
//!   Catches protocol bugs.
//!
//! Both share the same transitions, invariants, and oracle from
//! `sync::multi_peer`.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use holon_loro::multi_peer::*;
    use proptest::prelude::*;
    use proptest_state_machine::ReferenceStateMachine;
    use proptest_state_machine::StateMachineTest;

    // -- Direct sync PBT (Loro export/import, no network) --

    struct DirectGroupState;

    impl ReferenceStateMachine for DirectGroupState {
        type State = GroupState<()>;
        type Transition = GroupTransition;

        fn init_state() -> BoxedStrategy<Self::State> {
            Just(GroupState::new(Arc::new(DirectSync))).boxed()
        }

        fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
            generate_transitions(state)
        }

        fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
            check_preconditions(state, transition)
        }

        fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
            apply_transition(state, transition)
        }
    }

    struct SyncPbtDirect;

    impl StateMachineTest for SyncPbtDirect {
        type SystemUnderTest = ();
        type Reference = DirectGroupState;

        fn init_test(
            _: &<Self::Reference as ReferenceStateMachine>::State,
        ) -> Self::SystemUnderTest {
        }

        fn apply(
            state: Self::SystemUnderTest,
            _: &<Self::Reference as ReferenceStateMachine>::State,
            _: <Self::Reference as ReferenceStateMachine>::Transition,
        ) -> Self::SystemUnderTest {
            state
        }

        fn check_invariants(
            _: &Self::SystemUnderTest,
            ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        ) {
            check_invariants(ref_state);
        }
    }

    // -- Iroh sync PBT (real QUIC transport) --

    #[cfg(feature = "iroh-sync")]
    struct IrohSyncBackend(holon_loro::iroh_sync_adapter::IrohSync);

    #[cfg(feature = "iroh-sync")]
    impl SyncBackend for IrohSyncBackend {
        fn sync_pair(&self, doc_a: &loro::LoroDoc, doc_b: &loro::LoroDoc) -> anyhow::Result<()> {
            holon_loro::iroh_sync_adapter::SyncBackend::sync_pair(&self.0, doc_a, doc_b)
        }
    }

    #[cfg(feature = "iroh-sync")]
    struct IrohGroupState;

    #[cfg(feature = "iroh-sync")]
    impl ReferenceStateMachine for IrohGroupState {
        type State = GroupState<()>;
        type Transition = GroupTransition;

        fn init_state() -> BoxedStrategy<Self::State> {
            let backend = holon_loro::iroh_sync_adapter::IrohSync::new()
                .expect("Failed to create IrohSync runtime");
            Just(GroupState::new(Arc::new(IrohSyncBackend(backend)))).boxed()
        }

        fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
            generate_transitions(state)
        }

        fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
            check_preconditions(state, transition)
        }

        fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
            apply_transition(state, transition)
        }
    }

    #[cfg(feature = "iroh-sync")]
    struct SyncPbtIroh;

    #[cfg(feature = "iroh-sync")]
    impl StateMachineTest for SyncPbtIroh {
        type SystemUnderTest = ();
        type Reference = IrohGroupState;

        fn init_test(
            _: &<Self::Reference as ReferenceStateMachine>::State,
        ) -> Self::SystemUnderTest {
        }

        fn apply(
            state: Self::SystemUnderTest,
            _: &<Self::Reference as ReferenceStateMachine>::State,
            _: <Self::Reference as ReferenceStateMachine>::Transition,
        ) -> Self::SystemUnderTest {
            state
        }

        fn check_invariants(
            _: &Self::SystemUnderTest,
            ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        ) {
            check_invariants(ref_state);
        }
    }

    // -- Test entry points --

    proptest_state_machine::prop_state_machine! {
        #![proptest_config(ProptestConfig {
            cases: 30,
            failure_persistence: None,
            timeout: 30000,
            verbose: 2,
            .. ProptestConfig::default()
        })]

        #[test]
        fn test_multi_peer_sync_direct(sequential 1..40 => SyncPbtDirect);
    }

    #[cfg(feature = "iroh-sync")]
    proptest_state_machine::prop_state_machine! {
        #![proptest_config(ProptestConfig {
            cases: 30,
            failure_persistence: None,
            timeout: 120000,
            verbose: 2,
            .. ProptestConfig::default()
        })]

        #[test]
        #[serial_test::serial]
        fn test_multi_peer_sync_iroh(sequential 1..15 => SyncPbtIroh);
    }

    // -- Subtree sharing PBT --
    //
    // Exercises the full share/accept round-trip through the production
    // `LoroShareBackend` + real iroh transport. Random edits are applied on
    // both sides after accept; after `sync_with_peers`, both shared docs
    // must converge structurally.
    //
    // Invariants per case:
    //   S-SHARE-1  B's shared doc contains every node from A's shared
    //              subtree immediately after `accept_shared_subtree`.
    //   S-SHARE-2  After random edits on A and/or B and a pull from B to A
    //              and A to B, the set of alive node contents matches.
    //   S-SHARE-3  The mount node on A stays connected and addressable by
    //              its stable id.

    #[cfg(feature = "iroh-sync")]
    mod share_subtree_pbt {
        use std::path::Path;
        use std::sync::Arc;

        use holon_api::EntityName;
        use holon_api::InlineMark;
        use holon_api::StorageEntity;
        use holon_api::Value;
        use holon_core::OperationProvider;
        use holon_loro::LoroBlockOperations;
        use holon_loro::degraded_signal_bus::DegradedChange;
        use holon_loro::degraded_signal_bus::DegradedSignalBus;
        use holon_loro::degraded_signal_bus::ShareDegraded;
        use holon_loro::degraded_signal_bus::ShareDegradedReason;
        use holon_loro::device_key_store::load_or_create_device_key;
        use holon_loro::iroh_advertiser::IrohAdvertiser;
        use holon_loro::iroh_sync_adapter::SharedTreeSyncManager;
        use holon_loro::loro_document_store::LoroDocumentStore;
        use holon_loro::loro_share_backend::LoroShareBackend;
        use holon_loro::loro_share_backend::SettleScope;
        use holon_loro::loro_share_backend::SubtreeShareOperations;
        use holon_loro::loro_share_backend::rehydrate_shared_trees;
        use holon_loro::multi_peer::TREE_NAME;
        use holon_loro::multi_peer::get_alive_nodes;
        use holon_loro::shared_snapshot_store::SharedSnapshotStore;
        use holon_loro::shared_tree::SharedTreeStore;
        use loro::LoroDoc;
        use loro::LoroText;
        use loro::TreeID;
        use loro::TreeParentId;
        use proptest::prelude::*;
        use serde_json::Value as JsonValue;
        use tempfile::TempDir;
        use tokio::sync::RwLock;
        use tokio::sync::broadcast;

        #[derive(Clone, Debug)]
        enum Action {
            EditOnA(String),
            EditOnB(String),
            SettleSaves,
            PullBtoA,
            RestartA,
            RestartB,
            CorruptSharedOnA,
            /// Restart A, then edit on B, then wait past the sync
            /// debounce and assert A picked up B's edit purely through
            /// the auto-resync worker — no explicit `PullBtoA` call.
            /// Validates that known_peers persist across restart and
            /// that B's local commit triggers sync to A.
            CrossPeerSyncAfterRestart(String),
            /// Apply an inline mark to the most-recent suffix on A's
            /// shared root text. Tests that *our* share/restart/sync code
            /// preserves marks — Phase 0.1 spike already verified Loro's
            /// CRDT merge semantics, so we don't re-test those here.
            ///
            /// Specifically: this surfaces missing `configure_text_styles`
            /// calls on shared docs created by accept / extract /
            /// gc / snapshot-load paths (each of which calls
            /// `LoroDoc::new()` directly, bypassing the global doc's
            /// configuration site). With config missing, mark behaviour
            /// defaults differ from spike S3's contract.
            MarkOnA(MarkKind),
            MarkOnB(MarkKind),
            /// Create a child under the MOUNT node — the shape a user drives.
            /// After a share the page the UI navigates to IS the mount, so the
            /// create carries the mount's id as `parent_id`.
            CreateUnderMountOnA(String),
            CreateUnderMountOnB(String),
            /// Create a child under the shared ROOT by its stable id (the id
            /// the reader resolves through the mount).
            CreateUnderSharedRootOnA(String),
            CreateUnderSharedRootOnB(String),
            /// Delete the most recently added child of the shared subtree.
            DeleteChildOnA,
            DeleteChildOnB,
            /// Re-parent the newest child under the oldest one — a structural
            /// move entirely INSIDE the shared subtree.
            MoveChildOnA,
        }

        /// Subset of `InlineMark` variants whose Loro-key is a single
        /// well-known string. The PBT exercises these because they're the
        /// load-bearing ones for editor UX (Cmd+B / Cmd+I / `=code=`).
        /// Link / Sub / Super / Verbatim are covered by unit tests.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum MarkKind {
            Bold,
            Italic,
            Code,
            Strike,
            Underline,
        }

        impl MarkKind {
            fn to_inline(self) -> InlineMark {
                match self {
                    MarkKind::Bold => InlineMark::Bold,
                    MarkKind::Italic => InlineMark::Italic,
                    MarkKind::Code => InlineMark::Code,
                    MarkKind::Strike => InlineMark::Strike,
                    MarkKind::Underline => InlineMark::Underline,
                }
            }

            fn loro_key(self) -> &'static str {
                self.to_inline().loro_key()
            }
        }

        /// One expected mark — recorded by the suffix it covers, not by
        /// fixed scalar offsets. Loro merges relocate text within the
        /// shared root, so re-resolving the suffix at check time is the
        /// honest invariant ("the mark is on whatever range *currently*
        /// matches this suffix") instead of a brittle fixed range that
        /// re-implements Loro's merge semantics in the test.
        #[derive(Clone, Debug)]
        struct ExpectedMark {
            suffix: String,
            key: &'static str,
        }

        /// Expected per-peer state for invariant checks.
        #[derive(Clone, Debug)]
        struct RefPeer {
            /// Suffixes that must appear in the shared doc's root text.
            alive_suffixes: Vec<String>,
            /// Marks the test applied to this peer's shared root that must
            /// survive every subsequent action (including restart / sync).
            expected_marks: Vec<ExpectedMark>,
            /// Children of the shared subtree this peer must be able to see,
            /// as `(block id, content)`. Seeded with the two pre-share
            /// children; structural ops add and remove entries.
            children: Vec<(String, String)>,
            /// Contents this peer must NOT see any more (structurally deleted).
            tombstoned: Vec<String>,
            /// `(child content, new parent content)` pairs this peer's tree
            /// must reflect after a move.
            reparented: Vec<(String, String)>,
            /// Share is expected to be in the manager + advertiser.
            share_registered: bool,
            /// Share is expected to be editable / content is intact.
            share_usable: bool,
            /// `CorruptSharedOnA` landed; next `Restart` will fail the load.
            corrupt_pending: bool,
        }

        impl RefPeer {
            fn initial() -> Self {
                Self {
                    alive_suffixes: Vec::new(),
                    expected_marks: Vec::new(),
                    children: vec![
                        ("block:child-1".to_string(), "Child 1".to_string()),
                        ("block:child-2".to_string(), "Child 2".to_string()),
                    ],
                    tombstoned: Vec::new(),
                    reparented: Vec::new(),
                    share_registered: true,
                    share_usable: true,
                    corrupt_pending: false,
                }
            }
        }

        /// Everything we need to reconstruct a backend on the same dir
        /// — used by `RestartA` / `RestartB` to simulate process restart
        /// while preserving on-disk state.
        /// A peer: the share backend plus the Loro document store it was built
        /// over. The store is what the production write provider
        /// (`LoroBlockOperations`) is constructed from, so the PBT drives block
        /// ops through the same store the share machinery mutates.
        struct Peer {
            be: Arc<LoroShareBackend>,
            store: Arc<RwLock<LoroDocumentStore>>,
        }

        impl std::ops::Deref for Peer {
            type Target = LoroShareBackend;

            fn deref(&self) -> &LoroShareBackend {
                &self.be
            }
        }

        impl Peer {
            /// The production write provider, wired exactly as DI wires it:
            /// this peer's doc store plus its shared-tree registry.
            fn ops(&self) -> LoroBlockOperations {
                LoroBlockOperations::new(self.store.clone())
                    .with_shared_trees(self.be.manager_for_test() as Arc<dyn SharedTreeStore>)
            }

            /// The wired write backend `LoroBlockOperations` delegates to.
            /// `move_block` is driven here rather than through
            /// `execute_operation`: the op-level mover needs a `BlockOrdering`
            /// that only the full DI dispatcher supplies, and the routing
            /// question this PBT asks lives in the backend either way.
            async fn backend(&self) -> holon_loro::loro_backend::LoroBackend {
                let global = self.be.test_global_doc().await;
                holon_loro::loro_backend::LoroBackend::from_document(global)
                    .with_shared_trees(self.be.manager_for_test() as Arc<dyn SharedTreeStore>)
            }
        }

        async fn backend_fresh(dir_path: &Path, bus: Arc<DegradedSignalBus>) -> Peer {
            let store = Arc::new(RwLock::new(LoroDocumentStore::new(dir_path.to_path_buf())));
            let snapshot_store = Arc::new(SharedSnapshotStore::new(
                dir_path.to_path_buf(),
                bus.clone(),
            ));
            let manager = Arc::new(SharedTreeSyncManager::new());
            let key = load_or_create_device_key(dir_path).unwrap();
            // Bind the advertiser endpoint to the persistent device
            // key so iroh endpoint identity survives restarts —
            // otherwise the remote side treats a rejoining peer as a
            // stranger and the known_peers dedup-by-id fails.
            let advertiser = Arc::new(IrohAdvertiser::new_with_key(key.clone()));
            // `LoroShareBackend::new` already returns `Arc<Self>`.
            let be =
                LoroShareBackend::new(store.clone(), snapshot_store, manager, advertiser, bus, key);
            Peer { be, store }
        }

        async fn backend_at(dir_path: &Path, bus: Arc<DegradedSignalBus>) -> Peer {
            let peer = backend_fresh(dir_path, bus).await;
            let be = &peer.be;
            let collab = be.test_global_doc().await;
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let _ = rehydrate_shared_trees(be, doc).await.unwrap();
            drop(doc_arc);
            peer
        }

        /// Initial backend — creates a fresh `TempDir`. Skips
        /// rehydration (nothing to rehydrate on a fresh dir).
        async fn backend() -> (Peer, Arc<DegradedSignalBus>, TempDir) {
            let dir = TempDir::new().unwrap();
            let bus = Arc::new(DegradedSignalBus::new());
            let peer = backend_fresh(dir.path(), bus.clone()).await;
            (peer, bus, dir)
        }

        /// The mount node's block URI in `be`'s global tree for
        /// `shared_tree_id`. This is the id the UI carries as
        /// `parent_id` when the user adds a block to a shared page:
        /// after a share (or an accept) the page in the tree IS the
        /// mount.
        async fn mount_uri(be: &LoroShareBackend, shared_tree_id: &str) -> String {
            let collab = be.test_global_doc().await;
            collab
                .with_read(|doc| {
                    let tree = doc.get_tree(TREE_NAME);
                    for n in tree.get_nodes(false) {
                        if matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
                            continue;
                        }
                        if let Ok(meta) = tree.get_meta(n.id)
                            && let Some(loro::ValueOrContainer::Value(v)) =
                                meta.get("shared_tree_id")
                            && v.as_string().map(|s| s.as_str()) == Some(shared_tree_id)
                        {
                            return Ok(holon_api::EntityUri::block_from_tree_id(
                                n.id.peer,
                                n.id.counter,
                            )
                            .to_string());
                        }
                    }
                    panic!("no mount node for {shared_tree_id}");
                })
                .unwrap()
        }

        /// Drive a block create through the production intent boundary
        /// (`OperationProvider::execute_operation`) — the same surface the
        /// dispatcher, MCP and the editor reach.
        async fn create_child(peer: &Peer, parent_uri: &str, id: &str, content: &str) {
            let mut params = StorageEntity::new();
            params.insert("parent_id".into(), Value::String(parent_uri.to_string()));
            params.insert("id".into(), Value::String(id.to_string()));
            params.insert("content".into(), Value::String(content.to_string()));
            peer.ops()
                .execute_operation(&EntityName::new("block"), "create", params)
                .await
                .unwrap_or_else(|e| panic!("create {id} under {parent_uri} failed: {e}"));
        }

        async fn delete_child(peer: &Peer, id: &str) {
            let mut params = StorageEntity::new();
            params.insert("id".into(), Value::String(id.to_string()));
            peer.ops()
                .execute_operation(&EntityName::new("block"), "delete_subtree", params)
                .await
                .unwrap_or_else(|e| panic!("delete_subtree {id} failed: {e}"));
        }

        async fn move_child(peer: &Peer, id: &str, new_parent: &str) {
            use holon_api::repository::CoreOperations;
            // ALLOW(entity_uri_from_raw): ids the test itself minted
            let (id_uri, parent_uri) = (
                holon_api::EntityUri::from_raw(id),
                holon_api::EntityUri::from_raw(new_parent),
            );
            peer.backend()
                .await
                .move_block(&id_uri, parent_uri, None)
                .await
                .unwrap_or_else(|e| panic!("move_block {id} under {new_parent} failed: {e}"));
        }

        /// `(content, parent content)` for every alive node in a shared doc.
        fn content_parents(doc: &LoroDoc) -> Vec<(String, String)> {
            let nodes = get_alive_nodes(doc);
            let by_id: std::collections::HashMap<TreeID, String> =
                nodes.iter().map(|(id, _, c)| (*id, c.clone())).collect();
            nodes
                .iter()
                .filter_map(|(_, parent, content)| {
                    parent
                        .and_then(|p| by_id.get(&p))
                        .map(|pc| (content.clone(), pc.clone()))
                })
                .collect()
        }

        async fn seed(be: &LoroShareBackend, stable_id: &str, parent: Option<&str>, content: &str) {
            let collab = be.test_global_doc().await;
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let tree = doc.get_tree(TREE_NAME);
            let parent_tid = parent.map(|pid| find(doc, pid).unwrap());
            let node = tree.create(parent_tid).unwrap();
            let meta = tree.get_meta(node).unwrap();
            meta.insert("id", loro::LoroValue::from(stable_id)).unwrap();
            let text: LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
            text.insert(0, content).unwrap();
            doc.commit();
        }

        fn find(doc: &LoroDoc, stable_id: &str) -> Option<TreeID> {
            let tree = doc.get_tree(TREE_NAME);
            for node in tree.get_nodes(false) {
                if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
                    continue;
                }
                if let Ok(meta) = tree.get_meta(node.id)
                    && let Some(loro::ValueOrContainer::Value(v)) = meta.get("id")
                    && v.as_string().map(|s| s.as_str()) == Some(stable_id)
                {
                    return Some(node.id);
                }
            }
            None
        }

        fn append_text_on_root(doc: &LoroDoc, extra: &str) {
            let tree = doc.get_tree(TREE_NAME);
            let root = tree.roots()[0];
            let meta = tree.get_meta(root).unwrap();
            let t = match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t,
                _ => panic!("shared root missing content_raw"),
            };
            let len = t.len_unicode();
            t.insert(len, extra).unwrap();
            doc.commit();
        }

        /// Locate `suffix` in the shared root text and apply `kind` over
        /// the matching scalar range. Returns `true` on success.
        ///
        /// Uses `holon_loro::mark_to_loro_value` so the
        /// applied mark value matches what the production
        /// `update_block_marked` / `apply_inline_mark` paths use — which
        /// is the same path the editor will eventually drive.
        fn mark_suffix_on_root(doc: &LoroDoc, suffix: &str, kind: MarkKind) -> bool {
            let tree = doc.get_tree(TREE_NAME);
            let root = tree.roots()[0];
            let meta = tree.get_meta(root).unwrap();
            let t = match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t,
                _ => return false,
            };
            let s = t.to_string();
            let Some(byte_pos) = s.find(suffix) else {
                return false;
            };
            // byte → char offset (Loro's mark expects scalar offsets).
            let start = s[..byte_pos].chars().count();
            let end = start + suffix.chars().count();
            if start == end {
                return false;
            }
            let mark = kind.to_inline();
            let value = holon_loro::mark_to_loro_value(&mark);
            t.mark(start..end, mark.loro_key(), value)
                .expect("LoroText mark");
            doc.commit();
            true
        }

        /// Read the marks Loro reports on the shared root text and check
        /// every `expected` is present.
        ///
        /// **Containment, not exact match.** Per the Phase 0.1 spike (S8),
        /// `ExpandType::After`-keyed marks extend their right boundary
        /// when text is inserted at that boundary. So after `MarkOnA(Bold)`
        /// over " [A:foo]" followed by `EditOnA(" [A:bar]")`, Bold legally
        /// covers " [A:foo] [A:bar]" — a *superset* of the original range.
        /// The invariant we want is "the mark we applied is still on
        /// (at least) the chars we applied it to" — i.e. there's an
        /// observed mark with the same key whose range covers the
        /// suffix's current location.
        ///
        /// Returns the list of (suffix, key) entries that are missing.
        fn missing_expected_marks(
            doc: &LoroDoc,
            expected: &[ExpectedMark],
        ) -> Vec<(String, &'static str)> {
            let tree = doc.get_tree(TREE_NAME);
            let root = tree.roots()[0];
            let meta = tree.get_meta(root).unwrap();
            let t = match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t,
                _ => return expected.iter().map(|m| (m.suffix.clone(), m.key)).collect(),
            };
            let observed = holon_loro::read_marks_from_text(&t);
            let s = t.to_string();
            let mut missing = Vec::new();
            for em in expected {
                let Some(byte_pos) = s.find(&em.suffix) else {
                    missing.push((em.suffix.clone(), em.key));
                    continue;
                };
                let want_start = s[..byte_pos].chars().count();
                let want_end = want_start + em.suffix.chars().count();
                let found = observed.iter().any(|m| {
                    m.mark.loro_key() == em.key && m.start <= want_start && m.end >= want_end
                });
                if !found {
                    missing.push((em.suffix.clone(), em.key));
                }
            }
            missing
        }

        fn node_contents(doc: &LoroDoc) -> Vec<String> {
            let nodes = get_alive_nodes(doc);
            let mut out: Vec<String> = nodes.into_iter().map(|(_, _, c)| c).collect();
            out.sort();
            out
        }

        /// Locate the mount node on the owner side (A) and return its
        /// `shared_tree_id`. Returns `None` when the mount is gone.
        async fn find_mount_id(be: &LoroShareBackend, shared_tree_id: &str) -> bool {
            let collab = be.test_global_doc().await;
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let tree = doc.get_tree(TREE_NAME);
            for n in tree.get_nodes(false) {
                if matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
                    continue;
                }
                if let Ok(meta) = tree.get_meta(n.id)
                    && let Some(loro::ValueOrContainer::Value(v)) = meta.get("shared_tree_id")
                    && v.as_string().map(|s| s.as_str()) == Some(shared_tree_id)
                {
                    return true;
                }
            }
            false
        }

        /// Drain any queued `ShareDegraded` events from a receiver
        /// without blocking. Used between actions to observe which
        /// degraded signals fired.
        fn drain_bus(rx: &mut broadcast::Receiver<DegradedChange>) -> Vec<ShareDegraded> {
            let mut out = Vec::new();
            while let Ok(change) = rx.try_recv() {
                if let Some(ev) = change.raised() {
                    out.push(ev);
                }
            }
            out
        }

        /// Scan `shares/` under `dir_path` for files that would
        /// indicate a broken save — 0-byte `.loro` files (P-NO-SILENT-CORRUPT)
        /// or leftover `.tmp` files (P-NO-TMP-LEFTOVER).
        fn scan_for_corruption(
            dir_path: &Path,
        ) -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>) {
            let shares = dir_path.join("shares");
            if !shares.exists() {
                return (vec![], vec![]);
            }
            let mut zero_byte = Vec::new();
            let mut tmps = Vec::new();
            for entry in std::fs::read_dir(&shares).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if name.ends_with(".loro.tmp") {
                    tmps.push(path.clone());
                }
                if name.ends_with(".loro")
                    && !name.contains(".corrupt-")
                    && entry.metadata().unwrap().len() == 0
                {
                    zero_byte.push(path);
                }
            }
            (zero_byte, tmps)
        }

        /// Check post-action invariants that apply every step.
        #[allow(clippy::too_many_arguments)] // exercises both peers' full state across an action
        async fn check_invariants(
            a: &LoroShareBackend,
            b: &LoroShareBackend,
            dir_a: &Path,
            dir_b: &Path,
            shared_tree_id: &str,
            ref_a: &RefPeer,
            ref_b: &RefPeer,
            expected_peer_id_a: u64,
            expected_peer_id_b: u64,
        ) {
            // P-NO-SILENT-CORRUPT + P-NO-TMP-LEFTOVER on A and B.
            for (label, dir) in [("A", dir_a), ("B", dir_b)] {
                let (zero, _tmps) = scan_for_corruption(dir);
                assert!(
                    zero.is_empty(),
                    "P-NO-SILENT-CORRUPT: 0-byte snapshot on {label}: {zero:?}"
                );
                // Note: P-NO-TMP-LEFTOVER is only reliable after
                // SettleSaves. We don't assert it here because a mid-
                // debounce check could legitimately see a .tmp mid-write.
            }

            // P-REG: manager matches ref.share_registered.
            assert_eq!(
                a.manager_for_test().get_doc(shared_tree_id).is_some(),
                ref_a.share_registered,
                "P-REG/A: manager registration diverged from ref"
            );
            assert_eq!(
                b.manager_for_test().get_doc(shared_tree_id).is_some(),
                ref_b.share_registered,
                "P-REG/B: manager registration diverged from ref"
            );

            // P-MOUNT on the owner side: mount node must persist
            // even after restart / corrupt. (Mount node lives in the
            // global doc, not the shared doc, so Corrupt on shared
            // snapshot shouldn't remove it.)
            if ref_a.share_registered || ref_a.corrupt_pending {
                assert!(
                    find_mount_id(a, shared_tree_id).await,
                    "P-MOUNT: mount lost on A for {shared_tree_id}"
                );
            }

            // P-KEY on each peer: BETWEEN restarts the shared doc's
            // peer_id is stable; the expected value is re-baselined at
            // each restart (see RestartA/RestartB), where B2's generation
            // bump re-mints it. A change here (outside a restart) would be
            // an accidental peer-id instability regression.
            if ref_a.share_usable {
                let d = a.manager_for_test().get_doc(shared_tree_id).unwrap();
                assert_eq!(
                    d.peer_id(),
                    expected_peer_id_a,
                    "P-KEY/A: shared doc peer_id changed outside a restart"
                );
            }
            if ref_b.share_usable {
                let d = b.manager_for_test().get_doc(shared_tree_id).unwrap();
                assert_eq!(
                    d.peer_id(),
                    expected_peer_id_b,
                    "P-KEY/B: shared doc peer_id changed outside a restart"
                );
            }

            // P-CONTENT: root text on each usable peer contains every
            // suffix in ref.alive_suffixes.
            if ref_a.share_usable {
                let d = a.manager_for_test().get_doc(shared_tree_id).unwrap();
                let root_text = root_text_of(&d);
                for s in &ref_a.alive_suffixes {
                    assert!(
                        root_text.contains(s),
                        "P-CONTENT/A: missing suffix {s:?} in {root_text:?}"
                    );
                }
            }
            if ref_b.share_usable {
                let d = b.manager_for_test().get_doc(shared_tree_id).unwrap();
                let root_text = root_text_of(&d);
                for s in &ref_b.alive_suffixes {
                    assert!(
                        root_text.contains(s),
                        "P-CONTENT/B: missing suffix {s:?} in {root_text:?}"
                    );
                }
            }

            // P-STRUCT on each usable peer (see `check_structure`).
            if ref_a.share_usable {
                let d = a.manager_for_test().get_doc(shared_tree_id).unwrap();
                check_structure("A", &d, ref_a, shared_tree_id);
            }
            if ref_b.share_usable {
                let d = b.manager_for_test().get_doc(shared_tree_id).unwrap();
                check_structure("B", &d, ref_b, shared_tree_id);
            }

            // P-MARKS: every mark recorded in `expected_marks` must be
            // observable on the shared root text — by suffix range +
            // Loro key. Surfaces:
            //   - shared docs created without `configure_text_styles` (mark API still
            //     applies but ExpandType policy is wrong)
            //   - snapshot save/load not preserving Peritext deltas
            //   - sync paths dropping mark deltas
            //   - rehydration creating a fresh LoroDoc that loses marks
            if ref_a.share_usable {
                let d = a.manager_for_test().get_doc(shared_tree_id).unwrap();
                let missing = missing_expected_marks(&d, &ref_a.expected_marks);
                if !missing.is_empty() {
                    let observed = observed_marks_dump(&d);
                    let text = root_text_of(&d);
                    panic!(
                        "P-MARKS/A: missing marks {missing:?} in shared doc {shared_tree_id}\n  \
                         text:     {text:?}\n  expected: {:?}\n  observed: {observed:?}",
                        ref_a.expected_marks
                    );
                }
            }
            if ref_b.share_usable {
                let d = b.manager_for_test().get_doc(shared_tree_id).unwrap();
                let missing = missing_expected_marks(&d, &ref_b.expected_marks);
                if !missing.is_empty() {
                    let observed = observed_marks_dump(&d);
                    let text = root_text_of(&d);
                    panic!(
                        "P-MARKS/B: missing marks {missing:?} in shared doc {shared_tree_id}\n  \
                         text:     {text:?}\n  expected: {:?}\n  observed: {observed:?}",
                        ref_b.expected_marks
                    );
                }
            }
        }

        /// Structural-write bookkeeping for the generator.
        ///
        /// Steers cases around the engine defect pinned by
        /// `structure_merged_against_a_concurrent_op_panics_the_shallow_share_engine`:
        /// on a shallow share, a tree create merged against ANY op the other
        /// peer made concurrently panics the fork — the other op does not have
        /// to be structural, a plain text edit is enough, and it panics in
        /// either order. Each rule below was forced by a shrunk counterexample
        /// from a randomized batch, and all of them exist only for that defect
        /// — delete them when the fork is fixed.
        #[derive(Default)]
        struct Structural {
            /// The one peer that authors structure in this case.
            ///
            /// A deliberate over-approximation: two authors with ONE handover
            /// converge, and only a third structural op panics
            /// (`A…sync…B…sync…A`). Allowing the handover is measurably not
            /// safe here though — under the live sync workers the generator
            /// cannot tell which side of that line a case lands on, and
            /// permitting it put 3 of 4 randomized runs into
            /// `tree_state.rs:1198`. So structure gets a single author until
            /// the fork is fixed, at which point all of these rules go.
            author: Option<char>,
            /// Per peer: does it hold local ops of ANY kind the other peer has
            /// not taken? BOTH can, so this cannot be one slot — an `A` edit
            /// followed by a `B` edit leaves both sides in flight, and a create
            /// on either of them is then the panicking merge.
            unsynced_a: bool,
            unsynced_b: bool,
            /// The peer holding an unsynced STRUCTURAL write.
            unsynced_structure_from: Option<char>,
        }

        impl Structural {
            fn unsynced(&self, who: char) -> bool {
                if who == 'A' {
                    self.unsynced_a
                } else {
                    self.unsynced_b
                }
            }

            fn mark_unsynced(&mut self, who: char) {
                if who == 'A' {
                    self.unsynced_a = true;
                } else {
                    self.unsynced_b = true;
                }
            }

            fn other(who: char) -> char {
                if who == 'A' { 'B' } else { 'A' }
            }

            /// May `who` author structure now? Only as the case's single
            /// author, and only when the other peer has nothing in flight for
            /// the create to be merged against.
            fn may_author_structure(&mut self, who: char) -> bool {
                let uncontended = !self.unsynced(Self::other(who));
                let mine = self.author.is_none_or(|current| current == who);
                if uncontended && mine {
                    self.author = Some(who);
                    self.mark_unsynced(who);
                    self.unsynced_structure_from = Some(who);
                    true
                } else {
                    false
                }
            }

            /// May `who` make an ordinary (non-structural) edit now? Not while
            /// the other peer's structural write is still in flight — that is
            /// the same merge, reached from the other side.
            fn may_edit(&mut self, who: char) -> bool {
                let blocked = self.unsynced_structure_from.is_some_and(|peer| peer != who);
                if !blocked {
                    self.mark_unsynced(who);
                }
                !blocked
            }

            fn synced(&mut self) {
                self.unsynced_a = false;
                self.unsynced_b = false;
                self.unsynced_structure_from = None;
            }
        }

        /// Drop every expectation that names `content`. A delete on ONE peer
        /// retracts the expectation on BOTH: the peers sync live (each shared
        /// doc has an auto-sync worker), so the other peer may lose the block
        /// at any moment without the case ever asking for a pull.
        fn retract(r: &mut RefPeer, content: &str) {
            r.children.retain(|(_, c)| c != content);
            r.reparented.retain(|(c, p)| c != content && p != content);
        }

        /// Index of the newest child that is nobody's parent — the only child a
        /// `delete_subtree` can remove without taking a tracked sibling with
        /// it.
        fn deletable_index(r: &RefPeer) -> Option<usize> {
            (0..r.children.len()).rev().find(|&i| {
                let content = &r.children[i].1;
                !r.reparented.iter().any(|(_, parent)| parent == content)
            })
        }

        /// P-STRUCT: the shared subtree's STRUCTURE on this peer matches the
        /// reference — every child it must see is alive in ITS shared doc,
        /// every deleted one is gone, and every moved one hangs under its new
        /// parent.
        ///
        /// **The convergence rule this oracle is written to:** the two peers
        /// sync LIVE, so an expectation only ever holds one way — a structural
        /// write is asserted on its author immediately and on the other peer
        /// after a sync, while a delete retracts the expectation on BOTH peers
        /// at once, because the other peer may converge on its own at any
        /// moment without the case asking for a pull.
        ///
        /// This is the invariant a mis-routed structural write breaks: a create
        /// that lands in the peer's own global doc reads back fine through the
        /// block API, but is absent from the shared doc — so it never leaves
        /// the device, and the peers diverge for good.
        fn check_structure(label: &str, doc: &LoroDoc, r: &RefPeer, shared_tree_id: &str) {
            let contents = node_contents(doc);
            for (id, content) in &r.children {
                assert!(
                    contents.contains(content),
                    "P-STRUCT/{label}: child {id} ({content:?}) missing from shared doc \
                     {shared_tree_id}; alive: {contents:?}"
                );
            }
            for content in &r.tombstoned {
                assert!(
                    !contents.contains(content),
                    "P-STRUCT/{label}: deleted child {content:?} still alive in shared doc \
                     {shared_tree_id}; alive: {contents:?}"
                );
            }
            if !r.reparented.is_empty() {
                let pairs = content_parents(doc);
                for (child, parent) in &r.reparented {
                    assert!(
                        pairs.contains(&(child.clone(), parent.clone())),
                        "P-STRUCT/{label}: {child:?} is not a child of {parent:?} in shared doc \
                         {shared_tree_id}; pairs: {pairs:?}"
                    );
                }
            }
        }

        /// Debug helper for P-MARKS failures.
        fn observed_marks_dump(doc: &LoroDoc) -> Vec<(usize, usize, &'static str)> {
            let tree = doc.get_tree(TREE_NAME);
            let roots = tree.roots();
            if roots.is_empty() {
                return Vec::new();
            }
            let meta = tree.get_meta(roots[0]).unwrap();
            let t = match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t,
                _ => return Vec::new(),
            };
            holon_loro::read_marks_from_text(&t)
                .into_iter()
                .map(|m| (m.start, m.end, m.mark.loro_key()))
                .collect()
        }

        fn root_text_of(doc: &LoroDoc) -> String {
            let tree = doc.get_tree(TREE_NAME);
            let roots = tree.roots();
            if roots.is_empty() {
                return String::new();
            }
            let meta = tree.get_meta(roots[0]).unwrap();
            match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t.to_string(),
                _ => String::new(),
            }
        }

        async fn run_case(actions: Vec<Action>) {
            let (mut a, bus_a, dir_a) = backend().await;
            let (mut b, bus_b, dir_b) = backend().await;

            // Subscribe to both buses BEFORE the initial share so we
            // catch any degraded events during the scenario. Slow
            // subscribers would lag; broadcast channel has capacity 64
            // which is plenty for our invariants.
            let mut rx_a = bus_a.subscribe().changes;
            let _rx_b = bus_b.subscribe().changes;

            seed(&a, "root-a", None, "root-a").await;
            seed(&a, "shared-parent", Some("root-a"), "Shared heading").await;
            seed(&a, "child-1", Some("shared-parent"), "Child 1").await;
            seed(&a, "child-2", Some("shared-parent"), "Child 2").await;
            seed(&b, "root-b", None, "root-b").await;

            let share_resp = a
                .share_subtree("block:shared-parent", "none".into())
                .await
                .unwrap();
            let j: JsonValue = match share_resp.response.unwrap() {
                Value::String(s) => serde_json::from_str(&s).unwrap(),
                _ => panic!(),
            };
            let ticket = j["ticket"].as_str().unwrap().to_string();
            let shared_tree_id = j["shared_tree_id"].as_str().unwrap().to_string();
            let accept_resp = b
                .accept_shared_subtree("block:root-b", ticket)
                .await
                .unwrap();
            assert!(accept_resp.response.is_some());

            // Capture each peer's initial shared-doc peer_id — they
            // differ because `stable_peer_id` derives from the
            // *device* key, and A and B are distinct devices. Post-B2 a
            // peer_id is stable BETWEEN restarts but is re-minted with a
            // fresh generation AT each restart (so a stale snapshot can
            // never reuse a CRDT counter). P-KEY tracks the current
            // expected value; RestartA/RestartB re-baseline it and assert
            // the mint actually changed.
            let mut expected_peer_id_a = a
                .manager_for_test()
                .get_doc(&shared_tree_id)
                .unwrap()
                .peer_id();
            let mut expected_peer_id_b = b
                .manager_for_test()
                .get_doc(&shared_tree_id)
                .unwrap()
                .peer_id();

            // S-SHARE-1: initial content on B mirrors A's shared subtree.
            let a_shared0 = a.manager_for_test().get_doc(&shared_tree_id).unwrap();
            let b_shared0 = b.manager_for_test().get_doc(&shared_tree_id).unwrap();
            assert_eq!(
                node_contents(&a_shared0),
                node_contents(&b_shared0),
                "S-SHARE-1: B did not mirror A after accept"
            );
            drop(a_shared0);
            drop(b_shared0);

            let mut ref_a = RefPeer::initial();
            let mut ref_b = RefPeer::initial();
            // Tracks whether a corrupt-then-restart has fired on A,
            // so at end-of-case we can assert P-DEGRADED-ON-CORRUPT.
            let mut expected_load_failures_on_a: usize = 0;
            // Makes every structurally created block's id and content unique
            // across the case, so the content-keyed oracle stays unambiguous.
            let mut seq: usize = 0;
            // Structural authorship bookkeeping — see `claim_structural`.
            let mut structural = Structural::default();

            for action in actions {
                match action {
                    Action::EditOnA(s) => {
                        if ref_a.share_usable && structural.may_edit('A') {
                            let d = a.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            append_text_on_root(&d, &s);
                            ref_a.alive_suffixes.push(s);
                            // Republishes A's snapshot, repairing a
                            // pending corruption. See `CorruptSharedOnA`.
                            ref_a.corrupt_pending = false;
                        }
                    }
                    Action::EditOnB(s) => {
                        if ref_b.share_usable && structural.may_edit('B') {
                            let d = b.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            append_text_on_root(&d, &s);
                            ref_b.alive_suffixes.push(s);
                        }
                    }
                    Action::SettleSaves => {
                        // P-NO-TMP-LEFTOVER: no `.tmp` should REMAIN. The
                        // temp half of an atomic publish is legal while
                        // that publish runs, so settle, sweep, and retry:
                        // a writer the settle does not cover can start one
                        // after it, but a real orphan never clears. The
                        // budget starts after the FIRST settle so the
                        // settle cannot consume it.
                        let mut deadline: Option<std::time::Instant> = None;
                        let mut stale: Vec<(&str, Vec<std::path::PathBuf>)> = Vec::new();
                        loop {
                            for (label, peer) in [("A", &a), ("B", &b)] {
                                tokio::time::timeout(
                                    std::time::Duration::from_secs(60),
                                    peer.wait_for_workers_idle(SettleScope::LocalWrites),
                                )
                                .await
                                .unwrap_or_else(|_| {
                                    panic!("SettleSaves/{label}: share workers never went idle")
                                });
                            }
                            let deadline = *deadline.get_or_insert_with(|| {
                                std::time::Instant::now() + std::time::Duration::from_secs(30)
                            });
                            stale.clear();
                            for (label, dir) in [("A", dir_a.path()), ("B", dir_b.path())] {
                                let (_zero, tmps) = scan_for_corruption(dir);
                                if !tmps.is_empty() {
                                    stale.push((label, tmps));
                                }
                            }
                            if stale.is_empty() || std::time::Instant::now() >= deadline {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        }
                        assert!(
                            stale.is_empty(),
                            "P-NO-TMP-LEFTOVER: tmp files still present 30s after the first \
                             settle: {stale:?}"
                        );
                    }
                    Action::PullBtoA => {
                        if ref_a.share_usable && ref_b.share_usable {
                            let a_shared = a.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            let b_shared = b.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            holon_loro::multi_peer::SyncBackend::sync_pair(
                                &holon_loro::multi_peer::DirectSync,
                                &a_shared,
                                &b_shared,
                            )
                            .unwrap();
                            // After merge, both sides' expected
                            // content is the union of the two suffix lists.
                            let mut merged = ref_a.alive_suffixes.clone();
                            for s in &ref_b.alive_suffixes {
                                if !merged.contains(s) {
                                    merged.push(s.clone());
                                }
                            }
                            ref_a.alive_suffixes = merged.clone();
                            ref_b.alive_suffixes = merged;
                            // Merging commits on A. See `CorruptSharedOnA`.
                            ref_a.corrupt_pending = false;

                            // Marks merge similarly — but suffixes have
                            // shifted positions on the *receiver's* side
                            // because the merged text now contains the
                            // other peer's edits inline. Rather than
                            // recompute scalar offsets (which would risk
                            // re-implementing Loro's merge in the test),
                            // re-resolve each expected mark by suffix:
                            // find it again in the post-merge text and
                            // store the new range. Marks whose suffix no
                            // longer exists are dropped (rare; Loro merge
                            // shouldn't drop chars).
                            // Marks merge as the union (deduped by
                            // suffix+key). Suffix-based resolution makes
                            // this index-free — at check time each side
                            // re-locates the suffix in the current text.
                            let mut merged_marks = ref_a.expected_marks.clone();
                            for m in &ref_b.expected_marks {
                                if !merged_marks
                                    .iter()
                                    .any(|x| x.suffix == m.suffix && x.key == m.key)
                                {
                                    merged_marks.push(m.clone());
                                }
                            }
                            ref_a.expected_marks = merged_marks.clone();
                            ref_b.expected_marks = merged_marks;

                            // Structure merges the same way: the union of both
                            // sides' children minus everything either side
                            // deleted, and the union of both sides' moves.
                            let mut children = ref_a.children.clone();
                            for c in &ref_b.children {
                                if !children.contains(c) {
                                    children.push(c.clone());
                                }
                            }
                            let mut tombstoned = ref_a.tombstoned.clone();
                            for t in &ref_b.tombstoned {
                                if !tombstoned.contains(t) {
                                    tombstoned.push(t.clone());
                                }
                            }
                            children.retain(|(_, content)| !tombstoned.contains(content));
                            let mut reparented = ref_a.reparented.clone();
                            for p in &ref_b.reparented {
                                if !reparented.contains(p) {
                                    reparented.push(p.clone());
                                }
                            }
                            reparented.retain(|(child, parent)| {
                                !tombstoned.contains(child) && !tombstoned.contains(parent)
                            });
                            ref_a.children = children.clone();
                            ref_b.children = children;
                            ref_a.tombstoned = tombstoned.clone();
                            ref_b.tombstoned = tombstoned;
                            ref_a.reparented = reparented.clone();
                            ref_b.reparented = reparented;
                            structural.synced();

                            // P-CONVERGE: node_contents equal after pull.
                            assert_eq!(
                                node_contents(&a_shared),
                                node_contents(&b_shared),
                                "P-CONVERGE: A and B diverged after PullBtoA"
                            );
                        }
                    }
                    Action::CorruptSharedOnA => {
                        // Truncate the on-disk snapshot to a handful of
                        // random bytes. In-memory state is unchanged
                        // until the next RestartA, when rehydration
                        // will fail to import and quarantine the file.
                        // Corruption survives only until A's next
                        // commit: a commit republishes the snapshot, and
                        // `RestartA` then flushes, so `corrupt_pending`
                        // is cleared by every action that commits on A.
                        // Settle first for the same reason — an armed
                        // save would republish over these bytes. The
                        // scope includes the sync worker here because
                        // its barrier save is one such writer.
                        tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            a.wait_for_workers_idle(SettleScope::IncludingSync),
                        )
                        .await
                        .expect("CorruptSharedOnA: A's share workers never went idle");
                        let path = dir_a
                            .path()
                            .join("shares")
                            .join(format!("{shared_tree_id}.loro"));
                        if path.exists() {
                            std::fs::write(&path, b"\x00\x01not-loro").unwrap();
                            ref_a.corrupt_pending = true;
                        }
                    }
                    Action::RestartA => {
                        a.advertiser_for_test().close_all().await;
                        // Flush pending saves BUT only if the snapshot
                        // is meant to survive. When `corrupt_pending`,
                        // we're simulating "power-loss right after the
                        // corruption, before any subsequent flush" —
                        // flushing would overwrite the corrupt bytes
                        // with a valid snapshot and defeat the test.
                        if !ref_a.corrupt_pending {
                            a.flush_all().await;
                        }
                        let old_bus = bus_a.clone();
                        drop(a);
                        a = backend_at(dir_a.path(), old_bus).await;
                        if ref_a.corrupt_pending {
                            // Rehydration failed; share is gone on A.
                            ref_a.share_registered = false;
                            ref_a.share_usable = false;
                            ref_a.corrupt_pending = false;
                            expected_load_failures_on_a += 1;
                        } else if ref_a.share_usable {
                            // B2: rehydrate re-mints the peer_id under a fresh
                            // generation so a stale snapshot can never reuse a
                            // CRDT counter. Assert it changed, then re-baseline.
                            let new_pid = a
                                .manager_for_test()
                                .get_doc(&shared_tree_id)
                                .expect("A's shared doc re-registered after restart")
                                .peer_id();
                            assert_ne!(
                                new_pid, expected_peer_id_a,
                                "B2: A's shared-doc peer_id must change across restart \
                                 (generation bump)"
                            );
                            expected_peer_id_a = new_pid;
                        }
                    }
                    Action::RestartB => {
                        b.advertiser_for_test().close_all().await;
                        b.flush_all().await;
                        let old_bus = bus_b.clone();
                        drop(b);
                        b = backend_at(dir_b.path(), old_bus).await;
                        if ref_b.share_usable {
                            // B2: peer_id re-minted with a fresh generation on
                            // restart (see RestartA).
                            let new_pid = b
                                .manager_for_test()
                                .get_doc(&shared_tree_id)
                                .expect("B's shared doc re-registered after restart")
                                .peer_id();
                            assert_ne!(
                                new_pid, expected_peer_id_b,
                                "B2: B's shared-doc peer_id must change across restart \
                                 (generation bump)"
                            );
                            expected_peer_id_b = new_pid;
                        }
                    }
                    Action::MarkOnA(kind) => {
                        if ref_a.share_usable
                            && structural.may_edit('A')
                            && let Some(suffix) = ref_a.alive_suffixes.last().cloned()
                        {
                            let d = a.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            if mark_suffix_on_root(&d, &suffix, kind) {
                                ref_a.expected_marks.push(ExpectedMark {
                                    suffix,
                                    key: kind.loro_key(),
                                });
                            }
                            // Republishes A's snapshot, repairing a
                            // pending corruption. See `CorruptSharedOnA`.
                            ref_a.corrupt_pending = false;
                        }
                    }
                    Action::MarkOnB(kind) => {
                        if ref_b.share_usable
                            && structural.may_edit('B')
                            && let Some(suffix) = ref_b.alive_suffixes.last().cloned()
                        {
                            let d = b.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            if mark_suffix_on_root(&d, &suffix, kind) {
                                ref_b.expected_marks.push(ExpectedMark {
                                    suffix,
                                    key: kind.loro_key(),
                                });
                            }
                        }
                    }
                    Action::CrossPeerSyncAfterRestart(s) => {
                        // Skip if either side can't currently mutate —
                        // e.g. after a corrupt-then-restart. We only
                        // want to exercise the known_peers + auto-resync
                        // path, not the rehydration-failure path.
                        //
                        // Also skip when `corrupt_pending` is set on A:
                        // this action calls `flush_all()` internally,
                        // which overwrites the on-disk corrupt snapshot
                        // with a fresh valid one — defeating the test
                        // intent of `CorruptSharedOnA`. The interaction
                        // is a harness ambiguity, not a production bug.
                        // This action edits on B, so it is a B mutation for the
                        // in-flight-structure rule.
                        if !ref_a.share_usable
                            || !ref_b.share_usable
                            || ref_a.corrupt_pending
                            || !structural.may_edit('B')
                        {
                            continue;
                        }

                        // Restart A. Flush saves first so the sidecar
                        // (known_peers) is on disk.
                        a.advertiser_for_test().close_all().await;
                        a.flush_all().await;
                        let old_bus = bus_a.clone();
                        drop(a);
                        a = backend_at(dir_a.path(), old_bus).await;

                        // B2: this restart also re-mints A's peer_id under a
                        // fresh generation — re-baseline P-KEY (share_usable is
                        // guaranteed by the guard above).
                        {
                            let new_pid = a
                                .manager_for_test()
                                .get_doc(&shared_tree_id)
                                .expect("A's shared doc re-registered after restart")
                                .peer_id();
                            assert_ne!(
                                new_pid, expected_peer_id_a,
                                "B2: A's shared-doc peer_id must change across restart \
                                 (generation bump)"
                            );
                            expected_peer_id_a = new_pid;
                        }

                        // After restart, A's sidecar should still list
                        // B as a known peer (populated during the
                        // initial accept). Without that, auto-resync on
                        // B has nowhere to dial.
                        let peers_on_a = a
                            .snapshot_store()
                            .load_peers(&shared_tree_id)
                            .expect("load_peers after RestartA");
                        assert!(
                            !peers_on_a.is_empty(),
                            "known_peers sidecar empty on A after restart — persistence regressed"
                        );

                        // Edit on B. The auto-resync worker on B picks
                        // up the Local commit, debounces, and dials A.
                        let d = b.manager_for_test().get_doc(&shared_tree_id).unwrap();
                        append_text_on_root(&d, &s);
                        ref_a.alive_suffixes.push(s.clone());
                        ref_b.alive_suffixes.push(s.clone());

                        // Wait past the sync debounce (500 ms) + sync
                        // round-trip. Because B's auto-resync also
                        // coalesces EditOnB bursts, the 500 ms debounce
                        // is the floor, but a 2 s budget covers
                        // endpoint setup + VV exchange on a loaded
                        // worker machine.
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(5);
                        loop {
                            let a_doc = a
                                .manager_for_test()
                                .get_doc(&shared_tree_id)
                                .expect("A's shared doc re-registered after restart");
                            if root_text_of(&a_doc).contains(&s) {
                                break;
                            }
                            if std::time::Instant::now() >= deadline {
                                panic!(
                                    "CrossPeerSyncAfterRestart: A did not pick up B's edit {s:?} \
                                     within 5s via auto-resync"
                                );
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                        // A demonstrably holds B's edit now: the peers are in
                        // sync, so nothing is in flight any more.
                        structural.synced();
                    }
                    Action::CreateUnderMountOnA(s) => {
                        if ref_a.share_usable && structural.may_author_structure('A') {
                            seq += 1;
                            let parent = mount_uri(&a, &shared_tree_id).await;
                            let (id, content) = (format!("block:s{seq}"), format!("{s}-{seq}"));
                            create_child(&a, &parent, &id, &content).await;
                            ref_a.children.push((id, content));
                        }
                    }
                    Action::CreateUnderMountOnB(s) => {
                        if ref_b.share_usable && structural.may_author_structure('B') {
                            seq += 1;
                            let parent = mount_uri(&b, &shared_tree_id).await;
                            let (id, content) = (format!("block:s{seq}"), format!("{s}-{seq}"));
                            create_child(&b, &parent, &id, &content).await;
                            ref_b.children.push((id, content));
                        }
                    }
                    Action::CreateUnderSharedRootOnA(s) => {
                        if ref_a.share_usable && structural.may_author_structure('A') {
                            seq += 1;
                            let (id, content) = (format!("block:s{seq}"), format!("{s}-{seq}"));
                            create_child(&a, "block:shared-parent", &id, &content).await;
                            ref_a.children.push((id, content));
                        }
                    }
                    Action::CreateUnderSharedRootOnB(s) => {
                        if ref_b.share_usable && structural.may_author_structure('B') {
                            seq += 1;
                            let (id, content) = (format!("block:s{seq}"), format!("{s}-{seq}"));
                            create_child(&b, "block:shared-parent", &id, &content).await;
                            ref_b.children.push((id, content));
                        }
                    }
                    Action::DeleteChildOnA => {
                        // Only a leaf: deleting a node that other tracked
                        // children hang under would take them with it, which
                        // the content-keyed reference model does not model.
                        if ref_a.share_usable
                            && structural.may_author_structure('A')
                            && let Some(idx) = deletable_index(&ref_a)
                        {
                            let (id, content) = ref_a.children.remove(idx);
                            delete_child(&a, &id).await;
                            retract(&mut ref_a, &content);
                            retract(&mut ref_b, &content);
                            ref_a.tombstoned.push(content);
                        }
                    }
                    Action::DeleteChildOnB => {
                        if ref_b.share_usable
                            && structural.may_author_structure('B')
                            && let Some(idx) = deletable_index(&ref_b)
                        {
                            let (id, content) = ref_b.children.remove(idx);
                            delete_child(&b, &id).await;
                            retract(&mut ref_a, &content);
                            retract(&mut ref_b, &content);
                            ref_b.tombstoned.push(content);
                        }
                    }
                    Action::MoveChildOnA => {
                        if ref_a.share_usable
                            && structural.may_author_structure('A')
                            && ref_a.children.len() >= 2
                        {
                            let (child_id, child_content) = ref_a.children.last().cloned().unwrap();
                            let (parent_id, parent_content) =
                                ref_a.children.first().cloned().unwrap();
                            move_child(&a, &child_id, &parent_id).await;
                            ref_a.reparented.push((child_content, parent_content));
                        }
                    }
                }

                check_invariants(
                    &a,
                    &b,
                    dir_a.path(),
                    dir_b.path(),
                    &shared_tree_id,
                    &ref_a,
                    &ref_b,
                    expected_peer_id_a,
                    expected_peer_id_b,
                )
                .await;
            }

            // P-DEGRADED-ON-CORRUPT: every Corrupt→Restart sequence
            // on A must have produced a SnapshotLoadFailed on the bus.
            let evs = drain_bus(&mut rx_a);
            let load_failures = evs
                .iter()
                .filter(|e| matches!(e.reason, ShareDegradedReason::SnapshotLoadFailed(_)))
                .count();
            assert!(
                load_failures >= expected_load_failures_on_a,
                "P-DEGRADED-ON-CORRUPT: expected ≥{expected_load_failures_on_a} \
                 SnapshotLoadFailed events, saw {load_failures} (events: {evs:?})"
            );

            a.advertiser_for_test().close_all().await;
            b.advertiser_for_test().close_all().await;
        }

        fn arbitrary_mark_kind() -> impl Strategy<Value = MarkKind> {
            prop_oneof![
                Just(MarkKind::Bold),
                Just(MarkKind::Italic),
                Just(MarkKind::Code),
                Just(MarkKind::Strike),
                Just(MarkKind::Underline),
            ]
        }

        fn actions_strategy() -> impl Strategy<Value = Vec<Action>> {
            let edit_a = "[a-z]{1,6}".prop_map(|s| Action::EditOnA(format!(" [A:{s}]")));
            let edit_b = "[a-z]{1,6}".prop_map(|s| Action::EditOnB(format!(" [B:{s}]")));
            let cross_peer =
                "[a-z]{1,6}".prop_map(|s| Action::CrossPeerSyncAfterRestart(format!(" [X:{s}]")));
            let mark_a = arbitrary_mark_kind().prop_map(Action::MarkOnA);
            let mark_b = arbitrary_mark_kind().prop_map(Action::MarkOnB);
            let mount_a = "[a-z]{1,6}".prop_map(|s| Action::CreateUnderMountOnA(format!("mA:{s}")));
            let mount_b = "[a-z]{1,6}".prop_map(|s| Action::CreateUnderMountOnB(format!("mB:{s}")));
            let root_a =
                "[a-z]{1,6}".prop_map(|s| Action::CreateUnderSharedRootOnA(format!("rA:{s}")));
            let root_b =
                "[a-z]{1,6}".prop_map(|s| Action::CreateUnderSharedRootOnB(format!("rB:{s}")));
            let step = prop_oneof![
                6 => edit_a,
                6 => edit_b,
                3 => Just(Action::SettleSaves),
                4 => Just(Action::PullBtoA),
                2 => Just(Action::RestartA),
                2 => Just(Action::RestartB),
                3 => mark_a,
                3 => mark_b,
                1 => Just(Action::CorruptSharedOnA),
                1 => cross_peer,
                4 => mount_a,
                4 => mount_b,
                3 => root_a,
                3 => root_b,
                2 => Just(Action::DeleteChildOnA),
                2 => Just(Action::DeleteChildOnB),
                2 => Just(Action::MoveChildOnA),
            ];
            prop::collection::vec(step, 0..8)
        }

        // Regression test for the auto-resync-after-dual-restart race
        // (pinned proptest seed cc 8888f981): with relay and discovery
        // disabled, consecutive restarts of both peers used to leave
        // both sides holding stale socket addrs with no repair path —
        // B's debounced auto-resync dialed a dead port and never
        // retried. Fixed by the restart-stable advertiser port
        // (`start_advertising_stable`), addr-set merging in
        // `remember_peer`, and the retrying rehydrate kick-sync.
        // Run directly (no proptest, no fork) so RUST_LOG tracing
        // reaches stderr when debugging.
        #[test]
        fn cross_peer_sync_after_restart_repro() {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
                .try_init();
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(run_case(vec![
                Action::RestartA,
                Action::RestartB,
                Action::CrossPeerSyncAfterRestart(" [X:a]".to_string()),
            ]));
        }

        /// Deterministic regression for the structural-routing defect the
        /// 2026-09-02 two-instance dogfood found: a block created inside a
        /// shared subtree went to the peer's own global doc, so it never
        /// reached the other device — silently and permanently. Covers create
        /// under the mount (the shape the UI drives), create under the shared
        /// root, a move inside the subtree, and a delete — each followed by a
        /// sync, so the peer must see every one of them.
        #[test]
        fn structural_edits_in_a_shared_subtree_reach_the_peer() {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(run_case(vec![
                Action::CreateUnderMountOnA("mA:one".to_string()),
                Action::PullBtoA,
                Action::CreateUnderSharedRootOnA("rA:two".to_string()),
                Action::PullBtoA,
                Action::CreateUnderMountOnA("mA:three".to_string()),
                Action::PullBtoA,
                Action::MoveChildOnA,
                Action::PullBtoA,
                Action::DeleteChildOnA,
                Action::PullBtoA,
            ]));
        }

        /// Pins an ENGINE defect this lane found and does NOT fix: on a
        /// shallow-history shared doc, a tree node created on one peer panics
        /// loro's tree-diff when it is merged against ANY op the other peer
        /// made concurrently — another create, or merely a text edit
        /// (`tree_state.rs:apply_diff_and_convert`,
        /// `is_node_deleted(target).unwrap()` on a node the receiving state
        /// never saw), which then poisons the doc mutex.
        ///
        /// It is not Holon write routing: the reproducer below drives raw
        /// `LoroTree::create` and `LoroText::insert` on the shared docs,
        /// bypassing every Holon write path. Concurrency is what it needs —
        /// the same ops with a sync in between converge.
        ///
        /// Shares are always shallow in production (`retention = "full"` is
        /// refused since the B1 history-leak fix), so this is reachable from
        /// the UI the moment one person adds a block while the other types.
        /// Fixing it means either patching the loro fork's shallow-doc tree
        /// diff or exporting shares with subtree-only history instead of a
        /// shallow snapshot (the B1 remedy in
        /// `docs/Reference/SUBTREE_SHARING.md`) — a design change outside this
        /// lane, which owns write routing only.
        ///
        /// This is also what `Structural::may_author_structure` and
        /// `Structural::may_edit` steer the generator around.
        #[test]
        #[ignore = "known engine defect: a tree create merged against a concurrent op panics loro on a shallow share"]
        fn structure_merged_against_a_concurrent_op_panics_the_shallow_share_engine() {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let (a, _bus_a, _da) = backend().await;
                let (b, _bus_b, _db) = backend().await;
                seed(&a, "root-a", None, "root-a").await;
                seed(&a, "shared-parent", Some("root-a"), "Shared heading").await;
                seed(&a, "child-1", Some("shared-parent"), "Child 1").await;
                seed(&b, "root-b", None, "root-b").await;
                let resp = a
                    .share_subtree("block:shared-parent", "none".into())
                    .await
                    .unwrap();
                let j: JsonValue = match resp.response.unwrap() {
                    Value::String(s) => serde_json::from_str(&s).unwrap(),
                    _ => panic!(),
                };
                let ticket = j["ticket"].as_str().unwrap().to_string();
                let stid = j["shared_tree_id"].as_str().unwrap().to_string();
                b.accept_shared_subtree("block:root-b", ticket)
                    .await
                    .unwrap();

                fn raw_create(doc: &LoroDoc, label: &str) {
                    let tree = doc.get_tree(TREE_NAME);
                    let root = tree.roots()[0];
                    let node = tree.create(Some(root)).unwrap();
                    let meta = tree.get_meta(node).unwrap();
                    meta.insert("id", loro::LoroValue::from(label)).unwrap();
                    let t: LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
                    t.insert(0, label).unwrap();
                    doc.commit();
                }

                let da = a.manager_for_test().get_doc(&stid).unwrap();
                let db = b.manager_for_test().get_doc(&stid).unwrap();
                raw_create(&da, "concurrent-on-a");
                // Not even a structural op on the other side — a plain text
                // edit on the shared root is enough.
                append_text_on_root(&db, " [B:typing]");
                holon_loro::multi_peer::SyncBackend::sync_pair(
                    &holon_loro::multi_peer::DirectSync,
                    &da,
                    &db,
                )
                .unwrap();
                assert_eq!(node_contents(&da), node_contents(&db));
                a.advertiser_for_test().close_all().await;
                b.advertiser_for_test().close_all().await;
            });
        }

        // -------------------------------------------------------------
        // Inc 0b — the REPLICATE-ALL whole-vault path, shallow variant.
        // -------------------------------------------------------------
        //
        // The Inc 0 experiment (`two_instance_composed_pbt.rs`) proved the
        // replicate-all path converges under concurrent structure + text when
        // both peers' docs carry full history. But `save_all` writes a SHALLOW
        // snapshot on the FIRST save of every session and every 64th after
        // (`loro_document_store.rs:204-220`, `COMPACT_EVERY = 64`, kill-switch
        // `HOLON_LORO_COMPACT=off`), so after a restart the vault doc loads
        // shallow. "Replicate-all is the full-lineage path" therefore holds
        // only until the next session start.
        //
        // These tests re-run the same question with the OWNER's doc genuinely
        // shallow — saved compact, then reloaded through the production
        // `LoroDocumentStore` load path via `backend_at`. That restart seam is
        // why the variant lives HERE and not in the two-instance slice: the
        // composed session caches `Arc<LoroDocument>` inside `LoroBackend`
        // (`loro_backend.rs:1983`), so swapping the store's doc slot would
        // desync the session rather than restart it.
        //
        // The ops are raw `LoroTree` / `LoroText` calls, exactly as the D70
        // pin above argues: the question is about the ENGINE's merge, so
        // bypassing every Holon write path is what isolates it.

        /// A peer's whole tree as a peer-independent, sorted shape: stable id,
        /// stable parent id, and text. Comparing `TreeID`s directly would
        /// compare peer ids, which legitimately differ.
        fn tree_shape(doc: &LoroDoc) -> Vec<String> {
            let tree = doc.get_tree(TREE_NAME);
            let stable = |tid: TreeID| -> String {
                match tree.get_meta(tid) {
                    Ok(meta) => match meta.get("id") {
                        Some(loro::ValueOrContainer::Value(v)) => v
                            .as_string()
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_else(|| "<unnamed>".to_string()),
                        _ => "<unnamed>".to_string(),
                    },
                    Err(_) => "<gone>".to_string(),
                }
            };
            let mut out: Vec<String> = get_alive_nodes(doc)
                .into_iter()
                .map(|(id, parent, content)| {
                    let p = parent.map(stable).unwrap_or_else(|| "<root>".to_string());
                    format!("{} parent={p} text={content:?}", stable(id))
                })
                .collect();
            out.sort();
            out
        }

        fn raw_create_under(doc: &LoroDoc, parent_stable: &str, label: &str) {
            let tree = doc.get_tree(TREE_NAME);
            let parent = find(doc, parent_stable)
                .unwrap_or_else(|| panic!("no node with stable id {parent_stable}"));
            let node = tree.create(Some(parent)).unwrap();
            let meta = tree.get_meta(node).unwrap();
            meta.insert("id", loro::LoroValue::from(label)).unwrap();
            let t: LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
            t.insert(0, label).unwrap();
            doc.commit();
        }

        /// Reparent `child_stable` under `new_parent_stable` — the tree-MOVE
        /// arm, which is the shape `tree_state.rs` resolves and the one D70
        /// panics on.
        fn raw_move(doc: &LoroDoc, child_stable: &str, new_parent_stable: &str) {
            let tree = doc.get_tree(TREE_NAME);
            let child = find(doc, child_stable).unwrap();
            let parent = find(doc, new_parent_stable).unwrap();
            tree.mov(child, Some(parent)).unwrap();
            doc.commit();
        }

        fn raw_append_text(doc: &LoroDoc, stable_id: &str, extra: &str) {
            let tree = doc.get_tree(TREE_NAME);
            let node = find(doc, stable_id).unwrap();
            let meta = tree.get_meta(node).unwrap();
            let t: LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
            let len = t.len_unicode();
            t.insert(len, extra).unwrap();
            doc.commit();
        }

        /// One direction of a replicate-all round, over the SAME production
        /// functions the two-instance slice drives: `push_once` exports
        /// `ExportMode::updates_owned(from)` and `pull_once` admits and
        /// imports. Returns `(pushed, imported, refusals)`.
        #[allow(clippy::too_many_arguments)]
        async fn replicate_round(
            from: &holon_loro::ContainerRegistry,
            to: &holon_loro::ContainerRegistry,
            relay: &holon_loro::sync_transport::InMemoryRelay,
            from_session: &mut holon_sharing::sync::SyncSession,
            to_session: &mut holon_sharing::sync::SyncSession,
            sender: u64,
            chain: &holon_sharing::lease::MembershipChain,
            clock: &holon_api::TestClock,
        ) -> (usize, usize, Vec<String>) {
            use holon_sharing::acceptor::AcceptorContext;
            use holon_sharing::policy::Principal;
            use holon_sharing::policy::UnverifiedVerifier;
            use holon_sharing::sync::OutboundAuth;

            let audience = Principal("peer".to_string());
            let auth = OutboundAuth {
                sender: holon_loro::sync_transport::StablePeerId(sender),
                audience: audience.clone(),
                epoch: 0,
                chain: chain.clone(),
            };
            let push = holon_sharing::sync::push_once(from, relay, from_session, &auth)
                .await
                .expect(
                    "push_once must surface an export failure as Err; a shallow doc that cannot \
                     export its un-pushed delta is a FINDING, not something to swallow",
                );
            let ctx = AcceptorContext {
                receiver: &audience,
                clock,
                verifier: &UnverifiedVerifier,
            };
            let pull = holon_sharing::sync::pull_once(to, relay, to_session, &ctx)
                .await
                .expect("pull_once must surface a transport or import failure as Err");
            (
                push.pushed.len(),
                pull.imported.len(),
                pull.refusals
                    .iter()
                    .map(|(c, d)| format!("{c}: {d:?}"))
                    .collect(),
            )
        }

        /// The exchange the PRODUCTION iroh leg performs, mirrored here
        /// clause for clause from
        /// `iroh_sync_adapter::export_delta_or_full_snapshot`
        /// (`iroh_sync_adapter.rs:77-105`), which is private to that module:
        /// a peer that does not include our shallow base gets a self-contained
        /// SNAPSHOT; everyone else gets `ExportMode::updates(peer_vv)`; an
        /// `Err` from `updates` also falls back to a snapshot.
        ///
        /// The relay leg (`push_once`) has NO such guard — that difference is
        /// exactly what the two shallow variants below measure.
        fn export_like_iroh(doc: &LoroDoc, peer_vv: &loro::VersionVector) -> Vec<u8> {
            if doc.is_shallow() {
                let base = doc.shallow_since_vv().to_vv();
                if !peer_vv.includes_vv(&base) {
                    return doc.export(loro::ExportMode::Snapshot).unwrap();
                }
            }
            match doc.export(loro::ExportMode::updates(peer_vv)) {
                Ok(delta) => delta,
                Err(_) => doc.export(loro::ExportMode::Snapshot).unwrap(),
            }
        }

        fn sync_pair_like_iroh(a: &LoroDoc, b: &LoroDoc) {
            let import = |dst: &LoroDoc, payload: &[u8], side: &str, other: &LoroDoc| {
                if payload.is_empty() {
                    return;
                }
                if let Err(e) = dst.import(payload) {
                    panic!(
                        "[{side}] importing {} bytes FAILED: {e:?}\n  destination shallow={} \
                         since={:?}\n  source shallow={} since={:?}",
                        payload.len(),
                        dst.is_shallow(),
                        dst.shallow_since_vv().to_vv(),
                        other.is_shallow(),
                        other.shallow_since_vv().to_vv(),
                    );
                }
            };
            let to_b = export_like_iroh(a, &b.oplog_vv());
            import(b, &to_b, "owner->receiver", a);
            let to_a = export_like_iroh(b, &a.oplog_vv());
            import(a, &to_a, "receiver->owner", b);
        }

        /// `sync_pair_like_iroh` at the doc-boundary layer: both sides import,
        /// so both need the write guard. The two docs are distinct `Arc`s and
        /// therefore distinct locks, so the nesting cannot self-deadlock.
        fn pair_like_iroh(
            a: &holon_loro::loro_document::LoroDocument,
            b: &holon_loro::loro_document::LoroDocument,
        ) {
            a.with_write_origin("pair_probe", |doc_a| {
                b.with_write_origin("pair_probe", |doc_b| {
                    sync_pair_like_iroh(doc_a, doc_b);
                    Ok(())
                })
            })
            .unwrap();
        }

        /// Run the Inc 0b experiment. `shallow` decides whether the owner
        /// restarts over a compacted snapshot first; `iroh_like` picks the
        /// PRODUCTION exchange (snapshot fallback) over the relay leg.
        ///
        /// Returns `(owner_shape, receiver_shape, owner_doc_was_shallow)`.
        /// The axes of the Inc 0b experiment. A struct rather than five bools
        /// so a call site reads as the variant it is.
        #[derive(Clone, Copy)]
        struct PairVariant {
            /// Compact the owner's snapshot before restarting it.
            compact_owner: bool,
            /// Use the production iroh exchange (snapshot fallback) instead of
            /// the `push_once` relay leg.
            iroh_like: bool,
            /// Give the receiver a seed of its own before pairing.
            receiver_has_own_history: bool,
            /// Let the receiver type into content that predates the compaction.
            receiver_edits_pretrim_text: bool,
            /// Set `HOLON_LORO_COMPACT=off` before the owner's save — the crude
            /// stopgap. With it, the save is full and the restart is not
            /// shallow.
            compaction_disabled: bool,
        }

        impl PairVariant {
            fn shallow() -> Self {
                Self {
                    compact_owner: true,
                    iroh_like: true,
                    receiver_has_own_history: true,
                    receiver_edits_pretrim_text: true,
                    compaction_disabled: false,
                }
            }
        }

        async fn run_replicate_all_pair(v: PairVariant) -> (Vec<String>, Vec<String>, bool) {
            use holon_sharing::lease::Issuer;
            use holon_sharing::lease::Lease;
            use holon_sharing::lease::MembershipCert;
            use holon_sharing::lease::MembershipChain;
            use holon_sharing::policy::Capabilities;
            use holon_sharing::policy::Principal;
            use holon_sharing::types::BlockId;
            use holon_sharing::types::UnverifiedAuthority;

            let dir_a = TempDir::new().unwrap();
            let bus_a = Arc::new(DegradedSignalBus::new());
            let a = backend_fresh(dir_a.path(), bus_a.clone()).await;
            seed(&a, "root-a", None, "root-a").await;
            seed(&a, "p1", Some("root-a"), "Parent one").await;
            seed(&a, "c1", Some("p1"), "Child one").await;
            seed(&a, "c2", Some("p1"), "Child two").await;

            let PairVariant {
                compact_owner,
                iroh_like,
                receiver_has_own_history,
                receiver_edits_pretrim_text,
                compaction_disabled,
            } = v;

            if compaction_disabled {
                // SAFETY: nextest runs every test in its own process, so this
                // never races another test's environment.
                unsafe { std::env::set_var("HOLON_LORO_COMPACT", "off") };
            }

            let a = if compact_owner {
                // The FIRST `save_all` of a store compacts (counter starts at
                // 0, and 0 is a multiple of 64), so this writes the shallow
                // snapshot a restart then loads.
                a.store.read().await.save_all().await.unwrap();
                drop(a);
                backend_at(dir_a.path(), bus_a.clone()).await
            } else {
                a
            };

            let a_doc = a.be.test_global_doc().await;
            let a_is_shallow = a_doc.with_read(|d| Ok(d.is_shallow())).unwrap();
            let expected_shallow = compact_owner && !compaction_disabled;
            assert_eq!(
                a_is_shallow, expected_shallow,
                "the owner's doc shallowness is the whole independent variable; this variant \
                 implies shallow={expected_shallow} and the reloaded doc reports \
                 {a_is_shallow}, so the case would measure nothing"
            );

            // A peer with its OWN pre-pairing history is the real own-device
            // shape (a phone boots its own layout and journals before it ever
            // pairs), and it is what the two-instance slice models. The
            // no-history case isolates whether that independent lineage is
            // what a compacted owner cannot merge with.
            let (b, _bus_b, _dir_b) = backend().await;
            if receiver_has_own_history {
                seed(&b, "root-b", None, "root-b").await;
            }

            let reg_a = holon_loro::ContainerRegistry::new(a.store.read().await.clone());
            let reg_b = holon_loro::ContainerRegistry::new(b.store.read().await.clone());
            let relay = holon_loro::sync_transport::InMemoryRelay::new();
            let clock = holon_api::TestClock::new(0);
            let chain = MembershipChain::direct(MembershipCert::issue(
                BlockId(holon_loro::container_registry::ROOT_CONTAINER_ID.to_string()),
                Principal("peer".to_string()),
                Issuer::Owner,
                Capabilities::read_write(),
                false,
                Lease::starting_at(holon_api::Clock::now_millis(&clock), 60 * 60 * 1000),
                &UnverifiedAuthority,
            ));
            let mut sess_a = holon_sharing::sync::SyncSession::new();
            let mut sess_b = holon_sharing::sync::SyncSession::new();

            let da = a.be.test_global_doc().await;
            let db = b.be.test_global_doc().await;

            // Pair: one round each way so the receiver holds the owner's tree
            // and every later op merges against a SHARED ancestor.
            if iroh_like {
                pair_like_iroh(&da, &db);
            } else {
                replicate_round(
                    &reg_a,
                    &reg_b,
                    &relay,
                    &mut sess_a,
                    &mut sess_b,
                    1,
                    &chain,
                    &clock,
                )
                .await;
                replicate_round(
                    &reg_b,
                    &reg_a,
                    &relay,
                    &mut sess_b,
                    &mut sess_a,
                    2,
                    &chain,
                    &clock,
                )
                .await;
            }
            assert!(
                db.with_read(|d| Ok(find(d, "c1").is_some())).unwrap(),
                "the receiver never got the owner's tree, so every concurrent op below would \
                 merge against nothing"
            );

            // Concurrent, unsynced, on BOTH peers: structure on one side and
            // text on the other is the exact D70 shape, plus a tree MOVE. The
            // two sides do not sync in between, so grouping each peer's ops
            // into one batch leaves the concurrency the case is about intact.
            da.with_write_origin("pair_probe", |doc_a| {
                raw_create_under(doc_a, "p1", "a-new");
                raw_move(doc_a, "c2", "c1");
                Ok(())
            })
            .unwrap();
            let b_create_parent = if receiver_has_own_history {
                "root-b"
            } else {
                "p1"
            };
            db.with_write_origin("pair_probe", |doc_b| {
                if receiver_edits_pretrim_text {
                    // Typing into content that existed BEFORE the owner
                    // compacted — the ordinary own-device gesture (edit an
                    // existing note on the phone).
                    raw_append_text(doc_b, "c1", " [B typed]");
                }
                raw_create_under(doc_b, b_create_parent, "b-new");
                Ok(())
            })
            .unwrap();

            // Sync to a fixed point.
            for _ in 0..4 {
                if iroh_like {
                    pair_like_iroh(&da, &db);
                    continue;
                }
                let (pa, ia, ra) = replicate_round(
                    &reg_a,
                    &reg_b,
                    &relay,
                    &mut sess_a,
                    &mut sess_b,
                    1,
                    &chain,
                    &clock,
                )
                .await;
                let (pb, ib, rb) = replicate_round(
                    &reg_b,
                    &reg_a,
                    &relay,
                    &mut sess_b,
                    &mut sess_a,
                    2,
                    &chain,
                    &clock,
                )
                .await;
                assert!(
                    ra.is_empty() && rb.is_empty(),
                    "acceptor refused: {ra:?} {rb:?}"
                );
                if pa == 0 && ia == 0 && pb == 0 && ib == 0 {
                    break;
                }
            }

            let shape_a = da.with_read(|d| Ok(tree_shape(d))).unwrap();
            let shape_b = db.with_read(|d| Ok(tree_shape(d))).unwrap();
            drop(da);
            drop(db);
            a.advertiser_for_test().close_all().await;
            b.advertiser_for_test().close_all().await;
            (shape_a, shape_b, a_is_shallow)
        }

        fn pair_rt() -> tokio::runtime::Runtime {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap()
        }

        fn assert_pair_converged(a: &[String], b: &[String], label: &str) {
            if a == b {
                return;
            }
            let sa: std::collections::BTreeSet<&String> = a.iter().collect();
            let sb: std::collections::BTreeSet<&String> = b.iter().collect();
            panic!(
                "{label}: the two peers did NOT converge.\nonly on the owner:\n{:#?}\nonly on \
                 the receiver:\n{:#?}",
                sa.difference(&sb).collect::<Vec<_>>(),
                sb.difference(&sa).collect::<Vec<_>>()
            );
        }

        /// **Inc 0b, variant NON-SHALLOW.** The control: the same replicate-all
        /// round at the engine layer with a full-history owner doc. Green here
        /// and red in the shallow twin isolates shallowness as the cause.
        #[test]
        fn replicate_all_converges_under_concurrent_structure_and_text() {
            let rt = pair_rt();
            let (a, b, was_shallow) = rt.block_on(run_replicate_all_pair(PairVariant {
                compact_owner: false,
                iroh_like: false,
                ..PairVariant::shallow()
            }));
            assert!(!was_shallow);
            assert!(
                a.iter().any(|n| n.contains("a-new"))
                    && a.iter().any(|n| n.contains("[B typed]"))
                    && a.iter().any(|n| n.contains("b-new")),
                "the owner is missing one of the concurrent writes, so equality proves nothing: \
                 {a:#?}"
            );
            assert_pair_converged(&a, &b, "non-shallow");
        }

        /// **Inc 0b, variant SHALLOW over the RELAY leg — a real defect,
        /// pinned.**
        ///
        /// The owner restarts over a compacted snapshot first, which is what
        /// EVERY session start does (`save_all` compacts on its first save).
        /// The pair then cannot even bootstrap: `pull_once` returns
        ///
        /// ```text
        /// importing an ADMITTED 226-byte blob (seq Some(2)) into container `holon_tree`
        ///   Caused by: Import Failed: The dependencies of the importing updates are not
        ///   included in the shallow history of the doc.
        /// ```
        ///
        /// This is NOT the D70 panic — the engine refuses loudly and the doc
        /// is not poisoned. The defect is in the relay leg: `push_once`
        /// exports `ExportMode::updates_owned(from)` with NO shallow guard
        /// (`crates/holon-sharing/src/sync.rs:167`), while the production iroh
        /// leg falls back to a self-contained snapshot for a peer below the
        /// shallow base (`iroh_sync_adapter.rs:77-105`). The twin below shows
        /// that with production's fallback the same merge converges, so the
        /// fix is to give `push_once` the same guard.
        ///
        /// `#[ignore]`d because the fix belongs to the sharing lane, not to
        /// this increment. Un-ignore it when `push_once` grows the guard.
        #[test]
        #[ignore = "OPEN: push_once has no shallow-base guard, so a compacted owner doc cannot bootstrap a peer over the relay leg"]
        fn replicate_all_over_the_relay_leg_fails_when_the_owner_doc_is_shallow() {
            let rt = pair_rt();
            let (a, b, was_shallow) = rt.block_on(run_replicate_all_pair(PairVariant {
                iroh_like: false,
                ..PairVariant::shallow()
            }));
            assert!(was_shallow);
            assert_pair_converged(&a, &b, "shallow / relay leg");
        }

        /// **Inc 0b, variant SHALLOW over the PRODUCTION exchange.** The owner
        /// is shallow exactly as above, but the exchange is the one production
        /// performs — `export_delta_or_full_snapshot`'s snapshot fallback for
        /// a peer below the shallow base.
        ///
        /// It does NOT converge, and not with the D70 panic either. The
        /// reverse leg fails:
        ///
        /// ```text
        /// [receiver->owner] importing 227 bytes FAILED: ImportUpdatesThatDependsOnOutdatedVersion
        ///   destination shallow=true  since=VersionVector({<owner peer>: 46})
        ///   source      shallow=false since=VersionVector({})
        /// ```
        ///
        /// Read the two `shallow=` lines: the receiver imported the owner's
        /// shallow SNAPSHOT and did NOT inherit the shallow base — its own
        /// `shallow_since` is empty, so it believes it holds history from 0.
        /// Its later ops causally depend on containers the owner trimmed, and
        /// the owner's shallow doc cannot accept an op that depends on
        /// history it no longer holds. The protocol has no way to express
        /// "my base is at 46" to the peer, so nothing the receiver authors on
        /// top of the snapshot can ever come back.
        ///
        /// `#[ignore]`d: OPEN, and it belongs to the sharing/loro-pin lanes.
        #[test]
        #[ignore = "OPEN: a compacted owner cannot import anything a peer authored on top of the snapshot it sent (receiver does not inherit the shallow base)"]
        fn replicate_all_converges_when_the_owner_doc_is_shallow() {
            let rt = pair_rt();
            let (a, b, was_shallow) = rt.block_on(run_replicate_all_pair(PairVariant::shallow()));
            assert!(was_shallow);
            assert!(
                a.iter().any(|n| n.contains("a-new"))
                    && a.iter().any(|n| n.contains("[B typed]"))
                    && a.iter().any(|n| n.contains("b-new")),
                "the owner is missing one of the concurrent writes, so equality proves nothing: \
                 {a:#?}"
            );
            assert_pair_converged(&a, &b, "shallow / production exchange");
        }

        /// **Inc 0b, the isolating case.** Shallow owner, production exchange,
        /// and a receiver that authored NOTHING before pairing. If this
        /// converges while the twin above does not, the trigger is not
        /// compaction alone — it is a compacted owner meeting a peer that
        /// already has an independent lineage, which is exactly what an
        /// own-device pair is.
        ///
        /// It does NOT converge either — same
        /// `ImportUpdatesThatDependsOnOutdatedVersion` on the reverse leg. So
        /// the receiver's independent lineage is NOT the trigger.
        ///
        /// `#[ignore]`d: OPEN, same defect as the twin above.
        #[test]
        #[ignore = "OPEN: same shallow-base defect; the receiver's own history is not the trigger"]
        fn shallow_owner_converges_with_a_receiver_that_has_no_own_history() {
            let rt = pair_rt();
            let (a, b, was_shallow) = rt.block_on(run_replicate_all_pair(PairVariant {
                receiver_has_own_history: false,
                ..PairVariant::shallow()
            }));
            assert!(was_shallow);
            assert!(
                a.iter().any(|n| n.contains("a-new"))
                    && a.iter().any(|n| n.contains("[B typed]"))
                    && a.iter().any(|n| n.contains("b-new")),
                "the owner is missing one of the concurrent writes: {a:#?}"
            );
            assert_pair_converged(&a, &b, "shallow / no receiver history");
        }

        /// **Inc 0b, the mechanism probe.** Shallow owner, production
        /// exchange, and a receiver that only CREATES — it never types into
        /// content that existed before the owner compacted. Green here would
        /// mean the constraint is specifically "the peer may not edit
        /// pre-compaction content", which is the ordinary own-device gesture
        /// and therefore the whole point of D68.b.
        ///
        /// It does NOT converge. A create under a pre-compaction parent
        /// depends on that parent's creation op, which was trimmed, so even a
        /// pure create cannot come back. The constraint is not "do not edit
        /// old text" — it is that essentially NOTHING the peer authors on top
        /// of a compacted snapshot is mergeable by the owner.
        ///
        /// `#[ignore]`d: OPEN, same defect.
        #[test]
        #[ignore = "OPEN: same shallow-base defect; even a pure create under a pre-compaction parent cannot merge back"]
        fn shallow_owner_and_a_receiver_that_only_creates() {
            let rt = pair_rt();
            let (a, b, was_shallow) = rt.block_on(run_replicate_all_pair(PairVariant {
                receiver_edits_pretrim_text: false,
                ..PairVariant::shallow()
            }));
            assert!(was_shallow);
            assert!(
                a.iter().any(|n| n.contains("a-new")) && a.iter().any(|n| n.contains("b-new")),
                "the owner is missing one of the concurrent creates: {a:#?}"
            );
            assert_pair_converged(&a, &b, "shallow / receiver creates only");
        }

        /// **Inc 0b, the crude stopgap.** `HOLON_LORO_COMPACT=off` on the owner
        /// before it saves, so the restart reloads a FULL-history doc. If this
        /// converges, turning compaction off for a paired vault buys working
        /// pairing at the cost of an unbounded oplog.
        #[test]
        fn compaction_disabled_on_the_owner_lets_the_pair_converge() {
            let rt = pair_rt();
            let (a, b, was_shallow) = rt.block_on(run_replicate_all_pair(PairVariant {
                compaction_disabled: true,
                ..PairVariant::shallow()
            }));
            assert!(
                !was_shallow,
                "HOLON_LORO_COMPACT=off did not prevent the compacting save"
            );
            assert!(
                a.iter().any(|n| n.contains("a-new"))
                    && a.iter().any(|n| n.contains("[B typed]"))
                    && a.iter().any(|n| n.contains("b-new")),
                "the owner is missing one of the concurrent writes: {a:#?}"
            );
            assert_pair_converged(&a, &b, "compaction disabled");
        }

        /// **Inc 0b, P2 — the pairing-replaces-the-phone's-doc shape.**
        ///
        /// The earlier shallow variants had the receiver import the owner's
        /// snapshot into a doc that ALREADY existed (its schema init, and in
        /// most variants a seed too). The diagnostic showed the consequence:
        /// the receiver came out `shallow=false since={}`, i.e. it did not
        /// inherit the owner's base.
        ///
        /// Here the receiver's document IS the import: a bare `LoroDoc` with
        /// no prior history, exactly what "pairing replaces the fresh phone's
        /// document" would do. Returns whether the receiver ended up shallow
        /// and what base it carries, so the answer is measured, not inferred.
        async fn run_empty_receiver_bootstrap() -> (Vec<String>, Vec<String>, bool, String) {
            let dir_a = TempDir::new().unwrap();
            let bus_a = Arc::new(DegradedSignalBus::new());
            let a = backend_fresh(dir_a.path(), bus_a.clone()).await;
            seed(&a, "root-a", None, "root-a").await;
            seed(&a, "p1", Some("root-a"), "Parent one").await;
            seed(&a, "c1", Some("p1"), "Child one").await;
            seed(&a, "c2", Some("p1"), "Child two").await;
            a.store.read().await.save_all().await.unwrap();
            drop(a);
            let a = backend_at(dir_a.path(), bus_a.clone()).await;

            let da = a.be.test_global_doc().await;

            // The owner imports on every round, so the whole probe runs under
            // its write guard. The receiver is a bare `LoroDoc` this function
            // owns outright — no doc boundary to cross.
            let (shape_a, shape_b, b_is_shallow, b_since) = da
                .with_write_origin("empty_receiver_bootstrap", |doc_a| {
                    assert!(
                        doc_a.is_shallow(),
                        "the owner must be shallow for this probe"
                    );

                    // The receiver's document is CREATED by the pairing payload.
                    let doc_b = LoroDoc::new();
                    doc_b.set_peer_id(2).unwrap();
                    let bootstrap = export_like_iroh(doc_a, &doc_b.oplog_vv());
                    doc_b.import(&bootstrap).unwrap();
                    let b_is_shallow = doc_b.is_shallow();
                    let b_since = format!("{:?}", doc_b.shallow_since_vv().to_vv());

                    assert!(
                        find(&doc_b, "c1").is_some(),
                        "the bootstrap payload did not carry the owner's tree"
                    );

                    // The same concurrent structure + text as every other variant.
                    raw_create_under(doc_a, "p1", "a-new");
                    raw_append_text(&doc_b, "c1", " [B typed]");
                    raw_move(doc_a, "c2", "c1");
                    raw_create_under(&doc_b, "p1", "b-new");

                    for _ in 0..4 {
                        sync_pair_like_iroh(doc_a, &doc_b);
                    }

                    Ok((tree_shape(doc_a), tree_shape(&doc_b), b_is_shallow, b_since))
                })
                .unwrap();
            drop(da);
            a.advertiser_for_test().close_all().await;
            (shape_a, shape_b, b_is_shallow, b_since)
        }

        #[test]
        fn shallow_owner_converges_with_a_receiver_bootstrapped_into_an_empty_doc() {
            let rt = pair_rt();
            let (a, b, b_is_shallow, b_since) = rt.block_on(run_empty_receiver_bootstrap());
            eprintln!("[P2] receiver after bootstrap: shallow={b_is_shallow} since={b_since}");
            assert!(
                a.iter().any(|n| n.contains("a-new"))
                    && a.iter().any(|n| n.contains("[B typed]"))
                    && a.iter().any(|n| n.contains("b-new")),
                "the owner is missing one of the concurrent writes: {a:#?}"
            );
            assert_pair_converged(&a, &b, "empty-doc receiver bootstrap");
        }

        /// The accepter side of the same routing question: a block created
        /// under the mount B got from `accept_shared_subtree` must land in the
        /// shared doc and reach the sharer.
        #[test]
        fn structural_edits_on_the_accepter_reach_the_sharer() {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(run_case(vec![
                Action::CreateUnderMountOnB("mB:one".to_string()),
                Action::PullBtoA,
                Action::CreateUnderSharedRootOnB("rB:two".to_string()),
                Action::PullBtoA,
                Action::DeleteChildOnB,
                Action::PullBtoA,
            ]));
        }

        /// Shrunk counterexample from `subtree_share_round_trip_pbt`: A deletes
        /// a child and nothing in the case asks for a pull, yet B converges on
        /// its own once A's restart re-registers its share with the live
        /// advertiser. The first P-STRUCT model asserted B still had the block;
        /// a live-syncing pair cannot hold that.
        #[test]
        fn a_delete_reaches_the_peer_without_an_explicit_pull() {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(run_case(vec![
                Action::DeleteChildOnA,
                Action::SettleSaves,
                Action::RestartA,
                Action::SettleSaves,
            ]));
        }

        proptest! {
            #![proptest_config(ProptestConfig {
                cases: 24,
                timeout: 120000,
                failure_persistence: None,
                .. ProptestConfig::default()
            })]

            #[test]
            fn subtree_share_round_trip_pbt(actions in actions_strategy()) {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(run_case(actions));
            }
        }
    }
}
