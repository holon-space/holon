//! One-time relocation of the bundled layout out of the replicated global doc.
//!
//! A vault written before the layout doc existed holds `block:__default__` and
//! its descendants in the global tree, where whole-store replication ships them
//! to every paired device — the collision D68.b rules out. This moves that
//! subtree across once, at boot.

use std::collections::HashMap;

use anyhow::Result;
use anyhow::anyhow;
use holon_api::block::Block;
use holon_api::repository::CoreOperations;

use crate::loro_backend::LoroBackend;
use crate::loro_backend::NewBlockWithProperties;
use crate::loro_backend::snapshot_blocks_from_doc;
use crate::loro_document_store::DocScope;
use crate::loro_document_store::LoroDocumentStore;

/// Move `block:__default__` and its descendants from the global doc into the
/// layout doc, and return how many blocks moved.
///
/// Idempotent: the second boot finds the subtree only in the layout doc and
/// moves nothing. An id live in BOTH docs is an ambiguity — the two writes
/// would have diverged independently — so it errors rather than picking one.
pub async fn migrate_layout_out_of_global(store: &LoroDocumentStore) -> Result<usize> {
    let global = store.get_doc(DocScope::Global).await?;
    let layout = store.get_doc(DocScope::Layout).await?;

    let in_global = global.with_read(|doc| Ok(snapshot_blocks_from_doc(doc)))?;
    let in_layout = layout.with_read(|doc| Ok(snapshot_blocks_from_doc(doc)))?;

    let subtree = layout_subtree(&in_global);
    if subtree.is_empty() {
        return Ok(0);
    }

    let both: Vec<&str> = subtree
        .iter()
        .map(|b| b.id.as_str())
        .filter(|id| in_layout.contains_key(*id))
        .collect();
    if !both.is_empty() {
        return Err(anyhow!(
            "layout migration refused: {} block(s) are live in BOTH the global and the layout Loro \
             doc ({}). A half-migrated vault cannot be resolved by a rule — the two copies may \
             have diverged independently.",
            both.len(),
            both.join(", ")
        ));
    }

    let layout_backend = LoroBackend::from_document(layout);
    let requests: Vec<NewBlockWithProperties> = subtree
        .iter()
        .map(|b| NewBlockWithProperties {
            parent_id: b.parent_id.clone(),
            id: b.id.clone(),
            content: b.to_block_content(),
            properties: b.properties.clone(),
            edges: holon_api::BlockEdges::of(b),
        })
        .collect();
    layout_backend
        .create_blocks_with_properties(requests)
        .await
        .map_err(|e| anyhow!("layout migration: writing the subtree into the layout doc: {e:?}"))?;

    let global_backend = LoroBackend::from_document(global);
    // Leaves first: deleting a parent first would take its children with it,
    // and the per-id delete is what keeps this loop's count honest.
    for block in subtree.iter().rev() {
        global_backend
            .delete_block(block.id.as_str())
            .await
            .map_err(|e| {
                anyhow!(
                    "layout migration: removing {} from the global doc: {e:?}",
                    block.id
                )
            })?;
    }

    store.save_all().await?;
    Ok(subtree.len())
}

/// `block:__default__` and its descendants, PARENTS BEFORE CHILDREN and each
/// sibling group in the tree's own order — the order the layout doc's creates
/// have to replay to reproduce the authored column order.
fn layout_subtree(blocks: &HashMap<String, crate::loro_backend::SnapshotBlock>) -> Vec<Block> {
    let Some(root) = blocks.get(holon_api::DEFAULT_DOC_BLOCK_ID) else {
        return Vec::new();
    };
    let mut children: HashMap<&str, Vec<&crate::loro_backend::SnapshotBlock>> = HashMap::new();
    for snap in blocks.values() {
        children
            .entry(snap.block.parent_id.as_str())
            .or_default()
            .push(snap);
    }
    for group in children.values_mut() {
        group.sort_by(|a, b| {
            a.sort_key
                .cmp(&b.sort_key)
                .then(a.block.id.cmp(&b.block.id))
        });
    }

    let mut out = vec![root.block.clone()];
    let mut frontier = vec![root.block.id.as_str().to_string()];
    while let Some(parent) = frontier.pop() {
        for snap in children.get(parent.as_str()).into_iter().flatten() {
            out.push(snap.block.clone());
            frontier.push(snap.block.id.as_str().to_string());
        }
    }
    out
}
