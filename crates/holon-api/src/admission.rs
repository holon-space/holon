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
    /// The running claim this one is nested under.
    parent: Option<AdmissionSeq>,
    /// The live claims nested under this one; each settles before it.
    nested: BTreeSet<AdmissionSeq>,
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
                parent: None,
                nested: BTreeSet::new(),
            },
        );
        (seq, receiver)
    }

    /// See [`ClaimRef::nest`].
    fn nest(
        &mut self,
        parent: AdmissionSeq,
        footprint: Footprint,
    ) -> Result<Option<(AdmissionSeq, Option<oneshot::Receiver<()>>)>, NestRefused> {
        let holder = self
            .inflight
            .get(&parent)
            .unwrap_or_else(|| panic!("a claim nests under admission {parent}, not in flight"));
        assert!(
            holder.waiting_on.is_empty() && !holder.abandoned,
            "a claim nests under admission {parent}, which is not running"
        );
        if holder.footprint.covers(&footprint) {
            return Ok(None);
        }
        let waits = self.nested_waits(parent, &footprint)?;
        let seq = self.next;
        self.next += 1;
        let Footprint::Subjects { relation, .. } = &footprint else {
            unreachable!("nested_waits takes only Subjects");
        };
        self.by_relation
            .entry(relation.clone())
            .or_default()
            .insert(seq);
        self.inflight
            .get_mut(&parent)
            .expect("the parent is in flight")
            .nested
            .insert(seq);
        self.inflight.insert(
            seq,
            InFlight {
                footprint: footprint.clone(),
                waiting_on: BTreeSet::new(),
                dependents: BTreeSet::new(),
                release: None,
                abandoned: false,
                parent: Some(parent),
                nested: BTreeSet::new(),
            },
        );
        let receiver = self.hold_nested(seq, parent, footprint, waits);
        Ok(Some((seq, receiver)))
    }

    /// See [`Claim::grow`].
    fn grow(
        &mut self,
        seq: AdmissionSeq,
        footprint: Footprint,
    ) -> Result<Option<oneshot::Receiver<()>>, NestRefused> {
        let entry = self
            .inflight
            .get(&seq)
            .unwrap_or_else(|| panic!("admission {seq} grows but is not in flight"));
        let parent = entry
            .parent
            .unwrap_or_else(|| panic!("admission {seq} grows but is not a nested claim"));
        assert!(
            entry.waiting_on.is_empty() && !entry.abandoned,
            "admission {seq} grows but is not running"
        );
        assert!(
            footprint.covers(&entry.footprint),
            "admission {seq} grows from {:?} to {footprint:?}, which drops part of it",
            entry.footprint
        );
        let waits = self.nested_waits(parent, &footprint)?;
        Ok(self.hold_nested(seq, parent, footprint, waits))
    }

    /// What a claim nested under `parent` on `footprint` waits on: every
    /// in-flight claim on an overlapping footprint, except the write's own
    /// claims and those that wait on them. Refused when one of those waits
    /// cannot settle before `parent` does.
    fn nested_waits(
        &self,
        parent: AdmissionSeq,
        footprint: &Footprint,
    ) -> Result<BTreeSet<AdmissionSeq>, NestRefused> {
        assert!(
            matches!(footprint, Footprint::Subjects { .. }),
            "a nested claim names its subjects, got {footprint:?}"
        );
        let own = self.own(parent);
        let after = self.downstream(&own);
        let waits: BTreeSet<AdmissionSeq> = self
            .inflight
            .iter()
            .filter(|(seq, entry)| {
                !own.contains(seq) && !after.contains(seq) && overlaps(&entry.footprint, footprint)
            })
            .map(|(seq, _)| *seq)
            .collect();
        if let Some(through) = waits.iter().find(|w| self.reaches(**w, parent)) {
            return Err(NestRefused {
                parent,
                through: *through,
            });
        }
        Ok(waits)
    }

    /// `parent` and every claim nested under it.
    fn own(&self, parent: AdmissionSeq) -> BTreeSet<AdmissionSeq> {
        std::iter::once(parent)
            .chain(self.inflight[&parent].nested.iter().copied())
            .collect()
    }

    /// Give the nested claim `seq` its `footprint` and `waits`. It becomes the
    /// latest holder of each subject no claim queued behind the write holds.
    fn hold_nested(
        &mut self,
        seq: AdmissionSeq,
        parent: AdmissionSeq,
        footprint: Footprint,
        waits: BTreeSet<AdmissionSeq>,
    ) -> Option<oneshot::Receiver<()>> {
        let own = self.own(parent);
        let after = self.downstream(&own);
        let Footprint::Subjects { subjects, .. } = &footprint else {
            unreachable!("nested_waits takes only Subjects");
        };
        for subject in subjects {
            let behind_the_write = self
                .latest_by_subject
                .get(subject)
                .is_some_and(|latest| own.contains(latest) || after.contains(latest));
            if !behind_the_write {
                self.latest_by_subject.insert(subject.clone(), seq);
            }
        }
        for wait in &waits {
            self.inflight
                .get_mut(wait)
                .expect("a waited-on claim is in flight")
                .dependents
                .insert(seq);
        }
        let (release, receiver) = if waits.is_empty() {
            (None, None)
        } else {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        };
        let entry = self
            .inflight
            .get_mut(&seq)
            .expect("the nested claim is in flight");
        entry.footprint = footprint;
        entry.waiting_on = waits;
        entry.release = release;
        receiver
    }

    /// Every claim that waits, directly or through others, on one of `roots`.
    fn downstream(&self, roots: &BTreeSet<AdmissionSeq>) -> BTreeSet<AdmissionSeq> {
        let mut seen = BTreeSet::new();
        let mut frontier: Vec<AdmissionSeq> = roots.iter().copied().collect();
        while let Some(seq) = frontier.pop() {
            for dependent in &self.inflight[&seq].dependents {
                if seen.insert(*dependent) {
                    frontier.push(*dependent);
                }
            }
        }
        seen
    }

    /// Whether `from` cannot settle before `target` does: it waits on it, or
    /// a claim nested under it does, directly or through others.
    fn reaches(&self, from: AdmissionSeq, target: AdmissionSeq) -> bool {
        let mut seen = BTreeSet::new();
        let mut frontier = vec![from];
        while let Some(seq) = frontier.pop() {
            if seq == target {
                return true;
            }
            if !seen.insert(seq) {
                continue;
            }
            if let Some(entry) = self.inflight.get(&seq) {
                frontier.extend(entry.waiting_on.iter().copied());
                frontier.extend(entry.nested.iter().copied());
            }
        }
        false
    }

    /// The claim `seq` is done: it completed, failed, or was dropped.
    fn settle(&mut self, seq: AdmissionSeq) {
        let entry = self
            .inflight
            .get_mut(&seq)
            .unwrap_or_else(|| panic!("admission {seq} settled but is not in flight"));
        assert!(!entry.abandoned, "admission {seq} settled twice");
        assert!(
            entry.nested.is_empty(),
            "admission {seq} settled before its nested claim(s) {:?}",
            entry.nested
        );
        if let Some(parent) = entry.parent {
            self.inflight
                .get_mut(&parent)
                .expect("a nested claim settles before its parent")
                .nested
                .remove(&seq);
        }
        let entry = self.inflight.get_mut(&seq).expect("checked above");
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

impl Footprint {
    /// The smallest footprint that covers both: one relation's subjects merge,
    /// two relations take the fence.
    pub fn union(self, other: Footprint) -> Footprint {
        match (self, other) {
            (Footprint::Fence, _) | (_, Footprint::Fence) => Footprint::Fence,
            (
                Footprint::Subjects {
                    relation,
                    mut subjects,
                },
                Footprint::Subjects {
                    relation: r,
                    subjects: more,
                },
            ) if relation == r => {
                subjects.extend(more);
                Footprint::Subjects { relation, subjects }
            }
            (Footprint::Relation(relation), Footprint::Relation(r))
            | (Footprint::Relation(relation), Footprint::Subjects { relation: r, .. })
            | (Footprint::Subjects { relation: r, .. }, Footprint::Relation(relation))
                if relation == r =>
            {
                Footprint::Relation(relation)
            }
            _ => Footprint::Fence,
        }
    }

    /// Whether its holder may write everything `inner` names.
    pub fn covers(&self, inner: &Footprint) -> bool {
        match (self, inner) {
            (Footprint::Fence, _) => true,
            (_, Footprint::Fence) => false,
            (Footprint::Relation(r), Footprint::Relation(i))
            | (Footprint::Relation(r), Footprint::Subjects { relation: i, .. }) => r == i,
            (Footprint::Subjects { .. }, Footprint::Relation(_)) => false,
            (
                Footprint::Subjects {
                    relation: r,
                    subjects: outer,
                },
                Footprint::Subjects {
                    relation: i,
                    subjects: inner,
                },
            ) => r == i && inner.is_subset(outer),
        }
    }
}

fn overlaps(a: &Footprint, b: &Footprint) -> bool {
    match (a, b) {
        (Footprint::Fence, _) | (_, Footprint::Fence) => true,
        (Footprint::Relation(r), Footprint::Relation(i))
        | (Footprint::Relation(r), Footprint::Subjects { relation: i, .. })
        | (Footprint::Subjects { relation: r, .. }, Footprint::Relation(i)) => r == i,
        (
            Footprint::Subjects {
                relation: r,
                subjects: a,
            },
            Footprint::Subjects {
                relation: i,
                subjects: b,
            },
        ) => r == i && !a.is_disjoint(b),
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

impl Claim {
    /// A handle a running write nests claims under while this claim is held.
    pub fn handle(&self) -> ClaimRef {
        ClaimRef {
            seq: self.seq,
            overlay: Arc::clone(&self.overlay),
        }
    }

    /// Grow this running nested claim to `footprint`, which covers what it
    /// claims now. It keeps its place, so every claim queued behind it stays
    /// queued; it waits, as [`ClaimRef::nest`] would, on the other claims the
    /// growth overlaps. On refusal the claim is unchanged.
    pub fn grow(&mut self, footprint: Footprint) -> Result<(), NestRefused> {
        assert!(
            self.release.is_none(),
            "admission {} grows before it was released",
            self.seq
        );
        self.release = self
            .overlay
            .lock()
            .expect("admission overlay poisoned")
            .grow(self.seq, footprint)?;
        Ok(())
    }
}

/// The claim of a running write, as the code it runs sees it.
#[derive(Clone)]
pub struct ClaimRef {
    seq: AdmissionSeq,
    overlay: Arc<Mutex<OverlayState>>,
}

/// A nested claim that would close a wait cycle: `through` (or a claim nested
/// under it) waits on `parent`, and the nest would make `parent` wait on
/// `through`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "the nested claim of admission {parent} would wait on admission {through}, which waits on it"
)]
pub struct NestRefused {
    pub parent: AdmissionSeq,
    pub through: AdmissionSeq,
}

impl ClaimRef {
    pub fn seq(&self) -> AdmissionSeq {
        self.seq
    }

    /// Claim `footprint` too, from inside the running write: for a write that
    /// learns what it reaches only while it runs. `None` when this claim
    /// already covers it.
    ///
    /// The nested claim waits on every in-flight claim on an overlapping
    /// footprint except those that run after this write anyway, which it
    /// would deadlock on. So ordering holds per footprint at nest time: a
    /// claim admitted after this write that does not overlap it may run
    /// before the nested claim, and is judged against the state it finds.
    /// The nested claim settles before this one.
    pub fn nest(&self, footprint: Footprint) -> Result<Option<Claim>, NestRefused> {
        let nested = self
            .overlay
            .lock()
            .expect("admission overlay poisoned")
            .nest(self.seq, footprint)?;
        Ok(nested.map(|(seq, release)| Claim {
            seq,
            overlay: Arc::clone(&self.overlay),
            release,
        }))
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
#[path = "admission_nest_tests.rs"]
mod nest_tests;

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

    #[test]
    fn the_union_covers_both_footprints_and_no_more_than_their_relations() {
        let doc = Footprint::Relation(EntityName::new("document"));
        let cases = [
            (on(&["x"]), on(&["y"]), on(&["x", "y"])),
            (
                on(&["x"]),
                Footprint::Relation(block()),
                Footprint::Relation(block()),
            ),
            (
                Footprint::Relation(block()),
                on(&["x"]),
                Footprint::Relation(block()),
            ),
            (on(&["x"]), doc.clone(), Footprint::Fence),
            (Footprint::Fence, on(&["x"]), Footprint::Fence),
            (doc.clone(), doc.clone(), doc),
        ];
        for (a, b, union) in cases {
            let got = a.clone().union(b.clone());
            assert_eq!(got, union, "{a:?} ∪ {b:?}");
            assert!(
                got.covers(&a) && got.covers(&b),
                "{got:?} covers {a:?}, {b:?}"
            );
        }
        assert!(!on(&["x", "y"]).covers(&on(&["z"])));
    }
}
