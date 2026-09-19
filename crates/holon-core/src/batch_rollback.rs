//! Taking back a multi-op batch the block write authority partly applied.
//!
//! A caller that dispatches N operations one by one holds no transaction over
//! them: op K failing leaves ops `0..K` committed. Where the authority is a
//! CRDT it can be carried back to the version the batch started from, and this
//! is the seam that offers it — `None` from
//! [`OperationProvider::batch_rollback`](crate::OperationProvider::batch_rollback)
//! means this session's authority cannot, and the caller owes its user a
//! partial-apply disclosure instead.
//!
//! The rollback is a whole-document rewind, so it is sound only while the
//! window holds nothing but the batch's own ops. [`BatchWindow`] is what
//! establishes that, and what it can and cannot establish is stated on it.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use async_trait::async_trait;

/// One document's version at an instant: the ops it has seen from each peer,
/// and the frontier to return to.
///
/// The frontier is recorded, not derived from `counters`: a history trim in
/// between makes the two disagree, and a derived target is then a version the
/// caller never asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocVersion {
    pub local_peer: u64,
    pub counters: BTreeMap<u64, i64>,
    pub frontier: Vec<(u64, i64)>,
}

/// The first peer whose op count grew between `before` and `after`. A `before`
/// of `None` is a document that did not exist when the window opened, so every
/// op it holds is an advance.
fn advance(before: Option<&DocVersion>, after: &DocVersion) -> Option<(u64, i64)> {
    after.counters.iter().find_map(|(peer, count)| {
        let seen = before
            .and_then(|b| b.counters.get(peer))
            .copied()
            .unwrap_or(0);
        (seen < *count).then_some((*peer, count - seen))
    })
}

/// Every document the write authority can route a block write to, versioned at
/// one instant.
///
/// All of them, not just the vault document: a block write routes to the
/// device-local layout document and to shared-subtree documents too, and a
/// rollback that measured one document would report a full rewind while
/// leaving the others where the failed batch put them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityVersion {
    docs: BTreeMap<String, DocVersion>,
}

impl AuthorityVersion {
    pub fn new(docs: BTreeMap<String, DocVersion>) -> Self {
        Self { docs }
    }

    pub fn docs(&self) -> &BTreeMap<String, DocVersion> {
        &self.docs
    }

    /// A document present here and gone from `later` left the authority's
    /// routing set, so its ops can no longer be taken back.
    fn vanished_in(&self, later: &Self) -> Option<&str> {
        self.docs
            .keys()
            .find(|name| !later.docs.contains_key(*name))
            .map(String::as_str)
    }

    /// The first write by a peer other than the document's own that landed
    /// between this version and `later`.
    ///
    /// A remote peer is attributable whenever it is measured, so this check
    /// spans the whole window rather than one op boundary.
    pub fn remote_advance_over(&self, later: &Self) -> Option<RollbackRefused> {
        if let Some(doc) = self.vanished_in(later) {
            return Some(RollbackRefused::Unreachable {
                doc: doc.to_string(),
                detail: "the document left the write authority's routing set inside the batch \
                         window"
                    .to_string(),
            });
        }
        self.each_doc(later).find_map(|(name, before, after)| {
            let mut remote = after.clone();
            remote.counters.remove(&after.local_peer);
            advance(before, &remote).map(|(peer, ops)| RollbackRefused::PeerWroteInsideWindow {
                doc: name.to_string(),
                peer,
                ops,
            })
        })
    }

    /// The first write of ANY origin that landed between this version and
    /// `later`. Meaningful only across an interval in which the batch itself
    /// wrote nothing.
    pub fn any_advance_over(&self, later: &Self) -> Option<RollbackRefused> {
        if let Some(refusal) = self.remote_advance_over(later) {
            return Some(refusal);
        }
        self.each_doc(later).find_map(|(name, before, after)| {
            advance(before, after).map(|(_, ops)| RollbackRefused::LocalWroteInsideWindow {
                doc: name.to_string(),
                ops,
            })
        })
    }

