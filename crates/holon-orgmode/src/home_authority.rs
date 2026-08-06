//! Production [`HomeAuthority`] over the block-store seams.
//!
//! `home_by`'s accumulator is derived from authority reads rather than from
//! feed values, because the feed lags. This adapter supplies those reads from
//! the same seams the write-back layer already uses: [`BlockReader`] for the
//! ancestor walk that resolves a block's owning document, and [`BlockOrdering`]
//! for sibling order. Nothing here reads `sort_key` — order is expressed as
//! ordered id lists, so ADR 0005 stands.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering as AtomicOrdering;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::live_data::home_by::HomeAuthority;
use holon_api::live_data::home_by::Placement;
use holon_core::block_ordering::BlockOrdering;
use holon_filesystem::BlockReader;
use holon_filesystem::nearest_page_ancestor;

/// The document a block belongs to, or the absence of one.
///
/// `Unresolved` mirrors the write-back resolver's non-fatal fallbacks — a block
/// with no `Page` ancestor, or one the authority no longer holds. It is a key
/// like any other, so such blocks group together and retract correctly instead
/// of vanishing.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DocHome {
    Resolved(EntityUri),
    Unresolved,
}

pub struct BlockHomeAuthority {
    reader: Arc<dyn BlockReader>,
    ordering: Arc<dyn BlockOrdering>,
    /// Counts authoritative point reads, so a benchmark can attribute the
    /// O(subtree) fan-out cost of a cross-document reparent.
    locate_reads: Arc<AtomicU64>,
}

