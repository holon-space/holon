//! Purging the root containers a deleted block leaves behind.
//!
//! A block's mergeable children (`content_raw`, `properties`, …) are ROOT
//! containers: `ContainerID::new_mergeable` derives a deterministic root name
//! from `(parent meta map, key, kind)`. `LoroTree::delete` removes the node and
//! its meta map, but a root container has no parent to die with — it stays
//! live, holding the deleted block's full content in state and therefore in
//! every export.
//!
//! The purge clears the STATE, which is what shallow exports carry. It does not
//! reach the oplog: a full-history export still contains the deleted block's
//! original ops. `LoroDocumentStore::save_all` writes a full snapshot on 63 of
//! every 64 saves and a shallow one on the 64th, so on-disk plaintext survives
//! until the next compaction (see tasks #79/#80).
//!
//! `LoroDoc::delete_root_container` empties such a root with ordinary deletion
//! ops, so a peer that never purges converges on the same (empty) value once it
//! imports them.

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use loro::ContainerID;
use loro::ContainerTrait;
use loro::LoroDoc;
use loro::LoroMap;
use loro::LoroTree;
use loro::LoroValue;
use loro::TreeID;
use loro::ValueOrContainer;

/// Collect the root containers of `node` and every live descendant.
///
/// Must run BEFORE the delete: `LoroTree::delete` cascades to descendants, and
/// a gone node no longer names its roots. Descendants already deleted by an
/// earlier operation are not reachable here; their roots were purged by that
/// operation.
///
/// Errors if a node in the walk is absent from the tree. Under-collecting here
/// fails toward leaving plaintext behind, so an unwalkable node is loud rather
/// than an empty child list. `children` returning `None` is NOT that signal —
/// the tree state keys children by parent, so a childless leaf has no entry.
pub fn subtree_roots(tree: &LoroTree, node: TreeID) -> Result<Vec<ContainerID>> {
    let mut out = Vec::new();
    let mut queue = vec![node];
    while let Some(current) = queue.pop() {
        if !tree.contains(current) {
            bail!(
                "cannot collect the roots to purge under {current:?}: the node is absent from \
                 the tree, so its descendants' containers would be left behind"
            );
        }
        let meta = tree
            .get_meta(current)
            .with_context(|| format!("meta of block being deleted {current:?}"))?;
        collect_into(&meta, &mut out)?;
        queue.extend(tree.children(current).unwrap_or_default());
    }
    Ok(out)
}

/// Collect the root containers reachable from a tree node's `meta` map,
/// depth-first through nested mergeable maps.
///
/// Call this while the node is still alive: once it is deleted the meta map is
/// gone and the roots are no longer reachable by name.
pub fn mergeable_roots_under(meta: &LoroMap) -> Result<Vec<ContainerID>> {
    let mut out = Vec::new();
    collect_into(meta, &mut out)?;
    Ok(out)
}

fn collect_into(meta: &LoroMap, out: &mut Vec<ContainerID>) -> Result<()> {
    let keys: Vec<String> = match meta.get_value() {
        LoroValue::Map(m) => m.keys().cloned().collect(),
        other => bail!("tree node meta is not a map: {other:?}"),
    };

    for key in keys {
        let Some(ValueOrContainer::Container(container)) = meta.get(&key) else {
            continue;
        };
        if let loro::Container::Map(nested) = &container {
            collect_into(nested, out)?;
        }
        let cid = container.id();
        if cid.is_root() {
            out.push(cid);
        }
    }
    Ok(())
}

