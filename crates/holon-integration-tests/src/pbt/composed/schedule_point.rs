//! Where a masked transition can be suspended between two of its dispatches,
//! and the seeded scheduler that decides whether to suspend there.
//!
//! A driver that dispatches N intents in a row
//! ([`super::super::frontend_slice`]'s keystroke loops) calls
//! [`schedule_point`] after each one. Unarmed, and for any kind the mask does
//! not name, the task-local is unset and the call returns without touching
//! anything.
//!
//! The scheduler cannot wait for a boundary itself: the observables belong to
//! the slice's handle, which the driver has no access to. It sends the request
//! to the harness (which owns the handle) and awaits the answer, so the next
//! dispatch cannot happen until the boundary the seed chose has been observed.

use std::cell::Cell;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use tokio::sync::mpsc;
use tokio::sync::oneshot;

use super::boundary::Boundary;
use super::boundary::BoundaryOutcome;
use super::boundary::Resume;
use super::interleave::InterleavePlan;

/// One request from a schedule point to the harness that services it.
pub struct BoundaryRequest {
    pub boundary: Boundary,
    pub reply: oneshot::Sender<BoundaryOutcome>,
}

/// What one slot's predicate asked for and what came back — the record the
/// conformance oracle judges.
#[derive(Clone, Debug)]
pub struct SlotRecord {
    pub slot: u64,
    pub resume: Resume,
    pub outcome: Option<BoundaryOutcome>,
}

/// The seeded schedule for ONE masked transition, plus where it has got to.
pub struct Scheduler {
    plan: InterleavePlan,
    next_slot: Cell<u64>,
    records: RefCell<Vec<SlotRecord>>,
    requests: mpsc::UnboundedSender<BoundaryRequest>,
}

impl Scheduler {
    pub fn new(plan: InterleavePlan, requests: mpsc::UnboundedSender<BoundaryRequest>) -> Rc<Self> {
        Rc::new(Self {
            plan,
            next_slot: Cell::new(0),
            records: RefCell::new(Vec::new()),
            requests,
        })
    }

    /// Slots that have been reached. Zero on a transition whose driver has no
    /// schedule point, which the harness reports rather than passing off as a
    /// schedule that was honoured.
    pub fn reached(&self) -> u64 {
        self.next_slot.get()
    }

    pub fn records(&self) -> Vec<SlotRecord> {
        self.records.borrow().clone()
    }

    async fn take_slot(&self) {
        let slot = self.next_slot.get();
        self.next_slot.set(slot + 1);
        let resume = self.plan.resume_at(slot);
        let Resume::Wait(boundary) = resume else {
            self.records.borrow_mut().push(SlotRecord {
                slot,
                resume,
                outcome: None,
            });
            return;
        };
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(BoundaryRequest { boundary, reply })
            .unwrap_or_else(|_| {
                panic!(
                    "[schedule-point] slot {slot} asked for {boundary:?} but the harness is no \
                     longer servicing requests — the transition would silently run an unscheduled \
                     dispatch"
                )
            });
        let outcome = answer.await.unwrap_or_else(|e| {
            panic!("[schedule-point] slot {slot} got no answer for {boundary:?}: {e}")
        });
        self.records.borrow_mut().push(SlotRecord {
            slot,
            resume,
            outcome: Some(outcome),
        });
    }
}

tokio::task_local! {
    static SCHEDULER: Rc<Scheduler>;
}

/// Suspend here if this transition's schedule says to.
///
/// Call after each dispatch in a driver that emits several per transition. A
/// no-op unless the harness has armed this transition.
pub async fn schedule_point() {
    let scheduler = SCHEDULER.try_with(Rc::clone).ok();
    if let Some(scheduler) = scheduler {
        scheduler.take_slot().await;
    }
}

/// Run `body` with `scheduler` visible to every [`schedule_point`] it reaches.
pub async fn with_scheduler<F: Future>(scheduler: Rc<Scheduler>, body: F) -> F::Output {
    SCHEDULER.scope(scheduler, body).await
}