impl BlockHomeAuthority {
    pub fn new(reader: Arc<dyn BlockReader>, ordering: Arc<dyn BlockOrdering>) -> Self {
        Self {
            reader,
            ordering,
            locate_reads: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Number of authoritative point reads issued so far.
    pub fn locate_reads(&self) -> u64 {
        self.locate_reads.load(AtomicOrdering::Relaxed)
    }

    pub fn reset_locate_reads(&self) {
        self.locate_reads.store(0, AtomicOrdering::Relaxed);
    }

    /// The document that owns `id`: the nearest `Page` at or above it. A page
    /// is its own document — the walk starts at the block itself, which is what
    /// makes a page-ness toggle observable as a document change on the toggled
    /// block.
    ///
    /// `first` is the row for `id` when the caller already read it, so `locate`
    /// does not pay a second point read for the block it just fetched.
    #[tracing::instrument(skip_all, name = "home.resolve_doc")]
    async fn resolve_doc(&self, id: &EntityUri, first: Option<Block>) -> Result<DocHome> {
        Ok(
            match nearest_page_ancestor(self.reader.as_ref(), id, first, Some(&self.locate_reads))
                .await?
            {
                Some(page) => DocHome::Resolved(page.id),
                None => DocHome::Unresolved,
            },
        )
    }
}

#[async_trait]
impl HomeAuthority<DocHome> for BlockHomeAuthority {
    #[tracing::instrument(skip_all, name = "home.locate")]
    async fn locate(&self, id: &str) -> Result<Option<Placement<DocHome>>> {
        let uri = EntityUri::parse(id)?;
        self.locate_reads.fetch_add(1, AtomicOrdering::Relaxed);
        let Some(block) = self.reader.get_block_authoritative(&uri).await? else {
            return Ok(None);
        };
        // `no_parent` is the tree's root sentinel, expressed to the combinator
        // as "no parent" so the root group is keyed uniformly with `None`.
        let parent = if block.parent_id == EntityUri::no_parent() {
            None
        } else {
            Some(block.parent_id.as_str().to_string())
        };
        let doc = self.resolve_doc(&uri, Some(block)).await?;
        Ok(Some(Placement { doc, parent }))
    }

    #[tracing::instrument(skip_all, name = "home.children_of")]
    async fn children_of(&self, parent: Option<&str>) -> Result<Vec<String>> {
        let parent_uri = match parent {
            Some(p) => EntityUri::parse(p)?,
            None => EntityUri::no_parent(),
        };
        let kids = self
            .ordering
            .children(&parent_uri)
            .await
            .map_err(|e| anyhow::anyhow!("children({parent_uri}) failed: {e}"))?;
        Ok(kids.into_iter().map(|k| k.as_str().to_string()).collect())
    }

    #[tracing::instrument(skip_all, name = "home.prev_sibling")]
    async fn prev_sibling(&self, id: &str) -> Result<Option<String>> {
        let uri = EntityUri::parse(id)?;
        let prev = self
            .ordering
            .prev_sibling(&uri)
            .await
            .map_err(|e| anyhow::anyhow!("prev_sibling({uri}) failed: {e}"))?;
        Ok(prev.map(|p| p.as_str().to_string()))
    }

    #[tracing::instrument(skip_all, name = "home.subtree_of")]
    async fn subtree_of(&self, id: &str) -> Result<Vec<String>> {
        // Breadth-first over `children`, which is the only ordered-descendant
        // read the seams expose. This is the O(subtree) cost a cross-document
        // reparent pays; see `reparent_fanout_cost` for the measurement.
        let root = EntityUri::parse(id)?;
        let mut out = Vec::new();
        let mut frontier = vec![root];
        while let Some(node) = frontier.pop() {
            let kids = self
                .ordering
                .children(&node)
                .await
                .map_err(|e| anyhow::anyhow!("children({node}) failed: {e}"))?;
            for k in kids {
                out.push(k.as_str().to_string());
                frontier.push(k);
            }
        }
        Ok(out)
    }

    /// Amortized snapshot placement: one `children` read per distinct parent
    /// for order, and a single top-down pass carrying the nearest enclosing
    /// page down for documents, so an ancestor walk is never repeated.
    ///
    /// The default would issue two point reads per block, which at cold-boot
    /// scale is the per-block boot work already known to destabilise startup.
    #[tracing::instrument(skip_all, name = "home.locate_batch")]
    async fn locate_batch(&self, ids: &[String]) -> Result<BTreeMap<String, Placement<DocHome>>> {
        // One read per block for its own row (parent + page-ness); the ancestor
        // walk is then replaced by the top-down pass below.
        let mut parent_of: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut is_page: BTreeMap<String, bool> = BTreeMap::new();
        for id in ids {
            let uri = EntityUri::parse(id)?;
            self.locate_reads.fetch_add(1, AtomicOrdering::Relaxed);
            let Some(block) = self.reader.get_block_authoritative(&uri).await? else {
                continue;
            };
            let parent = if block.parent_id == EntityUri::no_parent() {
                None
            } else {
                Some(block.parent_id.as_str().to_string())
            };
            parent_of.insert(id.clone(), parent);
            is_page.insert(id.clone(), block.is_page());
        }

        // Resolve each block's document by memoized upward walk over the
        // in-memory parent map. Blocks whose chain leaves the snapshot fall
        // back to an authoritative walk.
        let mut doc_of: BTreeMap<String, DocHome> = BTreeMap::new();
        for id in parent_of.keys() {
            if doc_of.contains_key(id) {
                continue;
            }
            let mut chain: Vec<String> = Vec::new();
            let mut cur = id.clone();
            let resolved = loop {
                if let Some(d) = doc_of.get(&cur) {
                    break d.clone();
                }
                if is_page.get(&cur).copied().unwrap_or(false) {
                    break DocHome::Resolved(EntityUri::parse(&cur)?);
                }
                let Some(parent) = parent_of.get(&cur) else {
                    // Chain left the snapshot — pay one authoritative walk.
                    break self.resolve_doc(&EntityUri::parse(&cur)?, None).await?;
                };
                let Some(parent) = parent.clone() else {
                    break DocHome::Unresolved;
                };
                chain.push(cur);
                cur = parent;
            };
            doc_of.insert(cur, resolved.clone());
            for c in chain {
                doc_of.insert(c, resolved.clone());
            }
        }

        let mut out = BTreeMap::new();
        for (id, parent) in parent_of {
            let doc = doc_of.get(&id).cloned().unwrap_or(DocHome::Unresolved);
            out.insert(id, Placement { doc, parent });
        }
        Ok(out)
    }
}
