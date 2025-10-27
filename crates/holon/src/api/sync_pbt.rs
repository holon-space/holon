//! Multi-peer sync property-based tests with ephemeral Loro oracle.
//!
//! Two modes:
//! - **Direct** (`test_multi_peer_sync_direct`): Uses Loro export/import directly. Fast, deterministic.
//! - **Iroh** (`test_multi_peer_sync_iroh`): Uses real Iroh QUIC transport. Catches protocol bugs.
//!
//! Both share the same transitions, invariants, and oracle from `sync::multi_peer`.

#[cfg(test)]
mod tests {
    use crate::sync::multi_peer::*;
    use proptest::prelude::*;
    use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};
    use std::sync::Arc;

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
            _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        ) -> Self::SystemUnderTest {
        }

        fn apply(
            state: Self::SystemUnderTest,
            _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
            _transition: <Self::Reference as ReferenceStateMachine>::Transition,
        ) -> Self::SystemUnderTest {
            state
        }

        fn check_invariants(
            _state: &Self::SystemUnderTest,
            ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        ) {
            check_invariants(ref_state);
        }
    }

    // -- Iroh sync PBT (real QUIC transport) --

    #[cfg(feature = "iroh-sync")]
    struct IrohSyncBackend(crate::sync::iroh_sync_adapter::IrohSync);

    #[cfg(feature = "iroh-sync")]
    impl SyncBackend for IrohSyncBackend {
        fn sync_pair(&self, doc_a: &loro::LoroDoc, doc_b: &loro::LoroDoc) -> anyhow::Result<()> {
            crate::sync::iroh_sync_adapter::SyncBackend::sync_pair(&self.0, doc_a, doc_b)
        }
    }

    #[cfg(feature = "iroh-sync")]
    struct IrohGroupState;

    #[cfg(feature = "iroh-sync")]
    impl ReferenceStateMachine for IrohGroupState {
        type State = GroupState<()>;
        type Transition = GroupTransition;

        fn init_state() -> BoxedStrategy<Self::State> {
            let backend = crate::sync::iroh_sync_adapter::IrohSync::new()
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
            _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        ) -> Self::SystemUnderTest {
        }

        fn apply(
            state: Self::SystemUnderTest,
            _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
            _transition: <Self::Reference as ReferenceStateMachine>::Transition,
        ) -> Self::SystemUnderTest {
            state
        }

        fn check_invariants(
            _state: &Self::SystemUnderTest,
            ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        ) {
            check_invariants(ref_state);
        }
    }

    // -- Test entry points --

    proptest_state_machine::prop_state_machine! {
        #![proptest_config(ProptestConfig {
            cases: 30,
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::WithSource("pbt-regressions")
            )),
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
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::WithSource("pbt-regressions")
            )),
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
        use crate::sync::degraded_signal_bus::{
            DegradedSignalBus, ShareDegraded, ShareDegradedReason,
        };
        use crate::sync::device_key_store::load_or_create_device_key;
        use crate::sync::iroh_advertiser::IrohAdvertiser;
        use crate::sync::iroh_sync_adapter::SharedTreeSyncManager;
        use crate::sync::loro_document_store::LoroDocumentStore;
        use crate::sync::loro_share_backend::{
            LoroShareBackend, SubtreeShareOperations, rehydrate_shared_trees,
        };
        use crate::sync::multi_peer::{TREE_NAME, get_alive_nodes};
        use crate::sync::shared_snapshot_store::SharedSnapshotStore;
        use holon_api::{InlineMark, Value};
        use loro::{LoroDoc, LoroText, TreeID, TreeParentId};
        use proptest::prelude::*;
        use serde_json::Value as JsonValue;
        use std::path::Path;
        use std::sync::Arc;
        use tempfile::TempDir;
        use tokio::sync::{RwLock, broadcast};

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
                    share_registered: true,
                    share_usable: true,
                    corrupt_pending: false,
                }
            }
        }

        /// Everything we need to reconstruct a backend on the same dir
        /// — used by `RestartA` / `RestartB` to simulate process restart
        /// while preserving on-disk state.
        async fn backend_fresh(
            dir_path: &Path,
            bus: Arc<DegradedSignalBus>,
        ) -> Arc<LoroShareBackend> {
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
            LoroShareBackend::new(store, snapshot_store, manager, advertiser, bus, key)
        }

        async fn backend_at(dir_path: &Path, bus: Arc<DegradedSignalBus>) -> Arc<LoroShareBackend> {
            let be = backend_fresh(dir_path, bus).await;
            let collab = be.test_global_doc().await;
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let _ = rehydrate_shared_trees(&be, &doc).await.unwrap();
            be
        }

        /// Initial backend — creates a fresh `TempDir`. Skips
        /// rehydration (nothing to rehydrate on a fresh dir).
        async fn backend() -> (Arc<LoroShareBackend>, Arc<DegradedSignalBus>, TempDir) {
            let dir = TempDir::new().unwrap();
            let bus = Arc::new(DegradedSignalBus::new());
            let be = backend_fresh(dir.path(), bus.clone()).await;
            (be, bus, dir)
        }

        async fn seed(be: &LoroShareBackend, stable_id: &str, parent: Option<&str>, content: &str) {
            let collab = be.test_global_doc().await;
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let tree = doc.get_tree(TREE_NAME);
            let parent_tid = parent.map(|pid| find(&doc, pid).unwrap());
            let node = tree.create(parent_tid).unwrap();
            let meta = tree.get_meta(node).unwrap();
            meta.insert("id", loro::LoroValue::from(stable_id)).unwrap();
            let text: LoroText = meta
                .insert_container("content_raw", LoroText::new())
                .unwrap();
            text.insert(0, content).unwrap();
            doc.commit();
        }

        fn find(doc: &LoroDoc, stable_id: &str) -> Option<TreeID> {
            let tree = doc.get_tree(TREE_NAME);
            for node in tree.get_nodes(false) {
                if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
                    continue;
                }
                if let Ok(meta) = tree.get_meta(node.id) {
                    if let Some(loro::ValueOrContainer::Value(v)) = meta.get("id") {
                        if v.as_string().map(|s| s.as_str()) == Some(stable_id) {
                            return Some(node.id);
                        }
                    }
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
        /// Uses `crate::api::loro_backend::mark_to_loro_value` so the
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
            let value = crate::api::loro_backend::mark_to_loro_value(&mark);
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
            let observed = crate::api::loro_backend::read_marks_from_text(&t);
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
                if let Ok(meta) = tree.get_meta(n.id) {
                    if let Some(loro::ValueOrContainer::Value(v)) = meta.get("shared_tree_id") {
                        if v.as_string().map(|s| s.as_str()) == Some(shared_tree_id) {
                            return true;
                        }
                    }
                }
            }
            false
        }

        /// Drain any queued `ShareDegraded` events from a receiver
        /// without blocking. Used between actions to observe which
        /// degraded signals fired.
        fn drain_bus(rx: &mut broadcast::Receiver<ShareDegraded>) -> Vec<ShareDegraded> {
            let mut out = Vec::new();
            loop {
                match rx.try_recv() {
                    Ok(ev) => out.push(ev),
                    Err(_) => break,
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
                if name.ends_with(".loro") && !name.contains(".corrupt-") {
                    if entry.metadata().unwrap().len() == 0 {
                        zero_byte.push(path);
                    }
                }
            }
            (zero_byte, tmps)
        }

        /// Check post-action invariants that apply every step.
        async fn check_invariants(
            a: &LoroShareBackend,
            b: &LoroShareBackend,
            dir_a: &Path,
            dir_b: &Path,
            shared_tree_id: &str,
            ref_a: &RefPeer,
            ref_b: &RefPeer,
            initial_peer_id_a: u64,
            initial_peer_id_b: u64,
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

            // P-KEY on each peer: the shared doc's peer_id must not
            // drift across restarts on that peer (derived from the
            // persistent device.key).
            if ref_a.share_usable {
                let d = a.manager_for_test().get_doc(shared_tree_id).unwrap();
                assert_eq!(
                    d.peer_id(),
                    initial_peer_id_a,
                    "P-KEY/A: shared doc peer_id drifted after restart"
                );
            }
            if ref_b.share_usable {
                let d = b.manager_for_test().get_doc(shared_tree_id).unwrap();
                assert_eq!(
                    d.peer_id(),
                    initial_peer_id_b,
                    "P-KEY/B: shared doc peer_id drifted after restart"
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

            // P-MARKS: every mark recorded in `expected_marks` must be
            // observable on the shared root text — by suffix range +
            // Loro key. Surfaces:
            //   - shared docs created without `configure_text_styles`
            //     (mark API still applies but ExpandType policy is wrong)
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
                        "P-MARKS/A: missing marks {missing:?} in shared doc {shared_tree_id}\n  text:     {text:?}\n  expected: {:?}\n  observed: {observed:?}",
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
                        "P-MARKS/B: missing marks {missing:?} in shared doc {shared_tree_id}\n  text:     {text:?}\n  expected: {:?}\n  observed: {observed:?}",
                        ref_b.expected_marks
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
            crate::api::loro_backend::read_marks_from_text(&t)
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
            let mut rx_a = bus_a.subscribe();
            let _rx_b = bus_b.subscribe();

            seed(&a, "root-a", None, "root-a").await;
            seed(&a, "shared-parent", Some("root-a"), "Shared heading").await;
            seed(&a, "child-1", Some("shared-parent"), "Child 1").await;
            seed(&a, "child-2", Some("shared-parent"), "Child 2").await;
            seed(&b, "root-b", None, "root-b").await;

            let share_resp = a
                .share_subtree("block:shared-parent", "full".into())
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
            // *device* key, and A and B are distinct devices — but
            // each side's own peer_id must stay stable across its own
            // restarts (P-KEY).
            let initial_peer_id_a = a
                .manager_for_test()
                .get_doc(&shared_tree_id)
                .unwrap()
                .peer_id();
            let initial_peer_id_b = b
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

            for action in actions {
                match action {
                    Action::EditOnA(s) => {
                        if ref_a.share_usable {
                            let d = a.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            append_text_on_root(&d, &s);
                            ref_a.alive_suffixes.push(s);
                        }
                    }
                    Action::EditOnB(s) => {
                        if ref_b.share_usable {
                            let d = b.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            append_text_on_root(&d, &s);
                            ref_b.alive_suffixes.push(s);
                        }
                    }
                    Action::SettleSaves => {
                        // Wait longer than the save debounce so all
                        // pending worker writes finish.
                        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        // P-NO-TMP-LEFTOVER: no `.tmp` should remain.
                        for (label, dir) in [("A", dir_a.path()), ("B", dir_b.path())] {
                            let (_zero, tmps) = scan_for_corruption(dir);
                            assert!(
                                tmps.is_empty(),
                                "P-NO-TMP-LEFTOVER/{label}: stale tmp files: {tmps:?}"
                            );
                        }
                    }
                    Action::PullBtoA => {
                        if ref_a.share_usable && ref_b.share_usable {
                            let a_shared = a.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            let b_shared = b.manager_for_test().get_doc(&shared_tree_id).unwrap();
                            crate::sync::multi_peer::SyncBackend::sync_pair(
                                &crate::sync::multi_peer::DirectSync,
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
                        }
                    }
                    Action::RestartB => {
                        b.advertiser_for_test().close_all().await;
                        b.flush_all().await;
                        let old_bus = bus_b.clone();
                        drop(b);
                        b = backend_at(dir_b.path(), old_bus).await;
                    }
                    Action::MarkOnA(kind) => {
                        if ref_a.share_usable {
                            if let Some(suffix) = ref_a.alive_suffixes.last().cloned() {
                                let d = a.manager_for_test().get_doc(&shared_tree_id).unwrap();
                                if mark_suffix_on_root(&d, &suffix, kind) {
                                    ref_a.expected_marks.push(ExpectedMark {
                                        suffix,
                                        key: kind.loro_key(),
                                    });
                                }
                            }
                        }
                    }
                    Action::MarkOnB(kind) => {
                        if ref_b.share_usable {
                            if let Some(suffix) = ref_b.alive_suffixes.last().cloned() {
                                let d = b.manager_for_test().get_doc(&shared_tree_id).unwrap();
                                if mark_suffix_on_root(&d, &suffix, kind) {
                                    ref_b.expected_marks.push(ExpectedMark {
                                        suffix,
                                        key: kind.loro_key(),
                                    });
                                }
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
                        if !ref_a.share_usable || !ref_b.share_usable || ref_a.corrupt_pending {
                            continue;
                        }

                        // Restart A. Flush saves first so the sidecar
                        // (known_peers) is on disk.
                        a.advertiser_for_test().close_all().await;
                        a.flush_all().await;
                        let old_bus = bus_a.clone();
                        drop(a);
                        a = backend_at(dir_a.path(), old_bus).await;

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
                                    "CrossPeerSyncAfterRestart: A did not pick up B's edit {s:?} within 5s via auto-resync"
                                );
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
                    initial_peer_id_a,
                    initial_peer_id_b,
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
                "P-DEGRADED-ON-CORRUPT: expected ≥{expected_load_failures_on_a} SnapshotLoadFailed events, saw {load_failures} (events: {evs:?})"
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
            let step = prop_oneof![
                6 => edit_a,
                6 => edit_b,
                3 => Just(Action::SettleSaves),
                3 => Just(Action::PullBtoA),
                2 => Just(Action::RestartA),
                2 => Just(Action::RestartB),
                3 => mark_a,
                3 => mark_b,
                1 => Just(Action::CorruptSharedOnA),
                1 => cross_peer,
            ];
            prop::collection::vec(step, 0..8)
        }

        proptest! {
            #![proptest_config(ProptestConfig {
                cases: 24,
                timeout: 120000,
                failure_persistence: Some(Box::new(
                    proptest::test_runner::FileFailurePersistence::WithSource("pbt-regressions")
                )),
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
