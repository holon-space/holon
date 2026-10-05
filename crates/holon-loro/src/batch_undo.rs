//! Rolling back a failed multi-op batch by undoing only its own writes.
//!
//! Each routable document gets an `UndoManager` that records nothing but the
//! commits carrying the batch's origin ([`WriteOrigin::Batch`]). Undoing it
//! transforms the batch's ops over everything else that landed in the window —
//! a keystroke, another task's block op, a peer's import — so those survive.
//! What cannot survive is a write that depends on something the batch made: a
//! keystroke into a block the batch created goes with that block. The rollback
//! reports each such block as a [`LostWrite`].

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_core::BatchId;
use holon_core::LostWrite;
use holon_core::OpenBatch;
use holon_core::RollbackRefused;
use holon_core::RolledBack;
use loro::TreeID;
use tokio::sync::RwLock;

use crate::LoroDocument;
use crate::LoroDocumentStore;
use crate::loro_backend::TREE_NAME;
use crate::loro_document_store::DocScope;
use crate::shared_tree::SharedTreeStore;
use crate::write_origin::WriteOrigin;

const VAULT_DOC: &str = "vault";
const LAYOUT_DOC: &str = "layout";

/// Every document a block write can reach.
#[derive(Clone)]
pub(crate) struct Routing {
    pub(crate) doc_store: Arc<RwLock<LoroDocumentStore>>,
    pub(crate) shared_trees: Option<Arc<dyn SharedTreeStore>>,
}

impl Routing {
    /// The same set [`crate::LoroBackend::resolve_write_target_sync`] routes
    /// over: the device-local layout document, the vault document, and each
    /// shared subtree document.
    async fn docs(&self) -> anyhow::Result<Vec<(String, Arc<LoroDocument>)>> {
        let store = self.doc_store.read().await;
        let mut docs = Vec::new();
        for (name, scope) in [
            (VAULT_DOC, DocScope::Global),
            (LAYOUT_DOC, DocScope::Layout),
        ] {
            docs.push((
                name.to_string(),
                store
                    .get_doc(scope)
                    .await
                    .map_err(|e| anyhow::anyhow!("{name} document: {e}"))?,
            ));
        }
        if let Some(shared) = &self.shared_trees {
            for id in shared.shared_tree_ids() {
                let doc = shared.get_shared_doc(&id).ok_or_else(|| {
                    anyhow::anyhow!("shared tree {id} left the registry between listing and lookup")
                })?;
                // Re-wrapping the same `Arc<LoroDoc>` resolves to the same
                // doc-boundary lock as every other writer's wrapper.
                docs.push((
                    format!("shared:{id}"),
                    Arc::new(LoroDocument::from_existing(doc, id)),
                ));
            }
        }
        Ok(docs)
    }
}

/// What one document's subscription heard inside the window.
#[derive(Default)]
struct Heard {
    batch_commits: usize,
    checkouts: usize,
    /// Tree nodes a write that was not the batch's created, moved, or wrote
    /// below.
    touched_by_others: BTreeSet<TreeID>,
}

impl Heard {
    fn hear(&mut self, event: &loro::event::DiffEvent, batch_origin: &str) {
        if matches!(event.triggered_by, loro::EventTriggerKind::Checkout) {
            self.checkouts += 1;
            return;
        }
        if event.origin == batch_origin {
            self.batch_commits += 1;
            return;
        }
        for diff in &event.events {
            for (_, index) in diff.path {
                if let loro::Index::Node(node) = index {
                    self.touched_by_others.insert(*node);
                }
            }
            if let loro::event::Diff::Tree(tree) = &diff.diff {
                self.touched_by_others
                    .extend(tree.diff.iter().map(|item| item.target));
            }
        }
    }
}

/// One document's recording of the batch.
struct DocRecording {
    name: String,
    doc: Arc<LoroDocument>,
    peer: loro::PeerID,
    manager: loro::UndoManager,
    heard: Arc<Mutex<Heard>>,
    _subscription: loro::Subscription,
}

