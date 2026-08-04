//! Stateful `home_by` combinator over a [`LiveData`] change feed.
//!
//! # Why this exists
//!
//! The org write-back layer mirrors the block feed as `document -> ordered
//! blocks`. [`group_by`](super::group_by) already derives the *document* half
//! statefully, so a cross-document move retracts from the old document. The
//! *order* half is still hand-maintained in the file-sync controller, guarded
//! by a conjunction of independently-added checks. `home_by` folds both halves
//! into one declared combinator: its accumulator is
//! `block id -> (owning document, previous-sibling id)`, so a consumer renders
//! what the holder says and takes no structural decisions of its own.
//!
//! # Order is a previous-sibling id, read from the authority
//!
//! The domain `Block` carries no `sort_key` (ADR 0005) and the feed lags the
//! write authority, so an order carried *on the value* would be stale for the
//! same reason a document carried on the value is. Both halves of [`Home`] are
//! therefore **authority reads** ([`HomeAuthority`]), and the feed keeps its
//! current shape — no schema, matview, or ADR-0005 change.
//!
//! A previous-sibling id is a linked list, and a move rewrites the pointers of
//! neighbours that emit no event of their own (the authority rewrites only the
//! moved row's `sort_key`). The list is therefore **never** maintained by
//! incremental pointer math: on a structural event the affected sibling
//! group(s) are re-derived from one [`children_of`](HomeAuthority::children_of)
//! read, which is a total order and so cannot fork or cycle within one read.
//!
//! # Contract (MatView = View)
//!
//! Folding all emitted [`HomedDiff`]s (Upsert inserts/repositions, Remove
//! deletes) equals the authority's current `document -> blocks` grouping with
//! each block's previous sibling. This holds after **every** event, not just at
//! the end.
//!
//! # Error semantics
//!
//! Authority reads are fallible. An error is surfaced as an `Err` stream item
//! and then the stream **ends** — we never skip an element silently, and never
//! keep folding on an accumulator we could not update. Loud-and-stop over
//! silent-and-wrong.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::Stream;
use futures_signals::signal_map::MapDiff;
use futures_signals::signal_map::SignalMapExt as _;

use super::LiveData;

/// Where the authority currently homes one block: the document that owns it,
/// and the sibling it immediately follows.
///
/// `prev` is **document-relative**: it names the nearest preceding sibling that
/// belongs to the *same* document, skipping siblings that own their own
/// document (a child page is de-inlined into its own file, so it does not
/// separate its neighbours in the parent's file). `None` means "first in this
/// document under this parent". A pointer that left the document would be
/// unresolvable by a per-document consumer, which holds only that document's
/// blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home<K> {
    pub doc: K,
    pub prev: Option<String>,
}

/// Where the authority says a block sits structurally: its owning document and
/// its parent.
///
/// Deliberately carries no previous-sibling: `prev` is **document-relative**
/// (see [`Home`]) and so cannot be read one block at a time — it depends on
/// which of the block's siblings belong to the same document. The combinator
/// derives it from an ordered sibling read instead.
///
/// `parent` is combinator-internal — it selects which group to re-derive when a
/// neighbour moves — and never reaches [`HomedDiff`]: a consumer reads a
/// block's parent off the value's own parent field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement<K> {
    pub doc: K,
    pub parent: Option<String>,
}

/// The authoritative structure `home_by` derives [`Home`]s from.
///
/// In production these map onto reads that already exist: `locate` is the
/// `resolve_doc_for_block` ancestor walk plus `BlockOrdering::prev_sibling`,
/// `children_of` is `BlockOrdering::children`, and `subtree_of` is a
/// document-scoped subtree read.
#[async_trait]
pub trait HomeAuthority<K: Send + 'static>: Send + Sync {
    /// Where `id` currently sits, or `None` if the authority no longer has it.
    async fn locate(&self, id: &str) -> Result<Option<Placement<K>>>;

    /// `parent`'s children in authoritative sibling order.
    async fn children_of(&self, parent: Option<&str>) -> Result<Vec<String>>;

    /// The sibling immediately preceding `id` in the authority's order,
    /// ignoring documents.
    ///
    /// This is the *movement detector*, not the emitted order: a block whose
    /// document, parent and tree-relative predecessor are all unchanged cannot
    /// have moved, so a content-only edit answers with one point read instead
    /// of an ordered group read. It is deliberately not enough to emit from —
    /// [`Home::prev`] is document-relative.
    async fn prev_sibling(&self, id: &str) -> Result<Option<String>>;

    /// Every descendant of `id`, excluding `id` itself.
    async fn subtree_of(&self, id: &str) -> Result<Vec<String>>;

    /// Locate a whole snapshot at once.
    ///
    /// The default loops [`locate`](Self::locate), which is correct but costs
    /// two point reads per block — the cold-boot cost that per-block boot work
    /// is already known to destabilise. An implementation is expected to
    /// override it and amortize: parent and page-ness are domain fields of the
    /// values the caller already holds, so a real implementation needs only one
    /// `children_of` per *distinct parent* for order, plus one top-down walk
    /// carrying the nearest enclosing page down for documents.
    async fn locate_batch(&self, ids: &[String]) -> Result<BTreeMap<String, Placement<K>>> {
        let mut out = BTreeMap::new();
        for id in ids {
            if let Some(p) = self.locate(id).await? {
                out.insert(id.clone(), p);
            }
        }
        Ok(out)
    }
}

/// A single retraction/addition emitted by [`LiveData::home_by`].
///
/// A document *change* for one element is expressed as two items — a `Remove`
/// at the old document immediately followed by an `Upsert` at the new one
/// (a retraction never lands after the addition for the same element).
/// An `Upsert` with an unchanged document is a **reposition**: the block's
/// previous sibling moved, which is how a neighbour that emitted no feed event
/// of its own gets corrected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomedDiff<K, T> {
    Upsert {
        doc: K,
        key: String,
        prev: Option<String>,
        value: Arc<T>,
    },
    /// The element `key` no longer belongs to `doc`. Carries no value — the
    /// retained home is the only truth.
    Remove { doc: K, key: String },
}

impl<T: Clone + Send + Sync + 'static> LiveData<T> {
    /// Re-home this feed's keyed changelog by an authoritative
    /// `(document, previous-sibling)` derivation.
    ///
    /// The returned stream drains the feed's initial snapshot (delivered as a
    /// `MapDiff::Replace`) through [`HomeAuthority::locate_batch`], then emits
    /// the retractions/additions for each subsequent change. See the module
    /// docs for the convergence contract and error semantics.
    pub fn home_by<K, A>(&self, authority: Arc<A>) -> impl Stream<Item = Result<HomedDiff<K, T>>>
    where
        K: Clone + Ord + Send + 'static,
        A: HomeAuthority<K> + 'static,
    {
        let mut source = self.signal_map();
        let diffs = futures::stream::poll_fn(move |cx| source.poll_map_change_unpin(cx));
        home_diffs(diffs, authority)
    }
}

/// One accumulator slot: the last-emitted home, the sibling group it sits in,
/// and the value.
///
/// The value is retained because a neighbour or descendant that emits no feed
/// event of its own must still be re-emitted when its home changes, and the
/// emission carries a value. It is an `Arc` clone — one pointer per live block.
struct Entry<K, T> {
    home: Home<K>,
    parent: Option<String>,
    /// The tree-relative predecessor, kept only to detect movement (see
    /// [`HomeAuthority::prev_sibling`]). Never emitted.
    tree_prev: Option<String>,
    value: Arc<T>,
}

/// State threaded through the `unfold` that turns a `MapDiff` stream into a
/// `HomedDiff` stream.
struct HomeState<S, K, T, A> {
    source: S,
    /// block id -> its last-emitted home. Survives `Remove`/`Clear` (value
    /// gone, home retained) so a departure retracts from the right document,
    /// and survives a content edit so a *position* change stays observable.
    acc: BTreeMap<String, Entry<K, T>>,
    /// Ready-to-yield outputs from the current input event (one event can
    /// produce many — a re-home fans out over a subtree).
    pending: VecDeque<Result<HomedDiff<K, T>>>,
    authority: Arc<A>,
    /// Set once an authority error has been queued; after it drains we end.
    errored: bool,
}

/// Core combinator: fold a stream of `MapDiff<String, Arc<T>>` into a stream
/// of `Result<HomedDiff<K, T>>`. Split out from [`LiveData::home_by`] so it can
/// be driven at the `MapDiff` boundary directly.
fn home_diffs<S, K, T, A>(
    source: S,
    authority: Arc<A>,
) -> impl Stream<Item = Result<HomedDiff<K, T>>>
where
    S: Stream<Item = MapDiff<String, Arc<T>>>,
    K: Clone + Ord + Send + 'static,
    T: Send + Sync + 'static,
    A: HomeAuthority<K> + 'static,
{
    let state = HomeState {
        source: Box::pin(source),
        acc: BTreeMap::new(),
        pending: VecDeque::new(),
        authority,
        errored: false,
    };
    futures::stream::unfold(state, |mut st| async move {
        use futures::stream::StreamExt as _;
        loop {
            if let Some(item) = st.pending.pop_front() {
                return Some((item, st));
            }
            if st.errored {
                return None;
            }
            let diff = st.source.next().await?;
            if let Err(e) =
                process_diff(&mut st.acc, &mut st.pending, st.authority.as_ref(), diff).await
            {
                st.errored = true;
                st.pending.push_back(Err(e));
            }
        }
    })
}

