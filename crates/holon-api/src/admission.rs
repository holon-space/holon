//! Write-order admission: an operation takes its place in the write order when
//! it is admitted, and runs only after every earlier operation whose
//! [`Footprint`] overlaps its own has settled. Operations on disjoint
//! footprints never wait for each other.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use futures::channel::oneshot;

use crate::EntityName;
use crate::StorageEntity;
use crate::entity_uri::EntityUri;
use crate::operation_engine::OpOrigin;

pub type AdmissionSeq = u64;

/// What an operation may write, as far as admission orders it. Over-claiming
/// adds order and never loses a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Footprint {
    /// Writes only these entities of `relation`. Never empty.
    Subjects {
        relation: EntityName,
        subjects: BTreeSet<EntityUri>,
    },
    /// May write any entity of the relation.
    Relation(EntityName),
    /// May write anything, so it runs alone.
    Fence,
}

struct InFlight {
    footprint: Footprint,
    waiting_on: BTreeSet<AdmissionSeq>,
    dependents: BTreeSet<AdmissionSeq>,
    release: Option<oneshot::Sender<()>>,
    /// Its claim was dropped before release. It keeps its place until its
    /// waits settle, so the ops behind it stay ordered after what it waited on.
    abandoned: bool,
}

/// Every edge points from a later admission to an earlier one. Waiting on the
/// latest holder of a subject is enough: a holder runs only after its own
/// waits settled.
#[derive(Default)]
struct OverlayState {
    next: AdmissionSeq,
    inflight: BTreeMap<AdmissionSeq, InFlight>,
    latest_by_subject: HashMap<EntityUri, AdmissionSeq>,
    latest_relation: HashMap<EntityName, AdmissionSeq>,
    by_relation: HashMap<EntityName, BTreeSet<AdmissionSeq>>,
    latest_fence: Option<AdmissionSeq>,
}

impl OverlayState {
    fn admit(&mut self, footprint: Footprint) -> (AdmissionSeq, Option<oneshot::Receiver<()>>) {
        let seq = self.next;
        self.next += 1;
        let mut waits: BTreeSet<AdmissionSeq> = self.latest_fence.into_iter().collect();
        match &footprint {
            Footprint::Fence => {
                waits.extend(self.inflight.keys().copied());
                self.latest_fence = Some(seq);
            }
            Footprint::Subjects { relation, subjects } => {
                assert!(
                    !subjects.is_empty(),
                    "a Subjects footprint on '{relation}' names no subject; use Relation"
                );
                waits.extend(
                    subjects
                        .iter()
                        .filter_map(|s| self.latest_by_subject.get(s).copied()),
                );
                waits.extend(self.latest_relation.get(relation).copied());
                for subject in subjects {
                    self.latest_by_subject.insert(subject.clone(), seq);
                }
                self.by_relation
                    .entry(relation.clone())
                    .or_default()
                    .insert(seq);
            }
            Footprint::Relation(relation) => {
                waits.extend(
                    self.by_relation
                        .get(relation)
                        .into_iter()
                        .flatten()
                        .copied(),
                );
                self.latest_relation.insert(relation.clone(), seq);
                self.by_relation
                    .entry(relation.clone())
                    .or_default()
                    .insert(seq);
            }
        }
        for wait in &waits {
            self.inflight
                .get_mut(wait)
                .unwrap_or_else(|| {
                    panic!("admission {seq} waits on {wait}, which is not in flight")
                })
                .dependents
                .insert(seq);
        }
        let (release, receiver) = if waits.is_empty() {
            (None, None)
        } else {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        };
        self.inflight.insert(
            seq,
            InFlight {
                footprint,
                waiting_on: waits,
                dependents: BTreeSet::new(),
                release,
                abandoned: false,
            },
        );
        (seq, receiver)
    }

    /// The claim `seq` is done: it completed, failed, or was dropped.
    fn settle(&mut self, seq: AdmissionSeq) {
        let entry = self
            .inflight
            .get_mut(&seq)
            .unwrap_or_else(|| panic!("admission {seq} settled but is not in flight"));
        assert!(!entry.abandoned, "admission {seq} settled twice");
        if !entry.waiting_on.is_empty() {
            entry.abandoned = true;
            entry.release = None;
            return;
        }
        let mut ready = vec![seq];
        while let Some(done) = ready.pop() {
            for released in self.remove(done) {
                let entry = self
                    .inflight
                    .get_mut(&released)
                    .expect("a dependent is in flight");
                if entry.abandoned {
                    ready.push(released);
                } else {
                    entry
                        .release
                        .take()
                        .expect("an unreleased claim holds its release sender")
                        .send(())
                        .expect("a live claim holds its release receiver");
                }
            }
        }
    }

