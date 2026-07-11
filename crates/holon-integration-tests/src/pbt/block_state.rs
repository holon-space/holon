//! Block-tree fragments of the PBT reference model: layout classification and
//! the canonical block map. Extracted from `reference_state.rs`.

use std::collections::BTreeMap;
use std::collections::HashSet;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;

/// Typed classification of layout block IDs in index.org.
///
/// Layout blocks are split into three categories with different mutation rules:
/// - **headline_ids**: The text headline blocks that parent query/render
///   sources. These can have content, task_state, priority, tags mutated.
/// - **query_source_ids**: PRQL/GQL/SQL source blocks. These are truly
///   immutable because changing them would break `initial_widget()`.
/// - **render_source_ids**: Render DSL source blocks. These can have their
///   content changed to any valid render expression.
#[derive(Debug, Clone, Default)]
pub struct LayoutBlockInfo {
    pub headline_ids: HashSet<EntityUri>,
    pub query_source_ids: HashSet<EntityUri>,
    pub render_source_ids: HashSet<EntityUri>,
}

impl LayoutBlockInfo {
    /// Returns true if the block is part of the layout at all.
    pub fn contains(&self, id: &EntityUri) -> bool {
        self.headline_ids.contains(id)
            || self.query_source_ids.contains(id)
            || self.render_source_ids.contains(id)
    }

    /// Returns true if the block must never be mutated (query sources only).
    pub fn is_immutable(&self, id: &EntityUri) -> bool {
        self.query_source_ids.contains(id)
    }

    /// Returns true if the block is focusable — i.e. it has an EditableText
    /// node. Source blocks (query/render) are NOT focusable. Headline
    /// blocks (parents of source blocks) ARE focusable in the current
    /// reference model because the PBT uses them as navigation targets;
    /// marking them non-focusable would break ClickBlock generation
    /// entirely (see note in the editable transition generation).
    pub fn is_focusable(&self, id: &EntityUri) -> bool {
        !self.query_source_ids.contains(id) && !self.render_source_ids.contains(id)
    }

    /// Remove a block from all sets.
    pub fn remove(&mut self, id: &EntityUri) {
        self.headline_ids.remove(id);
        self.query_source_ids.remove(id);
        self.render_source_ids.remove(id);
    }
}

/// Block-related state that is affected by undo/redo operations.
/// Extracted so snapshots can be taken via `.clone()` before UI mutations.
#[derive(Debug, Clone)]
pub struct BlockState {
    /// Canonical block state (using production Block struct).
    ///
    /// `BTreeMap` (not `HashMap`) so iteration order is deterministic across
    /// process instantiations. The PBT canonicalizer (`apply_mutation`,
    /// `recanon_and_rebuild`) builds a `Vec<Block>` from these values and the
    /// resulting sequence numbers depend on iteration order — `HashMap`'s
    /// random seed made the same proptest seed produce different reference
    /// states across runs.
    pub blocks: BTreeMap<EntityUri, Block>,

    /// Mapping of block_id → doc_uri (persists even after blocks are deleted).
    /// `BTreeMap` for the same determinism reason as `blocks`.
    pub block_documents: BTreeMap<EntityUri, EntityUri>,

    /// ID counter for generating unique block IDs
    pub next_id: usize,
}

impl BlockState {
    /// Return a clone with every block's `id`/`parent_id` and the
    /// `block_documents` keys remapped through `map` (synthetic doc URI →
    /// real SUT UUID). URIs absent from `map` (i.e. all content-block IDs,
    /// which the ref and SUT already share) pass through unchanged.
    ///
    /// Instead of every invariant translating IDs at each comparison point,
    /// the reference model is mapped *once* into the SUT's ID space so
    /// capability-bound invariant bodies can compare directly. Only doc URIs
    /// differ, and only block `id`/`parent_id` + `block_documents` keys carry
    /// them, so this resolves exactly `block.id`, `block.parent_id`, and the
    /// `block_documents` keys.
    pub fn remapped_doc_uris(&self, map: &BTreeMap<EntityUri, EntityUri>) -> BlockState {
        let resolve = |u: &EntityUri| map.get(u).cloned().unwrap_or_else(|| u.clone());
        let blocks = self
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                b.id = resolve(&b.id);
                b.parent_id = resolve(&b.parent_id);
                // `requires` is an edge field of block-id references; remap its
                // targets into SUT ID space too so an edge-field comparison
                // (e.g. `/matview`) matches when a dependency points at a
                // minted (split-reconciled) block, not just a stable seed id.
                b.requires = b.requires.iter().map(|u| resolve(u)).collect();
                // `advice_suppressed` is likewise an edge field of block-id
                // references (ADR 0021 dismissal set); remap its targets the
                // same way, or a dismissal pointing at a split-reconciled block
                // keeps the synthetic id and diverges from the SUT's resolved id.
                b.advice_suppressed = b.advice_suppressed.iter().map(|u| resolve(u)).collect();
                (b.id.clone(), b)
            })
            .collect();
        let block_documents = self
            .block_documents
            .iter()
            .map(|(id, doc)| (resolve(id), doc.clone()))
            .collect();
        BlockState {
            blocks,
            block_documents,
            next_id: self.next_id,
        }
    }
}