/// `inv-schedule-conformance`: every slot that asked to wait got what it asked
/// for, before the next dispatch went out.
///
/// The wait is causally enforced — [`Scheduler::take_slot`] does not return
/// until the answer arrives — so this checks the answer, which is the part a
/// regression could break silently. A slot whose boundary was `Unobservable` is
/// exempt AND must carry its disclosed reason; a wedge never reaches here
/// because the harness panics on it.
pub fn assert_conformance(kind: &str, seed: u64, records: &[SlotRecord]) -> usize {
    let mut checked = 0;
    for record in records {
        let Resume::Wait(boundary) = record.resume else {
            continue;
        };
        let outcome = record.outcome.as_ref().unwrap_or_else(|| {
            panic!(
                "[inv-schedule-conformance] '{kind}' (seed {seed}) slot {} asked for {boundary:?} \
                 and recorded no answer — the dispatch after it ran unscheduled",
                record.slot
            )
        });
        match outcome {
            BoundaryOutcome::Observed(evidence) => {
                assert_eq!(
                    evidence.boundary, boundary,
                    "[inv-schedule-conformance] '{kind}' (seed {seed}) slot {} asked for \
                     {boundary:?} and was told about {:?}",
                    record.slot, evidence.boundary,
                );
                checked += 1;
            }
            BoundaryOutcome::Unobservable(reason) => assert!(
                !reason.trim().is_empty(),
                "[inv-schedule-conformance] '{kind}' (seed {seed}) slot {} refused {boundary:?} \
                 without disclosing why",
                record.slot,
            ),
            BoundaryOutcome::TimedOutWithPendingWork { .. } => unreachable!(
                "a wedge is raised where it is observed, not carried to the oracle: {outcome:?}"
            ),
        }
    }
    checked
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::boundary::BoundaryEvidence;
    use super::super::interleave::Shape;
    use super::*;

    fn plan(shape: Shape) -> InterleavePlan {
        InterleavePlan { seed: 7, shape }
    }

    fn observed(boundary: Boundary) -> BoundaryOutcome {
        BoundaryOutcome::Observed(BoundaryEvidence {
            boundary,
            waited: Duration::from_millis(1),
            detail: "settled 0 -> 1".to_string(),
        })
    }

    /// A burst schedule reaches its slots without ever asking the harness for a
    /// boundary, so an armed run under it behaves as the unarmed one did.
    #[tokio::test]
    async fn a_burst_schedule_sends_no_requests() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scheduler = Scheduler::new(plan(Shape::Burst), tx);
        with_scheduler(Rc::clone(&scheduler), async {
            for _ in 0..4 {
                schedule_point().await;
            }
        })
        .await;

        assert_eq!(scheduler.reached(), 4);
        assert!(rx.try_recv().is_err(), "burst asked the harness to wait");
        assert_eq!(assert_conformance("TypeChars", 7, &scheduler.records()), 0);
    }

    /// Every serial slot blocks on its answer: the dispatch after a schedule
    /// point cannot proceed until the harness replies.
    #[tokio::test]
    async fn a_serial_slot_blocks_until_the_harness_answers() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scheduler = Scheduler::new(plan(Shape::Serial), tx);
        let reached_before_answer = Cell::new(true);

        let driver = with_scheduler(Rc::clone(&scheduler), async {
            schedule_point().await;
            reached_before_answer.set(false);
        });
        let harness = async {
            let request = rx.recv().await.expect("the slot must ask for its boundary");
            assert_eq!(request.boundary, Boundary::AfterIntents(1));
            assert!(
                reached_before_answer.get(),
                "the driver ran past the schedule point before the boundary was answered"
            );
            request
                .reply
                .send(observed(Boundary::AfterIntents(1)))
                .expect("the slot is waiting for this");
        };
        tokio::join!(driver, harness);

        assert_eq!(assert_conformance("TypeChars", 7, &scheduler.records()), 1);
    }

    #[test]
    fn a_slot_answered_with_the_wrong_boundary_is_a_conformance_failure() {
        let records = vec![SlotRecord {
            slot: 0,
            resume: Resume::Wait(Boundary::AfterCdcBatch),
            outcome: Some(observed(Boundary::AfterIntents(1))),
        }];

        let panic = std::panic::catch_unwind(|| assert_conformance("TypeChars", 7, &records));
        assert!(panic.is_err(), "a mismatched answer must not pass");
    }

    #[test]
    fn a_slot_that_never_got_an_answer_is_a_conformance_failure() {
        let records = vec![SlotRecord {
            slot: 3,
            resume: Resume::Wait(Boundary::AfterIntents(1)),
            outcome: None,
        }];

        let panic = std::panic::catch_unwind(|| assert_conformance("TypeChars", 7, &records));
        assert!(panic.is_err(), "an unanswered wait must not pass");
    }

    /// Outside a scope there is no scheduler, so an unarmed driver runs its
    /// dispatches back to back.
    #[tokio::test]
    async fn a_schedule_point_outside_a_scope_does_nothing() {
        schedule_point().await;
    }
}
