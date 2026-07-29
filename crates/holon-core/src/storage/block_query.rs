//! Read-side abstraction over whatever substrate answers block queries.
//!
//! Turso (the `holon-turso` adapter, reading its CDC-driven `LiveData`
//! mirrors), an in-memory reference structure, and a Loro-native tree reader
//! all need to answer the same three questions the PBT invariants ask:
//!   1. a block by id,
//!   2. children / descendants in canonical sibling order (ADR 0005), and
//!   3. the navigation focus-roots.
//!
//! Keeping this in `holon-core` lets a wiring *without* Turso still satisfy
//! those queries — the seam ADR 0004 Phase 9 needs so the query substrate
//! becomes "one of four" rather than mandatory.
//!
//! ## Sync vs async — the bridge
//!
//! The *reads* are always against an in-memory view: Turso's `LiveData`
//! mirrors are in-memory, the Loro tree is in-memory, a reference structure is
//! in-memory. The only genuinely async concern is **waiting for CDC to settle**
//! before reading the Turso mirror. So the abstraction is split in two:
//!
//! * [`BlockQuery`] — a **synchronous** read surface over a consistent,
//!   point-in-time view. Tests, invariant bodies, and the Loro / in-memory
//!   implementations use this directly.
//! * [`BlockQuerySource`] — an **async** producer whose only job is the
//!   liveness concern: settle (for Turso, await CDC quiescence; for in-memory /
//!   Loro, a no-op) and then capture a [`BlockSnapshot`].
//!
//! [`BlockSnapshot`] is the one concrete, comparable value every backend
//! captures into, so an invariant reduces to `reference_snapshot ==
//! sut_snapshot` (it derives `PartialEq`).

use std::collections::HashMap;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::block::Block;

/// Fallible result with a thread-safe boxed error, matching the convention in
/// [`crate::traits`].
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A navigation focus-root row: `root_id` is the currently-open root block for
/// `region`. Mirrors the `(region, root_id)` projection of the Turso
/// `focus_roots` matview, but carries no Turso dependency.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FocusRoot {
    pub region: String,
    pub root_id: String,
}

impl FocusRoot {
    pub fn new(region: impl Into<String>, root_id: impl Into<String>) -> Self {
        Self {
            region: region.into(),
            root_id: root_id.into(),
        }
    }
}

/// Synchronous, point-in-time read surface over a consistent block view.
///
/// Sibling order is the single ordering primitive of the block domain (ADR
/// 0005); each implementor derives it from its own encoding (Loro fractional
/// index, SQL `ORDER BY sort_key`, in-memory child list) and the encoding never
/// leaks onto the returned [`Block`]. Reads are infallible against a captured
/// view: a missing block is `None`, not an error.
pub trait BlockQuery {
    /// A single block by its id, or `None` if absent.
    fn block_by_id(&self, id: &EntityUri) -> Option<Block>;

    /// Direct children of `parent`, in canonical sibling order (ADR 0005).
    fn children_ordered(&self, parent: &EntityUri) -> Vec<Block>;

    /// All descendants of `parent` (recursive) in pre-order: a parent appears
    /// immediately before its own subtree, siblings in canonical order.
    ///
    /// Default implementation is a recursive walk over
    /// [`children_ordered`](Self::children_ordered); implementors with a
    /// cheaper recursive query may override it.
    fn descendants_ordered(&self, parent: &EntityUri) -> Vec<Block> {
        let mut out = Vec::new();
        for child in self.children_ordered(parent) {
            let child_id = child.id.clone();
            out.push(child);
            out.extend(self.descendants_ordered(&child_id));
        }
        out
    }

    /// Current navigation focus-roots as `(region, root_id)` rows.
    fn focus_roots(&self) -> Vec<FocusRoot>;
}

/// A concrete, comparable, point-in-time capture of the block view.
///
/// Every backend captures into this one type, so cross-backend invariants
/// reduce to `==`. Built from blocks already in **canonical sibling order**
/// (the order-trusting model of ADR 0005 / Phase 8): the producer — a Turso
/// mirror read, a Loro tree walk, or a reference structure — is responsible for
/// emitting siblings in the right order; `BlockSnapshot` preserves it.
///
/// Equality is deliberately **producer-order-independent**: only the block set
/// (by id+content), the *per-parent* sibling order, and the focus-root set are
/// compared. So Turso (which sorts globally by `(parent_id, sort_key, id)`) and
/// a reference structure (which emits document order) produce equal snapshots
/// as long as each parent's children agree — the global emission order is not
/// part of the value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockSnapshot {
    /// id → block (order-independent; `HashMap` equality ignores insertion
    /// order).
    by_id: HashMap<EntityUri, Block>,
    /// parent id → child ids in canonical sibling order (per-parent order is
    /// the meaningful order and *is* compared).
    children: HashMap<EntityUri, Vec<EntityUri>>,
    /// Navigation focus-roots as a set (order-independent).
    focus_roots: std::collections::BTreeSet<FocusRoot>,
}