    /// Every document `later` knows, paired with this version's reading of it.
    fn each_doc<'a>(
        &'a self,
        later: &'a Self,
    ) -> impl Iterator<Item = (&'a str, Option<&'a DocVersion>, &'a DocVersion)> {
        let names: BTreeSet<&str> = later.docs.keys().map(String::as_str).collect();
        names.into_iter().filter_map(move |name| {
            let after = later.docs.get(name)?;
            Some((name, self.docs.get(name), after))
        })
    }
}

/// The span a multi-op batch is rolled back across, and the evidence that
/// rolling it back destroys nothing else.
///
/// # What it establishes
///
/// A remote peer's write is attributable whenever it is measured, so one is
/// refused wherever in the window it landed. A LOCAL write carries no batch
/// identity — the authority attributes ops to a peer, not to a caller — so it
/// is separable only across an interval in which the batch itself wrote
/// nothing. [`Self::observe_between_ops`] is that interval, and a local write
/// found there poisons the window for good.
///
/// # What it does not
///
/// A local write that commits while one of the batch's own ops is in flight
/// falls inside that op's interval and is attributed to the batch. Closing
/// that needs either a lock held across the whole batch (which stalls the
/// editor) or a batch identity carried on every write, neither of which this
/// seam has.
#[derive(Debug, Clone)]
pub struct BatchWindow {
    start: AuthorityVersion,
    accounted: AuthorityVersion,
    intruder: Option<RollbackRefused>,
}

impl BatchWindow {
    pub fn opened(start: AuthorityVersion) -> Self {
        Self {
            accounted: start.clone(),
            start,
            intruder: None,
        }
    }

    pub fn start(&self) -> &AuthorityVersion {
        &self.start
    }

    /// The refusal a rollback owes, if the window is already known dirty.
    pub fn intruder(&self) -> Option<&RollbackRefused> {
        self.intruder.as_ref()
    }

    /// The authority as it looks with no op of the batch in flight: any
    /// advance since the batch last accounted for it came from someone else.
    ///
    /// Remembered rather than raised — the batch is still healthy and must be
    /// allowed to finish; it is the ROLLBACK that can no longer be proven
    /// safe.
    pub fn observe_between_ops(&mut self, now: AuthorityVersion) {
        if self.intruder.is_none() {
            self.intruder = self.accounted.any_advance_over(&now);
        }
        self.accounted = now;
    }

    /// The authority after one of the batch's own ops, whose advance is the
    /// batch's by construction.
    pub fn absorb(&mut self, now: AuthorityVersion) {
        self.accounted = now;
    }
}

/// A rollback the authority did not complete.
///
/// Every variant but [`Self::Incomplete`] is decided before anything is
/// reverted, so the store is exactly as the failed batch left it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RollbackRefused {
    #[error(
        "peer {peer} committed {ops} op(s) into {doc} inside the batch window, and the rollback \
         would destroy them"
    )]
    PeerWroteInsideWindow { doc: String, peer: u64, ops: i64 },
    #[error(
        "{ops} op(s) this session did not dispatch as part of the batch landed in {doc} inside \
         the window, and the rollback would destroy them"
    )]
    LocalWroteInsideWindow { doc: String, ops: i64 },
    #[error("{doc}'s history no longer reaches back to the pre-batch version")]
    HistoryTrimmed { doc: String },
    #[error("{doc} could not be reached to roll it back: {detail}")]
    Unreachable { doc: String, detail: String },
    #[error(
        "the rollback reverted {reverted:?} and then could not revert {doc}: {detail} — the \
         store is in neither the pre-batch nor the post-batch state"
    )]
    Incomplete {
        reverted: Vec<String>,
        doc: String,
        detail: String,
    },
}

/// Carrying the block write authority back to a version a batch started from.
#[async_trait]
pub trait BatchRollback: Send + Sync {
    /// Every routable document's version right now.
    async fn observe(&self) -> crate::Result<AuthorityVersion>;

    /// Undo everything the authority recorded after `window.start()`, or
    /// refuse.
    async fn rollback_to(&self, window: &BatchWindow) -> Result<(), RollbackRefused>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: u64 = 1;

