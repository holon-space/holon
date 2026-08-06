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
use holon_api::live_data::home_by::HomeAuthority;
use holon_api::live_data::home_by::Placement;
use holon_core::block_ordering::BlockOrdering;
use holon_filesystem::BlockReader;
use holon_filesystem::BlockRowMemo;
use holon_filesystem::MemoSeam;
use holon_filesystem::nearest_page_ancestor;

/// Everything one `home_by` fold burst has already asked the authority.
///
/// The fold writes to nothing these reads touch, so no entry can go stale
/// while the burst runs; the combinator owns this value on its own stack and
/// drops it at the burst boundary, which is what makes that argument
/// structural instead of a rule someone has to remember.
///
/// `doc_block_topology` is deliberately absent: it is the oracle the
/// write-back completeness gate audits the fold WITH, and an oracle served
/// from the thing it audits proves nothing.
#[derive(Default)]
pub struct HomeBurstMemo {
    /// Authoritative rows, shared with every parent-chain walk.
    rows: BlockRowMemo,
    /// `parent -> ordered children`. Also serves the `subtree_of` BFS.
    children: BTreeMap<EntityUri, Vec<EntityUri>>,
    prev_sibling: BTreeMap<EntityUri, Option<EntityUri>>,
    /// Resolved owning document per id — the shared ancestor suffix of every
    /// sibling walk in the burst.
    docs: BTreeMap<EntityUri, DocHome>,
}

impl HomeBurstMemo {
    /// A memo whose staleness seams are armed explicitly rather than from the
    /// environment. Test-only: production reads the environment, where both
    /// halves are off.
    pub fn with_seam(seam: MemoSeam) -> Self {
        Self {
            rows: BlockRowMemo::with_seam(seam),
            ..Default::default()
        }
    }

    fn dual_read(&self) -> bool {
        self.rows.seam().dual_read
    }
}

/// Fail loud when a memo hit disagrees with a read issued beside it. Reached
/// only under the test-only dual-read seam.
fn agree<T: PartialEq + std::fmt::Debug>(
    what: &str,
    key: &EntityUri,
    memo: &T,
    live: &T,
) -> Result<()> {
    if memo != live {
        anyhow::bail!(
            "burst memo served a stale {what} for {key}: memo says {memo:?}, the authority says \
             {live:?} — a write landed inside a burst the memo assumes is read-only"
        );
    }
    Ok(())
}

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
    /// The walk shares the burst's row memo, so a caller that already fetched
    /// `id` does not pay for it twice and sibling walks split the cost of the
    /// ancestor suffix they have in common.
    #[tracing::instrument(skip_all, name = "home.resolve_doc")]
    async fn resolve_doc(&self, id: &EntityUri, memo: &mut HomeBurstMemo) -> Result<DocHome> {
        if let Some(hit) = memo.docs.get(id).cloned() {
            if memo.dual_read() {
                // Re-walk against a memo of its own: a walk sharing the burst's
                // rows would re-derive the same wrong answer from the same
                // wrong row and agree with itself.
                let live = self
                    .walk_doc(id, &mut BlockRowMemo::with_seam(MemoSeam::default()), None)
                    .await?;
                agree("resolve_doc", id, &hit, &live)?;
            }
            return Ok(hit);
        }
        let doc = self
            .walk_doc(id, &mut memo.rows, Some(&self.locate_reads))
            .await?;
        memo.docs.insert(id.clone(), doc.clone());
        Ok(doc)
    }

    async fn walk_doc(
        &self,
        id: &EntityUri,
        rows: &mut BlockRowMemo,
        reads: Option<&AtomicU64>,
    ) -> Result<DocHome> {
        Ok(
            match nearest_page_ancestor(self.reader.as_ref(), id, rows, reads).await? {
                Some(page) => DocHome::Resolved(page.id),
                None => DocHome::Unresolved,
            },
        )
    }

    async fn read_prev_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        self.ordering
            .prev_sibling(id)
            .await
            .map_err(|e| anyhow::anyhow!("prev_sibling({id}) failed: {e}"))
    }

    async fn read_children(&self, parent: &EntityUri) -> Result<Vec<EntityUri>> {
        self.ordering
            .children(parent)
            .await
            .map_err(|e| anyhow::anyhow!("children({parent}) failed: {e}"))
    }

    async fn children_uris(
        &self,
        parent: &EntityUri,
        memo: &mut HomeBurstMemo,
    ) -> Result<Vec<EntityUri>> {
        if let Some(hit) = memo.children.get(parent).cloned() {
            if memo.dual_read() {
                agree("children", parent, &hit, &self.read_children(parent).await?)?;
            }
            return Ok(hit);
        }
        let live = self.read_children(parent).await?;
        memo.children.insert(parent.clone(), live.clone());
        Ok(live)
    }
}