/// Purge the given root containers, emptying each with real deletion ops.
///
/// Idempotent across peers: a container another peer already purged is simply
/// empty, and re-purging it is a no-op write. A container that no longer exists
/// in this document is skipped — the purge ops arrived from the peer that owned
/// the delete.
pub fn purge_roots(doc: &LoroDoc, cids: &[ContainerID]) -> Result<()> {
    for cid in cids {
        if !doc.has_container(cid) {
            continue;
        }
        doc.delete_root_container(cid.clone())
            .with_context(|| format!("purging deleted block's root container {cid:?}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use holon_api::BlockContent;
    use holon_api::EntityUri;
    use holon_api::Value;
    use holon_api::repository::CoreOperations;
    use loro::ExportMode;

    use super::subtree_roots;
    use crate::LoroDocument;
    use crate::loro_backend::LoroBackend;

    const SECRET: &str = "PURGE-RUNG-SECRET-4b7e2a91";

    async fn backend_with_secret_block() -> (Arc<LoroDocument>, Arc<LoroBackend>) {
        let doc = Arc::new(LoroDocument::new("purge-rung".to_string()).unwrap());
        let backend = Arc::new(LoroBackend::from_document(doc.clone()));
        backend
            .create_block_with_properties(
                EntityUri::no_parent(),
                BlockContent::text(SECRET),
                Some(EntityUri::block("secret")),
                &HashMap::new(),
                &holon_api::BlockEdges::default(),
            )
            .await
            .unwrap();
        (doc, backend)
    }

    fn contains_secret(bytes: &[u8]) -> bool {
        String::from_utf8_lossy(bytes).contains(SECRET)
    }

    /// PRIVACY RUNG. Deleting a block must take its content with it: the
    /// shallow snapshot Holon persists (`export_compact_snapshot`) is the
    /// artifact a share, a backup or a stolen disk exposes.
    #[tokio::test]
    async fn deleting_a_block_removes_its_content_from_the_compact_snapshot() {
        let (doc, backend) = backend_with_secret_block().await;

        assert!(
            contains_secret(&doc.export_compact_snapshot().unwrap()),
            "precondition: the live block's content is in the compact snapshot"
        );

        backend.delete_block("block:secret").await.unwrap();

        assert!(
            !contains_secret(&doc.export_compact_snapshot().unwrap()),
            "deleted block's content survives in the persisted compact snapshot"
        );
    }

    /// The purge must not poison the id it freed. Undoing a delete re-creates
    /// the block under the same stable id, and the fork keeps a purged root
    /// permanently empty — so this only holds because a re-created node is a
    /// new tree node with new root container names.
    #[tokio::test]
    async fn recreating_a_deleted_block_under_the_same_id_restores_its_content() {
        let (_doc, backend) = backend_with_secret_block().await;
        backend.delete_block("block:secret").await.unwrap();

        backend
            .create_block_with_properties(
                EntityUri::no_parent(),
                BlockContent::text(SECRET),
                Some(EntityUri::block("secret")),
                &HashMap::new(),
                &holon_api::BlockEdges::default(),
            )
            .await
            .unwrap();

        let block = backend.get_block("block:secret").await.unwrap();
        assert!(
            format!("{:?}", block.content).contains(SECRET),
            "the re-created block came back empty — the purge poisoned its root \
             container: {:?}",
            block.content
        );
    }

    /// The same guarantee downstream: a peer bootstrapped from the persisted
    /// compact snapshot — the artifact `save_compact_to_file` writes and the
    /// shallow-share path ships — must neither carry nor be able to materialize
    /// the content.
    #[tokio::test]
    async fn a_peer_bootstrapped_from_the_compact_snapshot_cannot_materialize_the_content() {
        let (doc, backend) = backend_with_secret_block().await;
        backend.delete_block("block:secret").await.unwrap();

        let peer = LoroDocument::new("purge-rung-peer".to_string()).unwrap();
        peer.apply_update(&doc.export_compact_snapshot().unwrap())
            .unwrap();

        assert!(
            !contains_secret(&peer.export_compact_snapshot().unwrap()),
            "the bootstrapped peer re-exports the deleted block's content"
        );
        let value = peer.with_read(|d| Ok(d.get_deep_value())).unwrap();
        assert!(
            !format!("{value:?}").contains(SECRET),
            "the bootstrapped peer materializes the deleted block's content: {value:?}"
        );

        // The bootstrapped peer's history is trimmed, so its frontiers are the
        // shape `doc_lamport_height`'s `get_change(id).expect(...)` panics on if
        // a frontier ever references a trimmed change (task #78).
        peer.with_read(|d| Ok(crate::loro_backend::doc_lamport_height(d)))
            .unwrap();
    }

    /// CONVERGENCE RUNG. The purge is made of ordinary deletion ops, so a peer
    /// that edits the same block concurrently — and one that resurrects the
    /// node afterwards — must still converge with the purging peer, and the
    /// resurrected node must yield an EMPTY container rather than the content.
    #[tokio::test]
    async fn a_concurrent_editor_converges_and_resurrection_yields_an_empty_container() {
        let (doc_a, backend_a) = backend_with_secret_block().await;

        let doc_b = Arc::new(LoroDocument::new("purge-rung-b".to_string()).unwrap());
        let backend_b = Arc::new(LoroBackend::from_document(doc_b.clone()));
        sync(&doc_a, &doc_b);

        // Name the block's root containers while it is alive — after the delete
        // they are unreachable by name, and reading them by id is the only
        // oracle that distinguishes "purged" from "hidden but intact".
        let roots = doc_a
            .with_read(|d| {
                let tree = d.get_tree(crate::loro_backend::TREE_NAME);
                let mut roots = Vec::new();
                for node in tree.nodes() {
                    roots.extend(subtree_roots(&tree, node)?);
                }
                Ok(roots)
            })
            .unwrap();
        assert!(
            !roots.is_empty(),
            "precondition: the block has root children"
        );

        // B edits the block while A deletes (and purges) it. The edit must not
        // overwrite the content — a replacing write would remove the secret on
        // its own and make the oracle below vacuous.
        backend_b
            .update_block_properties(
                "block:secret",
                &HashMap::from([("b-touched".to_string(), Value::Boolean(true))]),
            )
            .await
            .unwrap();
        backend_a.delete_block("block:secret").await.unwrap();

        sync(&doc_a, &doc_b);

        let a_state = doc_a.with_read(|d| Ok(d.get_deep_value())).unwrap();
        let b_state = doc_b.with_read(|d| Ok(d.get_deep_value())).unwrap();
        assert_eq!(a_state, b_state, "peers diverged after a purge");

        // The purge writes ops on top of a merged DAG; the shadow-mesh oracle
        // reads `doc_lamport_height` at exactly such boundaries and panics if a
        // frontier references a change it cannot resolve (task #78).
        for (who, doc) in [("A", &doc_a), ("B", &doc_b)] {
            let height = doc
                .with_read(|d| Ok(crate::loro_backend::doc_lamport_height(d)))
                .unwrap();
            assert!(height > 0, "{who}: lamport height collapsed after a purge");
        }
        assert!(
            !format!("{a_state:?}").contains(SECRET),
            "the concurrent edit resurrected the purged content: {a_state:?}"
        );

        // Reading the roots by id is what a resurrection of the node would do.
        // On both peers a purged root is gone from the document, and if it is
        // still reachable it must be EMPTY, never intact.
        for (who, doc) in [("A", &doc_a), ("B", &doc_b)] {
            for cid in &roots {
                let content = doc
                    .with_read(|d| {
                        Ok(d.get_container(cid.clone()).map(|c| match c {
                            loro::Container::Text(t) => t.to_string(),
                            loro::Container::Map(m) => format!("{:?}", m.get_deep_value()),
                            other => format!("{other:?}"),
                        }))
                    })
                    .unwrap();
                let Some(content) = content else { continue };
                assert!(
                    !format!("{content:?}").contains(SECRET),
                    "{who} still serves the purged content from root {cid:?}: {content:?}"
                );
            }
        }
    }

    /// DISCLOSED GAP, not a purge defect. Once a doc has merged remote history,
    /// `export_compact_snapshot` (`shallow_snapshot` at the merged frontiers)
    /// stops compacting: the pre-merge ops — including the deleted block's
    /// original insert — stay in the exported bytes. Reproduced with a
    /// concurrent op on an UNRELATED block, so it is a property of the shallow
    /// export under a merged DAG, not of the purge, which the single-peer rung
    /// above shows working.
    ///
    /// Consequence for sharing: a multi-peer vault's on-disk snapshot, and the
    /// full snapshot `sync_doc_*` ships to a peer below the shallow base
    /// (`iroh_sync_adapter.rs:86`), still carry deleted plaintext. Closing it
    /// needs a loro-side shallow-export change, not a Holon call site.
    #[tokio::test]
    #[ignore = "known gap: shallow export stops compacting once remote history is merged; \
                needs a loro-fork change, not a purge call site"]
    async fn a_merged_dag_still_compacts_the_deleted_blocks_history() {
        let (doc_a, backend_a) = backend_with_secret_block().await;
        let doc_b = Arc::new(LoroDocument::new("purge-rung-merged".to_string()).unwrap());
        let backend_b = Arc::new(LoroBackend::from_document(doc_b.clone()));
        sync(&doc_a, &doc_b);

        // Deliberately UNRELATED to the secret block.
        backend_b
            .create_block_with_properties(
                EntityUri::no_parent(),
                BlockContent::text("unrelated"),
                Some(EntityUri::block("unrelated")),
                &HashMap::new(),
                &holon_api::BlockEdges::default(),
            )
            .await
            .unwrap();
        backend_a.delete_block("block:secret").await.unwrap();
        sync(&doc_a, &doc_b);

        assert!(
            !contains_secret(&doc_a.export_compact_snapshot().unwrap()),
            "a merged-history doc's compact snapshot retains the deleted block's content"
        );
    }

    fn sync(a: &LoroDocument, b: &LoroDocument) {
        let b_vv = b.doc().oplog_vv();
        let a_delta = a
            .with_read(|d| Ok(d.export(ExportMode::updates(&b_vv)).unwrap()))
            .unwrap();
        if !a_delta.is_empty() {
            b.apply_update(&a_delta).unwrap();
        }
        let a_vv = a.doc().oplog_vv();
        let b_delta = b
            .with_read(|d| Ok(d.export(ExportMode::updates(&a_vv)).unwrap()))
            .unwrap();
        if !b_delta.is_empty() {
            a.apply_update(&b_delta).unwrap();
        }
    }
}