/// Which members of a re-derived group get an `Upsert` even if their position
/// did not move.
#[derive(Clone, Copy)]
enum Emit<'a> {
    /// Only members whose previous-sibling actually changed.
    Changed,
    /// Those, plus this one — the element whose own event triggered the pass,
    /// which must be re-emitted because its *value* changed.
    One(&'a str),
    /// Everyone — used when documents were re-partitioned, so every member
    /// needs re-stating into whichever document now owns it.
    All,
}

/// Re-derive the document-relative previous-sibling of every live member of
/// `parent`'s group from one authoritative ordered read, queuing an `Upsert`
/// per [`Emit`].
///
/// Order is derived here rather than read per block because a move rewrites
/// only the moved row: the neighbours whose position shifted emit no feed
/// event of their own. A single ordered read is also internally total, so it
/// cannot fork or cycle the way incremental pointer-patching can.
///
/// The per-document cursor is what makes `prev` document-relative: a sibling
/// owning its own document is skipped over rather than separating its
/// neighbours. A member the feed has not delivered is skipped entirely — it is
/// in no holder, so it separates nothing.
async fn reassign_group<K, T, A>(
    acc: &mut BTreeMap<String, Entry<K, T>>,
    pending: &mut VecDeque<Result<HomedDiff<K, T>>>,
    authority: &A,
    parent: Option<&str>,
    emit: Emit<'_>,
) -> Result<()>
where
    K: Clone + Ord + Send + 'static,
    A: HomeAuthority<K>,
{
    let ordered = authority
        .children_of(parent)
        .await
        .with_context(|| format!("home_by children_of({parent:?}) failed"))?;
    let mut last_seen: BTreeMap<K, String> = BTreeMap::new();
    let mut prev_any: Option<String> = None;
    for id in &ordered {
        let Some(entry) = acc.get_mut(id) else {
            prev_any = Some(id.clone());
            continue;
        };
        let doc = entry.home.doc.clone();
        let new_prev = last_seen.get(&doc).cloned();
        let changed = entry.home.prev != new_prev;
        entry.home.prev = new_prev.clone();
        entry.tree_prev = prev_any.clone();
        prev_any = Some(id.clone());
        let should_emit = match emit {
            Emit::All => true,
            Emit::One(k) => changed || id == k,
            Emit::Changed => changed,
        };
        if should_emit {
            pending.push_back(Ok(HomedDiff::Upsert {
                doc: doc.clone(),
                key: id.clone(),
                prev: new_prev,
                value: entry.value.clone(),
            }));
        }
        last_seen.insert(doc, id.clone());
    }
    Ok(())
}

/// Re-home every live descendant of `root` whose owning document changed.
///
/// Invoked when `root` itself gained or lost page-ness. The descendants' own
/// rows did not change, so they emit no feed event — without this expansion
/// they would stay homed to the old document until each happened to change for
/// an unrelated reason. Because the accumulator *remembers* each descendant's
/// last document, this is a precise diff rather than a blanket reseed.
async fn rehome_subtree<K, T, A>(
    acc: &mut BTreeMap<String, Entry<K, T>>,
    pending: &mut VecDeque<Result<HomedDiff<K, T>>>,
    authority: &A,
    root: &str,
) -> Result<()>
where
    K: Clone + Ord + Send + 'static,
    A: HomeAuthority<K>,
{
    let descendants = authority
        .subtree_of(root)
        .await
        .with_context(|| format!("home_by subtree_of({root}) failed"))?;

    // Pass 1 — retract every descendant whose document changed, and record the
    // groups whose document composition shifted. All retractions precede every
    // addition pass 2 emits, which is Law 2 across the whole fan-out.
    let mut affected: std::collections::BTreeSet<Option<String>> =
        std::collections::BTreeSet::new();

    // One batched placement for the whole subtree, never a point read per
    // descendant: a per-descendant `locate` re-walks the ancestor chain for
    // every one of them, so an N-descendant reparent costs N*(1+depth) reads
    // instead of N. `locate_batch` resolves documents top-down in one pass.
    let live: Vec<String> = descendants
        .into_iter()
        .filter(|id| acc.contains_key(id))
        .collect();
    let placements = authority
        .locate_batch(&live)
        .await
        .with_context(|| format!("home_by locate_batch during subtree re-home of {root} failed"))?;
    for id in &live {
        let Some(placement) = placements.get(id).cloned() else {
            continue;
        };
        let entry = acc.get_mut(id).expect("filtered to present above");
        if entry.home.doc != placement.doc {
            pending.push_back(Ok(HomedDiff::Remove {
                doc: entry.home.doc.clone(),
                key: id.clone(),
            }));
            entry.home.doc = placement.doc;
            affected.insert(placement.parent.clone());
        }
        entry.parent = placement.parent;
    }

    // Pass 2 — re-derive each affected group. A document change shifts the
    // per-document cursor, so neighbours' previous-siblings move even though
    // the tree order did not.
    for parent in affected {
        reassign_group(acc, pending, authority, parent.as_deref(), Emit::All).await?;
    }
    Ok(())
}

