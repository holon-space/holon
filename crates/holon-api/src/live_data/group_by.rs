//! Stateful `group_by` combinator over a [`LiveData`] change feed.
//!
//! # Why this exists
//!
//! Downstream consumers (e.g. the org write-back resolver in
//! `holon-orgmode`) re-group a `LiveData`'s keyed changelog by a *derived*
//! key — for a `Block`, the owning document, computed by an async
//! authoritative parent-walk. The historical resolver did this as a
//! **stateless** map over the diff stream: it grouped each diff by
//! `key_fn(new value)` only. That shape has a structural defect — when an
//! element's derived key *changes* (a block moves to another document), the
//! **old** group never receives a retraction, so a source org file keeps a
//! departed block forever (the cross-doc-move source-convergence defect).
//!
//! The fix belongs in the signal layer as a *named, reusable, stateful*
//! combinator: it keeps a per-element accumulator (element key -> last emitted
//! group key) so a group-key change emits a `Remove` at the old group **and**
//! an `Upsert` at the new one, and a `Remove`/`Clear` retracts from the
//! *retained* group even though the value is already gone.
//!
//! # Contract (MatView = View)
//!
//! For every group `g`, folding all emitted [`GroupedDiff`]s for `g`
//! (Upsert inserts, Remove deletes) equals the grouping of the current map
//! state by `key_fn`. This holds after **every** event, not just at the end
//! (`O(delta)` maintenance; the only full rescans are on `Replace`/`Clear`).
//!
//! # Error semantics
//!
//! `key_fn` is async **and fallible** (the real key fn is an authoritative
//! `BlockReader` parent-walk returning `Result`). A `key_fn` error is
//! surfaced as an `Err` stream item and then the stream **ends** — we never
//! skip an element silently, and we never continue folding on top of an
//! accumulator we could not update (which would silently corrupt every later
//! group). Loud-and-stop over silent-and-wrong.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use futures::stream::Stream;
use futures_signals::signal_map::MapDiff;
use futures_signals::signal_map::SignalMapExt as _;

use super::LiveData;

/// A single retraction/addition emitted by [`LiveData::group_by`].
///
/// A group-key *change* for one element is expressed as two items — a
/// `Remove` at the old group immediately followed by an `Upsert` at the new
/// group (retraction never lands after the addition for the same element).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupedDiff<K, T> {
    /// The element `key` now belongs to `group` with value `value`
    /// (a fresh insert, an in-place value change, or the addition half of a
    /// group-key change).
    Upsert {
        group: K,
        key: String,
        value: Arc<T>,
    },
    /// The element `key` no longer belongs to `group` (a removal, or the
    /// retraction half of a group-key change). Carries no value — the value
    /// is already gone; the retained group is the only truth.
    Remove { group: K, key: String },
}

impl<T: Clone + Send + Sync + 'static> LiveData<T> {
    /// Re-group this feed's keyed changelog by an async, fallible derived key.
    ///
    /// The returned stream first drains the feed's initial snapshot (delivered
    /// by `MutableSignalMap` as a `MapDiff::Replace`) into `Upsert`s, then
    /// emits one or two [`GroupedDiff`]s per subsequent change. See the module
    /// docs for the convergence contract and error semantics.
    ///
    /// `key_fn` receives the element value as an `Arc<T>` (cheap to clone and
    /// hold across the `await`, avoiding a borrow tied to the map lock).
    pub fn group_by<K, F, Fut>(&self, key_fn: F) -> impl Stream<Item = Result<GroupedDiff<K, T>>>
    where
        K: Clone + Ord + Send + 'static,
        F: Fn(Arc<T>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<K>> + Send,
    {
        let mut source = self.signal_map();
        let diffs = futures::stream::poll_fn(move |cx| source.poll_map_change_unpin(cx));
        group_diffs(diffs, key_fn)
    }
}

/// State threaded through the `unfold` that turns a `MapDiff` stream into a
/// `GroupedDiff` stream.
struct GroupState<S, K, T, F> {
    source: S,
    /// element key -> last emitted group key. The whole point of the
    /// combinator: it survives `Remove`/`Clear` (value gone, group retained).
    acc: BTreeMap<String, K>,
    /// Ready-to-yield outputs from the current input event (a single event
    /// can produce many — `Replace`/`Clear` retract every retained entry).
    pending: VecDeque<Result<GroupedDiff<K, T>>>,
    key_fn: F,
    /// Set once a `key_fn` error has been queued; after it drains we end.
    errored: bool,
}