impl DocRecording {
    fn arm(name: String, doc: Arc<LoroDocument>, id: BatchId) -> anyhow::Result<Self> {
        let origin = WriteOrigin::Batch(id).as_origin().into_owned();
        let heard = Arc::new(Mutex::new(Heard::default()));
        let sink = heard.clone();
        // Under the write scope: Loro panics if a subscriber registers while
        // the doc is emitting.
        let (manager, subscription, peer) = doc.with_write(WriteOrigin::UndoArm, |txn| {
            let mut manager = loro::UndoManager::new(txn.doc());
            // A dropped oldest step would leave part of the batch in place.
            manager.set_max_undo_steps(usize::MAX);
            manager.add_include_origin_prefix(&origin);
            manager.group_start()?;
            let subscription = txn.subscribe_root(Arc::new(move |event| {
                sink.lock()
                    .expect("the batch's heard-set lock is never held across a panic")
                    .hear(&event, &origin)
            }));
            Ok((manager, subscription, txn.peer_id()))
        })?;
        Ok(Self {
            name,
            doc,
            peer,
            manager,
            heard,
            _subscription: subscription,
        })
    }

    /// Why this document's history cannot take the batch back, if it cannot.
    fn unrecorded(&mut self) -> Option<RollbackRefused> {
        self.manager.group_end();
        let heard = self
            .heard
            .lock()
            .expect("the batch's heard-set lock is never held across a panic");
        let detail = if self.doc.peer_id() != self.peer {
            format!(
                "the document's peer id changed from {} to {} inside the window, which clears \
                 the undo history",
                self.peer,
                self.doc.peer_id()
            )
        } else if heard.checkouts > 0 {
            "a checkout inside the window cleared the undo history".to_string()
        } else if heard.batch_commits > 0 && self.manager.undo_count() == 0 {
            format!(
                "{} commit(s) carried the batch's origin and none is on the undo stack",
                heard.batch_commits
            )
        } else {
            return None;
        };
        Some(RollbackRefused::Unrecorded {
            doc: self.name.clone(),
            detail,
        })
    }

    /// Undo every recorded step. Returns the step count and the blocks
    /// another writer touched that the undo took with it; on failure, the
    /// steps already undone.
    fn undo(&mut self) -> Result<(usize, Vec<LostWrite>), (usize, anyhow::Error)> {
        let touched: Vec<TreeID> = self
            .heard
            .lock()
            .expect("the batch's heard-set lock is never held across a panic")
            .touched_by_others
            .iter()
            .copied()
            .collect();
        let manager = &mut self.manager;
        let name = &self.name;
        let mut steps = 0;
        let result = self.doc.with_write(WriteOrigin::BatchRollback, |txn| {
            let tree = txn.get_tree(TREE_NAME);
            let live = |node: TreeID| -> anyhow::Result<bool> {
                Ok(tree.contains(node) && !tree.is_node_deleted(&node)?)
            };
            let mut live_before = Vec::new();
            for node in touched {
                if live(node)? {
                    let stable = crate::settled_read::read_stable_id(&tree.get_meta(node)?)
                        .ok_or_else(|| {
                            anyhow::anyhow!("live node {node:?} carries no stable id")
                        })?;
                    live_before.push((node, stable));
                }
            }
            while manager.can_undo() {
                // Every undo step is its own commit, and an unlabelled one
                // would land on the user's text-undo stack.
                txn.arm_origin();
                manager.undo()?;
                steps += 1;
            }
            let mut lost = Vec::new();
            for (node, stable) in live_before {
                if !live(node)? {
                    lost.push(LostWrite {
                        doc: name.clone(),
                        block: EntityUri::block(&stable).to_string(),
                    });
                }
            }
            Ok(lost)
        });
        result.map(|lost| (steps, lost)).map_err(|e| (steps, e))
    }
}

/// A batch being recorded in every routable document.
pub(crate) struct LoroOpenBatch {
    id: BatchId,
    routing: Routing,
    docs: Vec<DocRecording>,
}

impl LoroOpenBatch {
    pub(crate) async fn open(routing: Routing) -> anyhow::Result<Self> {
        let id = BatchId::fresh();
        let mut docs = Vec::new();
        for (name, doc) in routing.docs().await? {
            docs.push(DocRecording::arm(name, doc, id)?);
        }
        Ok(Self { id, routing, docs })
    }
}

#[async_trait]
impl OpenBatch for LoroOpenBatch {
    fn id(&self) -> BatchId {
        self.id
    }