impl BlockSnapshot {
    /// Build a snapshot from blocks already in canonical sibling order plus the
    /// focus-roots. The input order is authoritative — children of a parent are
    /// returned in the order they appear in `blocks`.
    pub fn from_ordered(
        blocks: impl IntoIterator<Item = Block>,
        focus_roots: impl IntoIterator<Item = FocusRoot>,
    ) -> Self {
        let mut by_id = HashMap::new();
        let mut children: HashMap<EntityUri, Vec<EntityUri>> = HashMap::new();
        for block in blocks {
            children
                .entry(block.parent_id.clone())
                .or_default()
                .push(block.id.clone());
            by_id.insert(block.id.clone(), block);
        }
        Self {
            by_id,
            children,
            focus_roots: focus_roots.into_iter().collect(),
        }
    }

    /// Number of blocks captured.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the snapshot has no blocks.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Iterate all captured blocks (unordered — for diagnostics / set
    /// comparison).
    pub fn iter_blocks(&self) -> impl Iterator<Item = &Block> {
        self.by_id.values()
    }
}

impl BlockQuery for BlockSnapshot {
    fn block_by_id(&self, id: &EntityUri) -> Option<Block> {
        self.by_id.get(id).cloned()
    }

    fn children_ordered(&self, parent: &EntityUri) -> Vec<Block> {
        self.children
            .get(parent)
            .map(|ids| {
                ids.iter()
                    .filter_map(|cid| self.by_id.get(cid).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn focus_roots(&self) -> Vec<FocusRoot> {
        self.focus_roots.iter().cloned().collect()
    }
}

/// An async producer that bridges a live substrate to a sync [`BlockSnapshot`].
///
/// The single async method, [`snapshot`](Self::snapshot), is where the liveness
/// concern lives:
/// * the Turso adapter awaits CDC quiescence, then captures its `LiveData`
///   mirrors (`live_blocks` / `live_focus_roots`) into a [`BlockSnapshot`];
/// * an in-memory reference or a Loro reader does no awaiting and captures
///   synchronously (see [`from_sync`]).
///
/// After `snapshot()` returns, all reads are synchronous against the captured
/// [`BlockSnapshot`].
#[async_trait]
pub trait BlockQuerySource: Send + Sync {
    /// Settle (await CDC quiescence where applicable) and capture a consistent
    /// point-in-time [`BlockSnapshot`].
    async fn snapshot(&self) -> Result<BlockSnapshot>;

    /// A cheap monotone version of the underlying substrate, for pollers that
    /// want to skip a full [`snapshot`](Self::snapshot) while nothing has
    /// changed. Two calls returning the same `Some(v)` promise an unchanged
    /// snapshot.
    ///
    /// `None` — the default — means "no cheap signal here"; a caller must
    /// snapshot every time rather than assume anything.
    fn change_version(&self) -> Option<u64> {
        None
    }
}

/// Adapt any synchronous capture closure into a [`BlockQuerySource`].
///
/// This is the bridge for the in-memory reference and the Loro reader: they
/// already have a synchronous way to produce a [`BlockSnapshot`], so wrap it in
/// a source whose `snapshot()` simply calls it. No CDC, no awaiting.
///
/// ```ignore
/// let source = from_sync(|| Ok(reference.capture_block_snapshot()));
/// let snap = source.snapshot().await?; // resolves immediately
/// ```
pub fn from_sync<F>(capture: F) -> SyncBlockQuerySource<F>
where
    F: Fn() -> Result<BlockSnapshot> + Send + Sync,
{
    SyncBlockQuerySource { capture }
}

/// A [`BlockQuerySource`] backed by a synchronous capture closure. See
/// [`from_sync`].
pub struct SyncBlockQuerySource<F> {
    capture: F,
}

#[async_trait]
impl<F> BlockQuerySource for SyncBlockQuerySource<F>
where
    F: Fn() -> Result<BlockSnapshot> + Send + Sync,
{
    async fn snapshot(&self) -> Result<BlockSnapshot> {
        (self.capture)()
    }
}