    fn doc(counters: &[(u64, i64)]) -> DocVersion {
        DocVersion {
            local_peer: OURS,
            counters: counters.iter().copied().collect(),
            frontier: vec![(OURS, 1)],
        }
    }

    fn version(docs: &[(&str, DocVersion)]) -> AuthorityVersion {
        AuthorityVersion::new(
            docs.iter()
                .map(|(name, v)| ((*name).to_string(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn a_window_holding_only_our_own_ops_is_rollable() {
        let mut window = BatchWindow::opened(version(&[("global", doc(&[(OURS, 4)]))]));
        window.absorb(version(&[("global", doc(&[(OURS, 9)]))]));
        window.observe_between_ops(version(&[("global", doc(&[(OURS, 9)]))]));
        assert!(window.intruder().is_none());
    }

    #[test]
    fn a_local_write_between_two_ops_poisons_the_window() {
        let mut window = BatchWindow::opened(version(&[("global", doc(&[(OURS, 4)]))]));
        window.absorb(version(&[("global", doc(&[(OURS, 6)]))]));
        window.observe_between_ops(version(&[("global", doc(&[(OURS, 8)]))]));
        assert_eq!(
            window.intruder(),
            Some(&RollbackRefused::LocalWroteInsideWindow {
                doc: "global".to_string(),
                ops: 2,
            })
        );
    }

    #[test]
    fn the_first_intruder_is_the_one_reported() {
        let mut window = BatchWindow::opened(version(&[("global", doc(&[(OURS, 4)]))]));
        window.observe_between_ops(version(&[("global", doc(&[(OURS, 5)]))]));
        window.observe_between_ops(version(&[("global", doc(&[(OURS, 99)]))]));
        assert_eq!(
            window.intruder(),
            Some(&RollbackRefused::LocalWroteInsideWindow {
                doc: "global".to_string(),
                ops: 1,
            })
        );
    }

    #[test]
    fn a_remote_write_is_refused_wherever_in_the_window_it_landed() {
        let start = version(&[("global", doc(&[(OURS, 4)]))]);
        let now = version(&[("global", doc(&[(OURS, 20), (2, 3)]))]);
        assert_eq!(
            start.remote_advance_over(&now),
            Some(RollbackRefused::PeerWroteInsideWindow {
                doc: "global".to_string(),
                peer: 2,
                ops: 3,
            })
        );
    }

    #[test]
    fn a_write_into_another_routable_doc_is_measured_too() {
        let start = version(&[("global", doc(&[(OURS, 4)])), ("layout", doc(&[(OURS, 2)]))]);
        let now = version(&[("global", doc(&[(OURS, 4)])), ("layout", doc(&[(OURS, 5)]))]);
        assert_eq!(
            start.any_advance_over(&now),
            Some(RollbackRefused::LocalWroteInsideWindow {
                doc: "layout".to_string(),
                ops: 3,
            })
        );
    }

    #[test]
    fn a_doc_that_appeared_inside_the_window_counts_every_op_as_an_advance() {
        let start = version(&[("global", doc(&[(OURS, 4)]))]);
        let now = version(&[
            ("global", doc(&[(OURS, 4)])),
            ("shared:abc", doc(&[(OURS, 2)])),
        ]);
        assert_eq!(
            start.any_advance_over(&now),
            Some(RollbackRefused::LocalWroteInsideWindow {
                doc: "shared:abc".to_string(),
                ops: 2,
            })
        );
    }

    #[test]
    fn a_doc_that_left_the_routing_set_cannot_be_rolled_back() {
        let start = version(&[
            ("global", doc(&[(OURS, 4)])),
            ("shared:abc", doc(&[(OURS, 2)])),
        ]);
        let now = version(&[("global", doc(&[(OURS, 4)]))]);
        assert!(matches!(
            start.remote_advance_over(&now),
            Some(RollbackRefused::Unreachable { doc, .. }) if doc == "shared:abc"
        ));
    }
}