    async fn roll_back(mut self: Box<Self>) -> Result<RolledBack, RollbackRefused> {
        // Every refusal is decided here, before the first undo, so a refused
        // rollback leaves the store exactly as the failed batch left it.
        let now: BTreeSet<String> = self
            .routing
            .docs()
            .await
            .map_err(|e| RollbackRefused::Unreachable {
                doc: "the write authority".to_string(),
                detail: e.to_string(),
            })?
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let recorded: BTreeSet<String> = self.docs.iter().map(|d| d.name.clone()).collect();
        if let Some(doc) = recorded.symmetric_difference(&now).next() {
            return Err(RollbackRefused::Unreachable {
                doc: doc.clone(),
                detail: "the document joined or left the write authority's routing set inside \
                         the batch window"
                    .to_string(),
            });
        }
        for doc in &mut self.docs {
            if let Some(refusal) = doc.unrecorded() {
                return Err(refusal);
            }
        }

        let mut rolled = RolledBack {
            undone_steps: 0,
            lost: Vec::new(),
        };
        let mut reverted: Vec<String> = Vec::new();
        for doc in &mut self.docs {
            match doc.undo() {
                Ok((steps, lost)) => {
                    if steps > 0 {
                        reverted.push(doc.name.clone());
                    }
                    rolled.undone_steps += steps;
                    rolled.lost.extend(lost);
                }
                Err((0, e)) if reverted.is_empty() => {
                    return Err(RollbackRefused::Unreachable {
                        doc: doc.name.clone(),
                        detail: format!("{e:#}"),
                    });
                }
                Err((steps, e)) => {
                    return Err(RollbackRefused::Incomplete {
                        reverted,
                        doc: doc.name.clone(),
                        detail: format!("after {steps} undo step(s) in it: {e:#}"),
                    });
                }
            }
        }
        Ok(rolled)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use holon_api::BlockContent;
    use holon_api::BlockEdges;
    use holon_api::repository::CoreOperations as _;

    use super::*;
    use crate::loro_backend::LoroBackend;

    async fn create(backend: &LoroBackend, parent: EntityUri, id: &str) {
        backend
            .create_block_with_properties(
                parent,
                BlockContent::text(id),
                Some(EntityUri::block(id)),
                &HashMap::new(),
                &BlockEdges::default(),
            )
            .await
            .unwrap();
    }

    /// The stable-id index hears the undo's commits through its tree watch
    /// like any local write, and after the undo equals an index built cold.
    #[tokio::test]
    async fn the_stable_id_index_after_a_rollback_equals_a_cold_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let store = LoroDocumentStore::new(dir.path().to_path_buf());
        let doc = store.get_doc(DocScope::Global).await.unwrap();
        let backend = LoroBackend::from_document(doc.clone());
        create(&backend, EntityUri::no_parent(), "root").await;
        create(&backend, EntityUri::block("root"), "p1").await;
        create(&backend, EntityUri::block("root"), "p2").await;
        create(&backend, EntityUri::block("p1"), "m").await;
        create(&backend, EntityUri::block("p1"), "d").await;
        let warm = doc
            .with_read(|d| Ok(crate::stable_id_index::snapshot(d)))
            .unwrap();

        let batch = Box::new(
            LoroOpenBatch::open(Routing {
                doc_store: Arc::new(RwLock::new(store.clone())),
                shared_trees: None,
            })
            .await
            .unwrap(),
        );
        batch
            .id()
            .scope(async {
                create(&backend, EntityUri::block("root"), "n1").await;
                create(&backend, EntityUri::block("n1"), "n2").await;
                backend
                    .move_block(&EntityUri::block("m"), EntityUri::block("p2"), None)
                    .await
                    .unwrap();
                backend.delete_block("block:d").await.unwrap();
            })
            .await;
        create(&backend, EntityUri::block("p2"), "c").await;
        batch.roll_back().await.unwrap();

        let incremental = doc
            .with_read(|d| Ok(crate::stable_id_index::snapshot(d)))
            .unwrap();
        let cold_doc = LoroDocument::new("cold".to_string()).unwrap();
        cold_doc
            .apply_update(&doc.export_snapshot().unwrap())
            .unwrap();
        let cold = cold_doc
            .with_read(|d| Ok(crate::stable_id_index::snapshot(d)))
            .unwrap();
        assert_eq!(incremental, cold);
        // Loro undoes a delete by creating a new node.
        let revived = backend.find_tree_id_by_stable_id_sync("d");
        assert!(revived.is_some(), "the undone delete must be live again");
        assert_ne!(revived, warm.get("d").and_then(|v| v.first().copied()));
        assert!(backend.find_tree_id_by_stable_id_sync("n1").is_none());
        assert!(backend.find_tree_id_by_stable_id_sync("c").is_some());
    }
}
