//! Projected Blocks → the store, through the `BlockOrdering` boundary.
//!
//! This is the only module that knows a store exists; decode and projection
//! stay `holon-api`-only. Two passes, in the order the trait's contract
//! requires: create every block parent-before-child, then state each parent's
//! total sibling order and let the order owner mint a fresh key sequence
//! (invariants 2, 3, 10 — we say what the sequence is, we never write
//! LogSeq's fracdex).

use anyhow::Result;
use holon_api::Block;
use holon_api::EntityUri;
use holon_core::block_ordering::BlockCreateRequest;
use holon_core::block_ordering::BlockOrdering;

use crate::ImportError;
use crate::project::Projection;

/// What entering the store did, for the caller to assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestReport {
    pub blocks_created: usize,
    /// How many creates the ordering AUTHORITY reported it persisted. A
    /// tree-backed authority may decline (returning `false`), in which case the
    /// block exists only if some other path creates it — and its position is
    /// not the authority's to state. Surfaced rather than swallowed, because a
    /// silent decline looks exactly like a successful import.
    pub blocks_persisted_by_authority: usize,
    /// Parents whose total sibling order was re-minted.
    pub parents_ordered: usize,
}

/// Create every projected block, then realize the sibling order.
pub async fn enter_store(
    projection: &Projection,
    ordering: &dyn BlockOrdering,
) -> Result<IngestReport> {
    let ordered = parents_before_children(&projection.blocks)?;
    let requests: Vec<BlockCreateRequest> =
        ordered.iter().map(|block| create_request(block)).collect();
    let persisted = ordering
        .create_in_tree_batch(&requests)
        .await
        .map_err(|e| anyhow::anyhow!("create_in_tree_batch({} block(s)): {e}", requests.len()))?;
    anyhow::ensure!(
        persisted.len() == requests.len(),
        "create_in_tree_batch returned {} flag(s) for {} request(s)",
        persisted.len(),
        requests.len()
    );
    // A `false` flag is a ROUTING SIGNAL, not a failure. The importer has no
    // second create route, so a declined block would simply never exist —
    // refuse, rather than report a clean import of nothing. The flag itself
    // only says the authority declined; it does not carry a reason, so the
    // message offers the expected cause without asserting it.
    let declined = persisted.iter().filter(|p| !**p).count();
    anyhow::ensure!(
        declined == 0,
        "the ordering authority declined {declined} of {} create(s) through create_in_tree, \
         and this importer has no second create route, so those blocks would not exist. \
         The flag carries no reason; the expected cause is a store that consolidates creates \
         itself and takes them via update_in_tree (Loro-consolidated mode) — the importer needs \
         one whose authority accepts create_in_tree (Turso-backed). Nothing here indicates \
         corruption: the graph was read fine, and this is about the destination.",
        requests.len()
    );

    for (parent, children) in &projection.ordered_children {
        ordering.place_all(parent, children).await.map_err(|e| {
            anyhow::anyhow!("place_all({parent}, {} child(ren)): {e}", children.len())
        })?;
    }

    Ok(IngestReport {
        blocks_created: requests.len(),
        blocks_persisted_by_authority: persisted.iter().filter(|p| **p).count(),
        parents_ordered: projection.ordered_children.len(),
    })
}

fn create_request(block: &Block) -> BlockCreateRequest {
    BlockCreateRequest::of(block, &block.parent_id)
}

/// Order the blocks so every block follows its parent.
///
/// `create_in_tree_batch` creates in request order, so a child ahead of its
/// parent has nowhere to attach. LogSeq's entity ids do not imply this order
/// and the projection sorts by uuid, so the batch has to be re-sequenced here.
/// A block whose parent never appears is not silently promoted to a root — an
/// import that quietly reparents a subtree looks successful and is not.
fn parents_before_children(blocks: &[Block]) -> Result<Vec<&Block>, ImportError> {
    let mut by_parent: std::collections::HashMap<&EntityUri, Vec<&Block>> =
        std::collections::HashMap::new();
    for block in blocks {
        by_parent.entry(&block.parent_id).or_default().push(block);
    }

    let root = EntityUri::no_parent();
    let mut out: Vec<&Block> = Vec::with_capacity(blocks.len());
    let mut frontier: Vec<&Block> = by_parent.get(&root).cloned().unwrap_or_default();
    while let Some(block) = frontier.pop() {
        out.push(block);
        if let Some(children) = by_parent.get(&block.id) {
            frontier.extend(children.iter().copied());
        }
    }

    if out.len() != blocks.len() {
        let unreachable: Vec<String> = blocks
            .iter()
            .filter(|b| !out.iter().any(|kept| kept.id == b.id))
            .take(5)
            .map(|b| format!("{} (parent {})", b.id, b.parent_id))
            .collect();
        return Err(ImportError::UnreachableBlocks {
            count: blocks.len() - out.len(),
            sample: unreachable.join(", "),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, parent: EntityUri) -> Block {
        Block {
            id: EntityUri::block(id),
            parent_id: parent,
            ..Block::default()
        }
    }

    #[test]
    fn every_block_follows_its_parent() {
        // Deliberately reversed: child listed before parent, grandchild first.
        let blocks = vec![
            block("c", EntityUri::block("b")),
            block("b", EntityUri::block("a")),
            block("a", EntityUri::no_parent()),
        ];
        let ordered = parents_before_children(&blocks).expect("orderable");
        let ids: Vec<String> = ordered.iter().map(|b| b.id.to_string()).collect();
        let position = |id: &str| {
            ids.iter()
                .position(|x| x == &EntityUri::block(id).to_string())
                .expect("present")
        };
        assert!(position("a") < position("b"));
        assert!(position("b") < position("c"));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn a_block_whose_parent_is_missing_is_a_loud_error() {
        let blocks = vec![block("orphan", EntityUri::block("nowhere"))];
        let err = parents_before_children(&blocks)
            .expect_err("an unreachable block must stop the import");
        assert!(
            matches!(err, ImportError::UnreachableBlocks { count: 1, .. }),
            "got {err:?}"
        );
    }

    /// A parent cycle is unreachable from the root, so it trips the same
    /// guard rather than spinning forever.
    #[test]
    fn a_parent_cycle_is_a_loud_error() {
        let blocks = vec![
            block("x", EntityUri::block("y")),
            block("y", EntityUri::block("x")),
        ];
        let err = parents_before_children(&blocks).expect_err("a cycle must stop the import");
        assert!(
            matches!(err, ImportError::UnreachableBlocks { count: 2, .. }),
            "got {err:?}"
        );
    }
}