#[async_trait]
impl HomeAuthority<DocHome> for BlockHomeAuthority {
    type Memo = HomeBurstMemo;

    #[tracing::instrument(skip_all, name = "home.locate")]
    async fn locate(
        &self,
        id: &str,
        memo: &mut HomeBurstMemo,
    ) -> Result<Option<Placement<DocHome>>> {
        let uri = EntityUri::parse(id)?;
        let Some(block) = memo
            .rows
            .get(self.reader.as_ref(), &uri, Some(&self.locate_reads))
            .await?
        else {
            return Ok(None);
        };
        // `no_parent` is the tree's root sentinel, expressed to the combinator
        // as "no parent" so the root group is keyed uniformly with `None`.
        let parent = if block.parent_id == EntityUri::no_parent() {
            None
        } else {
            Some(block.parent_id.as_str().to_string())
        };
        // The walk below starts at the row just read, which the memo now holds.
        let doc = self.resolve_doc(&uri, memo).await?;
        Ok(Some(Placement { doc, parent }))
    }

    #[tracing::instrument(skip_all, name = "home.children_of")]
    async fn children_of(
        &self,
        parent: Option<&str>,
        memo: &mut HomeBurstMemo,
    ) -> Result<Vec<String>> {
        let parent_uri = match parent {
            Some(p) => EntityUri::parse(p)?,
            None => EntityUri::no_parent(),
        };
        let kids = self.children_uris(&parent_uri, memo).await?;
        Ok(kids.into_iter().map(|k| k.as_str().to_string()).collect())
    }

    #[tracing::instrument(skip_all, name = "home.prev_sibling")]
    async fn prev_sibling(&self, id: &str, memo: &mut HomeBurstMemo) -> Result<Option<String>> {
        let uri = EntityUri::parse(id)?;
        if let Some(hit) = memo.prev_sibling.get(&uri).cloned() {
            if memo.dual_read() {
                agree(
                    "prev_sibling",
                    &uri,
                    &hit,
                    &self.read_prev_sibling(&uri).await?,
                )?;
            }
            return Ok(hit.map(|p| p.as_str().to_string()));
        }
        let prev = self.read_prev_sibling(&uri).await?;
        memo.prev_sibling.insert(uri, prev.clone());
        Ok(prev.map(|p| p.as_str().to_string()))
    }

    #[tracing::instrument(skip_all, name = "home.subtree_of")]
    async fn subtree_of(&self, id: &str, memo: &mut HomeBurstMemo) -> Result<Vec<String>> {
        // Breadth-first over `children`, which is the only ordered-descendant
        // read the seams expose. This is the O(subtree) cost a cross-document
        // reparent pays; see `reparent_fanout_cost` for the measurement.
        let root = EntityUri::parse(id)?;
        let mut out = Vec::new();
        let mut frontier = vec![root];
        while let Some(node) = frontier.pop() {
            for k in self.children_uris(&node, memo).await? {
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
    async fn locate_batch(
        &self,
        ids: &[String],
        memo: &mut HomeBurstMemo,
    ) -> Result<BTreeMap<String, Placement<DocHome>>> {
        // One read per block for its own row (parent + page-ness); the ancestor
        // walk is then replaced by the top-down pass below.
        let mut parent_of: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut is_page: BTreeMap<String, bool> = BTreeMap::new();
        for id in ids {
            let uri = EntityUri::parse(id)?;
            let Some(block) = memo
                .rows
                .get(self.reader.as_ref(), &uri, Some(&self.locate_reads))
                .await?
            else {
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
                    break self.resolve_doc(&EntityUri::parse(&cur)?, memo).await?;
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

        // The top-down pass resolved the same documents an ancestor walk would,
        // so the rest of the burst reads them from here instead of re-walking.
        for (id, doc) in &doc_of {
            memo.docs.insert(EntityUri::parse(id)?, doc.clone());
        }

        let mut out = BTreeMap::new();
        for (id, parent) in parent_of {
            let doc = doc_of.get(&id).cloned().unwrap_or(DocHome::Unresolved);
            out.insert(id, Placement { doc, parent });
        }
        Ok(out)
    }
}