/// Core combinator: fold a stream of `MapDiff<String, Arc<T>>` into a stream
/// of `Result<GroupedDiff<K, T>>`. Split out from [`LiveData::group_by`] so it
/// can be driven at the `MapDiff` boundary directly (all five variants,
/// including `Replace`/`Clear` which `MutableBTreeMap` only emits at
/// construction / on `clear`).
fn group_diffs<S, K, T, F, Fut>(
    source: S,
    key_fn: F,
) -> impl Stream<Item = Result<GroupedDiff<K, T>>>
where
    S: Stream<Item = MapDiff<String, Arc<T>>>,
    K: Clone + Ord + Send + 'static,
    T: Send + Sync + 'static,
    F: Fn(Arc<T>) -> Fut,
    Fut: Future<Output = Result<K>>,
{
    let state = GroupState {
        source: Box::pin(source),
        acc: BTreeMap::new(),
        pending: VecDeque::new(),
        key_fn,
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
            if let Err(e) = process_diff(&mut st.acc, &mut st.pending, &st.key_fn, diff).await {
                st.errored = true;
                st.pending.push_back(Err(e));
            }
        }
    })
}

/// Apply one `MapDiff` to the accumulator, queuing the resulting
/// `GroupedDiff`s. Retractions are always queued before the matching
/// addition. `O(delta)` except `Replace`/`Clear`, which are `O(retained)`.
async fn process_diff<K, T, F, Fut>(
    acc: &mut BTreeMap<String, K>,
    pending: &mut VecDeque<Result<GroupedDiff<K, T>>>,
    key_fn: &F,
    diff: MapDiff<String, Arc<T>>,
) -> Result<()>
where
    K: Clone + Ord,
    F: Fn(Arc<T>) -> Fut,
    Fut: Future<Output = Result<K>>,
{
    match diff {
        MapDiff::Replace { entries } => {
            // Retract every retained entry from its last group, then re-seed
            // from the snapshot. No silent state reset.
            for (key, group) in std::mem::take(acc) {
                pending.push_back(Ok(GroupedDiff::Remove { group, key }));
            }
            for (key, value) in entries {
                let group = key_fn(value.clone())
                    .await
                    .with_context(|| format!("group_by key_fn failed for Replace entry {key}"))?;
                acc.insert(key.clone(), group.clone());
                pending.push_back(Ok(GroupedDiff::Upsert { group, key, value }));
            }
        }
        MapDiff::Insert { key, value } => {
            let group = key_fn(value.clone())
                .await
                .with_context(|| format!("group_by key_fn failed for Insert {key}"))?;
            acc.insert(key.clone(), group.clone());
            pending.push_back(Ok(GroupedDiff::Upsert { group, key, value }));
        }
        MapDiff::Update { key, value } => {
            let new_group = key_fn(value.clone())
                .await
                .with_context(|| format!("group_by key_fn failed for Update {key}"))?;
            if let Some(old_group) = acc.get(&key) {
                if *old_group != new_group {
                    // Group changed: retract from the old group FIRST.
                    let old_group = old_group.clone();
                    pending.push_back(Ok(GroupedDiff::Remove {
                        group: old_group,
                        key: key.clone(),
                    }));
                }
            }
            acc.insert(key.clone(), new_group.clone());
            pending.push_back(Ok(GroupedDiff::Upsert {
                group: new_group,
                key,
                value,
            }));
        }
        MapDiff::Remove { key } => {
            // The value is already gone; the retained group is the only truth.
            let group = acc.remove(&key).with_context(|| {
                format!(
                    "group_by Remove for key {key} not in accumulator — feed/accumulator desync"
                )
            })?;
            pending.push_back(Ok(GroupedDiff::Remove { group, key }));
        }
        MapDiff::Clear {} => {
            for (key, group) in std::mem::take(acc) {
                pending.push_back(Ok(GroupedDiff::Remove { group, key }));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
