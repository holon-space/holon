//! Binding between the C1 container registry (`holon-loro`) and this crate's
//! sharing contracts (ADR 0028 Increment 3). The registry lives in `holon-loro`
//! (it owns the doc store + container tree); the contracts that reference it —
//! the A7 [`SubtreeContainment`] relation and the orphan-row tripwire — live
//! here, on the side of the dependency edge that can see both. (`holon-loro`
//! cannot depend on `holon-sharing`; the edge points the other way.)
//!
//! ## A7 disjointness gets the real relation
//!
//! [`RegistryContainment`] wraps a [`SubtreeIndex`] the registry snapshots from
//! its live container tree, so [`PolicySet::commit`] enforces A7 nesting
//! against real block ancestry instead of the test [`MapContainment`].
//!
//! ## Orphan-row tripwire wired for real
//!
//! [`assert_registry_no_orphan_rows`] resolves the projection invariant "every
//! projected block row's container is a registered container" against the
//! registry's actual registered-container set — the wiring the Inc 4 projection
//! helper was authored to receive.

use std::collections::BTreeSet;

use holon_loro::SubtreeIndex;

use crate::policy::SubtreeContainment;
use crate::projection::OrphanRowError;
use crate::projection::assert_no_orphan_rows;
use crate::types::BlockId;
use crate::types::ContainerId;

/// A [`SubtreeContainment`] backed by the registry's live container tree. Build
/// it from `registry.subtree_index().await` (in `holon-loro`) and hand it to
/// [`crate::policy::PolicySet::commit`].
///
/// [`crate::policy::PolicySet::commit`]: crate::policy::PolicySet
pub struct RegistryContainment {
    index: SubtreeIndex,
}

impl RegistryContainment {
    /// Wrap a snapshot the registry produced. Kept explicit (rather than taking
    /// `&ContainerRegistry` and awaiting) so this stays a pure, synchronous
    /// relation — exactly what the commit-time A7 check consumes.
    pub fn new(index: SubtreeIndex) -> Self {
        Self { index }
    }
}

impl SubtreeContainment for RegistryContainment {
    fn contains(&self, selector: &BlockId, block: &BlockId) -> bool {
        self.index.contains(&selector.0, &block.0)
    }
}

/// The orphan-row tripwire (Inc 4 projection invariant), resolved against a
/// registry's registered-container set.
///
/// `registered` is `registry.registered_container_ids().await` (in
/// `holon-loro`). `projected` is every *present* projected block row as
/// `(block, container)`. Fails loud on the first row whose container is not
/// registered — a crossing left a silently orphaned row behind. Transient
/// invisibility during a migration window is row ABSENCE and does not trip
/// this (there is no row to point at an unregistered container).
pub fn assert_registry_no_orphan_rows<'a, I>(
    projected: I,
    registered: &BTreeSet<String>,
) -> Result<(), OrphanRowError>
where
    I: IntoIterator<Item = (&'a BlockId, &'a ContainerId)>,
{
    let registered: BTreeSet<ContainerId> = registered.iter().cloned().map(ContainerId).collect();
    assert_no_orphan_rows(projected, &registered)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use holon_loro::ContainerRegistry;
    use holon_loro::LoroDocument;
    use holon_loro::LoroDocumentStore;

    use super::*;
    use crate::alias_ledger::AliasLedger;
    use crate::policy::MapContainment;
    use crate::types::StablePeerId;
    use crate::types::UnverifiedAuthority;

    fn tmp_store() -> LoroDocumentStore {
        let dir = std::env::temp_dir().join(format!(
            "holon-sharing-registry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        LoroDocumentStore::new(dir)
    }

    // The core Inc-3 gate: replicate_all iterates EXACTLY the replicable set.
    // The owner-private alias ledger's backing doc is `NonReplicated<LoroDoc>`,
    // which exposes no handle `register_container` accepts — so it can never
    // enter the set (OQ3, exclusion by construction). Constructing the ledger
    // here makes that concrete: there is no line that could register its doc.
    #[tokio::test]
    async fn replication_set_is_the_replicable_set_and_excludes_nonreplicated() {
        let registry = ContainerRegistry::new(tmp_store());
        registry
            .register_container(
                "share-b",
                Arc::new(LoroDocument::new("share-b".to_string()).unwrap()),
            )
            .await
            .unwrap();

        // Owner-private, NonReplicated. No API path routes its doc into the
        // registry (the following would not compile — NonReplicated yields no
        // replication handle):
        //   registry.register_container("ledger", ledger.doc()) // no such method
        let _ledger = AliasLedger::new(StablePeerId(7), Box::new(UnverifiedAuthority));

        let ids: BTreeSet<String> = registry
            .replication_set()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(
            ids,
            BTreeSet::from(["holon_tree".to_string(), "share-b".to_string()])
        );
        assert!(!ids.iter().any(|id| id.contains("ledger")));
    }

    // A `SubtreeIndex` is only constructible via the registry's async
    // `subtree_index()`, which needs a live doc store; the containment *logic*
    // it wraps is proven in holon-loro's `subtree_contains_walks_the_real_tree`.
    // Here we prove the two Inc-3 contracts that live on THIS side of the edge.

    #[test]
    fn orphan_tripwire_passes_when_every_container_registered() {
        let registered = BTreeSet::from(["holon_tree".to_string(), "share-b".to_string()]);
        let b1 = BlockId("block:1".into());
        let c_root = ContainerId("holon_tree".into());
        let b2 = BlockId("block:2".into());
        let c_b = ContainerId("share-b".into());
        let rows = vec![(&b1, &c_root), (&b2, &c_b)];
        assert!(assert_registry_no_orphan_rows(rows, &registered).is_ok());
    }

    #[test]
    fn orphan_tripwire_fails_loud_on_unregistered_container() {
        let registered = BTreeSet::from(["holon_tree".to_string()]);
        let b = BlockId("block:1".into());
        let c = ContainerId("ghost-container".into());
        let rows = vec![(&b, &c)];
        let err = assert_registry_no_orphan_rows(rows, &registered).unwrap_err();
        assert_eq!(err.container, c);
        assert_eq!(err.block, b);
    }

    // Sanity: `MapContainment` (test) and `RegistryContainment` (prod) are the
    // same `SubtreeContainment` seam PolicySet::commit consumes — the registry
    // one just carries a real tree instead of a hand-built map.
    #[test]
    fn containment_is_a_trait_object_seam() {
        fn takes_rel(rel: &dyn SubtreeContainment, a: &BlockId, b: &BlockId) -> bool {
            rel.contains(a, b)
        }
        let map =
            MapContainment::new().with_subtree(BlockId("root".into()), [BlockId("child".into())]);
        let root = BlockId("root".into());
        let child = BlockId("child".into());
        assert!(takes_rel(&map, &root, &child));
    }
}