/// Apply one `MapDiff` to the accumulator, queuing the resulting `HomedDiff`s.
/// Retractions are always queued before the matching addition.
async fn process_diff<K, T, A>(
    acc: &mut BTreeMap<String, Entry<K, T>>,
    pending: &mut VecDeque<Result<HomedDiff<K, T>>>,
    authority: &A,
    diff: MapDiff<String, Arc<T>>,
) -> Result<()>
where
    K: Clone + Ord + Send + 'static,
    A: HomeAuthority<K>,
{
    match diff {
        MapDiff::Replace { entries } => {
            // Law 3: retract every retained entry, then re-seed from the
            // snapshot, as one batch the consumer applies atomically.
            for (key, entry) in std::mem::take(acc) {
                pending.push_back(Ok(HomedDiff::Remove {
                    doc: entry.home.doc,
                    key,
                }));
            }
            let ids: Vec<String> = entries.iter().map(|(k, _)| k.clone()).collect();
            let placements = authority
                .locate_batch(&ids)
                .await
                .context("home_by locate_batch failed for the snapshot")?;
            let mut groups: std::collections::BTreeSet<Option<String>> =
                std::collections::BTreeSet::new();
            for (key, value) in entries {
                let placement = placements.get(&key).with_context(|| {
                    format!(
                        "home_by locate_batch omitted snapshot entry {key} — feed/authority desync"
                    )
                })?;
                acc.insert(
                    key.clone(),
                    Entry {
                        home: Home {
                            doc: placement.doc.clone(),
                            prev: None,
                        },
                        parent: placement.parent.clone(),
                        tree_prev: None,
                        value,
                    },
                );
                groups.insert(placement.parent.clone());
            }
            // One ordered read per distinct parent assigns every seeded block's
            // document-relative position, and emits the snapshot's additions.
            for parent in groups {
                reassign_group(acc, pending, authority, parent.as_deref(), Emit::All).await?;
            }
        }
        MapDiff::Insert { key, value } | MapDiff::Update { key, value } => {
            let placement = authority
                .locate(&key)
                .await
                .with_context(|| format!("home_by locate({key}) failed"))?
                .with_context(|| {
                    format!(
                        "home_by: feed has {key} but the authority does not — feed/authority desync"
                    )
                })?;

            let old = acc.get(&key).map(|e| {
                (
                    e.home.doc.clone(),
                    e.parent.clone(),
                    e.home.prev.clone(),
                    e.tree_prev.clone(),
                )
            });
            if let Some((old_doc, _, _, _)) = &old {
                if *old_doc != placement.doc {
                    // Law 1: the document changed, so retract from the old one
                    // FIRST — no observer sees the block in two documents.
                    pending.push_back(Ok(HomedDiff::Remove {
                        doc: old_doc.clone(),
                        key: key.clone(),
                    }));
                }
            }

            // Movement detector. A same-parent reorder changes neither the
            // document nor the parent — that is exactly the case a
            // `parent`/`tags` comparison is blind to — so the tree-relative
            // predecessor is compared too. All three unchanged proves the block
            // did not move, and a neighbour's move arrives as that neighbour's
            // own event, so the common content-only edit pays one point read
            // and no ordered group read.
            let tree_prev = authority
                .prev_sibling(&key)
                .await
                .with_context(|| format!("home_by prev_sibling({key}) failed"))?;
            let moved = match &old {
                None => true,
                Some((old_doc, old_parent, _, old_tree_prev)) => {
                    *old_doc != placement.doc
                        || *old_parent != placement.parent
                        || *old_tree_prev != tree_prev
                }
            };
            let carried_prev = old.as_ref().and_then(|(_, _, p, _)| p.clone());
            acc.insert(
                key.clone(),
                Entry {
                    home: Home {
                        doc: placement.doc.clone(),
                        prev: carried_prev,
                    },
                    parent: placement.parent.clone(),
                    tree_prev,
                    value,
                },
            );

            if moved {
                // The block may have left one sibling group and joined another;
                // both need re-deriving, and neighbours emit nothing themselves.
                if let Some((_, old_parent, _, _)) = &old {
                    if *old_parent != placement.parent {
                        reassign_group(
                            acc,
                            pending,
                            authority,
                            old_parent.as_deref(),
                            Emit::Changed,
                        )
                        .await?;
                    }
                }
                reassign_group(
                    acc,
                    pending,
                    authority,
                    placement.parent.as_deref(),
                    Emit::One(&key),
                )
                .await?;
            } else {
                let entry = acc.get(&key).expect("just inserted");
                pending.push_back(Ok(HomedDiff::Upsert {
                    doc: entry.home.doc.clone(),
                    key: key.clone(),
                    prev: entry.home.prev.clone(),
                    value: entry.value.clone(),
                }));
            }

            // A block's document is derived from its ancestor chain, so when
            // this block's document changes, every descendant's does too —
            // and their rows did not change, so they emit no event of their
            // own. The fan-out is gated on the document change ITSELF, never
            // on why it changed: page-ness flipping is only one cause, and a
            // plain cross-document reparent of a non-page block is another.
            if let Some((old_doc, _, _, _)) = &old {
                if *old_doc != placement.doc {
                    rehome_subtree(acc, pending, authority, &key).await?;
                }
            }
        }
        MapDiff::Remove { key } => {
            let entry = acc.remove(&key).with_context(|| {
                format!("home_by Remove for key {key} not in accumulator — feed/accumulator desync")
            })?;
            pending.push_back(Ok(HomedDiff::Remove {
                doc: entry.home.doc,
                key: key.clone(),
            }));
            // The departed block's successor now follows the block's own
            // predecessor; re-derive the group it left.
            reassign_group(
                acc,
                pending,
                authority,
                entry.parent.as_deref(),
                Emit::Changed,
            )
            .await?;
        }
        MapDiff::Clear {} => {
            for (key, entry) in std::mem::take(acc) {
                pending.push_back(Ok(HomedDiff::Remove {
                    doc: entry.home.doc,
                    key,
                }));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::task::Context;
    use std::task::Poll;

    use futures::stream::StreamExt as _;
    use futures_signals::signal_map::MapDiff;
    use proptest::prelude::*;

    use super::*;

    /// Synthetic feed value. Deliberately carries NOTHING structural: parent,
    /// page-ness and order live in the authority, which is exactly the
    /// combinator's view of a domain `Block` (whose `sort_key` is absent by
    /// ADR 0005 and whose feed copy lags).
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Item {
        content: u8,
    }

    fn arc(content: u8) -> Arc<Item> {
        Arc::new(Item { content })
    }

    const ROOT: &str = "p0";

    // ---- reference authority --------------------------------------------

    /// The authoritative tree the combinator reads. `live` is the feed's
    /// membership, kept separate because the authority legitimately knows
    /// about blocks the feed has not delivered.
    #[derive(Debug, Clone)]
    struct Tree {
        parent: BTreeMap<String, Option<String>>,
        is_page: BTreeSet<String>,
        order: BTreeMap<Option<String>, Vec<String>>,
        live: BTreeSet<String>,
    }

    impl Tree {
        fn new() -> Self {
            let mut t = Tree {
                parent: BTreeMap::new(),
                is_page: BTreeSet::new(),
                order: BTreeMap::new(),
                live: BTreeSet::new(),
            };
            t.parent.insert(ROOT.into(), None);
            t.is_page.insert(ROOT.into());
            t.order.insert(None, vec![ROOT.into()]);
            t.live.insert(ROOT.into());
            t
        }

        fn exists(&self, id: &str) -> bool {
            self.parent.contains_key(id)
        }

        fn resolve_doc(&self, id: &str) -> String {
            let mut cur = id.to_string();
            for _ in 0..64 {
                if self.is_page.contains(&cur) {
                    return cur;
                }
                match self.parent.get(&cur).and_then(|p| p.clone()) {
                    Some(p) => cur = p,
                    None => break,
                }
            }
            "unresolved".into()
        }

        fn prev_of(&self, id: &str) -> Option<String> {
            let parent = self.parent.get(id)?.clone();
            let sibs = self.order.get(&parent)?;
            let pos = sibs.iter().position(|s| s == id)?;
            if pos == 0 {
                None
            } else {
                Some(sibs[pos - 1].clone())
            }
        }

        fn descendants(&self, id: &str) -> Vec<String> {
            let mut out = Vec::new();
            let mut stack: Vec<String> = self
                .order
                .get(&Some(id.to_string()))
                .cloned()
                .unwrap_or_default();
            while let Some(n) = stack.pop() {
                stack.extend(
                    self.order
                        .get(&Some(n.clone()))
                        .cloned()
                        .unwrap_or_default(),
                );
                out.push(n);
            }
            out.sort();
            out
        }

        fn is_descendant_of(&self, id: &str, ancestor: &str) -> bool {
            let mut cur = id.to_string();
            for _ in 0..64 {
                match self.parent.get(&cur).and_then(|p| p.clone()) {
                    Some(p) if p == ancestor => return true,
                    Some(p) => cur = p,
                    None => return false,
                }
            }
            false
        }

        fn detach(&mut self, id: &str) {
            let parent = self.parent.get(id).and_then(|p| p.clone());
            if let Some(sibs) = self.order.get_mut(&parent) {
                sibs.retain(|s| s != id);
            }
        }

        /// Insert `id` under `parent` immediately after `after` (or first).
        fn attach(&mut self, id: &str, parent: Option<String>, after: Option<String>) {
            let sibs = self.order.entry(parent.clone()).or_default();
            let at = match after.and_then(|a| sibs.iter().position(|s| *s == a)) {
                Some(p) => p + 1,
                None => 0,
            };
            sibs.insert(at, id.to_string());
            self.parent.insert(id.to_string(), parent);
        }

        /// Global pre-order over the whole tree, which is the order a document
        /// render walks.
        fn preorder(&self) -> Vec<String> {
            fn walk(t: &Tree, parent: Option<String>, out: &mut Vec<String>) {
                for c in t.order.get(&parent).cloned().unwrap_or_default() {
                    out.push(c.clone());
                    walk(t, Some(c), out);
                }
            }
            let mut out = Vec::new();
            walk(self, None, &mut out);
            out
        }

        /// `document -> live blocks in document order`. The reference the
        /// combinator's emitted stream must fold to.
        fn naive_recompute(&self) -> BTreeMap<String, Vec<String>> {
            let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for id in self.preorder() {
                if !self.live.contains(&id) {
                    continue;
                }
                let doc = self.resolve_doc(&id);
                // Children-only convention: a page is not a member of the list
                // its own document exposes, matching `get_blocks`/`doc_blocks`.
                if doc == id {
                    continue;
                }
                out.entry(doc).or_default().push(id);
            }
            out
        }
    }

    #[derive(Default)]
    struct Calls {
        locate: std::sync::atomic::AtomicU64,
        children: std::sync::atomic::AtomicU64,
        subtree: std::sync::atomic::AtomicU64,
        batch: std::sync::atomic::AtomicU64,
        prev_sibling: std::sync::atomic::AtomicU64,
    }

    impl Calls {
        fn bump(c: &std::sync::atomic::AtomicU64) {
            c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn get(c: &std::sync::atomic::AtomicU64) -> u64 {
            c.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    struct TestAuthority {
        tree: Arc<Mutex<Tree>>,
        calls: Arc<Calls>,
    }

    impl TestAuthority {
        fn new(tree: Arc<Mutex<Tree>>) -> Self {
            Self {
                tree,
                calls: Arc::new(Calls::default()),
            }
        }
    }

    #[async_trait]
    impl HomeAuthority<String> for TestAuthority {
        async fn locate(&self, id: &str) -> Result<Option<Placement<String>>> {
            Calls::bump(&self.calls.locate);
            let t = self.tree.lock().unwrap();
            if !t.exists(id) {
                return Ok(None);
            }
            Ok(Some(Placement {
                doc: t.resolve_doc(id),
                parent: t.parent.get(id).and_then(|p| p.clone()),
            }))
        }

        async fn children_of(&self, parent: Option<&str>) -> Result<Vec<String>> {
            Calls::bump(&self.calls.children);
            let t = self.tree.lock().unwrap();
            Ok(t.order
                .get(&parent.map(|p| p.to_string()))
                .cloned()
                .unwrap_or_default())
        }

        async fn prev_sibling(&self, id: &str) -> Result<Option<String>> {
            Calls::bump(&self.calls.prev_sibling);
            let t = self.tree.lock().unwrap();
            Ok(t.prev_of(id))
        }

        async fn subtree_of(&self, id: &str) -> Result<Vec<String>> {
            Calls::bump(&self.calls.subtree);
            let t = self.tree.lock().unwrap();
            Ok(t.descendants(id))
        }

        /// Mirrors the production adapter's amortized shape: one batched pass,
        /// not a point read per id, so the benchmark measures what production
        /// will actually pay.
        async fn locate_batch(
            &self,
            ids: &[String],
        ) -> Result<BTreeMap<String, Placement<String>>> {
            Calls::bump(&self.calls.batch);
            let t = self.tree.lock().unwrap();
            let mut out = BTreeMap::new();
            for id in ids {
                if !t.exists(id) {
                    continue;
                }
                out.insert(
                    id.clone(),
                    Placement {
                        doc: t.resolve_doc(id),
                        parent: t.parent.get(id).and_then(|p| p.clone()),
                    },
                );
            }
            Ok(out)
        }
    }

    /// Drain a stream to quiescence with a no-op waker. The synthetic authority
    /// is immediately ready, so a `Pending` means "no more output from the
    /// inputs fed so far".
    fn drain<X>(s: &mut (impl Stream<Item = X> + Unpin + ?Sized)) -> Vec<X> {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut out = Vec::new();
        while let Poll::Ready(Some(x)) = s.poll_next_unpin(&mut cx) {
            out.push(x);
        }
        out
    }

    // ---- folding the emitted stream back into a holder -------------------

    type Folded = BTreeMap<String, BTreeMap<String, (Arc<Item>, Option<String>)>>;

    fn fold_emitted(folded: &mut Folded, diffs: Vec<Result<HomedDiff<String, Item>>>) {
        for d in diffs {
            match d.expect("no authority error expected in the convergence property") {
                HomedDiff::Upsert {
                    doc,
                    key,
                    prev,
                    value,
                } => {
                    folded.entry(doc).or_default().insert(key, (value, prev));
                }
                HomedDiff::Remove { doc, key } => {
                    if let Some(g) = folded.get_mut(&doc) {
                        g.remove(&key);
                        if g.is_empty() {
                            folded.remove(&doc);
                        }
                    }
                }
            }
        }
    }

    /// Order one sibling group by following its previous-sibling chain.
    /// A malformed chain (fork or cycle — what pointer-patching produces)
    /// terminates and appends the unreachable members sorted, so the property
    /// reports a mismatch instead of hanging.
    fn order_group(members: &[String], prev: &BTreeMap<String, Option<String>>) -> Vec<String> {
        let set: BTreeSet<&String> = members.iter().collect();
        let mut succ: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
        for m in members {
            let p = prev.get(m).cloned().flatten().filter(|p| set.contains(p));
            succ.entry(p).or_default().push(m.clone());
        }
        let mut out = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut cursor: Option<String> = None;
        loop {
            let Some(nexts) = succ.get(&cursor) else {
                break;
            };
            // A fork means two blocks claim the same predecessor; take the
            // lowest so the result is deterministic, and let the leftovers
            // surface as a mismatch.
            let Some(next) = nexts.iter().min().cloned() else {
                break;
            };
            if !seen.insert(next.clone()) {
                break;
            }
            out.push(next.clone());
            cursor = Some(next);
        }
        let mut leftovers: Vec<String> = members
            .iter()
            .filter(|m| !seen.contains(*m))
            .cloned()
            .collect();
        leftovers.sort();
        out.extend(leftovers);
        out
    }

    /// Reconstruct `document -> blocks in document order` from the folded
    /// holder. Parent comes from the tree, standing in for the domain
    /// `Block`'s own parent field that a real consumer holds; only the
    /// previous-sibling assignment under test comes from the emitted diffs.
    fn reconstruct(folded: &Folded, tree: &Tree) -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        for (doc, members) in folded {
            let keys: Vec<String> = members.keys().cloned().collect();
            let in_doc: BTreeSet<&String> = keys.iter().collect();
            let prev: BTreeMap<String, Option<String>> = members
                .iter()
                .map(|(k, (_, p))| (k.clone(), p.clone()))
                .collect();

            // Hang each block off its nearest ancestor that is ALSO in this
            // document. Collapsing straight to `None` when the immediate parent
            // is absent (the authority legitimately knows blocks the feed has
            // not delivered) would merge two unrelated sibling groups into one
            // chain, where both heads have `prev == None` and no order exists.
            let mut by_parent: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
            for k in &keys {
                let mut p = tree.parent.get(k).and_then(|p| p.clone());
                for _ in 0..64 {
                    match p {
                        Some(ref a) if !in_doc.contains(a) => {
                            p = tree.parent.get(a).and_then(|x| x.clone());
                        }
                        _ => break,
                    }
                }
                by_parent.entry(p).or_default().push(k.clone());
            }

            fn walk(
                parent: Option<String>,
                by_parent: &BTreeMap<Option<String>, Vec<String>>,
                prev: &BTreeMap<String, Option<String>>,
                out: &mut Vec<String>,
                depth: usize,
            ) {
                if depth > 32 {
                    return;
                }
                let Some(members) = by_parent.get(&parent) else {
                    return;
                };
                for m in order_group(members, prev) {
                    out.push(m.clone());
                    walk(Some(m), by_parent, prev, out, depth + 1);
                }
            }
            let mut ordered = Vec::new();
            walk(None, &by_parent, &prev, &mut ordered, 0);
            // Same children-only convention as `naive_recompute`: nest using
            // the root, then drop it from the exposed list.
            ordered.retain(|id| id != doc);
            out.insert(doc.clone(), ordered);
        }
        out
    }

    // ---- generator -------------------------------------------------------

    const BLOCKS: [&str; 5] = ["b1", "b2", "b3", "b4", "b5"];

    #[derive(Debug, Clone)]
    enum Op {
        /// Create `k` under `parent` after `after` if absent, else a pure
        /// content update (no structural change).
        Set {
            k: u8,
            parent: u8,
            after: u8,
            content: u8,
        },
        /// Move `k` under a different parent.
        Reparent {
            k: u8,
            parent: u8,
            after: u8,
        },
        /// Move `k` among its existing siblings — the case a `parent`/`tags`
        /// comparison cannot see.
        Reorder {
            k: u8,
            after: u8,
        },
        /// Toggle `k`'s page-ness, re-partitioning its whole subtree between
        /// documents while only `k`'s own row changes.
        PageToggle {
            k: u8,
        },
        Remove {
            k: u8,
        },
        Clear,
        Replace {
            keep: Vec<u8>,
        },
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            8 => (0u8..5, 0u8..6, 0u8..6, 0u8..4)
                .prop_map(|(k, parent, after, content)| Op::Set { k, parent, after, content }),
            4 => (0u8..5, 0u8..6, 0u8..6).prop_map(|(k, parent, after)| Op::Reparent { k, parent, after }),
            5 => (0u8..5, 0u8..6).prop_map(|(k, after)| Op::Reorder { k, after }),
            4 => (0u8..5).prop_map(|k| Op::PageToggle { k }),
            3 => (0u8..5).prop_map(|k| Op::Remove { k }),
            1 => prop::collection::vec(0u8..5, 0..4).prop_map(|keep| Op::Replace { keep }),
            1 => Just(Op::Clear),
        ]
    }

    fn bname(k: u8) -> String {
        BLOCKS[(k as usize) % BLOCKS.len()].to_string()
    }

    /// Candidate anchor: index 0 is the root page, 1..=5 are the blocks.
    fn anchor(i: u8) -> String {
        if i == 0 {
            ROOT.to_string()
        } else {
            bname(i - 1)
        }
    }

    /// Apply an op to the authority and return the feed diff it produces.
    ///
    /// The returned diff names ONLY the directly-changed block. Neighbours
    /// whose position shifted and descendants whose document changed emit
    /// nothing — that asymmetry is the whole point of the property.
    fn to_diff(tree: &mut Tree, op: &Op) -> Option<MapDiff<String, Arc<Item>>> {
        match op {
            Op::Set {
                k,
                parent,
                after,
                content,
            } => {
                let key = bname(*k);
                let value = arc(*content);
                if tree.exists(&key) {
                    // Re-delivering an existing block must not resurrect it
                    // above a parent the feed has since dropped — that breaks
                    // ancestor-closure just as attaching under a dead parent
                    // would. Live parent + the invariant ⇒ the whole chain.
                    let parent_live = match tree.parent.get(&key).and_then(|p| p.clone()) {
                        None => true,
                        Some(p) => tree.live.contains(&p),
                    };
                    if !parent_live {
                        return None;
                    }
                    tree.live.insert(key.clone());
                    return Some(MapDiff::Update { key, value });
                }
                let p = anchor(*parent);
                // Ancestor-closure: the feed mirrors every block, so a live
                // block always has live ancestors. Attaching under a parent the
                // feed has not delivered would model a state production cannot
                // reach, and leaves a document whose blocks have no common root
                // to order them against.
                if !tree.exists(&p) || !tree.live.contains(&p) {
                    return None;
                }
                let a = anchor(*after);
                let a = if tree.exists(&a)
                    && tree.parent.get(&a).and_then(|x| x.clone()) == Some(p.clone())
                {
                    Some(a)
                } else {
                    None
                };
                tree.attach(&key, Some(p), a);
                tree.live.insert(key.clone());
                Some(MapDiff::Insert { key, value })
            }
            Op::Reparent { k, parent, after } => {
                let key = bname(*k);
                let p = anchor(*parent);
                if !tree.exists(&key)
                    || !tree.live.contains(&key)
                    || !tree.exists(&p)
                    || !tree.live.contains(&p)
                {
                    return None;
                }
                // No cycles, and no self-parenting.
                if p == key || tree.is_descendant_of(&p, &key) {
                    return None;
                }
                if tree.parent.get(&key).and_then(|x| x.clone()) == Some(p.clone()) {
                    return None;
                }
                let a = anchor(*after);
                let a = if tree.exists(&a)
                    && tree.parent.get(&a).and_then(|x| x.clone()) == Some(p.clone())
                {
                    Some(a)
                } else {
                    None
                };
                tree.detach(&key);
                tree.attach(&key, Some(p), a);
                Some(MapDiff::Update { key, value: arc(0) })
            }
            Op::Reorder { k, after } => {
                let key = bname(*k);
                if !tree.exists(&key) || !tree.live.contains(&key) {
                    return None;
                }
                let parent = tree.parent.get(&key).and_then(|x| x.clone());
                let a = anchor(*after);
                if a == key || !tree.exists(&a) {
                    return None;
                }
                if tree.parent.get(&a).and_then(|x| x.clone()) != parent {
                    return None;
                }
                if tree.prev_of(&key) == Some(a.clone()) {
                    return None;
                }
                tree.detach(&key);
                tree.attach(&key, parent, Some(a));
                Some(MapDiff::Update { key, value: arc(1) })
            }
            Op::PageToggle { k } => {
                let key = bname(*k);
                if !tree.exists(&key) || !tree.live.contains(&key) {
                    return None;
                }
                if tree.is_page.contains(&key) {
                    tree.is_page.remove(&key);
                } else {
                    tree.is_page.insert(key.clone());
                }
                Some(MapDiff::Update { key, value: arc(2) })
            }
            Op::Remove { k } => {
                let key = bname(*k);
                if !tree.live.contains(&key) || !tree.exists(&key) {
                    return None;
                }
                // Only leaves, so the surviving structure stays well-formed.
                if !tree.descendants(&key).is_empty() {
                    return None;
                }
                tree.detach(&key);
                tree.parent.remove(&key);
                tree.is_page.remove(&key);
                tree.live.remove(&key);
                Some(MapDiff::Remove { key })
            }
            Op::Clear => {
                tree.live.clear();
                Some(MapDiff::Clear {})
            }
            Op::Replace { keep } => {
                let mut entries: BTreeMap<String, Arc<Item>> = BTreeMap::new();
                tree.live.clear();
                if tree.exists(ROOT) {
                    tree.live.insert(ROOT.into());
                    entries.insert(ROOT.into(), arc(9));
                }
                for k in keep {
                    let key = bname(*k);
                    if !tree.exists(&key) {
                        continue;
                    }
                    // Keep the snapshot ancestor-closed, as the real feed is.
                    let mut cur = Some(key.clone());
                    while let Some(id) = cur {
                        if !tree.live.insert(id.clone()) {
                            break;
                        }
                        entries.insert(id.clone(), arc(3));
                        cur = tree.parent.get(&id).and_then(|p| p.clone());
                    }
                }
                Some(MapDiff::Replace {
                    entries: entries.into_iter().collect(),
                })
            }
        }
    }

    // ---- the strawman ----------------------------------------------------

    /// The strawman's order derivation: the moved block's own position in its
    /// sibling group, tree-relative and read one block at a time. It neither
    /// skips siblings owning another document nor touches any neighbour.
    async fn naive_prev<A: HomeAuthority<String>>(
        authority: &A,
        id: &str,
        placement: &Placement<String>,
    ) -> Option<String> {
        let sibs = authority
            .children_of(placement.parent.as_deref())
            .await
            .unwrap();
        let pos = sibs.iter().position(|s| s == id)?;
        pos.checked_sub(1).map(|i| sibs[i].clone())
    }

    /// The teeth-proof baseline: today's behaviour lifted to `HomedDiff`.
    ///
    /// It keeps the document accumulator `group_by` already provides — so
    /// removals and a block's OWN document change retract correctly — and
    /// derives `prev` from the authority on that block's own event. What it
    /// does NOT do is the two mechanisms this combinator adds: re-deriving the
    /// affected sibling group, and re-homing a subtree when its ancestor's
    /// page-ness toggles. Keeping the already-solved retraction behaviour is
    /// deliberate: it makes every failure attributable to one of those two new
    /// mechanisms rather than to a class `group_by` already closed.
    fn home_diffs_strawman<S, T, A>(
        source: S,
        authority: Arc<A>,
    ) -> impl Stream<Item = Result<HomedDiff<String, T>>>
    where
        S: Stream<Item = MapDiff<String, Arc<T>>>,
        T: Send + Sync + 'static,
        A: HomeAuthority<String> + 'static,
    {
        struct St<S, T, A> {
            source: S,
            acc: BTreeMap<String, String>,
            pending: VecDeque<Result<HomedDiff<String, T>>>,
            authority: Arc<A>,
        }
        let st = St {
            source: Box::pin(source),
            acc: BTreeMap::new(),
            pending: VecDeque::new(),
            authority,
        };
        futures::stream::unfold(st, |mut st| async move {
            use futures::stream::StreamExt as _;
            loop {
                if let Some(item) = st.pending.pop_front() {
                    return Some((item, st));
                }
                let diff = st.source.next().await?;
                match diff {
                    MapDiff::Replace { entries } => {
                        for (key, doc) in std::mem::take(&mut st.acc) {
                            st.pending.push_back(Ok(HomedDiff::Remove { doc, key }));
                        }
                        for (key, value) in entries {
                            let p = st.authority.locate(&key).await.unwrap().unwrap();
                            let prev = naive_prev(st.authority.as_ref(), &key, &p).await;
                            st.acc.insert(key.clone(), p.doc.clone());
                            st.pending.push_back(Ok(HomedDiff::Upsert {
                                doc: p.doc,
                                key,
                                prev,
                                value,
                            }));
                        }
                    }
                    MapDiff::Insert { key, value } | MapDiff::Update { key, value } => {
                        let p = st.authority.locate(&key).await.unwrap().unwrap();
                        let prev = naive_prev(st.authority.as_ref(), &key, &p).await;
                        if let Some(old) = st.acc.get(&key) {
                            if *old != p.doc {
                                st.pending.push_back(Ok(HomedDiff::Remove {
                                    doc: old.clone(),
                                    key: key.clone(),
                                }));
                            }
                        }
                        st.acc.insert(key.clone(), p.doc.clone());
                        st.pending.push_back(Ok(HomedDiff::Upsert {
                            doc: p.doc,
                            key,
                            prev,
                            value,
                        }));
                    }
                    MapDiff::Remove { key } => {
                        let doc = st.acc.remove(&key).expect("strawman accumulator desync");
                        st.pending.push_back(Ok(HomedDiff::Remove { doc, key }));
                    }
                    MapDiff::Clear {} => {
                        for (key, doc) in std::mem::take(&mut st.acc) {
                            st.pending.push_back(Ok(HomedDiff::Remove { doc, key }));
                        }
                    }
                }
            }
        })
    }

    // ---- the property ----------------------------------------------------

    /// Which engine a property run drives.
    #[derive(Clone, Copy, PartialEq)]
    enum Engine {
        Real,
        Strawman,
    }

    /// Run the fold-equality property for one op sequence against one engine.
    ///
    /// After EVERY input event the folded emitted output must equal a naive
    /// recomputation from the authority. This single equality subsumes the
    /// three derived-data laws: a missing re-home leaves a block in the old
    /// document, a stale neighbour pointer reorders the document, and a torn
    /// re-seed blanks one — all surface as a mismatch at the step that caused
    /// them.
    fn run_convergence(ops: Vec<Op>, engine: Engine) -> std::result::Result<(), TestCaseError> {
        let tree = Arc::new(Mutex::new(Tree::new()));
        let authority = Arc::new(TestAuthority::new(tree.clone()));

        let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
        let mut real;
        let mut straw;
        let stream: &mut (dyn Stream<Item = Result<HomedDiff<String, Item>>> + Unpin) = match engine
        {
            Engine::Real => {
                real = Box::pin(home_diffs(rx, authority.clone()));
                &mut real
            }
            Engine::Strawman => {
                straw = Box::pin(home_diffs_strawman(rx, authority.clone()));
                &mut straw
            }
        };

        // Seed the feed with the root page so every step has a document.
        {
            let mut t = tree.lock().unwrap();
            t.live.insert(ROOT.into());
        }
        tx.unbounded_send(MapDiff::Insert {
            key: ROOT.into(),
            value: arc(0),
        })
        .expect("channel open");
        let mut folded: Folded = BTreeMap::new();
        fold_emitted(&mut folded, drain(stream));

        for op in &ops {
            let diff = {
                let mut t = tree.lock().unwrap();
                to_diff(&mut t, op)
            };
            let Some(diff) = diff else { continue };

            // The real feed mirrors every block, so a live block always has
            // live ancestors. A generator hole that breaks this produces a
            // document whose blocks share no root to order against, which
            // surfaces as a confusing order mismatch — fail loudly on the
            // model instead, so the next hole is diagnosed in one step.
            {
                let t = tree.lock().unwrap();
                for id in &t.live {
                    if let Some(p) = t.parent.get(id).and_then(|p| p.clone()) {
                        prop_assert!(
                            t.live.contains(&p),
                            "MODEL INVARIANT BROKEN: {} is live but its parent {} is not — the \
                             generator produced a feed state production cannot reach",
                            id,
                            p
                        );
                    }
                }
            }

            tx.unbounded_send(diff).expect("channel open");
            let step = drain(stream);

            // Law 2: for every element re-homed in this step, the retraction
            // must precede the addition.
            let mut removed_at: BTreeMap<String, usize> = BTreeMap::new();
            for (i, d) in step.iter().enumerate() {
                if let Ok(HomedDiff::Remove { key, .. }) = d {
                    removed_at.insert(key.clone(), i);
                }
            }
            for (i, d) in step.iter().enumerate() {
                if let Ok(HomedDiff::Upsert { key, .. }) = d {
                    if let Some(r) = removed_at.get(key) {
                        prop_assert!(
                            *r < i,
                            "Law 2 violated: Upsert for {} at {} precedes its Remove at {} — an \
                             observer would see the block in two documents at once",
                            key,
                            i,
                            r
                        );
                    }
                }
            }

            fold_emitted(&mut folded, step);

            let t = tree.lock().unwrap();
            let expected = t.naive_recompute();
            let actual = reconstruct(&folded, &t);
            let actual: BTreeMap<String, Vec<String>> =
                actual.into_iter().filter(|(_, v)| !v.is_empty()).collect();
            prop_assert_eq!(
                &actual,
                &expected,
                "MatView(fold of emitted HomedDiffs) != View(naive recompute from the authority) \
                 after op {:?}. A document retains a stale block (missing re-home), or a \
                 neighbour's previous-sibling was not re-derived (stale order).",
                op
            );
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        /// Incremental convergence holds after every event.
        #[test]
        fn prop_convergence(ops in prop::collection::vec(op_strategy(), 0..40)) {
            run_convergence(ops, Engine::Real)?;
        }
    }

    // ---- orphan-prefix convergence ---------------------------------------

    /// Deliver every live block in an ancestry-ignoring order, so a child can
    /// arrive before its parent while the AUTHORITY stays closed.
    ///
    /// Mid-flight the holder is legitimately ambiguous — a block whose parent
    /// has not arrived has no in-document ancestor to nest under, so two
    /// blocks can both be "first in their group" with nothing to order them
    /// against. The contract is convergence *at quiescence*, not equality at
    /// every intermediate step, so the equality is asserted once the feed has
    /// caught up with the authority.
    fn run_orphan_prefix(ops: Vec<Op>, seed: u64) -> std::result::Result<(), TestCaseError> {
        let tree = Arc::new(Mutex::new(Tree::new()));
        let authority = Arc::new(TestAuthority::new(tree.clone()));

        // Build the authority tree; the feed has seen nothing yet.
        {
            let mut t = tree.lock().unwrap();
            for op in &ops {
                let _ = to_diff(&mut t, op);
            }
        }

        let mut ids: Vec<String> = tree.lock().unwrap().live.iter().cloned().collect();
        ids.sort_by_key(|id| {
            let mut h = seed;
            for b in id.as_bytes() {
                h = h.wrapping_mul(1099511628211).wrapping_add(u64::from(*b));
            }
            h
        });

        let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
        let mut stream = Box::pin(home_diffs(rx, authority.clone()));
        let mut folded: Folded = BTreeMap::new();
        for id in &ids {
            tx.unbounded_send(MapDiff::Insert {
                key: id.clone(),
                value: arc(0),
            })
            .expect("channel open");
            fold_emitted(&mut folded, drain(&mut stream));
        }

        let t = tree.lock().unwrap();
        let expected = t.naive_recompute();
        let actual: BTreeMap<String, Vec<String>> = reconstruct(&folded, &t)
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
        prop_assert_eq!(
            &actual,
            &expected,
            "orphan-prefix delivery did not converge at quiescence: once the feed mirrors the \
             authority the holder must equal a naive recompute, whatever order blocks arrived in"
        );
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        /// A child delivered before its parent still converges once the feed
        /// catches up.
        #[test]
        fn prop_orphan_prefix_converges_at_quiescence(
            ops in prop::collection::vec(op_strategy(), 0..30),
            seed in any::<u64>(),
        ) {
            run_orphan_prefix(ops, seed)?;
        }
    }

    // ---- fan-out cost ----------------------------------------------------

    /// Authority-read cost of a cross-document reparent, which fans out over
    /// the moved block's whole subtree. Reported per descendant count so the
    /// growth law is visible; the absolute wall time here is against an
    /// in-memory authority and is NOT a backend latency figure — the read
    /// COUNT is the backend-independent quantity.
    #[test]
    fn reparent_fanout_cost() {
        for n in [10usize, 100, 1000] {
            let tree = Arc::new(Mutex::new(Tree::new()));
            let authority = Arc::new(TestAuthority::new(tree.clone()));
            let mut order = vec![ROOT.to_string(), "pageB".to_string(), "host".to_string()];
            {
                let mut t = tree.lock().unwrap();
                t.attach("pageB", Some(ROOT.into()), None);
                t.is_page.insert("pageB".into());
                t.live.insert("pageB".into());
                t.attach("host", Some(ROOT.into()), Some("pageB".into()));
                t.live.insert("host".into());
                let mut prev = None;
                for i in 0..n {
                    let id = format!("d{i}");
                    t.attach(&id, Some("host".into()), prev.clone());
                    t.live.insert(id.clone());
                    order.push(id.clone());
                    prev = Some(id);
                }
            }

            let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
            let mut stream = Box::pin(home_diffs(rx, authority.clone()));
            let mut folded: Folded = BTreeMap::new();
            for id in &order {
                tx.unbounded_send(MapDiff::Insert {
                    key: id.clone(),
                    value: arc(0),
                })
                .unwrap();
                fold_emitted(&mut folded, drain(&mut stream));
            }

            // Steady state reached; measure ONLY the reparent.
            authority
                .calls
                .locate
                .store(0, std::sync::atomic::Ordering::Relaxed);
            authority
                .calls
                .children
                .store(0, std::sync::atomic::Ordering::Relaxed);
            authority
                .calls
                .subtree
                .store(0, std::sync::atomic::Ordering::Relaxed);
            authority
                .calls
                .batch
                .store(0, std::sync::atomic::Ordering::Relaxed);
            authority
                .calls
                .prev_sibling
                .store(0, std::sync::atomic::Ordering::Relaxed);

            {
                let mut t = tree.lock().unwrap();
                t.detach("host");
                t.attach("host", Some("pageB".into()), None);
            }
            let started = std::time::Instant::now();
            tx.unbounded_send(MapDiff::Update {
                key: "host".into(),
                value: arc(1),
            })
            .unwrap();
            fold_emitted(&mut folded, drain(&mut stream));
            let elapsed = started.elapsed();

            let locate = Calls::get(&authority.calls.locate);
            let children = Calls::get(&authority.calls.children);
            let subtree = Calls::get(&authority.calls.subtree);
            let batch = Calls::get(&authority.calls.batch);
            let prev_sib = Calls::get(&authority.calls.prev_sibling);
            println!(
                "REPARENT_FANOUT n={n} locate={locate} locate_batch={batch} children={children} \
                 subtree={subtree} prev_sibling={prev_sib} total_calls={} in_memory_elapsed={:?}",
                locate + children + subtree + prev_sib + batch,
                elapsed
            );

            // The fan-out is correct as well as costly.
            let t = tree.lock().unwrap();
            let expected = t.naive_recompute();
            let actual: BTreeMap<String, Vec<String>> = reconstruct(&folded, &t)
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .collect();
            assert_eq!(
                actual, expected,
                "reparent fan-out must stay correct at n={n}"
            );
        }
    }

    // ---- teeth: the two defect classes, isolated -------------------------

    /// Drive one scenario through one engine and return the final
    /// `document -> ordered blocks` reconstruction.
    fn scenario(
        engine: Engine,
        ops: Vec<Op>,
    ) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
        let tree = Arc::new(Mutex::new(Tree::new()));
        let authority = Arc::new(TestAuthority::new(tree.clone()));
        let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
        let mut real;
        let mut straw;
        let stream: &mut (dyn Stream<Item = Result<HomedDiff<String, Item>>> + Unpin) = match engine
        {
            Engine::Real => {
                real = Box::pin(home_diffs(rx, authority.clone()));
                &mut real
            }
            Engine::Strawman => {
                straw = Box::pin(home_diffs_strawman(rx, authority.clone()));
                &mut straw
            }
        };
        tx.unbounded_send(MapDiff::Insert {
            key: ROOT.into(),
            value: arc(0),
        })
        .unwrap();
        let mut folded: Folded = BTreeMap::new();
        fold_emitted(&mut folded, drain(stream));
        for op in &ops {
            let diff = {
                let mut t = tree.lock().unwrap();
                to_diff(&mut t, op)
            };
            let Some(diff) = diff else { continue };
            tx.unbounded_send(diff).unwrap();
            fold_emitted(&mut folded, drain(stream));
        }
        let t = tree.lock().unwrap();
        // Same pruning as the property: under the children-only convention a
        // document whose only member was its own root exposes nothing, and an
        // absent entry and an empty one mean the same thing.
        let actual: BTreeMap<String, Vec<String>> = reconstruct(&folded, &t)
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
        (actual, t.naive_recompute())
    }

    /// Build: root page p0 -> b1 -> b2, then toggle b1 into a page. Only b1's
    /// row changes, so only b1 emits — b2's owning document silently moves
    /// from p0 to b1.
    fn page_toggle_ops() -> Vec<Op> {
        vec![
            Op::Set {
                k: 0,
                parent: 0,
                after: 0,
                content: 0,
            }, // b1 under p0
            Op::Set {
                k: 1,
                parent: 1,
                after: 0,
                content: 0,
            }, // b2 under b1
            Op::PageToggle { k: 0 }, // b1 becomes a page
        ]
    }

    /// Build siblings b1, b2, b3 under p0, then move b1 after b3. Only b1's
    /// row changes, so b2 and b3 keep stale previous-sibling pointers.
    fn reorder_ops() -> Vec<Op> {
        vec![
            Op::Set {
                k: 0,
                parent: 0,
                after: 0,
                content: 0,
            },
            Op::Set {
                k: 1,
                parent: 0,
                after: 1,
                content: 0,
            },
            Op::Set {
                k: 2,
                parent: 0,
                after: 2,
                content: 0,
            },
            Op::Reorder { k: 0, after: 3 },
        ]
    }

    /// RED SIGNATURE 1 against the strawman: an ancestor gaining page-ness
    /// leaves its descendants homed to the old document.
    #[test]
    fn real_rehomes_subtree_when_ancestor_becomes_a_page() {
        let (straw, expected) = scenario(Engine::Strawman, page_toggle_ops());
        assert_ne!(
            straw, expected,
            "strawman must FAIL to re-home the subtree — if it passes, the property has no \
             teeth for the ancestor-page-toggle class"
        );
        assert_eq!(
            straw.get("p0").map(|v| v.contains(&"b2".to_string())),
            Some(true),
            "strawman should leave b2 homed to the OLD document p0"
        );

        let (real, expected) = scenario(Engine::Real, page_toggle_ops());
        assert_eq!(
            real, expected,
            "real engine must re-home b2 from p0 to the newly-paged b1"
        );
    }

    /// RED SIGNATURE 2 against the strawman: a same-parent reorder leaves a
    /// neighbour's previous-sibling pointer stale, forking the sibling chain.
    #[test]
    fn real_rederives_sibling_group_after_a_same_parent_reorder() {
        let (straw, expected) = scenario(Engine::Strawman, reorder_ops());
        assert_ne!(
            straw, expected,
            "strawman must FAIL to re-derive the sibling group — if it passes, the property has \
             no teeth for the same-parent-reorder class"
        );

        let (real, expected) = scenario(Engine::Real, reorder_ops());
        assert_eq!(
            real, expected,
            "real engine must re-derive every neighbour's previous sibling from the authority"
        );
    }

    /// A departing block's successor closes the gap left behind.
    #[test]
    fn removal_redirects_the_successor_to_the_departed_blocks_predecessor() {
        let ops = vec![
            Op::Set {
                k: 0,
                parent: 0,
                after: 0,
                content: 0,
            },
            Op::Set {
                k: 1,
                parent: 0,
                after: 1,
                content: 0,
            },
            Op::Set {
                k: 2,
                parent: 0,
                after: 2,
                content: 0,
            },
            Op::Remove { k: 1 },
        ];
        let (real, expected) = scenario(Engine::Real, ops);
        assert_eq!(real, expected);
    }

    /// Build `p0 -> b5(page)` and `p0 -> b3 -> b1`, then reparent `b3` under
    /// the page `b5`. Only `b3`'s row changes, but `b1`'s owning document moves
    /// with it — and `b3` itself never gained or lost page-ness.
    fn reparent_into_page_ops() -> Vec<Op> {
        vec![
            Op::Set {
                k: 2,
                parent: 0,
                after: 0,
                content: 0,
            },
            Op::Set {
                k: 4,
                parent: 0,
                after: 0,
                content: 0,
            },
            Op::PageToggle { k: 4 },
            Op::Set {
                k: 0,
                parent: 3,
                after: 0,
                content: 0,
            },
            Op::Reparent {
                k: 2,
                parent: 5,
                after: 0,
            },
        ]
    }

    /// The mirror: `b3 -> b1` starts inside the page `b5`'s document and is
    /// reparented back out to `p0`. The descendant must leave `b5` too, or it
    /// lingers there and corrupts that document's order.
    fn reparent_out_of_page_ops() -> Vec<Op> {
        vec![
            Op::Set {
                k: 4,
                parent: 0,
                after: 0,
                content: 0,
            },
            Op::PageToggle { k: 4 },
            Op::Set {
                k: 2,
                parent: 5,
                after: 0,
                content: 0,
            },
            Op::Set {
                k: 0,
                parent: 3,
                after: 0,
                content: 0,
            },
            Op::Reparent {
                k: 2,
                parent: 0,
                after: 0,
            },
        ]
    }

    /// A cross-document reparent re-homes the moved block's whole subtree.
    /// Page-ness flipping is only one way a document changes; a plain reparent
    /// of a non-page block does it too, and the descendants emit no event.
    #[test]
    fn reparent_into_a_page_subtree_rehomes_descendants() {
        let (real, expected) = scenario(Engine::Real, reparent_into_page_ops());
        assert_eq!(
            real, expected,
            "b1 must follow b3 into b5's document — the fan-out cannot be gated on page-ness"
        );
    }

    #[test]
    fn reparent_out_of_a_page_subtree_rehomes_descendants() {
        let (real, expected) = scenario(Engine::Real, reparent_out_of_page_ops());
        assert_eq!(
            real, expected,
            "b1 must leave b5's document with b3, or it lingers and corrupts that document"
        );
    }

    /// The snapshot re-seed swaps atomically: every retained entry retracts
    /// before the snapshot's own additions land.
    #[test]
    fn replace_retracts_every_retained_entry_before_reseeding() {
        let ops = vec![
            Op::Set {
                k: 0,
                parent: 0,
                after: 0,
                content: 0,
            },
            Op::Set {
                k: 1,
                parent: 0,
                after: 1,
                content: 0,
            },
            Op::Replace { keep: vec![1] },
        ];
        let (real, expected) = scenario(Engine::Real, ops);
        assert_eq!(real, expected);
    }

    // ---- DI-level supervision (Inc 2 prerequisite) -----------------------
    //
    // The combinator's terminal latch is correct and stays. These tests pin
    // BOTH halves of the let-it-die ruling: unsupervised, a dead stream is
    // permanently dead (and that is what the latch is *for*); supervised, the
    // DI seam rebuilds it and the fold-equality contract holds again after the
    // restart, because a fresh subscription re-seeds through the same
    // `MapDiff::Replace` boot path.
    mod supervision {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::AtomicI64;
        use std::sync::atomic::Ordering as AtomicOrdering;

        use tokio::sync::mpsc::UnboundedReceiver;

        use super::*;
        use crate::StorageEntity;
        use crate::Value;
        use crate::live_data::LiveData;
        use crate::live_data::supervision::MAX_RESTARTS_IN_WINDOW;
        use crate::live_data::supervision::Supervised;
        use crate::live_data::supervision::run_supervised;
        use crate::streaming::Change;
        use crate::streaming::ChangeOrigin;

        /// The reference authority behind an injectable fault gate.
        ///
        /// Every authority method passes the same gate, so a failure can be
        /// armed mid-sequence without knowing which read the combinator will
        /// issue next — which is the point: production faults are not
        /// method-selective either.
        struct FlakyAuthority {
            inner: TestAuthority,
            /// Calls still served normally before failing starts.
            healthy_calls: AtomicI64,
            /// Calls that fail once `healthy_calls` is spent.
            failures_left: AtomicI64,
        }

        impl FlakyAuthority {
            fn new(tree: Arc<Mutex<Tree>>) -> Self {
                Self {
                    inner: TestAuthority::new(tree),
                    healthy_calls: AtomicI64::new(i64::MAX),
                    failures_left: AtomicI64::new(0),
                }
            }

            /// Serve `healthy` more calls, then fail the next `failures`.
            fn arm(&self, healthy: i64, failures: i64) {
                self.healthy_calls.store(healthy, AtomicOrdering::Relaxed);
                self.failures_left.store(failures, AtomicOrdering::Relaxed);
            }

            fn gate(&self) -> Result<()> {
                if self.healthy_calls.fetch_sub(1, AtomicOrdering::Relaxed) > 0 {
                    return Ok(());
                }
                if self.failures_left.fetch_sub(1, AtomicOrdering::Relaxed) > 0 {
                    anyhow::bail!("injected authority fault");
                }
                Ok(())
            }
        }

        #[async_trait]
        impl HomeAuthority<String> for FlakyAuthority {
            async fn locate(&self, id: &str) -> Result<Option<Placement<String>>> {
                self.gate()?;
                self.inner.locate(id).await
            }
            async fn children_of(&self, parent: Option<&str>) -> Result<Vec<String>> {
                self.gate()?;
                self.inner.children_of(parent).await
            }
            async fn prev_sibling(&self, id: &str) -> Result<Option<String>> {
                self.gate()?;
                self.inner.prev_sibling(id).await
            }
            async fn subtree_of(&self, id: &str) -> Result<Vec<String>> {
                self.gate()?;
                self.inner.subtree_of(id).await
            }
            async fn locate_batch(
                &self,
                ids: &[String],
            ) -> Result<BTreeMap<String, Placement<String>>> {
                self.gate()?;
                self.inner.locate_batch(ids).await
            }
        }

        // ---- a real feed, so the restart re-seeds the way production does --

        fn test_feed() -> Arc<LiveData<Item>> {
            LiveData::new(
                vec![],
                |r: &StorageEntity| match r.get("id") {
                    Some(Value::String(s)) => Ok(s.clone()),
                    other => anyhow::bail!("test row has no string id: {other:?}"),
                },
                |r: &StorageEntity| match r.get("content") {
                    Some(Value::Integer(i)) => Ok(Item { content: *i as u8 }),
                    other => anyhow::bail!("test row has no integer content: {other:?}"),
                },
            )
        }

        fn feed_remove(feed: &LiveData<Item>, key: &str) {
            feed.apply_changes(vec![Change::Deleted {
                id: key.to_string(),
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            }]);
        }

        /// Mirror one generated `MapDiff` into the real feed.
        ///
        /// `Replace`/`Clear` are expressed as removals plus inserts — the feed
        /// exposes no bulk swap, and the resulting *state* is identical. The
        /// atomicity of a real `Replace` is pinned separately by
        /// `replace_retracts_every_retained_entry_before_reseeding`.
        fn apply_to_feed(feed: &LiveData<Item>, diff: MapDiff<String, Arc<Item>>) {
            let live: Vec<String> = feed.read().keys().cloned().collect();
            match diff {
                MapDiff::Insert { key, value } | MapDiff::Update { key, value } => {
                    feed.insert(key, value)
                }
                MapDiff::Remove { key } => feed_remove(feed, &key),
                MapDiff::Clear {} => {
                    for k in live {
                        feed_remove(feed, &k);
                    }
                }
                MapDiff::Replace { entries } => {
                    let keep: BTreeSet<String> = entries.iter().map(|(k, _)| k.clone()).collect();
                    for k in live {
                        if !keep.contains(&k) {
                            feed_remove(feed, &k);
                        }
                    }
                    for (k, v) in entries {
                        feed.insert(k, v);
                    }
                }
            }
        }

        /// Drive the supervisor future as far as it can go synchronously.
        ///
        /// The synthetic authority and the unbounded channel are always ready,
        /// so one poll drains everything the feed currently holds and returns
        /// `Pending` exactly when the source is empty. `true` means the
        /// supervisor finished — it gave up, or the source is gone.
        fn pump<F: Future<Output = ()>>(fut: &mut Pin<Box<F>>) -> bool {
            let waker = futures::task::noop_waker();
            let mut cx = Context::from_waker(&waker);
            fut.as_mut().poll(&mut cx).is_ready()
        }

        type Emission = Supervised<HomedDiff<String, Item>>;

        fn take(rx: &mut UnboundedReceiver<Emission>) -> Vec<Emission> {
            let mut out = Vec::new();
            while let Ok(item) = rx.try_recv() {
                out.push(item);
            }
            out
        }

        /// Fold supervised emissions, honouring `Reset` as "drop all derived
        /// state" — the consumer contract the DI seam implements.
        fn fold_supervised(folded: &mut Folded, items: Vec<Emission>) -> usize {
            let mut resets = 0;
            for item in items {
                match item {
                    Supervised::Reset => {
                        resets += 1;
                        folded.clear();
                    }
                    Supervised::Diff(d) => fold_emitted(folded, vec![Ok(d)]),
                }
            }
            resets
        }

        fn reference(tree: &Arc<Mutex<Tree>>) -> BTreeMap<String, Vec<String>> {
            tree.lock().unwrap().naive_recompute()
        }

        fn observed(folded: &Folded, tree: &Arc<Mutex<Tree>>) -> BTreeMap<String, Vec<String>> {
            let t = tree.lock().unwrap();
            reconstruct(folded, &t)
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .collect()
        }

        /// Seed the root page into both the model and the feed.
        fn seed_root(tree: &Arc<Mutex<Tree>>, feed: &LiveData<Item>) {
            tree.lock().unwrap().live.insert(ROOT.into());
            feed.insert(ROOT.into(), arc(0));
        }

        fn step(tree: &Arc<Mutex<Tree>>, feed: &LiveData<Item>, op: &Op) {
            let diff = {
                let mut t = tree.lock().unwrap();
                to_diff(&mut t, op)
            };
            if let Some(diff) = diff {
                apply_to_feed(feed, diff);
            }
        }

        // ---- (b) the red premise: unsupervised, death is terminal ---------

        /// Without supervision an authority fault kills the derived stream for
        /// good: the `Err` surfaces, the stream ends, and every subsequent
        /// edit is silently absent from the fold.
        ///
        /// This is the latch working as designed — and exactly why recovery
        /// cannot live inside the combinator.
        #[test]
        fn unsupervised_stream_death_is_terminal() {
            let tree = Arc::new(Mutex::new(Tree::new()));
            let authority = Arc::new(FlakyAuthority::new(tree.clone()));
            let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
            let mut stream = Box::pin(home_diffs(rx, authority.clone()));

            tree.lock().unwrap().live.insert(ROOT.into());
            tx.unbounded_send(MapDiff::Insert {
                key: ROOT.into(),
                value: arc(0),
            })
            .unwrap();
            let mut folded: Folded = BTreeMap::new();
            fold_emitted(&mut folded, drain(&mut stream));

            let before = vec![Op::Set {
                k: 0,
                parent: 0,
                after: 0,
                content: 0,
            }];
            for op in &before {
                let diff = to_diff(&mut tree.lock().unwrap(), op).expect("op applies");
                tx.unbounded_send(diff).unwrap();
                fold_emitted(&mut folded, drain(&mut stream));
            }
            assert_eq!(observed(&folded, &tree), reference(&tree));

            authority.arm(0, 1);
            let killer = Op::Set {
                k: 1,
                parent: 0,
                after: 1,
                content: 0,
            };
            let diff = to_diff(&mut tree.lock().unwrap(), &killer).expect("op applies");
            tx.unbounded_send(diff).unwrap();
            let dying = drain(&mut stream);
            assert!(
                dying.iter().any(|d| d.is_err()),
                "the authority fault must surface as an Err item, not be swallowed: {dying:?}"
            );

            // The stream is now *gone*, not merely idle: `drain` above consumed
            // its terminal `None`, which drops the accumulator and with it the
            // combinator's half of the feed. A post-mortem edit cannot even be
            // delivered — strictly stronger than "emits nothing further".
            let after = Op::Set {
                k: 2,
                parent: 0,
                after: 2,
                content: 0,
            };
            let diff = to_diff(&mut tree.lock().unwrap(), &after).expect("op applies");
            assert!(
                tx.unbounded_send(diff).is_err(),
                "the dead combinator must have dropped the feed subscription"
            );

            assert_ne!(
                observed(&folded, &tree),
                reference(&tree),
                "with no supervisor the holder is permanently behind the authority — this is \
                 the gap the DI supervisor closes"
            );
        }

        // ---- (c)+(d) the supervisor restarts and converges again ----------

        /// A mid-sequence authority fault kills the stream; the supervisor
        /// rebuilds it, the consumer drops its derived state on `Reset`, and
        /// the fold matches the authority again — including the edits that
        /// landed while the old stream was dying.
        #[test]
        fn supervisor_respawns_and_fold_equality_holds_after_restart() {
            let tree = Arc::new(Mutex::new(Tree::new()));
            let authority = Arc::new(FlakyAuthority::new(tree.clone()));
            let feed = test_feed();
            seed_root(&tree, &feed);

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Emission>();
            let stream_feed = feed.clone();
            let stream_authority = authority.clone();
            let mut supervisor = Box::pin(run_supervised(
                "test-home-by",
                move || stream_feed.home_by(stream_authority.clone()),
                tx,
                |_, _, _| panic!("must not give up on a single transient fault"),
            ));

            let mut folded: Folded = BTreeMap::new();
            assert!(!pump(&mut supervisor));
            let boot_resets = fold_supervised(&mut folded, take(&mut rx));
            assert_eq!(
                boot_resets, 1,
                "boot must go through the same Reset the restart does"
            );

            for op in &reorder_ops() {
                step(&tree, &feed, op);
                assert!(!pump(&mut supervisor));
                fold_supervised(&mut folded, take(&mut rx));
            }
            assert_eq!(observed(&folded, &tree), reference(&tree));

            // Kill it mid-sequence, then keep editing: the restart must pick up
            // the state as it stands, not as it stood at the moment of death.
            authority.arm(0, 1);
            step(
                &tree,
                &feed,
                &Op::Set {
                    k: 3,
                    parent: 0,
                    after: 3,
                    content: 1,
                },
            );
            step(&tree, &feed, &Op::PageToggle { k: 1 });
            assert!(!pump(&mut supervisor));
            let restarts = fold_supervised(&mut folded, take(&mut rx));

            assert_eq!(
                restarts, 1,
                "the dead stream must be respawned exactly once"
            );
            assert_eq!(
                observed(&folded, &tree),
                reference(&tree),
                "after the restart the fold must equal the authority again — the fresh \
                 subscription re-seeds through the same Replace the boot path uses"
            );

            // And it keeps converging afterwards: the restarted incarnation is
            // a full participant, not a one-shot snapshot.
            step(&tree, &feed, &Op::Reorder { k: 2, after: 0 });
            assert!(!pump(&mut supervisor));
            assert_eq!(fold_supervised(&mut folded, take(&mut rx)), 0);
            assert_eq!(observed(&folded, &tree), reference(&tree));
        }

        // ---- (e) bounded restarts, then the give-up seam ------------------

        /// An authority that never recovers must not be retried forever: after
        /// `MAX_RESTARTS_IN_WINDOW` restarts the supervisor stops and calls the
        /// give-up seam exactly once.
        #[test]
        fn persistent_failure_exhausts_the_restart_budget_and_escalates() {
            let tree = Arc::new(Mutex::new(Tree::new()));
            let authority = Arc::new(FlakyAuthority::new(tree.clone()));
            let feed = test_feed();
            seed_root(&tree, &feed);

            let gave_up = Arc::new(Mutex::new(Vec::<(u32, String)>::new()));
            let recorder = gave_up.clone();

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Emission>();
            let stream_feed = feed.clone();
            let stream_authority = authority.clone();
            let mut supervisor = Box::pin(run_supervised(
                "test-home-by",
                move || stream_feed.home_by(stream_authority.clone()),
                tx,
                move |_, restarts, err| {
                    recorder
                        .lock()
                        .unwrap()
                        .push((restarts, format!("{err:#}")));
                },
            ));

            // Every boot fails from here on.
            authority.arm(0, i64::MAX);
            assert!(
                pump(&mut supervisor),
                "a permanently failing authority must exhaust the budget and finish, not spin"
            );

            let calls = gave_up.lock().unwrap();
            assert_eq!(calls.len(), 1, "the give-up seam fires exactly once");
            assert_eq!(calls[0].0, MAX_RESTARTS_IN_WINDOW + 1);
            assert!(
                calls[0].1.contains("injected authority fault"),
                "the give-up seam must carry the underlying cause: {}",
                calls[0].1
            );

            // One Reset per incarnation: the boot plus each restart. After the
            // budget is spent nothing further is emitted.
            let resets = take(&mut rx)
                .into_iter()
                .filter(|e| matches!(e, Supervised::Reset))
                .count();
            assert_eq!(resets as u32, MAX_RESTARTS_IN_WINDOW + 1);
        }
    }
}
