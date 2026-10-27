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
use std::path::PathBuf;
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

/// A durable format that can hold a [`Home`]'s document.
///
/// Holon's own store is the ABSENCE of a variant rather than one of them: a
/// home nothing outside Holon holds answers `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableFormat {
    Org,
}

impl DurableFormat {
    pub fn from_extension(extension: &str) -> Result<Self> {
        match extension {
            "org" => Ok(Self::Org),
            other => anyhow::bail!(
                "`.{other}` names no durable format Holon knows, so a home backed by it cannot \
                 be resolved"
            ),
        }
    }
}

/// A [`Home`]'s document key, read as the durable artifact holding it.
///
/// The key type varies by authority — a file path under the write-back
/// harness, a document uri under [`HomeAuthority`] — and only the key itself
/// knows what holds it.
pub trait HomeDoc {
    fn durable_format(&self) -> Result<Option<DurableFormat>>;
}

impl HomeDoc for Option<PathBuf> {
    fn durable_format(&self) -> Result<Option<DurableFormat>> {
        let Some(path) = self else {
            return Ok(None);
        };
        let extension = path.extension().and_then(|e| e.to_str()).with_context(|| {
            format!(
                "home file `{}` carries no readable extension",
                path.display()
            )
        })?;
        DurableFormat::from_extension(extension).map(Some)
    }
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
    /// Scratch space the combinator hands to every read of ONE fold burst and
    /// then drops.
    ///
    /// The fold mutates nothing the authority reads, so within a burst no
    /// answer given can become wrong — an implementation is free to serve a
    /// repeated lookup from here instead of re-reading. The combinator owns the
    /// value as a stack local of the burst frame and passes it as `&mut`, so an
    /// implementation cannot retain it past the burst and there is no
    /// invalidation rule to get wrong. `()` for an authority with nothing to
    /// memoize.
    type Memo: Default + Send;

    /// Where `id` currently sits, or `None` if the authority no longer has it.
    async fn locate(&self, id: &str, memo: &mut Self::Memo) -> Result<Option<Placement<K>>>;

    /// `parent`'s children in authoritative sibling order.
    async fn children_of(&self, parent: Option<&str>, memo: &mut Self::Memo)
    -> Result<Vec<String>>;

    /// The sibling immediately preceding `id` in the authority's order,
    /// ignoring documents.
    ///
    /// This is the *movement detector*, not the emitted order: a block whose
    /// document, parent and tree-relative predecessor are all unchanged cannot
    /// have moved, so a content-only edit answers with one point read instead
    /// of an ordered group read. It is deliberately not enough to emit from —
    /// [`Home::prev`] is document-relative.
    async fn prev_sibling(&self, id: &str, memo: &mut Self::Memo) -> Result<Option<String>>;

    /// Every descendant of `id`, excluding `id` itself.
    async fn subtree_of(&self, id: &str, memo: &mut Self::Memo) -> Result<Vec<String>>;

