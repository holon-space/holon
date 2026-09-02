//! C1 container registry — the everything-policy replication fast path (ADR
//! 0028 Increment 3).
//!
//! ## What this types away
//!
//! Device sync is already replicate-all today. This module *names and types*
//! that enumeration as the **replication set** and closes the door on a
//! per-doc filter ever being threaded through it. The registry wraps (does not
//! replace) [`LoroDocumentStore`]: the store stays the persistence substrate,
//! the registry is the authoritative answer to "what replicates".
//!
//! ## Why overshare is unrepresentable here (ADR C1)
//!
//! [`ContainerRegistry::replicate_all`] — the biggest gun in the system — takes
//! **no filter parameter**. It iterates the replication set and advertises each
//! container. There is no self-device code path on which a filter could be
//! mis-set, because there is no filter at all. The only gate is *enrollment*
//! (the SELF/THIRD-PARTY classification the fast path trusts, a hard
//! precondition — the A5/H5 ceremony landed): `replicate_all` requires a
//! [`SharedRoster`] and advertises every container **gated** by it, so a peer
//! the owner has not signed into the roster is rejected at
//! `acceptor_enroll` and never reaches a container doc. A third party gets the
//! per-share extract-prune-mount path instead; it never reaches this fast path.
//!
//! ## NonReplicated exclusion (OQ3)
//!
//! The alias ledger (holon-sharing) is owner-private and marked
//! `NonReplicated<T>`. That marker deliberately exposes no API returning a
//! replication handle — so a `NonReplicated` doc **cannot be handed to**
//! [`ContainerRegistry::register_container`], which accepts only a plain
//! replicable [`LoroDocument`]. The exclusion is therefore structural: an
//! owner-private doc is never enumerated because it can never enter the set.
//!
//! ## Blind-relay guardrail ([SR])
//!
//! The replication surface treats each container's payload as an opaque doc
//! handed to the transport — no API here assumes the replicating peer can read
//! doc internals. A future encrypted store-and-forward relay is exactly such a
//! peer.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use anyhow::anyhow;
use tokio::sync::RwLock;

use crate::LoroDocument;
use crate::loro_backend::snapshot_blocks_from_doc;
use crate::loro_document_store::DocScope;
use crate::loro_document_store::LoroDocumentStore;

/// Stable share id of the root container — the whole vault's global LoroTree
/// (single-global-doc model). The everything-policy self-device share is
/// advertised under this id.
pub const ROOT_CONTAINER_ID: &str = "holon_tree";

/// One container in the replication set = one shared subtree / `LoroDoc` (ADR
/// 0028 §5). A crossing moves a block between containers; a `TreeID` move never
/// spans containers, so a container is the unit of replication.
#[derive(Clone)]
pub struct RegisteredContainer {
    /// Stable share id this container is advertised under.
    pub id: String,
    /// The container's replicable document. Opaque to the transport ([SR]
    /// blind-relay: no reader assumes it can decode this).
    pub doc: Arc<LoroDocument>,
}

/// The authoritative enumeration of what replicates (C1). Wraps a
/// [`LoroDocumentStore`] — the store's global doc is the root container; extra
/// shared-subtree containers register explicitly.
#[derive(Clone)]
pub struct ContainerRegistry {
    store: LoroDocumentStore,
    /// Shared-subtree containers registered on top of the root. Only plain
    /// (replicable) docs land here — a `NonReplicated<T>` has no handle to
    /// pass.
    extra: Arc<RwLock<Vec<RegisteredContainer>>>,
}