    /// Remove a settled op whose waits are all settled; returns, in admission
    /// order, the dependents this leaves with nothing to wait on.
    fn remove(&mut self, seq: AdmissionSeq) -> Vec<AdmissionSeq> {
        let done = self.inflight.remove(&seq).expect("removed op is in flight");
        assert!(
            done.waiting_on.is_empty(),
            "admission {seq} removed while waiting"
        );
        match &done.footprint {
            Footprint::Fence => {
                if self.latest_fence == Some(seq) {
                    self.latest_fence = None;
                }
            }
            Footprint::Subjects { relation, subjects } => {
                for subject in subjects {
                    if self.latest_by_subject.get(subject) == Some(&seq) {
                        self.latest_by_subject.remove(subject);
                    }
                }
                self.remove_from_relation(relation, seq);
            }
            Footprint::Relation(relation) => {
                if self.latest_relation.get(relation) == Some(&seq) {
                    self.latest_relation.remove(relation);
                }
                self.remove_from_relation(relation, seq);
            }
        }
        done.dependents
            .into_iter()
            .filter(|dependent| {
                let entry = self
                    .inflight
                    .get_mut(dependent)
                    .expect("a dependent is in flight");
                entry.waiting_on.remove(&seq);
                entry.waiting_on.is_empty()
            })
            .collect()
    }

    fn remove_from_relation(&mut self, relation: &EntityName, seq: AdmissionSeq) {
        let members = self
            .by_relation
            .get_mut(relation)
            .expect("an op with a relation footprint is listed under it");
        members.remove(&seq);
        if members.is_empty() {
            self.by_relation.remove(relation);
        }
    }
}

/// The in-flight operations of one engine and the order between them.
#[derive(Default)]
pub struct AdmissionOverlay {
    state: Arc<Mutex<OverlayState>>,
}

impl AdmissionOverlay {
    pub fn admit(&self, footprint: Footprint) -> Claim {
        let (seq, release) = self
            .state
            .lock()
            .expect("admission overlay poisoned")
            .admit(footprint);
        Claim {
            seq,
            overlay: Arc::clone(&self.state),
            release,
        }
    }

    pub fn owns(&self, claim: &Claim) -> bool {
        Arc::ptr_eq(&self.state, &claim.overlay)
    }

    pub fn census(&self) -> AdmissionCensus {
        let state = self.state.lock().expect("admission overlay poisoned");
        let live = state.inflight.values().filter(|entry| !entry.abandoned);
        let (waiting, released): (Vec<_>, Vec<_>) =
            live.partition(|entry| !entry.waiting_on.is_empty());
        AdmissionCensus {
            released: released.len(),
            waiting: waiting.len(),
        }
    }
}

/// The live claims of an overlay: `released` may run or are running, `waiting`
/// still wait on an earlier claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionCensus {
    pub released: usize,
    pub waiting: usize,
}

/// One admitted operation's place in the write order. Dropping it settles the
/// place, whether the operation completed, failed, or never ran.
pub struct Claim {
    seq: AdmissionSeq,
    overlay: Arc<Mutex<OverlayState>>,
    release: Option<oneshot::Receiver<()>>,
}

impl Claim {
    pub fn seq(&self) -> AdmissionSeq {
        self.seq
    }