    /// Locate a whole snapshot at once.
    ///
    /// The default loops [`locate`](Self::locate), which is correct but costs
    /// two point reads per block — the cold-boot cost that per-block boot work
    /// is already known to destabilise. An implementation is expected to
    /// override it and amortize: parent and page-ness are domain fields of the
    /// values the caller already holds, so a real implementation needs only one
    /// `children_of` per *distinct parent* for order, plus one top-down walk
    /// carrying the nearest enclosing page down for documents.
    async fn locate_batch(
        &self,
        ids: &[String],
        memo: &mut Self::Memo,
    ) -> Result<BTreeMap<String, Placement<K>>> {
        let mut out = BTreeMap::new();
        for id in ids {
            if let Some(p) = self.locate(id, memo).await? {
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

/// Whether a `home_by` stream currently owes its consumer output.
///
/// The fold between a source diff arriving and the last item it produces being
/// yielded is `await`-heavy — every home is an authority read — and while it
/// runs NOTHING downstream has moved yet. A settle watching only the source
/// (the feed) and the sink (the write-back loop) reads that window as
/// quiescence and concludes too early, which is exactly what left a receiver's
/// org files empty in the two-instance slice (BugFunnel
/// `2026-09-02-two-instance-binary-is-red-on-main-and-in-no-gate`).
///
/// Shared with a stream via [`LiveData::home_by_with_progress`].
#[derive(Debug, Default)]
pub struct HomeByProgress {
    folding: std::sync::atomic::AtomicBool,
}

impl HomeByProgress {
    /// `true` between a source burst being taken and the last diff it produced
    /// being handed to the consumer.
    pub fn is_folding(&self) -> bool {
        self.folding.load(std::sync::atomic::Ordering::Acquire)
    }

    fn set(&self, folding: bool) {
        self.folding
            .store(folding, std::sync::atomic::Ordering::Release);
    }
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
        self.home_by_with_progress(authority, Arc::new(HomeByProgress::default()))
    }

    /// [`home_by`](Self::home_by), reporting mid-fold state into `progress` so
    /// a settle can tell "no output yet" apart from "nothing to output".
    pub fn home_by_with_progress<K, A>(
        &self,
        authority: Arc<A>,
        progress: Arc<HomeByProgress>,
    ) -> impl Stream<Item = Result<HomedDiff<K, T>>>
    where
        K: Clone + Ord + Send + 'static,
        A: HomeAuthority<K> + 'static,
    {
        let mut source = self.signal_map();
        let diffs = futures::stream::poll_fn(move |cx| source.poll_map_change_unpin(cx));
        home_diffs_with_progress(diffs, authority, progress)
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
    /// Set once the source has reported end-of-stream mid-burst; the burst
    /// still gets processed, then we end.
    source_done: bool,
    progress: Arc<HomeByProgress>,
}

/// A stream that stops being polled — dropped mid-fold, or abandoned by a
/// supervisor that has given up — must not leave its handle claiming work is in
/// flight. Nothing would ever lower it again.
impl<S, K, T, A> Drop for HomeState<S, K, T, A> {
    fn drop(&mut self) {
        self.progress.set(false);
    }
}

/// Core combinator: fold a stream of `MapDiff<String, Arc<T>>` into a stream
/// of `Result<HomedDiff<K, T>>`. Split out from [`LiveData::home_by`] so it can
/// be driven at the `MapDiff` boundary directly.
/// [`home_diffs_with_progress`] for a caller that observes only the stream.
#[cfg(test)]
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
    home_diffs_with_progress(source, authority, Arc::new(HomeByProgress::default()))
}

fn home_diffs_with_progress<S, K, T, A>(
    source: S,
    authority: Arc<A>,
    progress: Arc<HomeByProgress>,
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
        source_done: false,
        progress,
    };
    futures::stream::unfold(state, |mut st| async move {
        use futures::stream::StreamExt as _;
        loop {
            if let Some(item) = st.pending.pop_front() {
                return Some((item, st));
            }
            if st.errored || st.source_done {
                st.progress.set(false);
                return None;
            }
            // Every item of the last burst has been handed over and nothing is
            // queued — about to park on the source. This is the only point at
            // which the fold owes its consumer nothing.
            st.progress.set(false);

            // The burst: everything the source already holds when it wakes us.
            // A single op fans one diff per affected member, so the whole fan
            // is queued by the time the first one arrives — draining it here is
            // what gives the authority reads below a scope over which nothing
            // can change (the fold writes to nothing they read), and what lets
            // the downstream controller collapse the fan to one render per
            // document. A write that lands mid-burst is echoed by its own diff
            // in the NEXT burst, with a fresh memo: convergence at quiescence.
            let mut burst = vec![st.source.next().await?];
            // A diff is in hand. The authority reads below are the window this
            // flag exists for, so it must be raised BEFORE them: everything
            // downstream is idle for their whole duration.
            st.progress.set(true);
            loop {
                match futures::poll!(st.source.next()) {
                    std::task::Poll::Ready(Some(d)) => burst.push(d),
                    std::task::Poll::Ready(None) => {
                        st.source_done = true;
                        break;
                    }
                    std::task::Poll::Pending => break,
                }
            }

            let mut memo = A::Memo::default();
            for diff in burst {
                if let Err(e) = process_diff(
                    &mut st.acc,
                    &mut st.pending,
                    st.authority.as_ref(),
                    diff,
                    &mut memo,
                )
                .await
                {
                    // Lower BEFORE queueing the Err. A consumer under
                    // supervision may never poll this stream again — on the
                    // give-up path `run_supervised` breaks on the error it
                    // just took — so the `errored` branch at the top of the
                    // loop is not reachable, and a flag left raised there is
                    // raised for the life of the process.
                    st.progress.set(false);
                    st.errored = true;
                    st.pending.push_back(Err(e));
                    break;
                }
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
    memo: &mut A::Memo,
    parent: Option<&str>,
    emit: Emit<'_>,
) -> Result<()>
where
    K: Clone + Ord + Send + 'static,
    A: HomeAuthority<K>,
{
    let ordered = authority
        .children_of(parent, memo)
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
    memo: &mut A::Memo,
    root: &str,
) -> Result<()>
where
    K: Clone + Ord + Send + 'static,
    A: HomeAuthority<K>,
{
    let descendants = authority
        .subtree_of(root, memo)
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
        .locate_batch(&live, memo)
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
        reassign_group(acc, pending, authority, memo, parent.as_deref(), Emit::All).await?;
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
    memo: &mut A::Memo,
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
                .locate_batch(&ids, memo)
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
                reassign_group(acc, pending, authority, memo, parent.as_deref(), Emit::All).await?;
            }
        }
        MapDiff::Insert { key, value } | MapDiff::Update { key, value } => {
            let placement = authority
                .locate(&key, memo)
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
                .prev_sibling(&key, memo)
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
                            memo,
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
                    memo,
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
                    rehome_subtree(acc, pending, authority, memo, &key).await?;
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
                memo,
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
mod tests;
