//! Block-tree fragments of the PBT reference model: layout classification and
//! the canonical block map. Extracted from `reference_state.rs`.

use std::collections::BTreeMap;
use std::collections::HashSet;

use holon_api::ContentType;
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

    /// Find a page block by its title (first line of content, e.g. "index").
    pub fn doc_uri_by_name(&self, title: &str) -> Option<EntityUri> {
        self.blocks
            .values()
            .find(|b| b.is_page() && b.title() == title)
            .map(|b| b.id.clone())
    }

    /// Get IDs of text blocks only (not source blocks).
    pub fn text_block_ids(&self) -> Vec<EntityUri> {
        self.blocks
            .iter()
            .filter(|(_, b)| b.content_type == ContentType::Text)
            .map(|(id, _)| id.clone())
            .collect()
    }

    // ── Block hierarchy query helpers ──────────────────────────────────

    /// Children of parent sorted by sequence then ID (matching canonical
    /// ordering).
    pub fn sorted_children_of(&self, parent_id: &EntityUri) -> Vec<&Block> {
        use holon_orgmode::models::OrgBlockExt;
        let mut children: Vec<&Block> = self
            .blocks
            .values()
            .filter(|b| b.parent_id == *parent_id)
            .collect();
        children.sort_by(|a, b| {
            a.sequence()
                .cmp(&b.sequence())
                .then_with(|| a.id.cmp(&b.id))
        });
        children
    }

    /// Predicted ordered child ids of `parent_id`. Mirrors what
    /// `BlockOrdering::children(parent_id)` should return on the live
    /// side. The encoding-free child-id list is the contract — both
    /// sides produce a `Vec<EntityUri>`, no `sort_key` / `sequence`
    /// strings cross the boundary.
    pub fn children_of(&self, parent_id: &EntityUri) -> Vec<EntityUri> {
        self.sorted_children_of(parent_id)
            .into_iter()
            .map(|b| b.id.clone())
            .collect()
    }

    /// Previous sibling of block_id (same parent, immediately before in
    /// sequence order).
    pub fn previous_sibling(&self, block_id: &EntityUri) -> Option<EntityUri> {
        let block = self.blocks.get(block_id)?;
        let children = self.sorted_children_of(&block.parent_id);
        let idx = children.iter().position(|b| b.id == *block_id)?;
        if idx > 0 {
            Some(children[idx - 1].id.clone())
        } else {
            None
        }
    }

    /// Next sibling of block_id (same parent, immediately after in sequence
    /// order).
    pub fn next_sibling(&self, block_id: &EntityUri) -> Option<EntityUri> {
        let block = self.blocks.get(block_id)?;
        let children = self.sorted_children_of(&block.parent_id);
        let idx = children.iter().position(|b| b.id == *block_id)?;
        children.get(idx + 1).map(|b| b.id.clone())
    }

    /// Grandparent of block_id (parent's parent). None if at root level.
    pub fn grandparent(&self, block_id: &EntityUri) -> Option<EntityUri> {
        let block = self.blocks.get(block_id)?;
        let parent = self.blocks.get(&block.parent_id)?;
        if parent.parent_id.is_no_parent() || parent.parent_id.is_sentinel() {
            None
        } else {
            Some(parent.parent_id.clone())
        }
    }

    /// Check if `block_id` is a descendant of any block in `roots` (or is
    /// itself in `roots`).
    pub fn is_descendant_of_any(
        &self,
        block_id: &EntityUri,
        roots: &std::collections::BTreeSet<EntityUri>,
    ) -> bool {
        if roots.contains(block_id) {
            return true;
        }
        // Walk up parent chain
        let mut current = block_id.clone();
        for _ in 0..50 {
            if let Some(block) = self.blocks.get(&current) {
                if roots.contains(&block.parent_id) {
                    return true;
                }
                if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                    return false;
                }
                current = block.parent_id.clone();
            } else {
                return false;
            }
        }
        false
    }

    /// Depth-first collection of text-block descendants of `parent_id`, in
    /// canonical child order, recording each visited block's parent (skipping
    /// the synthetic `no_parent` root). Backs `build_reference_navigator`'s
    /// tree/outline navigator construction.
    pub fn collect_dfs_order(
        &self,
        parent_id: &EntityUri,
        dfs_order: &mut Vec<EntityUri>,
        parent_map: &mut std::collections::HashMap<EntityUri, EntityUri>,
    ) {
        let children = self.sorted_children_of(parent_id);
        for child in children {
            if child.content_type != ContentType::Text {
                continue;
            }
            dfs_order.push(child.id.clone());
            if parent_id != &EntityUri::no_parent() {
                parent_map.insert(child.id.clone(), parent_id.clone());
            }
            self.collect_dfs_order(&child.id, dfs_order, parent_map);
        }
    }
}