    /// Resolves once every earlier op on an overlapping footprint has settled.
    pub async fn released(&mut self) {
        if let Some(release) = &mut self.release {
            release
                .await
                .expect("the overlay holds the release sender until it releases this claim");
            self.release = None;
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.overlay
            .lock()
            .expect("admission overlay poisoned")
            .settle(self.seq);
    }
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim")
            .field("seq", &self.seq)
            .field("released", &self.release.is_none())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct OpRequest {
    pub entity_name: EntityName,
    pub op_name: String,
    pub params: StorageEntity,
    pub origin: OpOrigin,
}

/// An admitted operation waiting to be run by the engine that admitted it.
#[must_use = "a ticket holds its place in the write order until it is run"]
#[derive(Debug)]
pub struct Ticket {
    inner: Option<(Claim, OpRequest)>,
}

impl Ticket {
    pub fn new(claim: Claim, request: OpRequest) -> Self {
        Self {
            inner: Some((claim, request)),
        }
    }

    pub fn request(&self) -> &OpRequest {
        &self
            .inner
            .as_ref()
            .expect("a live ticket holds its request")
            .1
    }

    pub fn into_parts(mut self) -> (Claim, OpRequest) {
        self.inner.take().expect("a live ticket holds its parts")
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if let Some((claim, request)) = &self.inner {
            tracing::error!(
                seq = claim.seq(),
                "ticket for '{}' on '{}' dropped without being run; the ops queued behind it go on",
                request.op_name,
                request.entity_name
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;

    use super::*;

    fn block() -> EntityName {
        EntityName::new("block")
    }

    fn on(ids: &[&str]) -> Footprint {
        Footprint::Subjects {
            relation: block(),
            subjects: ids.iter().map(|id| EntityUri::block(id)).collect(),
        }
    }

    fn is_released(claim: &mut Claim) -> bool {
        claim.released().now_or_never().is_some()
    }

    #[test]
    fn disjoint_subjects_go_at_once() {
        let overlay = AdmissionOverlay::default();
        let mut a = overlay.admit(on(&["a"]));
        let mut b = overlay.admit(on(&["b"]));
        let mut c = overlay.admit(Footprint::Relation(EntityName::new("document")));
        assert!(is_released(&mut a) && is_released(&mut b) && is_released(&mut c));
    }

    #[test]
    fn census_counts_live_claims_and_skips_abandoned_ones() {
        let overlay = AdmissionOverlay::default();
        let head = overlay.admit(on(&["x"]));
        let behind = overlay.admit(on(&["x"]));
        let _also_behind = overlay.admit(on(&["x"]));
        let _elsewhere = overlay.admit(on(&["y"]));
        assert_eq!(
            overlay.census(),
            AdmissionCensus {
                released: 2,
                waiting: 2
            }
        );
        drop(behind);
        assert_eq!(
            overlay.census(),
            AdmissionCensus {
                released: 2,
                waiting: 1
            }
        );
        drop(head);
        assert_eq!(
            overlay.census(),
            AdmissionCensus {
                released: 2,
                waiting: 0
            }
        );
    }

    #[test]
    fn same_subject_chains_in_admission_order_and_a_settle_releases_exactly_the_next() {
        let overlay = AdmissionOverlay::default();
        let k1 = overlay.admit(on(&["x"]));
        let mut k2 = overlay.admit(on(&["x"]));
        let mut k3 = overlay.admit(on(&["x", "y"]));
        let mut other = overlay.admit(on(&["y"]));
        assert!(!is_released(&mut k2));
        assert!(!is_released(&mut k3));
        assert!(!is_released(&mut other), "y is held by k3");

        drop(k1);
        assert!(is_released(&mut k2));
        assert!(!is_released(&mut k3));
        drop(k2);
        assert!(is_released(&mut k3));
        assert!(!is_released(&mut other));
        drop(k3);
        assert!(is_released(&mut other));
    }

    #[test]
    fn a_fence_waits_for_all_and_all_later_wait_for_it() {
        let overlay = AdmissionOverlay::default();
        let a = overlay.admit(on(&["a"]));
        let b = overlay.admit(Footprint::Relation(EntityName::new("document")));
        let mut fence = overlay.admit(Footprint::Fence);
        let mut later = overlay.admit(on(&["z"]));
        let mut later_fence = overlay.admit(Footprint::Fence);

        drop(a);
        assert!(!is_released(&mut fence));
        drop(b);
        assert!(is_released(&mut fence));
        assert!(!is_released(&mut later));
        drop(fence);
        assert!(is_released(&mut later));
        assert!(!is_released(&mut later_fence));
        drop(later);
        assert!(is_released(&mut later_fence));
    }

    #[test]
    fn relation_and_subjects_of_the_same_relation_order_both_ways() {
        let overlay = AdmissionOverlay::default();
        let a = overlay.admit(on(&["a"]));
        let b = overlay.admit(on(&["b"]));
        let mut whole = overlay.admit(Footprint::Relation(block()));
        let mut after = overlay.admit(on(&["c"]));
        let mut elsewhere = overlay.admit(on(&["c"]));
        let mut other_relation = overlay.admit(Footprint::Relation(EntityName::new("document")));
        assert!(is_released(&mut other_relation));

        drop(a);
        assert!(!is_released(&mut whole));
        drop(b);
        assert!(is_released(&mut whole));
        assert!(!is_released(&mut after));
        drop(whole);
        assert!(is_released(&mut after));
        assert!(!is_released(&mut elsewhere));
    }

    #[test]
    fn a_dropped_waiting_claim_keeps_its_place_then_releases_its_dependents() {
        let overlay = AdmissionOverlay::default();
        let running = overlay.admit(on(&["x"]));
        let waiting = overlay.admit(on(&["x"]));
        let mut behind = overlay.admit(on(&["x"]));
        drop(waiting);
        assert!(
            !is_released(&mut behind),
            "behind stays ordered after running"
        );
        let mut newcomer = overlay.admit(on(&["x"]));
        drop(running);
        assert!(is_released(&mut behind));
        assert!(!is_released(&mut newcomer));
        drop(behind);
        assert!(is_released(&mut newcomer));
        drop(newcomer);
        let state = overlay.state.lock().unwrap();
        assert!(state.inflight.is_empty() && state.latest_by_subject.is_empty());
        assert!(state.by_relation.is_empty() && state.latest_fence.is_none());
    }

    #[test]
    fn a_ticket_dropped_without_run_releases_the_ops_behind_it() {
        let overlay = AdmissionOverlay::default();
        let request = OpRequest {
            entity_name: block(),
            op_name: "set_field".into(),
            params: StorageEntity::default(),
            origin: OpOrigin::User,
        };
        let ticket = Ticket::new(overlay.admit(on(&["x"])), request);
        let mut behind = overlay.admit(on(&["x"]));
        drop(ticket);
        assert!(is_released(&mut behind));
    }

    #[test]
    #[should_panic(expected = "settled twice")]
    fn settle_twice_panics() {
        let mut state = OverlayState::default();
        let (first, _) = state.admit(on(&["x"]));
        let (second, _release) = state.admit(on(&["x"]));
        state.settle(second);
        state.settle(second);
        state.settle(first);
    }
}