impl ContainerRegistry {
    /// Wrap a document store as a container registry. The store's global doc
    /// becomes the root container; no extra containers are registered yet.
    pub fn new(store: LoroDocumentStore) -> Self {
        Self {
            store,
            extra: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// The wrapped store (registry wraps, does not replace — consumers keep
    /// their `LoroDocumentStore` signatures).
    pub fn store(&self) -> &LoroDocumentStore {
        &self.store
    }

    /// Register a shared-subtree container into the replication set.
    ///
    /// Accepts only a plain, replicable [`LoroDocument`]. An owner-private
    /// `NonReplicated<T>` (holon-sharing) exposes no handle that fits here, so
    /// it can never enter the set (OQ3 — the exclusion is by construction, not
    /// a runtime filter). Fails loud on a duplicate id or a collision with the
    /// reserved root id.
    pub async fn register_container(
        &self,
        id: impl Into<String>,
        doc: Arc<LoroDocument>,
    ) -> Result<()> {
        let id = id.into();
        if id == ROOT_CONTAINER_ID {
            return Err(anyhow!(
                "container id `{id}` collides with the reserved root container id"
            ));
        }
        let mut extra = self.extra.write().await;
        if extra.iter().any(|c| c.id == id) {
            return Err(anyhow!("container `{id}` is already registered"));
        }
        extra.push(RegisteredContainer { id, doc });
        Ok(())
    }

    /// THE replication set (C1): the root container followed by every
    /// registered extra container. **No filter parameter** — this is the whole
    /// negative space Inc 3 types away. Iterating this is replicate-all.
    pub async fn replication_set(&self) -> Result<Vec<RegisteredContainer>> {
        let root = self.store.get_doc(DocScope::Global).await?;
        let mut set = vec![RegisteredContainer {
            id: ROOT_CONTAINER_ID.to_string(),
            doc: root,
        }];
        set.extend(self.extra.read().await.iter().cloned());
        Ok(set)
    }

    /// The set of registered container ids — the input to the orphan-row
    /// tripwire (holon-sharing `assert_no_orphan_rows`): every projected block
    /// row's container must be one of these.
    pub async fn registered_container_ids(&self) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        ids.insert(ROOT_CONTAINER_ID.to_string());
        for c in self.extra.read().await.iter() {
            ids.insert(c.id.clone());
        }
        ids
    }

    /// A point-in-time A7 subtree-containment index over the root container's
    /// live LoroTree — the real relation the holon-sharing policy layer's
    /// disjointness check needs (it supplies only the trait; the registry
    /// supplies the tree). The caller snapshots it once (this `await`) and the
    /// returned [`SubtreeIndex`] answers containment synchronously, which is
    /// what a policy-commit-time `SubtreeContainment` impl requires.
    pub async fn subtree_index(&self) -> Result<SubtreeIndex> {
        let doc = self.store.get_doc(DocScope::Global).await?;
        let blocks = doc.with_read(|d| Ok(snapshot_blocks_from_doc(d)))?;
        // child_uri -> parent_uri, both in EntityUri string form for a
        // scheme-consistent walk.
        let parents: HashMap<String, String> = blocks
            .values()
            .map(|sb| {
                (
                    sb.block.id.as_str().to_string(),
                    sb.block.parent_id.as_str().to_string(),
                )
            })
            .collect();
        Ok(SubtreeIndex { parents })
    }

    /// Convenience: does the subtree rooted at `selector` contain `block`? See
    /// [`SubtreeIndex::contains`].
    pub async fn subtree_contains(&self, selector: &str, block: &str) -> Result<bool> {
        Ok(self.subtree_index().await?.contains(selector, block))
    }
}

/// A point-in-time snapshot of the container tree's parent chains, answering
/// A7 subtree-containment synchronously. `selector`/`block` are [`EntityUri`]
/// string forms (as returned by `EntityUri::as_str`).
///
/// [`EntityUri`]: holon_api::EntityUri
pub struct SubtreeIndex {
    parents: HashMap<String, String>,
}

impl SubtreeIndex {
    /// Does the subtree rooted at `selector` contain `block` (reflexive:
    /// `contains(x, x)` is always true)?
    pub fn contains(&self, selector: &str, block: &str) -> bool {
        if selector == block {
            return true;
        }
        // Walk `block`'s ancestors up to the root; bounded by the node count so
        // a corrupted cyclic parent chain fails as "not contained", never hangs.
        let mut cur = block;
        for _ in 0..=self.parents.len() {
            match self.parents.get(cur) {
                Some(parent) if parent == selector => return true,
                Some(parent) => cur = parent,
                None => return false,
            }
        }
        false
    }
}

#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
mod replicate {
    use iroh::EndpointAddr;

    use super::*;
    use crate::iroh_advertiser::IrohAdvertiser;
    use crate::iroh_advertiser::SharedRoster;

    impl ContainerRegistry {
        /// The everything-policy fast path (ADR C1). Advertises **every**
        /// container in the replication set, each gated by the self-device
        /// `roster`. Returns each container's `(id, addr)`.
        ///
        /// There is deliberately **no filter parameter** and **no un-gated
        /// variant**: the self path cannot overshare because it has nothing to
        /// mis-set, and it cannot serve an un-enrolled peer because `roster`
        /// (the SELF/THIRD-PARTY classification) gates admission at
        /// `acceptor_enroll`. A third-party peer is not owner-signed into this
        /// roster and never reaches a container doc — it takes the per-share
        /// extract-prune-mount path instead.
        ///
        /// No policy predicate is evaluated on this path (the Inc 0(a) hook
        /// shape): enrollment is identity, not per-share audience policy.
        pub async fn replicate_all(
            &self,
            advertiser: &IrohAdvertiser,
            roster: SharedRoster,
        ) -> Result<Vec<(String, EndpointAddr)>> {
            let mut advertised = Vec::new();
            for container in self.replication_set().await? {
                let addr = advertiser
                    .start_share_gated(
                        container.id.clone(),
                        // ALLOW(loro_doc_escape): handed to the iroh advertiser
                        // as a sync transport handle, not read here.
                        container.doc.doc(),
                        roster.clone(),
                        None,
                        None,
                    )
                    .await?;
                advertised.push((container.id, addr));
            }
            Ok(advertised)
        }
    }
}

#[cfg(test)]
mod tests {
    use loro::LoroDoc;
    use loro::LoroText;

    use super::*;
    use crate::loro_backend::STABLE_ID;
    use crate::loro_backend::TREE_NAME;

    fn tmp_store() -> LoroDocumentStore {
        let dir = std::env::temp_dir().join(format!("holon-registry-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        LoroDocumentStore::new(dir)
    }

    fn container_doc(id: &str) -> Arc<LoroDocument> {
        Arc::new(LoroDocument::new(id.to_string()).unwrap())
    }

    #[tokio::test]
    async fn replication_set_is_root_plus_registered() -> Result<()> {
        let registry = ContainerRegistry::new(tmp_store());
        registry
            .register_container("share-b", container_doc("share-b"))
            .await?;

        let ids: Vec<String> = registry
            .replication_set()
            .await?
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(
            ids,
            vec![ROOT_CONTAINER_ID.to_string(), "share-b".to_string()]
        );
        assert_eq!(
            registry.registered_container_ids().await,
            BTreeSet::from([ROOT_CONTAINER_ID.to_string(), "share-b".to_string()])
        );
        Ok(())
    }

    #[tokio::test]
    async fn duplicate_and_reserved_ids_fail_loud() -> Result<()> {
        let registry = ContainerRegistry::new(tmp_store());
        registry
            .register_container("share-b", container_doc("share-b"))
            .await?;
        assert!(
            registry
                .register_container("share-b", container_doc("share-b"))
                .await
                .is_err()
        );
        assert!(
            registry
                .register_container(ROOT_CONTAINER_ID, container_doc("x"))
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn subtree_contains_walks_the_real_tree() -> Result<()> {
        // Build root -> child -> grandchild in the store's global doc.
        let store = tmp_store();
        let doc = store.get_doc(DocScope::Global).await?;
        let raw: Arc<LoroDoc> = doc.doc();
        let (root_uri, child_uri, gc_uri, sibling_uri);
        {
            let tree = raw.get_tree(TREE_NAME);
            tree.enable_fractional_index(0);
            let mk = |tree: &loro::LoroTree, parent: Option<loro::TreeID>, sid: &str| {
                let node = tree.create(parent).unwrap();
                let meta = tree.get_meta(node).unwrap();
                meta.insert(STABLE_ID, sid).unwrap();
                let text: LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
                text.insert(0, "x").unwrap();
                node
            };
            let root = mk(&tree, None, "root");
            let child = mk(&tree, Some(root), "child");
            let gc = mk(&tree, Some(child), "grandchild");
            let sibling = mk(&tree, None, "sibling");
            let index = crate::loro_backend::build_tid_index(&raw);
            root_uri = index.get(&root).cloned().unwrap();
            child_uri = index.get(&child).cloned().unwrap();
            gc_uri = index.get(&gc).cloned().unwrap();
            sibling_uri = index.get(&sibling).cloned().unwrap();
        }
        raw.commit();

        let registry = ContainerRegistry::new(store);
        // Reflexive.
        assert!(registry.subtree_contains(&root_uri, &root_uri).await?);
        // Direct and transitive descendants.
        assert!(registry.subtree_contains(&root_uri, &child_uri).await?);
        assert!(registry.subtree_contains(&root_uri, &gc_uri).await?);
        // Not an ancestor / disjoint sibling.
        assert!(!registry.subtree_contains(&child_uri, &root_uri).await?);
        assert!(!registry.subtree_contains(&root_uri, &sibling_uri).await?);
        Ok(())
    }
}
